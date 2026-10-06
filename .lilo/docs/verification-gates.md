# Verification gates
<!-- lilo-page kind=gates schema=1 -->
<!-- lilo-covers .github/workflows/ci.yml .github/workflows/release.yml .cargo/config.toml Cargo.toml xtask/src/main.rs -->
<!-- lilo-verified digest=f4d164133c6521f3 commit=59265b590b58 date=2026-10-06 -->

Up: [knowmoretabs system map](index.md)

## Gates

| Gate | Command | Executed |
| --- | --- | --- |
| Scoped | `cargo test --locked --test <file>` | exit 0, 2026-10-06, as `--test tags`, Homebrew rustc 1.99.0 |
| Structural | `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 101, 2026-10-06, Homebrew rustc 1.99.0; see Expected failures |
| Full | `cargo test --workspace --all-targets --locked` | exit 0, 2026-10-06, Homebrew rustc 1.99.0 |
| CI parity | `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check` | exit 101, 2026-10-06, stopped at clippy on Homebrew rustc 1.99.0 |

Scoped: `<file>` is an integration test in `tests/` (`code:tests/tags.rs` is one); unit tests live beside the code, and the crate has a binary target only, so `cargo test --locked --bin knowmoretabs <filter>` scopes those; `--lib` fails with no library target. Integration tests drive the real binary.

## Conditional

- Formatting: `cargo fmt --all -- --check`; exit 0, 2026-10-06.
- Slice metadata, after touching a `//!` header, `slices.toml` or adding a source file: `cargo run --locked --package xtask -- slices --check`; exit 0, 2026-10-06. Without `--check`, `cargo xtask slices` regenerates the matrix doc (`code:xtask/src/main.rs#run`, `code:.cargo/config.toml`).
- Release machinery: `.github/workflows/release.yml` runs on a `v*` tag or by hand; on a tag it fails unless the tag matches the `Cargo.toml` version and the changelog has a dated heading for it (`code:.github/workflows/release.yml`). Not executed.

## Hooks

none: no tracked hook config and no `core.hooksPath`.

## CI

`code:.github/workflows/ci.yml` runs on pull requests only, never on pushes to `main`. Jobs, all on toolchain 1.89.0, the `Cargo.toml` `rust-version`:

- rustfmt: `cargo fmt --all -- --check`, on `ubuntu-24.04`.
- clippy: `cargo clippy --workspace --all-targets --locked -- -D warnings`, on `ubuntu-24.04`.
- test: `cargo test --workspace --all-targets --locked` on linux, linux-arm, macos and windows, `fail-fast: false`; Windows first disables `core.autocrlf` and fails if any checked-out file has CRLF.
- xtask slices: `cargo run --locked --package xtask -- slices --check`.

Actions are pinned to commit SHAs.

## Expected failures

- Clippy on a toolchain newer than 1.89.0 fails on lints 1.89.0 lacks: `chunks_exact_to_as_chunks` in `code:src/snss.rs` and an assert-on-empty lint across `tests/`, as of 2026-10-06 with Homebrew rustc 1.99.0. CI parity therefore needs toolchain 1.89.0 first on `PATH`; with rustup, `rustup run 1.89.0` alone does not help when a Homebrew `cargo-clippy` shadows it.
