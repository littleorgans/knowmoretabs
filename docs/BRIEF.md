# knowmoretabs — authoritative brief

> This file is the single source of truth for what we are building and why.
> Every agent working on this repo reads it first. If something here is wrong,
> say so in your report — do not silently deviate.

---

## 1. The product

### Problem

You have a hundred tabs open. They are a to-do list you cannot read, a memory
you cannot search, and one crash away from gone. Closing them feels like
throwing something away. Keeping them costs memory and attention.

### What knowmoretabs does

It reads your browser's own on-disk session files, saves a dated snapshot, and
builds a searchable library of every page you have ever had open. Snapshots are
immutable and local. You can always go back.

### The 80/20

Two things carry almost all the value:

1. `knowmoretabs save` — one command, snapshot taken, never overwrites anything.
2. `knowmoretabs serve` — a local page where you search everything you have ever
   had open, and forget what you do not want.

Everything else is polish. When a decision is close, pick the option that keeps
those two excellent.

### Non-goals — say these out loud in the README

No sync. No accounts. No cloud. No telemetry. No browser extension (for now).
No full-text indexing of page contents. No tag taxonomy. We never touch,
close, or reorder tabs in the live browser. We never modify the browser's own
files — we read and copy, nothing else.

---

## 2. Principles

1. **KISS, and the 80/20 rule on product design.** A feature that serves a fifth
   of the value for half the work does not ship. When in doubt, cut it and add a
   row to the "future" band of the slice matrix.
2. **The archive is sacred.** Never overwrite a snapshot. Never modify a browser
   file. Every write to the archive is atomic (temp + rename) and lock-guarded.
   A failure mid-run leaves the previous state exactly as it was.
3. **Forward compatibility over strictness.** Chrome changes its session format
   without notice. An unknown record is skipped and counted, never fatal. A torn
   tail is reported, never fatal. The reference implementation raises on both;
   this is the single biggest thing we improve.
4. **Plain files first.** JSON snapshots are the source of truth. Anything
   derived (indexes, exported HTML) is rebuildable and disposable. No database
   until a real query is actually slow — see "deliberately not built" below.
5. **No build step for the frontend.** Hand-written HTML, CSS and JS. No npm, no
   bundler, no framework. The same assets serve `file://` and `serve`.
6. **One binary, no runtime.** `knowmoretabs` is a single static-ish executable.
   The user installs nothing else.
7. **Exemplar layout.** A reader should be able to open any file and know from
   its first ten lines what it is for and which slice it belongs to. That is
   what the slice metadata (§6) is for.

### Deliberately not built

- **SQLite.** ~50 snapshots × ~300 tabs is ~15k rows. Loading JSON into memory
  is instant at that scale and costs zero migrations, zero corruption modes and
  zero dependencies. It is a "future" row until a measured query is slow.
- **Async.** A localhost single-user server does not need tokio. If the chosen
  HTTP crate is blocking, that is a feature.
- **A config file.** CLI flags plus sensible defaults. Add config only when a
  user has to repeat themselves.

---

## 3. The reference implementation

`reference/b_tabs_export.py` (538 lines, vendored read-only) and
`reference/test_b_tabs_export.py` (15 tests, all passing) are the prior art.
They are **inspiration, not a specification.** We are not chasing byte-identical
output. Read them for:

- the SNSS command IDs and payload layouts that are known to work on real data
- the navigation-pruning semantics (commands 5, 11, 24) which are subtle
- the "skip a snapshot whose layout is unchanged" behaviour, which is good
- the set of failure modes the tests pin down (torn file, no session, concurrent
  runs, same-second collisions, forget/restore)

Do **not** copy its structure. It is one file because it was a script.

**Known weaknesses to fix, in priority order:**

