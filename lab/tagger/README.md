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

## Search, select and tag app (P1, one screen since P5)

`uv run --locked tagger app --root <archive>` serves the prototype on 127.0.0.1:7879 (`--port 0` picks a free one)
and prints its address. Needs `dataset` and `embed --model eg2` done; runs offline (`HF_HUB_OFFLINE`). The dataset
must be a subset of the archive's known pages (snapshot tabs plus `pages/added.jsonl`, folded as knowmoretabs folds
it); known pages it lacks are built from `--root` and embedded at startup (`engine.add_pages`, B vector only, no
image vector, unlabelled). With no owner vocabulary the app starts with no owner tags; every tag name `state.json`
holds (made in the app, or in any decision) loads as a zero shot tag.
One screen: search (EG2 cosine over B, else A, fused by reciprocal rank with TF-IDF over title, metadata and text;
images on request; "Untagged only" keeps pages with no app tag), click results to select them, tag the selection.
Results come 50 at a time ("51 to 100 of 240", of every page the search ranks); Previous and Next replace them.
App tags are the tags made in this app, applied directly or kept in an earlier review; archive tags stay out of the
screen. The sticky strip above the grid lists the app tags in view, each `[× | Trains 26/38 | ≈]`, acting on the
selection only: the name adds the tag to every selected page (filled when all have it, filled in part when some do);
× takes it off every selected page that has it (shown when one does); the count is how many of the selection have
it (hidden with none selected); ≈ ("pages like this, without this tag") ranks pages without that app tag by refine's
prototype (the tag's name as the query, toward its pages, away from pages it was taken off) and is a toggle: ≈
again, Esc or Back returns to the search, page, scroll and selection intact. "+ New tag" applies a tag to the selection,
picking yours in any case or making one by `tag --import`'s name rules (kept in `state.json`). A tile chip's ×
takes that tag off that page. A tag change offers Undo ("Added Trains to 38 pages"), which retracts that action's
decisions, so every page is exactly as before. Over a tile's picture (hover, focus, always on touch): Forget (red,
`f`), Open (`o`, an http or https page in a new tab) and Pin (`p`); none selects. Forget takes the page out of every
search, ≈, the selection, the pins and the counts at once ("Forgotten", Undo puts it back in its place, selected and
pinned as it was). Pin parks a page for later; "Pinned N" beside the selection shows the pinned pages as a view (again,
Esc or Back returns to the results). Keys: `/` search, space or `x` select, `o` open, `f` forget, `p` pin, `u` or
Ctrl+Z undo while the message shows, Esc leaves ≈ or Pinned (else clears the selection), `?` help.
Decisions live in `<data>/app/state.json` (review sessions, and direct decisions: page, tag, kept, time, batch); the
selection, the pins and the forgotten pages in `selection.json`, `pinned.json` and `forgotten.json` beside it.
Export writes `<data>/app/exports/<UTC time>/answers.jsonl` (a `tag --import` file, source `kmt-tagger-app`, the kept
tags per page, latest decision winning, forgotten pages left out), `decisions.jsonl` (page, tag, answer, the model's
precheck and session, both null for a direct decision) and, when pages were forgotten, `forget.urls` (their exact
addresses, each ended by a NUL). Created tags need `tag --import --accept-new`, which creates them in the archive; the
forgotten pages go through `xargs -0 knowmoretabs forget -- < forget.urls` (`restore` in place of `forget` brings
them back). Both dialog commands target your default library, independently of the app's snapshot root.
Pins are not exported. The app never writes an archive itself.
Add link (`#add`; `#add=<encoded link>` fills the box and focuses Add, which waits for Enter): Add, Enter, or a
paste of a web address into the empty box starts a job that runs `knowmoretabs --root <archive> add --json -- <link>`
(the binary from PATH), relays its stage events (`GET /api/add/<id>`, polled every 250 ms) and then indexes the page
(`engine.sync`). Four segments show Library, Content, Image and Search; a failure shows its value and one action:
Try signed in (`add --signed-in`), Retry (the same add again), Remove (`forget`) or Restore (`restore`, then `add`).
The tile is framed once the library lists the page and opens at Indexed with every app tag in the strip, acting on
that page only. knowmoretabs is the only writer of the archive, so the app must run on the archive `add` should
change. Page images are addressed by their hashed name (`/img/<sha256>`), so a cached image stays with its page.
The P1 to P4 review, swipe, tag picker, suggestions and query exclusions (`<data>/app/not-relevant.json`) keep
their code, API and tests but are not on this screen; their decisions still count as app tags.
`--smoke` prints load and query timings, then exits.
The grid renders with lit-html (keyed by page, so a tile keeps its node across updates); the other parts of the
screen are plain DOM. `static/lit-html.js` is lit-html 3.3.3 (BSD-3-Clause, licence comments kept), from the npm
tarball `lit-html-3.3.3.tgz` (sha512 matches the registry's integrity), bundled to one ES module of `html`, `render`,
`nothing`, `repeat` and `unsafeHTML` with `esbuild@0.28.2 --bundle --format=esm --minify --legal-comments=inline`;
10,129 bytes, sha256 `afcf956778ba22a4d5f62b2192746610a89216ed391088073f9bd10ff748187f`. No eval, so it runs under
the app's CSP (`script-src 'self'`). `static/lit.js` hands it to the classic scripts, all deferred to keep their order.
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

Synthetic regression checks: `uv run --locked pytest`. `tests/test_frontend.py` runs the scripts in Node with a
string DOM; `tests/test_browser.py` runs the screens in headless Chrome (`tests/browser.cjs`), skipped without one.
Add link tests drive a fake knowmoretabs (`tests/fake_knowmoretabs.py`) on a copy of the synthetic archive.
