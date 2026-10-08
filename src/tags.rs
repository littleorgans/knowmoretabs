//! Tags the owner sets on pages, and the vocabulary they come from: the
//! second write path of `library.json`.
//!
//! slice: tags
//! why: Tags are the owner's decisions, so they live beside `forgotten` and
//!      take the same lock, read and atomic rewrite. One module owns the
//!      rules (what a name may be, how case is matched, what adding and
//!      removing do to a page's two lists) so that the CLI and the server
//!      cannot disagree. Undo records only the decisions a request changed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Archive};
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, PageTags, State, Term, VocabularyEntry, fold, tag_order};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admitted {
    Known,
    Created,
    Revived,
}

/// The vocabulary's spelling of `name`, after putting it there or bringing
/// it back from retirement if need be.
pub fn admit(state: &mut State, name: &str, now: Timestamp) -> (String, Admitted) {
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

/// Where a request puts a name on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Add,
    Remove,
    /// Neither list: the owner has not decided, so a suggestion shows again.
    Clear,
}

impl Target {
    /// Whether the page's `add` list holds the name afterwards, and whether
    /// its `remove` list does.
    fn lists(self) -> (bool, bool) {
        match self {
            Self::Add => (true, false),
            Self::Remove => (false, true),
            Self::Clear => (false, false),
        }
    }
}

/// Sets `page`'s decision on `name`, whatever spelling it was in before.
fn decide(page: &mut PageTags, name: &str, (add, remove): (bool, bool)) {
    strip(&mut page.add, name);
    strip(&mut page.remove, name);
    if add {
        page.add.insert(name.to_owned());
    }
    if remove {
        page.remove.insert(name.to_owned());
    }
}

/// The previous decisions for just the names a request changed. Newly created
/// vocabulary entries remain, but revival restores the old retirement time.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undo {
    pub tags: Vec<Decision>,
    pub vocabulary: BTreeMap<String, Option<Timestamp>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    url: String,
    name: String,
    add: bool,
    remove: bool,
}

impl Decision {
    fn read(state: &State, url: &str, name: &str) -> Self {
        let page = state.tags.get(url);
        let has = |set: &BTreeSet<String>| set.iter().any(|n| fold(n) == fold(name));
        Self {
            url: url.to_owned(),
            name: name.to_owned(),
            add: page.is_some_and(|p| has(&p.add)),
            remove: page.is_some_and(|p| has(&p.remove)),
        }
    }
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub undo: Undo,
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
    /// Names this request cleared, in the vocabulary's spelling: a decision
    /// it changed on one of these is neither a tag nor a dismissal.
    pub cleared: Vec<String>,
    /// The active vocabulary afterwards.
    pub vocabulary: Vec<VocabularyEntry>,
}

