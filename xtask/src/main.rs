//! `cargo xtask slices`: the slice-metadata lint and the generator for
//! `docs/SLICES.md`.
//!
//! slice: capture
//! why: The slice matrix is only useful while source files and the matrix
//!      agree, and nothing keeps them agreeing except a check that fails
//!      CI. This is that check, kept deliberately small: it reads
//!      `slices.toml`, walks the source tree for `slice:`/`why:` headers,
//!      and regenerates the human-readable matrix so the document can never
//!      drift from the data it is generated from.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MATRIX: &str = "slices.toml";
const GENERATED: &str = "docs/SLICES.md";
const SOURCE_ROOTS: [&str; 2] = ["src", "xtask/src"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    if args.first().map(String::as_str) != Some("slices") || args.len() > 2 {
        eprintln!("usage: cargo xtask slices [--check]");
        return ExitCode::from(2);
    }
    let repo = repo_root();
    match run(&repo, check) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problems) => {
            for p in &problems {
                eprintln!("slices: {p}");
            }
            eprintln!("slices: {} problem(s)", problems.len());
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn run(repo: &Path, check: bool) -> Result<(), Vec<String>> {
    let toml_text =
        std::fs::read_to_string(repo.join(MATRIX)).map_err(|e| vec![format!("{MATRIX}: {e}")])?;
    let matrix: toml::Table =
        toml::from_str(&toml_text).map_err(|e| vec![format!("{MATRIX}: {e}")])?;
    let mut files = Vec::new();
    for root in SOURCE_ROOTS {
        collect_rust_files(&repo.join(root), &mut files);
    }
    let headers: Vec<(String, Result<Header, String>)> = files
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(repo)
                .unwrap_or(path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(path).unwrap_or_default();
            (rel, parse_header(&text))
        })
        .collect();
    let mut problems = lint(&matrix, &headers);
    let generated = render(&matrix);
    let target = repo.join(GENERATED);
    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != generated {
            problems.push(format!(
                "{GENERATED} is out of date; run `cargo xtask slices`"
            ));
        }
    } else if let Err(e) = std::fs::write(&target, &generated) {
        problems.push(format!("{GENERATED}: {e}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// What a module header declares. `title` is its first line. `slices` is
/// usually one entry; a file that genuinely serves several slices lists them
/// comma-separated (`slice: browsers, platforms`), because splitting a file
/// to satisfy the lint would be the tail wagging the dog.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    title: String,
    slices: Vec<String>,
    why: String,
}

/// Reads the leading `//!` block. Fails with a reason when the block has no
/// usable `slice:` and `why:` pair.
fn parse_header(text: &str) -> Result<Header, String> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|l| l.starts_with("//!"))
        .map(|l| {
            l.trim_start_matches("//!")
                .strip_prefix(' ')
                .unwrap_or(l.trim_start_matches("//!"))
        })
        .collect();
    let title = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_owned());
    let mut slices = None;
    let mut why = String::new();
    let mut in_why = false;
    for line in &lines {
        if let Some(rest) = line.trim_start().strip_prefix("slice:") {
            slices = Some(
                rest.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            );
            in_why = false;
        } else if let Some(rest) = line.trim_start().strip_prefix("why:") {
            rest.trim().clone_into(&mut why);
            in_why = true;
        } else if in_why && line.starts_with(' ') && !line.trim().is_empty() {
            why.push(' ');
            why.push_str(line.trim());
        } else {
            in_why = false;
        }
    }
    match (title, slices) {
        (Some(title), Some(slices)) if !slices.is_empty() => Ok(Header { title, slices, why }),
        (None, _) => Err("no `//!` module header".to_owned()),
        (Some(_), _) => Err("module header has no `slice:` marker".to_owned()),
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The five rules from `docs/BRIEF.md` §6, except the generated-doc check.
fn lint(matrix: &toml::Table, headers: &[(String, Result<Header, String>)]) -> Vec<String> {
    let mut problems = Vec::new();
    let slices = array(matrix, "slice");
    let declared: BTreeSet<&str> = slices
        .iter()
        .filter_map(|s| s.get("id").and_then(toml::Value::as_str))
        .collect();
    let mut claimed: BTreeSet<&str> = BTreeSet::new();
    for (file, header) in headers {
        match header {
            Err(reason) => problems.push(format!("{file}: {reason}")),
            Ok(h) => {
                for slice in &h.slices {
                    if !declared.contains(slice.as_str()) {
                        problems.push(format!(
                            "{file}: slice `{slice}` is not declared in {MATRIX}"
                        ));
                    }
                    claimed.insert(slice.as_str());
                }
                let why = words(&h.why);
                let title = words(&h.title);
                if why.is_empty() {
                    problems.push(format!("{file}: `why:` is empty"));
                } else if why.len() < 8 || title.windows(why.len()).any(|w| w == why.as_slice()) {
                    problems.push(format!("{file}: `why:` restates the title instead of explaining why the file exists"));
                }
            }
        }
    }
    for slice in &slices {
        let id = slice.get("id").and_then(toml::Value::as_str).unwrap_or("?");
        if slice.get("status").and_then(toml::Value::as_str) == Some("done")
            && !claimed.contains(id)
        {
            problems.push(format!("slice `{id}` is done but no source file claims it"));
        }
    }
    problems
}

fn array<'a>(table: &'a toml::Table, key: &str) -> Vec<&'a toml::Table> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_table).collect())
        .unwrap_or_default()
}

