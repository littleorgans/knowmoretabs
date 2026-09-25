# knowmoretabs

Save your live tabs, and everything that becomes possible once they are saved.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` saves a dated
snapshot of every open window and tab, and gives you a local page where you
search everything you have ever had open, see what you keep reopening, and
forget what you do not want. It reads your browser's own session file from
disk to do it, so there is nothing to install in the browser. Nothing leaves
your machine unless you run `enrich`, which fetches the `<head>` of pages you
already visited.

It works with Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and
Vivaldi, on macOS, Linux and Windows.

## Install

With a Rust toolchain (1.89 or newer), one command builds it from the
repository and puts it on your `PATH`:

```
cargo install --git https://github.com/littleorgans/knowmoretabs knowmoretabs
```

Or clone the repository and run `cargo build --release`; the binary is
`target/release/knowmoretabs`, and it depends on nothing else.

Nothing is tagged yet: the prebuilt binaries and the crates.io package
described next arrive with v0.1.0, and until then the repository is the only
source.

Each tagged release publishes the crate, so `cargo install knowmoretabs`
works without cloning, and puts prebuilt binaries on the
[Releases](https://github.com/littleorgans/knowmoretabs/releases) page:

| Platform | Archive |
|---|---|
| macOS, Apple silicon | `knowmoretabs-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `knowmoretabs-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, x86-64 | `knowmoretabs-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux, ARM64 | `knowmoretabs-<version>-aarch64-unknown-linux-gnu.tar.gz` |
| Windows, x86-64 | `knowmoretabs-<version>-x86_64-pc-windows-msvc.zip` |

Unpack one and put the `knowmoretabs` binary somewhere on your `PATH`. Each
archive also carries this README, the changelog, `LICENSE-MIT` and
`LICENSE-APACHE`, and
`SHA256SUMS` on the release page lets you check what you downloaded:

```
sha256sum -c --ignore-missing SHA256SUMS
```

The binaries are not code-signed. macOS may refuse to open one that arrived
through a browser; either download with `curl`, which sets no quarantine
flag, or clear it with `xattr -d com.apple.quarantine knowmoretabs`. The
Linux binaries are built on Ubuntu 24.04 against its glibc; if one refuses
to start on an older distribution, build from source instead.

## The two commands that matter

```
knowmoretabs save    # snapshot what is open right now (also the default)
knowmoretabs serve   # search everything you have ever had open, and forget what you don't want
```

```
$ knowmoretabs
saved 113 tabs across 12 windows, 3 groups to /Users/you/.knowmoretabs/snapshots/2026-09-20-084415Z from chrome / Default (Person 1)

$ knowmoretabs
no change since 2026-09-20-084415Z: 113 tabs across 12 windows, 3 groups. Nothing saved; use --force to save anyway.

$ knowmoretabs serve
Your library is at http://127.0.0.1:7878/
Local only: it answers this machine and nothing else. Press Ctrl-C to stop.
```

`save` reads the newest session file, copies it verbatim, and records every
tab's URL, title, window, position, pinned state, tab group (name, colour,
collapsed) and last-active time. Tabs on this machine (`localhost`, its
subdomains, and loopback addresses such as `127.0.0.1`) are left out: a
development server is not a page you can go back to. `save` says how many it
left out, and they never count as a change. A run whose window, tab and URL
layout matches the newest snapshot saves nothing; `--force` saves anyway.

`save` also records what the browser's own History knows about each tab's
page: how often you visited it and typed its address, when History first and
last saw it, how long it was in the foreground, the search that led to it (up
to three links back), and the page you came from. It stays on this machine,
in the snapshot, and the library shows it under each page's history (see
below). Chrome keeps about 90 days of
History; a snapshot keeps what it saw for good, so clearing your browsing
data in the browser does not clear it from snapshots already saved.
`save --no-history` leaves it out of the snapshots it writes. If History
cannot be read, `save` says so once and saves the tabs without it.

