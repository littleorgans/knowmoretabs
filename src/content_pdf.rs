//! What `content` keeps of a page that is a PDF: its text, read on this
//! machine, as markdown, or why there is none.
//!
//! slice: content
//! why: A PDF is a document like any page, but no extractor for HTML reads
//!      it and no browser renders it as text. Its text is read here, in
//!      process and in pure Rust, so nothing about the file leaves the
//!      machine and no tool needs installing. The file comes from a site,
//!      so it is read as hostile: no more than a page's body is read, no
//!      stream is inflated past a cap, and a panic in the reader ends the
//!      page, never the run. The same thin rule as a web page applies, and
//!      a PDF with no text, a scan, says so rather than reading as thin.

use std::panic::{self, AssertUnwindSafe};

use lopdf::{Document, LoadOptions};

use crate::content_fetch::BODY_CAP;
use crate::content_store::{Completeness, Page};
use crate::extract;

/// A page served as a PDF.
pub const MIME: &str = "application/pdf";
/// The reader a PDF's line names.
pub const EXTRACTOR: &str = "lopdf";
/// The `lopdf` release in `Cargo.lock`; a unit test holds them equal.
pub const EXTRACTOR_VERSION: &str = "0.45.0";
/// The most one stream of a PDF is inflated to: a small file must not
/// grow into one that fills the memory.
const INFLATE_CAP: usize = 64 * 1024 * 1024;

/// The text of a PDF's `bytes`, as read up to [`BODY_CAP`], or why it has
/// none to keep. A read that reached the cap is a file over it.
pub fn read(bytes: &[u8]) -> Result<Page, &'static str> {
    if bytes.len() >= BODY_CAP {
        return Err("PDF over 10 MB");
    }
    let (title, markdown) = guarded(|| text(bytes))?;
    if markdown.is_empty() {
        return Err("PDF without text");
    }
    let chars = extract::plain_chars(&markdown);
    Ok(Page {
        title,
        extractor: format!("{EXTRACTOR} {EXTRACTOR_VERSION}"),
        completeness: if chars >= extract::ENOUGH {
            Completeness::Full
        } else {
            Completeness::Thin
        },
        chars,
        captions: None,
        markdown,
    })
}

/// `read`'s outcome, with a parse error or a panic as an unreadable PDF.
fn guarded<T>(read: impl FnOnce() -> Result<T, lopdf::Error>) -> Result<T, &'static str> {
    match panic::catch_unwind(AssertUnwindSafe(read)) {
        Ok(Ok(read)) => Ok(read),
        Ok(Err(_)) | Err(_) => Err("unreadable PDF"),
    }
}

/// A PDF's title and its pages' text, each line trimmed, runs of blank
/// lines made one, and a blank line between pages.
fn text(bytes: &[u8]) -> Result<(Option<String>, String), lopdf::Error> {
    let doc = Document::load_mem_with_options(
        bytes,
        LoadOptions::with_max_decompressed_size(INFLATE_CAP),
    )?;
    let mut markdown = String::new();
    for page in doc.get_pages().into_keys() {
        let mut gap = true;
        for line in doc
            .extract_text_with_limit(&[page], INFLATE_CAP)?
            .lines()
            .map(str::trim)
        {
            if line.is_empty() {
                gap = true;
                continue;
            }
            if !markdown.is_empty() {
                markdown.push_str(if gap { "\n\n" } else { "\n" });
            }
            markdown.push_str(line);
            gap = false;
        }
    }
    if !markdown.is_empty() {
        markdown.push('\n');
    }
    Ok((title(&doc), markdown))
}

/// The title a PDF's Info dictionary gives, when it gives one.
fn title(doc: &Document) -> Option<String> {
    let info = doc.trailer.get_deref(b"Info", doc).ok()?.as_dict().ok()?;
    let title = lopdf::decode_text_string(info.get_deref(b"Title", doc).ok()?).ok()?;
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use lopdf::content::{Content, Operation};
    use lopdf::{Object, Stream, dictionary};

    use super::*;

    /// A PDF whose pages each show `lines`, titled `title`.
    fn pdf(pages: &[&[&str]], title: Option<&str>) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font },
        });
        let kids: Vec<Object> = pages
            .iter()
            .map(|lines| {
                let mut operations = vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                ];
                for line in *lines {
                    operations.push(Operation::new("Tj", vec![Object::string_literal(*line)]));
                    operations.push(Operation::new("Td", vec![0.into(), (-14).into()]));
                }
                operations.push(Operation::new("ET", vec![]));
                let content = Content { operations }.encode().unwrap();
                let contents = doc.add_object(Stream::new(dictionary! {}, content));
                doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => contents,
                })
                .into()
            })
            .collect();
        let count = i64::try_from(kids.len()).unwrap();
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
                "Resources" => resources,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog);
        if let Some(title) = title {
            let info = doc.add_object(dictionary! { "Title" => lopdf::text_string(title) });
            doc.trailer.set("Info", info);
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_long_pdf_is_full_text_with_its_title() {
        let line = "Words a printed paper says, page after page.";
        let lines = vec![line; 40];
        let page = read(&pdf(&[&lines, &lines], Some(" A paper "))).unwrap();
        assert_eq!(page.completeness, Completeness::Full);
        assert!(page.chars >= extract::ENOUGH, "{}", page.chars);
        assert_eq!(page.title.as_deref(), Some("A paper"));
        assert_eq!(page.extractor, "lopdf 0.45.0");
        assert!(page.markdown.starts_with(line), "{}", page.markdown);
        assert!(page.markdown.ends_with(".\n"), "{}", page.markdown);
        assert!(!page.markdown.contains("\n\n\n"));
    }

    #[test]
    fn pages_are_a_blank_line_apart() {
        let page = read(&pdf(&[&["First page."], &["Second page."]], None)).unwrap();
        assert_eq!(page.markdown, "First page.\n\nSecond page.\n");
        assert_eq!(page.completeness, Completeness::Thin);
        assert_eq!(page.title, None);
    }

    #[test]
    fn a_pdf_without_text_reads_as_one() {
        assert_eq!(read(&pdf(&[&[]], Some("A scan"))), Err("PDF without text"));
    }

    #[test]
    fn a_broken_or_cut_pdf_is_unreadable() {
        let whole = pdf(&[&["Some text."]], None);
        assert_eq!(read(b"%PDF-1.5\nnot a pdf"), Err("unreadable PDF"));
        assert_eq!(read(&whole[..whole.len() / 2]), Err("unreadable PDF"));
    }

    #[test]
    fn a_pdf_at_the_cap_is_over_it() {
        assert_eq!(read(&vec![b' '; BODY_CAP]), Err("PDF over 10 MB"));
    }

    #[test]
    fn the_extractor_version_is_the_locked_one() {
        assert_eq!(extract::tests::locked(EXTRACTOR), EXTRACTOR_VERSION);
    }

    #[test]
    fn a_panic_in_the_reader_is_an_unreadable_pdf() {
        let panicked = guarded::<()>(|| panic!("a reader bug"));
        assert_eq!(panicked, Err("unreadable PDF"));
    }
}
