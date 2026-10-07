//! The command line: clap types for `knowmoretabs` and its subcommands.
//!
//! slice: capture, library, triage, browsers, tags, history, enrich, content
//! why: The whole CLI surface is visible in one file, so "what flags exist
//!      and where are they allowed" is a question answered by reading forty
//!      lines rather than grepping. Global options are global so that
//!      `knowmoretabs --json save` and `knowmoretabs save --json` mean the
//!      same thing, and no subcommand means `save`, because that is the one
//!      command people run every day.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};

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
        /// Include the searches that led to pages and the pages they came from
        #[arg(long)]
        with_history: bool,
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
    /// Add tags to pages or take them off, or get suggestions from an agent you run
    Tag(TagArgs),
    /// Fetch the <head> of library pages, without cookies, and record what they say about themselves
    #[command(
        long_about = "Fetches the <head> of each library page that has no metadata yet, without cookies, \
and appends what it finds to <root>/pages/metadata.jsonl: title, description, og: and twitter: tags, \
JSON-LD types, language and canonical URL, and for public GitHub repositories their topics and README. \
This sends the URLs it fetches to their own sites. Forgotten pages, the private network, search results \
and URLs that carry a token are never fetched; login screens are recorded as behind a login, not fetched. \
One request a second per site."
    )]
    Enrich(FetchArgs),
    /// Capture the main text of library pages as markdown, kept private in the archive
    #[command(
        long_about = "Fetches each library page not captured before, without cookies, keeps its main text \
as markdown in <root>/pages/content/, and appends one line per attempt to <root>/pages/content.jsonl. \
This sends the URLs it fetches to their own sites. X posts are read from the public X post API at \
api.fxtwitter.com, the one third party service it uses, sending only the post's number. \
GitHub repositories, issues, pull requests and discussions are read through gh api when gh is installed and \
signed in, at most four at a time; gh keeps the token. Without it they are read as web pages. \
YouTube videos are read through yt-dlp, with deno or node, given only the video's id and no cookies, one \
video at a time: the description, chapters and one caption track, English first, else the video's own \
language. Without yt-dlp they wait for a later run. What enrich never fetches, content never fetches \
either; X profiles and YouTube channels and playlists are recorded as not a document, and login screens \
as behind a login, without a request. A page that failed is tried again on the next run, and after three \
failed runs it is recorded as unavailable. Each captured page also gets one preview image, fetched from \
the address the page or its route names, which may be on another host: kept as a JPEG of at most 768 \
pixels in <root>/pages/images/, with one line per attempt in <root>/pages/images.jsonl. One request a \
second per site. Never part of save."
    )]
    Content {
        #[command(flatten)]
        fetch: FetchArgs,
        /// Capture only this library page (repeatable)
        #[arg(long = "url", value_name = "URL")]
        urls: Vec<String>,
        /// Capture text only: no preview images this run
        #[arg(long)]
        no_images: bool,
        /// Render nothing in a browser this run: pages that need one wait
        #[arg(long)]
        no_browser: bool,
    },
    /// Say which ways of reading pages this machine can use, and what the archive holds of page text
    #[command(
        long_about = "Reports, for each way content reads pages, whether it is ready, missing or degraded, \
and how to fix it: the generic web tier (compiled in), GitHub through gh (found and its version, plus \
with --live whether gh auth status exits 0; never its output), the X post API, YouTube through yt-dlp with deno or node, and \
headless reading with the --browser binary (chrome by default). Then the archive: whether it is private, \
whether pages/content exists, and its pages by their latest status. Offline by default. With --live, gh \
checks its own sign in with GitHub and the X post API is asked once for a fixed public post. Sends nothing of yours. Exits 0 when \
the generic web tier is ready; missing tools are warnings."
    )]
    Doctor {
        /// Check GitHub sign in and ask the X post API once for a fixed public post
        #[arg(long)]
        live: bool,
    },
    /// Show the History signals the library keeps for its pages, or refresh them
    #[command(
        long_about = "Shows what <root>/pages/history.json holds: how many library pages have History \
signals, when they were last refreshed, and how many History has since forgotten. With --refresh, copies \
the browser's History privately into the archive, as save does, and looks up every page in the library, \
except forgotten pages and pages on this machine. A page History no longer knows keeps its earlier signals."
    )]
    History {
        /// Read History now and update every library page's signals
        #[arg(long)]
        refresh: bool,
    },
    /// List the tag vocabulary with how many pages carry each tag
    Tags {
        /// Add a tag to the vocabulary, or bring back a retired one (repeatable)
        #[arg(long, value_name = "NAME")]
        create: Vec<String>,
        /// Retire a tag: hidden everywhere, kept in library.json (repeatable)
        #[arg(long, value_name = "NAME")]
        retire: Vec<String>,
        /// Say what a tag means, for you and for a tagging agent; "" clears it (repeatable)
        #[arg(long, num_args = 2, value_names = ["NAME", "TEXT"], action = ArgAction::Append)]
        define: Vec<String>,
        /// Include retired tags
        #[arg(long)]
        all: bool,
    },
}