A snapshot only has signals for the tabs open when it was taken, so the
library also keeps its own record, `pages/history.json`, with the signals of
every page it lists. Each `save` that writes a snapshot refreshes it from the
same copy of History; `knowmoretabs history --refresh` does it without a save,
and `knowmoretabs history` says how many pages it covers and when it was last
refreshed. Forgotten pages and pages on this machine are not looked up. A page
History has forgotten keeps the signals it had, with the date History last
knew it. A skipped save and `save --no-history` leave the record alone.

`serve` is a page on `127.0.0.1` listing every page you have ever had open,
once, with search, a site filter, an open-or-not filter, five sort orders,
tab groups, and each page's history: the snapshots and windows it appeared
in and, for pages History knew when the library's record was last refreshed
(or when a snapshot with signals saw them), the search that found it, the
page you came from, its visits and its time on page. Select rows and forget them; undo from the toast; find them again under
"Forgotten". `/` focuses search, `j` and `k` move, `f` forgets. `--port N`
picks another port and `--open` opens your browser.

Forgetting hides a page from the library. It never touches a snapshot: every
page you forget is still in every snapshot it was ever in, and `restore`
brings it back.

`export` writes the same library as files, and leaves out the search that led
to each page and the page you came from, because an exported folder is the
copy most likely to be sent somewhere. Visits, typed visits and time on page
stay. `export --with-history` puts the two back. Either way an export never
names a forgotten page, not even as where another page came from.

Tags are yours to set, and flat: a page carries as many as it needs, and the
meaning is in the combination. `tag` puts them on pages or takes them off, and
`tags` lists them with how many pages carry each. A name you have not used
before joins your vocabulary, and names match without regard to case, so
`mcp` finds `MCP`. `tags --retire NAME` hides a tag everywhere and keeps it,
and every page it was on, in `library.json`; adding it to a page again brings
it back. `tags --define NAME TEXT` says what a tag means, and
`tags --imply DPO Training` records a parent rule that always holds.

Nothing is tagged for you without asking, and `knowmoretabs` contains no model
and makes no request to one. Instead it hands the work to an agent you choose:
`tag --prompt DIR` writes a folder with `prompt.md` (how you tag, what each of
your tags means, the exact answer format, and a check to run before
finishing) and `pages.jsonl` (every page not yet tagged). Open Claude Code,
Codex or any other agent in that folder and say "Read prompt.md and carry it
out." It writes `tags.jsonl`; `tag --import DIR/tags.jsonl` checks every line
against your library and vocabulary, refuses the whole file if anything is
wrong, and otherwise stores the answers as suggestions, with the model's name,
the date and the vocabulary version. Your own tags are never changed. The
library shows each page's suggestions after its tags, dashed, with a mark for
each source that made them, so a tag two agents agree on stands out from one
only one of them suggested. They count toward the tag bar and its filters until
you decide. In `serve`, click a suggestion (or press `+`) to confirm it with ✓
or dismiss it with ×, one page at a time or across a selection, and undo
either from the toast. Show ▸ "Has suggested tags" lists the pages still
waiting. A suggestion you confirm or dismiss is yours from then on; an export
shows suggestions but cannot decide them.

## The rest of the command line

```
knowmoretabs list                # snapshots, newest first
knowmoretabs export [DIR]        # the same library as a static site that opens from file://
                                 # --with-history adds searches and the pages you came from
knowmoretabs history             # how many library pages have History signals, and how current they are
knowmoretabs history --refresh   # read History now and update every library page's signals
knowmoretabs enrich              # fetch the <head> of library pages, without cookies; --dry-run, --limit N, --refetch
knowmoretabs forget <URL>...     # hide pages from the library; the snapshots keep them
knowmoretabs restore <URL>...    # bring them back
knowmoretabs tag <URL>... --add NAME --remove NAME   # tag pages, or untag them; both repeatable
knowmoretabs tags                # your tags, with how many pages carry each; --create, --retire, --all
knowmoretabs tags --define NAME TEXT   # what a tag means, for you and for a tagging agent
knowmoretabs tag --prompt DIR    # a work folder for an agent: prompt.md and the pages not yet tagged
knowmoretabs tag --import FILE   # check an agent's tags.jsonl and store it as suggestions; --dry-run
```

