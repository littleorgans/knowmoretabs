#!/bin/sh
# Snapshot experiment pipeline. Requires explicit KMT_TAGGER_DATA, holding the snapshot
# (or set KMT_TAGGER_SNAPSHOT) and zeroshot/descriptions.json. Never uses library/tagger by default.
set -eu
if [ -z "${KMT_TAGGER_DATA:-}" ]; then
    echo 'run.sh requires KMT_TAGGER_DATA' >&2
    exit 1
fi
snapshot="$(uv run --locked python -c 'from tagger.paths import resolve; print(resolve(None).snapshot)')"
uv run --locked tagger dataset
uv run --locked tagger embed --root "$snapshot"
uv run --locked tagger eval
uv run --locked tagger embed --root "$snapshot" --chunked --model "$(uv run --locked python -c 'from tagger.paths import resolve,read_json; print(read_json(resolve(None).eval / "cv_summary.json")["best"].split("-")[0])')"
uv run --locked tagger eval --final
uv run --locked tagger zeroshot
uv run --locked tagger suggest
uv run --locked tagger cluster
uv run --locked tagger cost
uv run --locked tagger report
