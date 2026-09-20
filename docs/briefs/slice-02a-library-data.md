# Slice 2a — The library, data side

**Ships:** `knowmoretabs list` and `knowmoretabs export`. Reading every snapshot
into one deduplicated library, and writing it out as a static site's data.

Slice 2 is split in two because the two halves want different skills. **This
brief is the Rust half.** Slice 2b takes the design prototype in `design-b/`
and makes it the production frontend, consuming exactly the JSON you produce
here. You are not writing any HTML, CSS or JavaScript.

## Required reading

1. `docs/BRIEF.md` — all of it.
2. `slices.toml` — slice `library`, and the `future` band for what not to build.
3. `docs/design-decision.md` — why design B won and what changes in it.
4. **`design-b/NOTES.md` §5 — the JSON contract. This is a contract, not a
   suggestion. It was designed against a realistic 2,000-page fixture and the
   frontend already consumes it.** `design-b/fixtures/library.json` is a
   worked example and `design-b/fixtures/generate.py` shows how it was built.
5. `src/model.rs` — what slice 1 writes, which is your input.
6. `docs/research/rust-stack.md` — the stack, already decided.

## In scope

- `knowmoretabs list` — snapshots newest first, human-readable; `--json` for
  the machine-readable form.
- `knowmoretabs export [DIR]` — default `<root>/export`. Writes the static site:
  the frontend assets plus the library data embedded into `index.html`.
- A library module that reads every `snapshot.json` under `<root>/snapshots/`,
  deduplicates pages by exact URL, and assembles the contract in §4 above.
- `library.json` at the archive root, holding forgotten URLs. Read it here and
  honour it in `export`; **the `forget`/`restore` commands that write it are
  slice 3** — do not build them.
- Local pages (`file://`, `localhost`, `127.0.0.1`) are excluded from the
  library. Slice 1's snapshots keep them; the library leaves them out.

## Out of scope — do not build

`serve`, any HTTP, any write path for `library.json`, `forget`, `restore`,
SQLite, any new browser or platform, tab-group *display* (parse them into the
contract, but the frontend is 2b's problem).

## The contract

Produce exactly what `design-b/NOTES.md` §5 specifies. Where slice 1's
`snapshot.json` carries something the contract has no place for, the contract
wins — do not extend it unilaterally. Two things need care:

- **Snapshots ascend by time; sightings derive from per-snapshot tab tuples.**
  This is what keeps the payload at ~487 KB rather than ~810 KB for 2,000
  pages. Do not "simplify" it into a sighting list per page.
- **Tab groups** are in the contract (`snapshots[].groups`, and the group slot
  in each tab tuple). Slice 1 captures them; carry them through.

Write a test that asserts your output validates against
`design-b/fixtures/library.json`'s shape, so that 2b's frontend cannot be
broken by a silent contract change.

## Things that will bite

- **A page's title changes between snapshots.** Use the most recent non-empty
  one. Say so in a comment; it is not obvious.
- **The same URL appears twice in one snapshot** — two windows, or two tabs in
  one window. Both are sightings of one page. Do not deduplicate them away.
- **Scale.** ~50 snapshots × ~300 tabs today. Read them all into memory and
  keep it simple — `slices.toml`'s `scale-index` row is the recorded decision
  that a database is not warranted. But do not be gratuitously quadratic:
  deduplicating 15,000 rows by URL is a hash map, not a nested loop.
- **A malformed or half-written `snapshot.json`** must not fail the whole
  library. Skip it, count it, report it — the same degradation discipline slice
  1 applies to session files.
- **Sorting.** Snapshot directory names sort lexicographically by design, but
  slice 1's review found that `-10` sorts before `-9`. There is already numeric
  suffix ordering in `src/archive.rs`; reuse it rather than writing a second one.

## Asset embedding

Per `docs/research/rust-stack.md` §6: `include_str!` with a debug-time
disk-reading path so the frontend can be edited without recompiling. The assets
live at `web/` — **2b creates that directory.** For now, point at
`design-b/`'s three files so you can build and test; 2b will move them and fix
the path. Leave a clear `slice: library` comment at that seam saying so.

## Tests

- Round-trip: build an archive of several synthetic snapshots, export, and
  assert the contract holds — counts, ordering, dedup, sightings.
- A URL seen in snapshots 1 and 3 but not 2.
- A URL seen twice in one snapshot.
- A title that changes; assert the newest non-empty wins.
- Forgotten URLs are absent from an export.
- Local URLs are absent from the library but present in the snapshot.
- A corrupt `snapshot.json` is skipped and counted, and the export still works.
- An empty archive exports something valid rather than crashing.
- `list` output, both human and `--json`.

**Manual, not committed:** run against the real archive if one exists, and
report counts only. No URLs or titles in the repo or in your report.

## Fences

- You own: `src/**`, `tests/**`, `Cargo.toml`, `Cargo.lock`, `README.md`.
- Do not touch: `design/**`, `design-b/**` (read only), `reference/**`,
  `docs/**` except adding nothing, `slices.toml`, `.github/**`, `xtask/**`.
- Every new source file carries its `slice:` / `why:` header, and
  `cargo xtask slices --check` must pass.
- **Run no git commands at all.** Version control is handled outside your run.

## Report

1. Files created or changed, with line counts.
2. Output of `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo xtask slices --check`.
3. The contract you produced, and any place you had to deviate from
   `design-b/NOTES.md` §5 — with the reason.
4. Judgment calls, especially anywhere the brief was silent or wrong.
5. What you could not verify, and what a reviewer should attack first.
