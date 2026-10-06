# Repo instructions: knowmoretabs
- Scoped: `cargo test --locked --test <file>`
- Structural: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- Full: `cargo test --workspace --all-targets --locked`
- CI parity: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check`
- CI pins toolchain 1.89.0; clippy on a newer rustc fails on new lints. Put 1.89.0 first on `PATH`.
- Every `.rs` under `src/` and `xtask/src/` needs a `//!` header with `slice:` and `why:`; run the slices check after changing one.
- The slice matrix doc is generated from `slices.toml` by `cargo xtask slices`; never hand-edit it.
- Never rewrite a snapshot or a browser file; hold the archive lock to write; replace files by stage and rename.
- No `println!`/`eprintln!`: user output goes through `src/out.rs`. No unsafe.
- Frontend in `web/` is embedded at build time, no build step; keep LF line endings.
Docs: `.lilo/docs/`, start at `index.md`.
- `index.md`: purpose, responsibilities, vocabulary, rules, sources.
- `verification-gates.md`: gates, CI jobs, expected failures.
<!-- lilo-onboarding status=complete-quick date=2026-10-06 schema=1 -->
<!-- lilo-digest sha256:eaaeb9e7d312f1c8 .lilo/docs/index.md -->
<!-- lilo-digest sha256:e834dc1da8761fad .lilo/docs/verification-gates.md -->
