//! Suggested tags: what an owner's agent answered, validated whole and kept
//! in `tags/suggested.jsonl`, and the per-page view the library shows.
//!
//! slice: triage
//! why: A suggestion is someone else's opinion, so it never becomes one of
//!      the owner's decisions: it sits in its own append-only file, with
//!      where it came from, when, and which vocabulary it answered. An
//!      import is all or nothing: a file with one bad line is refused whole,
//!      with every problem named by line, so the agent that wrote it can fix
//!      them in one pass and nothing half-imported has to be untangled. The
//!      library shows a suggested tag only while the owner has neither added
//!      nor removed it, which is what makes the owner's decisions win.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Archive};
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, State, fold, tag_order};
use crate::out;
use crate::tags::{self, normalize};
use crate::triage::plural;

pub const DIR: &str = "tags";
pub const FILE: &str = "suggested.jsonl";
/// What `tag --prompt` writes beside the pages, and `--import` looks for.
pub const PAGES_FILE: &str = "pages.jsonl";
pub const VOCABULARY_FILE: &str = "vocabulary.json";

/// A model name, not a sentence.
const SOURCE_LIMIT: usize = 80;

pub fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// One imported page: one line of `suggested.jsonl`. An empty `tags` is an
/// answer too: the page was read and nothing applied, so it is not due again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub url: String,
    pub tags: Vec<String>,
    pub source: String,
    pub imported_at: Timestamp,
    pub vocabulary_version: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// Every line, oldest first. A missing file is no suggestions; a damaged one
/// is refused like damaged `library.json`, since every write here is a
/// whole-file replacement and damage means something else wrote it.
pub fn read(root: &Path) -> Result<Vec<Line>, Error> {
    let path = path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io("read suggested tags", &path)(err)),
    };
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line = serde_json::from_str(line).map_err(|err| Error::LibraryState {
            path: path.clone(),
            reason: format!("line {}: {}", index + 1, without_position(&err)),
        })?;
        lines.push(line);
    }
    Ok(lines)
}

/// Appends in one whole-file replacement, so an import is in the file
/// entirely or not at all. Caller holds the archive lock.
fn append(root: &Path, new: &[Line]) -> Result<(), Error> {
    let dir = root.join(DIR);
    archive::create_private_dir(&dir).map_err(Error::io("create", &dir))?;
    let path = path(root);
    let mut bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(Error::io("read suggested tags", &path)(err)),
    };
    if bytes.last().is_some_and(|b| *b != b'\n') {
        bytes.push(b'\n');
    }
    for line in new {
        let json = serde_json::to_vec(line).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        bytes.extend_from_slice(&json);
        bytes.push(b'\n');
    }
    archive::replace_file(&path, &bytes)
}

/// The newest line from each source, per URL. Sources match without regard
/// to case, like tag names.
fn latest(lines: &[Line]) -> HashMap<&str, BTreeMap<String, &Line>> {
    let mut latest: HashMap<&str, BTreeMap<String, &Line>> = HashMap::new();
    for line in lines {
        latest
            .entry(line.url.as_str())
            .or_default()
            .insert(fold(&line.source), line);
    }
    latest
}

/// One suggested tag on a page, and every source that suggested it. Two or
/// more sources is what the library calls confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggested {
    pub name: String,
    pub sources: Vec<String>,
}

