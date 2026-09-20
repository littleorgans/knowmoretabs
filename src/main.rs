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
        Outcome::Saved { path, snapshot } => {
            let _ = writeln!(out, "saved {} to {}", summary(snapshot), path.display());
            snapshot
        }
        Outcome::Skipped {
            previous_id,
            snapshot,
        } => {
            let _ = writeln!(
                out,
                "no change since {previous_id}: {}. Nothing saved; use --force to save anyway.",
                summary(snapshot)
            );
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

fn json_outcome(outcome: &Outcome) -> serde_json::Value {
    match outcome {
        Outcome::Saved { path, snapshot } => serde_json::json!({
            "saved": {
                "id": snapshot.id,
                "path": path,
                "tabs": snapshot.stats.tabs,
                "windows": snapshot.stats.windows,
                "groups": snapshot.stats.groups,
                "degraded": snapshot.stats.is_degraded(),
                "stats": snapshot.stats,
                "source": snapshot.source,
            }
        }),
        Outcome::Skipped {
            previous_id,
            snapshot,
        } => serde_json::json!({
            "skipped": {
                "previous": previous_id,
                "tabs": snapshot.stats.tabs,
                "windows": snapshot.stats.windows,
                "groups": snapshot.stats.groups,
                "degraded": snapshot.stats.is_degraded(),
                "stats": snapshot.stats,
                "source": snapshot.source,
            }
        }),
    }
}
