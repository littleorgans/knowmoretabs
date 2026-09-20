# Slice 1 — Capture

**Ships:** `knowmoretabs save` writes an immutable dated snapshot of the newest
Chrome session on macOS. Plus the project foundation, because slice 1 is where
it earns its place rather than being a horizontal slice of its own.

## Required reading, in order

1. `docs/BRIEF.md` — the product, the principles, the conventions. All of it.
2. `slices.toml` — slice 1 is what you are building; the `future` band is what
   you are not.
3. `docs/research/rust-stack.md` — **the stack is decided.** Implement it. If
   you disagree with something, implement it anyway and say so in your report.
   Do not re-litigate in code.
4. `docs/research/session-format.md` — the parser specification. §7 and the
   command-ID table are the heart of it.
5. `docs/research/encrypted-sessions.md` — §6, the staleness check you must ship.
6. `docs/research/browsers.md` — §3 profile discovery. You need only macOS
   Chrome for this slice, but read it so the shape you build has room for the rest.
7. `reference/b_tabs_export.py` — prior art, read-only, **inspiration not spec**.
   Its pruning arithmetic for commands 5/11/24 was verified correct; borrow the
   logic, not the structure. Everything else in it, treat with suspicion.

## In scope

- `knowmoretabs save` (also the default with no subcommand) and `--version`/`--help`.
- Flags: `--root`, `--session`, `--profile`, `--force`, `--json`, `-v`, `-q`.
- Newest-session discovery for Chrome on macOS via `Local State`.
- The SNSS parser.
- Snapshot writing: atomic, locked, never overwriting.
- The unchanged-session skip.
- The encrypted-sessions staleness check.
- Cargo workspace, lint config, `xtask slices`, CI on Blacksmith, README.

## Out of scope — do not build these

`serve`, `export`, `list`, `forget`, `restore`, any HTML or CSS or JS, SQLite,
any browser other than Chrome, any OS other than macOS, any config file, any
async runtime. `design/` and `design-b/` are other agents' work: do not read
them, do not touch them, do not wire anything to them.

---

## The parser — where the value is

The reference implementation raises `ValueError` on anything it does not
recognise. That is the single biggest weakness we are fixing. **A session file
that Chrome itself can read must never make us fail.**

### Rules

1. **Unknown command ID → skip, count, continue.** Never fatal. The count goes
   into the snapshot's `stats.unknown_commands` and is shown under `-v`.
2. **Torn tail → stop, count the remaining bytes, keep everything parsed so
   far.** These files are append logs and Chrome may be mid-write. Record
   `stats.truncated_bytes`. Not fatal.
3. **A tab with no navigation at its selected index → do what Chromium does:**
   take the nearest navigation with `index >= selected`, else the last one. If
   there is genuinely nothing, drop that one tab and count it. Never fail the run.
4. **A tab with incomplete window metadata → drop that tab, count it.** Not fatal.
5. **Genuinely fatal, and this should be the whole list:** the file does not
   exist, is not readable, or does not start with a recognised SNSS magic and
   version. Anything else degrades.

`stats` is how we stay honest about degradation: `commands`, `unknown_commands`,
`dropped_tabs`, `truncated_bytes`, `tabs`, `windows`. If any of the degradation
counters is non-zero, `save` says so on stdout in one line.

### Commands to handle

Per `docs/research/session-format.md`. At minimum: 0 SetTabWindow, 2
SetTabIndexInWindow, 5/11/24 the navigation-pruning trio, 6 UpdateTabNavigation,
7 SetSelectedNavigationIndex, 8 SetSelectedTabInIndex, 9 SetWindowType, 12
SetPinnedState, 16 TabClosed, 17 WindowClosed, 255 the marker.

Also parse, because they are cheap and they are product:

- **21 LastActiveTime** → `Tab.last_active`. This is what tells someone a tab
  has been sitting untouched since March, which is the single most useful fact
  for deciding what to close. Nothing displays it until slice 2; capture it now.
- **25 SetTabGroup** and **27 SetTabGroupMetadata2** → tab groups with title,
  colour and collapsed state. Groups are how people actually organise a hundred
  tabs. Model them properly: a snapshot has a `groups` table and a tab has an
  optional group reference.

Note that `Tabs_*` files use a **different command-ID table** from `Session_*`.
This slice reads `Session_*` only — but make that a explicit, named decision in
the code, not an accident, so slice 4 can add the other table cleanly.

### The staleness check

Per `docs/research/encrypted-sessions.md` §6. Implement exactly the comparison
it specifies. When it trips, `save` writes nothing and exits non-zero.

Use the message from that document, but **drop any clause offering a
"live-tabs/DevTools fallback"** — we do not have one, and promising a user a
feature that does not exist is worse than the staleness itself. Point them at
the issue tracker instead.