`tag --prompt` covers pages that show none of your tags and have no imported
answer yet; `--all` covers every page, for a second opinion or after changing
the vocabulary. Forgotten pages are never in it. `--with-history` adds the
searches and referrers History recorded. `tag --import` refuses a tag your
vocabulary lacks unless you pass `--accept-new`, and a prompt's page with no
answer unless you pass `--partial`; `--source NAME` names the model when the
file does not.

Options, all accepted before or after the command: `--root DIR` (where the
archive lives), `--browser NAME`, `--profile NAME` (a profile directory or its
display name), `--user-data-dir DIR` (a relocated browser user-data
directory), `--session FILE` (read this session file, skipping discovery),
`--json` (one machine-readable document on stdout), `-v` (source file,
statistics, error causes) and `-q`. `--help` on any command lists them.

With no browser flag, the supported browser with the newest session wins and
the others found are named. Arc is refused with an explanation: its open tabs
live in a different file, and reading its session log would snapshot the
wrong thing convincingly.

Exit status is 0 when a snapshot was saved or nothing needed saving, 1 on an
error, and 3 when the encrypted-sessions check refused to save.

## Page metadata, if you ask for it

```
knowmoretabs enrich --dry-run    # what would be fetched, what would not and why; sends nothing
knowmoretabs enrich --limit 30   # fetch at most 30 pages this run
knowmoretabs enrich              # fetch every page not fetched before
```

`enrich` is the only command that sends anything anywhere, and it runs only
when you run it. For each library page it has not fetched before, it asks the
page's own site for it, without cookies, and reads no further than `</head>`,
and never more than 3 MB. From the head it keeps the title, the description,
the `og:` and `twitter:` tags, the JSON-LD types, the language and the
canonical address. For a public GitHub repository's own page it also keeps
the repository's topics and the start of its README, from the same page: no
token, no account. `serve` and `export` do not show it yet; `tag --prompt`
hands it to your tagging agent.

Each attempt is appended to `pages/metadata.jsonl` as one line with its date,
and the newest line for a page is the one that counts. A page that was
attempted, including a failed fetch or a page behind a login, is not fetched
again unless you pass `--refetch`. Interrupting a run keeps every page it had
already recorded.

What a site sees is one request for the page's address from your IP address,
with a user agent that names knowmoretabs. There are no cookies, no referrer
and no proxy taken from your environment. At most one request a second goes
to any one site, and a few sites are fetched at once. HTTPS uses TLS built
into the binary with its own copy of the Mozilla root certificates, so a
certificate authority installed only on your system, such as a company's, is
not trusted.

What is never fetched:

- pages you have forgotten;
- this machine and the private network: addresses such as `192.168.1.1`,
  names such as `nas.local` or a bare `wiki`, and any name that resolves to
  such an address, checked again on every redirect;
- search results pages, whose query is already in the address;
- addresses whose query carries something that looks like a token or a
  password, because fetching a one-time link can spend it.

Sign-in, sign-up and verification screens are recorded as behind a login
without being fetched, and so is a page that redirects to one or that is only
a sign-in form. None of this hides a page from your library; forgetting it is
your call. `--dry-run` lists every page and the reason, and `--json` reports
the counts.

## Where the data lives

| Platform | Archive root |
|---|---|
| macOS, Linux | `~/.knowmoretabs` |
| Windows | `%LOCALAPPDATA%\knowmoretabs` |

