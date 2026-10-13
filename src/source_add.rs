//! `knowmoretabs add --file`: one local document into the library by
//! reference, a committed file in a repository by its portable key, a
//! loose file or pasted text by its hash.
//!
//! slice: sources
//! why: Research notes and papers belong beside the pages they were read
//!      with, but the archive is not their home and must not hold a copy or
//!      a local path. So a committed file joins as its repository's name and
//!      path, pinned to the blob and commit git has for it; a file with
//!      changes, or not committed, is refused, because a pin to bytes only
//!      this machine has resolves nowhere else. A file outside a repository
//!      with a remote, or text on stdin, is copied once to the shelf the
//!      owner names, under its hash, and joins by that hash; with no shelf
//!      it is refused. The pin and the intake line are one hold of the
//!      archive lock, the pin first, so a listed document always has one;
//!      adding again writes a pin only when the bytes changed. The checkout
//!      a repository was found in is written to the resolver file when it
//!      names none, so search and `read` find it later. The page is then any
//!      added page: listed, tagged, forgotten and ranked like the rest, and
//!      never sent anywhere, since no web rule takes its key.

use std::cell::Cell;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;

use crate::add;
use crate::add_content::Ended;
use crate::archive;
use crate::capture::Log;
use crate::content_fetch::BODY_CAP;
use crate::digest::sha256_hex;
use crate::error::Error;
use crate::jsonl::Appender;
use crate::source_git::Git;
use crate::source_key;
use crate::sources::{self, Checkouts, Format, Pin};

/// What `add --file` was asked for.
#[derive(Debug, Clone, Copy)]
pub struct Args<'a> {
    /// The document, or `-` for text on stdin.
    pub file: &'a Path,
    pub title: Option<&'a str>,
    /// The resolver file.
    pub checkouts: &'a Path,
}

/// The document read whole: its bytes, format and text, and what it is
/// called in a message. A message never names its folder.
struct Document {
    /// The canonical path; `None` for stdin.
    path: Option<PathBuf>,
    name: String,
    format: Format,
    bytes: Vec<u8>,
    title: Option<String>,
}

/// Where the document's bytes are kept.
enum Home {
    /// Committed at `path` in `repo`, whose checkout is `top`.
    Repo {
        repo: String,
        path: String,
        blob: String,
        commit: String,
        top: PathBuf,
    },
    /// On the shelf, by its hash.
    Shelf { sha256: String },
}

/// `knowmoretabs add --file`. Success when the document is in the library
/// at the end; a forgotten one is failure, as for an address.
pub fn command(root: &Path, args: Args<'_>, json: bool, log: Log) -> Result<ExitCode, Error> {
    let doc = Document::read(args.file)?;
    let refused = |reason: &str| Error::SourceFile {
        file: doc.name.clone(),
        reason: reason.to_owned(),
    };
    let checkouts = Checkouts::read(args.checkouts)?;
    let home = home(&doc, &checkouts).map_err(refused)?;
    let (key, pin) = match &home {
        Home::Repo {
            repo,
            path,
            blob,
            commit,
            ..
        } => {
            let key = source_key::git(repo, path);
            let mut pin = Pin::new(&key, doc.format, archive::now());
            pin.repo = Some(repo.clone());
            pin.path = Some(path.clone());
            pin.blob = Some(blob.clone());
            pin.commit = Some(commit.clone());
            (key, pin)
        }
        Home::Shelf { sha256 } => {
            shelve(&checkouts, sha256, &doc).map_err(refused)?;
            (
                source_key::blob(sha256),
                Pin::new(&source_key::blob(sha256), doc.format, archive::now()),
            )
        }
    };
    // A file is called by its name when it names itself nothing; pasted
    // text by nothing.
    let title = args
        .title
        .or(doc.title.as_deref())
        .or(doc.path.is_some().then_some(doc.name.as_str()));
    let stages = add::Stages::start(json, log);
    let pinned = Cell::new(false);
    let outcome = add::admit(root, &key, title, false, stages.waiting(), |lock| {
        let pins = sources::pins(root)?;
        if pins
            .pages
            .get(&key)
            .is_some_and(|held| held.blob == pin.blob)
        {
            return Ok(());
        }
        Appender::open_locked(root, sources::path(root), lock)?.append_locked(&pin, lock)?;
        pinned.set(true);
        Ok(())
    })?;
    if let (Home::Repo { repo, top, .. }, true) = (&home, outcome.in_library()) {
        remember(args.checkouts, repo, top)?;
    }
    stages.library(&key, &outcome, pinned.get());
    Ok(stages.done(
        &key,
        &outcome,
        &Ended::default(),
        json!({"pinned": pinned.get(), "source": pin.source()}),
    ))
}

