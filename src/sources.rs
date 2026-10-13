//! Local documents kept by reference: the pins log `pages/sources.jsonl`,
//! the machine's `checkouts.json`, and the resolver from a key to the
//! pinned bytes and their text.
//!
//! slice: sources
//! why: A note in a repository or a paper on disk is the owner's own
//!      document, and the archive keeps no copy of it, only which bytes it
//!      was: the git blob and commit, or the hash of a loose file. Where
//!      those bytes live differs per machine, so it is not in the archive
//!      but in one file per machine outside it, naming each repository's
//!      checkout and the shelf loose files are copied to; nothing prints it.
//!      Resolving reads the pinned bytes back, live, on every read, and says
//!      whether they are still current (`fresh`), behind the checkout's
//!      `HEAD` (`stale`) or out of reach (`unresolved`), so no answer is
//!      silently missing or silently outdated. Text is read from the bytes
//!      in memory, a PDF through content's own reader, and never written.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::content_fetch::BODY_CAP;
use crate::digest::sha256_hex;
use crate::error::Error;
use crate::jsonl::{self, Keyed};
use crate::page_text::Text;
use crate::source_git::Git;
use crate::source_key::{self, Key};
use crate::{archive, content_pdf, metadata};

pub const FILE: &str = "sources.jsonl";
pub const SCHEMA_VERSION: u32 = 1;
/// The resolver's file, in the knowmoretabs folder of the config home.
pub const CHECKOUTS_FILE: &str = "checkouts.json";
const CHECKOUTS_SCHEMA_VERSION: u32 = 1;

pub fn path(root: &Path) -> PathBuf {
    root.join(metadata::DIR).join(FILE)
}

/// What a document is, by its file name's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Md,
    Txt,
    Pdf,
}

impl Format {
    /// The format `path`'s extension names; `None` for any other file.
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" => Some(Self::Md),
            "txt" | "text" => Some(Self::Txt),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    /// The extension a shelf copy is named with.
    pub fn ext(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Pdf => "pdf",
        }
    }

    /// The text of `bytes` in this format, or why there is none. Markdown
    /// names itself by its first `# ` heading, a PDF by its Info title.
    pub fn text(self, bytes: &[u8]) -> Result<Text, &'static str> {
        if bytes.len() >= BODY_CAP {
            return Err("over 10 MB");
        }
        match self {
            Self::Pdf => content_pdf::read(bytes).map(|page| Text {
                title: page.title,
                body: page.markdown,
            }),
            Self::Md | Self::Txt => {
                let body = String::from_utf8(bytes.to_vec()).map_err(|_| "not UTF-8 text")?;
                let title = (self == Self::Md)
                    .then(|| {
                        body.lines()
                            .find_map(|line| line.strip_prefix("# "))
                            .map(str::trim)
                            .filter(|title| !title.is_empty())
                            .map(ToOwned::to_owned)
                    })
                    .flatten();
                Ok(Text { title, body })
            }
        }
    }
}

/// Which bytes a source key named when it was added. The latest line for a
/// key wins. A loose file's key is its hash, so its line names no
/// repository, path, blob or commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    #[serde(deserialize_with = "jsonl::schema::<_, SCHEMA_VERSION>")]
    pub schema_version: u32,
    pub url: String,
    pub kind: Format,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub blob: Option<String>,
    pub commit: Option<String>,
    pub added_at: Timestamp,
}

impl Keyed for Pin {
    fn key(&self) -> &str {
        &self.url
    }
}

impl Pin {
    pub fn new(url: &str, kind: Format, added_at: Timestamp) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            url: url.to_owned(),
            kind,
            repo: None,
            path: None,
            blob: None,
            commit: None,
            added_at,
        }
    }

    /// The pin as a result names its source: no time, nothing local.
    pub fn source(&self) -> Value {
        json!({"repo": self.repo, "path": self.path, "blob": self.blob, "commit": self.commit})
    }
}

/// The pins log; a missing file is an empty log.
pub fn pins(root: &Path) -> Result<jsonl::Latest<Pin>, Error> {
    jsonl::read(&path(root), "read the sources log")
}

