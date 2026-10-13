//! Builds synthetic PDFs for tests: pages of invented lines in Courier, an
//! optional Info title. Nothing here comes from a real document.
//!
//! Shared by the PDF reader's unit tests (via `#[path]`) and the
//! integration tests, so both read files built the same way.

#![allow(dead_code)]

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};

/// A PDF whose pages each show `lines`, titled `title`.
pub fn pdf(pages: &[&[&str]], title: Option<&str>) -> Vec<u8> {
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
