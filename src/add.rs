//! `knowmoretabs add`: one page into the library by its address, and the
//! stage lines a caller follows it by.
//!
//! slice: add
//! why: Saving a whole session to keep the one page you are reading is the
//!      wrong cost, and a page with no session has no snapshot to join. So
//!      `add` writes one line to the intake log, which the library folds in,
//!      and only when the page is new: run again, it writes nothing. What
//!      the URL alone rules out (not the web, this machine or the private
//!      network, what the library would not list) is refused before the
//!      archive is touched, and a forgotten page stays forgotten until
//!      `restore`. The check and the line are one hold of the archive lock,
//!      so two adds of one page write it once. A page in the library then
//!      gets its text and image (`add_content`), unless `--no-content`, and
//!      a title its text was kept with when its line had none. Under
//!      `--json` each stage is one line as it happens, so a caller can show
//!      progress, and the last line says how it ended.

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
use crate::{guard, intake, out};

/// Why a URL was refused before the archive was read: its `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Not an `http` or `https` URL with a host, or not a URL at all.
    NotWeb,
    /// This machine or the private network, by the fetcher's rule.
    Private,
    /// A page the library would not list. Every such URL is refused above
    /// today; this keeps "added" meaning "listed" if the listing rule grows.
    Hidden,
}

impl Reason {
    fn name(self) -> &'static str {
        match self {
            Self::NotWeb => "not_web",
            Self::Private => "private",
            Self::Hidden => "hidden",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::NotWeb => "not a web page address (http or https)",
            Self::Private => "on this machine or the private network",
            Self::Hidden => "not a page the library lists",
        }
    }
}

/// How the `library` stage ended: its `value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new page, and its intake line written.
    Added,
    /// Already in the library; nothing written.
    Known,
    /// Forgotten; `restore` brings it back. Nothing written.
    Forgotten,
    Refused(Reason),
}

impl Outcome {
    fn value(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Known => "known",
            Self::Forgotten => "forgotten",
            Self::Refused(_) => "refused",
        }
    }

    /// The page is in the library at the end: the run succeeded.
    fn in_library(self) -> bool {
        matches!(self, Self::Added | Self::Known)
    }
}

/// What the URL alone rules out, by the rules the fetcher and the library
/// already apply; `None` when it may be added.
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

/// Puts `url` in the library unless it is there already. The check and the
/// write are one hold of the archive lock; `waiting` is called once if
/// another run holds it.
pub fn apply(
    root: &Path,
    url: &str,
    title: Option<&str>,
    waiting: impl FnOnce(),
) -> Result<Outcome, Error> {
    if let Some(reason) = refusal(url) {
        return Ok(Outcome::Refused(reason));
    }
    let archive = Archive::open(root)?;
    let lock = archive.lock(waiting)?;
    if State::read(root)?.forgotten.contains(url) {
        return Ok(Outcome::Forgotten);
    }
    let loaded = library::load(&archive)?;
    if library::known_urls(&loaded.snapshots).contains(url) {
        return Ok(Outcome::Known);
    }
    let mut log = Appender::open_locked(root, intake::path(root), &lock)?;
    log.append_locked(&intake::Line::new(url, archive::now(), title), &lock)?;
    Ok(Outcome::Added)
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
    let outcome = apply(root, url, args.title, || {
        if json {
            out::json(&library_line("waiting"));
        } else {
            log.warn(archive::WAITING);
        }
    })?;
    if json {
        out::json(&library_done(outcome));
    } else {
        report(url, outcome, args.content.is_some(), log);
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
        out::json(&done_line(url, outcome, &ended));
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

fn library_done(outcome: Outcome) -> Value {
    let mut line = library_line("done");
    line["value"] = outcome.value().into();
    if let Outcome::Refused(reason) = outcome {
        line["reason"] = reason.name().into();
    }
    line
}

/// The last line: how the library stage ended, and the text's and the
/// image's status names, null when there is none.
fn done_line(url: &str, outcome: Outcome, ended: &Ended) -> Value {
    json!({"stage": "done", "state": "done", "url": url, "value": outcome.value(),
        "content": ended.content, "image": ended.image})
}

/// A page in the library is news on stdout; a refusal is the error the run
/// exits on, on stderr whatever `-q` says, as every command's is. A known
/// page is left as it was only when nothing more is captured.
fn report(url: &str, outcome: Outcome, capturing: bool, log: Log) {
    match outcome {
        Outcome::Added if !log.quiet => out::line(&format!("added {url} to your library")),
        Outcome::Known if !log.quiet && capturing => {
            out::line(&format!("already in your library: {url}"));
        }
        Outcome::Known if !log.quiet => {
            out::line(&format!("already in your library: {url}; nothing changed"));
        }
        Outcome::Added | Outcome::Known => {}
        Outcome::Forgotten => out::problem(&format!(
            "knowmoretabs: you forgot {url}; nothing changed; knowmoretabs restore brings it back"
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