/// The machine's resolver file: each repository's checkout, by its
/// portable name, and the shelf loose files are kept on. Fields a later
/// build adds survive a rewrite.
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkouts {
    schema_version: u32,
    #[serde(default)]
    pub repos: BTreeMap<String, PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelf: Option<PathBuf>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

impl Default for Checkouts {
    fn default() -> Self {
        Self {
            schema_version: CHECKOUTS_SCHEMA_VERSION,
            repos: BTreeMap::new(),
            shelf: None,
            extra: serde_json::Map::new(),
        }
    }
}

impl Checkouts {
    /// The file at `path`; none there reads as empty. Its errors never name
    /// a path: the file's lines are checkouts.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(Error::Checkouts(format!("cannot read it: {}", err.kind()))),
        };
        let checkouts: Self = serde_json::from_slice(&bytes)
            .map_err(|err| Error::Checkouts(format!("not valid: {err}")))?;
        if checkouts.schema_version != CHECKOUTS_SCHEMA_VERSION {
            return Err(Error::Checkouts("unsupported schema_version".to_owned()));
        }
        Ok(checkouts)
    }

    /// Replaces the file whole, staged beside it and renamed.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| {
                Error::Checkouts(format!("cannot create its folder: {}", err.kind()))
            })?;
        }
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|source| Error::Json {
            path: PathBuf::from(CHECKOUTS_FILE),
            source,
        })?;
        bytes.push(b'\n');
        archive::replace_file(path, &bytes)
    }
}

/// Whether a pin's bytes are still the document's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The checkout's `HEAD`, or the shelf, has the pinned bytes.
    Fresh,
    /// The checkout's `HEAD` has other bytes at the path, or none; the
    /// pinned bytes are still read.
    Stale,
    /// The pinned bytes cannot be read here.
    Unresolved,
}

impl State {
    /// The state's name, as results write it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unresolved => "unresolved",
        }
    }
}

/// A pin resolved: its state, why when it is not fresh, and its text when
/// the bytes could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub state: State,
    pub why: Option<&'static str>,
    pub text: Option<Text>,
}

impl Resolved {
    fn unresolved(why: &'static str) -> Self {
        Self {
            state: State::Unresolved,
            why: Some(why),
            text: None,
        }
    }
}

/// Resolves pins against this machine's checkouts, asking git for each
/// repository's `HEAD` once.
pub struct Resolver {
    checkouts: Checkouts,
    git: Option<Git>,
    trees: HashMap<String, Option<HashMap<String, String>>>,
}

impl Resolver {
    pub fn new(checkouts: Checkouts) -> Self {
        Self {
            checkouts,
            git: Git::find(),
            trees: HashMap::new(),
        }
    }

    /// `pin`'s bytes and their state; a source key with no pin is
    /// unresolved.
    pub fn resolve(&mut self, pin: Option<&Pin>) -> Resolved {
        let Some(pin) = pin else {
            return Resolved::unresolved("no pin; add the file again");
        };
        let (state, why, bytes) = match source_key::parse(&pin.url) {
            Some(Key::Git { repo, path }) => match self.git_bytes(pin, &repo, &path) {
                Ok(found) => found,
                Err(why) => return Resolved::unresolved(why),
            },
            Some(Key::Blob { sha256 }) => match self.shelf_bytes(pin.kind, &sha256) {
                Ok(bytes) => (State::Fresh, None, bytes),
                Err(why) => return Resolved::unresolved(why),
            },
            None => return Resolved::unresolved("not a source key"),
        };
        match pin.kind.text(&bytes) {
            Ok(text) => Resolved {
                state,
                why,
                text: Some(text),
            },
            Err(why) => Resolved::unresolved(why),
        }
    }

