# kmt-tagger (prototype)

Local embedding tagger for knowmoretabs, evaluated against the owner's tags. Lives outside the binary and CI.
Every output goes to a data directory outside the repo; nothing derived from an archive is committed.
Page text is read only by `embed` and never printed.

Models: `google/embeddinggemma-2` (text 270M; full model for images) and `Qwen/Qwen3-Embedding-0.6B`, MPS fp32,
weights in the default Hugging Face cache.

## Run

From the repository root, run `just install` after each pull, then `tagger app --port 41879` from any
folder. Archive precedence is `--root` > `KMT_ROOT` > `~/.knowmoretabs`;
data precedence is `--data` > `KMT_TAGGER_DATA` > `<resolved root>/tagger`.
The app requires prepared text vectors in that data directory and cached
model weights. Missing or empty data fails before model loading or writes.
Empty environment variables mean unset. `tagger embed --model eg2` prepares
the app's stores from the resolved live archive without needing `dataset/`.
Current keyed manifests never read experiment dataset files or legacy bodies.
Web assets ship in the installed package; knowmoretabs is found on `PATH`.

The experiment pipeline still runs from `lab/tagger`. Its data directory
must hold the snapshot and `zeroshot/descriptions.json`:

```sh
export KMT_TAGGER_DATA=<experiment data>   # required; run.sh refuses unset or empty
./run.sh                                     # every step below, in order
```

| Step | Command | Writes |
| --- | --- | --- |
| 1 | `uv run --locked tagger dataset` | `dataset/` page records, tag counts |
| 2 | `uv run --locked tagger embed --root <snapshot>` | `emb/<model>/{A,B}/` keyed stores, `emb/image/` |
| 3 | `uv run --locked tagger eval` | `eval/split.json`, `eval/cv/`, `eval/cv_summary.json` |
| 3b | `uv run --locked tagger embed --root <snapshot> --chunked --model <best model>`, then `eval` again | `emb/<model>/B8k.npy` |
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

