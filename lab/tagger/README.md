# kmt-tagger (prototype)

Local embedding tagger for knowmoretabs, evaluated against the owner's tags. Lives outside the binary and CI.
Every output goes to a data directory outside the repo; nothing derived from an archive is committed.
Page text is read only by `embed` and never printed.

Models: `google/embeddinggemma-2` (text 270M; full model for images) and `Qwen/Qwen3-Embedding-0.6B`, MPS fp32,
weights in the default Hugging Face cache.

## Run

From the repository root, run `just install` after each pull, then `just up`; `tagger app --port 41879
--secret-file <key>` runs from any folder, for a Library given the same key. Archive precedence is `--root` > `KMT_ROOT` > `~/.knowmoretabs`;
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

App suggestion quality: from the repository root, run `uv run --locked --project lab/tagger tagger --root <archive-copy>
--data <archive-copy>/tagger tag-eval --split <private>/split.json --out <private>`. This evaluates every tagged page
once in five saved page folds, using the app's B then A vectors, LR heads and cosine fallback, and `engine.suggest`.
Training pages alone set fitted rules and ranking standardization. Outputs are `baseline.json`, `baseline.md` and
`baseline-logs/tag-eval.log`; repeats reuse the split and require a fresh output directory. `tagger/tag_eval.py`
exposes `PAGE_SOURCES` and `STRATEGIES` registries for adding experiments against the same split and metrics.
The CLI sets umask 077, so every output is private (0600).

With the saved baseline and split, run the source and scorer grid from the repository root:
`uv run --locked --project lab/tagger tagger --root <archive-copy> --data <archive-copy>/tagger tag-eval
--split <private>/split.json --out <grid-output> --grid-baseline <private>/baseline.json`.
Outputs are `grid.json`, `grid.md` and `grid-logs/tag-grid.log`.
For calibration, abstention and paired C comparisons, run
`uv run --locked --project lab/tagger tagger --root <archive-copy> --data <archive-copy>/tagger tag-eval
--split <private>/split.json --out <calib-output> --calib-baseline <private>/baseline.json`.
Outputs are `calib.json`, `calib.md` and `calib-logs/tag-calib.log`. Both studies reuse the saved split,
require a fresh output directory and validate the baseline's inputs. Calibration reports fixed app and knn
abstention and fusion scorers without choosing a winner from held out precision.

## Search, select and tag app (P1, one screen since P5)

