//! The SNSS container: file header, command framing, torn-tail detection, and
//! the `base::Pickle` reader that variable-length commands are encoded with.
//!
//! slice: capture
//! why: Chrome's session log has no checksum, no trailer and no length field.
//!      The only structural facts a reader can rely on are the eight-byte
//!      header and "each command says how long it is", so everything here is
//!      bounded by the bytes actually present. A half-written command at the
//!      end is reported as a torn tail rather than an error, because Chrome
//!      may be mid-append at the instant we read. This module knows nothing
//!      about what commands mean; that belongs to the per-file-type tables.

use std::fmt;

/// The four bytes every session file starts with.
pub const MAGIC: [u8; 4] = *b"SNSS";
/// `kFileVersionWithMarker`: the cleartext format this slice reads.
pub const VERSION_CLEARTEXT: u32 = 3;
/// `kFileVersionEncryptedWithOSCrypt`: readable framing, unreadable commands.
pub const VERSION_ENCRYPTED: u32 = 5;
/// Magic plus a little-endian `int32` version.
pub const HEADER_LEN: usize = 8;

/// The only conditions that stop a parse before any command is read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HeaderError {
    #[error("file is {0} bytes long; a session file starts with an 8-byte header")]
    TooShort(usize),
    #[error("not a Chrome session file (the first four bytes are not 'SNSS')")]
    BadMagic,
    #[error(
        "SNSS version {0} is Chrome's encrypted session format, which knowmoretabs cannot read"
    )]
    Encrypted(u32),
    #[error("SNSS version {0} is not supported; only version 3 (cleartext) is")]
    UnsupportedVersion(u32),
}

/// Validates the header and returns the file version, which is always
/// [`VERSION_CLEARTEXT`] on success.
pub fn read_header(bytes: &[u8]) -> Result<u32, HeaderError> {
    if bytes.len() < HEADER_LEN {
        return Err(HeaderError::TooShort(bytes.len()));
    }
    if bytes[..4] != MAGIC {
        return Err(HeaderError::BadMagic);
    }
    match u32_at(bytes, 4) {
        Some(VERSION_CLEARTEXT) => Ok(VERSION_CLEARTEXT),
        Some(VERSION_ENCRYPTED) => Err(HeaderError::Encrypted(VERSION_ENCRYPTED)),
        Some(other) => Err(HeaderError::UnsupportedVersion(other)),
        None => Err(HeaderError::TooShort(bytes.len())),
    }
}

/// One framed command: `id` and its contents.
#[derive(Debug, Clone, Copy)]
pub struct Command<'a> {
    pub id: u8,
    pub contents: &'a [u8],
}

/// Everything after the header, split into whole commands.
#[derive(Debug)]
pub struct Framing<'a> {
    pub commands: Vec<Command<'a>>,
    /// Bytes at the end that did not form a whole command. Zero is normal;
    /// non-zero means Chrome was writing when we read, or the file was cut.
    pub truncated_bytes: usize,
}

/// Walks commands from the end of the header to the end of the buffer using
/// Chromium's three stop conditions: fewer than two bytes left, a zero size,
/// or a size that overruns the buffer. Each is a torn tail, never an error.
pub fn frame(bytes: &[u8]) -> Framing<'_> {
    let mut commands = Vec::new();
    let mut offset = HEADER_LEN.min(bytes.len());
    loop {
        let remaining = bytes.len() - offset;
        if remaining == 0 {
            return Framing {
                commands,
                truncated_bytes: 0,
            };
        }
        let whole = u16_at(bytes, offset)
            .map(usize::from)
            .filter(|&size| size >= 1 && 2 + size <= remaining);
        let Some(size) = whole else {
            return Framing {
                commands,
                truncated_bytes: remaining,
            };
        };
        commands.push(Command {
            id: bytes[offset + 2],
            contents: &bytes[offset + 3..offset + 2 + size],
        });
        offset += 2 + size;
    }
}

/// A 128-bit `base::Token`, the identity of a tab group or split view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token {
    pub high: u64,
    pub low: u64,
}

impl fmt::Display for Token {
    /// Matches `base::Token::ToString`: 32 upper-case hex digits, high first.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016X}{:016X}", self.high, self.low)
    }
}

/// A cursor over a `base::Pickle` payload with Chromium's read rules: every
/// item is padded to four bytes, lengths are `int32` and never negative, and a
/// short read fails without consuming anything further.
#[derive(Debug)]
pub struct Pickle<'a> {
    payload: &'a [u8],
    pos: usize,
}

