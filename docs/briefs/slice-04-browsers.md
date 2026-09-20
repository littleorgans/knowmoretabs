# Slice 4 — The browser you actually use

**Ships:** Brave, Edge, Chromium and Vivaldi alongside Chrome, with profile
discovery and a sensible answer when several are installed.

This slice should be mostly a **data table**, not an investigation. That is
deliberate: `docs/research/browsers.md` did the investigation up front so that
adding a browser is adding a row. If you find yourself writing per-browser
logic, stop and ask whether the table is missing a column instead.

## Required reading

1. **`docs/research/browsers.md`** — the whole thing. It is the specification
   for this slice: the path matrix, profile discovery, the safety rules, and a
   recommended zero-flag behaviour. It was verified against this machine.
2. `docs/BRIEF.md` §2 and §5.
3. `src/platform.rs` — the existing Chrome-on-macOS discovery. Slice 1 was
   told to leave room for exactly this; check whether it did, and say so.
4. `docs/research/session-format.md` §5 — `Session_*` versus `Tabs_*`, which
   matters because the forks differ in what they leave lying around.

## In scope

- Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge, Vivaldi. **macOS
  paths only** — Linux and Windows are slice 5. Structure the table so slice 5
  adds a column rather than rewriting it.
- `--browser <name>` selecting one explicitly, with a helpful error naming
  what is actually installed when the name is unknown.
- Profile discovery per `browsers.md` §3: accept directory names (`Default`,
  `Profile 1`) **and** display names (`Work`) from `profile.info_cache`.
  `profile.last_used` is frequently absent — the research found it missing on
  Brave, Edge and Vivaldi on this very machine — so fall back to
  `last_active_profiles[0]`, then `Default`.
- **Zero-flag behaviour**, per `browsers.md` §5: scan every supported browser,
  pick the newest `Session_*`, and always name the winner. Do not prefer
  Chrome, and do not require a flag. The research argued this and I agree: the
  slice is called "the browser you actually use", and a static Chrome-first
  list gets that wrong for anyone who left Chrome.
- The "also found" report when several browsers have sessions. `browsers.md`
  §5 drafted the output; it is good, use it.
- `browser` in `snapshot.json` already exists and is honest today. Keep it so.

## Out of scope

**Arc.** `slices.toml` has it in the `future` band with the reason: it writes
only `Tabs_*`, and its real open-tab model lives in a proprietary
`StorableSidebar.json`. Parsing its session files would confidently snapshot
the wrong thing. Do not add it. If `--browser arc` is asked for, say plainly
that Arc stores its tabs differently and is not supported, rather than failing
obscurely.

Also out: Opera (not installed anywhere we can verify), Linux and Windows
paths, any parser change, anything in the `future` band.

## Safety — `browsers.md` §4

A `--profile` value reaches the filesystem. The reference implementation's
guard was judged sufficient against traversal, but confirm it survives contact
with the new code and add what §4 says it misses. Specifically: reject `Guest`
and `System` profiles, validate against the `info_cache` allowlist, and **never
put a display name into a path** — display names are user-controlled text and
can contain anything at all.

Non-ASCII profile display names are normal and must work.

## The multi-browser question you must answer

With several browsers installed, `save` picks one and names it. But the
library then holds snapshots from several browsers, and nothing in slices 1–3
anticipated that. Before writing code, work out and state in your report:

- Does the unchanged-session skip compare against the last snapshot **for the
  same browser and profile**, or the last snapshot overall? Get this wrong and
  alternating browsers defeats the skip entirely, or one browser masks another.
  Read what `src/archive.rs` actually does today.
- Does the library deduplicate a URL across browsers into one page, or keep
  them apart? One page is almost certainly right — it is the same page — but
  the sighting needs to say which browser, and the contract may not have room.
  **If the contract needs a field, say so in your report and do not add it
  unilaterally** — the frontend shipped and a silent contract change breaks it.

## Tests

- The path table: every browser resolves to the right user-data directory.
- Profile discovery with `last_used` present, absent, and pointing at a
  profile that no longer exists.
- A display name resolving to its directory; an ambiguous display name shared
  by two profiles erroring rather than guessing.
- Traversal attempts and `Guest`/`System` rejected.
- A non-ASCII display name.
- Zero-flag selection across three fake browsers with different mtimes,
  including the tie.
- An unknown `--browser` naming what is installed.
- `--browser arc` explaining itself.
- The unchanged-session skip with two browsers alternating.
- A browser installed but with no sessions, and one with an empty profile.

Use temporary directories with synthetic trees. **Do not read the real
browser profiles on this machine in a committed test.** Manual verification
against them is expected and welcome — report counts only, no URLs or titles.

## Fences

- You own: `src/platform.rs`, `src/cli.rs`, `src/capture.rs`, `src/archive.rs`,
  `src/error.rs`, `src/main.rs`, `tests/**`, `README.md`.
- Do not touch: `src/session.rs`, `src/snss.rs` (the parser is done and
  reviewed), `web/**`, `reference/**`, `slices.toml`, `docs/**`, `.github/**`,
  `xtask/**`.
- No new dependencies.
- `slice:` / `why:` headers on new files; `cargo xtask slices --check` passes.
- **Run no git commands at all.**

## Report

1. Files changed, with line counts.
2. The four gate outputs.
3. Your answers to the two multi-browser questions above, with what the code
   does today as evidence.
4. Which browsers you verified against real installations on this machine, and
   which are table-only. Counts, never content.
5. Whether slice 1's `platform.rs` actually left room for this, honestly.
6. Judgment calls, and what a reviewer should attack first.
