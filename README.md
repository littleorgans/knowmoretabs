# knowmoretabs

Save your live tabs, and everything that becomes possible once they are saved.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` saves a dated
snapshot of every open window and tab, and gives you a local page where you
search everything you have ever had open, see what you keep reopening, and
forget what you do not want. It reads your browser's own session file from
disk to do it, so there is nothing to install in the browser. Nothing leaves
your machine unless you run `enrich` or `content`, which fetch pages you
already visited: `enrich` reads their `<head>`, `content` their main text.

It works with Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and
Vivaldi, on macOS, Linux and Windows.

## Install

With a Rust toolchain (1.89 or newer), one command builds it from the
repository and puts it on your `PATH`:

```
cargo install --git https://github.com/littleorgans/knowmoretabs knowmoretabs
```

Or clone the repository and run `cargo build --release`; the binary is
`target/release/knowmoretabs`, and it depends on nothing else. `content`
works without any other tool; it reads GitHub through `gh` when gh is
installed and signed in, and planned routes will let it use yt-dlp and
Chrome. `knowmoretabs doctor` says which of them it can use.

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
it back. `tags --define NAME TEXT` says what a tag means.
`tag URL --clear NAME` takes back your decision about a tag on a page, so it
is neither yours nor dismissed; a suggestion for it shows again.

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
knowmoretabs content             # keep the main text of library pages as markdown; --dry-run, --limit N, --refetch, --url URL
knowmoretabs doctor              # which ways of reading pages this machine can use; --live checks GitHub sign in and asks the X post API once
knowmoretabs forget <URL>...     # hide pages from the library; the snapshots keep them
knowmoretabs restore <URL>...    # bring them back
knowmoretabs tag <URL>... --add NAME --remove NAME --clear NAME   # tag pages, untag them, or take back a decision; all repeatable
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

`enrich` and `content` are the only commands that send anything anywhere,
and each runs only when you run it. For each library page `enrich` has not
fetched before, it asks the page's own site for it, without cookies, and
reads no further than `</head>`, and never more than 3 MB. From the head it
keeps the title, the description, the `og:` and `twitter:` tags, the JSON-LD
types, the language and the canonical address. For a public GitHub
repository's own page it also keeps the repository's topics and the start of
its README, from the same page: no token, no account. `serve` and `export` do
not show it yet; `tag --prompt` hands it to your tagging agent.

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

## Page text, if you ask for it

```
knowmoretabs content --dry-run   # what would be fetched, what would not and why; sends nothing
knowmoretabs content --limit 30  # capture at most 30 pages this run
knowmoretabs content             # capture every page not captured before, and retry the failed ones
knowmoretabs content --url URL   # capture only this library page; repeatable
knowmoretabs doctor              # which ways of reading pages are ready, and what the archive holds
```

Content capture is opt in: a command of its own that runs only when you run
it, never as part of `save`. It sends the addresses of pages you visited to
their own sites, to read each page's main text, and stores that text as
markdown, private to you: one file per page under `pages/content/`, named by
the SHA-256 of the page's address, with one line per attempt in
`pages/content.jsonl`. It reads most pages over plain HTTP, the same way
`enrich` does: without cookies, one request a second per site, a few sites
at once, and no more than 10 MB of a page. An X post (`x.com` or
`twitter.com`, `/<user>/status/<id>`) is read from the public X post API at
`api.fxtwitter.com` instead, the one third party service it uses: it sends
the post's number, not the page's address, without cookies, one request a
second. A GitHub repository, issue, pull request or discussion is read
through gh, GitHub's own command line tool, when it is installed and signed
in. Planned routes will use yt-dlp for video captions and Chrome for pages
that need a browser to show their text. Without these tools it does what it
can over plain HTTP. robots.txt is not consulted, as with `enrich`: every
address is one you opened yourself.

When article extraction misses a page's text, the fallback tries the whole
body, removing menus, banners, footers and sidebars outside `<main>`. It
keeps those elements inside `<main>` because they can contain page headings
and usage instructions.

What `enrich` never fetches, `content` never fetches either. Forgotten pages,
private network addresses, search results, URLs carrying tokens and pages that
are not web pages are counted in the report without being written to the
content store. It records X profiles and YouTube channels and playlists as not
a document, without a request, and anything that is not HTML as `not_html`
with its type. A page that has text is `ok`, or `thin` when it has less than
1,500 characters; either way the text is kept. A page drawn entirely by
scripts is `empty_shell`, a sign-in form or a 401 is `behind_login`, a 403 is
`blocked`, a 404 or 410 is `not_found`, and a short page whose publisher marks
it as not free is `paywalled`, keeping what it showed. A timeout, a 429 or an
HTTP 502, 503 or 504 is tried twice more in the same run, waiting 2 and then 8
seconds or as long as the site's `Retry-After` asks, up to a minute. A longer
`Retry-After` ends retries for that run. A site that answers 429 has its
request interval doubled, up to eight seconds, for the rest of the run. A page
still failing is `error` and is tried again on the next run; after three runs
it is `unavailable`. Every other outcome stands until you pass `--refetch`,
and a refetch that finds the same text leaves the file as it was.

From an X post it keeps the post's text, the post it quotes, an article's
body with its code and a link to each post it embeds, who wrote it and
when, and the descriptions of its images; one post, not the thread around
it. A post the API does not find, or one deleted or suspended, is
`not_found`; a private post or a protected account is `behind_login`; a
post the API reports as blocked is `blocked`; a post with no text at all is
`thin`. The API's 429 and 5xx answers are retried as a site's are. Two
addresses of the same post are fetched once.

A repository (`github.com/<owner>/<repo>`), issue (`/issues/<n>`), pull
request (`/pull/<n>`) or discussion (`/discussions/<n>`) is read with
`gh api`. gh holds your GitHub sign in; knowmoretabs never sees the token.
gh runs without a shell, at most four calls at a time, each stopped after
20 seconds. From a repository it keeps the description, the topics and the
README as its author wrote it. From an issue, pull request or discussion it
keeps the title, who opened it and when, its state, the opening post and
the first 10 comments, with the total, since a thread's opening says what it
is about; a discussion's accepted answer is kept too. Only public
repositories are kept: a private one is `behind_login`, even when gh could
read it. GitHub answers 404 alike for a page that does not exist and one
your account may not see, so either is `not_found`. A repository with no
README, or a thread with only a title, is `thin`. Rate limits and 5xx answers
are retried as a site's are; a gh that fails or times out is `error`, tried
again on the next run. Other GitHub pages, such as files, profiles and
settings, are read as web pages. Without gh, or with gh not signed in, GitHub
pages are read as web pages too, and the report says so, for example
`GitHub API: gh not found, used the web page for 3 pages`. A dry run sends
nothing, so it checks only that gh is installed.

`knowmoretabs doctor` says, for each way `content` reads pages, whether it
is ready, missing or degraded, and how to fix it: the web tier, which is
compiled in; GitHub through gh, found with its version; the X post API; yt-dlp
with deno or node for YouTube, and the `--browser` binary (Chrome by
default) for headless reading, both ahead of their routes. Then the archive:
whether it is private, whether `pages/content` exists, and its pages by
latest status. It is offline by default, with GitHub sign in marked as not
checked. With `--live`, gh checks its own sign in (the `gh auth status` exit
code only; its output is never read), and the X post API is asked once for
a fixed public post. It sends nothing of yours. It exits 0 whenever the web tier is ready; a missing tool is a warning.

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
│   ├── content.jsonl          # what `content` captured, one line per attempt; append-only
│   ├── content/               # one markdown file per captured page, named by the SHA-256 of its address
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

Nothing leaves the machine unless you run `enrich` or `content`. `save`,
`serve`, `export` and the tag commands make no network requests of any kind;
`enrich` and `content` are the only code that sends your pages' addresses,
and what each sends is described above. `doctor` is offline by default.
`doctor --live` lets gh check its own sign in with GitHub and asks the X
post API for one fixed public post, sending nothing of yours. A
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
gets a `-2` suffix rather than being overwritten. `library.json`,
`pages/history.json` and the files under `pages/content/` are written the
same way.

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
now). No page text unless you ask for it: `content` is opt in, and keyword
search over what it stores comes later. No tag hierarchy. It never touches,
closes or reorders tabs in the live browser, and never modifies the
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
- **Searching page contents.** Keyword search over the page text that
  `content` stores. It waits for that store, and for a
  measured query to say what the index should be.
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
`slices.toml` is the reasoning behind what is built and what is not,
`docs/SLICES.md` the build order generated from it, and `docs/briefs/` the
brief each slice was built from. Those briefs cite `docs/BRIEF.md`, the
original project brief, retired after commit 2256595;
`git show 2256595:docs/BRIEF.md` reads it. Releases are built by
`.github/workflows/release.yml` from a `v*` tag.

`save` and `serve` carry almost all the value; when a decision is close, pick
the option that keeps those two excellent. A feature that serves a fifth of the
value for half the work does not ship: cut it and record it in the `future`
band of `slices.toml`. Beyond that:

- The archive is sacred. A browser file is never modified. A snapshot is
  published whole by one rename and never written again, and `library.json`
  and `pages/history.json` are replaced whole, so a failure mid-write leaves
  the previous file as it was. `pages/metadata.jsonl` and
  `pages/content.jsonl` are append-only instead: an interrupted `enrich` or
  `content` keeps every record it completed.
- Plain files first. JSON snapshots are the source of truth and anything
  derived is rebuildable. No database until a measured query is slow; no
  async, since a single-user localhost server does not need it; no config
  file until a user has to repeat themselves.
- No build step for the frontend: hand-written HTML, CSS and JS, the same
  assets for `file://` and `serve`.
- One binary, no runtime. Optional local tools may widen what an opt-in
  command can reach (gh for content capture; planned: yt-dlp and Chrome), never
  what the binary needs to run.
- Comments explain why, never what. Library code returns typed errors; no
  `unwrap()` outside tests and no `panic!` on user input. A clippy `allow` is
  justified by a comment where it is written.
- Unit tests sit beside the code; integration tests in `tests/` drive the real
  binary. Fixtures are synthetic or redacted: never commit a real URL or title
  from anyone's browser.
- `reference/` is the vendored prior art, a Python script and its tests:
  inspiration, not a specification, and read-only.
- Commits are conventional commits; `main` stays green.

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
