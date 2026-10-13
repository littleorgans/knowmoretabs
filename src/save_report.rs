//! The report a `save` run ends with: what was saved or skipped, from
//! where, what else was found, as lines or as JSON.
//!
//! slice: capture, browsers
//! why: A save is read by its report: which browser and profile it came
//!      from, how much it held, whether it degraded, and which other
//!      sessions were passed over and why. Wording, column alignment and the
//!      JSON shape live apart from the entry point, so `main.rs` stays the
//!      short dispatch it is meant to be.

use std::fmt::Write as _;

use crate::capture::Outcome;
use crate::model::Snapshot;
use crate::platform;

fn summary(snapshot: &Snapshot) -> String {
    let s = &snapshot.stats;
    let groups = match s.groups {
        0 => String::new(),
        1 => ", 1 group".to_owned(),
        n => format!(", {n} groups"),
    };
    let excluded = match s.excluded_tabs {
        0 => String::new(),
        1 => " (1 localhost tab left out)".to_owned(),
        n => format!(" ({n} localhost tabs left out)"),
    };
    format!(
        "{} tabs across {} window{}{groups}{excluded}",
        s.tabs,
        s.windows,
        if s.windows == 1 { "" } else { "s" }
    )
}

pub fn human_outcome(outcome: &Outcome, verbose: bool) -> String {
    let mut out = String::new();
    let snapshot = match outcome {
        Outcome::Saved {
            path,
            snapshot,
            also_found,
        } => {
            let _ = writeln!(
                out,
                "saved {} to {} from {}",
                summary(snapshot),
                path.display(),
                source_label(snapshot)
            );
            write_also_found(&mut out, snapshot, also_found);
            snapshot
        }
        Outcome::Skipped {
            previous_id,
            snapshot,
            also_found,
        } => {
            let _ = writeln!(
                out,
                "no change since {previous_id}: {}. Nothing saved; use --force to save anyway.",
                summary(snapshot)
            );
            write_also_found(&mut out, snapshot, also_found);
            snapshot
        }
    };
    if snapshot.stats.is_degraded() {
        let _ = writeln!(out, "degraded: {}", snapshot.stats.degradation_summary());
    }
    if verbose {
        let src = &snapshot.source;
        let _ = writeln!(
            out,
            "source: {} ({} bytes, sha256 {})",
            src.path.display(),
            src.bytes,
            src.sha256
        );
        if let Some(saved_at) = src.saved_at {
            let _ = writeln!(out, "session last written: {saved_at}");
        }
        if let Some(profile) = &src.profile {
            let _ = writeln!(
                out,
                "profile: {profile} ({})",
                src.profile_display.as_deref().unwrap_or("no display name")
            );
        }
        let by_id: Vec<String> = snapshot
            .stats
            .commands_by_id
            .iter()
            .map(|(id, n)| format!("{id}:{n}"))
            .collect();
        let _ = writeln!(
            out,
            "commands: {} ({})",
            snapshot.stats.commands,
            by_id.join(" ")
        );
    }
    out
}

fn source_label(snapshot: &Snapshot) -> String {
    let source = &snapshot.source;
    format!(
        "{} / {} ({})",
        source.browser.as_deref().unwrap_or("unknown browser"),
        source.profile.as_deref().unwrap_or("unknown profile"),
        source
            .profile_display
            .as_deref()
            .unwrap_or("no display name")
    )
}

fn write_also_found(
    out: &mut String,
    snapshot: &Snapshot,
    candidates: &[platform::BrowserCandidate],
) {
    if candidates.is_empty() {
        return;
    }
    let rows: Vec<(String, String, String)> = candidates
        .iter()
        .rev()
        .map(|candidate| {
            let label = format!(
                "{} ({})",
                candidate.profile.dir_name,
                candidate
                    .profile
                    .display
                    .as_deref()
                    .unwrap_or("no display name")
            );
            let age = match age_seconds(snapshot, candidate) {
                Some(seconds) => format!("{} older", format_age(seconds)),
                None => "age unknown".to_owned(),
            };
            (candidate.browser.id.to_owned(), label, age)
        })
        .collect();
    let width = |pick: fn(&(String, String, String)) -> &str| {
        rows.iter()
            .map(|row| display_width(pick(row)))
            .max()
            .unwrap_or(0)
    };
    let (id_width, label_width, age_width) = (width(|r| &r.0), width(|r| &r.1), width(|r| &r.2));
    let _ = writeln!(out, "  also found:");
    for ((id, label, age), candidate) in rows.iter().zip(candidates.iter().rev()) {
        let note = if candidate.stale.is_some() {
            " (would refuse: encrypted sessions are newer than cleartext)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "    {} / {}  {}  — --browser {id}{note}",
            pad(id, id_width),
            pad(label, label_width),
            pad_left(age, age_width),
        );
    }
}

