//! Reads a page's `<head>`: title, meta tags, canonical link, language and
//! JSON-LD, from bytes in whatever charset the page declared.
//!
//! slice: library
//! why: Enrich wants a dozen values out of the head, not a DOM. A scanner that
//!      knows comments, quoted attributes and the elements whose content is
//!      raw text does that in a few hundred lines with no parser dependency,
//!      and it stops at `</head>` because that is all it is ever given.
//!      Charsets are the ones the web actually declares: UTF-8, UTF-16 by
//!      byte-order mark, and windows-1252 under its many labels; any other
//!      is named rather than decoded into nonsense.

use std::borrow::Cow;

/// What the scanner found, raw: values decoded but not yet chosen between.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub title: Option<String>,
    pub lang: Option<String>,
    /// `<link rel="canonical">`'s `href`, as written; relative is possible.
    pub canonical: Option<String>,
    /// `(key, content)` in document order, the key lower-cased: the meta
    /// tag's `property`, else its `name`, else its `itemprop`.
    pub meta: Vec<(String, String)>,
    /// The text of each `<script type="application/ld+json">`.
    pub jsonld: Vec<String>,
    /// `<meta charset>` or its `http-equiv` spelling.
    pub charset: Option<String>,
}

impl Head {
    /// The first content for `key`: a page that repeats a tag meant the first.
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Incremental boundary detection keeps comments, attributes and raw text
/// opaque even when a delimiter is split across socket reads.
#[derive(Default)]
pub struct Boundary {
    tag: Option<Vec<u8>>,
    name_done: bool,
    quote: Option<u8>,
    comment: bool,
    raw: Option<Vec<u8>>,
    tail: Vec<u8>,
}

impl Boundary {
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            if self.comment {
                self.tail.push(byte);
                if self.tail.len() > 3 {
                    self.tail.remove(0);
                }
                if self.tail == b"-->" {
                    self.comment = false;
                    self.tail.clear();
                }
                continue;
            }
            if let Some(close) = &self.raw {
                self.tail.push(byte.to_ascii_lowercase());
                if self.tail.len() > close.len() + 1 {
                    self.tail.remove(0);
                }
                if self.tail.starts_with(close)
                    && self.tail.len() == close.len() + 1
                    && (byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
                {
                    self.raw = None;
                    self.tail.clear();
                    if byte != b'>' {
                        self.tag = Some(Vec::new());
                        self.name_done = true;
                    }
                }
                continue;
            }
            if let Some(tag) = &mut self.tag {
                if let Some(quote) = self.quote {
                    if byte == quote {
                        self.quote = None;
                    }
                    continue;
                }
                if byte == b'>' {
                    if tag == b"/head" || tag == b"body" {
                        return true;
                    }
                    if [
                        b"script".as_slice(),
                        b"style",
                        b"title",
                        b"textarea",
                        b"xmp",
                        b"noscript",
                        b"template",
                    ]
                    .contains(&tag.as_slice())
                    {
                        let mut close = b"</".to_vec();
                        close.extend_from_slice(tag);
                        self.raw = Some(close);
                    }
                    self.tag = None;
                    continue;
                }
                if !self.name_done {
                    if byte.is_ascii_whitespace() || (byte == b'/' && !tag.is_empty()) {
                        self.name_done = true;
                    } else if tag.len() < 16 {
                        tag.push(byte.to_ascii_lowercase());
                        if tag == b"!--" {
                            self.comment = true;
                            self.tag = None;
                        }
                    }
                } else if matches!(byte, b'\'' | b'"') {
                    self.quote = Some(byte);
                }
            } else if byte == b'<' {
                self.tag = Some(Vec::new());
                self.name_done = false;
            }
        }
        false
    }
}

