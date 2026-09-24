//! The command line: clap types for `knowmoretabs` and its subcommands.
//!
//! slice: capture, library, triage, browsers
//! why: The whole CLI surface is visible in one file, so "what flags exist
//!      and where are they allowed" is a question answered by reading forty
//!      lines rather than grepping. Global options are global so that
//!      `knowmoretabs --json save` and `knowmoretabs save --json` mean the
//!      same thing, and no subcommand means `save`, because that is the one
//!      command people run every day.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "knowmoretabs",
    version,
    about = "Keep and search the browser tabs you have had open",
    long_about = "Reads a Chromium-family browser's own session file, saves a dated snapshot of every open \
window and tab, and never overwrites an earlier one. With no subcommand, runs `save`.",
    after_help = "Exit status: 0 success; 1 error; 3 refused because Chrome's \
encrypted session files are newer than the cleartext ones it still writes."
)]
pub struct Cli {
    /// Archive directory (default: ~/.knowmoretabs; on Windows %LOCALAPPDATA%\knowmoretabs)
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// Read this session file instead of discovering the newest one
    #[arg(long, global = true, value_name = "FILE")]
    pub session: Option<PathBuf>,

    /// Browser id (chrome, chrome-beta, chrome-canary, chromium, brave, edge, or vivaldi)
    #[arg(long, global = true, value_name = "NAME", conflicts_with = "session")]
    pub browser: Option<String>,

    /// Browser profile: a directory name ("Profile 1") or its display name ("Work")
    #[arg(long, global = true, value_name = "NAME", conflicts_with = "session")]
    pub profile: Option<String>,

    /// The browser's user-data directory, when it is not in the default place
    #[arg(long, global = true, value_name = "DIR", conflicts_with = "session")]
    pub user_data_dir: Option<PathBuf>,

    /// Machine-readable output on stdout
    #[arg(long, global = true)]
    pub json: bool,

    /// Show the source file, statistics, and the cause chain of errors
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Print nothing on success and no warnings
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(flatten)]
    pub save: SaveArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Capture the newest browser session as a snapshot (the default)
    Save(SaveArgs),
    /// List saved snapshots, newest first
    List,
    /// Write the library as a static site that opens from file://
    Export {
        /// Directory to write it to (default: <root>/export)
        #[arg(value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Serve the library on 127.0.0.1 with live forget, restore and tagging
    Serve {
        /// Port to listen on; 0 picks a free one
        #[arg(long, default_value_t = DEFAULT_PORT, value_name = "N")]
        port: u16,
        /// Open the page in your browser once the server is listening
        #[arg(long)]
        open: bool,
    },
    /// Hide pages from the library; the snapshots keep them
    Forget {
        /// Page URLs to hide, each exactly as the library shows it
        #[arg(required = true, value_name = "URL")]
        urls: Vec<String>,
    },
    /// Bring forgotten pages back into the library
    Restore {
        /// Forgotten URLs to bring back, each exactly as it was forgotten
        #[arg(required = true, value_name = "URL")]
        urls: Vec<String>,
    },
    /// Add tags to pages or take them off
    Tag {
        /// Page URLs, each exactly as the library shows it
        #[arg(required = true, value_name = "URL")]
        urls: Vec<String>,
        /// A tag to add; a new name joins the vocabulary (repeatable)
        #[arg(long, value_name = "NAME", required_unless_present = "remove")]
        add: Vec<String>,
        /// A tag to take off (repeatable)
        #[arg(long, value_name = "NAME")]
        remove: Vec<String>,
    },
    /// List the tag vocabulary with how many pages carry each tag
    Tags {
        /// Add a tag to the vocabulary, or bring back a retired one (repeatable)
        #[arg(long, value_name = "NAME")]
        create: Vec<String>,
        /// Retire a tag: hidden everywhere, kept in library.json (repeatable)
        #[arg(long, value_name = "NAME")]
        retire: Vec<String>,
        /// Include retired tags
        #[arg(long)]
        all: bool,
    },
}

/// The port the frontend's stand-in used, so a bookmark from then still works.
pub const DEFAULT_PORT: u16 = 7878;

#[derive(Debug, Args, Default, Clone)]
pub struct SaveArgs {
    /// Save even if the window and tab layout matches the previous snapshot
    #[arg(long)]
    pub force: bool,
    /// Do not read the browser's History: no searches, referrers or visit counts
    #[arg(long)]
    pub no_history: bool,
}

impl Cli {
    /// The effective `save` arguments whether or not the subcommand was named.
    pub fn save_args(&self) -> SaveArgs {
        match &self.command {
            Some(Command::Save(args)) => SaveArgs {
                force: args.force || self.save.force,
                no_history: args.no_history || self.save.no_history,
            },
            _ => self.save.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_means_save_and_flags_work_in_both_positions() {
        let cli = Cli::try_parse_from(["knowmoretabs", "--force", "--json"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.save_args().force);
        assert!(cli.json);

        let cli = Cli::try_parse_from(["knowmoretabs", "save", "--force", "-v"]).unwrap();
        assert!(cli.save_args().force);
        assert!(cli.verbose);

        let cli = Cli::try_parse_from(["knowmoretabs", "--root", "/r", "save"]).unwrap();
        assert_eq!(cli.root.as_deref(), Some(std::path::Path::new("/r")));
        assert!(!cli.save_args().force);
        assert!(!cli.save_args().no_history);

        for args in [
            &["knowmoretabs", "--no-history"][..],
            &["knowmoretabs", "save", "--no-history"],
        ] {
            assert!(Cli::try_parse_from(args).unwrap().save_args().no_history);
        }
    }

    #[test]
    fn conflicting_flags_are_rejected() {
        assert!(Cli::try_parse_from(["knowmoretabs", "-v", "-q"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "--session", "f", "--profile", "p"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "forget"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "serve", "--port", "x"]).is_err());
    }

    #[test]
    fn triage_commands_take_urls_and_serve_takes_a_port() {
        let cli = Cli::try_parse_from([
            "knowmoretabs",
            "forget",
            "https://a.test/",
            "https://b.test/",
        ])
        .unwrap();
        assert!(matches!(&cli.command, Some(Command::Forget { urls }) if urls.len() == 2));
        let cli = Cli::try_parse_from(["knowmoretabs", "serve"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Serve {
                port: DEFAULT_PORT,
                open: false
            })
        ));
        let cli = Cli::try_parse_from(["knowmoretabs", "serve", "--port", "0", "--open"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Serve {
                port: 0,
                open: true
            })
        ));
    }

    #[test]
    fn tag_takes_urls_and_repeatable_names_and_needs_one() {
        let cli = Cli::try_parse_from([
            "knowmoretabs",
            "tag",
            "https://a.test/",
            "--add",
            "Harness",
            "https://b.test/",
            "--add",
            "MCP",
            "--remove",
            "Old",
        ])
        .unwrap();
        assert!(matches!(
            &cli.command,
            Some(Command::Tag { urls, add, remove })
                if urls.len() == 2 && add == &["Harness", "MCP"] && remove == &["Old"]
        ));
        assert!(Cli::try_parse_from(["knowmoretabs", "tag", "https://a.test/"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "tag", "--add", "X"]).is_err());
        assert!(
            Cli::try_parse_from(["knowmoretabs", "tag", "https://a.test/", "--remove", "X"])
                .is_ok()
        );
        let cli =
            Cli::try_parse_from(["knowmoretabs", "tags", "--retire", "Old", "--all"]).unwrap();
        assert!(matches!(
            &cli.command,
            Some(Command::Tags { create, retire, all: true }) if create.is_empty() && retire == &["Old"]
        ));
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
