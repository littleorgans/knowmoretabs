//! A page's main text as markdown, and what kind of page it is: an
//! article, a short page, an app shell, a sign-in form or a paywall.
//!
//! slice: content
//! why: Most of what makes a page worth keeping is its main text, and most
//!      of what a page sends is not: menus, footers, scripts. Readability
//!      finds the text and writes it as markdown, and a few measured rules
//!      say whether what it found is the page or a sign that the page needs
//!      a browser, a login or a subscription to show it. When readability
//!      picks the wrong part of a page that plainly has text, as listing and
//!      index pages lead it to, the whole page less its menus, banners,
//!      footers and sidebars outside `main` is kept instead, and judged on
//!      what is left. Inside `main`, those elements can hold page headings
//!      and usage instructions, so their text is kept.
//!      Pure: HTML in, a verdict out, so the rules can be checked against
//!      pages alone.

use dom_query::{Document, NodeRef};
use dom_smoothie::{Config, Readability, TextMode};

use crate::head;
use crate::jsonld;

pub const EXTRACTOR: &str = "dom_smoothie";
/// The `dom_smoothie` release in `Cargo.lock`; a unit test holds them equal.
pub const EXTRACTOR_VERSION: &str = "0.18.2";
/// The extractor recorded when the whole page was kept.
pub const FULL_PAGE: &str = "full-page";
/// Characters of text that make a page more than a stub.
pub const ENOUGH: usize = 1500;
/// Fewer visible characters than this, with scripts, is a page drawn by them.
const SHELL: usize = 300;
/// Scripts that run, as opposed to data such as JSON-LD.
const SCRIPTS: &str = r#"script:not([type]), script[type=""], script[type="module"], script[type*="javascript" i], script[type*="ecmascript" i]"#;
/// Elements a single-page app mounts itself into.
const APP_MOUNTS: &str = "#root, #app, #__next, #__nuxt";
/// What the whole-page markdown leaves out.
const NOT_TEXT: [&str; 8] = [
    "head", "script", "style", "noscript", "template", "svg", "meta", "iframe",
];
/// Page chrome the whole-page markdown leaves out when it is outside
/// `main`: menus, banners, footers and sidebars. Inside `main`, the same
/// elements can hold substantive headings and usage instructions.
const CHROME: &str = "nav, header, footer, aside, [role~=navigation], [role~=banner], [role~=contentinfo], [role~=complementary]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ok,
    Thin,
    EmptyShell,
    BehindLogin,
    Paywalled,
}

/// What a page is and the text kept from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extract {
    pub class: Class,
    pub title: Option<String>,
    pub lang: Option<String>,
    /// [`EXTRACTOR`] or [`FULL_PAGE`].
    pub extractor: &'static str,
    /// `# Title`, then the text, ending in one newline; empty when nothing
    /// is kept.
    pub markdown: String,
    /// Characters of text in `markdown`, its syntax left out.
    pub chars: usize,
    /// Characters of visible text on the whole page.
    pub visible: usize,
    /// Why a page is not `ok`.
    pub reason: Option<&'static str>,
    /// The HTML of the article readability found, when the text is its.
    pub article: Option<String>,
}

