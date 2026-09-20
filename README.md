# knowmoretabs

Every tab you ever had open, kept and searchable.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` reads a
Chromium-family browser's own session file from disk, saves a dated snapshot
of every open window and tab, and never overwrites an earlier one. It supports
Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and Vivaldi, on
macOS, Linux and Windows. Later, a local page lets you search everything you
have ever had open.

## The two commands that matter

```
knowmoretabs save    # snapshot what is open right now (also the default)
knowmoretabs serve   # search everything you have ever had open, and forget what you don't want
```

The rest of the CLI:

```
knowmoretabs list                # snapshots, newest first
knowmoretabs export [DIR]        # the same library as a static site that opens from file://
knowmoretabs forget <URL>...     # hide pages from the library; the snapshots keep them
knowmoretabs restore <URL>...    # bring them back
```

```
$ knowmoretabs
saved 113 tabs across 12 windows, 3 groups to /Users/you/.knowmoretabs/snapshots/2026-09-20-084415Z

$ knowmoretabs
no change since 2026-09-20-084415Z: 113 tabs across 12 windows, 3 groups. Nothing saved; use --force to save anyway.
```

Flags: `--root DIR` (where the archive lives), `--session FILE` (read a
specific session file), `--browser NAME`, `--profile NAME` (a browser profile
directory or its display name), `--user-data-dir DIR` (a relocated Chromium
user-data directory), `--force`, `--json`, `-v`, `-q`. With no browser flag,
the newest supported `Session_*` file wins and the command names the other
browsers it found. Arc is excluded because its open tabs live in a different
file format. `--help` lists them all.

Exit status is 0 when a snapshot was saved or nothing needed saving, 1 on an
error, and 3 when the encrypted-sessions check below refuses to save.

## The library, served

```
$ knowmoretabs serve
Your library is at http://127.0.0.1:7878/
Local only: it answers this machine and nothing else. Press Ctrl-C to stop.
```

`serve` runs the same page `export` writes, with the data fetched from a
small JSON API and the Forget and Restore buttons live: select rows, forget
them, undo from the toast, and find them again under "Forgotten". `--port N`
picks another port; `--open` opens your browser, through `open` on macOS,
`xdg-open` on Linux and `start` on Windows. If none of them is there — a
headless Linux box has no `xdg-open` — `serve` says so and keeps serving.

Forgetting hides a page from the library. It is a filter, recorded in
`library.json`, and it never touches a snapshot: every page you forget is
still in every snapshot it was ever in, and `restore` brings it back.
`forget` and `restore` from the terminal do exactly what the buttons do.

The server is deliberately unreachable from anywhere but this machine, and
from anywhere but its own page. It binds `127.0.0.1` only; it refuses any
request whose `Host` header is not `127.0.0.1:<port>` or `localhost:<port>`,
which is what stops a web page from reaching it through DNS rebinding; it
refuses any request that carries another origin's `Origin` header; it sends
no CORS headers; and the page's own Content-Security-Policy allows no request
to anywhere else. There is no authentication because there is nobody to
authenticate: the only client that can reach it is a browser on your own
machine, and the only page that can read from it is its own.

## Install

For now, build from source with Rust 1.89 or newer:

```
cargo install --path .
```

Prebuilt binaries are a later slice.

## Where the data lives

```
~/.knowmoretabs/                 # %LOCALAPPDATA%\knowmoretabs on Windows
├── snapshots/
│   └── 2026-09-20-084415Z/    # UTC, sorts as text, never rewritten
│       ├── snapshot.json      # the tabs, windows, groups and parse statistics
│       └── session.snss       # a verbatim copy of Chrome's session file
├── library.json               # your own state: the forgotten URLs
└── export/                    # what `export` writes by default; rebuildable
```

`--root DIR` puts it somewhere else.

The archive is a record of everything you browse, so it is created private to
you. On macOS and Linux that is mode `0700`, set when the directory is made.
Windows has no such bit, and setting an access-control list needs Win32 calls
this tool does not make, so on Windows the protection is inherited instead:
`%LOCALAPPDATA%` grants you, SYSTEM and Administrators and nobody else, and a
directory created inside it inherits exactly that. **This is why the default
root on Windows is `%LOCALAPPDATA%\knowmoretabs` rather than a dotfile in your
profile** — the profile directory is what enterprise folder redirection roams
to a file server, and an archive of your browsing is the last thing that
should be copied off the machine. The consequence is worth knowing: a
`--root` you point somewhere else on Windows is only as private as wherever
you put it, and a directory under `C:\` is readable by every local user. On
macOS and Linux `--root` is `0700` wherever it is.

`snapshot.json` is pretty-printed JSON with a `schema_version`; it is the
source of truth and readable in any editor. Each snapshot is written to a
temporary directory and renamed into place in one step, so an interrupted run
leaves the archive exactly as it was — on all three platforms. The rename
never replaces anything: the destination is checked under the archive lock
and a snapshot id that is already taken gets a `-2` suffix instead. That
matters because renaming a directory onto an existing one is the one
filesystem operation whose meaning differs between POSIX and Windows, and
publication does not depend on it.

A run whose window, tab and URL layout matches the newest snapshot for the same
browser and profile saves nothing. Titles and timestamps do not count as change.

`library.json` is written the same way a snapshot is, staged and renamed in
one step under the archive lock, so a forget that is interrupted, or two
that run at once, cannot lose anything. A damaged `library.json` stops
`export`, `serve`, `forget` and `restore` with a message naming the file
rather than quietly showing pages you had hidden.

## What it reads

Chromium-family browsers keep their open windows in
`<profile>/Sessions/Session_<n>`, an
append-only log of commands. `knowmoretabs` reads the newest one, copies it
verbatim, and folds it the way Chrome's own session restore does. It records
each tab's URL, title, window, position, pinned state, group (name, colour,
collapsed) and last-active time.

The parser never fails on a file Chrome can read. A command it does not
recognise is skipped and counted; a half-written tail is counted and the
rest kept; a tab with no usable navigation is dropped and counted. Every one
of those counters is in `snapshot.json` under `stats`, and `save` prints one
line on stdout when any of them is non-zero. Only three things are fatal: the
file does not exist, cannot be read, or does not start with a recognised
header.

## The encrypted-sessions clock

Chrome is moving its session files to an encrypted format. Today it writes
both `Sessions/` (cleartext, which this tool reads) and `Sessions_Encrypted/`
(which it cannot). At some future update Chrome will stop writing the
cleartext copy. When that happens the cleartext file does not disappear at
once; it goes stale.

To avoid archiving old tabs as current, `save` compares the modification
times of the two directories before reading anything. If the encrypted files
are five minutes or more newer than the cleartext ones, or the cleartext
directory has emptied while the encrypted one has files, it refuses with a
clear message and exit status 3. `--force` does not override this. There is
no workaround yet; progress is tracked in the issue tracker.

The check runs for every browser the no-flag scan considers. A browser whose
cleartext has gone stale is ranked by its encrypted files, so if it is the one
used most recently the run refuses rather than quietly saving another browser
in its place; if another browser is newer, that one is saved and the stale
browser is listed under "also found" with a note saying it would be refused.

## Non-goals

No sync. No accounts. No cloud. No telemetry. No browser extension (for now).
No full-text indexing of page contents. No tag taxonomy. It never touches,
closes or reorders tabs in the live browser, and never modifies the browser's
own files: it reads and copies, nothing else.

## Where it looks for browsers

One table, one row per browser, one column per platform.

| Platform | Where a browser's user data is |
|---|---|
| macOS | `~/Library/Application Support/<product>` |
| Linux | `$XDG_CONFIG_HOME/<product>`, else `~/.config/<product>` |
| Windows | `%LOCALAPPDATA%\<product>\User Data` |

On Linux a native, a Snap and a Flatpak install of the same browser are three
separate installs with three separate user-data directories, and a machine can
have two of them. All of them are probed — `~/snap/<name>/common/…`,
`~/.var/app/<flatpak id>/config/…` — and the newest session wins, the same
rule that picks between browsers. Snap and Flatpak hard-code their own config
roots inside the sandbox, so `$XDG_CONFIG_HOME` moves the native path and
leaves those alone.

`$CHROME_CONFIG_HOME` replaces `~/.config` for Chrome and Chromium, and
`$CHROME_USER_DATA_DIR` names a whole user-data directory for them. Both are
Chrome's own Linux variables, and both are honoured for the Chrome family
only: someone who set one for Chrome Remote Desktop should not find every
browser reported at the same directory. `%LOCALAPPDATA%` and `%APPDATA%` are
read from the environment, falling back to the `FOLDERID_LocalAppData` and
`FOLDERID_RoamingAppData` known folders when they are not set.

`--user-data-dir DIR` overrides all of it, and `--session FILE` skips
discovery entirely. If a browser was launched with its own `--user-data-dir`,
`chrome://version` → Profile Path names where it went; its parent is the
directory to pass.

Windows rejects file names the other two accept. A `--root` containing a
reserved device name (`CON`, `NUL`, `COM1`…), a component ending in a dot or
a space, or a character Windows forbids is refused by name on Windows before
anything is created, rather than failing later as something unrecognisable.
Long roots are fine: the paths the archive writes can pass the classic
260-character limit, because Rust's standard library switches to the `\\?\`
form beyond it.

## Scope today

`save` supports the browsers above on macOS, Linux and Windows. The library
commands work anywhere the archive is. See `docs/SLICES.md` for the build
order and `docs/BRIEF.md` for the reasoning.

## Development

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask slices --check    # slice metadata lint and docs/SLICES.md
```

Every source file starts with a `slice:` / `why:` header naming the slice it
belongs to and why it exists. `cargo xtask slices` enforces that.

## Licence

MIT or Apache-2.0, at your option.
