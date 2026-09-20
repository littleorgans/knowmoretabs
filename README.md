# knowmoretabs

Every tab you ever had open, kept and searchable.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` reads your
browser's own session file from disk, saves a dated snapshot of every open
window and tab, and gives you a local page where you can search everything
you have ever had open. Nothing leaves your machine.

It works with Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and
Vivaldi, on macOS, Linux and Windows.

## Before you depend on it

Chrome is moving its session files to an encrypted format that this tool
cannot read. Today Chrome writes both the cleartext files `knowmoretabs`
reads and the encrypted ones; at some future update it will stop writing the
cleartext copy. When that happens, `knowmoretabs save` notices that the
cleartext files have gone stale and **refuses to save**, with exit status 3,
rather than reporting months-old tabs as current. Your archive stays intact
and searchable; new snapshots stop until there is a second way to see your
tabs. Google has not published a date. The detail, and why decrypting is the
wrong answer for a tool that asks for no browser secrets, is in
[`docs/research/encrypted-sessions.md`](docs/research/encrypted-sessions.md).

## Install

Prebuilt binaries for each tagged release are on the
[Releases](https://github.com/littleorgans/knowmoretabs/releases) page:

| Platform | Archive |
|---|---|
| macOS, Apple silicon | `knowmoretabs-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `knowmoretabs-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, x86-64 | `knowmoretabs-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux, ARM64 | `knowmoretabs-<version>-aarch64-unknown-linux-gnu.tar.gz` |
| Windows, x86-64 | `knowmoretabs-<version>-x86_64-pc-windows-msvc.zip` |

Unpack it and put the `knowmoretabs` binary somewhere on your `PATH`. Each
archive also carries this README, the changelog and both licence files, and
`SHA256SUMS` on the release page lets you check what you downloaded:

```
sha256sum -c --ignore-missing SHA256SUMS
```

The binaries are not code-signed. macOS may refuse to open one that arrived
through a browser; either download with `curl`, which sets no quarantine
flag, or clear it with `xattr -d com.apple.quarantine knowmoretabs`. The
Linux binaries are built on Ubuntu 24.04 against its glibc; if one refuses
to start on an older distribution, build from source instead.

If you already have a Rust toolchain (1.89 or newer):

```
cargo install knowmoretabs                                        # from crates.io
cargo install --git https://github.com/littleorgans/knowmoretabs  # from the repository
```

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
collapsed) and last-active time. A run whose window, tab and URL layout
matches the newest snapshot saves nothing; `--force` saves anyway.

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

## Non-goals

No sync. No accounts. No cloud. No telemetry. No browser extension (for
now). No full-text indexing of page contents. No tag taxonomy. It never
touches, closes or reorders tabs in the live browser, and never modifies the
browser's own files: it reads and copies, nothing else.

## Deliberately not built

Each of these is a recorded decision, with its reasoning in
[`slices.toml`](slices.toml) and [`docs/SLICES.md`](docs/SLICES.md).

- **Live tabs, not the last session written to disk.** Needs a browser
  extension, and an extension cannot recover anything after a crash. It
  belongs as a second source beside session files, and it is also the real
  answer to the encrypted-sessions clock above.
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

## Licence

MIT or Apache-2.0, at your option.
