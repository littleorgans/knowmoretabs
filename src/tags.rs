//! Tags the owner sets on pages, and the vocabulary they come from: the
//! second write path of `library.json`.
//!
//! slice: triage
//! why: Tags are the owner's decisions, so they live beside `forgotten` and
//!      take the same lock, read and atomic rewrite. One module owns the
//!      rules (what a name may be, how case is matched, what adding and
//!      removing do to a page's two lists) so that the CLI and the server
//!      cannot disagree, and the same request with its lists swapped is its
//!      undo from either side.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use jiff::Timestamp;

use crate::archive::Archive;
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, State, Term, VocabularyEntry, fold, tag_order};
use crate::out;
use crate::triage::{already, plural};

/// A chip, not a sentence.
pub const NAME_LIMIT: usize = 40;

/// The one spelling a typed name is stored under: trimmed, inner whitespace
/// collapsed to a single space, non-empty, at most [`NAME_LIMIT`] characters
/// and free of control characters.
pub fn normalize(raw: &str) -> Result<String, Error> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let reason = if name.is_empty() {
        "it is empty"
    } else if name.chars().count() > NAME_LIMIT {
        "it is longer than 40 characters"
    } else if name.chars().any(char::is_control) {
        "it contains a control character"
    } else {
        return Ok(name);
    };
    Err(Error::TagName {
        name: raw.to_owned(),
        reason,
    })
}

/// Normalized, and without repeats in any case.
fn names(raw: &[String]) -> Result<Vec<String>, Error> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for raw in raw {
        let name = normalize(raw)?;
        if seen.insert(fold(&name)) {
            names.push(name);
        }
    }
    Ok(names)
}

