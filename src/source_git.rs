//! What a local document's sources ask of git: where a file's checkout is,
//! which repository it is, whether it is committed, which blob it is, and
//! the bytes of a blob.
//!
//! slice: sources
//! why: A file in a repository is kept by reference, so git is the store and
//!      its object ids are the pin. Every question goes to the `git` the
//!      owner has, through the bounded runner content's tools use: an
//!      argument vector and never a shell, no input, a timeout, capped
//!      output. Each asks only and never writes: optional locks are off, so
//!      even `status` leaves the index alone, and git never prompts. Paths
//!      come back as git spells them, `/` separated and NUL ended, so no
//!      quoting or platform separator leaks into a key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::content_fetch::BODY_CAP;
use crate::tools::{self, Limits, Probe, System};

/// One question to git.
const TIMEOUT: Duration = Duration::from_secs(30);
/// What every run sets: never prompt, never take the index lock, and a
/// path is only ever a path, never a pattern.
const ENV: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_LITERAL_PATHSPECS", "1"),
];
/// The most a listing of a whole tree is read to.
const LISTING_CAP: usize = 64 * 1024 * 1024;

/// The `git` on `PATH`.
#[derive(Debug, Clone)]
pub struct Git {
    path: PathBuf,
}

impl Git {
    pub fn find() -> Option<Self> {
        System.find("git").map(|path| Self { path })
    }

    /// Standard output of `git -C <dir> <args>` when it exits 0 within
    /// `cap`; `None` otherwise.
    fn ask(&self, dir: &Path, args: &[&str], cap: usize) -> Option<Vec<u8>> {
        let dir = dir.to_str()?;
        let mut command = vec!["-C", dir];
        command.extend_from_slice(args);
        let limits = Limits {
            timeout: TIMEOUT,
            stdout_cap: cap,
        };
        let output = tools::run(&self.path, &command, &ENV, limits).ok()?;
        (output.code == Some(0) && !output.overflowed).then_some(output.stdout)
    }

    fn line(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let out = self.ask(dir, args, 64 * 1024)?;
        let text = String::from_utf8(out).ok()?;
        let line = text.trim_end_matches(['\n', '\r']);
        (!line.is_empty() && !line.contains('\n')).then(|| line.to_owned())
    }

    /// The top of the work tree `dir` is in; `None` outside one.
    pub fn toplevel(&self, dir: &Path) -> Option<PathBuf> {
        self.line(dir, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    /// The URL of `origin`, else of the only remote; `None` with no remote
    /// or several and no `origin`.
    pub fn remote_url(&self, top: &Path) -> Option<String> {
        let names = String::from_utf8(self.ask(top, &["remote"], 64 * 1024)?).ok()?;
        let names: Vec<&str> = names.lines().filter(|n| !n.is_empty()).collect();
        let name = match names.as_slice() {
            [only] => *only,
            _ => *names.iter().find(|n| **n == "origin")?,
        };
        self.line(top, &["remote", "get-url", name])
    }

    /// The commit `HEAD` names.
    pub fn head(&self, top: &Path) -> Option<String> {
        self.line(top, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
    }

    /// The blob `path` is at `HEAD`, and the path as git spells it; `None`
    /// when `HEAD` has no file there.
    pub fn committed(&self, top: &Path, path: &str) -> Option<(String, String)> {
        let out = self.ask(
            top,
            &["ls-tree", "-z", "--full-name", "HEAD", "--", path],
            64 * 1024,
        )?;
        let mut entries = entries(&out);
        let entry = entries.next()?;
        entries.next().is_none().then_some(entry)
    }

    /// Whether `path` differs from `HEAD` in the index or the work tree, or
    /// git does not know it; `None` when git could not say.
    pub fn changed(&self, top: &Path, path: &str) -> Option<bool> {
        let out = self.ask(
            top,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--",
                path,
            ],
            64 * 1024,
        )?;
        Some(!out.is_empty())
    }

    /// Every file at `HEAD`: its path as git spells it, to its blob.
    pub fn tree(&self, top: &Path) -> Option<HashMap<String, String>> {
        let out = self.ask(
            top,
            &["ls-tree", "-r", "-z", "--full-tree", "HEAD"],
            LISTING_CAP,
        )?;
        Some(entries(&out).map(|(blob, path)| (path, blob)).collect())
    }

    /// The bytes of blob `id`, when this checkout has it and it fits the
    /// body cap.
    pub fn blob(&self, top: &Path, id: &str) -> Option<Vec<u8>> {
        if !is_object_id(id) {
            return None;
        }
        self.ask(top, &["cat-file", "blob", id], BODY_CAP)
    }
}

/// `ls-tree -z` entries that are blobs: `<mode> blob <id>\t<path>\0`.
fn entries(out: &[u8]) -> impl Iterator<Item = (String, String)> + '_ {
    out.split(|&b| b == 0).filter_map(|entry| {
        let entry = std::str::from_utf8(entry).ok()?;
        let (meta, path) = entry.split_once('\t')?;
        let mut meta = meta.split(' ');
        let (_mode, kind, id) = (meta.next()?, meta.next()?, meta.next()?);
        (kind == "blob" && is_object_id(id)).then(|| (id.to_owned(), path.to_owned()))
    })
}

/// A full SHA-1 or SHA-256 object id, lowercase hex.
pub fn is_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_blob_entries_with_full_ids_are_read() {
        let blob = "a".repeat(40);
        let listing = format!(
            "100644 blob {blob}\tnotes/a b.md\0040000 tree {blob}\tnotes\0160000 commit {blob}\tsub\0100644 blob short\tx\0"
        );
        let read: Vec<_> = entries(listing.as_bytes()).collect();
        assert_eq!(read, vec![(blob, "notes/a b.md".to_owned())]);
    }

    #[test]
    fn object_ids_are_full_lowercase_hex() {
        assert!(is_object_id(&"0".repeat(40)));
        assert!(is_object_id(&"f".repeat(64)));
        assert!(!is_object_id(&"F".repeat(40)));
        assert!(!is_object_id("--help"));
        assert!(!is_object_id(&"0".repeat(39)));
    }
}
