# Rust stack decision

Status: implementation decision. This document is intentionally opinionated.
It was checked against the brief and `slices.toml` on 2026-07-14. The rule is
simple: a dependency must remove a concrete correctness or portability risk;
otherwise the standard library wins.

## Thirteen decisions at a glance

| # | One-line answer |
|---:|---|
| 1 | Use Rust 2024 with `rust-version = "1.89"`; it buys the edition and stable cross-platform `File` locks while keeping a defensible source-build floor. |
| 2 | Do not split a `knowmoretabs-core` crate: one binary package with modules plus `xtask` is enough until there is a second consumer. |
| 3 | Use `clap` derive for the top-level command and subcommands; its validated help and global-argument handling earn its cost. |
| 4 | Use `thiserror` for typed internal errors and a small hand-written stderr renderer; do not add `anyhow`, `miette`, or `color-eyre`. |
| 5 | Use blocking `tiny_http` 0.12, bound only to `127.0.0.1`; it is dormant but has a five-package normal dependency tree versus 48 for a realistic `axum` + `tokio` tree. |
| 6 | Use four `include_str!` calls with debug builds reading the source asset directory; do not add an embedding crate. |
| 7 | Use `jiff` 0.2 with only `std` and `serde` features; it handles UTC instants and formatting without hand-written calendar arithmetic. |
| 8 | Use `directories::BaseDirs` only for the home directory, and hand-write the small browser/profile table; `etcetera` solves a different problem. |
| 9 | Use `serde`/`serde_json`, `sha2`, and `tempfile`; use `std` for rename, syncing, and advisory locks (MSRV 1.89). |
| 10 | Use unit tests, standard-library binary integration tests, `tempfile`, and a small `proptest` property suite; skip `assert_cmd`, `insta`, and a fuzzing campaign. |
| 11 | Deny Rust `unsafe_code`, Clippy `all` and `pedantic`, and add no global pedantic exceptions. |
| 12 | Run one Blacksmith job on `blacksmith-2vcpu-ubuntu-2404` using `useblacksmith/checkout@v1`, `dtolnay/rust-toolchain@1.89.0`, and upstream `Swatinem/rust-cache@v2`. |
| 13 | Use `cargo-dist` for release orchestration, overriding Linux jobs to the Blacksmith runner; keep hand-written release logic out of the product. |

The application has **10 direct runtime dependencies**. The `xtask` package
adds one tooling-only dependency, `toml`; the test-only dependency is
`proptest`. This is 10 crates in the shipped binary, not a framework stack.

## 1. Edition and MSRV

### Recommendation

Set:

```toml
edition = "2024"
rust-version = "1.89"
```

Rust 2024 became stable with Rust 1.85. Rust 1.89 (August 2025) stabilized
`File::lock`, `lock_shared`, `try_lock`, and `unlock` on Unix and Windows. That
second fact matters here: it removes a file-locking crate from a product whose
archive must be lock-guarded. Rust 1.89 is old enough to be a realistic source
build floor in late 2026, while still providing the exact standard-library API
the design needs. The release workflow should test 1.89; a developer may use
newer stable Rust.

