//! `knowmoretabs tag --prompt DIR`: a work folder the owner hands to an agent
//! of their choice, which reads the pages and writes suggested tags back.
//!
//! slice: triage
//! why: The tool contains no model and makes no request, so tagging is a
//!      handoff: this writes the instructions, the vocabulary and the pages,
//!      and `tag --import` validates what comes back. `prompt.md` is the
//!      product: it carries the owner's settled tagging model (flat facets,
//!      broad tags, no exclusions, "substantially about", favour recall),
//!      what each tag means, the exact answer format and a check the agent
//!      runs before it stops. The folder holds page text from a private
//!      library, so it is created private, never inside the archive, and
//!      search terms and referrers go into it only when asked for.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::archive::{self, Archive};
use crate::capture::Log;
use crate::error::Error;
use crate::library::{self, Shape, State, Term, fold, tag_order};
use crate::metadata::{self, Status};
use crate::model::Snapshot;
use crate::suggestions::{self, PAGES_FILE, VOCABULARY_FILE};
use crate::triage::plural;
use crate::{out, tags};

pub const PROMPT_FILE: &str = "prompt.md";
pub const ANSWER_FILE: &str = "tags.jsonl";
/// Enough of a README to say what the repository is; a passing mention
/// further down is not a topic.
const README_LIMIT: usize = 1500;
/// Pages per reading chunk the prompt asks for.
const CHUNK: usize = 50;

pub struct PromptOptions<'a> {
    pub dir: &'a Path,
    /// Every page the library shows, not only the ones not yet tagged.
    pub all: bool,
    /// Put search terms and referrers from History into the pages.
    pub with_history: bool,
}

/// One line of `pages.jsonl`. Only `url` and `title` are always there.
#[derive(Debug, Default, Serialize)]
struct PageLine {
    url: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    topics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    readme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    referrer: Option<String>,
}

#[derive(Debug, Serialize)]
struct VocabularyTag<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    definition: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    implies: Vec<String>,
}

/// `vocabulary.json`: what the agent was shown, and the version `--import`
/// records when it finds this file beside the answer.
#[derive(Debug, Serialize)]
struct VocabularyFile<'a> {
    version: &'a str,
    tags: &'a [VocabularyTag<'a>],
}

#[derive(Debug, Default, Serialize)]
pub struct Written {
    /// `None` when no page was due and nothing was written.
    pub dir: Option<PathBuf>,
    pub pages: usize,
    pub vocabulary_version: String,
    pub tags: usize,
    /// Active tags with no definition: the agent gets only their name.
    pub undefined: Vec<String>,
    /// Pages left out because they already show tags of the owner's, or
    /// already have an imported answer. Both zero with `--all`.
    pub skipped_tagged: usize,
    pub skipped_suggested: usize,
    /// Pages that carry text `enrich` fetched.
    pub enriched: usize,
    pub with_history: bool,
}

/// Single-quoted for a POSIX shell, so a path with spaces still runs.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(collapse).filter(|text| !text.is_empty())
}

/// Refuses a folder that holds anything (an earlier agent's answer must
/// never be overwritten) and one inside, or around, the archive.
fn check_folder(root: &Path, dir: &Path) -> Result<PathBuf, Error> {
    let refuse = |reason| Error::PromptFolder {
        path: dir.to_path_buf(),
        reason,
    };
    let absolute = crate::export::resolve_destination(dir)?;
    match fs::read_dir(&absolute) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(refuse("it is not empty"));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => {
            return Err(refuse("it is not a folder"));
        }
        Err(err) => return Err(Error::io("read", &absolute)(err)),
    }
    if absolute.is_file() {
        return Err(refuse("it is not a folder"));
    }
    let root = crate::export::resolve_destination(root)?;
    if crate::platform::contains_path(&root, &absolute)
        || crate::platform::contains_path(&absolute, &root)
    {
        return Err(refuse("it overlaps the archive"));
    }
    Ok(absolute)
}

