//! The command line: clap types for `knowmoretabs` and its subcommands.
//!
//! slice: capture
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
    long_about = "Reads Chrome's own session file, saves a dated snapshot of every open \
window and tab, and never overwrites an earlier one. With no subcommand, runs `save`.",
    after_help = "Exit status: 0 success; 1 error; 3 refused because Chrome's \
encrypted session files are newer than the cleartext ones it still writes."
)]
pub struct Cli {
    /// Archive directory (default: ~/.knowmoretabs)
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// Read this session file instead of discovering Chrome's newest
    #[arg(long, global = true, value_name = "FILE")]
    pub session: Option<PathBuf>,

    /// Chrome profile: a directory name ("Profile 1") or its display name ("Work")
    #[arg(long, global = true, value_name = "NAME", conflicts_with = "session")]
    pub profile: Option<String>,

    /// Chrome user-data directory, when it is not in the default place
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
    /// Capture the newest Chrome session as a snapshot (the default)
    Save(SaveArgs),
    /// List saved snapshots, newest first
    List,
    /// Write the offline library (default: <root>/export)
    Export {
        #[arg(value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

#[derive(Debug, Args, Default, Clone)]
pub struct SaveArgs {
    /// Save even if the window and tab layout matches the previous snapshot
    #[arg(long)]
    pub force: bool,
}

impl Cli {
    /// The effective `save` arguments whether or not the subcommand was named.
    pub fn save_args(&self) -> SaveArgs {
        match &self.command {
            Some(Command::Save(args)) => SaveArgs {
                force: args.force || self.save.force,
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
    }

    #[test]
    fn conflicting_flags_are_rejected() {
        assert!(Cli::try_parse_from(["knowmoretabs", "-v", "-q"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "--session", "f", "--profile", "p"]).is_err());
        assert!(Cli::try_parse_from(["knowmoretabs", "serve"]).is_err());
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