Sources: [Rust 2024 announcement](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
and [Rust 1.89 stabilization list](https://blog.rust-lang.org/2025/08/07/Rust-1.89.0/).

Alternative: Rust 1.85 is the widest compatible MSRV, but then locking needs a
crate such as `fs4`, which is a permanent cost for one operation. Rust 2021 is
also valid, but gives up the current edition's improvements for no product
benefit.

## 2. Workspace layout

### Recommendation

Use one workspace with two packages:

```text
Cargo.toml              # the knowmoretabs binary package and workspace root
src/main.rs             # command dispatch and process exit
src/cli.rs              # clap types
src/error.rs            # typed errors and human/JSON rendering
src/capture.rs          # discovery and capture orchestration
src/snss.rs             # bounded, tolerant SNSS parser
src/archive.rs          # lock, staging, fsync, rename, snapshots
src/library.rs          # JSON user state and derived page views
src/server.rs           # tiny_http loop and route matching
src/assets.rs           # embedded/debug-loaded assets
src/platform.rs         # browser/profile path table
assets/                 # index.html, styles.css, app.js, favicon.svg
tests/                  # real-binary integration tests
xtask/Cargo.toml
xtask/src/main.rs       # slice marker and generated-doc checker
```

Do not create `knowmoretabs-core`. A separate library becomes concretely useful
when a second binary, an external consumer, or a separately compiled library
API must consume the parser/archive code. None exists in the brief. Unit tests
can exercise private modules beside their code, and integration tests can drive
the real binary; a split would instead add a package boundary, public API
decisions, and another compile target now.

Keep modules direct and concrete. No traits for the one filesystem, browser,
or HTTP implementation. `xtask` is separate because it is a developer command
that reads repository metadata and must not ship in the user binary.

Alternative: a `src/lib.rs` target in the same package would be a reasonable
later step if integration tests need typed library calls; it is not a reason to
make a second `-core` package today.

## 3. CLI

### Recommendation

Use `clap` 4 with derive. Model the global options on the root `Args` type and
the commands in an enum; make the command optional so no subcommand means
`save`. Mark the root options global so `knowmoretabs --json list` and
`knowmoretabs list --json` have the same semantics. Let clap handle `--`,
unknown options, shell-safe values, generated help, and exit status.

This is worth one direct dependency because a hand parser has concrete failure
modes in this CLI: global flags accepted in one position but not the other,
bad port values reaching the server, malformed repeated URLs, and inconsistent
help/error output. The derive declarations also make the command surface
visible in one file.

Alternative: `pico-args` or `lexopt` are smaller, but require us to reimplement
subcommand/global-option validation and help; `argh` is a derive alternative
with less of the clap command UX we need.

## 4. Errors

### Recommendation

Use `thiserror` 2 for module error enums. The binary's `main` converts those
errors into one actionable line on stderr, with the operation and path in the
outer message; `-v` walks `Error::source()` and prints the chain. `--json`
uses a small explicit error object. Do not print a backtrace by default.

For a torn session file, the parser returns successfully with a
`truncated_bytes` count and the command prints a warning such as “session tail
was incomplete; saved 241 complete tabs and skipped 37 bytes”. That is a data
quality result, not a process error. A failure such as an unreadable browser
file says what to do next: “cannot read ...; close the browser or pass
`--session PATH`”.

`thiserror` is enough because typed variants preserve the distinction between
missing input, invalid user arguments, I/O, and archive invariants without a
runtime reporting framework. The renderer is short and belongs to this
binary's UX.

Alternative: `anyhow` would make context convenient but would erase useful
error categories at module boundaries; `miette` and `color-eyre` add
pretty-report/backtrace machinery that is noisy for one-line CLI failures and
does not improve the torn-tail message.

## 5. HTTP server for `serve`

### Recommendation

Use `tiny_http = { version = "0.12", default-features = false }`. Bind only
to `127.0.0.1`, match the handful of routes with ordinary `match` statements,
cap request bodies before reading them, and return explicit content types and
status codes. The server is synchronous; the archive code stays synchronous.

Maintenance is the cost to acknowledge: 0.12.0 is the latest release and was
published 2022-10-06. The upstream repository has an open May 2026 issue
asking for additional maintainers. It is therefore not an actively evolving
dependency. It is still the recommendation for this product because its
protocol implementation is mature, its normal dependency tree is five
packages, and loopback single-user traffic does not need HTTP/2, websockets,
TLS, middleware, or an async executor. Pin 0.12 in the manifest and exercise
the actual routes in integration tests; do not expose it beyond loopback.

The dependency count below is from fresh isolated probe crates, resolved on
2026-07-14, using `cargo tree --edges normal`: `tiny_http` plus its four normal
dependencies = **5 dependency packages**; `axum = "0.8"` plus
`tokio = "1"` with `macros,rt-multi-thread,net` and axum's normal features =
**48 dependency packages**. The probe root is excluded. The full tree is not a
performance benchmark; it is the moving-parts comparison requested here.

Sources: [tiny_http 0.12.0 and its dependency list](https://docs.rs/crate/tiny_http/0.12.0),
[the 0.12.0 release notes](https://github.com/tiny-http/tiny-http/releases/tag/0.12.0),
and [the maintainer request](https://github.com/tiny-http/tiny-http/issues/286).

Alternative: `axum` plus `tokio` is the right choice once concurrent clients,
streaming, websockets, or long-lived async I/O are real requirements, but those
requirements are absent and it adds about 43 packages in this probe. A
hand-written HTTP parser is rejected because malformed requests and framing
are exactly the security-sensitive code not worth owning.

## 6. Static assets

### Recommendation

Use `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/index.html"))`
and the equivalent calls for CSS, JS, and the favicon. In a debug build,
attempt to read the same four paths from disk on every request, falling back to
the embedded strings if the files are absent. Release builds use the embedded
content directly. This gives an editable `cargo run` frontend and a single
binary release without a feature flag or build script.

Escape any exported JSON before putting it in the HTML script element; the
asset loader is not the place to weaken the content-security policy.

Alternative: `rust-embed` adds a proc-macro asset abstraction and `include_dir`
adds a directory tree abstraction; both solve many-file asset trees, while
four known files need four standard macros and no new API.

## 7. Dates and times

### Recommendation

Use `jiff` 0.2 with `default-features = false` and features `std, serde`.
Convert filesystem `SystemTime` values to `jiff::Timestamp`, format the
directory ID in UTC with a fixed pattern, and serialize the captured timestamp
as an explicit RFC 3339-like string. Do not enable bundled timezone data: the
archive ID is UTC and does not need local-zone lookup.

`std::time` can measure an instant but cannot format a Gregorian UTC directory
name without hand-written calendar arithmetic. `jiff` is the current actively
maintained date/time choice and has a direct `SystemTime`/timestamp path. Its
feature set lets this app avoid timezone functionality it does not use.

Alternative: `time` is a sound lower-level choice but needs several formatting
and parsing features wired correctly; `chrono` is older API surface and would
not improve this UTC-only job. Neither earns replacing Jiff here.

Sources: [Jiff features](https://docs.rs/jiff/latest/jiff/#crate-features),
[Jiff's 2026 maintenance notes](https://github.com/BurntSushi/jiff), and
[the `time` feature model](https://docs.rs/time/latest/time/).

## 8. Platform paths

### Recommendation

Use `directories::BaseDirs::home_dir()` for the default root, then append
`.knowmoretabs`. Keep browser discovery in a small explicit table under
`src/platform.rs`: browser name, macOS relative paths, Linux/XDG paths, and
Windows local-app-data paths, plus the profile layout. Test the table with
synthetic environment/path inputs; discovery must never touch a live browser
file except to read and copy the selected session.

The concrete reason not to hand-roll the home path is Windows: the correct
profile directory comes from the Known Folder API, not merely `HOME`, and Unix
fallback behavior matters when an environment is incomplete. `directories`
does that one job. No general path crate can know Chrome/Brave/Edge/Vivaldi/Arc
profile layouts, so that knowledge remains application code.

Alternative: `etcetera` is useful when choosing XDG, Apple, Windows, or
single-folder configuration conventions, but its strategy layer is unnecessary
when the brief fixes the archive root and we still need a browser-specific
table. Pure `std::env` is rejected for the Windows home failure above.

Source: [directories' platform table](https://docs.rs/crate/directories/latest).

## 9. Data, hashing, durable writes, and locks

| Need | Recommendation | Why it earns a dependency (or not) |
|---|---|---|
| Serialization | `serde` with `derive`, `serde_json` | JSON is the source of truth; deriving version-tolerant structs and skipping unknown fields is safer than handwritten JSON. |
| SHA-256 | `sha2` | A correct portable SHA-256 implementation is not in `std`; this hashes the copied source file for identity/deduplication. Format the digest with `LowerHex`; no `hex` crate. |
| Temporary staging | `tempfile` | Creating a unique sibling temp directory/file and cleaning it on early error has race and cleanup edge cases in a hand-rolled PID/timestamp name. It is used by production archive writes and tests. |
| Atomic rename | `std::fs::rename` | No crate. Stage beside the destination, `sync_all` the file, rename once, and never rename over an existing snapshot. Same-volume sibling staging is the needed invariant. |
| Durability | `std::fs::File::sync_all` and, on Unix, sync the parent directory after rename | No crate. The archive promise is about a crash between writes; a buffered close alone is not enough. Handle the platform-specific directory-sync limitation explicitly. |
| Advisory lock | `std::fs::File::{lock,try_lock,unlock}` | No crate at MSRV 1.89. Hold an exclusive lock on `.knowmoretabs/lock` across read/parse/stage/rename and library-state writes. `try_lock` gives a useful “another save is running” error. |
| URL semantics | `url` 2.5 | Browser URLs are not safe to split on `://`: authority, userinfo, ports, IPv6, percent escapes, and internationalized hosts are concrete counterexamples. Use it for domain/search normalization; keep the original URL string in the model. |

The lock file itself is not a snapshot and may be created before the archive
root's mode is finalized. Create the root with restrictive permissions on Unix,
then open the lock read/write because Windows file locks reject append-only
handles. Never use a lock as a substitute for the atomic rename.

Alternative: `fs4`/`fs2` would support older compilers, but that is precisely a
dependency avoided by choosing MSRV 1.89. An `atomic-write` crate would hide a
small sequence whose ordering is part of this product's archive invariant.

## 10. Testing

### Recommendation

Use unit tests beside the parser, navigation-pruning, archive, and path-table
modules. Add `tests/` integration tests that invoke the compiled binary with
`std::process::Command` and `CARGO_BIN_EXE_knowmoretabs`; assert exit status,
stdout, stderr, and the resulting files. This avoids adding `assert_cmd`: the
standard library already supplies the exact process boundary needed here.

Use `tempfile` for isolated archive roots. Use `proptest` only for a small,
maintained parser property suite; it earns its place because shrinking a
minimal failing byte sequence is materially better than hand-debugging a
random SNSS length/offset failure. Do not add `insta`: the CLI output is short
and intentional assertions make changes visible without snapshot files.

A good byte-format parser test does not merely parse one happy fixture:

```rust
#[test]
fn every_truncation_of_a_valid_record_is_nonfatal() {
    let bytes = synthetic_session_with_two_complete_records();

    for cut in 0..=bytes.len() {
        let result = parse(&bytes[..cut]);
        assert!(result.is_ok(), "cut at {cut} must not panic or abort");
        let parsed = result.unwrap(); // test-only assertion convenience
        assert!(parsed.stats.tabs <= 2);
        assert!(parsed.stats.truncated_bytes <= bytes.len());
    }
}
```

The real version should assert exact complete-record counts and remainder
lengths, include an unknown command ID and a missing selected-navigation index,
and use a synthetic fixture with no real URLs. A `proptest` property should feed
bounded arbitrary byte vectors and assert: no panic, no allocation based only
on an untrusted declared length, and a result whose counters are bounded by
the input. That is property testing, not a separate fuzzing product.

Do **not** start `cargo fuzz`. We would not personally maintain a corpus or
run a fuzz target in CI at this scale. Reconsider only after a real parser bug
escapes the truncation/property tests and someone owns a minimized corpus.

Alternative: `assert_cmd` is a good choice for a large CLI matrix, but here a
20-line `Command` helper avoids a test-only dependency; `insta` is appropriate
for large stable renderings, not a handful of lines.

## 11. Lints

### Recommendation

Put exactly this in the root package's `Cargo.toml`:

```toml
[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = "deny"
pedantic = "deny"
```

`all` covers the normal correctness/style/complexity/performance groups;
`pedantic` adds the deliberate-quality checks required by the brief. Do not
enable `nursery` or `restriction` globally: they are noisy and change more
often than this small product needs. The CI command still passes `-D warnings`
so rustc and Clippy warnings cannot slip through.

There are no global pedantic allows. If domain vocabulary creates a genuine
false positive, use a narrow point allow with a reason, for example:

```rust
// `SnapshotError` is the domain type name; shortening it would make the error less clear.
#[allow(clippy::module_name_repetitions)]
enum SnapshotError { /* ... */ }
```

Do not add an allow merely to make CI green. Keep `unwrap`/`expect` out of
production code by review; test assertions may use them locally.

## 12. Blacksmith CI

### Recommendation

Use one uncomplicated job on the required runner. The action names were
verified against their repositories: [Blacksmith checkout](https://github.com/useblacksmith/checkout)
documents `useblacksmith/checkout@v1`; [Blacksmith's Rust-cache repository](https://github.com/useblacksmith/rust-cache)
documents `useblacksmith/rust-cache@v3` but is archived and says to use the
upstream maintained action; [Blacksmith's cache design](https://www.blacksmith.sh/blog/cache)
now routes standard cache requests transparently. Rust setup is
[dtolnay/rust-toolchain](https://github.com/dtolnay/rust-toolchain), not a
nonexistent `useblacksmith/setup-rust` action.

This is the concrete workflow to add later as `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  rust:
    runs-on: blacksmith-2vcpu-ubuntu-2404
    timeout-minutes: 20
    steps:
      - name: Check out
        uses: useblacksmith/checkout@v1

      - name: Install the MSRV toolchain
        uses: dtolnay/rust-toolchain@1.89.0
        with:
          components: rustfmt, clippy

      - name: Cache Cargo
        uses: Swatinem/rust-cache@v2

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --workspace --all-targets --locked -- -D warnings

      - name: Test
        run: cargo test --workspace --all-targets --locked

      - name: Check slice metadata
        run: cargo run --locked --package xtask -- slices --check
```

`cargo run -p xtask -- slices --check` is used rather than relying on a local
Cargo alias, so a fresh checkout has no hidden configuration requirement. Pin
action references to full commit SHAs when the workflow is committed; the
names above are the verified action identities.

Alternative: `actions/checkout` plus `actions/cache` would work on a Blacksmith
runner because Blacksmith transparently accelerates cache requests, but the
Blacksmith checkout action also caches Git objects and costs no application
dependency. The archived `useblacksmith/rust-cache@v3` is not recommended.

## 13. Release

### Recommendation

Use `cargo-dist` as a release tool, generated and reviewed into the repository.
Put the following in `dist.toml` (not in the root `Cargo.toml`).
Start with these targets:

```toml
[dist]
dist = true
ci = ["github"]
targets = [
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
]

[dist.github-custom-runners]
global = "blacksmith-2vcpu-ubuntu-2404"
x86_64-unknown-linux-gnu = "blacksmith-2vcpu-ubuntu-2404"
```

The exact generated workflow must retain Blacksmith for the Linux target and
for global packaging/hosting jobs; macOS and Windows may use their native
runners when those jobs are enabled in slice 6. Never let generated Linux jobs
fall back to `ubuntu-latest`. `cargo-dist` already produces archives, checksums,
installers when selected, and GitHub-release orchestration; its documented
custom-runner table accepts target triples and a `global` runner.

Alternative: `taiki-e/upload-rust-binary-action` is a useful low-level uploader,
but it still needs a release-creation job, matrix, toolchain setup, archive
policy, and the Linux runner override. A hand-written workflow gives maximum
control but makes us own that matrix and every checksum/archive edge case.

Sources: [cargo-dist's simple workspace guide](https://axodotdev.github.io/cargo-dist/book/workspaces/simple-guide.html),
[its target and custom-runner configuration](https://axodotdev.github.io/cargo-dist/book/reference/config.html),
and [taiki-e's action inputs and multi-platform workflow](https://github.com/taiki-e/upload-rust-binary-action).

## Deferred, with triggers

| Defer | Trigger to revisit |
|---|---|
| `knowmoretabs-core` package | A second binary, external consumer, or separately versioned library API appears. |
| `axum`/`tokio` | Measured need for many simultaneous clients, streaming/websockets, or an async-only dependency. |
| SQLite/index crate | A measured archive size/query exceeds the JSON-in-memory budget, for example search becomes perceptibly slow around 100k tabs. |
| `miette`/`color-eyre` | Users need multi-span source diagnostics or an opt-in developer report mode; ordinary torn tails do not. |
| Fuzzing corpus | A parser bug escapes the maintained property tests and an owner commits to triaging minimized corpus cases. |
| Embedded-asset crate | The asset set becomes a generated directory tree whose file enumeration is itself more error-prone than the four macros. |

## Recommended root `Cargo.toml`

This is complete and pasteable for the root package. The separate `xtask`
manifest is shown immediately after it because it is a real workspace package,
not a runtime dependency.

```toml
[package]
name = "knowmoretabs"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"
description = "Keep and search the browser tabs you have had open"
repository = "https://github.com/littleorgans/knowmoretabs"
license = "MIT OR Apache-2.0"
readme = "README.md"

[workspace]
members = [".", "xtask"]
resolver = "3"

[dependencies]
clap = { version = "4.5", features = ["derive"] }
directories = "6"
jiff = { version = "0.2", default-features = false, features = ["std", "serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
tempfile = "3"
thiserror = "2"
tiny_http = { version = "0.12", default-features = false }
url = "2.5"

[dev-dependencies]
proptest = "1"

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = "deny"
pedantic = "deny"
```

`xtask/Cargo.toml` is:

```toml
[package]
name = "xtask"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"
publish = false

[dependencies]
toml = "0.9"
```