/// Writes the work folder for every page not yet tagged: shown in the
/// library (not forgotten), carrying none of the owner's tags, and with no
/// imported answer from any source.
pub fn write(root: &Path, options: &PromptOptions, log: Log) -> Result<Written, Error> {
    // Refuse before creating anything, the archive included.
    if State::read(root)?.active_vocabulary().is_empty() {
        return Err(Error::EmptyVocabulary);
    }
    let dir = check_folder(root, options.dir)?;
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| log.warn("another knowmoretabs run holds the archive; waiting"))?;
    let loaded = library::load(&archive)?;
    let state = State::read(root)?;
    let tags = vocabulary(&state);
    if tags.is_empty() {
        return Err(Error::EmptyVocabulary);
    }
    let lines = suggestions::read(root)?;
    let answered = suggestions::answered(&lines);
    let metadata = metadata::read(root)?;
    if metadata.unreadable > 0 {
        log.warn(&format!(
            "skipped {} unreadable in {}",
            plural(metadata.unreadable, "line"),
            metadata::path(root).display()
        ));
    }
    let spellings = state.spellings(false);
    // Export shape: forgotten pages are left out, so a page the owner hid
    // never reaches their agent.
    let library = library::build(&loaded.snapshots, &state, &HashMap::new(), Shape::Export);
    let mut written = Written {
        vocabulary_version: tags::vocabulary_version(&state),
        tags: tags.len(),
        undefined: tags
            .iter()
            .filter(|tag| tag.definition.is_none())
            .map(|tag| tag.name.to_owned())
            .collect(),
        with_history: options.with_history,
        ..Written::default()
    };
    let mut pages = Vec::new();
    for (url, title) in library.titles() {
        if !options.all {
            if !state.page_tags(url, &spellings).is_empty() {
                written.skipped_tagged += 1;
                continue;
            }
            if answered.contains(url) {
                written.skipped_suggested += 1;
                continue;
            }
        }
        let mut page = PageLine {
            url: url.to_owned(),
            title: collapse(title),
            ..PageLine::default()
        };
        if let Some(record) = metadata.pages.get(url) {
            page.enrich(record);
            if page.description.is_some() || page.readme.is_some() || page.kind.is_some() {
                written.enriched += 1;
            }
        }
        if options.with_history {
            page.remember(&loaded.snapshots, &state);
        }
        pages.push(page);
    }
    written.pages = pages.len();
    if !pages.is_empty() {
        publish(root, &dir, &pages, &tags, &written)?;
        written.dir = Some(dir);
    }
    Ok(written)
}

impl PageLine {
    /// What `enrich` fetched, when the fetch saw the page itself. A page
    /// behind a login keeps only its tab title: the fetch saw a login form.
    fn enrich(&mut self, record: &metadata::Record) {
        if record.status != Status::Ok {
            return;
        }
        self.page_title =
            non_empty(record.title.as_deref()).filter(|fetched| fold(fetched) != fold(&self.title));
        self.description = non_empty(record.best_description());
        self.site = non_empty(record.og.as_ref().and_then(|og| og.site_name.as_deref()));
        self.kind = if record.jsonld_types.is_empty() {
            // Every page is a "website"; saying so tells a reader nothing.
            non_empty(record.og.as_ref().and_then(|og| og.kind.as_deref()))
                .filter(|kind| kind != "website")
        } else {
            Some(record.jsonld_types.join(", "))
        };
        if let Some(github) = &record.github {
            self.topics.clone_from(&github.topics);
            self.readme = non_empty(github.readme.as_deref())
                .map(|text| text.chars().take(README_LIMIT).collect());
        }
    }

    /// The search and referrer from the newest snapshot that has signals
    /// for this page. A referrer the owner has forgotten stays out.
    fn remember(&mut self, snapshots: &[Snapshot], state: &State) {
        let history = snapshots
            .iter()
            .rev()
            .flat_map(|snapshot| &snapshot.tabs)
            .find(|tab| tab.url == self.url && tab.history.is_some())
            .and_then(|tab| tab.history.as_ref());
        if let Some(history) = history {
            self.search = history
                .search
                .as_ref()
                .and_then(|search| non_empty(Some(&search.term)));
            self.referrer = history
                .referrer
                .clone()
                .filter(|referrer| !state.forgotten.contains(referrer));
        }
    }
}

/// The active vocabulary in tag order, parents resolved to active names.
fn vocabulary(state: &State) -> Vec<VocabularyTag<'_>> {
    let spellings = state.spellings(false);
    let mut terms: Vec<(&String, &Term)> = state
        .vocabulary
        .iter()
        .filter(|(_, term)| term.retired_at.is_none())
        .collect();
    terms.sort_by(|(a, _), (b, _)| tag_order(a, b));
    terms
        .into_iter()
        .map(|(name, term)| {
            let mut implies: Vec<String> = term
                .implies
                .iter()
                .filter_map(|parent| spellings.get(&fold(parent)).map(|s| (*s).to_owned()))
                .collect();
            implies.sort_by(|a, b| tag_order(a, b));
            VocabularyTag {
                name,
                definition: term.definition.as_deref(),
                implies,
            }
        })
        .collect()
}