/// What every command that fetches library pages accepts.
#[derive(Debug, Clone, Copy, Args)]
pub struct FetchArgs {
    /// List what would be fetched and what would not, and why; send nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Fetch at most N pages this run
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Fetch pages again, even those fetched before
    #[arg(long)]
    pub refetch: bool,
}

impl From<FetchArgs> for crate::targets::Options {
    fn from(args: FetchArgs) -> Self {
        Self {
            dry_run: args.dry_run,
            limit: args.limit,
            refetch: args.refetch,
        }
    }
}

/// `tag` has three forms: tag pages yourself, write a prompt for an agent,
/// or import what the agent wrote. Clap keeps them apart.
#[derive(Debug, Args, Default)]
pub struct TagArgs {
    /// Page URLs, each exactly as the library shows it
    #[arg(
        value_name = "URL",
        required_unless_present_any = ["prompt", "import"],
        conflicts_with_all = ["prompt", "import"]
    )]
    pub urls: Vec<String>,
    /// A tag to add; a new name joins the vocabulary (repeatable)
    #[arg(
        long,
        value_name = "NAME",
        required_unless_present_any = ["remove", "clear", "prompt", "import"],
        conflicts_with_all = ["prompt", "import"]
    )]
    pub add: Vec<String>,
    /// A tag to take off (repeatable)
    #[arg(long, value_name = "NAME", conflicts_with_all = ["prompt", "import"])]
    pub remove: Vec<String>,
    /// Take back your decision about a tag: neither yours nor dismissed (repeatable)
    #[arg(long, value_name = "NAME", conflicts_with_all = ["prompt", "import"])]
    pub clear: Vec<String>,
    #[command(flatten)]
    pub prompt: PromptArgs,
    #[command(flatten)]
    pub import: ImportArgs,
}

#[derive(Debug, Args, Default)]
pub struct PromptArgs {
    /// Write a work folder (prompt.md, pages.jsonl) for an agent you run, covering pages not yet tagged
    #[arg(
        long = "prompt",
        id = "prompt",
        value_name = "DIR",
        conflicts_with = "import"
    )]
    pub dir: Option<PathBuf>,
    /// With --prompt: every page in the library, tagged or not
    #[arg(long, requires = "prompt", conflicts_with_all = ["import", "urls", "add", "remove", "clear"])]
    pub all: bool,
    /// With --prompt: include the searches and referrers History recorded
    #[arg(long, requires = "prompt", conflicts_with_all = ["import", "urls", "add", "remove", "clear"])]
    pub with_history: bool,
}

#[derive(Debug, Args, Default)]
pub struct ImportArgs {
    /// Validate an agent's tags.jsonl and store it as suggested tags
    #[arg(long = "import", id = "import", value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// With --import: who made the suggestions, instead of the file's "source"
    #[arg(long, value_name = "NAME", requires = "import", conflicts_with_all = ["prompt", "urls", "add", "remove", "clear"])]
    pub source: Option<String>,
    /// With --import: create tags the vocabulary does not have
    #[arg(long, requires = "import", conflicts_with_all = ["prompt", "urls", "add", "remove", "clear"])]
    pub accept_new: bool,
    /// With --import: allow pages from the prompt to be missing
    #[arg(long, requires = "import", conflicts_with_all = ["prompt", "urls", "add", "remove", "clear"])]
    pub partial: bool,
    /// With --import: validate and report; store nothing
    #[arg(long, requires = "import", conflicts_with_all = ["prompt", "urls", "add", "remove", "clear"])]
    pub dry_run: bool,
}

