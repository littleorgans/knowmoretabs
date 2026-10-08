set positional-arguments

check:
    cargo fmt --all -- --check
    python3 scripts/clippy.py
    cargo run --locked --package xtask -- slices --check
    uv run --locked --project lab/tagger ruff check lab/tagger
    uv run --locked --project lab/tagger ruff format --check lab/tagger
    @echo 'Python type checker: none configured'

test:
    cargo test --workspace --all-targets --locked
    node --test tests/serve_frontend.cjs
    uv run --locked --project lab/tagger pytest -q lab/tagger/tests

install:
    #!/usr/bin/env sh
    set -eu
    cargo install --path . --locked
    constraints="$(mktemp "${TMPDIR:?TMPDIR must name a scratch directory}/kmt-tagger-constraints.XXXXXX")"
    uv export --locked --project lab/tagger --no-dev --no-emit-project --no-hashes --output-file "$constraints" > /dev/null
    uv tool install --force --constraints "$constraints" ./lab/tagger

tagger *args:
    uv run --locked --project lab/tagger tagger app "$@"