```
~/.knowmoretabs/
├── snapshots/
│   └── 2026-09-20-084415Z/    # UTC, sorts as text, never rewritten
│       ├── snapshot.json      # the tabs, windows, groups and parse statistics
│       └── session.snss       # a verbatim copy of the browser's session file
├── pages/
│   ├── metadata.jsonl         # what `enrich` fetched, one line per attempt; append-only
│   └── history.json           # what History last said about each page; refreshed, never drops a page
├── library.json               # your own state: the forgotten URLs, your tags and their vocabulary
├── tags/
│   └── suggested.jsonl        # imported suggestions, one line per page per import; append-only
├── lock                       # held for the length of a run, so two can't collide
└── export/                    # what `export` writes by default; rebuildable
```

`--root DIR` puts it somewhere else. `snapshot.json` is pretty-printed JSON
with a `schema_version`; it is the source of truth and readable in any
editor. Everything under `export/` is derived and can be deleted.

Nothing leaves the machine unless you run `enrich`. `save`, `serve`,
`export` and the tag commands make no network requests of any kind; `enrich`
is the only code that does, and what it sends is described above. A
`tag --prompt` folder is the one thing made to be handed on: it holds the
addresses and titles of pages in your library, what `enrich` recorded about
them if you ran it, and their searches and referrers if you ask for them. It
is created private to you, and where it goes, and which agent and model
provider read it, is your choice.
`serve` binds `127.0.0.1` only, refuses any request whose `Host` is not
`127.0.0.1` or `localhost` with its own port, which is what stops a web page
reaching it through DNS rebinding, refuses any request carrying another
origin's `Origin` header, and
sends no CORS headers; the page's own Content-Security-Policy allows no
request to anywhere else. The exported site opens from `file://` with the
same policy.

The archive is a record of everything you browse, so it is created private
to you: mode `0700` on macOS and Linux. Windows has no such bit, which is why
the default root is under `%LOCALAPPDATA%`, the per-user folder whose
permissions a new directory inherits and which enterprise folder redirection
does not copy to a file server. A `--root` you point elsewhere on Windows
inherits whatever its parent grants; `knowmoretabs` warns when you do that.

Every snapshot is written to a temporary directory and renamed into place in
one step, under a lock, so an interrupted run leaves the archive exactly as
it was and two runs at once both succeed. A snapshot id that is already taken
gets a `-2` suffix rather than being overwritten. `library.json` and
`pages/history.json` are written the same way.

## What it reads, and what it tolerates

Chromium-family browsers keep their open windows in
`<profile>/Sessions/Session_<n>`, an append-only log that `knowmoretabs`
folds the way the browser's own session restore does. It reads and copies;
it never modifies a browser file, and it never touches the live browser.
The profile's `History` database is copied, with its journal or write-ahead
log, into a scratch directory inside the archive; only the copy is opened, and
it is deleted before `save` or `history --refresh` finishes. A skipped run
does not read it at all.

The parser never fails on a file the browser can read. A record it does not
recognise is skipped and counted; a half-written tail is counted and the
rest kept; a tab with no usable page is dropped and counted. The counters
are in `snapshot.json` under `stats`, and `save` prints one line when any of
them is non-zero. Only three things are fatal: the file does not exist,
cannot be read, or does not start with a recognised header.

Where it looks:

| Platform | Browser user data |
|---|---|
| macOS | `~/Library/Application Support/<product>` |
| Linux | `$XDG_CONFIG_HOME/<product>`, else `~/.config/<product>`; Snap and Flatpak installs in their own sandboxes |
| Windows | `%LOCALAPPDATA%\<product>\User Data` |

On Linux, Chrome's own `CHROME_CONFIG_HOME` and `CHROME_USER_DATA_DIR` are
honoured for Chrome and Chromium. If a browser was launched with its own
`--user-data-dir`, `chrome://version` shows the profile path; pass its
parent as `--user-data-dir`.

## Chrome is encrypting its session files

