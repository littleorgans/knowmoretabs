//! The key a local document is kept under: a file in a git repository by
//! the repository's portable name and its path, a loose file by the hash
//! of its bytes.
//!
//! slice: sources
//! why: An archive travels between machines and agents, so a document's key
//!      must name the same document everywhere and say nothing about this
//!      machine. A repository is named by its remote, less the scheme, the
//!      user, any token, the port and `.git`, with the host lowercased, so
//!      every clone of it agrees and no credential or local path is kept; a
//!      remote that is itself a local path has no portable name, and its
//!      files count as loose. `//` cannot occur in that name or in a git
//!      path, so it splits a git key unambiguously. Both forms are opaque
//!      URIs with no host, which every web rule (refusal, fetch planning,
//!      History lookup, key cleaning) already leaves alone. The key names
//!      the document, not a version: which bytes it was is the pin's.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// A file in a repository: `kmt-git:<repo>//<path>`.
pub const GIT: &str = "kmt-git:";
/// A loose file: `kmt-blob:sha256-<hex>`.
pub const BLOB: &str = "kmt-blob:";
const SHA256: &str = "sha256-";

/// What RFC 3986 lets stand unescaped in a path, with `/` kept as the
/// separator.
const PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// A source key, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Git { repo: String, path: String },
    Blob { sha256: String },
}

/// The key of `path` in repository `repo`.
pub fn git(repo: &str, path: &str) -> String {
    format!(
        "{GIT}{}//{}",
        utf8_percent_encode(repo, PATH),
        utf8_percent_encode(path, PATH)
    )
}

/// The key of a loose file whose bytes hash to `sha256`, lowercase hex.
pub fn blob(sha256: &str) -> String {
    format!("{BLOB}{SHA256}{sha256}")
}

/// `key` as a source key; `None` for any other string.
pub fn parse(key: &str) -> Option<Key> {
    if let Some(rest) = key.strip_prefix(GIT) {
        let (repo, path) = rest.split_once("//")?;
        let decode = |part: &str| {
            percent_decode_str(part)
                .decode_utf8()
                .ok()
                .map(std::borrow::Cow::into_owned)
                .filter(|text| !text.is_empty())
        };
        return Some(Key::Git {
            repo: decode(repo)?,
            path: decode(path)?,
        });
    }
    let sha256 = key.strip_prefix(BLOB)?.strip_prefix(SHA256)?;
    (sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    .then(|| Key::Blob {
        sha256: sha256.to_owned(),
    })
}

/// The portable name of the repository behind `remote`, a git remote URL:
/// `host/owner/repo`. `None` for a local path, or a remote with no host or
/// no path.
pub fn repo_id(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (host, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (strip_port(host), path)
    } else {
        // Git's own rule: a colon before any slash is the scp form, unless
        // what precedes it is a drive letter.
        let (authority, path) = remote.split_once(':')?;
        if authority.contains('/') || authority.len() < 2 {
            return None;
        }
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (host, path)
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    let fits = |part: &str| {
        !part.is_empty()
            && !part
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    };
    let segments_fit = path
        .split('/')
        .all(|segment| fits(segment) && segment != "." && segment != "..");
    (fits(host) && !host.contains(['/', ':', '@']) && fits(path) && segments_fit)
        .then(|| format!("{}/{path}", host.to_ascii_lowercase()))
}

/// `host` less a `:port`. An IPv6 literal keeps its colons, so it names no
/// portable repository and its files count as loose.
fn strip_port(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_becomes_the_same_portable_name_in_every_form() {
        for remote in [
            "git@example.test:o/r.git",
            "git@EXAMPLE.test:o/r",
            "example.test:o/r.git/",
            "https://example.test/o/r.git",
            "https://user:token@Example.Test:8443/o/r.git",
            "ssh://git@example.test:22/o/r.git",
            "ssh://git@example.test/o/r",
            "git://example.test/o/r.git",
        ] {
            assert_eq!(
                repo_id(remote).as_deref(),
                Some("example.test/o/r"),
                "{remote}"
            );
        }
        assert_eq!(
            repo_id("https://example.test/group/sub/r.git").as_deref(),
            Some("example.test/group/sub/r")
        );
        // Only the host is lowercased.
        assert_eq!(
            repo_id("git@Example.test:Owner/Repo.git").as_deref(),
            Some("example.test/Owner/Repo")
        );
    }

    #[test]
    fn a_local_or_unnamed_remote_has_no_portable_name() {
        for remote in [
            "/srv/git/r.git",
            "../r.git",
            "./r",
            "file:///srv/git/r.git",
            "C:\\repos\\r.git",
            "c:/repos/r.git",
            "https://example.test/",
            "https://example.test",
            "git@example.test:",
            "https://example.test/o/../r",
            "",
        ] {
            assert_eq!(repo_id(remote), None, "{remote}");
        }
    }

    #[test]
    fn a_git_key_round_trips_and_escapes_its_path() {
        let key = git("example.test/o/r", "notes/llm evals #1.md");
        assert_eq!(key, "kmt-git:example.test/o/r//notes/llm%20evals%20%231.md");
        assert_eq!(
            parse(&key),
            Some(Key::Git {
                repo: "example.test/o/r".to_owned(),
                path: "notes/llm evals #1.md".to_owned(),
            })
        );
    }

    #[test]
    fn a_blob_key_round_trips_and_only_takes_a_sha256() {
        let hex = "ab".repeat(32);
        let key = blob(&hex);
        assert_eq!(key, format!("kmt-blob:sha256-{hex}"));
        assert_eq!(parse(&key), Some(Key::Blob { sha256: hex }));
        for bad in [
            "kmt-blob:sha256-ab12",
            "kmt-blob:sha1-0000",
            &format!("kmt-blob:sha256-{}", "AB".repeat(32)),
            "kmt-git:example.test/o/r",
            "kmt-git://notes/x.md",
            "https://example.test/x",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn source_keys_are_opaque_uris_every_web_rule_leaves_alone() {
        let key = git("example.test/o/r", "notes/x.md");
        let url = url::Url::parse(&key).unwrap();
        assert_eq!(url.host(), None);
        assert_eq!(url.as_str(), key);
        assert_eq!(crate::page_key::clean(&key), key);
        assert_eq!(crate::add::refusal(&key), Some(crate::add::Reason::NotWeb));
        assert!(crate::library::is_listed(&key));
        assert!(crate::library::is_listed(&blob(&"0".repeat(64))));
    }
}
