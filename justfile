set positional-arguments

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo run --locked --package xtask -- slices --check
    uv run --locked --project lab/tagger ruff check lab/tagger
    uv run --locked --project lab/tagger ruff format --check lab/tagger
    @echo 'Python type checker: none configured'

test:
    cargo test --workspace --all-targets --locked
    node --test tests/serve_frontend.cjs
    uv run --locked --project lab/tagger pytest -q lab/tagger/tests

install:
    cargo install --path . --locked
    uv tool install --force ./lab/tagger

tagger *args:
    uv run --locked --project lab/tagger tagger app "$@"