/// The three files, each replaced atomically, `prompt.md` last so that a
/// folder with a prompt in it is a complete folder.
fn publish(
    root: &Path,
    dir: &Path,
    pages: &[PageLine],
    tags: &[VocabularyTag],
    written: &Written,
) -> Result<(), Error> {
    let json_error = |name: &str| {
        let path = dir.join(name);
        move |source| Error::Json { path, source }
    };
    archive::create_private_dir(dir).map_err(Error::io("create", dir))?;
    let mut jsonl = Vec::new();
    let mut fields = BTreeSet::new();
    for page in pages {
        let value = serde_json::to_value(page).map_err(json_error(PAGES_FILE))?;
        fields.extend(
            value
                .as_object()
                .into_iter()
                .flat_map(|o| o.keys().cloned()),
        );
        // The struct, not the value, so `url` leads every line.
        serde_json::to_writer(&mut jsonl, page).map_err(json_error(PAGES_FILE))?;
        jsonl.push(b'\n');
    }
    archive::replace_file(&dir.join(PAGES_FILE), &jsonl)?;
    let file = VocabularyFile {
        version: &written.vocabulary_version,
        tags,
    };
    let mut vocabulary = serde_json::to_vec_pretty(&file).map_err(json_error(VOCABULARY_FILE))?;
    vocabulary.push(b'\n');
    archive::replace_file(&dir.join(VOCABULARY_FILE), &vocabulary)?;
    let prompt = render(dir, written, tags, &fields, &check_command(root, dir));
    archive::replace_file(&dir.join(PROMPT_FILE), prompt.as_bytes())
}

/// The validation command, with this binary's own path and the archive's,
/// so it runs from wherever the agent's shell is.
fn check_command(root: &Path, dir: &Path) -> String {
    let exe = std::env::current_exe().map_or_else(|_| "knowmoretabs".to_owned(), |exe| quote(&exe));
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    format!(
        "{exe} --root {} tag --import {} --dry-run",
        quote(&root),
        quote(&dir.join(ANSWER_FILE))
    )
}

/// `prompt.md`. Plain Markdown that reads the same to a person as to an
/// agent: the owner is meant to read it before handing it over. `fields`
/// names the page fields that occur, so only those are explained.
fn render(
    dir: &Path,
    written: &Written,
    tags: &[VocabularyTag],
    fields: &BTreeSet<String>,
    check: &str,
) -> String {
    let mut md = introduction(dir, written.pages);
    md.push_str(GUIDELINES);
    let _ = write!(
        md,
        "\n## The tags\n\n{}, vocabulary version `{}`.\n\n",
        plural(tags.len(), "tag"),
        written.vocabulary_version
    );
    for tag in tags {
        let _ = write!(md, "- **{}**: ", tag.name);
        md.push_str(
            tag.definition
                .unwrap_or("(no definition yet: use the plain meaning of the name)"),
        );
        match tag.implies.as_slice() {
            [] => {}
            [parent] => {
                let _ = write!(
                    md,
                    " Whenever it applies, {parent} applies too; the import adds it if you leave it out."
                );
            }
            parents => {
                let _ = write!(
                    md,
                    " Whenever it applies, {} apply too; the import adds them if you leave them out.",
                    parents.join(" and ")
                );
            }
        }
        md.push('\n');
    }
    md.push_str("\n## What a page line holds\n\n");
    for (field, text) in FIELDS {
        if matches!(*field, "url" | "title") || fields.contains(*field) {
            let _ = writeln!(md, "- `{field}`: {text}");
        }
    }
    if fields
        .iter()
        .any(|field| !matches!(field.as_str(), "url" | "title"))
    {
        md.push_str(
            "\nOnly `url` and `title` are on every line; the rest appear when they are known.\n",
        );
    }
    md.push_str(&how_to_work(written.pages));
    md.push_str(ANSWER);
    let _ = write!(md, "{}", check_section(check));
    md
}

fn introduction(dir: &Path, pages: usize) -> String {
    format!(
        "# Tag these pages

You are tagging pages from someone's library of browser tabs. They keep every tab they have had \
open, and they find pages again by filtering on tags. Read each page in `pages.jsonl` and decide \
which of their tags it carries. Your answers become suggestions: the owner reviews them, and their \
own decisions always win.

Your work folder is `{dir}`. It holds:

- `prompt.md`: this file.
- `pages.jsonl`: {pages}, one JSON object per line.
- `vocabulary.json`: the tags below, for scripts that check your answer.

You write `tags.jsonl` here. It does not exist yet.
",
        dir = dir.display(),
        pages = plural(pages, "page"),
    )
}

