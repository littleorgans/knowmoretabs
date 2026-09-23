# knowmoretabs

Save your live tabs, and everything that becomes possible once they are saved.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` saves a dated
snapshot of every open window and tab, and gives you a local page where you
search everything you have ever had open, see what you keep reopening, and
forget what you do not want. It reads your browser's own session file from
disk to do it, so there is nothing to install in the browser. Nothing leaves
your machine.

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

`serve` is a page on `127.0.0.1` listing every page you have ever had open,
once, with search, a site filter, an open-or-not filter, five sort orders,
tab groups, and each page's history of the snapshots and windows it appeared
in. Select rows and forget them; undo from the toast; find them again under
"Forgotten". `/` focuses search, `j` and `k` move, `f` forgets. `--port N`
picks another port and `--open` opens your browser.

Forgetting hides a page from the library. It never touches a snapshot: every
page you forget is still in every snapshot it was ever in, and `restore`
brings it back.

## The rest of the command line

```
knowmoretabs list                # snapshots, newest first
knowmoretabs export [DIR]        # the same library as a static site that opens from file://
knowmoretabs forget <URL>...     # hide pages from the library; the snapshots keep them
knowmoretabs restore <URL>...    # bring them back
```

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
├── library.json               # your own state: the forgotten URLs
├── lock                       # held for the length of a run, so two can't collide
└── export/                    # what `export` writes by default; rebuildable
```

`--root DIR` puts it somewhere else. `snapshot.json` is pretty-printed JSON
with a `schema_version`; it is the source of truth and readable in any
editor. Everything under `export/` is derived and can be deleted.

Nothing leaves the machine. The tool makes no network requests of any kind.
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
gets a `-2` suffix rather than being overwritten. `library.json` is written
the same way.

## What it reads, and what it tolerates

Chromium-family browsers keep their open windows in
`<profile>/Sessions/Session_<n>`, an append-only log that `knowmoretabs`
folds the way the browser's own session restore does. It reads and copies;
it never modifies a browser file, and it never touches the live browser.

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
now). No full-text indexing of page contents. No tag taxonomy. It never
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
- **Notes and tags on a page.** `library.json` is where they would go;
  forgetting is the first use of it.
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