| # | Weakness | Fix |
|---|---|---|
| 1 | Raises `ValueError` on any session command ID not in its allowlist | Skip unknown commands, count them, surface the count |
| 2 | Raises on a truncated trailing command | Chrome's session file is an append log; a torn tail is normal. Parse what is whole, report the remainder |
| 3 | Raises if a tab's selected navigation index is missing | Degrade: keep the tab with its best-known navigation, or drop that one tab, never the whole run |
| 4 | Chrome-only, macOS-only, hardcoded path | Browser table + platform paths |
| 5 | Ignores tab-group commands | Tab groups are how people actually organise. Confirmed recoverable from `Session_*` alone: command 25 gives tab → group token, command 27 gives token → title, colour and collapsed state. Parse them in slice 1; display them in slice 2 |
| 6 | ~150 lines of HTML/CSS/JS inside Python string literals | Real `.html` / `.css` / `.js` files |
| 7 | "Forget" requires copying a shell command from the page | A real button |

### Test corpus — real data you can use

`~/Documents/chrome-tabs/*/session-backup/` holds **8 genuine Chrome session
files** (6 × `Session_*`, 2 × `Tabs_*` — and note the two use *different*
command-ID tables), 741 KB – 4.3 MB, alongside the
`tabs.json` the reference implementation produced from them (74–289 tabs, up to
12 windows). Use them as a fixture corpus: they are the only proof that a parser
works on real Chrome output rather than on hand-built bytes.

They are personal data. **Copy the parsed shapes, never the URLs, into the
repo.** Committed fixtures must be synthetic or redacted.

---

## 4. Commands (the whole CLI)

```
knowmoretabs                     # same as `save`
knowmoretabs save                # capture the newest session as a snapshot
knowmoretabs serve               # local web UI on 127.0.0.1
knowmoretabs export [DIR]        # write the static offline library
knowmoretabs list                # snapshots, newest first
knowmoretabs forget <URL>...     # hide pages from the library
knowmoretabs restore <URL>...    # bring them back
```

Global options: `--root <DIR>` (default `~/.knowmoretabs`), `--browser <NAME>`,
`--profile <NAME>`, `--session <FILE>`, `--json`, `-v/--verbose`, `-q/--quiet`.
`save` takes `--force`. `serve` takes `--port` and `--open`.

Output is for humans by default and machine-readable under `--json`. Errors go
to stderr with an actionable next step, never a bare panic or backtrace.

---

## 5. Storage layout

```
~/.knowmoretabs/
├── snapshots/
│   └── 2026-09-20-084415Z/          # UTC, sortable, no characters Windows rejects
│       ├── snapshot.json            # tabs + metadata — THE SOURCE OF TRUTH
│       └── session.snss             # verbatim copy of the browser's file
├── library.json                     # user state: forgotten URLs (and later, notes)
└── export/                          # generated static site — rebuildable, disposable
```

Rules:

- A snapshot directory, once renamed into place, is never written to again.
- Collisions within the same second get a `-2`, `-3` suffix. Never overwrite.
- Everything derived lives outside `snapshots/` so `rm -rf` on it is safe.
- The archive root is created `0700`. It contains a record of everything the
  user browses; treat it as sensitive.

### Data model (shape, not final Rust)

```
Snapshot  { id, captured_at, source, stats, tabs[] }
Source    { browser, profile, path, sha256, saved_at, bytes }
Stats     { commands, unknown_commands, tabs, windows, truncated_bytes }
Tab       { url, title, window, position, pinned, group?, tab_id }
Group     { id, title, colour }                    # if recoverable
Page      { url, title, domain, sightings[] }      # derived across snapshots
Sighting  { snapshot_id, window, tab_id }
```

`snapshot.json` carries a `schema_version`. Readers tolerate unknown fields.

---

## 6. Vertical slices and inline metadata

### The matrix

`slices.toml` at the repo root is the single source of truth. It declares the
capability rows (what someone wants), the slices that deliver them, and a
"future" band that is deliberately not built. `docs/SLICES.md` is **generated**
from it — never hand-edited.

Each slice is vertical: it cuts top to bottom through CLI → parse → archive →
render and ships standalone user value. The foundation (workspace, CI, lints)
rides along inside slice 1 rather than being a horizontal slice of its own.

### Inline metadata

Every non-trivial source file declares, in its module header, which slice it
belongs to and why it exists:

```rust
//! Reads a browser session file into a list of tabs.
//!
//! slice: capture
//! why: A snapshot is worthless if the parser dies on a record Chrome added
//!      last Tuesday. Unknown records are skipped and counted, never fatal.
```