/// The owner's tagging model, settled in the lab and in slice 7's brief.
const GUIDELINES: &str = "
## How the owner tags

1. **Tags are flat facets.** Each tag answers one question on its own: is this page substantially \
about this? There is no hierarchy. The meaning is in the combination: two tags together narrow a \
filter, and one tag alone is meant to be broad. A page can carry several tags at once, and many do.
2. **Broad tags are good.** Read every definition in its widest plain sense. A tag covers its whole \
domain, not only its most typical example.
3. **No exclusions.** Never leave a tag off because another tag also fits, or because another tag \
seems more specific. There are no \"X, not Y\" rules: if both apply, give both.
4. **\"Substantially about\" is the bar.** A tag applies when the page is substantially about that \
thing: it is the page's subject, its purpose, or a large part of what it covers. A passing mention \
does not count: a sponsor line, a dependency in a README, \"works with X\" in a feature list, or a \
page saying it is *not* an X.
5. **Favour recall.** A missing tag hides the page from every filter that uses that tag, and the \
owner may never see it again. An extra tag costs them one click to dismiss. When a tag plausibly \
applies in substance, include it.
6. **Thin pages.** Some pages have only a title and an address. Tag them when those make the \
subject clear; otherwise give them no tags.
7. **Only these tags.** Use the names below, spelled as written. Don't invent tags. If a subject \
keeps coming up (on three pages or more) and no tag covers it, add a line about it to `notes.md` \
in this folder for the owner, and keep it out of `tags.jsonl`.
";

/// Every field a page line can have, in the order they are explained.
const FIELDS: &[(&str, &str)] = &[
    (
        "url",
        "the page's address. Copy it exactly into your answer.",
    ),
    (
        "title",
        "the title the owner saw in the browser tab. For a page behind a login it is often the \
best evidence there is.",
    ),
    (
        "page_title",
        "the title the page itself declares, when it differs from the tab's.",
    ),
    ("description", "the page's own summary of itself."),
    ("site", "the site's name for itself."),
    (
        "kind",
        "what sort of page it says it is (an article, a video, source code).",
    ),
    (
        "topics",
        "a GitHub repository's topics, as its authors set them.",
    ),
    (
        "readme",
        "the start of a GitHub repository's README. Judge the repository by what it is for; a \
name dropped further down is a passing mention.",
    ),
    (
        "search",
        "what the owner searched for on the way to this page. Good evidence of what they wanted \
from it.",
    ),
    ("referrer", "the address of the page they came from."),
];

fn how_to_work(pages: usize) -> String {
    format!(
        "
## How to work

1. Read `pages.jsonl` in chunks of about {CHUNK} lines, in order, until you have read all {pages}. \
Don't load the whole file at once.
2. Tag each page yourself, by reading it and weighing it against every tag. Don't assign tags \
with keyword or regex scripts, and don't call a model or classifier API: the owner wants your \
reading. Scripts are fine for reading chunks, writing lines and checking the file.
3. After each chunk, append its lines to `tags.jsonl`. Work then survives an interruption: to \
resume, find the last URL in `tags.jsonl` and carry on from the page after it.
4. Write only inside this folder. Change nothing else on this machine, and make no network \
requests: everything you need is here.
5. These pages are the owner's private browsing. Don't copy them anywhere outside this folder.
"
    )
}

const ANSWER: &str = "
## Your answer: `tags.jsonl`

One line per page in `pages.jsonl`, each URL exactly once, in any order:

```json
{\"url\": \"https://example.com/a-page\", \"tags\": [\"First tag\", \"Second tag\"], \"source\": \"your-model-name\"}
```

- `url`: exactly as it is in `pages.jsonl`.
- `tags`: every tag that applies, spelled as in the list above. A page that no tag fits gets `[]`. \
That is an answer too, so write its line.
- `source`: the model you are running as, as precisely as you know it (for example \
`claude-opus-5-5` or `gpt-6`). The same on every line.
- `note` (optional): one short sentence, only for a hard call.

No other fields, and nothing else in the file.
";

