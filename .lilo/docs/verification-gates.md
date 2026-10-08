# Verification gates
<!-- lilo-page kind=gates schema=1 -->
<!-- lilo-covers .github/workflows/ci.yml .github/workflows/release.yml .cargo/config.toml Cargo.toml xtask/src/main.rs tests/serve_frontend.cjs -->
<!-- lilo-verified digest=5ec43ceb7ac5a40f commit=959c169c6354 date=2026-10-06 -->

Up: [knowmoretabs system map](index.md)

## Gates

| Gate | Command | Executed |
| --- | --- | --- |
| Scoped | `cargo test --locked --test <file>` | exit 0, 2026-10-06, as `--test tags`, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Structural | `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, 2026-10-06, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Full | `cargo test --workspace --all-targets --locked` | exit 0, 2026-10-06, on rustc 1.89.0 and Homebrew rustc 1.99.0 |
| Frontend | `node --test tests/serve_frontend.cjs` | exit 0, 2026-10-08, on Node 20.20.2 and 25.9.0 |
| CI parity | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check && node --test tests/serve_frontend.cjs` | exit 0, 2026-10-08, on Homebrew rustc 1.99.0 and Node 25.9.0; clippy also on 1.89.0 |

Scoped: `<file>` is an integration test in `tests/` (`code:tests/tags.rs` is one); unit tests live beside the code, and the crate has a binary target only, so `cargo test --locked --bin knowmoretabs <filter>` scopes those; `--lib` fails with no library target. Integration tests drive the real binary.

Frontend: Cargo does not run JavaScript, so `code:tests/serve_frontend.cjs` runs the shipped `web/app.js` renderer in Node's `vm` with its DOM startup cut off, using Node's own test runner and no packages. CI uses the Node that comes with the `ubuntu-24.04` image; the test passes on Node 20 and 25.

Toolchains: on the owner's machine Homebrew's rust comes before rustup on `PATH`, so plain `cargo` is Homebrew's, newer than CI's 1.89.0, and `rustup run 1.89.0` alone still finds Homebrew's `cargo-clippy`. That order is the owner's setup; leave the shell config alone. To run a gate on 1.89.0, prefix it with `PATH="$HOME/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin:$PATH"` and check that `cargo clippy --version` reports 0.1.89. Clippy must pass on both: CI runs 1.89.0, and a newer clippy adds lints.

## Conditional

- Formatting: `cargo fmt --all -- --check`; exit 0, 2026-10-06.
- Slice metadata, after touching a `//!` header, `slices.toml` or adding a source file: `cargo run --locked --package xtask -- slices --check`; exit 0, 2026-10-06. Without `--check`, `cargo xtask slices` regenerates the matrix doc (`code:xtask/src/main.rs#run`, `code:.cargo/config.toml`).
- Release machinery: `.github/workflows/release.yml` runs on a `v*` tag or by hand; on a tag it fails unless the tag matches the `Cargo.toml` version and the changelog has a dated heading for it (`code:.github/workflows/release.yml`). Not executed.

## Hooks

none: no tracked hook config and no `core.hooksPath`.

## CI

`code:.github/workflows/ci.yml` runs on pull requests only, never on pushes to `main`. Rust jobs, all on toolchain 1.89.0, the `Cargo.toml` `rust-version`, then one Node job:

- rustfmt: `cargo fmt --all -- --check`, on `ubuntu-24.04`.
- clippy: `cargo clippy --workspace --all-targets --locked -- -D warnings`, on `ubuntu-24.04`.
- test: `cargo test --workspace --all-targets --locked` on linux, linux-arm, macos and windows, `fail-fast: false`; Windows first disables `core.autocrlf` and fails if any checked-out file has CRLF.
- xtask slices: `cargo run --locked --package xtask -- slices --check`.
- frontend: `node --test tests/serve_frontend.cjs`, on `ubuntu-24.04` with the image's Node, no Rust toolchain.

Actions are pinned to commit SHAs.

## Expected failures

none: every gate passes on rustc 1.89.0 and Homebrew rustc 1.99.0.