`just up` serves the prototype on 127.0.0.1:41879 (`tagger app --port 0` picks a free one)
and prints its address. Needs `tagger embed --model eg2` done; runs offline (`HF_HUB_OFFLINE`). No `dataset/`
directory is needed. Page records come from the resolved archive; matching keyed vectors are reused, and
changed or missing inputs are embedded at startup. With no owner vocabulary the app starts with no owner tags; every tag name `state.json`
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
picking yours in any case or making one by knowmoretabs' tag name rules (kept in `state.json`). A tile chip's ×
takes that tag off that page. A tag change offers Undo ("Added Trains to 38 pages"), which retracts that action's
decisions, so every page is exactly as before. Over a tile's picture (hover, focus, always on touch): Forget (red,
`f`), Open (`o`, an http or https page in a new tab) and Pin (`p`); none selects. Forget takes the page out of every
search, ≈, the selection, the pins and the counts at once ("Forgotten", Undo puts it back in its place, selected and
pinned as it was). Pin parks a page for later; "Pinned N" beside the selection shows the pinned pages as a view (again,
Esc or Back returns to the results). Keys: `/` search, space or `x` select, `o` open, `f` forget, `p` pin, `u` or
Ctrl+Z undo while the message shows, Esc leaves ≈ or Pinned (else clears the selection), `?` help.
Decisions live in `<data>/app/state.json` (review sessions, and direct decisions: page, tag, kept, time, batch); the
selection, the pins and the forgotten pages in `selection.json`, `pinned.json` and `forgotten.json` beside it.
Send to library (`e`) makes the archive the app runs on match the app, through knowmoretabs
(`tagger/app/send.py`). The app is the source of truth: every name made or decided on here joins the library's
vocabulary unless it holds the name, active or retired; every library page ends with exactly the app's tags for it,
as the owner's own (a page the app holds no tag for ends with none; a Library edit is overwritten); a "no" here
becomes a dismissal where the library holds that name on neither list for the page (a retired name stays retired);
pages forgotten here are forgotten in the library. Send never restores: pages the library forgot stay forgotten,
including pages undone here after a send; restore them in the Library. Pages the app holds that the library does not
are skipped. Its dialog first shows what will change, computed from the archive and writing nothing: tags added,
tags removed (and of those, on pages never tagged here), pages cleared, dismissals, new tag names, names back from
retirement, pages to forget, pages forgotten in the library only, pages not in the library. Send then writes the
record, `<data>/app/exports/<UTC time>/`: `answers.jsonl` (source `kmt-tagger-app`, kept tags per page, forgotten
pages left out), `decisions.jsonl` (page, tag, answer, the model's precheck and session, both null for a direct
decision) and, when pages were forgotten, `forget.urls` (exact addresses, each ended by a NUL). Then it runs
`knowmoretabs --root <archive> tags --create=<name>…`, then `tag --add=<name> -- <urls>` per tag, then
`tag --remove=<name> -- <urls>` for tags coming off and for dismissals, then `forget -- <urls>`, each call atomic,
with a 120 s timeout. A failed call stops the send and shows knowmoretabs' error: the calls before it stay applied.
Nothing is remembered between sends, so sending again plans afresh and converges. The record's paths and the
commands sit under Details. Refused on a snapshot copy inside the data directory.
`knowmoretabs serve` reads the archive on every request: reload the Library to see a send. Pins are not sent.
Add link (`#add`; `#add=<encoded link>&title=<encoded title>` fills the box and focuses Add, which waits for Enter):
Add, Enter, or a paste of a web address into the empty box starts a job that runs
`knowmoretabs --root <archive> add --json [--title=<title>] -- <link>` (the binary from PATH; the title only for the
deep link's own link), relays its stage events (`GET /api/add/<id>`, polled every 250 ms) and then indexes the page
(`engine.sync`). Four segments show Library, Content, Image and Search; a failure shows its value and one action. Try
signed in (`add --signed-in`) follows a blocked, login or paywalled read on a public tier. Retry runs only the failed
stages in one run: `--retry content` after an error or a timeout, `--retry content --signed-in` when Chrome was out of
reach or not allowed, `--retry image` after an image error (`--retry` needs knowmoretabs quick add step C). `Unavailable`, `Not recorded` and every other outcome
stand. Retry after a job that ended without a result asks for the same again, except that a Restore that held is not
repeated; Retry after Not indexed indexes the page in the lab only. Remove is `forget`, Restore is `restore` then `add`.
Bookmarklet (Chrome's bookmarks bar):
`javascript:void(window.open('http://127.0.0.1:41879/#add='+encodeURIComponent(location.href)+'&title='+encodeURIComponent(document.title),'kmt-tagger'))`.
From the moment the box holds a web address the page's tile (a placeholder until the library lists the page) and its
tag panel show (`static/panel.js`): Suggested and Recently used rows of one line each (empty for now), a box that
filters the app tags or, on Enter with no match, makes one, and every app tag folded under "All N tags" in letter rows
(`#` first, letters without tags hidden, case and accents folded) of same width cells. A cell is that page's tag, on or
off; no stage, pick or write moves a cell. Tags picked before the page has a row show striped, go with the link when
it is added and are written once Indexed gives it a row, as one change with one Undo; a newer link does not stop an
older job writing its own. A Library refusal (forgotten, not a web page, a snapshot) clears them; Not indexed keeps
them for Retry. They live in this tab: a reload before Indexed loses them. knowmoretabs is the only writer of the archive, so the app must run on the archive `add` should
change. Page images are addressed by their hashed name (`/img/<sha256>`), so a cached image stays with its page.
The P1 to P4 review, swipe, tag picker, suggestions and query exclusions (`<data>/app/not-relevant.json`) keep
their code, API and tests but are not on this screen; their decisions still count as app tags.
`--smoke` prints load and query timings, then exits.
The grid renders with lit-html (keyed by page, so a tile keeps its node across updates), and so does the Add link
panel (keyed by tag name, so a clicked cell keeps the focus); the other parts of the screen are plain DOM. `static/lit-html.js` is lit-html 3.3.3 (BSD-3-Clause, licence comments kept), from the npm
tarball `lit-html-3.3.3.tgz` (sha512 matches the registry's integrity), bundled to one ES module of `html`, `render`,
`nothing`, `repeat` and `unsafeHTML` with `esbuild@0.28.2 --bundle --format=esm --minify --legal-comments=inline`;
10,129 bytes, sha256 `afcf956778ba22a4d5f62b2192746610a89216ed391088073f9bd10ff748187f`. No eval, so it runs under
the app's CSP (`script-src 'self'`). `static/lit.js` hands it to the classic scripts, all deferred to keep their order.
Synthetic archive for trying it: `uv run --locked python tests/synthetic_archive.py <archive> <data>`.

Steps skip work whose output exists; use a fresh data directory to recompute.
`eval --final` reuses its saved test result and refuses a different CV pick.
`KMT_TAGGER_SNAPSHOT` points at a snapshot elsewhere. `suggest` builds the binary
(`cargo build --release --locked`) and dry runs the import against `import-check/`, a copy of the snapshot.

Reproduce the snapshot experiment from `lab/tagger`: set `KMT_TAGGER_DATA` to a fresh
data directory containing the snapshot, or set `KMT_TAGGER_SNAPSHOT` to its original
path. Run `./run.sh`; both embed steps pass that snapshot explicitly as `--root`.
The pipeline refuses missing or empty `KMT_TAGGER_DATA` and never defaults to library/tagger. Then run
`uv run --locked tagger --data <fresh> compare <original>` (metrics only; timings and memory excluded; exit 1 on any difference above 1e-3).

## Leakage boundary

`evaluate.cross_validate` receives only the CV pool rows; `evaluate.score_test_once` is the only scoring path for held out rows.
Every head (`heads.fit_lr`, `fit_knn`, `fit_prior`) sees only its fit rows and tunes C, k and thresholds on inner folds of them.
`zeroshot.run` takes its input choice from `cv_summary.json` and can run before test scoring.

Synthetic regression checks: `uv run --locked pytest`. `tests/test_frontend.py` runs the scripts in Node with a
string DOM (`tests/frontend_dom.cjs`; the Add link cases in `tests/frontend_add.cjs`); `tests/test_browser.py` runs the screens in headless Chrome (`tests/browser.cjs`), skipped without one.
Add link tests drive a fake knowmoretabs (`tests/fake_knowmoretabs.py`) on a copy of the synthetic archive.