fn check_section(check: &str) -> String {
    format!(
        "
## Before you finish: check your answer

Run this command exactly as written:

```sh
{check}
```

It checks every line against the owner's library and stores nothing. It passes when it prints a \
line starting `valid:`. Otherwise it names each problem with its line number: fix them all and run \
it again, until it passes. Every page must have a line. Don't run it without `--dry-run`: storing \
the suggestions is the owner's decision.

When it passes, reply with how many pages you tagged, how many got at least one tag, and the path \
to `tags.jsonl`.
"
    )
}

/// `knowmoretabs tag --prompt DIR`.
pub fn prompt_command(
    root: &Path,
    options: &PromptOptions,
    json: bool,
    log: Log,
) -> Result<(), Error> {
    let written = write(root, options, log)?;
    if !written.undefined.is_empty() {
        log.warn(&format!(
            "no definition for {}: the agent gets only the name; add one with `knowmoretabs tags --define NAME TEXT`",
            written.undefined.join(", ")
        ));
    }
    if json {
        out::json(&serde_json::json!({ "prompt": written }));
        return Ok(());
    }
    if log.quiet {
        return Ok(());
    }
    let Some(dir) = &written.dir else {
        out::line(&format!(
            "nothing to tag: every page in your library shows your tags or has suggestions already ({} tagged, {} with suggestions); --all includes them",
            written.skipped_tagged, written.skipped_suggested
        ));
        return Ok(());
    };
    let mut text = format!(
        "wrote a prompt for {} to {} ({}, vocabulary version {})\n",
        plural(written.pages, "page"),
        dir.display(),
        plural(written.tags, "tag"),
        written.vocabulary_version
    );
    let skipped = written.skipped_tagged + written.skipped_suggested;
    if skipped > 0 {
        let _ = writeln!(
            text,
            "left out {} already tagged or with suggestions; --all includes them",
            plural(skipped, "page")
        );
    }
    let mut holds = "the addresses and titles of those pages".to_owned();
    if written.enriched > 0 {
        let _ = write!(holds, ", text fetched for {} of them", written.enriched);
    }
    if written.with_history {
        holds.push_str(", and the searches and referrers History recorded");
    }
    let _ = writeln!(
        text,
        "The folder holds {holds}. Where it goes is your choice: knowmoretabs sends nothing."
    );
    let _ = writeln!(
        text,
        "Next: open any agent in that folder and say \"Read prompt.md and carry it out.\""
    );
    let _ = writeln!(
        text,
        "Then: knowmoretabs tag --import {}",
        quote(&dir.join(ANSWER_FILE))
    );
    out::block(&text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn field_names(page: &PageLine) -> HashSet<String> {
        serde_json::to_value(page)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn quoting_survives_spaces_and_quotes() {
        assert_eq!(quote(Path::new("/a b/c")), "'/a b/c'");
        assert_eq!(quote(Path::new("/it's")), r"'/it'\''s'");
    }

    #[test]
    fn a_bare_page_has_only_url_and_title() {
        let page = PageLine {
            url: "u".into(),
            title: "t".into(),
            ..PageLine::default()
        };
        assert_eq!(
            field_names(&page),
            HashSet::from(["url".to_owned(), "title".to_owned()])
        );
    }

    #[test]
    fn the_prompt_explains_only_the_fields_present() {
        let written = Written {
            pages: 2,
            vocabulary_version: "abc".into(),
            ..Written::default()
        };
        let tags = [
            VocabularyTag {
                name: "DPO",
                definition: Some("Direct Preference Optimization."),
                implies: vec!["Training".into()],
            },
            VocabularyTag {
                name: "Voice",
                definition: None,
                implies: Vec::new(),
            },
            VocabularyTag {
                name: "X",
                definition: Some("Ex."),
                implies: vec!["A".into(), "B".into()],
            },
        ];
        let fields = BTreeSet::from(["url".to_owned(), "title".to_owned(), "topics".to_owned()]);
        let md = render(Path::new("/w"), &written, &tags, &fields, "check");
        assert!(md.contains("- **DPO**: Direct Preference Optimization. Whenever it applies, Training applies too; the import adds it if you leave it out.\n"));
        assert!(md.contains("- **X**: Ex. Whenever it applies, A and B apply too; the import adds them if you leave them out.\n"));
        assert!(
            md.contains("- **Voice**: (no definition yet: use the plain meaning of the name)\n")
        );
        assert!(md.contains("- `url`: "));
        assert!(md.contains("- `topics`: "));
        assert!(!md.contains("- `search`"));
        assert!(!md.contains("- `readme`"));
        assert!(md.contains("```sh\ncheck\n```"));
    }
}
