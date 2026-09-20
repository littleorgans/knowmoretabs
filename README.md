# knowmoretabs

Every tab you ever had open, kept and searchable.

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. `knowmoretabs` reads Chrome's
own session file from disk, saves a dated snapshot of every open window and
tab, and never overwrites an earlier one. Later, a local page lets you search
everything you have ever had open.

## The two commands that matter

```
knowmoretabs save    # snapshot what is open right now (also the default)
knowmoretabs serve   # search everything you have ever had open (not built yet)
```

Today only `save` exists. It is the tracer bullet: the data model and the
durability guarantees it establishes are what every later feature reads.

```
$ knowmoretabs
saved 113 tabs across 12 windows, 3 groups to /Users/you/.knowmoretabs/snapshots/2026-09-20-084415Z

$ knowmoretabs
no change since 2026-09-20-084415Z: 113 tabs across 12 windows, 3 groups. Nothing saved; use --force to save anyway.
```

Flags: `--root DIR` (where the archive lives), `--session FILE` (read a
specific session file), `--profile NAME` (a Chrome profile directory or its
display name), `--user-data-dir DIR` (a relocated Chrome), `--force`, `--json`,
`-v`, `-q`. `--help` lists them all.

Exit status is 0 when a snapshot was saved or nothing needed saving, 1 on an
error, and 3 when the encrypted-sessions check below refuses to save.

## Install

For now, build from source with Rust 1.89 or newer:

```
cargo install --path .
```

Prebuilt binaries are a later slice.

## Where the data lives

```
~/.knowmoretabs/
└── snapshots/
    └── 2026-09-20-084415Z/    # UTC, sorts as text, never rewritten
        ├── snapshot.json      # the tabs, windows, groups and parse statistics
        └── session.snss       # a verbatim copy of Chrome's session file
```

The root is created with mode `0700` because it is a record of everything you
browse. `snapshot.json` is pretty-printed JSON with a `schema_version`; it is
the source of truth and readable in any editor. Each snapshot is written to a
temporary directory and renamed into place in one step, so an interrupted run
leaves the archive exactly as it was.

A run whose window, tab and URL layout matches the newest snapshot for the
same profile saves nothing. Titles and timestamps do not count as change.

## What it reads

Chrome keeps its open windows in `<profile>/Sessions/Session_<n>`, an
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

## Non-goals

No sync. No accounts. No cloud. No telemetry. No browser extension (for now).
No full-text indexing of page contents. No tag taxonomy. It never touches,
closes or reorders tabs in the live browser, and never modifies the browser's
own files: it reads and copies, nothing else.

## Scope today

Chrome on macOS only. Other Chromium browsers and other platforms are later
slices; see `docs/SLICES.md` for the build order and `docs/BRIEF.md` for the
reasoning.

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