/// How much older a candidate's newest session is than the one saved, in
/// whole seconds. `None` when either side has no usable suffix; never
/// negative, because the saved file can be an older sibling of the winner's
/// newest when that one did not parse.
fn age_seconds(snapshot: &Snapshot, candidate: &platform::BrowserCandidate) -> Option<i64> {
    let winner = snapshot
        .source
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| platform::session_suffix(name, "Session_"))?;
    if candidate.suffix <= 0 {
        return None;
    }
    Some(winner.saturating_sub(candidate.suffix).max(0) / 1_000_000)
}

fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds} seconds")
    } else if seconds < 3_600 {
        format!("{} minutes", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} hours", seconds / 3_600)
    } else {
        format!("{} days", seconds / 86_400)
    }
}

/// Terminal cells a string takes up. `format!`'s width counts code points,
/// which lines up ASCII and nothing else: profile display names are free
/// text, and East Asian characters and emoji are two cells wide while
/// combining marks and joiners are zero. This is the small table that gets
/// those right without a Unicode-width dependency; anything it does not know
/// counts as one cell.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| match u32::from(c) {
            0x0300..=0x036F | 0x200B..=0x200F | 0x20D0..=0x20FF | 0xFE00..=0xFE0F => 0,
            0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F000..=0x1FAFF
            | 0x20000..=0x3FFFD => 2,
            _ => 1,
        })
        .sum()
}

fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(display_width(text)))
    )
}

fn pad_left(text: &str, width: usize) -> String {
    format!(
        "{}{text}",
        " ".repeat(width.saturating_sub(display_width(text)))
    )
}

fn also_found_json(
    snapshot: &Snapshot,
    also_found: &[platform::BrowserCandidate],
) -> Vec<serde_json::Value> {
    also_found
        .iter()
        .rev()
        .map(|candidate| {
            serde_json::json!({
                "browser": candidate.browser.id,
                "profile": candidate.profile.dir_name,
                "profile_display": candidate.profile.display,
                "age_seconds": age_seconds(snapshot, candidate),
                "stale": candidate.stale.is_some(),
            })
        })
        .collect()
}

pub fn json_outcome(outcome: &Outcome) -> serde_json::Value {
    match outcome {
        Outcome::Saved {
            path,
            snapshot,
            also_found,
        } => serde_json::json!({
            "saved": {
                "id": snapshot.id,
                "path": path,
                "browser": snapshot.source.browser,
                "profile": snapshot.source.profile,
                "profile_display": snapshot.source.profile_display,
                "tabs": snapshot.stats.tabs,
                "windows": snapshot.stats.windows,
                "groups": snapshot.stats.groups,
                "degraded": snapshot.stats.is_degraded(),
                "stats": snapshot.stats,
                "source": snapshot.source,
                "history": snapshot.history,
            },
            "also_found": also_found_json(snapshot, also_found),
        }),
        Outcome::Skipped {
            previous_id,
            snapshot,
            also_found,
        } => serde_json::json!({
            "skipped": {
                "previous": previous_id,
                "tabs": snapshot.stats.tabs,
                "windows": snapshot.stats.windows,
                "groups": snapshot.stats.groups,
                "degraded": snapshot.stats.is_degraded(),
                "stats": snapshot.stats,
                "source": snapshot.source,
            },
            "also_found": also_found_json(snapshot, also_found),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_cells_not_code_points() {
        assert_eq!(display_width("Person 1"), 8);
        assert_eq!(display_width("研究"), 4);
        assert_eq!(display_width("🐙"), 2);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(pad("研究", 6), "研究  ");
        assert_eq!(pad_left("ab", 4), "  ab");
        assert_eq!(pad("too wide", 2), "too wide");
    }

    #[test]
    fn age_is_whole_seconds_and_never_negative() {
        assert_eq!(format_age(59), "59 seconds");
        assert_eq!(format_age(3_599), "59 minutes");
        assert_eq!(format_age(86_399), "23 hours");
        assert_eq!(format_age(200_000), "2 days");
    }
}