impl<'a> Pickle<'a> {
    /// Applies `PickleIterator::WithData`: the header holds the payload size,
    /// the payload is the *last* `payload_size` bytes, and whatever precedes
    /// it must be a multiple of four. Anything else is not a pickle.
    pub fn from_contents(contents: &'a [u8]) -> Option<Self> {
        let payload_size = u32_at(contents, 0).and_then(|n| usize::try_from(n).ok())?;
        let header_size = contents.len().checked_sub(payload_size)?;
        if header_size < 4 || header_size % 4 != 0 {
            return None;
        }
        Some(Self {
            payload: &contents[header_size..],
            pos: 0,
        })
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let bytes = self.payload.get(self.pos..end)?;
        self.pos = end.div_ceil(4) * 4;
        Some(bytes)
    }

    pub fn read_u32(&mut self) -> Option<u32> {
        self.take(4).and_then(|b| u32_at(b, 0))
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        self.read_u32().map(|v| i32::from_le_bytes(v.to_le_bytes()))
    }

    pub fn read_u64(&mut self) -> Option<u64> {
        self.take(8).and_then(|b| u64_at(b, 0))
    }

    pub fn read_bool(&mut self) -> Option<bool> {
        self.take(1).map(|b| b[0] != 0)
    }

    /// A length-prefixed run of bytes; `WriteString` and `WriteData` both
    /// produce this shape.
    pub fn read_bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.read_i32()?;
        let len = usize::try_from(len).ok()?;
        self.take(len)
    }

    /// `std::string` on the wire. Chromium never validates the encoding, so
    /// neither do we: invalid UTF-8 is replaced, not rejected.
    pub fn read_string(&mut self) -> Option<String> {
        self.read_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// `std::u16string` on the wire: the length counts UTF-16 code units, not
    /// bytes. Unpaired surrogates are replaced, not rejected.
    pub fn read_string16(&mut self) -> Option<String> {
        let units = self.read_i32()?;
        let units = usize::try_from(units).ok()?;
        let bytes = self.take(units.checked_mul(2)?)?;
        let decoded = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        Some(
            char::decode_utf16(decoded)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect(),
        )
    }

    pub fn read_token(&mut self) -> Option<Token> {
        let high = self.read_u64()?;
        let low = self.read_u64()?;
        Some(Token { high, low })
    }

    /// Bytes not yet consumed. Zero after a well-understood record.
    #[cfg(test)]
    pub fn remaining(&self) -> usize {
        self.payload.len().saturating_sub(self.pos)
    }
}

pub fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

pub fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    <[u8; 4]>::try_from(slice).ok().map(u32::from_le_bytes)
}

pub fn i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    u32_at(bytes, offset).map(|v| i32::from_le_bytes(v.to_le_bytes()))
}

pub fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset.checked_add(8)?)?;
    <[u8; 8]>::try_from(slice).ok().map(u64::from_le_bytes)
}

