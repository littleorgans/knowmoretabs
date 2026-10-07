//! Classifier parity: runs the `content` classifier over a folder of
//! labelled pages and prints how often it agrees with the labels.
//!
//! A development tool, never run in CI:
//!
//! ```text
//! cargo run --release --example content_parity -- <folder>
//! ```
//!
//! The folder holds `manifest.csv`, with `url_sha256` and `status` columns,
//! and `pages/<url_sha256>.html`. Rows labelled `ok`, `thin` or
//! `empty_shell` are compared; the rest are ignored. Labelled pages are
//! someone's browsing, so nothing from a page is printed: only counts, and
//! for each disagreement the first characters of its hash and the numbers
//! the rules used.

// The binary's own modules, included by path because the crate has no
// library target; this tool calls only part of them.
#![allow(dead_code)]

#[path = "../src/extract.rs"]
mod extract;
#[path = "../src/head.rs"]
mod head;
#[path = "../src/jsonld.rs"]
mod jsonld;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

const LABELS: [&str; 3] = ["ok", "thin", "empty_shell"];

fn main() -> ExitCode {
    let Some(folder) = std::env::args_os().nth(1) else {
        return fail("usage: content_parity <folder with manifest.csv and pages/>");
    };
    let folder = Path::new(&folder);
    let manifest = match std::fs::read_to_string(folder.join("manifest.csv")) {
        Ok(text) => text,
        Err(err) => return fail(&format!("cannot read manifest.csv: {err}")),
    };
    let rows = csv(&manifest);
    let Some(header) = rows.first() else {
        return fail("manifest.csv is empty");
    };
    let column = |name: &str| header.iter().position(|h| h == name);
    let (Some(hash_at), Some(label_at)) = (column("url_sha256"), column("status")) else {
        return fail("manifest.csv needs url_sha256 and status columns");
    };
    // Optional: the labeller's own visible text count, to check ours against.
    let visible_at = column("visible_text_chars");
    let mut report = String::new();
    let mut extractors: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut same_visible, mut near_visible) = (0, 0);
    let mut confusion: BTreeMap<(String, &str), usize> = BTreeMap::new();
    let mut disagreements = Vec::new();
    let (mut compared, mut agreed, mut unreadable) = (0, 0, 0);
    for row in &rows[1..] {
        let (Some(hash), Some(label)) = (row.get(hash_at), row.get(label_at)) else {
            continue;
        };
        if !LABELS.contains(&label.as_str()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(folder.join("pages").join(format!("{hash}.html"))) else {
            unreadable += 1;
            continue;
        };
        let Ok(html) = head::decode(&bytes, None) else {
            unreadable += 1;
            continue;
        };
        let page = extract::page(&html, None);
        let got = word(page.class);
        compared += 1;
        if !page.markdown.is_empty() {
            *extractors.entry(page.extractor).or_default() += 1;
        }
        if let Some(theirs) = visible_at
            .and_then(|at| row.get(at))
            .and_then(|v| v.parse::<usize>().ok())
        {
            same_visible += usize::from(theirs == page.visible);
            near_visible += usize::from(theirs.abs_diff(page.visible) * 100 <= theirs.max(1) * 2);
        }
        *confusion.entry((label.clone(), got)).or_default() += 1;
        if got == label {
            agreed += 1;
        } else {
            disagreements.push(format!(
                "  {}  label {label}, got {got} ({}), visible {}, text {}, extractor {}",
                hash.get(..12).unwrap_or(hash),
                page.reason.unwrap_or("-"),
                page.visible,
                page.chars,
                page.extractor,
            ));
        }
    }
    let percent = if compared == 0 {
        0.0
    } else {
        f64::from(agreed) * 100.0 / f64::from(compared)
    };
    let _ = writeln!(
        report,
        "compared {compared}, agreed {agreed} ({percent:.1}%), unreadable {unreadable}"
    );
    if visible_at.is_some() {
        let _ = writeln!(
            report,
            "visible text: {same_visible} equal to the label's count, {near_visible} within 2%"
        );
    }
    let _ = writeln!(report, "text kept, by extractor: {extractors:?}");
    let _ = writeln!(report, "label -> classifier:");
    for ((label, got), n) in &confusion {
        let _ = writeln!(report, "  {label} -> {got}: {n}");
    }
    if !disagreements.is_empty() {
        let _ = writeln!(report, "disagreements:");
        for line in &disagreements {
            let _ = writeln!(report, "{line}");
        }
    }
    let _ = std::io::stdout().write_all(report.as_bytes());
    ExitCode::SUCCESS
}

fn word(class: extract::Class) -> &'static str {
    match class {
        extract::Class::Ok => "ok",
        extract::Class::Thin => "thin",
        extract::Class::EmptyShell => "empty_shell",
        extract::Class::BehindLogin => "behind_login",
        extract::Class::Paywalled => "paywalled",
    }
}

fn fail(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "{message}");
    ExitCode::FAILURE
}

/// RFC 4180 rows: quoted fields may hold commas, doubled quotes and line
/// breaks.
fn csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (_, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}
