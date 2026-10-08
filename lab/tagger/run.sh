#!/bin/sh
# Every step in order. The resolved data directory must hold the snapshot (or set
# KMT_TAGGER_SNAPSHOT) and zeroshot/descriptions.json. Each step skips work whose output exists.
set -eu
uv run --locked tagger dataset
uv run --locked tagger embed
uv run --locked tagger eval
uv run --locked tagger embed --chunked --model "$(uv run --locked python -c 'from tagger.paths import resolve,read_json; print(read_json(resolve(None).eval / "cv_summary.json")["best"].split("-")[0])')"
uv run --locked tagger eval --final
uv run --locked tagger zeroshot
uv run --locked tagger suggest
uv run --locked tagger cluster
uv run --locked tagger cost
uv run --locked tagger report