pub fn i64_at(bytes: &[u8], offset: usize) -> Option<i64> {
    u64_at(bytes, offset).map(|v| i64::from_le_bytes(v.to_le_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: u32) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&version.to_le_bytes());
        v
    }

    fn command(id: u8, contents: &[u8]) -> Vec<u8> {
        let size = u16::try_from(contents.len() + 1).unwrap();
        let mut v = size.to_le_bytes().to_vec();
        v.push(id);
        v.extend_from_slice(contents);
        v
    }

    #[test]
    fn header_accepts_only_version_three() {
        assert_eq!(read_header(&header(3)), Ok(3));
        assert_eq!(read_header(&header(5)), Err(HeaderError::Encrypted(5)));
        assert_eq!(
            read_header(&header(1)),
            Err(HeaderError::UnsupportedVersion(1))
        );
        assert_eq!(
            read_header(&header(6)),
            Err(HeaderError::UnsupportedVersion(6))
        );
        assert_eq!(read_header(b"SNSS\x03\0\0"), Err(HeaderError::TooShort(7)));
        assert_eq!(read_header(b"SSNS\x03\0\0\0"), Err(HeaderError::BadMagic));
        assert_eq!(read_header(b""), Err(HeaderError::TooShort(0)));
    }

    #[test]
    fn frames_whole_commands_and_reports_the_tail() {
        let mut bytes = header(3);
        bytes.extend(command(7, &[1, 2, 3]));
        bytes.extend(command(255, &[]));
        let clean = frame(&bytes);
        assert_eq!(clean.commands.len(), 2);
        assert_eq!(clean.commands[0].id, 7);
        assert_eq!(clean.commands[0].contents, &[1, 2, 3]);
        assert_eq!(clean.commands[1].id, 255);
        assert_eq!(clean.commands[1].contents.len(), 0);
        assert_eq!(clean.truncated_bytes, 0);

        // One byte short of the last command: it is torn, the first survives.
        let torn = frame(&bytes[..bytes.len() - 1]);
        assert_eq!(torn.commands.len(), 1);
        assert_eq!(torn.truncated_bytes, 2);

        // Only a single trailing byte: cannot even read a size.
        bytes.push(0x09);
        let torn = frame(&bytes);
        assert_eq!(torn.commands.len(), 2);
        assert_eq!(torn.truncated_bytes, 1);
    }

    #[test]
    fn zero_size_is_a_torn_tail() {
        let mut bytes = header(3);
        bytes.extend(command(0, &[0; 8]));
        bytes.extend([0, 0, 0, 0, 0, 0]);
        let framed = frame(&bytes);
        assert_eq!(framed.commands.len(), 1);
        assert_eq!(framed.truncated_bytes, 6);
    }

    #[test]
    fn header_only_file_has_no_commands() {
        let bytes = header(3);
        let framed = frame(&bytes);
        assert!(framed.commands.is_empty());
        assert_eq!(framed.truncated_bytes, 0);
    }

    fn pickle_bytes(payload: &[u8]) -> Vec<u8> {
        let mut v = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn pickle_reads_primitives_with_alignment() {
        let mut payload = Vec::new();
        payload.extend(7i32.to_le_bytes());
        payload.extend([1, 0, 0, 0]); // bool + pad
        payload.extend(3i32.to_le_bytes());
        payload.extend(b"abc\0"); // string + pad
        payload.extend(2i32.to_le_bytes());
        payload.extend([b'h', 0, b'i', 0]); // string16, already aligned
        payload.extend(0x1122_3344_5566_7788u64.to_le_bytes());
        payload.extend(0x99u64.to_le_bytes());
        let contents = pickle_bytes(&payload);
        let mut p = Pickle::from_contents(&contents).unwrap();
        assert_eq!(p.read_i32(), Some(7));
        assert_eq!(p.read_bool(), Some(true));
        assert_eq!(p.read_string().as_deref(), Some("abc"));
        assert_eq!(p.read_string16().as_deref(), Some("hi"));
        assert_eq!(
            p.read_token().unwrap().to_string(),
            "11223344556677880000000000000099"
        );
        assert_eq!(p.remaining(), 0);
        assert_eq!(p.read_i32(), None);
    }

    #[test]
    fn pickle_rejects_negative_lengths_and_short_reads() {
        let mut payload = Vec::new();
        payload.extend((-1i32).to_le_bytes());
        let contents = pickle_bytes(&payload);
        let mut p = Pickle::from_contents(&contents).unwrap();
        assert_eq!(p.read_string(), None);

        let mut payload = Vec::new();
        payload.extend(100i32.to_le_bytes());
        payload.extend(b"short");
        let contents = pickle_bytes(&payload);
        let mut p = Pickle::from_contents(&contents).unwrap();
        assert_eq!(p.read_string16(), None);
        assert_eq!(p.read_string(), None);
    }

    #[test]
    fn pickle_takes_payload_from_the_end_like_chromium() {
        // payload_size smaller than the buffer: leading slack is skipped, but
        // only when the slack is a multiple of four.
        let mut contents = 4u32.to_le_bytes().to_vec();
        contents.extend([0xAA, 0xBB, 0xCC, 0xDD]); // slack
        contents.extend(42i32.to_le_bytes());
        let mut p = Pickle::from_contents(&contents).unwrap();
        assert_eq!(p.read_i32(), Some(42));

        let mut misaligned = 4u32.to_le_bytes().to_vec();
        misaligned.extend([0xAA, 0xBB]);
        misaligned.extend(42i32.to_le_bytes());
        assert!(Pickle::from_contents(&misaligned).is_none());

        let overlong = 99u32.to_le_bytes().to_vec();
        assert!(Pickle::from_contents(&overlong).is_none());
        assert!(Pickle::from_contents(&[]).is_none());
    }

    #[test]
    fn lossy_decoding_never_fails() {
        let mut payload = Vec::new();
        payload.extend(2i32.to_le_bytes());
        payload.extend([0xFF, 0xFE, 0, 0]);
        payload.extend(1i32.to_le_bytes());
        payload.extend([0x00, 0xD8, 0, 0]); // lone high surrogate
        let contents = pickle_bytes(&payload);
        let mut p = Pickle::from_contents(&contents).unwrap();
        assert_eq!(p.read_string().unwrap(), "\u{FFFD}\u{FFFD}");
        assert_eq!(p.read_string16().unwrap(), "\u{FFFD}");
    }
}