`just up` runs the prototype on 127.0.0.1:41879 (`tagger app --port 0` picks a free one) behind the Library, which
serves it below `http://127.0.0.1:41878/model/` with Tag and Add link in its nav. The app answers only requests the
Library proves with the launch's key (`--secret-file`), so a browser never reaches it directly; Add link runs the
knowmoretabs `--kmt-bin` names, which `just up` sets to the one serving the Library. Needs `tagger embed --model eg2` done; runs offline (`HF_HUB_OFFLINE`). No `dataset/`
directory is needed. Page records come from the resolved archive; matching keyed vectors are reused, and
changed or missing inputs are embedded at startup. With no owner vocabulary the app starts with no owner tags.
Every tag, dismissal, name and forgotten page is the library's: the screens write them through the Library's own API
(`POST /api/tags`, `/api/vocabulary`, `/api/forget`, `/api/restore`), and the app follows `library.json` before each
request that shows them, reading it again only when its inode, size or mtime moved. A name the vocabulary gains joins
as a zero shot tag, its name as the query; a retired one leaves every list and comes back in its own column.
Owner tag changes refit heads, thresholds and the score SD used by the Add margin in the background, using the
cached vectors. Requests use the previous fit until the replacement is ready, then switch to it together.
Changes during a fit coalesce into one follow up on the latest tags; an outdated fit is discarded. Vectors and the
keyword index are unchanged by a tag decision. A file it cannot read answers 503 and is read again on the next request.
One screen: search (EG2 cosine over B, else A, fused by reciprocal rank with TF-IDF over title, metadata and text;
images on request; "Untagged only" keeps pages with no tag), click results to select them, tag the selection.
Results come 50 at a time ("51 to 100 of 240", of every page the search ranks); Previous and Next replace them.
The sticky strip above the grid lists the tags in view, each `[× | Trains 26/38 | ≈]`, acting on the selection only:
the name adds the tag to every selected page (filled when all have it, filled in part when some do); × takes it off
every selected page that has it (shown when one does); the count is how many of the selection have it (hidden with
none selected); ≈ ("pages like this, without this tag") ranks pages without that tag by refine's prototype (the tag's
name as the query, toward its pages, away from pages it was taken off or turned down on) and is a toggle: ≈ again,
Esc or Back returns to the search, page, scroll and selection intact. "+ New tag" puts a tag on the selection, the
library making the name or finding it in any case and answering its spelling. A tile chip's × takes that tag off that
page. A tag change shows once the library answers, with Undo ("Added Trains to 38 pages"), which sends the library's
own record of that write back, so every page it touched is exactly as before, whatever was written since; a write
that fails is said and leaves every page as it was. Over a tile's picture (hover, focus, always on touch): Forget
(red, `f`), Open (`o`, an http or https page in a new tab) and Pin (`p`); none selects. Forget has the library forget
the page, which then leaves every search, ≈ and count; it keeps its place in the selection and the pins, out of
sight, and Undo restores it there ("Forgotten"). Pin parks a page for later; "Pinned N" beside the selection shows
the pinned pages as a view (again, Esc or Back returns to the results). Keys: `/` search, space or `x` select, `o`
open, `f` forget, `p` pin, `u` or Ctrl+Z undo while the message shows, Esc leaves ≈ or Pinned (else clears the
selection), `?` help.
The selection and the pins live in `<data>/app/selection.json` and `pinned.json`, results marked not relevant in
`not-relevant.json`.
`knowmoretabs serve` reads the archive on every request: reload the Library to see a change made here.
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
`javascript:void(window.open('http://127.0.0.1:41878/#add='+encodeURIComponent(location.href)+'&title='+encodeURIComponent(document.title),'kmt-tagger'))`
(the Library hands the fragment, whole, to Add link).
Toolbar button (`extension/`, Chrome): in `chrome://extensions` turn on Developer mode, then Load unpacked and pick
the `extension` folder. A click opens the Library tab already open on port 41878, else a new one, at Add link with the
tab's address and title filled in; a page that is not on the web (`chrome://`, `file://`) opens Add link empty. It asks
for `activeTab`, to read the clicked tab, and for `http://127.0.0.1:41878/*`, to find the Library tab, nothing else.
From the moment the box holds a web address the page's tile (a placeholder until the library lists the page) and its
tag panel show (`static/panel.js`): Suggested, one line, and Recently used, three lines held open, a box that
filters the tags or, on Enter with no match, has the library make one, and every active name folded under "All N tags" in letter rows
(`#` first, letters without tags hidden, case and accents folded) of same width cells. A cell is that page's tag, on or
off; no stage, pick or write moves a cell. At Indexed, Suggested fills with the active names the model would put on
the page (`App.suggested`: `engine.prechecks`, those clearing their threshold plus `ADD_K_SD` of their score SD, by confidence), less those it holds,
has turned down or was picked for, as many as its line holds, and stays put while the page is on screen. Recently used
holds the active names by last use, latest first, as many as three rows of its columns fit, nine at most: a use is an add the library records as
the term's `used_at`, which removing the tag or undoing the add leaves as it was; a write moves its tag to the front.
Tags picked before the page has a row show striped, go with the link when it is added and are written once Indexed
gives it a row, as one library write for the key Indexed names with one Undo; a newer link does not stop an older
job writing its own, and a write that fails is said and never sent again. A Library refusal (forgotten, not a web page, a snapshot) clears them; Not indexed keeps
them for Retry. They live in this tab: a reload before Indexed loses them. knowmoretabs is the only writer of the archive, so the app must run on the archive `add` should
change. Page images are addressed by their hashed name (`/img/<sha256>`), so a cached image stays with its page.
The query exclusions (`/api/exclusions`, `<data>/app/not-relevant.json`) and refine keep their API and tests but have
no control on this screen; the P1 to P4 review, swipe and tag picker are gone.
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

## Startup after a chunker or model change

When Store C already exists, a change to the chunker (`chunks.CHUNKER`) or embedding model re-embeds every
chunk of its visible pages at startup. A model change first refreshes the selected text store: B if present,
otherwise A. The app listens only after startup work finishes. Until then the Library silently uses keyword
search, and Tag and Add link are unavailable.

`just up` waits 120 seconds for the port, then reports a failure and exits 1 while the rebuild continues.
Wait for the configured tagger port (41879 by default) to listen. A store publishes only after rebuilding;
restarting an unfinished rebuild repeats its work. A previous install report recorded about 35 minutes for
Store C on about 1,650 pages (2026-10-24); that timing is unverified here. Rebuild work depends on chunk count.

If Store C fails to rebuild at startup, the app serves without it. The next Add link sync retries under the
app lock. Library ranking answers busy and the Library uses keyword search; Tag API requests wait and may
time out. That job's Search stage waits for the retry to finish.

## Leakage boundary

`evaluate.cross_validate` receives only the CV pool rows; `evaluate.score_test_once` is the only scoring path for held out rows.
Every head (`heads.fit_lr`, `fit_knn`, `fit_prior`) sees only its fit rows and tunes C, k and thresholds on inner folds of them.
`zeroshot.run` takes its input choice from `cv_summary.json` and can run before test scoring.

Synthetic regression checks: `uv run --locked pytest`. `tests/test_frontend.py` runs the scripts in Node with a
string DOM (`tests/frontend_dom.cjs`; the Add link cases in `tests/frontend_add.cjs`); `tests/test_browser.py` runs the screens in headless Chrome (`tests/browser.cjs`), skipped without one.
Add link tests drive a fake knowmoretabs (`tests/fake_knowmoretabs.py`) on a copy of the synthetic archive.