/// Classifies a decoded page and keeps its text. `url` is where the page
/// was fetched from, for resolving its links.
pub fn page(html: &str, url: Option<&str>) -> Extract {
    let head = head::scan(html);
    let document = Document::from(html);
    let visible_text = visible_text(&document);
    let visible = visible_text.chars().count();
    let scripts = document.select(SCRIPTS).length();
    let title = head
        .title
        .as_deref()
        .map(head::collapse)
        .filter(|t| !t.is_empty());
    let mut verdict = Extract {
        class: Class::Thin,
        title,
        lang: head.lang.clone(),
        extractor: EXTRACTOR,
        markdown: String::new(),
        chars: 0,
        visible,
        reason: None,
        article: None,
    };
    let stop = |mut verdict: Extract, class, reason| {
        verdict.class = class;
        verdict.reason = Some(reason);
        verdict
    };
    if head.is_sign_in_page() {
        return stop(verdict, Class::BehindLogin, "sign-in page");
    }
    if visible < ENOUGH && document.select("input[type=password]").exists() {
        return stop(verdict, Class::BehindLogin, "password form");
    }
    if visible < SHELL && scripts > 0 {
        return stop(verdict, Class::EmptyShell, "drawn by scripts");
    }
    if visible < ENOUGH && asks_for_javascript(&visible_text) {
        return stop(verdict, Class::EmptyShell, "needs JavaScript");
    }
    if visible < SHELL && document.select(APP_MOUNTS).exists() {
        return stop(verdict, Class::EmptyShell, "app shell");
    }
    let paywall = paywalled(&document);
    let config = Config {
        text_mode: TextMode::Markdown,
        ..Config::default()
    };
    let article = Readability::with_document(document, url, Some(config))
        .and_then(|mut readability| readability.parse())
        .ok();
    let mut body = article
        .as_ref()
        .map(|a| a.text_content.to_string())
        .unwrap_or_default();
    if let Some(article) = &article {
        let found = head::collapse(&article.title);
        if !found.is_empty() {
            verdict.title = Some(found);
        }
        verdict.lang = article.lang.clone().or(verdict.lang);
    }
    let mut chars = plain_chars(&body);
    if chars < ENOUGH && (visible >= ENOUGH || chars == 0) {
        let whole = whole_page(html);
        let whole_chars = plain_chars(&whole);
        if whole_chars > chars {
            body = whole;
            chars = whole_chars;
            verdict.extractor = FULL_PAGE;
        }
    }
    if verdict.extractor == EXTRACTOR {
        verdict.article = article.map(|a| a.content.to_string());
    }
    verdict.markdown = with_title(verdict.title.as_deref(), &body);
    verdict.chars = chars;
    if chars >= ENOUGH {
        verdict.class = Class::Ok;
    } else if paywall {
        verdict.class = Class::Paywalled;
        verdict.reason = Some("paywall");
    } else {
        verdict.reason = Some("short text");
    }
    verdict
}

/// What a reader sees on the whole page, whitespace collapsed: no head,
/// nothing that runs or draws, no `noscript` fallback, and nothing the page
/// hides with `hidden`, `aria-hidden` or an inline `display: none` or
/// `visibility: hidden`.
fn visible_text(document: &Document) -> String {
    const NOT_SEEN: [&str; 6] = ["head", "script", "style", "noscript", "template", "svg"];
    let hidden = |node: &NodeRef| {
        node.node_name()
            .is_some_and(|name| NOT_SEEN.contains(&name.to_ascii_lowercase().as_str()))
            || node.has_attr("hidden")
            || node
                .attr("aria-hidden")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
            || node.attr("style").is_some_and(|style| {
                let style: String = style
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
                    .to_ascii_lowercase();
                style.contains("display:none") || style.contains("visibility:hidden")
            })
    };
    let mut text = String::new();
    let mut stack = vec![document.root()];
    while let Some(node) = stack.pop() {
        if node.is_text() {
            text.push_str(&node.text());
            text.push(' ');
        } else if !(node.is_element() && hidden(&node)) {
            stack.extend(node.children().into_iter().rev());
        }
    }
    head::collapse(&text)
}

/// The body as markdown, its [`CHROME`] outside `main` left out.
fn whole_page(html: &str) -> String {
    let document = without_chrome(html);
    let body = document.select("body");
    body.nodes()
        .first()
        .map(|node| node.md(Some(&NOT_TEXT)).to_string())
        .unwrap_or_default()
}

/// The page with its [`CHROME`] outside `main` left out: what the
/// whole-page text is read from, and where a page's own images are looked
/// for when readability found no article.
pub fn without_chrome(html: &str) -> Document {
    let document = Document::from(html);
    document.select(CHROME).filter(":not(main *)").remove();
    document
}