/// A name on both sides of a request has no order to settle it, and the
/// swapped request would be the same request, so it could not be undone.
fn disjoint(first: &[String], second: &[String], reason: &'static str) -> Result<(), Error> {
    let first: HashSet<String> = first.iter().map(|name| fold(name)).collect();
    match second.iter().find(|name| first.contains(&fold(name))) {
        Some(name) => Err(Error::TagName {
            name: name.clone(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Vocabulary times are whole seconds, like the snapshot ids beside them.
fn now() -> Timestamp {
    let now = Timestamp::now();
    Timestamp::from_second(now.as_second()).unwrap_or(now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admitted {
    Known,
    Created,
    Revived,
}

/// The vocabulary's spelling of `name`, after putting it there or bringing
/// it back from retirement if need be.
fn admit(state: &mut State, name: &str, now: Timestamp) -> (String, Admitted) {
    let folded = fold(name);
    if let Some((spelling, term)) = state
        .vocabulary
        .iter_mut()
        .find(|(spelling, _)| fold(spelling) == folded)
    {
        let admitted = if term.retired_at.take().is_some() {
            Admitted::Revived
        } else {
            Admitted::Known
        };
        return (spelling.clone(), admitted);
    }
    state.vocabulary.insert(name.to_owned(), Term::new(now));
    (name.to_owned(), Admitted::Created)
}

/// Takes every spelling of `name` out of `set`.
fn strip(set: &mut BTreeSet<String>, name: &str) {
    let folded = fold(name);
    set.retain(|entry| fold(entry) != folded);
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// Known URLs whose shown tags changed, in request order, without repeats.
    pub changed: Vec<String>,
    /// Known URLs already tagged that way; a repeated request is not an error.
    pub unchanged: Vec<String>,
    /// Not a page the library holds. The CLI refuses these; the API reports them.
    pub unknown: Vec<String>,
    /// Every known URL in the request, with the tags it shows afterwards.
    pub tags: BTreeMap<String, Vec<String>>,
    /// Names this request added to the vocabulary.
    pub created: Vec<String>,
    /// Retired names this request brought back by adding them to a page.
    pub revived: Vec<String>,
    /// The active vocabulary afterwards.
    pub vocabulary: Vec<VocabularyEntry>,
}

/// Adds `add` to and takes `remove` off every page in `urls`. Adding puts
/// the name in the page's `add` list and out of its `remove` list; removing
/// does the opposite, so the two lists never share a name and the request
/// with the lists swapped undoes this one. Names match the vocabulary
/// without regard to case and are stored in its spelling; adding an unknown
/// name creates it, adding a retired one brings it back, and removing a name
/// the vocabulary has never held changes nothing. With `strict`, an unknown
/// URL is an error and nothing is written, as for `forget`.
pub fn apply(
    root: &Path,
    urls: &[String],
    add: &[String],
    remove: &[String],
    strict: bool,
    log: Log,
) -> Result<Outcome, Error> {
    let add = names(add)?;
    let remove = names(remove)?;
    disjoint(&add, &remove, "it is named both to add and to remove")?;
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let loaded = library::load(&archive)?;
    let known = library::known_urls(&loaded.snapshots);
    let mut state = State::read(root)?;
    let mut outcome = Outcome::default();
    let mut seen = HashSet::new();
    let mut pages = Vec::new();
    for url in urls {
        if !seen.insert(url.as_str()) {
            continue;
        }
        if known.contains(url.as_str()) {
            pages.push(url.clone());
        } else {
            outcome.unknown.push(url.clone());
        }
    }
    if strict && !outcome.unknown.is_empty() {
        return Err(Error::NotInLibrary(outcome.unknown));
    }
    let before: Vec<Vec<String>> = {
        let spellings = state.spellings(false);
        pages
            .iter()
            .map(|url| state.page_tags(url, &spellings))
            .collect()
    };
    let mut dirty = false;
    // A request whose every URL went stale tags nothing, so it creates nothing.
    if !pages.is_empty() {
        let now = now();
        let mut adding = Vec::new();
        for name in &add {
            let (spelling, admitted) = admit(&mut state, name, now);
            match admitted {
                Admitted::Known => {}
                Admitted::Created => outcome.created.push(spelling.clone()),
                Admitted::Revived => outcome.revived.push(spelling.clone()),
            }
            dirty |= admitted != Admitted::Known;
            adding.push(spelling);
        }
        let removing: Vec<String> = remove
            .iter()
            .filter_map(|name| state.term(name).map(|(spelling, _)| spelling.to_owned()))
            .collect();
        for url in &pages {
            let page = state.tags.entry(url.clone()).or_default();
            let was = (page.add.clone(), page.remove.clone());
            for name in &adding {
                strip(&mut page.remove, name);
                strip(&mut page.add, name);
                page.add.insert(name.clone());
            }
            for name in &removing {
                strip(&mut page.add, name);
                strip(&mut page.remove, name);
                page.remove.insert(name.clone());
            }
            dirty |= (&page.add, &page.remove) != (&was.0, &was.1);
            if page.is_empty() {
                state.tags.remove(url);
            }
        }
    }
    let spellings = state.spellings(false);
    for (url, before) in pages.into_iter().zip(before) {
        let after = state.page_tags(&url, &spellings);
        if after == before {
            outcome.unchanged.push(url.clone());
        } else {
            outcome.changed.push(url.clone());
        }
        outcome.tags.insert(url, after);
    }
    if dirty {
        state.write(root)?;
    }
    outcome.vocabulary = state.active_vocabulary();
    Ok(outcome)
}

#[derive(Debug, Default)]
pub struct VocabularyOutcome {
    pub created: Vec<String>,
    pub revived: Vec<String>,
    pub retired: Vec<String>,
    /// Named to retire but never in the vocabulary. The CLI refuses these;
    /// the API ignores them, since there is nothing to retire.
    pub unknown: Vec<String>,
    /// The active vocabulary afterwards.
    pub vocabulary: Vec<VocabularyEntry>,
}

/// Creates and retires vocabulary entries. Creating a retired name brings
/// it back; retiring records the time and never touches a page, so a page
/// tagged with a retired name shows it again the day it comes back.
pub fn edit_vocabulary(
    root: &Path,
    create: &[String],
    retire: &[String],
    strict: bool,
    log: Log,
) -> Result<VocabularyOutcome, Error> {
    let create = names(create)?;
    let retire = names(retire)?;
    disjoint(&create, &retire, "it is named both to create and to retire")?;
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let mut state = State::read(root)?;
    let mut outcome = VocabularyOutcome::default();
    let now = now();
    for name in &retire {
        let folded = fold(name);
        match state
            .vocabulary
            .iter_mut()
            .find(|(spelling, _)| fold(spelling) == folded)
        {
            Some((spelling, term)) if term.retired_at.is_none() => {
                term.retired_at = Some(now);
                outcome.retired.push(spelling.clone());
            }
            Some(_) => {}
            None => outcome.unknown.push(name.clone()),
        }
    }
    if strict && !outcome.unknown.is_empty() {
        return Err(Error::UnknownTag(outcome.unknown));
    }
    for name in &create {
        match admit(&mut state, name, now) {
            (_, Admitted::Known) => {}
            (spelling, Admitted::Created) => outcome.created.push(spelling),
            (spelling, Admitted::Revived) => outcome.revived.push(spelling),
        }
    }
    if !(outcome.created.is_empty() && outcome.revived.is_empty() && outcome.retired.is_empty()) {
        state.write(root)?;
    }
    outcome.vocabulary = state.active_vocabulary();
    Ok(outcome)
}

/// `knowmoretabs tag`.
pub fn tag_command(
    root: &Path,
    urls: &[String],
    add: &[String],
    remove: &[String],
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let outcome = apply(root, urls, add, remove, true, log)?;
    if json {
        out::json(&serde_json::json!({
            "changed": outcome.changed,
            "unchanged": outcome.unchanged,
            "tags": outcome.tags,
            "created": outcome.created,
            "revived": outcome.revived,
        }));
    } else if !log.quiet {
        out::line(&human_tag(&outcome));
    }
    Ok(())
}

fn human_tag(outcome: &Outcome) -> String {
    let changed = outcome.changed.len();
    let unchanged = outcome.unchanged.len();
    let mut line = if changed == 0 {
        format!(
            "already tagged that way: {}; nothing changed",
            plural(unchanged, "page")
        )
    } else {
        format!(
            "retagged {}{}",
            plural(changed, "page"),
            already(unchanged, "already tagged that way")
        )
    };
    if !outcome.created.is_empty() {
        let _ = write!(line, "; new tags: {}", outcome.created.join(", "));
    }
    if !outcome.revived.is_empty() {
        let _ = write!(
            line,
            "; back from retirement: {}",
            outcome.revived.join(", ")
        );
    }
    line
}

/// `knowmoretabs tags`: the vocabulary with how many pages in the library
/// carry each tag, after any `--create` and `--retire`.
pub fn tags_command(
    root: &Path,
    create: &[String],
    retire: &[String],
    all: bool,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let edited = if create.is_empty() && retire.is_empty() {
        None
    } else {
        Some(edit_vocabulary(root, create, retire, true, log)?)
    };
    let loaded = library::load(&Archive::at(root))?;
    let state = State::read(root)?;
    let counts = page_counts(&state, &library::known_urls(&loaded.snapshots));
    let mut listed: Vec<(&String, &Term)> = state
        .vocabulary
        .iter()
        .filter(|(_, term)| all || term.retired_at.is_none())
        .collect();
    listed.sort_by(|(a, _), (b, _)| tag_order(a, b));
    let count = |name: &str| counts.get(&fold(name)).copied().unwrap_or(0);
    let edited = edited.unwrap_or_default();
    if json {
        let vocabulary: Vec<_> = listed
            .iter()
            .map(|(name, term)| {
                let mut entry = serde_json::json!({
                    "name": name, "created_at": term.created_at, "pages": count(name),
                });
                if let Some(retired_at) = term.retired_at {
                    entry["retired_at"] = serde_json::json!(retired_at);
                }
                entry
            })
            .collect();
        out::json(&serde_json::json!({
            "vocabulary": vocabulary,
            "created": edited.created,
            "revived": edited.revived,
            "retired": edited.retired,
        }));
        return Ok(());
    }
    if log.quiet {
        return Ok(());
    }
    let mut changes = Vec::new();
    for (verb, names) in [
        ("created", &edited.created),
        ("brought back", &edited.revived),
        ("retired", &edited.retired),
    ] {
        if !names.is_empty() {
            changes.push(format!("{verb} {}", names.join(", ")));
        }
    }
    if !changes.is_empty() {
        out::line(&changes.join("; "));
    }
    if listed.is_empty() {
        out::line("No tags yet; add one with `knowmoretabs tag <URL> --add NAME`.");
        return Ok(());
    }
    let width = listed
        .iter()
        .map(|(name, _)| count(name).to_string().len())
        .max()
        .unwrap_or(1);
    for (name, term) in &listed {
        let retired = if term.retired_at.is_some() {
            "  (retired)"
        } else {
            ""
        };
        out::line(&format!("{:>width$}  {name}{retired}", count(name)));
    }
    Ok(())
}

/// Pages the library shows, per folded tag name, retired tags included so
/// `--all` can say what retiring hid. Forgotten pages are not counted: the
/// library does not show them.
fn page_counts(state: &State, known: &HashSet<&str>) -> BTreeMap<String, usize> {
    let spellings = state.spellings(true);
    let mut counts = BTreeMap::new();
    for url in state.tags.keys() {
        if !known.contains(url.as_str()) || state.forgotten.contains(url) {
            continue;
        }
        for name in state.page_tags(url, &spellings) {
            *counts.entry(fold(&name)).or_default() += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_collapsed_bounded_and_printable() {
        assert_eq!(
            normalize("  Model \t context\nprotocol ").unwrap(),
            "Model context protocol"
        );
        assert_eq!(normalize(&"x".repeat(40)).unwrap().len(), 40);
        assert_eq!(normalize("研究").unwrap(), "研究");
        for bad in ["", "   ", &"x".repeat(41), "a\u{7}b", "\u{0}"] {
            assert!(
                matches!(normalize(bad), Err(Error::TagName { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_name_on_both_sides_is_refused_in_any_case() {
        let add = names(&["MCP".into(), "mcp".into()]).unwrap();
        assert_eq!(add, ["MCP"]);
        assert!(disjoint(&add, &["Mcp".into()], "both").is_err());
        assert!(disjoint(&add, &["Harness".into()], "both").is_ok());
    }

    #[test]
    fn human_line_counts_and_names_new_tags() {
        let outcome = Outcome {
            changed: vec!["a".into(), "b".into()],
            unchanged: vec!["c".into()],
            created: vec!["Harness".into()],
            ..Outcome::default()
        };
        assert_eq!(
            human_tag(&outcome),
            "retagged 2 pages (1 was already tagged that way); new tags: Harness"
        );
        let nothing = Outcome {
            unchanged: vec!["c".into()],
            ..Outcome::default()
        };
        assert_eq!(
            human_tag(&nothing),
            "already tagged that way: 1 page; nothing changed"
        );
    }
}
