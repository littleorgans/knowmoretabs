# Slice 5 — Linux and Windows

**Ships:** session discovery on Linux and Windows, path handling that survives
both, and a CI matrix that proves it rather than assuming it.

Slice 4 built the browser table with per-platform path columns. If that work
was done well, this slice fills in two columns and fixes what turns out to be
macOS-shaped elsewhere. Expect the second half to be the larger half.

## Required reading

1. **`docs/research/browsers.md`** §1, §2, §4, §6 — the path matrix for Linux
   (including Flatpak and Snap, which differ) and Windows, plus which
   environment overrides to honour. It is the specification.
2. `docs/BRIEF.md` §2 and §5.
3. `src/platform.rs` — the table as slice 4 left it.
4. `.github/workflows/ci.yml` — one Linux job today.

## In scope

- Linux paths for every browser already supported, including the **Flatpak and
  Snap variants**, which live somewhere different and are common enough to
  matter. `XDG_CONFIG_HOME`, `CHROME_CONFIG_HOME` and `CHROME_USER_DATA_DIR`
  honoured per `browsers.md` §6.
- Windows paths via `%LOCALAPPDATA%`. Use the crate already in the dependency
  list rather than reading the variable directly where it offers the known
  folder.
- **Everything else that is quietly macOS-only.** Find it rather than assuming
  slice 4 got it all. Candidates worth checking specifically: the archive root
  (`~/.knowmoretabs` — right on Linux, questionable on Windows: decide and say
  why), `--open`, file locking, path separators in anything user-visible, case
  sensitivity, reserved filenames, and the `0700` archive permission, which
  means nothing on Windows.
- **A CI matrix that actually runs the suite on all three.** Linux stays on
  `blacksmith-2vcpu-ubuntu-2404`. macOS and Windows have no Blacksmith
  equivalent, so they use GitHub-hosted runners — **this is the one sanctioned
  exception to the no-`ubuntu-latest` rule**, which exists because the Linux
  credit pool is exhausted, not because GitHub runners are banned outright.
  Keep the macOS and Windows jobs lean: test only, no duplicate fmt or clippy,
  since those are platform-independent and already run on Linux.

## Out of scope

New browsers, Arc, Opera, parser changes, anything in the `future` band,
packaging and release (slice 6), and any attempt to support WSL as a distinct
platform.

## Things that will bite

- **Windows path length.** The archive nests snapshot directories under a
  user-chosen root. Combined with a long `--root` this can exceed the classic
  260-character limit. Know what happens and make it a clear error rather than
  a mysterious one.
- **Reserved names.** `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`,
  and trailing dots or spaces are illegal in Windows filenames. Profile
  directory names come from the browser so they are probably safe, but a
  `--root` is not.
- **Atomic rename.** Slice 1's durability rests on `rename` replacing
  atomically. On Windows `rename` fails if the destination exists, which is
  different from POSIX. Check every place the archive relies on it — publishing
  a snapshot and `replace_file` in `src/archive.rs` — and prove the guarantee
  still holds. **This is the most important item in this slice**, because a
  silent loss of atomicity is exactly the kind of bug nobody notices until it
  costs someone their archive.
- **Advisory file locking.** `flock` semantics are POSIX. Establish what the
  standard library gives you on Windows and whether the concurrent-save test
  still means anything there.
- **Line endings.** Do not let CRLF creep into written files or test fixtures.
- **`--open`.** `open` on macOS, `xdg-open` on Linux, `start`/`ShellExecute` on
  Windows. Failure to open must never fail the command.

## Tests

- Path resolution per platform, table-driven, with the environment overrides.
- Flatpak and Snap layouts on Linux.
- Existing tests must pass unchanged on all three platforms. Where one cannot,
  that is a finding to report, not a `cfg` to hide behind. **Do not disable a
  test to make a platform green** — if a guarantee genuinely differs, say so
  and test the real guarantee.
- Windows-specific: a path near the length limit, a reserved name as `--root`,
  and snapshot publication over an existing directory.

## Fences

- You own: `src/platform.rs`, `src/archive.rs`, `src/capture.rs`, `src/cli.rs`,
  `src/main.rs`, `src/out.rs`, `src/server.rs`, `tests/**`,
  `.github/workflows/ci.yml`, `README.md`, `Cargo.toml`.
- Do not touch: `src/session.rs`, `src/snss.rs`, `web/**`, `reference/**`,
  `slices.toml`, `docs/**`, `xtask/**`.
- No new dependencies without naming the concrete portability problem it
  solves that the standard library does not.
- `cargo xtask slices --check` passes.
- **Run no git commands at all.**

## Report

1. Files changed, with line counts.
2. The four gate outputs on macOS, and the suite's result on Linux.
3. **The atomic-rename answer**, with the evidence: what Windows does, what you
   changed, and why the durability guarantee still holds.
4. Everything you found that was quietly macOS-only, whether or not you fixed it.
5. What is genuinely unverifiable without a real Windows machine, stated
   plainly. I would rather have an honest "CI is the first real test of this"
   than a confident guess.