impl Document {
    fn read(file: &Path) -> Result<Self, Error> {
        if file == Path::new("-") {
            let mut bytes = Vec::new();
            let refused = |reason: &str| Error::SourceFile {
                file: "stdin".to_owned(),
                reason: reason.to_owned(),
            };
            std::io::stdin()
                .lock()
                .take(BODY_CAP as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| refused("cannot read stdin"))?;
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Err(refused("no text on stdin"));
            }
            return Self::of(None, "stdin".to_owned(), Format::Md, bytes);
        }
        let name = file.file_name().map_or_else(
            || file.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let refused = |reason: &str| Error::SourceFile {
            file: name.clone(),
            reason: reason.to_owned(),
        };
        let path = std::fs::canonicalize(file).map_err(|_| refused("no such file"))?;
        if !path.is_file() {
            return Err(refused("not a file"));
        }
        let format = Format::of(&path)
            .ok_or_else(|| refused("not markdown, text or PDF (.md, .txt, .pdf)"))?;
        let bytes = std::fs::read(&path).map_err(|_| refused("cannot read it"))?;
        Self::of(Some(path), name, format, bytes)
    }

    /// The document, when its bytes have text in its format.
    fn of(
        path: Option<PathBuf>,
        name: String,
        format: Format,
        bytes: Vec<u8>,
    ) -> Result<Self, Error> {
        let text = format.text(&bytes).map_err(|reason| Error::SourceFile {
            file: name.clone(),
            reason: reason.to_owned(),
        })?;
        Ok(Self {
            path,
            name,
            format,
            bytes,
            title: text.title,
        })
    }
}

/// Where the document is kept: committed in a repository with a portable
/// name, else on the shelf; or why it cannot be added.
fn home(doc: &Document, checkouts: &Checkouts) -> Result<Home, &'static str> {
    if let Some(found) = doc.path.as_deref().and_then(repo_of) {
        let (git, top, repo, relative) = found;
        let (blob, path) = git
            .committed(&top, &relative)
            .ok_or("it is not committed; commit it first")?;
        if git.changed(&top, &relative) != Some(false) {
            return Err("it has changes not committed; commit them first");
        }
        let commit = git.head(&top).ok_or("its repository has no commit")?;
        return Ok(Home::Repo {
            repo,
            path,
            blob,
            commit,
            top,
        });
    }
    if checkouts.shelf.is_none() {
        return Err(
            "it is in no git repository with a remote, and no shelf is set for loose files; set \"shelf\" in checkouts.json",
        );
    }
    Ok(Home::Shelf {
        sha256: sha256_hex(&doc.bytes),
    })
}

/// The repository `file` is in, when git knows one with a remote that has
/// a portable name: git, the checkout's top, the name, and the file's path
/// from the top, `/` separated.
fn repo_of(file: &Path) -> Option<(Git, PathBuf, String, String)> {
    let git = Git::find()?;
    let top = std::fs::canonicalize(git.toplevel(file.parent()?)?).ok()?;
    let repo = source_key::repo_id(&git.remote_url(&top)?)?;
    let relative = file
        .strip_prefix(&top)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?
        .join("/");
    Some((git, top, repo, relative))
}

/// Copies the document to the shelf as `<sha256>.<ext>`, once: a copy
/// already there is the same bytes by its name.
fn shelve(checkouts: &Checkouts, sha256: &str, doc: &Document) -> Result<(), &'static str> {
    let shelf = checkouts.shelf.as_ref().ok_or("no shelf is set")?;
    let copy = shelf.join(format!("{sha256}.{}", doc.format.ext()));
    if copy.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(shelf).map_err(|_| "cannot create the shelf")?;
    archive::replace_file(&copy, &doc.bytes).map_err(|_| "cannot copy it to the shelf")
}

/// Names `top` as `repo`'s checkout in the resolver file when it names
/// none, read again just before so an edit since is kept.
fn remember(file: &Path, repo: &str, top: &Path) -> Result<(), Error> {
    let mut checkouts = Checkouts::read(file)?;
    if checkouts.repos.contains_key(repo) {
        return Ok(());
    }
    checkouts.repos.insert(repo.to_owned(), top.to_owned());
    checkouts.write(file)
}
