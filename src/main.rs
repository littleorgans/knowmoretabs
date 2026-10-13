//! Entry point: parses the command line, runs the chosen command, and turns
//! the outcome into stdout lines or JSON and an exit status.
//!
//! slice: capture, library, triage, tags, history, enrich, content, add, sources
//! why: Commands are dispatched here, with presentation delegated to their
//!      modules and save_report.rs. Typed errors become terminal lines or
//!      JSON and exit codes here. Every slice that added a command added
//!      its dispatch here, so this file claims each of them.

mod add;
mod add_content;
mod archive;
mod assets;
mod browser;
mod capture;
mod cdp;
mod cli;
mod content;
mod content_events;
mod content_fetch;
mod content_headless;
mod content_image;
mod content_pdf;
mod content_plan;
mod content_report;
mod content_route;
mod content_signed_in;
mod content_store;
#[cfg(test)]
mod content_test;
mod content_view;
mod doctor;
mod enrich;
mod error;
mod export;
mod extract;
mod fetch;
mod github;
mod github_api;
mod github_page;
mod guard;
mod head;
mod history;
mod http;
mod image_fetch;
mod image_page;
mod image_pick;
mod image_store;
mod intake;
mod jsonl;
mod jsonld;
mod library;
mod library_commands;
mod library_history;
mod local;
mod metadata;
mod metadata_fetch;
mod metadata_writer;
mod model;
mod model_routes;
mod model_service;
mod out;
mod page_key;
mod platform;
mod process_tree;
mod prompt;
mod save_report;
mod search;
mod server;
mod service_auth;
mod session;
mod snss;
mod socks;
mod source_add;
mod source_git;
mod source_key;
mod sources;
mod staleness;
mod suggestions;
mod tags;
mod targets;
mod tools;
mod triage;
mod xpost;
mod youtube;
mod youtube_page;
mod ytdlp;

use std::process::ExitCode;

use clap::Parser;

use capture::{Log, Options};
use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let log = Log {
        quiet: cli.quiet,
        verbose: cli.verbose,
    };
    let roots = platform::Roots::detect();
    let Some(platform::ArchiveRoot {
        path: root,
        explicit,
    }) = platform::resolve_root(
        cli.root.clone(),
        std::env::var_os("KMT_ROOT").map(std::path::PathBuf::from),
        roots.as_ref(),
    )
    else {
        return fail(&error::Error::NoHome, &cli);
    };
    // Before any command touches it: a root Windows cannot represent has to
    // say so by name, not as a mysterious failure three calls later.
    if let Err(problem) = platform::check_root(&root) {
        return fail(
            &error::Error::RootName {
                root: root.clone(),
                problem,
            },
            &cli,
        );
    }
    warn_if_root_is_not_private(&cli, &root, roots.as_ref(), explicit);
    let result = match &cli.command {
        Some(Command::List) => library_commands::list(&root, cli.json, log),
        Some(Command::Export { dir, with_history }) => {
            library_commands::export(&root, dir.as_deref(), *with_history, cli.json, log)
        }
        Some(Command::Serve(args)) => serve(root, args, cli.json, log),
        Some(Command::Add(args)) => match add(&root, args, &cli, roots.as_ref(), log) {
            Ok(code) => return code,
            Err(err) => Err(err),
        },
        Some(Command::Search(args)) => search(&root, args, &cli, roots.as_ref(), log),
        Some(Command::Read { key }) => read(&root, key, &cli, roots.as_ref(), log),
        Some(Command::Forget { urls }) => {
            triage::command(&root, urls, triage::Action::Forget, cli.json, log)
        }
        Some(Command::Restore { urls }) => {
            triage::command(&root, urls, triage::Action::Restore, cli.json, log)
        }
        Some(Command::Tag(args)) => tag(&root, args, cli.json, log),
        Some(Command::Tags {
            create,
            retire,
            define,
            all,
        }) => {
            let edit = tags::VocabularyEdit {
                create: create.clone(),
                retire: retire.clone(),
                define: cli::pairs(define),
            };
            tags::tags_command(&root, &edit, *all, cli.json, log)
        }
        Some(Command::Enrich(fetch)) => enrich::command(&root, (*fetch).into(), cli.json, log),
        Some(Command::Content {
            fetch,
            urls,
            no_images,
            no_browser,
            signed_in,
        }) => content::command(
            &root,
            content::Args {
                options: (*fetch).into(),
                urls,
                no_images: *no_images,
                browser: cli.browser.as_deref().unwrap_or(platform::CHROME),
                no_browser: *no_browser,
                signed_in: *signed_in,
                events: None,
                retry: None,
            },
            cli.json,
            log,
        ),
        Some(Command::Doctor { live }) => {
            doctor::command(&root, cli.browser.as_deref(), *live, cli.json, log)
        }
        Some(Command::History { refresh: true }) => {
            library_history::refresh_command(&capture_options(&cli, root), cli.json, log)
        }
        Some(Command::History { refresh: false }) => {
            library_history::status_command(&root, cli.json, log)
        }
        _ => save(&cli, root, log),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&err, &cli),
    }
}

/// The checkouts file: `--checkouts`, else the config home's.
fn checkouts(
    cli: &Cli,
    roots: Option<&platform::Roots>,
) -> Result<std::path::PathBuf, error::Error> {
    cli.checkouts
        .clone()
        .or_else(|| roots.map(|roots| roots.config_file(sources::CHECKOUTS_FILE)))
        .ok_or(error::Error::NoHome)
}

