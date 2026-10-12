//! `knowmoretabs add`: one page into the library by its address, and the
//! stage lines a caller follows it by.
//!
//! slice: add
//! why: Saving a whole session to keep the one page you are reading is the
//!      wrong cost, and a page with no session has no snapshot to join. So
//!      `add` writes one line to the intake log, which the library folds in,
//!      and only when the page is new: run again, it writes nothing. A new
//!      page is kept under its cleaned address (`page_key`), and a page the
//!      library holds under any address that cleans the same is that page,
//!      so a link with an anchor or a tracking tag adds no duplicate; every
//!      later stage, and the caller, use the key the library holds. What
//!      the URL alone rules out (not the web, this machine or the private
//!      network, what the library would not list) is refused before the
//!      archive is touched, and a forgotten page stays forgotten until
//!      `restore`. The check and the line are one hold of the archive lock,
//!      so two adds of one page write it once. A page in the library then
//!      gets its text and image (`add_content`), unless `--no-content`, and
//!      a title its text was kept with when its line had none. A Retry
//!      (`--retry`) is for a page already listed: it writes no line, and a
//!      page the library does not hold is refused, under the same hold of
//!      the lock, so a retry can never add a page. Under
//!      `--json` each stage is one line as it happens, so a caller can show
//!      progress, and the last line says how it ended.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};
use url::Url;

use crate::add_content::{self, Ask, Ended};
use crate::archive::{self, Archive};
use crate::capture::Log;
use crate::error::Error;
use crate::jsonl::Appender;
use crate::library::{self, State};
use crate::{guard, intake, out, page_key};

/// Why a URL was refused, nothing written: its `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Not an `http` or `https` URL with a host, or not a URL at all.
    NotWeb,
    /// This machine or the private network, by the fetcher's rule.
    Private,
    /// A page the library would not list. Every such URL is refused above
    /// today; this keeps "added" meaning "listed" if the listing rule grows.
    Hidden,
    /// A Retry of a page the library does not hold.
    NotInLibrary,
}

impl Reason {
    fn name(self) -> &'static str {
        match self {
            Self::NotWeb => "not_web",
            Self::Private => "private",
            Self::Hidden => "hidden",
            Self::NotInLibrary => "not_in_library",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::NotWeb => "not a web page address (http or https)",
            Self::Private => "on this machine or the private network",
            Self::Hidden => "not a page the library lists",
            Self::NotInLibrary => "not in your library; add it before a retry",
        }
    }
}

/// How the `library` stage ended: its `value`, and the key the library
/// holds the page under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A new page, and its intake line written.
    Added(String),
    /// Already in the library; nothing written.
    Known(String),
    /// Forgotten; `restore` brings it back. Nothing written.
    Forgotten(String),
    Refused(Reason),
}

impl Outcome {
    fn value(&self) -> &'static str {
        match self {
            Self::Added(_) => "added",
            Self::Known(_) => "known",
            Self::Forgotten(_) => "forgotten",
            Self::Refused(_) => "refused",
        }
    }

    /// The key the library holds the page under; none for a refusal.
    fn key(&self) -> Option<&str> {
        match self {
            Self::Added(key) | Self::Known(key) | Self::Forgotten(key) => Some(key),
            Self::Refused(_) => None,
        }
    }

    /// The page is in the library at the end: the run succeeded.
    fn in_library(&self) -> bool {
        matches!(self, Self::Added(_) | Self::Known(_))
    }
}

/// What the URL alone rules out, before the archive is touched, by the
/// rules the fetcher and the library already apply; `None` when it may be
/// added.
pub fn refusal(url: &str) -> Option<Reason> {
    let Some(parsed) = Url::parse(url).ok().filter(guard::is_web) else {
        return Some(Reason::NotWeb);
    };
    if guard::is_private_host(&parsed) {
        Some(Reason::Private)
    } else if !library::is_listed(url) {
        Some(Reason::Hidden)
    } else {
        None
    }
}

/// Puts `url` in the library, under its cleaned key, unless the library
/// holds it already (`held`). The check and the write are one hold of the
/// archive lock; `waiting` is called once if another run holds it.
/// `known_only`, for a Retry, refuses a page the library does not hold
/// instead of adding it.
pub fn apply(
    root: &Path,
    url: &str,
    title: Option<&str>,
    known_only: bool,
    waiting: impl FnOnce(),
) -> Result<Outcome, Error> {
    if let Some(reason) = refusal(url) {
        return Ok(Outcome::Refused(reason));
    }
    // No archive holds no page, and opening one would create it.
    if known_only && !root.exists() {
        return Ok(Outcome::Refused(Reason::NotInLibrary));
    }
    let archive = Archive::open(root)?;
    let lock = archive.lock(waiting)?;
    let key = page_key::clean(url);
    let forgotten = State::read(root)?.forgotten;
    let loaded = library::load(&archive)?;
    let known = library::known_urls(&loaded.snapshots);
    if let Some(held) = held(url, &key, &known, &forgotten) {
        return Ok(held);
    }
    if known_only {
        return Ok(Outcome::Refused(Reason::NotInLibrary));
    }
    let mut log = Appender::open_locked(root, intake::path(root), &lock)?;
    log.append_locked(&intake::Line::new(&key, archive::now(), title), &lock)?;
    Ok(Outcome::Added(key.into_owned()))
}

