# Slice 6 — Install it in one line

**Ships:** tagged releases with prebuilt binaries for macOS, Linux and Windows,
a changelog, install instructions that do not begin with "first install Rust",
and a crate ready to publish.

**Publishing is not part of this slice.** Build the machinery and leave the
trigger unpulled. Do not publish to crates.io, do not create a Homebrew tap,
do not push a tag that triggers a real release. That is the owner's decision
and they are not here.

## Required reading

1. `docs/BRIEF.md` §1 (including the non-goals) and §7.
2. `docs/research/rust-stack.md` §13 — the release-tooling recommendation.
3. `slices.toml` — slice `release`, and the whole `future` band, which is what
   the README must be honest about.
4. `.github/workflows/ci.yml` — the existing three-platform matrix.
5. `README.md` — five slices of accretion. It needs a proper pass, not a patch.

## 1. Fix the slice lint first

`slices.toml` now marks `platforms` done and the lint refuses:

```
slices: slice `platforms` is done but no source file claims it
```

This is the metadata system meeting its first real case, and the lint is
right to complain. `src/platform.rs` genuinely serves **both** `browsers` and
`platforms` — it is one table with a column per operating system. Splitting it
to satisfy a linter would be the tail wagging the dog.

So: **let a file claim more than one slice.** Extend the marker to accept a
list, update `cargo xtask slices` to parse and enforce it, keep the rules in
`docs/BRIEF.md` §6 otherwise intact, and regenerate `docs/SLICES.md`. Then
mark up `src/platform.rs` honestly, and check every other file for the same
situation now that it is expressible — several probably qualify.

Keep the lint small. It was budgeted at ~300 lines and it is a helper, not a
product.

## 2. Release workflow

- Triggered by a tag matching `v*`, and buildable on demand via
  `workflow_dispatch` so it can be exercised without tagging.
- Binaries for at least: `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `x86_64-pc-windows-msvc`.
- **Linux jobs on `blacksmith-2vcpu-ubuntu-2404`.** The GitHub-hosted Linux
  credit pool is exhausted; this applies to release CI exactly as it does to
  test CI. macOS and Windows use GitHub-hosted runners, the same sanctioned
  exception slice 5 established.
- Archives with checksums. Include the licences and the README in each.
- A GitHub Release with notes drawn from the changelog.
- `docs/research/rust-stack.md` §13 recommended `cargo-dist`. Check it is still
  maintained and that it can be told to use Blacksmith for Linux. **If it
  cannot, a hand-written workflow is the correct answer** — the project has
  form here: slice 3 rejected `tiny_http` after reading its source, and that
  was the right call. Decide, implement, and justify it in a paragraph.
- **Prove it runs** via `workflow_dispatch` on a branch. A release workflow
  that has never executed is a guess. If you cannot trigger it yourself, say
  so and I will run it.

## 3. Crate metadata

`Cargo.toml` needs `description`, `repository`, `homepage`, `license`,
`keywords`, `categories`, `readme`, `rust-version`, and an `exclude` that
keeps `reference/`, `docs/`, `web/fixtures/` and similar out of the published
package. Verify with `cargo package --list` that what ships is what should
ship — the binary needs `web/`'s three assets at build time, so check they are
actually included. `cargo publish --dry-run` must pass. **Do not publish.**

## 4. CHANGELOG.md

Keep a Changelog format. One entry per slice, written for a **user**, not a
git log: what they can now do that they could not before. The git history is
detailed and the PR descriptions are good; use them. Version `0.1.0`,
unreleased.

## 5. The README, properly

This is the first thing anyone sees and it has accreted across five slices.
Rewrite it for someone who has never heard of the project. It should cover:

- What it is and the problem it solves, in the first three lines.
- Install: prebuilt binary first, `cargo install` second. Never "install Rust".
- The two commands that matter — `save` and `serve` — before anything else.
- Where data lives per platform, and that nothing leaves the machine.
- The non-goals from `docs/BRIEF.md` §1, stated plainly.
- **The encrypted-sessions clock**, honestly. Chromium is migrating to
  encrypted session storage; when it completes, `knowmoretabs` stops being
  able to read sessions and refuses rather than reporting stale tabs as
  current. `docs/research/encrypted-sessions.md` has the detail. Someone
  deciding whether to depend on this deserves to know.
- What is deliberately not built, from `slices.toml`'s `future` band.

No badges for things that do not exist, no roadmap fiction, no "blazingly
fast". Match the tone of the rest of the project's prose.

## 6. A final honest pass

You are the last agent on this project before it is handed over. Check:

- `cargo package` contents; no personal data anywhere in the repo, including
  fixtures and test data. This matters more than anything else on the list.
- Every `slice:` / `why:` header is accurate after five slices of change.
- `docs/SLICES.md` regenerated.
- `docs/briefs/` — these are how the work was specified. Keeping them is
  right; make sure nothing in them reads as a promise that was not kept.
- No TODO, FIXME or stub left in `src/`.
- `--help` output reads well for every command.
- The licence files are correct and the copyright line is right.

Anything you find wrong but out of scope: **report it, do not fix it.**

## Fences

- You own: `.github/**`, `Cargo.toml`, `README.md`, `CHANGELOG.md`,
  `xtask/**`, `slices.toml` (markers and statuses only — **do not edit any
  slice's prose, `intent`, or the `future` band**), `docs/SLICES.md`
  (generated), and `slice:` headers in `src/**`.
- Do not change behaviour. No feature work, no refactoring, no new
  dependencies in the binary. If a test fails, that is a finding.
- Do not touch `web/`'s three assets, `reference/`, `docs/BRIEF.md`,
  `docs/research/**`, or `docs/briefs/**`.
- **Run no git commands at all**, and do not create or push a tag.

## Report

1. Files created or changed.
2. All four gates, and confirmation CI still passes on three platforms.
3. Your `cargo-dist` decision and the paragraph justifying it.
4. `cargo package --list` output, and `cargo publish --dry-run` result.
5. Whether the release workflow actually ran, and where.
6. The final-pass findings, including anything you found and deliberately left.
