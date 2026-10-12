# Verification gates
<!-- lilo-page kind=gates schema=1 -->
<!-- lilo-covers .github/workflows/ci.yml .github/workflows/release.yml .cargo/config.toml Cargo.toml xtask/src/main.rs tests/serve_frontend.cjs justfile scripts/clippy.py lab/tagger/pyproject.toml -->
<!-- lilo-verified digest=0e774807df454ac3 commit=6dba24657080 date=2026-10-12 -->

Up: [knowmoretabs system map](index.md)

## Gates

| Gate | Command | Executed |
| --- | --- | --- |
| Scoped | `cargo test --locked --test <file>` | exit 0, 2026-10-06, as `--test tags`, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Structural | `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, 2026-10-06, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Full | `cargo test --workspace --all-targets --locked` | exit 0, 2026-10-06, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Frontend | `node --test tests/serve_frontend.cjs tests/extension.cjs` | renderer only: exit 0, 2026-10-08, on Node 20.20.2 and 25.9.0 |
| CI parity | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check && node --test tests/serve_frontend.cjs tests/extension.cjs` | previous command without extension tests: exit 0, 2026-10-08, on Homebrew rustc 1.99.0 and Node 25.9.0; clippy also on 1.89.0 |

Scoped: `<file>` is an integration test in `tests/` (`code:tests/tags.rs` is one); unit tests live beside the code, and the crate has a binary target only, so `cargo test --locked --bin knowmoretabs <filter>` scopes those; `--lib` fails with no library target. Integration tests drive the real binary.

Frontend: Cargo does not run JavaScript, so `code:tests/serve_frontend.cjs` runs the shipped `web/app.js` renderer in Node's `vm` with its DOM startup cut off, and `code:tests/extension.cjs` checks the toolbar extension's link building and port. Both use Node's own test runner and no packages. CI uses the Node that comes with the `ubuntu-24.04` image; the renderer test passes on Node 20 and 25.

Toolchains: on the owner's machine Homebrew's rust comes before rustup on `PATH`, so plain `cargo` is Homebrew's, newer than CI's 1.89.0, and `rustup run 1.89.0` alone still finds Homebrew's `cargo-clippy`. That order is the owner's setup; leave the shell config alone. To run a gate on 1.89.0, prefix it with `PATH="$HOME/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin:$PATH"` and check that `cargo clippy --version` reports 0.1.89, or run `python3 scripts/clippy.py`, which reads the pin from `ci.yml` and puts that rustup toolchain's clippy first. Clippy must pass on both: CI runs 1.89.0, and a newer clippy adds lints.

## Conditional

- Formatting: `cargo fmt --all -- --check`; exit 0, 2026-10-06.
- Slice metadata, after touching a `//!` header, `slices.toml` or adding a source file: `cargo run --locked --package xtask -- slices --check`; exit 0, 2026-10-06. Without `--check`, `cargo xtask slices` regenerates the matrix doc (`code:xtask/src/main.rs#run`, `code:.cargo/config.toml`).
- Local recipes: `just check` runs rustfmt, `scripts/clippy.py`, the slice check and ruff on `lab/tagger`; `just test` runs the Full and Frontend gates and the tagger tests (`code:justfile`). Both exit 0, 2026-10-12, on Homebrew rustc 1.99.0, Node 25.9.0 and uv 0.12.5.
- Tagger lab, after touching `lab/tagger`: `uv run --locked --project lab/tagger pytest -q lab/tagger/tests`; exit 0, 2026-10-12. CI runs it as the lab job. With `KMT_BIN` set to a build (`cargo build --locked`, then `KMT_BIN="$PWD/target/debug/knowmoretabs"`), `RealSendTests` also runs the Send tests against that binary on scratch roots; without it they skip.
- Release machinery: `.github/workflows/release.yml` runs on a `v*` tag or by hand; on a tag it fails unless the tag matches the `Cargo.toml` version and the changelog has a dated heading for it (`code:.github/workflows/release.yml`). Not executed.

## Hooks

none: no tracked hook config and no `core.hooksPath`.

## CI

`code:.github/workflows/ci.yml` runs on pull requests only, never on pushes to `main`. Rust jobs, all on toolchain 1.89.0, the `Cargo.toml` `rust-version`, then the lab and one Node job:

- rustfmt: `cargo fmt --all -- --check`, on `ubuntu-24.04`.
- clippy: `cargo clippy --workspace --all-targets --locked -- -D warnings`, on `ubuntu-24.04`.
- test: `cargo test --workspace --all-targets --locked` on linux, linux-arm, macos and windows, `fail-fast: false`; Windows first disables `core.autocrlf` and fails if any checked-out file has CRLF.
- xtask slices: `cargo run --locked --package xtask -- slices --check`.
- frontend: `node --test tests/serve_frontend.cjs tests/extension.cjs`, on `ubuntu-24.04` with the image's Node, no Rust toolchain.
- lab: the Tagger lab gate on `ubuntu-24.04` and `macos-latest`, `fail-fast: false`, 25 minutes: `cargo build --locked`, uv 0.12.5 with Python 3.12 and a cache keyed on `lab/tagger/uv.lock`, `uv sync --locked`, then pytest with `KMT_BIN` on the build and the Hugging Face offline variables. No Windows (`fcntl`) and no linux-arm (cost).

Actions are pinned to commit SHAs.

## Expected failures

none: every gate passes on rustc 1.89.0 and Homebrew rustc 1.99.0.