---

## Storage

Per `docs/BRIEF.md` §5. Specifics for you to get right:

- Snapshot directory names are UTC and sort lexicographically:
  `2026-09-20-084415Z`. Same-second collisions get `-2`, `-3`.
- Write the whole snapshot into a temporary directory inside the root, then a
  single `rename` into place. A kill at any moment leaves either the old state
  or the new one, never a half-snapshot. Clean up stale temporaries on startup.
- An exclusive advisory lock on the root serialises concurrent runs. Two
  `knowmoretabs save` processes must both succeed and produce two snapshots —
  the reference has a test for exactly this; it is a good test.
- The root is created `0700`.
- `snapshot.json` carries `schema_version: 1`. Pretty-printed — people will read
  it in an editor in five years and that matters more than bytes.
- The session file is copied verbatim, and the copy is verified stable: if the
  file changed underneath you mid-read, retry, then fail cleanly. The reference's
  `copy_stable_session` is the right idea.
- **Never write to, modify, or lock anything inside the browser's own directory.**

## The unchanged-session skip

If the new session's window/tab/URL layout matches the most recent snapshot for
the same profile, save nothing and say so. `--force` overrides. Compare the
layout, not the bytes — Chrome rewrites these files constantly with no
user-visible change.

---

## Foundation

- `Cargo.toml` exactly as `docs/research/rust-stack.md` specifies, including the
  `[lints]` table. `cargo clippy` must be clean at the configured level.
- `xtask` crate implementing `cargo xtask slices`, enforcing the five rules in
  `docs/BRIEF.md` §6, and generating `docs/SLICES.md`. Keep it under ~300 lines;
  it is a helper, not a product. It must fail on a bad marker and pass on this
  repo.
- Every source file you write carries its `slice:` / `why:` header. Make the
  `why` genuinely informative — "why this file exists at all", not a restatement
  of the filename. That header is the feature; write it like you mean it.
- `.github/workflows/ci.yml` on `blacksmith-2vcpu-ubuntu-2404`. **Never
  `ubuntu-latest`** — the GitHub-hosted credit pool is exhausted and a workflow
  using it simply will not run. Jobs: fmt, clippy, test, `xtask slices`.
- `README.md`: what it is, the two commands that matter, install, where data
  lives, the non-goals from `docs/BRIEF.md` §1, and an honest note about the
  encrypted-sessions clock. Write it for someone who has never seen the project.
  No badges for CI that has never run, no roadmap fiction.

## Tests

Per `docs/research/rust-stack.md` §10. What must be covered:

- A synthetic session builder in test code — you need one, because the
  pruning commands (5, 11, 24) **never occur in the real corpus** and are the
  subtlest logic in the parser.
- Every degradation path in "Rules" above, each asserting that the run
  *succeeds* and the counter is right.
- Unknown command IDs, including one in the middle of a valid file.
- A torn tail at several truncation points, including mid-length-prefix.
- Tab groups: membership, title, colour, and a tab in no group.
- Atomicity: kill between staging and rename, assert the archive is untouched.
- Two concurrent saves both succeed.
- Same-second collision produces `-2`.
- The staleness check, both trips and does not trip.
- The unchanged-session skip, and `--force` overriding it.

**Real-corpus check, manual, not committed as a test:** 8 genuine session files
live at `~/Documents/chrome-tabs/*/session-backup/`. Run your parser over all of
them and report tab counts, group counts, and every degradation counter. The
sibling `tabs.json` holds what the reference produced — a rough cross-check, not
a target; we are not chasing byte-identical output and you should expect to find
*more* than it did. **This is personal data: no URLs, titles or other content in
the repo, in a test, in a fixture, or in your report. Counts only.**

---

## Fences

- Paths you own: `Cargo.toml`, `Cargo.lock`, `src/**`, `tests/**`, `xtask/**`,
  `.github/**`, `README.md`, `docs/SLICES.md` (generated only).
- Paths you must not touch: `docs/BRIEF.md`, `slices.toml`, `docs/research/**`,
  `docs/briefs/**`, `reference/**`, `design/**`, `design-b/**`.
- **Run no git commands at all.** Not `add`, not `commit`, not `status`, not
  `diff`. Another agent is working in this checkout and the orchestrator handles
  version control. This is absolute.
- If something in the research documents is wrong or contradictory, implement
  your best reading and **say so in your report**. Do not edit the research.

## Report

1. Files created, with line counts.
2. Output of `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo xtask slices`.
3. The real-corpus run: per-file tab count, group count, unknown-command count,
   dropped-tab count, truncated bytes. Counts only.
4. Every judgment call you made and why — especially anywhere the research
   documents were silent, wrong, or disagreed with each other.
5. What you could not verify, and what you would want a reviewer to attack first.