fn search(
    root: &std::path::Path,
    args: &cli::SearchArgs,
    cli: &Cli,
    roots: Option<&platform::Roots>,
    log: Log,
) -> Result<(), error::Error> {
    let checkouts = checkouts(cli, roots)?;
    let model = args.model.model_port.and_then(|port| {
        model_service::ModelService::new(port, args.model.model_secret_file.as_deref(), log)
    });
    let query = args.query.join(" ");
    search::command(root, &query, &checkouts, model.as_ref(), cli.json, log)
}

fn read(
    root: &std::path::Path,
    key: &str,
    cli: &Cli,
    roots: Option<&platform::Roots>,
    log: Log,
) -> Result<(), error::Error> {
    sources::read_command(root, key, &checkouts(cli, roots)?, cli.json, log)
}

/// `add`, whose exit status is its own.
fn add(
    root: &std::path::Path,
    args: &cli::AddArgs,
    cli: &Cli,
    roots: Option<&platform::Roots>,
    log: Log,
) -> Result<ExitCode, error::Error> {
    if let Some(file) = &args.file {
        let args = source_add::Args {
            file,
            title: args.title.as_deref(),
            checkouts: &checkouts(cli, roots)?,
        };
        return source_add::command(root, args, cli.json, log);
    }
    let retry: Vec<content::Stage> = args
        .retry
        .iter()
        .map(|stage| match stage {
            cli::RetryStage::Content => content::Stage::Content,
            cli::RetryStage::Image => content::Stage::Image,
        })
        .collect();
    let ask = add_content::Ask {
        browser: cli.browser.as_deref().unwrap_or(platform::CHROME),
        signed_in: args.signed_in,
        retry: &retry,
    };
    let args = add::Args {
        url: args.url.as_deref().unwrap_or_default(),
        title: args.title.as_deref(),
        content: (!args.no_content).then_some(ask),
    };
    add::command(root, args, cli.json, log)
}

fn serve(
    root: std::path::PathBuf,
    args: &cli::ServeArgs,
    json: bool,
    log: Log,
) -> Result<(), error::Error> {
    server::run(&server::Options {
        root,
        port: args.port,
        model_port: args.model.model_port,
        model_secret_file: args.model.model_secret_file.clone(),
        open: args.open,
        json,
        log,
    })
}

/// The three forms of `tag`, which clap has already kept apart.
fn tag(
    root: &std::path::Path,
    args: &cli::TagArgs,
    json: bool,
    log: Log,
) -> Result<(), error::Error> {
    if let Some(dir) = &args.prompt.dir {
        let options = prompt::PromptOptions {
            dir,
            all: args.prompt.all,
            with_history: args.prompt.with_history,
        };
        return prompt::prompt_command(root, &options, json, log);
    }
    if let Some(file) = &args.import.file {
        let options = suggestions::ImportOptions {
            file,
            source: args.import.source.as_deref(),
            accept_new: args.import.accept_new,
            partial: args.import.partial,
            dry_run: args.import.dry_run,
        };
        return suggestions::import_command(root, &options, json, log);
    }
    tags::tag_command(
        root,
        &args.urls,
        &args.add,
        &args.remove,
        &args.clear,
        json,
        log,
    )
}

/// The one way a command ends badly, so there is one answer to which stream
/// the report goes to.
///
/// Under `--json` the report is a document and goes to stdout, beside the
/// documents every other command writes there: `--json` means machine-
/// readable output on stdout, and a caller that pipes stdout to a parser
/// should get something parseable whether the run worked or not. It cannot
/// go to stderr, because stderr is the human channel in every mode — it
/// carries the warnings and the `-v` notes — and a document sharing a
/// stream with free text is a document nobody can read: one warning ahead
/// of it and the whole stream stops being JSON.
///
/// Without `--json` there is no document, and the one line goes to stderr
/// where the rest of the prose is.
fn fail(err: &error::Error, cli: &Cli) -> ExitCode {
    if cli.json {
        out::json(&error::render_json(err));
    } else {
        out::problem(&error::render_human(err, cli.verbose));
    }
    ExitCode::from(err.exit_code())
}

/// Windows only, and only when the root is somewhere Windows would not keep
/// private on its own: see [`platform::root_outside_home`] for what that
/// means and why passing `--root` is not by itself worth saying anything
/// about. A default root is always private, so it is never worth a word.
fn warn_if_root_is_not_private(
    cli: &Cli,
    root: &std::path::Path,
    roots: Option<&platform::Roots>,
    explicit: bool,
) {
    if !cfg!(windows) || cli.quiet || !explicit {
        return;
    }
    let (Some(roots), Ok(current_dir)) = (roots, std::env::current_dir()) else {
        return;
    };
    if platform::root_outside_home(root, &roots.home, &current_dir) {
        out::problem(&format!(
            "warning: {} is outside your user profile, so it keeps whatever permissions its parent grants and other accounts on this machine may be able to read it; an archive of every page you have open belongs somewhere private, such as under %LOCALAPPDATA%",
            root.display()
        ));
    }
}

/// Where to find the browser, from the global flags, and what `save` does.
fn capture_options(cli: &Cli, root: std::path::PathBuf) -> Options {
    Options {
        root,
        session: cli.session.clone(),
        browser: cli.browser.clone(),
        profile: cli.profile.clone(),
        user_data_dir: cli.user_data_dir.clone(),
        force: cli.save_args().force,
        history: !cli.save_args().no_history,
    }
}

fn save(cli: &Cli, root: std::path::PathBuf, log: Log) -> Result<(), error::Error> {
    let opts = capture_options(cli, root);
    let outcome = capture::save(&opts, log)?;
    if cli.json {
        out::json(&save_report::json_outcome(&outcome));
    } else if !cli.quiet {
        out::block(&save_report::human_outcome(&outcome, cli.verbose));
    }
    Ok(())
}
