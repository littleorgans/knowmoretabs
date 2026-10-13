//! The proof the shell and the model service exchange on every forward,
//! and the key file it rests on.
//!
//! slice: library, triage
//! why: The shell serves the service's pages in its own origin, so a
//!      process squatting on the service's port must not be able to answer
//!      in its place. Each request carries a fresh nonce, the time and an
//!      HMAC of its framed transcript; a reply counts only when its HMAC
//!      binds that nonce, the request, the status, the type, the archive's
//!      identity and the exact body. The key never crosses the wire.
//!      `lab/tagger/tagger/app/service_auth.py` is the service's side;
//!      `tests/fixtures/service_auth.json` holds both to the same bytes.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac as _};
use sha2::Sha256;

use crate::digest::{hex, sha256_hex};

/// Header names, as the service reads and writes them.
pub const NONCE: &str = "X-KMT-Nonce";
pub const TIME: &str = "X-KMT-Time";
pub const REQUEST_MAC: &str = "X-KMT-Request-MAC";
pub const RESPONSE_MAC: &str = "X-KMT-Response-MAC";
const REQUEST_VERSION: &str = "kmt-s2-request-v1";
const RESPONSE_VERSION: &str = "kmt-s2-response-v1";

/// The decoded key. No `Debug` and no `Display`: nothing can print it.
pub struct Key([u8; 32]);

/// Why the key file was not used. Named only, never with its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unusable {
    Unreadable,
    /// A link, not a regular file, someone else's, or readable by others.
    Unsafe,
    /// Not exactly 64 lowercase hex characters and a newline.
    Malformed,
}

impl Key {
    /// Opened, then judged by what was opened: a regular file of this user's
    /// that only this user can read, at a path that is not a link.
    pub fn read(path: &Path) -> Result<Self, Unusable> {
        let file = File::open(path).map_err(|_| Unusable::Unreadable)?;
        let opened = file.metadata().map_err(|_| Unusable::Unreadable)?;
        let named = std::fs::symlink_metadata(path).map_err(|_| Unusable::Unreadable)?;
        if !opened.is_file() || named.file_type().is_symlink() || !private(&opened, &named) {
            return Err(Unusable::Unsafe);
        }
        let mut text = Vec::with_capacity(66);
        file.take(66)
            .read_to_end(&mut text)
            .map_err(|_| Unusable::Unreadable)?;
        Self::decode(&text).ok_or(Unusable::Malformed)
    }

    fn decode(text: &[u8]) -> Option<Self> {
        let hex = std::str::from_utf8(text.strip_suffix(b"\n")?).ok()?;
        decode_hex(hex)?.try_into().ok().map(Self)
    }

    fn mac(&self) -> Hmac<Sha256> {
        <Hmac<Sha256>>::new_from_slice(&self.0).expect("HMAC takes a key of any length")
    }

    /// The request's MAC, lowercase hex.
    pub fn sign(&self, request: &Request<'_>) -> String {
        let mut mac = self.mac();
        mac.update(&request.transcript());
        hex(&mac.finalize().into_bytes())
    }

    /// Whether `given` is this key's MAC of the reply, compared in constant
    /// time.
    pub fn verify(&self, reply: &Reply<'_>, given: &str) -> bool {
        let mut mac = self.mac();
        mac.update(&reply.transcript());
        decode_hex(given).is_some_and(|given| mac.verify_slice(&given).is_ok())
    }
}

/// One request as the service receives it.
pub struct Request<'a> {
    pub nonce: &'a str,
    /// Unix milliseconds, decimal.
    pub time: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    /// Empty when the request sends none.
    pub content_type: &'a str,
    pub body: &'a [u8],
}

impl Request<'_> {
    fn transcript(&self) -> Vec<u8> {
        framed(&[
            REQUEST_VERSION,
            self.nonce,
            self.time,
            self.method,
            self.path,
            self.content_type,
            &self.body.len().to_string(),
            &sha256_hex(self.body),
        ])
    }
}

/// One reply, as the shell received it, to the request with `nonce`.
pub struct Reply<'a> {
    pub nonce: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status: u16,
    pub content_type: &'a str,
    pub archive: &'a str,
    pub body: &'a [u8],
}

impl Reply<'_> {
    fn transcript(&self) -> Vec<u8> {
        framed(&[
            RESPONSE_VERSION,
            self.nonce,
            self.method,
            self.path,
            &self.status.to_string(),
            self.content_type,
            self.archive,
            &self.body.len().to_string(),
            &sha256_hex(self.body),
        ])
    }
}

/// 32 random bytes, lowercase hex: one exchange's nonce.
pub fn nonce() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(hex(&bytes))
}

/// Now, in Unix milliseconds.
pub fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
        .to_string()
}