/// `# Title`, a blank line and the body, unless the body already starts
/// with that heading; one newline at the end. Empty when there is no body.
fn with_title(title: Option<&str>, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    match title {
        Some(title) if !body.starts_with(&format!("# {title}")) => {
            format!("# {title}\n\n{body}\n")
        }
        _ => format!("{body}\n"),
    }
}

/// A notice that the page needs JavaScript: the word, with "enable",
/// "require" or "turn on" shortly before it or "disabled" or "required"
/// shortly after.
fn asks_for_javascript(text: &str) -> bool {
    const NEAR: usize = 60;
    let text = text.to_lowercase();
    text.match_indices("javascript").any(|(at, word)| {
        let mut start = at.saturating_sub(NEAR);
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (at + word.len() + NEAR).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let before = &text[start..at];
        let after = &text[at + word.len()..end];
        ["enable", "require", "turn on"]
            .iter()
            .any(|w| before.contains(w))
            || ["disabled", "required"].iter().any(|w| after.contains(w))
    })
}

/// The publisher says the page is not free: JSON-LD
/// `isAccessibleForFree: false`, or a locked content tier.
fn paywalled(document: &Document) -> bool {
    fn not_free(value: &serde_json::Value) -> bool {
        let mut found = false;
        jsonld::walk(value, &mut |map, _| {
            found |= map.get("isAccessibleForFree").is_some_and(|free| {
                free == &serde_json::Value::Bool(false)
                    || free
                        .as_str()
                        .is_some_and(|v| v.eq_ignore_ascii_case("false"))
            });
        });
        found
    }
    let locked = document
        .select(r#"meta[property="article:content_tier"], meta[name="article:content_tier"]"#)
        .iter()
        .any(|meta| {
            meta.attr("content")
                .is_some_and(|tier| tier.trim().eq_ignore_ascii_case("locked"))
        });
    locked
        || document
            .select(r#"script[type="application/ld+json"]"#)
            .iter()
            .any(|script| {
                serde_json::from_str::<serde_json::Value>(script.text().trim())
                    .is_ok_and(|value| not_free(&value))
            })
}

/// Characters a reader sees in `markdown`: marks, link targets and runs of
/// whitespace left out.
pub fn plain_chars(markdown: &str) -> usize {
    let mut text = String::with_capacity(markdown.len());
    for line in markdown.lines() {
        let line = line.trim_start();
        let line = line.trim_start_matches(['#', '>', ' ']);
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("+ "))
            .unwrap_or(line);
        let mut chars = line.chars().peekable();
        let mut previous = ' ';
        while let Some(c) = chars.next() {
            match c {
                // `](target)`: the target is not text.
                ']' if chars.peek() == Some(&'(') => {
                    let mut depth = 0;
                    for c in chars.by_ref() {
                        match c {
                            '(' => depth += 1,
                            ')' if depth == 1 => break,
                            ')' => depth -= 1,
                            _ => {}
                        }
                    }
                }
                '\\' if chars.peek().is_some_and(char::is_ascii_punctuation) => {
                    if let Some(escaped) = chars.next() {
                        text.push(escaped);
                    }
                }
                '*' | '_' | '`' | '~' | '[' | ']' | '|' => {}
                '!' if chars.peek() == Some(&'[') => {}
                '-' if previous == '-' || chars.peek() == Some(&'-') => {}
                c => text.push(c),
            }
            previous = c;
        }
        text.push(' ');
    }
    text.split_whitespace()
        .map(|word| word.chars().count() + 1)
        .sum::<usize>()
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> String {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "content",
            name,
        ]
        .iter()
        .collect();
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    fn classify(name: &str) -> Extract {
        page(&fixture(name), Some("https://example.test/page"))
    }

    #[test]
    fn the_extractor_version_is_the_locked_one() {
        let lock = include_str!("../Cargo.lock");
        let entry = lock
            .split("[[package]]")
            .find(|entry| entry.contains("name = \"dom_smoothie\"\n"))
            .expect("dom_smoothie is locked");
        assert!(
            entry.contains(&format!("version = \"{EXTRACTOR_VERSION}\"")),
            "{entry}"
        );
    }

    #[test]
    fn a_long_article_is_ok_and_its_menus_are_left_behind() {
        let page = classify("article.html");
        assert_eq!((page.class, page.extractor), (Class::Ok, EXTRACTOR));
        assert!(page.chars >= ENOUGH, "{}", page.chars);
        assert!(
            page.markdown.starts_with("# How Tide Pools Work\n\n"),
            "{}",
            &page.markdown[..80]
        );
        assert!(page.markdown.contains("barnacle"));
        assert!(!page.markdown.contains("Subscribe to our newsletter"));
        assert!(page.markdown.ends_with('\n') && !page.markdown.ends_with("\n\n"));
        assert_eq!(page.lang.as_deref(), Some("en"));
        assert_eq!(page.reason, None);
    }

    #[test]
    fn a_repository_page_keeps_its_readme() {
        let page = classify("repo.html");
        assert_eq!(page.class, Class::Ok);
        assert!(page.markdown.contains("cargo install tidepool"));
        assert!(page.markdown.contains("## Usage"));
    }

    #[test]
    fn an_index_page_readability_misreads_keeps_the_whole_page() {
        let page = classify("index-page.html");
        assert_eq!((page.class, page.extractor), (Class::Ok, FULL_PAGE));
        assert!(page.visible >= ENOUGH);
        for n in [1, 17, 30] {
            assert!(page.markdown.contains(&format!("Entry {n}:")), "entry {n}");
        }
    }

    #[test]
    fn in_main_usage_instructions_survive_fallback() {
        let instructions =
            "load_tidepool_model(); inspect_species(); record_observations();\n".repeat(40);
        let summary = "A tide pool holds small creatures through the low tide. \
                       We record the water temperature, identify each species, \
                       and compare our observations with earlier visits. "
            .repeat(5);
        let page = page(
            &format!(
                "<html><body><header><nav><a href=\"/\">Global menu</a></nav></header>\
                 <main><header><h1>Tide pool guide</h1><nav class=\"sr-only\">\
                 <h2>Usage</h2><ul><li><pre>{instructions}</pre></li></ul></nav></header>\
                 <article><p>{summary}</p></article></main><footer>Site footer</footer>\
                 </body></html>"
            ),
            None,
        );
        assert_eq!(page.class, Class::Ok);
        assert_eq!(page.extractor, FULL_PAGE);
        assert!(page.markdown.contains("Tide pool guide"));
        assert!(page.markdown.contains("record_observations"));
        assert!(!page.markdown.contains("Global menu"));
        assert!(!page.markdown.contains("Site footer"));
    }

    #[test]
    fn a_page_of_only_menus_and_footers_is_thin() {
        let links =
            |label: &str| format!("<li><a href=\"/{label}\">{label} link</a></li>").repeat(30);
        let page = page(
            &format!(
                "<html><body><header><ul>{}</ul></header><nav><ul>{}</ul></nav>\
                 <div role=\"navigation\"><ul>{}</ul></div>\
                 <main><p>A gallery of tide pool photographs.</p></main>\
                 <aside><ul>{}</ul></aside><div role=\"complementary\"><ul>{}</ul></div>\
                 <footer><ul>{}</ul></footer><div role=\"contentinfo\"><p>Example Field Notes.</p></div>\
                 </body></html>",
                links("Banner"),
                links("Section"),
                links("Language"),
                links("Gallery"),
                links("Related"),
                links("Footer")
            ),
            None,
        );
        assert!(page.visible >= ENOUGH, "{}", page.visible);
        assert_eq!((page.class, page.reason), (Class::Thin, Some("short text")));
        assert!(page.markdown.contains("A gallery of tide pool photographs"));
        for chrome in [
            "Banner", "Section", "Language", "Gallery", "Related", "Footer",
        ] {
            assert!(!page.markdown.contains(chrome), "{chrome}");
        }
    }

    #[test]
    fn a_short_landing_page_is_thin_and_keeps_its_text() {
        let page = classify("landing.html");
        assert_eq!(page.class, Class::Thin);
        assert_eq!(page.reason, Some("short text"));
        assert!(page.chars > 0 && page.chars < ENOUGH);
        assert!(page.markdown.contains("fits on the fridge"));
    }

    #[test]
    fn script_drawn_pages_are_empty_shells() {
        let x = classify("x-shell.html");
        assert_eq!(x.class, Class::EmptyShell);
        assert!(x.visible < SHELL, "{}", x.visible);
        assert_eq!(x.markdown, "");
        let react = classify("react-shell.html");
        assert_eq!(react.class, Class::EmptyShell);
        assert_eq!(react.reason, Some("drawn by scripts"));
        let notice = page(
            &format!(
                "<html><body><script src=a.js></script><p>{}</p><p>Please enable JavaScript to use this site.</p></body></html>",
                "Some words. ".repeat(40)
            ),
            None,
        );
        assert_eq!(
            (notice.class, notice.reason),
            (Class::EmptyShell, Some("needs JavaScript"))
        );
        let mount = page(
            "<html><body><div id=\"__next\"></div><p>Loading your dashboard</p></body></html>",
            None,
        );
        assert_eq!(
            (mount.class, mount.reason),
            (Class::EmptyShell, Some("app shell"))
        );
    }

    #[test]
    fn sign_in_forms_are_behind_a_login() {
        let page = classify("sign-in.html");
        assert_eq!(
            (page.class, page.reason),
            (Class::BehindLogin, Some("password form"))
        );
        assert_eq!(page.markdown, "");
        let titled = super::page(
            "<title>Sign in</title><body><p>Welcome back</p></body>",
            None,
        );
        assert_eq!(
            (titled.class, titled.reason),
            (Class::BehindLogin, Some("sign-in page"))
        );
    }

    #[test]
    fn a_paywalled_teaser_is_paywalled_and_keeps_what_it_showed() {
        let page = classify("paywall.html");
        assert_eq!(
            (page.class, page.reason),
            (Class::Paywalled, Some("paywall"))
        );
        assert!(page.markdown.contains("The first paragraph"));
        let locked = super::page(
            "<head><meta property=\"article:content_tier\" content=\"locked\"></head><body><p>A teaser.</p></body>",
            None,
        );
        assert_eq!(locked.class, Class::Paywalled);
        let free = super::page(
            "<head><script type=\"application/ld+json\">{\"isAccessibleForFree\":true}</script></head><body><p>Short and free.</p></body>",
            None,
        );
        assert_eq!(free.class, Class::Thin);
    }

    #[test]
    fn plain_characters_leave_markdown_out() {
        assert_eq!(
            plain_chars("# Title\n\nSome **bold** and _it_ text."),
            "Title Some bold and it text.".len()
        );
        assert_eq!(
            plain_chars("[a link](https://example.test/x_(y)) and ![alt](i.png)"),
            "a link and alt".len()
        );
        assert_eq!(
            plain_chars("- one\n- two\n\n> quoted\n\n---\n"),
            "one two quoted".len()
        );
        assert_eq!(plain_chars("a \\* star | cell |"), "a * star cell".len());
        assert_eq!(plain_chars("  \n "), 0);
        assert_eq!(plain_chars("naïve café"), 10);
    }

    #[test]
    fn javascript_notices_are_near_the_word() {
        assert!(asks_for_javascript(
            "You need to enable JavaScript to run this app."
        ));
        assert!(asks_for_javascript(
            "JavaScript is disabled in your browser."
        ));
        assert!(asks_for_javascript("This site requires JavaScript."));
        assert!(!asks_for_javascript("A JavaScript tutorial for beginners."));
        let far = format!("enable{}JavaScript", " filler".repeat(20));
        assert!(!asks_for_javascript(&far));
        assert!(asks_for_javascript(&format!(
            "{}enable JavaScript{}",
            "é".repeat(40),
            "ü".repeat(40)
        )));
    }
}
