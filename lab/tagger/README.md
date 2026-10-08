# kmt-tagger (prototype)

Local embedding tagger for knowmoretabs, evaluated against the owner's tags. Lives outside the binary and CI.
Every output goes to a data directory outside the repo; nothing derived from an archive is committed.
Page text is read only by `embed` and never printed.

Models: `google/embeddinggemma-2` (text 270M; full model for images) and `Qwen/Qwen3-Embedding-0.6B`, MPS fp32,
weights in the default Hugging Face cache.

## Run

```sh
export KMT_TAGGER_DATA=<thread home>/data   # holds snapshot-2026-10-07/ and zeroshot/descriptions.json
./run.sh                                     # every step below, in order
```

| Step | Command | Writes |
| --- | --- | --- |
| 1 | `uv run --locked tagger dataset` | `dataset/` page records, tag counts |
| 2 | `uv run --locked tagger embed` | `emb/<model>/{A,B}.npy`, `emb/image/` |
| 3 | `uv run --locked tagger eval` | `eval/split.json`, `eval/cv/`, `eval/cv_summary.json` |
| 3b | `uv run --locked tagger embed --chunked --model <best model>`, then `eval` again | `emb/<model>/B8k.npy` |
| 3c | `uv run --locked tagger eval --final` | `eval/final.json` (test split, once) |
| 4 | `uv run --locked tagger zeroshot` | `out/zeroshot.json` |
| 5 | `uv run --locked tagger suggest` | `out/kmt-tagger-<config>.jsonl`, scores CSV, dry run report |
| 6 | `uv run --locked tagger cluster` | `out/clusters.json` |
| 7 | `uv run --locked tagger cost` | `out/cost.json` |
| 8 | `uv run --locked tagger report` | `out/results.md` |

Guided discovery gate (design D1, cheapest test): `uv run --locked tagger gate` writes `guided/gate.json`
(description query plus 5, 10 or 20 answers per tag, at random or by doubt; eg2-B, CV pool folds). Query
vectors are cached under `emb/<model>/queries/`; set `HF_HUB_OFFLINE=1` when the cache may need the model.

Tags as saved searches (test R1): `uv run --locked tagger retrieval` writes `retrieval/retrieval.json` and the
session grids to `retrieval/sessions.jsonl` (eg2 A and B, tag name and description queries, CV pool folds).
A session (`guided/session.py`) sees only the answers for the pages it shows; `guided/oracle.py` alone reads labels.
Search then tag (test S2): `uv run --locked tagger search-tag` writes `search-tag/search_tag.json` and one record
per result set to `search-tag/sets.jsonl`. Each owner tag's query searches a held out fold (top 20 or 50); the
oracle picks the tags held by 3 or more results (plus 2 wrong ones as a variant); `guided/search_tag.decide`
assigns them from fit fold rules alone: zero shot, LR heads (zero shot below 10 fit positives), or in set
(the top share of the set, that share calibrated on fit fold slices). Baseline: the same heads over every tag.
The CLI sets umask 077, so every output is private (0600).

## Search then tag app (P1)

`uv run --locked tagger app --root <snapshot>` serves the prototype on 127.0.0.1 (a free port, or `--port`) and
prints its address. Needs `dataset` and `embed --model eg2` done for that snapshot; runs offline (`HF_HUB_OFFLINE`).
Search (EG2 cosine over B, else A, fused by reciprocal rank with TF-IDF over title, metadata and text; images on
request), pick tags (suggested by S2's mean z ranking, or any of yours), review (S2's supervised rules fitted on
every owner label, zero shot under 10 positives; picked tags the page holds are shown, not asked), export.
Decisions live in `<data>/app/state.json`; export writes `<data>/app/exports/<UTC time>/answers.jsonl` (a
`tag --import` file, source `kmt-tagger-app`, the kept tags per page) and `decisions.jsonl` (page, tag, answer,
the model's precheck). A click on a search result (x on the focused one) marks it not relevant to that query;
Cut here (c) excludes every result below one, and new results that later show below it; Refine ranks again toward
the kept results and away from the excluded ones. Excluded results never reach pick, suggestions or review for that
query, and touch no other query. They live in `<data>/app/not-relevant.json` (query, page, rank when excluded, by
click or cut), apart from the decisions, and are never exported. Open (o on the focused result or the swipe card) shows an http or https page in a new
tab and changes no exclusion or decision. "+ New tag" in the suggestions row takes a name (↵ adds, Esc cancels): one
you have in any case is picked; any other, checked by `tag --import`'s name rules, becomes a zero shot tag (its name
the query, no positives), is picked and is kept in `state.json`. Its kept answers need `tag --import --accept-new`,
which creates it in the archive. The app never writes an archive. `--smoke` prints load and query timings, then exits.
Synthetic archive for trying it: `uv run --locked python tests/synthetic_archive.py <archive> <data>`.

Steps skip work whose output exists; use a fresh data directory to recompute.
`eval --final` reuses its saved test result and refuses a different CV pick.
`KMT_TAGGER_SNAPSHOT` points at a snapshot elsewhere. `suggest` builds the binary
(`cargo build --release --locked`) and dry runs the import against `import-check/`, a copy of the snapshot.

Reproduce: run everything into a fresh data directory, then
`uv run --locked tagger --data <fresh> compare <original>` (metrics only; timings and memory excluded; exit 1 on any difference above 1e-3).

## Leakage boundary

`evaluate.cross_validate` receives only the CV pool rows; `evaluate.score_test_once` is the only scoring path for held out rows.
Every head (`heads.fit_lr`, `fit_knn`, `fit_prior`) sees only its fit rows and tunes C, k and thresholds on inner folds of them.
`zeroshot.run` takes its input choice from `cv_summary.json` and can run before test scoring.

Synthetic regression checks: `uv run --locked pytest`.
