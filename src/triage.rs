//! Forgetting and restoring: the write path of `library.json`.
//!
//! slice: triage
//! why: Forgetting is a filter over the library, never an edit to a snapshot,
//!      and the page says so at the point of action. One function owns the
//!      read-modify-write under the archive lock so that the CLI and the
//!      server cannot disagree about what "in your library" means, and two
//!      concurrent forgets cannot lose each other's change.

use std::collections::HashSet;
use std::path::Path;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, State};
use crate::out;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Forget,
    Restore,
}

impl Action {
    /// The key the frontend's stand-in server used for the changed URLs.
    pub fn key(self) -> &'static str {
        match self {
            Self::Forget => "forgotten",
            Self::Restore => "restored",
        }
    }
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// URLs whose flag flipped, in request order, without repeats.
    pub changed: Vec<String>,
    /// Already in the requested state; a repeated request is not an error.
    pub unchanged: Vec<String>,
    /// Not a page the library holds. The CLI refuses these; the API reports them.
    pub unknown: Vec<String>,
    /// Forgotten pages the archive holds afterwards, as `stats.forgotten` counts them.
    pub forgotten: usize,
}

/// Flips the forgotten flag on `urls`. With `strict`, an unknown URL is an
/// error and nothing is written; without it the known ones are applied and
/// the rest reported, which is what a page whose list may be minutes old
/// needs from a bulk request and its undo.
pub fn apply(
    root: &Path,
    urls: &[String],
    action: Action,
    strict: bool,
    log: Log,
) -> Result<Outcome, Error> {
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let loaded = library::load(&archive)?;
    let known = library::known_urls(&loaded.snapshots);
    let mut state = State::read(root)?;
    let mut outcome = Outcome::default();
    let mut seen = HashSet::new();
    for url in urls {
        if !seen.insert(url.as_str()) {
            continue;
        }
        // A forgotten URL whose snapshots are gone is still restorable;
        // otherwise it would be stuck in the state file forever.
        let in_library = known.contains(url.as_str())
            || (action == Action::Restore && state.forgotten.contains(url));
        if !in_library {
            outcome.unknown.push(url.clone());
            continue;
        }
        let changed = match action {
            Action::Forget => state.forgotten.insert(url.clone()),
            Action::Restore => state.forgotten.remove(url),
        };
        if changed {
            outcome.changed.push(url.clone());
        } else {
            outcome.unchanged.push(url.clone());
        }
    }
    if strict && !outcome.unknown.is_empty() {
        return Err(Error::NotInLibrary(outcome.unknown));
    }
    if !outcome.changed.is_empty() {
        state.write(root)?;
    }
    outcome.forgotten = state
        .forgotten
        .iter()
        .filter(|url| known.contains(url.as_str()))
        .count();
    Ok(outcome)
}

/// `knowmoretabs forget` and `restore`.
pub fn command(
    root: &Path,
    urls: &[String],
    action: Action,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let outcome = apply(root, urls, action, true, log)?;
    if json {
        out::json(&serde_json::json!({
            "action": match action { Action::Forget => "forget", Action::Restore => "restore" },
            "changed": outcome.changed,
            "unchanged": outcome.unchanged,
            "forgotten": outcome.forgotten,
        }));
    } else if !log.quiet {
        out::line(&human(action, &outcome));
    }
    Ok(())
}

fn human(action: Action, outcome: &Outcome) -> String {
    let changed = outcome.changed.len();
    let unchanged = outcome.unchanged.len();
    match action {
        Action::Forget if changed == 0 => format!(
            "already forgotten: {}; nothing changed",
            plural(unchanged, "page")
        ),
        Action::Forget => format!(
            "forgot {}{}; hidden from the library, the snapshots are untouched",
            plural(changed, "page"),
            already(unchanged, "already forgotten")
        ),
        Action::Restore if changed == 0 => format!(
            "not forgotten: {}; nothing changed",
            plural(unchanged, "page")
        ),
        Action::Restore => format!(
            "restored {}{}; back in the library",
            plural(changed, "page"),
            already(unchanged, "not forgotten")
        ),
    }
}

pub fn already(n: usize, what: &str) -> String {
    match n {
        0 => String::new(),
        1 => format!(" (1 was {what})"),
        n => format!(" ({n} were {what})"),
    }
}

pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_lines_count_and_reassure() {
        let outcome = Outcome {
            changed: vec!["a".into(), "b".into()],
            unchanged: vec!["c".into()],
            ..Outcome::default()
        };
        assert_eq!(
            human(Action::Forget, &outcome),
            "forgot 2 pages (1 was already forgotten); hidden from the library, the snapshots are untouched"
        );
        assert_eq!(
            human(Action::Restore, &outcome),
            "restored 2 pages (1 was not forgotten); back in the library"
        );
        let nothing = Outcome {
            unchanged: vec!["c".into(), "d".into()],
            ..Outcome::default()
        };
        assert_eq!(
            human(Action::Forget, &nothing),
            "already forgotten: 2 pages; nothing changed"
        );
        assert_eq!(
            human(Action::Restore, &nothing),
            "not forgotten: 2 pages; nothing changed"
        );
    }
}