/// Scans until `</head>` or `<body>`, or the end of the text.
pub fn scan(html: &str) -> Head {
    let bytes = html.as_bytes();
    let mut head = Head::default();
    let mut pos = 0;
    while let Some(offset) = bytes[pos..].iter().position(|&b| b == b'<') {
        let start = pos + offset;
        let rest = &bytes[start..];
        if rest.starts_with(b"<!--") {
            pos = find(bytes, start + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            pos = find(bytes, start, b">").map_or(bytes.len(), |end| end + 1);
            continue;
        }
        let closing = rest.get(1) == Some(&b'/');
        let name_start = start + 1 + usize::from(closing);
        let name_end = bytes[name_start..]
            .iter()
            .position(|b| !b.is_ascii_alphanumeric() && *b != b'-' && *b != b':')
            .map_or(bytes.len(), |n| name_start + n);
        if name_end == name_start || !bytes[name_start].is_ascii_alphabetic() {
            pos = start + 1;
            continue;
        }
        let name = html[name_start..name_end].to_ascii_lowercase();
        let (attrs, tag_end) = attributes(html, name_end);
        pos = tag_end;
        if closing {
            if name == "head" {
                break;
            }
            continue;
        }
        match name.as_str() {
            "body" => break,
            "html" if head.lang.is_none() => {
                head.lang = attr(&attrs, "lang")
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned);
            }
            "meta" => read_meta(&mut head, &attrs),
            "link" if head.canonical.is_none() => {
                let canonical = attr(&attrs, "rel").is_some_and(|rel| {
                    rel.split_ascii_whitespace()
                        .any(|r| r.eq_ignore_ascii_case("canonical"))
                });
                if canonical {
                    head.canonical = attr(&attrs, "href")
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                        .map(str::to_owned);
                }
            }
            "title" => {
                let (text, after) = raw_text(html, pos, "title");
                pos = after;
                if head.title.is_none() {
                    head.title = Some(collapse(&decode_entities(text))).filter(|t| !t.is_empty());
                }
            }
            "script" => {
                let (text, after) = raw_text(html, pos, "script");
                pos = after;
                let jsonld = attr(&attrs, "type")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"));
                if jsonld {
                    head.jsonld.push(text.to_owned());
                }
            }
            "style" | "template" | "noscript" | "textarea" | "xmp" => {
                pos = raw_text(html, pos, &name).1;
            }
            _ => {}
        }
    }
    head
}

fn read_meta(head: &mut Head, attrs: &[(String, String)]) {
    if head.charset.is_none() {
        head.charset = attr(attrs, "charset")
            .map(|c| c.trim().to_owned())
            .or_else(|| {
                attr(attrs, "http-equiv")
                    .filter(|h| h.trim().eq_ignore_ascii_case("content-type"))
                    .and_then(|_| attr(attrs, "content"))
                    .and_then(charset_param)
            });
    }
    let key = ["property", "name", "itemprop"]
        .iter()
        .find_map(|k| attr(attrs, k).map(str::trim).filter(|v| !v.is_empty()));
    let content = attr(attrs, "content").map(str::trim);
    if let (Some(key), Some(content)) = (key, content)
        && !content.is_empty()
    {
        head.meta
            .push((key.to_ascii_lowercase(), content.to_owned()));
    }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// A tag's attributes from just after its name, and where the tag ends.
/// Every index this stops at sits on or just past an ASCII byte, so slicing
/// the `str` there is always on a character boundary.
fn attributes(html: &str, mut i: usize) -> (Vec<(String, String)>, usize) {
    let b = html.as_bytes();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut names = std::collections::HashSet::new();
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return (attrs, b.len());
        }
        if b[i] == b'>' {
            return (attrs, i + 1);
        }
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = html[name_start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = Cow::Borrowed("");
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let quote = [b[i]];
                let start = i + 1;
                let end = find(b, start, &quote).unwrap_or(b.len());
                value = decode_entities(&html[start..end]);
                i = (end + 1).min(b.len());
            } else {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = decode_entities(&html[start..i]);
            }
        }
        // An empty name is a stray `=`, whose value was just stepped over.
        if !name.is_empty() && names.insert(name.clone()) {
            attrs.push((name, value.into_owned()));
        }
    }
}

/// The content of a raw-text element up to its closing tag, and the index
/// just past that tag.
fn raw_text<'a>(html: &'a str, from: usize, name: &str) -> (&'a str, usize) {
    let bytes = html.as_bytes();
    let close = format!("</{name}");
    let mut search = from;
    let end = loop {
        let Some(at) = find_ci(bytes, search, close.as_bytes()) else {
            break None;
        };
        if bytes
            .get(at + close.len())
            .is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
        {
            break Some(at);
        }
        search = at + close.len();
    };
    match end {
        Some(end) => {
            let after = find(bytes, end, b">").map_or(bytes.len(), |gt| gt + 1);
            (&html[from..end], after)
        }
        None => (&html[from..], bytes.len()),
    }
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

/// `needle` must be lower-case ASCII.
fn find_ci(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|i| from + i)
}

pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `charset=` out of a `Content-Type` value, unquoted.
pub fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let (key, value) = param.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_owned())
            .filter(|v| !v.is_empty())
    })
}

/// Character references. Numeric ones in full, as HTML reads them; named
/// ones from the short list pages actually put in titles and descriptions.
/// An unknown name stays as written, which is what a reader would see
/// anyway if the page had not escaped it.
pub fn decode_entities(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some((decoded, len)) = reference(rest) {
            out.push(decoded);
            rest = &rest[len..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// One reference at the start of `text`, which begins with `&`: the
/// character and how many bytes it spelled.
fn reference(text: &str) -> Option<(char, usize)> {
    let body = &text[1..];
    if let Some(num) = body.strip_prefix('#') {
        let (digits, radix, prefix) = match num.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16, 2),
            None => (num, 10, 1),
        };
        let len = digits
            .bytes()
            .take_while(|b| b.is_ascii_digit() || (radix == 16 && b.is_ascii_hexdigit()))
            .count();
        if len == 0 {
            return None;
        }
        let value = u32::from_str_radix(&digits[..len], radix).unwrap_or(u32::MAX);
        let semicolon = usize::from(digits[len..].starts_with(';'));
        return Some((numeric_char(value), 1 + prefix + len + semicolon));
    }
    let len = body.bytes().take_while(u8::is_ascii_alphanumeric).count();
    if !body[len..].starts_with(';') {
        return None;
    }
    let c = named(&body[..len])?;
    Some((c, 1 + len + 1))
}

/// HTML's rules: C1 controls are read as windows-1252, and nothing that is
/// not a character comes out as one.
fn numeric_char(value: u32) -> char {
    if let Ok(byte) = u8::try_from(value)
        && (0x80..0xA0).contains(&byte)
    {
        return WINDOWS_1252_HIGH[usize::from(byte - 0x80)];
    }
    match value {
        0 => '\u{FFFD}',
        v => char::from_u32(v).unwrap_or('\u{FFFD}'),
    }
}

fn named(name: &str) -> Option<char> {
    Some(match name {
        "amp" | "AMP" => '&',
        "lt" | "LT" => '<',
        "gt" | "GT" => '>',
        "quot" | "QUOT" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "zwnj" => '\u{200C}',
        "zwj" => '\u{200D}',
        "shy" => '\u{AD}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "middot" => '·',
        "bull" => '•',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "hearts" => '♥',
        "check" => '✓',
        "iexcl" => '¡',
        "iquest" => '¿',
        "szlig" => 'ß',
        "aacute" => 'á',
        "Aacute" => 'Á',
        "agrave" => 'à',
        "Agrave" => 'À',
        "acirc" => 'â',
        "auml" => 'ä',
        "Auml" => 'Ä',
        "aring" => 'å',
        "Aring" => 'Å',
        "aelig" => 'æ',
        "AElig" => 'Æ',
        "atilde" => 'ã',
        "ccedil" => 'ç',
        "Ccedil" => 'Ç',
        "eacute" => 'é',
        "Eacute" => 'É',
        "egrave" => 'è',
        "Egrave" => 'È',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "iacute" => 'í',
        "igrave" => 'ì',
        "icirc" => 'î',
        "iuml" => 'ï',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        "oacute" => 'ó',
        "Oacute" => 'Ó',
        "ograve" => 'ò',
        "ocirc" => 'ô',
        "otilde" => 'õ',
        "ouml" => 'ö',
        "Ouml" => 'Ö',
        "oslash" => 'ø',
        "Oslash" => 'Ø',
        "uacute" => 'ú',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "uuml" => 'ü',
        "Uuml" => 'Ü',
        "yacute" => 'ý',
        "yuml" => 'ÿ',
        _ => return None,
    })
}

/// A page's bytes as text, or the charset label it declared that this does
/// not decode. The precedence is HTML's: byte-order mark, then the
/// `Content-Type` header, then `<meta charset>`, then UTF-8 if the bytes
/// are UTF-8 and windows-1252 if they are not, as a browser would guess.
pub fn decode(bytes: &[u8], header_charset: Option<&str>) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return Ok(String::from_utf8_lossy(rest).into_owned());
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return Ok(utf16(rest, u16::from_le_bytes));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return Ok(utf16(rest, u16::from_be_bytes));
    }
    if let Some(label) = header_charset {
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-16" | "utf-16le" => return Ok(utf16(bytes, u16::from_le_bytes)),
            "utf-16be" => return Ok(utf16(bytes, u16::from_be_bytes)),
            _ => {}
        }
    }
    let declared = match header_charset {
        Some(label) => Some(label.to_owned()),
        // The markup that declares a charset is ASCII in every charset this
        // could be, so a lossy read finds it.
        None => scan(&String::from_utf8_lossy(bytes)).charset,
    };
    let encoding = match declared {
        Some(label) => Encoding::for_label(&label).ok_or(label)?,
        None if std::str::from_utf8(bytes).is_ok() => Encoding::Utf8,
        None => Encoding::Windows1252,
    };
    Ok(match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Windows1252 => bytes.iter().map(|&b| windows_1252(b)).collect(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Windows1252,
}

impl Encoding {
    /// Labels from the WHATWG Encoding Standard. `utf-16` declared in markup
    /// the scanner could read is not UTF-16, and HTML reads it as UTF-8.
    fn for_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" | "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8"
            | "x-unicode20utf8" | "utf-16" | "utf-16le" | "utf-16be" => Some(Self::Utf8),
            "windows-1252" | "cp1252" | "x-cp1252" | "iso-8859-1" | "iso8859-1" | "iso_8859-1"
            | "iso88591" | "iso-ir-100" | "latin1" | "l1" | "cp819" | "ibm819" | "csisolatin1"
            | "us-ascii" | "ascii" | "ansi_x3.4-1968" | "iso_8859-1:1987" => {
                Some(Self::Windows1252)
            }
            _ => None,
        }
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| unit([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// 0x80–0x9F in windows-1252. The five bytes it leaves undefined map to the
/// C1 control of the same number, as the Encoding Standard says.
const WINDOWS_1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

fn windows_1252(byte: u8) -> char {
    if (0x80..0xA0).contains(&byte) {
        WINDOWS_1252_HIGH[usize::from(byte - 0x80)]
    } else {
        char::from(byte)
    }
}

/// Readable text from an HTML fragment: tags gone, code and images left
/// out, block elements on their own lines, whitespace collapsed.
pub fn text_of(html: &str) -> String {
    const SKIP: [&str; 7] = [
        "pre", "code", "svg", "script", "style", "picture", "template",
    ];
    const BLOCK: [&str; 12] = [
        "p",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "br",
        "tr",
        "div",
        "blockquote",
    ];
    let bytes = html.as_bytes();
    let mut out = String::new();
    let mut pos = 0;
    let mut skipping: Option<String> = None;
    let mut depth = 0usize;
    while pos < bytes.len() {
        let Some(offset) = bytes[pos..].iter().position(|&b| b == b'<') else {
            if skipping.is_none() {
                out.push_str(&decode_entities(&html[pos..]));
            }
            break;
        };
        let start = pos + offset;
        if skipping.is_none() {
            out.push_str(&decode_entities(&html[pos..start]));
        }
        if bytes[start..].starts_with(b"<!--") {
            pos = find(bytes, start + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        let closing = bytes.get(start + 1) == Some(&b'/');
        let name_start = start + 1 + usize::from(closing);
        let name_end = bytes[name_start..]
            .iter()
            .position(|b| !b.is_ascii_alphanumeric())
            .map_or(bytes.len(), |n| name_start + n);
        if name_end == name_start {
            if skipping.is_none() {
                out.push('<');
            }
            pos = start + 1;
            continue;
        }
        let name = html[name_start..name_end].to_ascii_lowercase();
        pos = attributes(html, name_end).1;
        match &skipping {
            Some(skipped) if *skipped == name => {
                if closing {
                    depth -= 1;
                    if depth == 0 {
                        skipping = None;
                    }
                } else {
                    depth += 1;
                }
            }
            None if !closing && SKIP.contains(&name.as_str()) => {
                skipping = Some(name);
                depth = 1;
            }
            None if BLOCK.contains(&name.as_str()) => out.push('\n'),
            Some(_) | None => {}
        }
    }
    out.lines()
        .map(collapse)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_survive_every_byte_split() {
        for html in [
            "<head><!-- </head> --><title>x</title></head>",
            "<head><meta content='</head>'><title>x</title></head>",
            "<head><script>'</scripture></head>';</script><title>x</title></head>",
        ] {
            let bytes = html.as_bytes();
            for split in 0..bytes.len() {
                let mut boundary = Boundary::default();
                assert!(!boundary.feed(&bytes[..split]), "{split}: {html}");
                assert!(boundary.feed(&bytes[split..]), "{split}: {html}");
            }
        }
    }

    #[test]
    fn malformed_input_and_random_bytes_finish_without_panicking() {
        let mut seed = 0x1234_5678_u32;
        for len in 0..4096 {
            let bytes: Vec<u8> = (0..len % 512)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed.to_le_bytes()[0]
                })
                .collect();
            let text = decode(&bytes, None).unwrap_or_default();
            let _ = scan(&text);
            let _ = text_of(&text);
            let _ = decode_entities(&text);
            let mut boundary = Boundary::default();
            for chunk in bytes.chunks(3) {
                let _ = boundary.feed(chunk);
            }
        }
        for html in [
            "<!-- unclosed",
            "<meta a='unclosed",
            "<",
            "</",
            "<meta = = =>",
            "<é/>",
        ] {
            let _ = scan(html);
            let _ = text_of(html);
        }
        let mut huge = String::from("<meta");
        for n in 0..30_000 {
            use std::fmt::Write;
            let _ = write!(huge, " a{n}=x");
        }
        huge.push_str(" name=description content=kept>");
        let started = std::time::Instant::now();
        assert_eq!(scan(&huge).meta("description"), Some("kept"));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(decode_entities("&#00000000065;"), "A");
        assert_eq!(decode_entities("&#x00000000041;"), "A");
    }

    #[test]
    fn the_values_enrich_keeps_come_out_of_an_ordinary_head() {
        let head = scan(concat!(
            "<!doctype html><HTML Lang='en-GB'><head>\n",
            "<meta charset=utf-8>",
            "<title>\n  A  &amp; B &#8212; C&#x2019;s  </title>",
            "<meta name=\"description\" content=\"Plain &quot;words&quot;\">",
            "<meta property=og:title content='OG title'>",
            "<meta property=\"og:title\" content=\"second one loses\">",
            "<meta name=\"twitter:card\" content=\"summary\">",
            "<link rel=\"alternate stylesheet\" href=\"/x.css\">",
            "<LINK REL=\"Canonical\" HREF=\"/canonical\">",
            "<script>var s = '<title>not this</title></head>';</script>",
            "<script type=\"application/ld+json\">{\"@type\":\"Article\"}</script>",
            "</head><body><meta name=\"description\" content=\"body\"></body>",
        ));
        assert_eq!(head.title.as_deref(), Some("A & B — C’s"));
        assert_eq!(head.lang.as_deref(), Some("en-GB"));
        assert_eq!(head.charset.as_deref(), Some("utf-8"));
        assert_eq!(head.canonical.as_deref(), Some("/canonical"));
        assert_eq!(head.meta("description"), Some("Plain \"words\""));
        assert_eq!(head.meta("og:title"), Some("OG title"));
        assert_eq!(head.meta("twitter:card"), Some("summary"));
        assert_eq!(head.jsonld, ["{\"@type\":\"Article\"}"]);
        assert_eq!(head.meta.len(), 4, "nothing after </head> is read");
    }

    #[test]
    fn comments_styles_and_odd_markup_do_not_derail_the_scan() {
        let head = scan(concat!(
            "<!-- <title>commented</title> --><? xml ?>",
            "<style>a > b { content: '<title>' }</style>",
            "<noscript><meta name=description content=hidden></noscript>",
            "1 < 2 <3 <meta name = description content = unquoted />",
            "<meta itemprop=name content=\"Item\">",
            "<meta name=\"empty\" content=\"  \"><meta =broken name=x content=y>",
            "<title>Real</title><title>Second</title>",
        ));
        assert_eq!(head.title.as_deref(), Some("Real"));
        assert_eq!(head.meta("description"), Some("unquoted"));
        assert_eq!(head.meta("name"), Some("Item"));
        assert_eq!(head.meta("empty"), None);
        assert_eq!(head.meta("x"), Some("y"));
    }

    #[test]
    fn a_head_that_never_closes_ends_at_the_body_or_the_text() {
        assert!(Boundary::default().feed(b"<head><title>x</title><body>"));
        assert!(Boundary::default().feed(b"<HEAD></HEAD><body>"));
        assert!(!Boundary::default().feed(b"<head><title>x</title>"));
        let head = scan("<title>Unclosed");
        assert_eq!(head.title.as_deref(), Some("Unclosed"));
        let head = scan("<meta name=description content=\"cut off");
        assert_eq!(head.meta("description"), Some("cut off"));
    }

    #[test]
    fn character_references_decode_as_html_reads_them() {
        for (raw, decoded) in [
            ("a &amp; b", "a & b"),
            ("&lt;tag&gt;", "<tag>"),
            ("&#39;&#x27;&#X27;", "'''"),
            ("&#150;", "–"),
            ("&#0; &#xD800; &#99999999999;", "\u{FFFD} \u{FFFD} \u{FFFD}"),
            ("&#233 no semicolon", "é no semicolon"),
            ("&eacute;t&eacute;", "été"),
            ("&notanentity; & &amp", "&notanentity; & &amp"),
            ("&#;&#x;", "&#;&#x;"),
            ("日本 &mdash; 語", "日本 — 語"),
        ] {
            assert_eq!(decode_entities(raw), decoded, "{raw}");
        }
    }

    #[test]
    fn charsets_follow_bom_then_header_then_meta_then_a_guess() {
        let cafe_1252 = b"<meta charset=\"windows-1252\"><title>caf\xE9 \x93q\x94</title>";
        assert_eq!(
            scan(&decode(cafe_1252, None).unwrap()).title.as_deref(),
            Some("café “q”")
        );
        let cafe_utf8 = "<meta charset=\"iso-8859-1\"><title>café</title>".as_bytes();
        assert_eq!(
            scan(&decode(cafe_utf8, Some("UTF-8")).unwrap())
                .title
                .as_deref(),
            Some("café"),
            "the header outranks the meta tag"
        );
        let undeclared_latin1 = b"<title>na\xEFve</title>";
        assert_eq!(
            scan(&decode(undeclared_latin1, None).unwrap())
                .title
                .as_deref(),
            Some("naïve")
        );
        let http_equiv =
            b"<meta http-equiv=Content-Type content='text/html; charset=\"latin1\"'><title>\xE0</title>";
        assert_eq!(
            scan(&decode(http_equiv, None).unwrap()).title.as_deref(),
            Some("à")
        );
        let big_endian: Vec<u8> = "<title>日本</title>"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert_eq!(
            scan(&decode(&big_endian, Some("utf-16be")).unwrap())
                .title
                .as_deref(),
            Some("日本")
        );
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "<title>é</title>".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(
            scan(&decode(&utf16, Some("windows-1252")).unwrap())
                .title
                .as_deref(),
            Some("é"),
            "the byte-order mark outranks the header"
        );
        assert_eq!(
            decode(b"<meta charset=shift_jis><title>\x93\xfa</title>", None),
            Err("shift_jis".to_owned())
        );
        assert_eq!(
            charset_param("text/html; Charset=\"UTF-8\""),
            Some("UTF-8".into())
        );
        assert_eq!(charset_param("text/html"), None);
    }

    #[test]
    fn readme_text_keeps_prose_and_drops_code_and_markup() {
        let text = text_of(concat!(
            "<article><h1>Tool</h1><p>Does <em>one</em> thing &amp; well.</p>",
            "<pre><code>cargo install tool</code></pre>",
            "<p><img src=x alt=badge><svg><path d=M0/><svg></svg></svg>Second   paragraph</p>",
            "<ul><li>one</li><li>two</li></ul> a < b</article>",
        ));
        assert_eq!(
            text,
            "Tool\nDoes one thing & well.\nSecond paragraph\none\ntwo\na < b"
        );
    }
}