/// URL to the suggestions the library shows for it: active tags only, in the
/// vocabulary's spelling and tag order, and only those the owner has neither
/// added nor removed. Each source counts with its newest answer.
pub fn by_page(lines: &[Line], state: &State) -> HashMap<String, Vec<Suggested>> {
    let spellings = state.spellings(false);
    let mut pages = HashMap::new();
    for (url, sources) in latest(lines) {
        let decided: HashSet<String> = state
            .tags
            .get(url)
            .map(|page| {
                page.add
                    .iter()
                    .chain(&page.remove)
                    .map(|n| fold(n))
                    .collect()
            })
            .unwrap_or_default();
        let mut tags: BTreeMap<String, Suggested> = BTreeMap::new();
        for line in sources.values() {
            for name in &line.tags {
                let folded = fold(name);
                let Some(spelling) = spellings.get(&folded) else {
                    continue;
                };
                if decided.contains(&folded) {
                    continue;
                }
                let entry = tags.entry(folded).or_insert_with(|| Suggested {
                    name: (*spelling).to_owned(),
                    sources: Vec::new(),
                });
                if !entry.sources.iter().any(|s| fold(s) == fold(&line.source)) {
                    entry.sources.push(line.source.clone());
                }
            }
        }
        let mut tags: Vec<Suggested> = tags.into_values().collect();
        if tags.is_empty() {
            continue;
        }
        for tag in &mut tags {
            tag.sources.sort_by(|a, b| tag_order(a, b));
        }
        tags.sort_by(|a, b| tag_order(&a.name, &b.name));
        pages.insert(url.to_owned(), tags);
    }
    pages
}

/// URLs any source has answered for, empty answers included.
pub fn answered(lines: &[Line]) -> HashSet<&str> {
    lines.iter().map(|line| line.url.as_str()).collect()
}

/// One line of an agent's `tags.jsonl`: exactly what the prompt asks for.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    url: String,
    tags: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    /// For the owner reading the file; not stored.
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

pub struct ImportOptions<'a> {
    pub file: &'a Path,
    pub source: Option<&'a str>,
    pub accept_new: bool,
    pub partial: bool,
    pub dry_run: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub source: String,
    /// Lines in the file: one per page.
    pub pages: usize,
    /// Pages with at least one tag, and with none.
    pub tagged: usize,
    pub empty: usize,
    /// Tags suggested, over all pages, parent rules included.
    pub suggestions: usize,
    /// Of those, how many only a parent rule added.
    pub implied: usize,
    /// Pages whose answer from this source was already stored as it stands.
    pub unchanged: usize,
    /// Names `--accept-new` added to the vocabulary, or brought back.
    pub created: Vec<String>,
    pub revived: Vec<String>,
    /// Pages in the prompt's `pages.jsonl` without a line (only with `--partial`).
    pub missing: usize,
    pub vocabulary_version: String,
    /// The vocabulary changed after the prompt was written.
    pub vocabulary_changed: bool,
}

/// serde's position is always line 1 here: each line is parsed alone.
fn without_position(err: &serde_json::Error) -> String {
    let text = err.to_string();
    match text.rfind(" at line ") {
        Some(at) => text[..at].to_owned(),
        None => text,
    }
}

fn source_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        Err("it is empty")
    } else if name.chars().count() > SOURCE_LIMIT {
        Err("it is longer than 80 characters")
    } else if name.chars().any(char::is_control) {
        Err("it contains a control character")
    } else {
        Ok(name)
    }
}

/// All an import needs from a line of the prompt's `pages.jsonl`.
#[derive(Deserialize)]
struct PromptedPage {
    url: String,
}

/// The URLs of the prompt beside `file`, when there is one, in its order.
fn prompted_urls(file: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(file.parent()?.join(PAGES_FILE)).ok()?;
    Some(
        text.lines()
            .filter_map(|line| serde_json::from_str::<PromptedPage>(line).ok())
            .map(|page| page.url)
            .collect(),
    )
}

