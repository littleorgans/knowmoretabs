//! What a page says: a web page's kept text or a local document's pinned
//! bytes, by its key, and `knowmoretabs read`.
//!
//! slice: sources
//! why: Search, read and the drawer each need a page's text, and a web page
//!      and a document keep it in different places: the content store, and
//!      git or the shelf through the resolver. One reader answers for both,
//!      so no caller chooses for itself and none can miss a kind, as the
//!      drawer once did for documents. Pins and checkouts are read only on
//!      the first document, so a damaged checkouts file never stops a web
//!      page's text. No access rule lives here: each caller keeps its own
//!      rule for which pages it answers. `read` answers the pages search
//!      does, added, listed and not forgotten, never a snapshot's, and
//!      prints their text, so an agent or the tagger lab gets it from this
//!      one reader rather than a second copy of it.

use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use serde_json::{Value, json};

use crate::capture::Log;
use crate::error::{self, Error};
use crate::jsonl;
use crate::library::{self, State};
use crate::model::Tab;
use crate::source_key::{self, Key};
use crate::sources::{self, Checkouts, Pin, Resolver};
use crate::{content_store, intake, out};

/// The version of `read --json`'s lines.
pub const SCHEMA_VERSION: u32 = 1;

/// What a key is, by its prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Web,
    Git,
    Blob,
}

impl Kind {
    pub fn of(key: &str) -> Self {
        match source_key::parse(key) {
            Some(Key::Git { .. }) => Self::Git,
            Some(Key::Blob { .. }) => Self::Blob,
            None => Self::Web,
        }
    }
}

/// A page's text, and the title it gives itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub title: Option<String>,
    pub body: String,
}

/// One page's answer. Serializes as the fields search results and read
/// lines share: kind, state, why and source.
#[derive(Debug, Serialize)]
pub struct PageText {
    pub kind: Kind,
    /// The pin's resolution; `None` for a web page.
    pub state: Option<sources::State>,
    pub why: Option<&'static str>,
    /// The pin as a result names it; null for a web page or no pin.
    pub source: Value,
    #[serde(skip)]
    pub text: Option<Text>,
}

/// Reads pages' text. The pins and the checkouts file are read on the
/// first document, and git is asked once per repository.
pub struct Reader<'a> {
    root: &'a Path,
    checkouts: Option<&'a Path>,
    documents: Option<(jsonl::Latest<Pin>, Resolver)>,
}

impl<'a> Reader<'a> {
    /// Without a checkouts file, documents resolve against none and come
    /// back unresolved.
    pub fn new(root: &'a Path, checkouts: Option<&'a Path>) -> Self {
        Self {
            root,
            checkouts,
            documents: None,
        }
    }

    pub fn text(&mut self, key: &str) -> Result<PageText, Error> {
        let kind = Kind::of(key);
        if kind == Kind::Web {
            return Ok(PageText {
                kind,
                state: None,
                why: None,
                source: Value::Null,
                text: content_store::kept_text(self.root, key)?,
            });
        }
        let (pins, resolver) = match &mut self.documents {
            Some(documents) => documents,
            none => {
                let checkouts = match self.checkouts {
                    Some(path) => Checkouts::read(path)?,
                    None => Checkouts::default(),
                };
                none.insert((sources::pins(self.root)?, Resolver::new(checkouts)))
            }
        };
        let pin = pins.pages.get(key);
        let pinned = resolver.resolve(pin);
        Ok(PageText {
            kind,
            state: Some(pinned.state),
            why: pinned.why,
            source: pin.map_or(Value::Null, Pin::source),
            text: pinned.text,
        })
    }
}

/// The pages search and read answer: added pages the library lists, less
/// forgotten ones, in intake order; and the state already read for eligibility.
pub fn added(root: &Path) -> Result<(Vec<Tab>, State), Error> {
    let state = State::read(root)?;
    let tabs = intake::snapshot(root)?.map_or_else(Vec::new, |added| {
        added
            .tabs
            .into_iter()
            .filter(|tab| library::is_listed(&tab.url) && !state.forgotten.contains(&tab.url))
            .collect()
    });
    Ok((tabs, state))
}

/// One key's line under `--json`.
#[derive(Serialize)]
struct Answer<'a> {
    schema_version: u32,
    url: &'a str,
    #[serde(flatten)]
    page: &'a PageText,
    title: Option<&'a str>,
    text: &'a str,
}