/// `--define A x --define B y` as (A, x), (B, y).
pub fn pairs(values: &[String]) -> Vec<(String, String)> {
    values
        .chunks(2)
        .filter_map(|pair| match pair {
            [a, b] => Some((a.clone(), b.clone())),
            _ => None,
        })
        .collect()
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
    fn export_leaves_out_history_unless_asked() {
        let cli = Cli::try_parse_from(["knowmoretabs", "export"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Export {
                dir: None,
                with_history: false
            })
        ));
        let cli = Cli::try_parse_from(["knowmoretabs", "export", "out", "--with-history"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Export {
                dir: Some(_),
                with_history: true
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
            Some(Command::Tag(TagArgs { urls, add, remove, prompt: PromptArgs { dir: None, .. }, import: ImportArgs { file: None, .. }, .. }))
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
            Some(Command::Tags { create, retire, all: true, .. }) if create.is_empty() && retire == &["Old"]
        ));
    }

    #[test]
    fn tag_prompt_and_import_are_their_own_forms() {
        let parse = |args: &[&str]| {
            let mut all = vec!["knowmoretabs", "tag"];
            all.extend_from_slice(args);
            Cli::try_parse_from(all)
        };
        let cli = parse(&["--prompt", "/w", "--with-history", "--all"]).unwrap();
        assert!(matches!(
            &cli.command,
            Some(Command::Tag(TagArgs {
                prompt: PromptArgs { dir: Some(_), with_history: true, all: true }, urls, ..
            })) if urls.is_empty()
        ));
        let cli = parse(&[
            "--import",
            "/w/tags.jsonl",
            "--source",
            "m",
            "--accept-new",
            "--partial",
            "--dry-run",
        ])
        .unwrap();
        assert!(matches!(
            &cli.command,
            Some(Command::Tag(TagArgs {
                import: ImportArgs {
                    file: Some(_),
                    source: Some(_),
                    accept_new: true,
                    partial: true,
                    dry_run: true
                },
                ..
            }))
        ));
        for bad in [
            &["--prompt", "/w", "--import", "/f"][..],
            &["https://a.test/", "--prompt", "/w"],
            &["--prompt", "/w", "--add", "X"],
            &["--prompt", "/w", "--dry-run"],
            &["--import", "/f", "--with-history"],
            &["https://a.test/", "--add", "X", "--source", "m"],
            &["--all"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let cli = Cli::try_parse_from([
            "knowmoretabs",
            "tags",
            "--define",
            "DPO",
            "Direct preference",
            "--define",
            "X",
            "",
        ])
        .unwrap();
        let Some(Command::Tags { define, .. }) = &cli.command else {
            panic!("not tags");
        };
        assert_eq!(
            pairs(define),
            [
                ("DPO".into(), "Direct preference".into()),
                ("X".into(), String::new())
            ]
        );
        assert!(Cli::try_parse_from(["knowmoretabs", "tags", "--define", "DPO"]).is_err());
    }

    #[test]
    fn tags_has_no_parent_rules_and_tag_takes_clear() {
        for rule in ["--imply", "--unimply"] {
            assert!(Cli::try_parse_from(["knowmoretabs", "tags", rule, "A", "B"]).is_err());
        }
        let parse = |args: &[&str]| {
            let mut all = vec!["knowmoretabs", "tag"];
            all.extend_from_slice(args);
            Cli::try_parse_from(all)
        };
        let cli = parse(&["https://a.test/", "--clear", "X", "--clear", "Y"]).unwrap();
        assert!(matches!(
            &cli.command,
            Some(Command::Tag(TagArgs { urls, add, remove, clear, .. }))
                if urls.len() == 1 && add.is_empty() && remove.is_empty() && clear == &["X", "Y"]
        ));
        for bad in [
            &["--prompt", "/w", "--clear", "X"][..],
            &["--import", "/f", "--clear", "X"],
            &["--clear", "X"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn enrich_takes_its_three_flags_and_the_global_ones() {
        let cli = Cli::try_parse_from(["knowmoretabs", "enrich"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Enrich(FetchArgs {
                dry_run: false,
                limit: None,
                refetch: false
            }))
        ));
        let cli = Cli::try_parse_from([
            "knowmoretabs",
            "--json",
            "enrich",
            "--dry-run",
            "--limit",
            "30",
            "--refetch",
        ])
        .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Some(Command::Enrich(FetchArgs {
                dry_run: true,
                limit: Some(30),
                refetch: true
            }))
        ));
        assert!(Cli::try_parse_from(["knowmoretabs", "enrich", "--limit", "x"]).is_err());
        assert!(
            Cli::try_parse_from(["knowmoretabs", "enrich", "--url", "https://a.test/"]).is_err()
        );
    }

    #[test]
    fn content_shares_the_fetch_flags_and_takes_urls() {
        let cli = Cli::try_parse_from([
            "knowmoretabs",
            "content",
            "--dry-run",
            "--limit",
            "5",
            "--url",
            "https://a.test/",
            "--url",
            "https://b.test/",
            "--json",
        ])
        .unwrap();
        assert!(cli.json);
        let Some(Command::Content {
            fetch,
            urls,
            no_images,
            no_browser,
        }) = cli.command
        else {
            panic!("content parsed as {:?}", cli.command);
        };
        assert!(fetch.dry_run && !fetch.refetch && !no_images && !no_browser);
        assert_eq!(fetch.limit, Some(5));
        assert_eq!(urls, ["https://a.test/", "https://b.test/"]);
        let cli = Cli::try_parse_from(["knowmoretabs", "content", "--no-images"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Content {
                no_images: true,
                ..
            })
        ));
        assert!(Cli::try_parse_from(["knowmoretabs", "enrich", "--no-images"]).is_err());
        let cli = Cli::try_parse_from(["knowmoretabs", "content", "--no-browser"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Content {
                no_browser: true,
                ..
            })
        ));
        assert!(Cli::try_parse_from(["knowmoretabs", "enrich", "--no-browser"]).is_err());
    }

    #[test]
    fn history_shows_by_default_and_refreshes_when_asked() {
        let cli = Cli::try_parse_from(["knowmoretabs", "history"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::History { refresh: false })
        ));
        let cli =
            Cli::try_parse_from(["knowmoretabs", "history", "--refresh", "--profile", "Work"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::History { refresh: true })
        ));
        assert_eq!(cli.profile.as_deref(), Some("Work"));
        assert!(Cli::try_parse_from(["knowmoretabs", "history", "--no-history"]).is_err());
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