/// The vocabulary version the prompt beside `file` was written with.
fn prompted_version(file: &Path) -> Option<String> {
    let bytes = fs::read(file.parent()?.join(VOCABULARY_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value["version"].as_str().map(str::to_owned)
}

/// A validated answer, its tags normalized but not yet resolved.
struct Resolved {
    url: String,
    tags: Vec<String>,
}

/// Everything read from an answer file, and everything wrong with it.
#[derive(Default)]
struct Parsed {
    answers: Vec<Resolved>,
    problems: Vec<String>,
    /// The one source, and the line that first named it (0 for `--source`).
    source: Option<(String, usize)>,
    /// Folded name to (spelling, lines) for names the vocabulary lacks.
    unknown: BTreeMap<String, (String, Vec<usize>)>,
    /// URL to the line it is on.
    urls: HashMap<String, usize>,
    unsourced: Vec<usize>,
}

impl Parsed {
    fn source(&mut self, number: usize, raw: Option<&str>) {
        let name = match raw.map(source_name) {
            None => return self.unsourced.push(number),
            Some(Err(reason)) => {
                return self
                    .problems
                    .push(format!("line {number}: cannot use that source: {reason}"));
            }
            Some(Ok(name)) => name,
        };
        match &self.source {
            None => self.source = Some((name, number)),
            Some((first, at)) if fold(first) != fold(&name) => self.problems.push(format!(
                "line {number}: source {name:?} differs from {first:?} on line {at}; one import is one source"
            )),
            Some(_) => {}
        }
    }

    fn line(
        &mut self,
        number: usize,
        line: &str,
        known: &HashSet<&str>,
        active: &HashSet<String>,
        sourced: bool,
    ) {
        let answer: Answer = match serde_json::from_str(line) {
            Ok(answer) => answer,
            Err(err) => {
                return self
                    .problems
                    .push(format!("line {number}: {}", without_position(&err)));
            }
        };
        if !sourced {
            self.source(number, answer.source.as_deref());
        }
        if let Some(first) = self.urls.get(&answer.url) {
            return self.problems.push(format!(
                "line {number}: {} is already on line {first}; give each page one line",
                answer.url
            ));
        }
        self.urls.insert(answer.url.clone(), number);
        if !known.contains(answer.url.as_str()) {
            return self.problems.push(format!(
                "line {number}: not in your library: {}",
                answer.url
            ));
        }
        let mut names = Vec::new();
        let mut folded = HashSet::new();
        for raw in &answer.tags {
            let name = match normalize(raw) {
                Ok(name) => name,
                Err(err) => {
                    self.problems.push(format!("line {number}: {err}"));
                    continue;
                }
            };
            if !folded.insert(fold(&name)) {
                continue;
            }
            if !active.contains(&fold(&name)) {
                self.unknown
                    .entry(fold(&name))
                    .or_insert_with(|| (name.clone(), Vec::new()))
                    .1
                    .push(number);
            }
            names.push(name);
        }
        self.answers.push(Resolved {
            url: answer.url,
            tags: names,
        });
    }
}

/// Reads and checks every line; problems are collected, never raised.
fn parse(
    text: &str,
    options: &ImportOptions,
    known: &HashSet<&str>,
    active: &HashSet<String>,
) -> Result<Parsed, Error> {
    let mut parsed = Parsed::default();
    if let Some(raw) = options.source {
        let name = source_name(raw).map_err(|reason| Error::Import {
            path: options.file.to_path_buf(),
            problems: vec![format!("--source {raw:?}: {reason}")],
        })?;
        parsed.source = Some((name, 0));
    }
    for (index, line) in text.lines().enumerate() {
        if !line.trim().is_empty() {
            parsed.line(index + 1, line, known, active, options.source.is_some());
        }
    }
    if !parsed.unsourced.is_empty() {
        let lines = if parsed.unsourced.len() == 1 {
            "1 line has"
        } else {
            "lines have"
        };
        parsed.problems.push(format!(
            "{lines} no source (line {}); add \"source\" with the model's name, or pass --source NAME",
            numbers(&parsed.unsourced)
        ));
    }
    if !options.accept_new {
        for (name, lines) in parsed.unknown.values() {
            parsed.problems.push(format!(
                "no such tag: {name} (line {}); use the vocabulary's names, or pass --accept-new to create it",
                numbers(lines)
            ));
        }
    }
    Ok(parsed)
}

/// Pages the prompt beside `file` listed that have no line. Missing pages
/// are a problem unless `--partial`: an agent that stopped early must not
/// pass its own check.
fn coverage(file: &Path, parsed: &mut Parsed, partial: bool) -> usize {
    let Some(prompted) = prompted_urls(file) else {
        return 0;
    };
    let absent: Vec<&String> = prompted
        .iter()
        .filter(|url| !parsed.urls.contains_key(url.as_str()))
        .collect();
    if let Some(first) = absent.first()
        && !partial
    {
        parsed.problems.push(format!(
            "{} in {PAGES_FILE} {} no line, the first is {first}; tag every page, or pass --partial to import what is there",
            plural(absent.len(), "page"),
            if absent.len() == 1 { "has" } else { "have" },
        ));
    }
    absent.len()
}

/// Validates `tags.jsonl` against the library and stores it as suggestions,
/// or with `dry_run` only says what it would store. Every problem is
/// collected before anything is refused, and nothing is written unless the
/// whole file passes.
pub fn import(root: &Path, options: &ImportOptions, log: Log) -> Result<Report, Error> {
    let file = options.file;
    let text = fs::read(file).map_err(Error::io("read", file))?;
    let text = String::from_utf8_lossy(&text);
    // A dry run is what an agent runs over and over; it must never wait on,
    // or create, the archive it only reads.
    let archive = if options.dry_run {
        Archive::at(root)
    } else {
        Archive::open(root)?
    };
    let _lock = if options.dry_run {
        None
    } else {
        Some(archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?)
    };
    let loaded = library::load(&archive)?;
    let known = library::known_urls(&loaded.snapshots);
    let mut state = State::read(root)?;
    let existing = read(root)?;
    let active: HashSet<String> = state.spellings(false).into_keys().collect();
    let mut parsed = parse(&text, options, &known, &active)?;
    let missing = coverage(file, &mut parsed, options.partial);
    if parsed.answers.is_empty() && parsed.problems.is_empty() {
        parsed.problems.push("there are no lines".to_owned());
    }
    if !parsed.problems.is_empty() {
        return Err(Error::Import {
            path: file.to_path_buf(),
            problems: parsed.problems,
        });
    }
    let source = parsed
        .source
        .map(|(name, _)| name)
        .expect("a line has a source");
    let current = tags::vocabulary_version(&state);
    let version = prompted_version(file).unwrap_or_else(|| current.clone());
    let mut report = Report {
        dry_run: options.dry_run,
        vocabulary_changed: version != current,
        vocabulary_version: version,
        source,
        pages: parsed.answers.len(),
        missing,
        ..Report::default()
    };
    // New names join the vocabulary before parent rules run, so a rule on a
    // name just brought back applies. In a dry run the state is not written.
    let now = tags::now();
    for (name, _) in parsed.unknown.values() {
        match tags::admit(&mut state, name, now) {
            (_, tags::Admitted::Known) => {}
            (spelling, tags::Admitted::Created) => report.created.push(spelling),
            (spelling, tags::Admitted::Revived) => report.revived.push(spelling),
        }
    }
    let lines = suggest(&state, &existing, parsed.answers, &mut report, now);
    if options.dry_run {
        return Ok(report);
    }
    if !(report.created.is_empty() && report.revived.is_empty()) {
        state.write(root)?;
    }
    if !lines.is_empty() {
        append(root, &lines)?;
    }
    Ok(report)
}

/// The lines to store: each answer with its parent rules applied, less
/// those this source already gave exactly so, under the same version.
fn suggest(
    state: &State,
    existing: &[Line],
    answers: Vec<Resolved>,
    report: &mut Report,
    now: Timestamp,
) -> Vec<Line> {
    let previous = latest(existing);
    let mut lines = Vec::new();
    for answer in answers {
        let tags = tags::with_parents(state, &answer.tags);
        report.implied += tags.len().saturating_sub(answer.tags.len());
        report.suggestions += tags.len();
        if tags.is_empty() {
            report.empty += 1;
        } else {
            report.tagged += 1;
        }
        let same = previous
            .get(answer.url.as_str())
            .and_then(|sources| sources.get(&fold(&report.source)))
            .is_some_and(|line| {
                line.vocabulary_version == report.vocabulary_version
                    && folded_set(&line.tags) == folded_set(&tags)
            });
        if same {
            report.unchanged += 1;
            continue;
        }
        lines.push(Line {
            url: answer.url,
            tags,
            source: report.source.clone(),
            imported_at: now,
            vocabulary_version: report.vocabulary_version.clone(),
            extra: serde_json::Map::new(),
        });
    }
    lines
}

fn folded_set(names: &[String]) -> std::collections::BTreeSet<String> {
    names.iter().map(|name| fold(name)).collect()
}

/// "3, 7, 12", or the first ten and a count.
fn numbers(lines: &[usize]) -> String {
    let mut text = lines
        .iter()
        .take(10)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if lines.len() > 10 {
        let _ = write!(text, " and {} more", lines.len() - 10);
    }
    text
}

/// `knowmoretabs tag --import FILE`.
pub fn import_command(
    root: &Path,
    options: &ImportOptions,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let report = import(root, options, log)?;
    if json {
        out::json(&serde_json::json!({ "imported": report }));
    } else if !log.quiet {
        out::line(&human_report(&report, options.file, &path(root)));
    }
    Ok(())
}

fn human_report(report: &Report, file: &Path, store: &Path) -> String {
    let mut text = if report.dry_run {
        format!(
            "valid: {} from {}",
            plural(report.pages, "page"),
            report.source
        )
    } else {
        format!(
            "imported suggestions for {} from {}",
            plural(report.pages, "page"),
            report.source
        )
    };
    let _ = write!(
        text,
        ": {} with tags ({}",
        report.tagged,
        plural(report.suggestions, "suggestion")
    );
    if report.implied > 0 {
        let _ = write!(text, ", {} from parent rules", report.implied);
    }
    let _ = write!(text, "), {} with none", report.empty);
    if report.unchanged > 0 {
        let _ = write!(text, "; {} already imported as they are", report.unchanged);
    }
    if report.missing > 0 {
        let _ = write!(
            text,
            "; {} from the prompt left out (--partial)",
            plural(report.missing, "page")
        );
    }
    let created = if report.dry_run {
        "would create"
    } else {
        "new tags"
    };
    if !report.created.is_empty() {
        let _ = write!(text, "; {created}: {}", report.created.join(", "));
    }
    if !report.revived.is_empty() {
        let _ = write!(
            text,
            "; back from retirement: {}",
            report.revived.join(", ")
        );
    }
    if report.vocabulary_changed {
        let _ = write!(
            text,
            "; your vocabulary has changed since this prompt was written, so these are recorded under its version {}",
            report.vocabulary_version
        );
    }
    if report.dry_run {
        let _ = write!(
            text,
            ". Nothing stored: {} is ready to import",
            file.display()
        );
    } else {
        let _ = write!(text, ". Stored in {}", store.display());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_positions_are_dropped_since_every_line_is_line_one() {
        let err =
            serde_json::from_str::<Answer>(r#"{"url":"u","tags":[],"reason":"x"}"#).unwrap_err();
        let text = without_position(&err);
        assert!(text.starts_with("unknown field `reason`"), "{text}");
        assert!(!text.contains("column"), "{text}");
    }

    #[test]
    fn line_lists_are_capped() {
        assert_eq!(numbers(&[3, 7]), "3, 7");
        let many: Vec<usize> = (1..=12).collect();
        assert_eq!(numbers(&many), "1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 2 more");
    }

    #[test]
    fn sources_are_names() {
        assert_eq!(source_name("  claude  opus ").unwrap(), "claude opus");
        assert!(source_name(" ").is_err());
        assert!(source_name(&"x".repeat(81)).is_err());
        assert!(source_name("a\u{7}").is_err());
    }
}