/// The page the library holds for `url`, first match wins: `url` itself,
/// then its cleaned `key`, forgotten before known; then any key that cleans
/// to `key`, a visible one before a forgotten one, the smallest of several.
/// So an address that resolves today resolves to itself, and a forgotten
/// variant keeps the page forgotten unless a visible one lists it.
fn held(
    url: &str,
    key: &str,
    known: &HashSet<&str>,
    forgotten: &BTreeSet<String>,
) -> Option<Outcome> {
    for exact in [url, key] {
        if forgotten.contains(exact) {
            return Some(Outcome::Forgotten(exact.to_owned()));
        }
        if known.contains(exact) {
            return Some(Outcome::Known(exact.to_owned()));
        }
    }
    let variants: BTreeSet<&str> = known
        .iter()
        .copied()
        .chain(forgotten.iter().map(String::as_str))
        .filter(|held| page_key::clean(held) == key)
        .collect();
    match variants.iter().find(|held| !forgotten.contains(**held)) {
        Some(visible) => Some(Outcome::Known((*visible).to_owned())),
        None => variants
            .first()
            .map(|held| Outcome::Forgotten((*held).to_owned())),
    }
}

/// What `add` was asked for.
#[derive(Debug, Clone, Copy)]
pub struct Args<'a> {
    pub url: &'a str,
    pub title: Option<&'a str>,
    /// How to capture the page's text and image; `None` for none.
    pub content: Option<Ask<'a>>,
}

/// `knowmoretabs add`. The exit status is the run's: success when the page
/// is in the library at the end, however its text and image ended, failure
/// when it was refused.
pub fn command(root: &Path, args: Args<'_>, json: bool, log: Log) -> Result<ExitCode, Error> {
    // Surrounding whitespace is what a paste brings, never part of an address.
    let url = args.url.trim();
    if json {
        out::json(&library_line("running"));
    }
    let retry = args.content.is_some_and(|ask| !ask.retry.is_empty());
    let outcome = apply(root, url, args.title, retry, || {
        if json {
            out::json(&library_line("waiting"));
        } else {
            log.warn(archive::WAITING);
        }
    })?;
    if json {
        out::json(&library_done(&outcome));
    }
    // What follows is about the page the library holds, by its key.
    let url = outcome.key().unwrap_or(url);
    if !json {
        report(url, &outcome, args.content.is_some(), log);
    }
    let ended = match args.content {
        Some(ask) if outcome.in_library() => add_content::capture(root, url, ask, json, log),
        _ => Ended::default(),
    };
    if let Some(title) = &ended.title
        && let Err(err) = intake::retitle(root, url, title, || {
            if !json {
                log.warn(archive::WAITING);
            }
        })
    {
        out::problem(&format!("knowmoretabs: {err}"));
    }
    if json {
        out::json(&done_line(url, &outcome, &ended));
    }
    Ok(if outcome.in_library() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn library_line(state: &str) -> Value {
    json!({"stage": "library", "state": state})
}

/// How the library stage ended, and the key the library holds the page
/// under, so a caller has it before any later stage.
fn library_done(outcome: &Outcome) -> Value {
    let mut line = library_line("done");
    line["value"] = outcome.value().into();
    if let Some(key) = outcome.key() {
        line["url"] = key.into();
    }
    if let Outcome::Refused(reason) = outcome {
        line["reason"] = reason.name().into();
    }
    line
}

/// The last line: the page's key (the asked address on a refusal), how the
/// library stage ended, and the text's and the image's status names, null
/// when there is none.
fn done_line(url: &str, outcome: &Outcome, ended: &Ended) -> Value {
    json!({"stage": "done", "state": "done", "url": url, "value": outcome.value(),
        "content": ended.content, "image": ended.image})
}

/// A page in the library is news on stdout; a refusal is the error the run
/// exits on, on stderr whatever `-q` says, as every command's is. A known
/// page is left as it was only when nothing more is captured.
fn report(url: &str, outcome: &Outcome, capturing: bool, log: Log) {
    match outcome {
        Outcome::Added(_) if !log.quiet => out::line(&format!("added {url} to your library")),
        Outcome::Known(_) if !log.quiet && capturing => {
            out::line(&format!("already in your library: {url}"));
        }
        Outcome::Known(_) if !log.quiet => {
            out::line(&format!("already in your library: {url}; nothing changed"));
        }
        Outcome::Added(_) | Outcome::Known(_) => {}
        Outcome::Forgotten(_) => out::problem(&format!(
            "knowmoretabs: you forgot {url}; nothing changed; knowmoretabs restore brings it back"
        )),
        Outcome::Refused(Reason::NotInLibrary) => out::problem(&format!(
            "knowmoretabs: not retried: {url} is {}",
            Reason::NotInLibrary.describe()
        )),
        Outcome::Refused(reason) => out::problem(&format!(
            "knowmoretabs: not added: {url} is {}",
            reason.describe()
        )),
    }
}

#[cfg(test)]
#[path = "add_tests.rs"]
mod tests;