/// Each field as its UTF-8 length, unsigned 64 bit big endian, then its
/// bytes: no field can pass for two.
fn framed(fields: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in fields {
        out.extend_from_slice(&(field.len() as u64).to_be_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    out
}

/// Lowercase hex only, as both sides write it.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// The same file under both looks, owned by this user, and no access for
/// group or others.
#[cfg(unix)]
fn private(opened: &std::fs::Metadata, named: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    (opened.dev(), opened.ino()) == (named.dev(), named.ino())
        && opened.uid() == nix::unistd::getuid().as_raw()
        // No bit for the group or for others.
        && opened.mode().trailing_zeros() >= 6
}

/// No owner or mode to judge here; the model service runs on Unix only.
#[cfg(not(unix))]
fn private(_opened: &std::fs::Metadata, _named: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/service_auth.json")).unwrap()
    }

    fn key(vectors: &Value) -> Key {
        Key::decode(format!("{}\n", vectors["key"].as_str().unwrap()).as_bytes()).unwrap()
    }

    fn text<'a>(case: &'a Value, name: &str) -> &'a str {
        case[name].as_str().unwrap()
    }

    #[test]
    fn requests_sign_to_the_shared_vectors() {
        let vectors = vectors();
        let key = key(&vectors);
        for case in vectors["requests"].as_array().unwrap() {
            let request = Request {
                nonce: text(case, "nonce"),
                time: text(case, "time"),
                method: text(case, "method"),
                path: text(case, "path"),
                content_type: text(case, "content_type"),
                body: text(case, "body").as_bytes(),
            };
            assert_eq!(key.sign(&request), text(case, "mac"), "{case}");
        }
    }

    #[test]
    fn replies_verify_against_the_shared_vectors_and_nothing_else() {
        let vectors = vectors();
        let key = key(&vectors);
        let other = Key([7; 32]);
        for case in vectors["responses"].as_array().unwrap() {
            let body = text(case, "body").as_bytes();
            let status = u16::try_from(case["status"].as_u64().unwrap()).unwrap();
            let mac = text(case, "mac");
            assert!(key.verify(&reply(case, status, body), mac), "{case}");
            assert!(
                !other.verify(&reply(case, status, body), mac),
                "another key"
            );
            assert!(
                !key.verify(&reply(case, status + 1, body), mac),
                "another status"
            );
            assert!(
                !key.verify(&reply(case, status, b"{}"), mac),
                "another body"
            );
            assert!(!key.verify(&reply(case, status, body), &mac.to_uppercase()));
            assert!(!key.verify(&reply(case, status, body), &mac[..62]));
            let changed = [
                Reply {
                    archive: "1:2",
                    ..reply(case, status, body)
                },
                Reply {
                    nonce: "00",
                    ..reply(case, status, body)
                },
                Reply {
                    method: "PUT",
                    ..reply(case, status, body)
                },
                Reply {
                    path: "/api/other",
                    ..reply(case, status, body)
                },
                Reply {
                    content_type: "text/html",
                    ..reply(case, status, body)
                },
            ];
            for changed in changed {
                assert!(!key.verify(&changed, mac), "{case}");
            }
        }
    }

    fn reply<'a>(case: &'a Value, status: u16, body: &'a [u8]) -> Reply<'a> {
        Reply {
            nonce: text(case, "nonce"),
            method: text(case, "method"),
            path: text(case, "path"),
            status,
            content_type: text(case, "content_type"),
            archive: text(case, "archive"),
            body,
        }
    }

    #[test]
    fn fields_are_framed_by_their_length() {
        let vectors = vectors();
        let fields: Vec<&str> = vectors["framed"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        assert_eq!(
            hex(&framed(&fields)),
            vectors["framed"]["hex"].as_str().unwrap()
        );
        assert_ne!(framed(&["ab", "c"]), framed(&["a", "bc"]));
    }

    #[test]
    fn nonces_are_fresh_lowercase_hex() {
        let (a, b) = (nonce().unwrap(), nonce().unwrap());
        assert_ne!(a, b);
        assert_eq!(decode_hex(&a).map(|bytes| bytes.len()), Some(32));
    }

    #[cfg(unix)]
    #[test]
    fn only_a_private_well_formed_key_file_is_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str, mode: u32| {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let good = format!("{}\n", "ab".repeat(32));
        assert!(Key::read(&write("good", &good, 0o600)).is_ok());
        assert!(Key::read(&write("owner-only", &good, 0o400)).is_ok());
        assert_eq!(
            Key::read(&write("shared", &good, 0o640)).err(),
            Some(Unusable::Unsafe)
        );
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("good"), &link).unwrap();
        assert_eq!(Key::read(&link).err(), Some(Unusable::Unsafe));
        for (name, text) in [
            ("short", "ab".repeat(31) + "\n"),
            ("upper", "AB".repeat(32) + "\n"),
            ("bare", "ab".repeat(32)),
            ("crlf", "ab".repeat(32) + "\r\n"),
            ("long", "ab".repeat(32) + "\n\n"),
        ] {
            assert_eq!(
                Key::read(&write(name, &text, 0o600)).err(),
                Some(Unusable::Malformed),
                "{name}"
            );
        }
        assert_eq!(
            Key::read(&dir.path().join("missing")).err(),
            Some(Unusable::Unreadable)
        );
        assert_eq!(Key::read(dir.path()).err(), Some(Unusable::Unsafe));
    }
}