`knowmoretabs` reads the cleartext session file that Chromium browsers have
always written to disk. Chrome is moving that file to an encrypted format this
tool does not read, and will not: decrypting it means asking the operating
system for Chrome's keys, and the point of this tool is that it asks for
nothing. Today Chrome writes both formats, the cleartext one still carries
every tab, and `save` works as described above. At some future update Chrome
will stop writing the cleartext file. Google has published no date.

When that happens, `save` notices that the cleartext files are older than the
encrypted ones and refuses, with exit status 3, rather than reporting
months-old tabs as current. Nothing else changes. Every snapshot you have is
kept, `serve` and `export` keep working, and new snapshots stop until there
is a second way to see your tabs.

The second way is a browser extension. It is designed and not built; the
design is in
[`docs/research/native-messaging.md`](docs/research/native-messaging.md).
The two sources see different things:

- **A session file** is what the browser last flushed to disk. It lags the
  live screen by however long since that flush. It can be read after a crash
  and with the browser closed, the verbatim copy in each snapshot holds every
  tab's back-and-forward list, and it is the one with the clock on it.
- **An extension** sees the live tabs exactly as they are on screen, on
  Chrome, Brave, Edge, Vivaldi and Chromium, and keeps working after Chrome
  encrypts session storage. It also sees favicons, window state and which
  tabs are actually loaded. It sees only what is open at that moment: nothing
  from before a crash, nothing while the browser is closed, and one URL per
  tab.

An extension also changes who runs what. Stable Chrome has no supported way
for a command-line process to pull tabs out of the browser, so the extension
pushes snapshots on its own schedule, and `save`, run by hand or from cron,
cannot obtain live tabs. Installing it means a permission prompt that reads
"Read your browsing history", and a listing on the Chrome Web Store, with a
developer account, a fee and a review queue.

It is not built yet because of what it costs: the store listing and its
review round-trip, an install step for seven browsers on three platforms, and
a permission prompt on a tool whose pitch is that it asks for nothing. The
session file serves in the meantime, and `save` detects the moment it stops.
Why decrypting is not the answer is in
[`docs/research/encrypted-sessions.md`](docs/research/encrypted-sessions.md).

## Non-goals

No sync. No accounts. No cloud. No telemetry. No browser extension (for
now). No full-text indexing of page contents. No tag hierarchy. It never
touches, closes or reorders tabs in the live browser, and never modifies the
browser's own files: it reads and copies, nothing else.

## Deliberately not built

Each of these is a recorded decision, with its reasoning in
[`slices.toml`](slices.toml) and [`docs/SLICES.md`](docs/SLICES.md).

- **Live tabs, exactly as they are on screen.** Needs a browser extension,
  which sees what is open in the running browser at that moment and nothing
  from before a crash or while it is closed. Costed in the section above: a
  store listing, an install step per browser and platform, and a "Read your
  browsing history" prompt.
- **Arc.** Its open tabs live in a proprietary sidebar file, not in the
  session log.
- **Notes on a page.** `library.json` is where they would go, beside the
  forgotten URLs and the tags.
- **Searching page contents.** Means fetching and storing page bodies: a
  different product with a different privacy story.
- **An index for years of snapshots.** Reading JSON into memory is instant at
  fifteen thousand rows. A rebuildable index arrives when a measured query is
  slow.
- **Closing the tabs you have triaged.** Would make a read-only tool able to
  destroy what it archives.
- **An encrypted archive.** Key management means a way to lock yourself out
  of your own history; worth doing properly or not at all.

## Development

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask slices --check    # slice metadata lint and docs/SLICES.md
```

Every source file starts with a `slice:` / `why:` header naming the slice or
slices it belongs to and why it exists; `cargo xtask slices` enforces it.
`docs/BRIEF.md` is the reasoning behind the project, `docs/SLICES.md` the
build order, and `docs/briefs/` the brief each slice was built from.
Releases are built by `.github/workflows/release.yml` from a `v*` tag.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