    fn git_bytes(
        &mut self,
        pin: &Pin,
        repo: &str,
        path: &str,
    ) -> Result<(State, Option<&'static str>, Vec<u8>), &'static str> {
        let dir = self
            .checkouts
            .repos
            .get(repo)
            .ok_or("no checkout of its repository in checkouts.json")?;
        if !dir.is_dir() {
            return Err("its checkout is missing");
        }
        let git = self.git.as_ref().ok_or("git is not installed")?;
        let blob = pin.blob.as_deref().ok_or("its pin names no blob")?;
        let tree = self
            .trees
            .entry(repo.to_owned())
            .or_insert_with(|| git.tree(dir))
            .as_ref()
            .ok_or("its checkout is not a git work tree with a commit")?;
        let (state, why) = match tree.get(path) {
            Some(current) if current == blob => (State::Fresh, None),
            Some(_) => (State::Stale, Some("changed at HEAD since it was added")),
            None => (State::Stale, Some("gone from HEAD since it was added")),
        };
        let bytes = git
            .blob(dir, blob)
            .ok_or("the pinned blob is not in its checkout")?;
        Ok((state, why, bytes))
    }

    fn shelf_bytes(&self, kind: Format, sha256: &str) -> Result<Vec<u8>, &'static str> {
        let dir = self
            .checkouts
            .shelf
            .as_ref()
            .ok_or("no shelf in checkouts.json")?;
        let file = dir.join(format!("{sha256}.{}", kind.ext()));
        let bytes = jsonl::kept_file(&file)
            .ok()
            .flatten()
            .ok_or("not on the shelf")?;
        if sha256_hex(&bytes) != sha256 {
            return Err("its shelf copy changed");
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_is_named_by_its_extension() {
        assert_eq!(Format::of(Path::new("a/Notes.MD")), Some(Format::Md));
        assert_eq!(Format::of(Path::new("a.markdown")), Some(Format::Md));
        assert_eq!(Format::of(Path::new("a.txt")), Some(Format::Txt));
        assert_eq!(Format::of(Path::new("a.pdf")), Some(Format::Pdf));
        assert_eq!(Format::of(Path::new("a.html")), None);
        assert_eq!(Format::of(Path::new("README")), None);
    }

    #[test]
    fn markdown_names_itself_by_its_first_heading() {
        let text = Format::Md
            .text(b"intro\n## Not this\n# The title \nbody\n")
            .unwrap();
        assert_eq!(text.title.as_deref(), Some("The title"));
        assert_eq!(
            Format::Txt.text(b"# not a heading here\n").unwrap().title,
            None
        );
        assert_eq!(Format::Md.text(&[0xff, 0xfe]), Err("not UTF-8 text"));
    }

    #[test]
    fn a_checkouts_file_keeps_what_a_later_build_added() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("knowmoretabs").join(CHECKOUTS_FILE);
        assert!(Checkouts::read(&file).unwrap().repos.is_empty());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"schema_version":1,"repos":{},"shelf":"/s","later":true}"#,
        )
        .unwrap();
        let mut checkouts = Checkouts::read(&file).unwrap();
        checkouts
            .repos
            .insert("example.test/o/r".to_owned(), PathBuf::from("/c"));
        checkouts.write(&file).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["later"], true);
        assert_eq!(written["shelf"], "/s");
        assert_eq!(written["repos"]["example.test/o/r"], "/c");
        std::fs::write(&file, r#"{"schema_version":2}"#).unwrap();
        assert!(matches!(Checkouts::read(&file), Err(Error::Checkouts(_))));
    }

    #[test]
    fn a_state_says_its_word_as_json_does() {
        for state in [State::Fresh, State::Stale, State::Unresolved] {
            assert_eq!(json!(state), state.word());
        }
    }

    #[test]
    fn a_loose_pin_resolves_from_the_shelf_only_while_its_hash_holds() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"# Shelf note\nwords\n";
        let sha = sha256_hex(bytes);
        let pin = Pin::new(&source_key::blob(&sha), Format::Md, archive::now());
        let mut checkouts = Checkouts::default();
        assert_eq!(
            Resolver::new(Checkouts::default()).resolve(Some(&pin)),
            Resolved::unresolved("no shelf in checkouts.json")
        );
        checkouts.shelf = Some(dir.path().to_owned());
        let mut resolver = Resolver::new(checkouts);
        assert_eq!(resolver.resolve(Some(&pin)).why, Some("not on the shelf"));
        let copy = dir.path().join(format!("{sha}.md"));
        std::fs::write(&copy, bytes).unwrap();
        let fresh = resolver.resolve(Some(&pin));
        assert_eq!(fresh.state, State::Fresh);
        assert_eq!(fresh.text.unwrap().title.as_deref(), Some("Shelf note"));
        std::fs::write(&copy, b"edited").unwrap();
        assert_eq!(
            resolver.resolve(Some(&pin)).why,
            Some("its shelf copy changed")
        );
        assert_eq!(resolver.resolve(None).state, State::Unresolved);
    }
}