fn field<'a>(table: &'a toml::Table, key: &str) -> &'a str {
    table.get(key).and_then(toml::Value::as_str).unwrap_or("")
}

fn list<'a>(table: &'a toml::Table, key: &str) -> Vec<&'a str> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default()
}

fn number(table: &toml::Table) -> i64 {
    table
        .get("number")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0)
}

fn joined(items: &[&str]) -> String {
    if items.is_empty() {
        "—".to_owned()
    } else {
        items.join(", ")
    }
}

/// `docs/SLICES.md`, byte for byte.
fn render(matrix: &toml::Table) -> String {
    let meta = matrix.get("matrix").and_then(toml::Value::as_table);
    let title = meta.map_or("", |m| field(m, "title"));
    let tagline = meta.map_or("", |m| field(m, "tagline"));
    let slices = array(matrix, "slice");
    let mut out = String::new();
    let _ = writeln!(out, "# {title} — slices\n");
    let _ = writeln!(
        out,
        "<!-- Generated by `cargo xtask slices` from `{MATRIX}`. Do not edit by hand. -->\n"
    );
    let _ = writeln!(out, "> {tagline}\n");
    let _ = writeln!(out, "## What people want, and which slice delivers it\n");
    let _ = writeln!(out, "| Capability | Want | Delivered by |\n|---|---|---|");
    for cap in array(matrix, "capability") {
        let id = field(cap, "id");
        let by: Vec<String> = slices
            .iter()
            .filter(|s| list(s, "delivers").contains(&id))
            .map(|s| format!("{} `{}`", number(s), field(s, "id")))
            .collect();
        let _ = writeln!(
            out,
            "| `{id}` | {} | {} |",
            field(cap, "want"),
            if by.is_empty() {
                "—".to_owned()
            } else {
                by.join(", ")
            }
        );
    }
    let _ = writeln!(out, "\n## Build order\n");
    for s in &slices {
        let _ = writeln!(
            out,
            "### {}. {} (`{}`) — {}\n",
            number(s),
            field(s, "title"),
            field(s, "id"),
            field(s, "status")
        );
        let _ = writeln!(out, "- Delivers: {}", joined(&list(s, "delivers")));
        let _ = writeln!(out, "- Depends on: {}\n", joined(&list(s, "depends_on")));
        let _ = writeln!(out, "**Ships.** {}\n", field(s, "ships").trim());
        let _ = writeln!(out, "**Intent.** {}\n", field(s, "intent").trim());
    }
    let _ = writeln!(out, "## Prepared for, deliberately not built\n");
    for f in array(matrix, "future") {
        let _ = writeln!(out, "### `{}` — {}\n", field(f, "id"), field(f, "want"));
        let _ = writeln!(out, "- Prepared by: {}\n", joined(&list(f, "prepared_by")));
        let _ = writeln!(out, "{}\n", field(f, "why_not_now").trim());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "//! Reads things.\n//!\n//! slice: capture\n//! why: Because the reader has to survive records Chrome added last\n//!      Tuesday without anyone noticing.\n\nfn x() {}\n";
    const MATRIX_TOML: &str = "[matrix]\ntitle = \"t\"\ntagline = \"g\"\n[[capability]]\nid = \"c\"\nwant = \"w\"\n[[slice]]\nid = \"capture\"\nnumber = 1\ntitle = \"Cap\"\nstatus = \"done\"\ndelivers = [\"c\"]\ndepends_on = []\nships = \"s\"\nintent = \"i\"\n[[slice]]\nid = \"later\"\nnumber = 2\ntitle = \"L\"\nstatus = \"done\"\ndelivers = []\ndepends_on = [\"capture\"]\nships = \"s\"\nintent = \"i\"\n";

    fn matrix() -> toml::Table {
        toml::from_str(MATRIX_TOML).unwrap()
    }

    #[test]
    fn parses_a_good_header_with_continuation_lines() {
        let h = parse_header(GOOD).unwrap();
        assert_eq!(h.title, "Reads things.");
        assert_eq!(h.slices, ["capture"]);
        assert!(h.why.starts_with("Because the reader") && h.why.ends_with("noticing."));
    }

    #[test]
    fn a_file_may_claim_several_slices() {
        let h = parse_header("//! One table, one column per OS.\n//! slice: browsers, platforms\n//! why: Splitting the table would serve the linter, not the reader.\n").unwrap();
        assert_eq!(h.slices, ["browsers", "platforms"]);
        assert!(
            parse_header("//! T\n//! slice: ,\n//! why: x\n")
                .unwrap_err()
                .contains("no `slice:`")
        );
        let claimed = lint(
            &matrix(),
            &[(
                "both.rs".to_owned(),
                parse_header(
                    "//! T\n//! slice: capture, later\n//! why: One reason long enough to pass the restatement rule here.\n",
                ),
            )],
        );
        assert!(claimed.is_empty(), "{}", claimed.join("\n"));
    }

    #[test]
    fn missing_header_or_marker_is_an_error() {
        assert!(parse_header("fn x() {}").unwrap_err().contains("no `//!`"));
        assert!(
            parse_header("//! Title only\n")
                .unwrap_err()
                .contains("no `slice:`")
        );
    }

    #[test]
    fn lint_flags_every_rule() {
        let headers = vec![
            ("ok.rs".to_owned(), parse_header(GOOD)),
            (
                "bad_slice.rs".to_owned(),
                parse_header(
                    "//! T\n//! slice: nope\n//! why: A perfectly long and informative reason is written here.\n",
                ),
            ),
            (
                "restated.rs".to_owned(),
                parse_header(
                    "//! Reads the session file into tabs.\n//! slice: capture\n//! why: reads the session file into tabs\n",
                ),
            ),
            (
                "empty_why.rs".to_owned(),
                parse_header("//! T\n//! slice: capture\n//! why:\n"),
            ),
            ("none.rs".to_owned(), parse_header("fn main() {}")),
        ];
        let problems = lint(&matrix(), &headers);
        let text = problems.join("\n");
        assert!(
            text.contains("bad_slice.rs: slice `nope` is not declared"),
            "{text}"
        );
        assert!(
            text.contains("restated.rs: `why:` restates the title"),
            "{text}"
        );
        assert!(text.contains("empty_why.rs: `why:` is empty"), "{text}");
        assert!(text.contains("none.rs: no `//!` module header"), "{text}");
        assert!(
            text.contains("slice `later` is done but no source file claims it"),
            "{text}"
        );
        assert_eq!(problems.len(), 5, "{text}");
        assert!(
            lint(&matrix(), &[("ok.rs".to_owned(), parse_header(GOOD))])
                .iter()
                .all(|p| p.contains("`later`"))
        );
    }

    #[test]
    fn render_is_deterministic_and_mentions_everything() {
        let a = render(&matrix());
        assert_eq!(a, render(&matrix()));
        assert!(a.contains("# t — slices"));
        assert!(a.contains("| `c` | w | 1 `capture` |"));
        assert!(a.contains("### 1. Cap (`capture`) — done"));
        assert!(a.contains("- Depends on: capture"));
    }

    #[test]
    fn this_repository_passes() {
        run(&repo_root(), true).unwrap_or_else(|p| panic!("{}", p.join("\n")));
    }
}