This exists so that an agent — or a person — can answer "what is this file for,
and what is it part of" without reading the code, and can find every file in a
slice with `rg 'slice: triage'`.

### The custom lint

`cargo xtask slices` enforces it and fails CI:

- every `slice:` marker names a slice declared in `slices.toml`
- every source file that is not a test or generated carries a marker
- every marker has a non-empty `why:` that is not a restatement of the title
- every slice with `status = "done"` is claimed by at least one file
- `docs/SLICES.md` matches what `slices.toml` would generate (`--check`)

Keep the lint under ~300 lines. It is a helper, not a product.

---

## 7. Conventions

- **Rust.** Edition and MSRV pinned in `Cargo.toml` and in CI. `rustfmt` default
  profile. `clippy -D warnings` with `pedantic` on, and any allow justified by a
  comment at the point of the allow.
- **Comment density: low, and load-bearing.** Comments explain *why*, never
  *what*. If a comment restates the code, delete the comment. The slice headers
  are where the "why" lives.
- **Errors.** Library code returns typed errors. The binary turns them into a
  one-line human message plus, under `-v`, the chain. No `unwrap()` outside
  tests; no `panic!` on user input.
- **Tests.** Unit tests beside the code. Integration tests in `tests/` drive the
  real binary. Fixtures are synthetic or redacted, never personal data. A test
  that cannot fail is not a test — prefer tests that pin behaviour a future
  change could plausibly break.
- **Commits.** Conventional commits, one slice's worth of work per PR-sized
  branch. `main` stays green.
- **CI.** GitHub Actions on **Blacksmith runners: `blacksmith-2vcpu-ubuntu-2404`**
  (the GitHub-hosted credit pool is exhausted — do not use `ubuntu-latest`).
  Cache with the Blacksmith cache actions where available. macOS/Windows jobs
  wait until slice 5.
- **Licence.** Dual MIT OR Apache-2.0, the Rust default. Both files at root.
- **Repo.** `github.com/littleorgans/knowmoretabs`, public, default branch
  `main`. Remote `origin` is already configured over SSH.

---

## 8. Frontend

One set of hand-written assets serves both hosts:

- **Export mode** (`file://`): data is embedded in the page as
  `<script type="application/json">`. Read-only; the forget control explains
  how to do it from the CLI, or is hidden.
- **Serve mode** (`http://127.0.0.1`): the same assets, data fetched from a
  small JSON API, and the forget/restore controls are live.

The JS detects which mode it is in by looking for the embedded blob first and
falling back to `fetch`. One renderer, two hosts, no build step.

Design direction: calm, editorial, high information density, legible at 300+
rows. Light and dark via `color-scheme` and CSS custom properties. No remote
fonts, no remote images, no network requests of any kind — a `Content-Security-Policy`
meta tag should make that structural, not just a promise. Links carry
`rel="noopener noreferrer"` and the page sets `referrer: no-referrer`.
Keyboard-first: `/` focuses search, `j`/`k` move, `f` forgets. Accessible:
real semantics, visible focus, `aria-live` on result counts.

The reference implementation's HTML (inside `reference/b_tabs_export.py`, from
the `SHARED_STYLES` constant onward) is worth reading for the information
architecture it arrived at — snapshots view, pages view, per-page sighting
history — but its markup should not be carried over.

---

## 9. Working agreements for agents on this repo

- Read this file and `slices.toml` before you touch anything.
- **Stay inside the paths your brief names.** Another agent may be working in
  this same checkout. Never run `git add -A`, `git add .`, `git commit -a`,
  `git stash`, `git checkout .`, `git reset --hard`, or `git clean`. Stage by
  explicit path and run `git status --short` before committing.
- `reference/` is read-only. Never edit it.
- Never read, copy, or commit anything from `~/Documents/chrome-tabs` beyond
  what your brief explicitly authorises, and never commit real URLs or titles.
- Run only the tests your change can reach. The orchestrator runs the full gate.
- Report: files changed, each judgment call and why, command output as evidence,
  and anything you could not verify.
