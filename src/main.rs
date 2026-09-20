//! Entry point: parses the command line, runs the capture, and turns the
//! outcome into stdout lines or JSON and an exit status.
//!
//! slice: capture
//! why: Presentation lives here and nowhere else. The capture code returns
//!      typed outcomes and typed errors; this file decides what a person
//!      sees on a terminal, what a script sees under `--json`, and which
//!      exit code each outcome maps to. Keeping that in one short file means
//!      the library-shaped modules stay silent and testable.

mod archive;
mod assets;
mod capture;
mod cli;
mod error;
mod export;
mod library;
mod library_commands;
mod model;
mod out;
mod platform;
mod server;
mod session;
mod snss;
mod staleness;
mod triage;

use std::fmt::Write as _;
use std::process::ExitCode;

use clap::Parser;

use capture::{Log, Options, Outcome};
use cli::{Cli, Command};
use model::Snapshot;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let log = Log {
        quiet: cli.quiet,
        verbose: cli.verbose,
    };
    let Some(root) = cli.root.clone().or_else(default_root) else {
        out::problem(&error::render(&error::Error::NoHome, cli.verbose, cli.json));
        return ExitCode::from(error::Error::NoHome.exit_code());
    };
    let result = match &cli.command {
        Some(Command::List) => library_commands::list(&root, cli.json, log),
        Some(Command::Export { dir }) => {
            library_commands::export(&root, dir.as_deref(), cli.json, log)
        }
        Some(Command::Serve { port, open }) => server::run(&server::Options {
            root,
            port: *port,
            open: *open,
            json: cli.json,
            log,
        }),
        Some(Command::Forget { urls }) => {
            triage::command(&root, urls, triage::Action::Forget, cli.json, log)
        }
        Some(Command::Restore { urls }) => {
            triage::command(&root, urls, triage::Action::Restore, cli.json, log)
        }
        _ => save(&cli, root, log),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            out::problem(&error::render(&err, cli.verbose, cli.json));
            ExitCode::from(err.exit_code())
        }
    }
}

fn save(cli: &Cli, root: std::path::PathBuf, log: Log) -> Result<(), error::Error> {
    let opts = Options {
        root,
        session: cli.session.clone(),
        browser: cli.browser.clone(),
        profile: cli.profile.clone(),
        user_data_dir: cli.user_data_dir.clone(),
        force: cli.save_args().force,
    };
    let outcome = capture::save(&opts, log)?;
    if cli.json {
        out::json(&json_outcome(&outcome));
    } else if !cli.quiet {
        out::block(&human_outcome(&outcome, cli.verbose));
    }
    Ok(())
}

fn default_root() -> Option<std::path::PathBuf> {
    platform::home_dir().map(|h| h.join(platform::DEFAULT_ROOT_NAME))
}

fn summary(snapshot: &Snapshot) -> String {
    let s = &snapshot.stats;
    let groups = match s.groups {
        0 => String::new(),
        1 => ", 1 group".to_owned(),
        n => format!(", {n} groups"),
    };
    format!(
        "{} tabs across {} window{}{groups}",
        s.tabs,
        s.windows,
        if s.windows == 1 { "" } else { "s" }
    )
}

fn human_outcome(outcome: &Outcome, verbose: bool) -> String {
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
    let winner_suffix = snapshot
        .source
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| platform::session_suffix(name, "Session_"));
    let _ = writeln!(out, "  also found:");
    for candidate in candidates.iter().rev() {
        let age = winner_suffix.map_or(0, |suffix| {
            suffix.saturating_sub(candidate.suffix) / 1_000_000
        });
        let _ = writeln!(
            out,
            "    {} / {} ({})       {} older  — --browser {}",
            candidate.browser.id,
            candidate.profile.dir_name,
            candidate
                .profile
                .display
                .as_deref()
                .unwrap_or("no display name"),
            format_age(age),
            candidate.browser.id
        );
    }
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

fn json_outcome(outcome: &Outcome) -> serde_json::Value {
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
            },
            "also_found": also_found.iter().map(|candidate| serde_json::json!({
                "browser": candidate.browser.id,
                "profile": candidate.profile.dir_name,
                "profile_display": candidate.profile.display,
                "age_seconds": snapshot.source.path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| platform::session_suffix(name, "Session_"))
                    .map_or(0, |suffix| suffix.saturating_sub(candidate.suffix) / 1_000_000),
            })).collect::<Vec<_>>(),
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
            "also_found": also_found.iter().map(|candidate| serde_json::json!({
                "browser": candidate.browser.id,
                "profile": candidate.profile.dir_name,
                "profile_display": candidate.profile.display,
            })).collect::<Vec<_>>(),
        }),
    }
}
