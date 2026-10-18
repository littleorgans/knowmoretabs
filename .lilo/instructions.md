# Repo instructions: knowmoretabs
- Scoped: `cargo test --locked --test <file>`
- Structural: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- Full: `cargo test --workspace --all-targets --locked`
- CI parity: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --all-targets --locked && cargo run --locked --package xtask -- slices --check && node --test tests/serve_frontend.cjs tests/extension.cjs`
- CI pins 1.89.0; run clippy on it too: `python3 scripts/clippy.py`.
- Every `.rs` under `src/` and `xtask/src/` needs a `//!` header with `slice:` and `why:`; run the slices check after changing one.
- The slice matrix doc is generated from `slices.toml` by `cargo xtask slices`; never hand-edit it.
- Never rewrite a snapshot or a browser file; hold the archive lock to write; replace files by stage and rename.
- No `println!`/`eprintln!`: user output goes through `src/out.rs`. No unsafe.
- Frontend in `web/`: no build step, embedded in release, read from disk in debug builds; keep LF.
- Fixtures are synthetic: never commit a real URL or title from a browser. `reference/` is read-only.
Docs: `.lilo/docs/`, start at `index.md`.
- `index.md`: purpose, responsibilities, vocabulary, rules, sources.
- `domain/rules.md`: archive, network, serve and model service invariants.
- `verification-gates.md`: gates, CI jobs, expected failures.
<!-- lilo-onboarding status=complete-quick date=2026-10-06 schema=1 -->
<!-- lilo-digest sha256:389645426b75d453 .lilo/docs/index.md -->
<!-- lilo-digest sha256:32563cd9d1504626 .lilo/docs/verification-gates.md -->
<!-- lilo-digest sha256:692de4a40c1d2301 .lilo/docs/domain/rules.md -->
