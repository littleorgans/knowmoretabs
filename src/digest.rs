//! Lowercase hex, and the lowercase hex SHA-256 of a byte string.
//!
//! slice: capture, library, triage, tags, content, sources
//! why: File names, blob keys, the vocabulary version and the service's
//!      MACs are all compared as text written by an earlier run or by the
//!      service, so every digest must come out in the same spelling: two
//!      lowercase digits per byte, no separators. One encoder keeps a caller
//!      from drifting to another case or padding and orphaning what is kept.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// Two lowercase hex digits per byte, in order.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The SHA-256 of `bytes`, as 64 lowercase hex digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_two_lowercase_digits_each() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x00, 0x0a, 0xab, 0xff]), "000aabff");
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