/// `knowmoretabs read KEY...`: each added page's text, a web page's as
/// kept and a document's as pinned. One key prints its text; with `--json`,
/// one line per key in the order given. Exits 1 when any key is refused,
/// after every line.
pub fn read_command(
    root: &Path,
    keys: &[String],
    checkouts: &Path,
    json: bool,
    log: Log,
) -> Result<ExitCode, Error> {
    if !json && keys.len() > 1 {
        return Err(Error::ReadSeveral);
    }
    let (added, state) = added(root)?;
    let added: std::collections::BTreeSet<&str> =
        added.iter().map(|tab| tab.url.as_str()).collect();
    let mut reader = Reader::new(root, Some(checkouts));
    // Every key is read before a line is printed, so a failure to read at
    // all is the one error, never a line among answers.
    let answers = keys
        .iter()
        .map(|key| {
            let refused = |reason: &str| Error::SourceRead {
                key: key.clone(),
                reason: reason.to_owned(),
            };
            if state.forgotten.contains(key) {
                return Ok(Err(refused("it is forgotten; restore it first")));
            }
            if !added.contains(key.as_str()) {
                return Ok(Err(refused("not a page you added; search lists them")));
            }
            let mut page = reader.text(key)?;
            Ok(match page.text.take() {
                Some(text) => Ok((page, text)),
                None if page.kind == Kind::Web => Err(refused("no text kept for it")),
                None => Err(refused(page.why.unwrap_or("unreadable"))),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if !json {
        let Some(answer) = answers.into_iter().next() else {
            return Ok(ExitCode::SUCCESS);
        };
        let (page, text) = answer?;
        if let Some(why) = page.why {
            log.warn(&format!(
                "{} is stale: {why}; this is the text as added",
                keys[0]
            ));
        }
        out::block(&text.body);
        return Ok(ExitCode::SUCCESS);
    }
    for (key, answer) in keys.iter().zip(&answers) {
        out::json(&match answer {
            Ok((page, text)) => json!(Answer {
                schema_version: SCHEMA_VERSION,
                url: key,
                page,
                title: text.title.as_deref(),
                text: &text.body,
            }),
            Err(err) => {
                let mut line = error::render_json(err);
                line["url"] = json!(key);
                line
            }
        });
    }
    Ok(if answers.iter().all(Result::is_ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::archive;
    use crate::digest::sha256_hex;
    use crate::sources::Format;

    const WEB: &str = "https://web.test/page";

    fn kept(root: &Path, url: &str, file: &str) {
        let dir = content_store::dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(content_store::file_name(url)), file).unwrap();
    }

    #[test]
    fn a_web_page_reads_as_kept_and_names_itself_by_its_front_matter() {
        let root = tempfile::tempdir().unwrap();
        let mut reader = Reader::new(root.path(), None);
        let page = reader.text(WEB).unwrap();
        assert_eq!((page.kind, page.state, page.text), (Kind::Web, None, None));
        kept(
            root.path(),
            WEB,
            "---\nschema_version: 1\ntitle: \"Kept\"\n---\n# Body\n",
        );
        assert_eq!(
            reader.text(WEB).unwrap().text,
            Some(Text {
                title: Some("Kept".to_owned()),
                body: "# Body\n".to_owned(),
            })
        );
    }

    #[test]
    fn a_document_reads_its_pin_from_the_shelf_and_none_is_unresolved() {
        let root = tempfile::tempdir().unwrap();
        let shelf = tempfile::tempdir().unwrap();
        let bytes = b"# Shelf note\nwords\n";
        let sha = sha256_hex(bytes);
        let key = source_key::blob(&sha);
        let pin = Pin::new(&key, Format::Md, archive::now());
        let pins = sources::path(root.path());
        std::fs::create_dir_all(pins.parent().unwrap()).unwrap();
        std::fs::write(&pins, format!("{}\n", serde_json::to_string(&pin).unwrap())).unwrap();
        std::fs::write(shelf.path().join(format!("{sha}.md")), bytes).unwrap();
        let checkouts = root.path().join("checkouts.json");
        std::fs::write(
            &checkouts,
            json!({"schema_version": 1, "shelf": shelf.path()}).to_string(),
        )
        .unwrap();

        let page = Reader::new(root.path(), Some(&checkouts))
            .text(&key)
            .unwrap();
        assert_eq!(
            (page.kind, page.state),
            (Kind::Blob, Some(sources::State::Fresh))
        );
        assert_eq!(page.text.unwrap().body, "# Shelf note\nwords\n");
        assert_eq!(json!(page.source)["blob"], Value::Null);

        let unpinned = source_key::blob(&"0".repeat(64));
        let page = Reader::new(root.path(), Some(&checkouts))
            .text(&unpinned)
            .unwrap();
        assert_eq!(page.state, Some(sources::State::Unresolved));
        assert_eq!(page.why, Some("no pin; add the file again"));
        assert_eq!((page.source, page.text), (Value::Null, None));
    }

    #[test]
    fn a_web_page_answers_while_the_checkouts_file_is_damaged() {
        let root = tempfile::tempdir().unwrap();
        let checkouts = root.path().join("checkouts.json");
        std::fs::write(&checkouts, "not json").unwrap();
        kept(root.path(), WEB, "plain body\n");
        let mut reader = Reader::new(root.path(), Some(&checkouts));
        assert_eq!(reader.text(WEB).unwrap().text.unwrap().body, "plain body\n");
        assert!(matches!(
            reader.text(&source_key::blob(&"0".repeat(64))),
            Err(Error::Checkouts(_))
        ));
    }
}
