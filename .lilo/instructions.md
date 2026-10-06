# Repo instructions: knowmoretabs
- Scoped: `cargo test --locked --test <file>`
- Structural: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- Full: `cargo test --workspace --all-targets --locked`
- CI parity: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check`
- CI pins 1.89.0; run clippy on it too, with `PATH="$HOME/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin:$PATH"`.
- Every `.rs` under `src/` and `xtask/src/` needs a `//!` header with `slice:` and `why:`; run the slices check after changing one.
- The slice matrix doc is generated from `slices.toml` by `cargo xtask slices`; never hand-edit it.
- Never rewrite a snapshot or a browser file; hold the archive lock to write; replace files by stage and rename.
- No `println!`/`eprintln!`: user output goes through `src/out.rs`. No unsafe.
- Frontend in `web/` is embedded at build time, no build step; keep LF line endings.
- Fixtures are synthetic: never commit a real URL or title from a browser. `reference/` is read-only.
Docs: `.lilo/docs/`, start at `index.md`.
- `index.md`: purpose, responsibilities, vocabulary, rules, sources.
- `verification-gates.md`: gates, CI jobs, expected failures.
<!-- lilo-onboarding status=complete-quick date=2026-10-06 schema=1 -->
<!-- lilo-digest sha256:be5eed8783935e04 .lilo/docs/index.md -->
<!-- lilo-digest sha256:1e3c4258923c2ed9 .lilo/docs/verification-gates.md -->