/// Adds `add` to, takes `remove` off and clears `clear` from every page in
/// `urls`. Adding puts the name in the page's `add` list and out of its
/// `remove` list; removing does the opposite, so the two lists never share a
/// name; clearing takes it out of both, so the page is undecided again. The
/// outcome carries the previous decisions for exact undo. Names match the
/// vocabulary without regard to case and are stored in its spelling; adding
/// an unknown name creates it, adding a retired one brings it back, and
/// removing or clearing a name the vocabulary has never held changes nothing.
/// With `strict`, an unknown URL is an error and nothing is written, as for
/// `forget`.
pub fn apply(
    root: &Path,
    urls: &[String],
    add: &[String],
    remove: &[String],
    clear: &[String],
    strict: bool,
    log: Log,
) -> Result<Outcome, Error> {
    let add = names(add)?;
    let remove = names(remove)?;
    let clear = names(clear)?;
    disjoint(&add, &remove, "it is named both to add and to remove")?;
    disjoint(&add, &clear, "it is named both to add and to clear")?;
    disjoint(&remove, &clear, "it is named both to remove and to clear")?;
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn(archive::WAITING))?;
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
        let targets = targets(&mut state, &add, &remove, &clear, &mut outcome);
        dirty |= !(outcome.created.is_empty() && outcome.revived.is_empty());
        for url in &pages {
            for (name, target) in &targets {
                let before = Decision::read(&state, url, name);
                if (before.add, before.remove) != target.lists() {
                    outcome.undo.tags.push(before);
                }
            }
            let page = state.tags.entry(url.clone()).or_default();
            let was = (page.add.clone(), page.remove.clone());
            for (name, target) in &targets {
                decide(page, name, target.lists());
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

/// Each name of a request in the vocabulary's spelling, with where it goes.
/// A name to add is admitted first, and what that created or brought back is
/// recorded in `outcome`; a name to remove or clear that the vocabulary has
/// never held is left out.
fn targets(
    state: &mut State,
    add: &[String],
    remove: &[String],
    clear: &[String],
    outcome: &mut Outcome,
) -> Vec<(String, Target)> {
    let now = archive::now();
    let mut targets = Vec::new();
    for name in add {
        if let Some((spelling, term)) = state.term(name)
            && term.retired_at.is_some()
        {
            outcome
                .undo
                .vocabulary
                .insert(spelling.to_owned(), term.retired_at);
        }
        let (spelling, admitted) = admit(state, name, now);
        match admitted {
            Admitted::Known => {}
            Admitted::Created => outcome.created.push(spelling.clone()),
            Admitted::Revived => outcome.revived.push(spelling.clone()),
        }
        targets.push((spelling, Target::Add));
    }
    for (names, target) in [(remove, Target::Remove), (clear, Target::Clear)] {
        for name in names {
            if let Some((spelling, _)) = state.term(name) {
                targets.push((spelling.to_owned(), target));
            }
        }
    }
    outcome.cleared = targets
        .iter()
        .filter(|(_, target)| *target == Target::Clear)
        .map(|(spelling, _)| spelling.clone())
        .collect();
    targets
}

/// Restores touched decisions in one lock-guarded write. Unrelated names and
/// pages survive even when another writer has changed them since the request.
pub fn undo(root: &Path, undo: &Undo, log: Log) -> Result<Outcome, Error> {
    let mut seen = HashSet::new();
    for decision in &undo.tags {
        normalize(&decision.name)?;
        if (decision.add && decision.remove) || !seen.insert((&decision.url, fold(&decision.name)))
        {
            return Err(Error::TagName {
                name: decision.name.clone(),
                reason: "invalid undo decision",
            });
        }
    }
    let mut names = HashSet::new();
    for name in undo.vocabulary.keys() {
        normalize(name)?;
        if !names.insert(fold(name)) {
            return Err(Error::TagName {
                name: name.clone(),
                reason: "duplicate undo vocabulary name",
            });
        }
    }
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn(archive::WAITING))?;
    let loaded = library::load(&archive)?;
    let known = library::known_urls(&loaded.snapshots);
    let mut state = State::read(root)?;
    let mut outcome = Outcome::default();
    let mut before = BTreeMap::new();
    let spellings = state.spellings(false);
    for decision in &undo.tags {
        if known.contains(decision.url.as_str()) {
            before.insert(
                decision.url.clone(),
                state.page_tags(&decision.url, &spellings),
            );
        } else if !outcome.unknown.contains(&decision.url) {
            outcome.unknown.push(decision.url.clone());
        }
    }
    // Retirement can change pages outside the original request too.
    if !undo.vocabulary.is_empty() {
        for url in &known {
            before.insert((*url).to_owned(), state.page_tags(url, &spellings));
        }
    }
    for decision in &undo.tags {
        if !known.contains(decision.url.as_str()) {
            continue;
        }
        let Some((name, _)) = state.term(&decision.name) else {
            continue;
        };
        let name = name.to_owned();
        let previous = Decision::read(&state, &decision.url, &name);
        if previous.add == decision.add && previous.remove == decision.remove {
            continue;
        }
        outcome.undo.tags.push(previous);
        let page = state.tags.entry(decision.url.clone()).or_default();
        decide(page, &name, (decision.add, decision.remove));
        if page.is_empty() {
            state.tags.remove(&decision.url);
        }
    }
    for (name, retired_at) in &undo.vocabulary {
        let Some((spelling, _)) = state.term(name) else {
            continue;
        };
        let spelling = spelling.to_owned();
        let term = state.vocabulary.get_mut(&spelling).expect("known term");
        if term.retired_at != *retired_at {
            outcome.undo.vocabulary.insert(spelling, term.retired_at);
            term.retired_at = *retired_at;
        }
    }
    let spellings = state.spellings(false);
    for (url, was) in before {
        let after = state.page_tags(&url, &spellings);
        if after == was {
            outcome.unchanged.push(url.clone());
        } else {
            outcome.changed.push(url.clone());
        }
        outcome.tags.insert(url, after);
    }
    if !outcome.undo.tags.is_empty() || !outcome.undo.vocabulary.is_empty() {
        state.write(root)?;
    }
    outcome.vocabulary = state.active_vocabulary();
    Ok(outcome)
}

/// What one `tags` command, or one `POST /api/vocabulary`, asks of the
/// vocabulary. The pairs are (tag, definition).
#[derive(Debug, Default)]
pub struct VocabularyEdit {
    pub create: Vec<String>,
    pub retire: Vec<String>,
    pub define: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct VocabularyOutcome {
    pub created: Vec<String>,
    pub revived: Vec<String>,
    pub retired: Vec<String>,
    /// Tags whose definition this request set, changed or cleared.
    pub defined: Vec<String>,
    /// Parent rules a development build left in `library.json`, which this
    /// edit took out: tags are flat.
    pub dropped_rules: usize,
    /// Named to retire but never in the vocabulary. The CLI refuses these;
    /// the API ignores them, since there is nothing to retire.
    pub unknown: Vec<String>,
    /// The active vocabulary afterwards.
    pub vocabulary: Vec<VocabularyEntry>,
}

/// A definition is read by a tagging agent as one line of a list, so it is
/// one paragraph: whitespace collapsed, at most this many characters.
pub const DEFINITION_LIMIT: usize = 500;

/// The stored spelling of a definition; empty means "no definition".
fn definition(name: &str, raw: &str) -> Result<Option<String>, Error> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let reason = if text.chars().count() > DEFINITION_LIMIT {
        "it is longer than 500 characters"
    } else if text.chars().any(char::is_control) {
        "it contains a control character"
    } else {
        return Ok((!text.is_empty()).then_some(text));
    };
    Err(Error::TagDefinition {
        name: name.to_owned(),
        reason,
    })
}

/// The vocabulary a tagging agent was shown, as twelve hex digits: the
/// start of the SHA-256 of the active tags in tag order, each with its
/// definition. Computed, never stored, so two prompts from the same
/// vocabulary carry the same version and any change a tagger could act on
/// (a tag or a definition) gives a new one. Retired tags and creation times
/// are not part of it.
pub fn vocabulary_version(state: &State) -> String {
    use sha2::{Digest, Sha256};
    let canonical: Vec<serde_json::Value> = state
        .active_vocabulary()
        .into_iter()
        .map(|entry| serde_json::json!([entry.name, entry.definition.unwrap_or_default()]))
        .collect();
    let digest = Sha256::digest(serde_json::Value::Array(canonical).to_string().as_bytes());
    format!("{digest:x}")[..12].to_owned()
}

/// Creates, retires and defines vocabulary entries. Creating a retired name
/// brings it back; retiring records the time and never touches a page, so a
/// page tagged with a retired name shows it again the day it comes back.
/// Defining a name the vocabulary lacks creates it, as typing a new name
/// does. Every edit also drops the parent rules a development build left,
/// and that alone is a change worth writing.
pub fn edit_vocabulary(
    root: &Path,
    edit: &VocabularyEdit,
    strict: bool,
    log: Log,
) -> Result<VocabularyOutcome, Error> {
    let create = names(&edit.create)?;
    let retire = names(&edit.retire)?;
    disjoint(&create, &retire, "it is named both to create and to retire")?;
    let mut define = Vec::new();
    for (name, text) in &edit.define {
        let name = normalize(name)?;
        let text = definition(&name, text)?;
        define.push((name, text));
    }
    let defined_names: Vec<String> = define.iter().map(|(name, _)| name.clone()).collect();
    disjoint(
        &defined_names,
        &retire,
        "it is named both to define and to retire",
    )?;
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn(archive::WAITING))?;
    let mut state = State::read(root)?;
    let mut outcome = VocabularyOutcome::default();
    let now = archive::now();
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
    for name in create.iter().chain(&defined_names) {
        match admit(&mut state, name, now) {
            (_, Admitted::Known) => {}
            (spelling, Admitted::Created) => outcome.created.push(spelling),
            (spelling, Admitted::Revived) => outcome.revived.push(spelling),
        }
    }
    for (name, text) in define {
        let (spelling, _) = state.term(&name).expect("admitted above");
        let spelling = spelling.to_owned();
        let term = state.vocabulary.get_mut(&spelling).expect("known term");
        if term.definition != text {
            term.definition = text;
            outcome.defined.push(spelling);
        }
    }
    outcome.dropped_rules = state.drop_parent_rules();
    let changed = !(outcome.created.is_empty()
        && outcome.revived.is_empty()
        && outcome.retired.is_empty()
        && outcome.defined.is_empty()
        && outcome.dropped_rules == 0);
    if changed {
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
    clear: &[String],
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let outcome = apply(root, urls, add, remove, clear, true, log)?;
    if json {
        out::json(&serde_json::json!({
            "changed": outcome.changed,
            "unchanged": outcome.unchanged,
            "dismissed": dismissed(&outcome).0,
            "cleared": cleared(&outcome).0,
            "tags": outcome.tags,
            "created": outcome.created,
            "revived": outcome.revived,
        }));
    } else if !log.quiet {
        out::line(&human_tag(&outcome));
    }
    Ok(())
}

/// The pages and the names of the decisions this request changed that
/// `keep` selects, each once, in request order.
fn decided(outcome: &Outcome, keep: impl Fn(&Decision) -> bool) -> (Vec<&str>, Vec<&str>) {
    let mut urls: Vec<&str> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for decision in outcome.undo.tags.iter().filter(|d| keep(d)) {
        if !urls.contains(&decision.url.as_str()) {
            urls.push(&decision.url);
        }
        if !names.iter().any(|n| fold(n) == fold(&decision.name)) {
            names.push(&decision.name);
        }
    }
    (urls, names)
}

fn is_cleared(outcome: &Outcome, decision: &Decision) -> bool {
    outcome
        .cleared
        .iter()
        .any(|name| fold(name) == fold(&decision.name))
}

/// Pages whose shown tags stayed the same but whose decisions changed, and
/// the names decided: removing a tag a page does not show (one only
/// suggested, or none at all) still records it, so it is not suggested there
/// again. Those pages are in `unchanged`, since `changed` counts shown tags.
/// A cleared decision is not a dismissal.
fn dismissed(outcome: &Outcome) -> (Vec<&str>, Vec<&str>) {
    decided(outcome, |d| {
        !is_cleared(outcome, d) && outcome.unchanged.contains(&d.url)
    })
}

/// Pages where a cleared name was decided before, changed or not, and the
/// names: they are undecided again.
fn cleared(outcome: &Outcome) -> (Vec<&str>, Vec<&str>) {
    decided(outcome, |d| is_cleared(outcome, d))
}

fn human_tag(outcome: &Outcome) -> String {
    let changed = outcome.changed.len();
    let (urls, names) = dismissed(outcome);
    // Already tagged that way: no decision on the page changed at all.
    let unchanged = outcome
        .unchanged
        .iter()
        .filter(|url| !outcome.undo.tags.iter().any(|d| &d.url == *url))
        .count();
    let mut parts = Vec::new();
    if changed > 0 {
        parts.push(format!("retagged {}", plural(changed, "page")));
    }
    if !urls.is_empty() {
        let (was, will) = if names.len() == 1 {
            ("it was not a tag you set there", "it")
        } else {
            ("they were not tags you set there", "they")
        };
        parts.push(format!(
            "dismissed {} on {} ({was}, and {will} will not be suggested there again)",
            names.join(", "),
            plural(urls.len(), "page")
        ));
    }
    let (urls, names) = cleared(outcome);
    if !urls.is_empty() {
        parts.push(format!(
            "cleared {} on {} (undecided again)",
            names.join(", "),
            plural(urls.len(), "page")
        ));
    }
    let mut line = if parts.is_empty() {
        format!(
            "already tagged that way: {}; nothing changed",
            plural(unchanged, "page")
        )
    } else {
        parts.join("; ") + &already(unchanged, "already tagged that way")
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
/// carry each tag, and what each means, after any edits.
pub fn tags_command(
    root: &Path,
    edit: &VocabularyEdit,
    all: bool,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let edited = if edit.create.is_empty() && edit.retire.is_empty() && edit.define.is_empty() {
        VocabularyOutcome::default()
    } else {
        edit_vocabulary(root, edit, true, log)?
    };
    let loaded = library::load(&Archive::at(root))?;
    let state = State::read(root)?;
    let counts = page_counts(&state, &library::known_urls(&loaded.snapshots));
    let mut listed: Vec<Listed> = state
        .vocabulary
        .iter()
        .filter(|(_, term)| all || term.retired_at.is_none())
        .map(|(name, term)| Listed {
            name,
            term,
            pages: counts.get(&fold(name)).copied().unwrap_or(0),
        })
        .collect();
    listed.sort_by(|a, b| tag_order(a.name, b.name));
    if json {
        out::json(&json_listing(&listed, &edited, &vocabulary_version(&state)));
    } else if !log.quiet {
        out::block(&human_listing(&listed, &edited));
    }
    Ok(())
}

/// One row of `tags`.
struct Listed<'a> {
    name: &'a str,
    term: &'a Term,
    pages: usize,
}

fn json_listing(listed: &[Listed], edited: &VocabularyOutcome, version: &str) -> serde_json::Value {
    let vocabulary: Vec<_> = listed
        .iter()
        .map(|row| {
            let mut entry = serde_json::json!({
                "name": row.name, "created_at": row.term.created_at, "pages": row.pages,
            });
            if let Some(retired_at) = row.term.retired_at {
                entry["retired_at"] = serde_json::json!(retired_at);
            }
            if let Some(definition) = &row.term.definition {
                entry["definition"] = serde_json::json!(definition);
            }
            entry
        })
        .collect();
    serde_json::json!({
        "vocabulary": vocabulary,
        "version": version,
        "created": edited.created,
        "revived": edited.revived,
        "retired": edited.retired,
        "defined": edited.defined,
        "dropped_rules": edited.dropped_rules,
    })
}

/// What changed, then one line per tag: its page count, its name, and what
/// it means when the owner has said.
fn human_listing(listed: &[Listed], edited: &VocabularyOutcome) -> String {
    let mut text = String::new();
    let mut changes = Vec::new();
    for (verb, names) in [
        ("created", &edited.created),
        ("brought back", &edited.revived),
        ("retired", &edited.retired),
        ("defined", &edited.defined),
    ] {
        if !names.is_empty() {
            changes.push(format!("{verb} {}", names.join(", ")));
        }
    }
    if edited.dropped_rules > 0 {
        changes.push(format!(
            "dropped {} (tags are flat)",
            plural(edited.dropped_rules, "parent rule")
        ));
    }
    if !changes.is_empty() {
        let _ = writeln!(text, "{}", changes.join("; "));
    }
    if listed.is_empty() {
        text.push_str("No tags yet; add one with `knowmoretabs tag <URL> --add NAME`.\n");
        return text;
    }
    let width = listed
        .iter()
        .map(|row| row.pages.to_string().len())
        .max()
        .unwrap_or(1);
    for row in listed {
        let _ = write!(text, "{:>width$}  {}", row.pages, row.name);
        if row.term.retired_at.is_some() {
            text.push_str("  (retired)");
        }
        if let Some(definition) = &row.term.definition {
            let _ = write!(text, " — {definition}");
        }
        text.push('\n');
    }
    text
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

    #[test]
    fn human_line_says_a_removal_it_recorded_without_changing_shown_tags() {
        let decision = |url: &str, name: &str| Decision {
            url: url.into(),
            name: name.into(),
            add: false,
            remove: false,
        };
        let outcome = Outcome {
            changed: vec!["a".into()],
            unchanged: vec!["b".into(), "c".into(), "d".into()],
            undo: Undo {
                tags: vec![
                    decision("a", "MCP"),
                    decision("b", "MCP"),
                    decision("c", "mcp"),
                ],
                vocabulary: BTreeMap::new(),
            },
            ..Outcome::default()
        };
        assert_eq!(
            human_tag(&outcome),
            "retagged 1 page; dismissed MCP on 2 pages (it was not a tag you set there, \
             and it will not be suggested there again) (1 was already tagged that way)"
        );
        let only = Outcome {
            unchanged: vec!["b".into()],
            undo: Undo {
                tags: vec![decision("b", "MCP"), decision("b", "Skills")],
                vocabulary: BTreeMap::new(),
            },
            ..Outcome::default()
        };
        assert_eq!(
            human_tag(&only),
            "dismissed MCP, Skills on 1 page (they were not tags you set there, \
             and they will not be suggested there again)"
        );
    }

    #[test]
    fn human_line_says_a_clear_returned_tags_to_undecided() {
        let decision = |url: &str, name: &str, add: bool| Decision {
            url: url.into(),
            name: name.into(),
            add,
            remove: !add,
        };
        // A cleared add changed what "a" shows; a cleared remove did not
        // change what "b" shows. Neither is a dismissal, and neither page
        // was already tagged that way; "c" was.
        let outcome = Outcome {
            changed: vec!["a".into()],
            unchanged: vec!["b".into(), "c".into()],
            cleared: vec!["Agent".into(), "Skills".into()],
            undo: Undo {
                tags: vec![decision("a", "Agent", true), decision("b", "Skills", false)],
                vocabulary: BTreeMap::new(),
            },
            ..Outcome::default()
        };
        assert_eq!(dismissed(&outcome), (vec![], vec![]));
        assert_eq!(cleared(&outcome), (vec!["a", "b"], vec!["Agent", "Skills"]));
        assert_eq!(
            human_tag(&outcome),
            "retagged 1 page; cleared Agent, Skills on 2 pages (undecided again) \
             (1 was already tagged that way)"
        );
    }
}
