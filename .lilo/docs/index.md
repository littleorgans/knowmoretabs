# knowmoretabs system map
<!-- lilo-page kind=index schema=1 -->
<!-- lilo-covers Cargo.toml Cargo.lock .cargo/config.toml xtask/Cargo.toml xtask/src/main.rs slices.toml src/*.rs web/index.html web/app.css web/app.js justfile scripts/clippy.py lab/tagger/tagger/paths.py lab/tagger/tagger/app/kmt.py lab/tagger/tagger/app/add.py lab/tagger/tagger/app/static/pages.js scripts/installation.py extension/manifest.json extension/*.js -->
<!-- lilo-verified digest=a7129be686e7a504 commit=3a0159524068 date=2026-10-13 -->

> One Rust binary that snapshots the open tabs of Chromium browsers from their on-disk session files and serves the snapshots as a local, searchable library.

## Purpose and scope

knowmoretabs saves a dated, immutable snapshot of every open window and tab by reading the browser's own session file, so nothing is installed in the browser. The snapshots become a library of every page ever open: search, history per page, forget and restore, tags. `save` and `serve` carry almost all the value; when a decision is close, keep those two excellent (claimed by S1, unverified).

Browsers: Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and Vivaldi on macOS, Linux and Windows (`code:src/platform.rs`). Arc is refused (`code:src/error.rs#ArcUnsupported`).

Non-goals: sync, accounts, cloud, telemetry, page text unless you ask for it (`content` is opt in), copies of local documents (`add --file` keeps a reference), tag hierarchy, touching or closing live tabs, modifying browser files (claimed by S1, unverified).

## Responsibilities

- CLI, output and errors: `code:src/cli.rs`, `code:src/main.rs`, `code:src/out.rs`, `code:src/error.rs`.
- Capture: browser and profile discovery, SNSS parsing, the encrypted-sessions preflight, History signals: `code:src/capture.rs`, `code:src/platform.rs`, `code:src/snss.rs`, `code:src/session.rs`, `code:src/staleness.rs`, `code:src/history.rs`, `code:src/model.rs`.
- Archive: root creation, advisory lock, staged writes: `code:src/archive.rs`.
- Library: pages derived across snapshots, user state, export, the library's History record: `code:src/library.rs`, `code:src/library_commands.rs`, `code:src/export.rs`, `code:src/library_history.rs`.
- Serve and triage: the HTTP wire `code:src/http.rs`, the routes and JSON API `code:src/server.rs`, forget and restore `code:src/triage.rs`, the drawer's Content tab `code:src/content_view.rs`; the frontend `code:web/app.js`, with the nav and the page shown when the model service cannot answer, loaded by `code:src/assets.rs#load`.
- Model service: the shell's forward to the tagger lab's `tagger app` on a loopback port, through which the Library ranks search and Tag and Add link are served under `/model/`: the forward `code:src/model_service.rs`, its route table `code:src/model_routes.rs`, the proof on each exchange `code:src/service_auth.rs`.
- Add, one page by its address: the command and its stage lines `code:src/add.rs`, the key a new page is kept under `code:src/page_key.rs`, its content stage `code:src/add_content.rs`, the intake log and its fold into the library `code:src/intake.rs`.
- Local documents, kept by reference: `add --file` `code:src/source_add.rs`, document keys `code:src/source_key.rs`, the git questions `code:src/source_git.rs`, the pins log, checkouts file and resolver `code:src/sources.rs`, the one reader of a page's text and `read KEY...` `code:src/page_text.rs`, and `search` over added pages and documents by keyword or the model service `code:src/search.rs`.
- Tags and suggestions: `code:src/tags.rs`, `code:src/prompt.rs`, `code:src/suggestions.rs`.
- Enrich, the opt-in head fetch: `code:src/enrich.rs`, `code:src/metadata_fetch.rs`, `code:src/head.rs`, `code:src/metadata.rs`, `code:src/metadata_writer.rs`; GitHub address parsing and the embedded repository data `code:src/github.rs`, shared with content.
- Shared by the network commands: the URL refusal rules `code:src/guard.rs`, the guarded GET and pacer `code:src/fetch.rs`, the planner and per-host workers `code:src/targets.rs`, the append-only logs and the log-plus-files store `code:src/jsonl.rs`, the JSON-LD walker `code:src/jsonld.rs`.
- Content, the opt-in page text capture: `code:src/content.rs`, `code:src/content_plan.rs`, `code:src/content_fetch.rs`, `code:src/content_store.rs`, `code:src/extract.rs`, PDF text `code:src/content_pdf.rs`; the router `code:src/content_route.rs`, the X post route `code:src/xpost.rs`, the GitHub route through `gh api` `code:src/github_api.rs` with its markdown `code:src/github_page.rs`, and the YouTube route through yt-dlp `code:src/ytdlp.rs` with its video addresses `code:src/youtube.rs` and its caption choice, transcript and markdown `code:src/youtube_page.rs`; the headless tier: which pages are rendered, what a render writes and what waits `code:src/content_headless.rs`, the installed browser run headless, or the owner's running browser attached to `code:src/browser.rs`, the one tab a render opens and the only way to its page `code:src/browser_tab.rs`, its `DevTools` protocol `code:src/cdp.rs`, and the loopback relay every rendered page's connections go through `code:src/socks.rs`; the signed in tier: which pages `--signed-in` opens, the personal apps it never opens, and how the owner's browser is found `code:src/content_signed_in.rs`, `code:src/guard.rs#PERSONAL_APPS`; external tool discovery and bounded runs `code:src/tools.rs`, each program in a process group of its own with the registry and signal handler that end it `code:src/process_tree.rs`; the readiness report `code:src/doctor.rs`; classifier parity tool `code:examples/content_parity.rs`. Each page's preview image: which pages get one and the retries `code:src/content_image.rs`, candidates and the ladder `code:src/image_pick.rs`, candidates from HTML and README markup `code:src/image_page.rs`, the guarded fetch, checks, decode and JPEG `code:src/image_fetch.rs`, the image log and files, and the kept images `serve` names in the library and answers on `GET /api/image/<hex>` `code:src/image_store.rs`.
- Slice lint and matrix generator: `code:xtask/src/main.rs`.
- Developer recipes: `code:justfile` (`check`, `test`, `install`, `tagger`, `up`, `down`); `install` writes a receipt and default `up` refuses an install that does not match it (`code:scripts/installation.py`); `code:scripts/clippy.py` runs clippy on the toolchain CI pins.
- Toolbar extension: a Chrome button that points a Library tab at Add link for the current tab (`code:extension/background.js`, `code:extension/link.js`).
- Tagger lab, a Python prototype outside the binary, tested by the CI lab job: `lab/tagger`. It resolves the archive as `--root`, then `KMT_ROOT`, then `~/.knowmoretabs`, and its data as `--data`, then `KMT_TAGGER_DATA`, then `<root>/tagger` (`code:lab/tagger/tagger/paths.py`); Add link runs `knowmoretabs --root <root>`, the binary `tagger app --kmt-bin` names or else the one on `PATH`, and embeds a local document's text from its `read` (`code:lab/tagger/tagger/app/kmt.py`); Tag and Add link keep tags, names and forgotten state in the archive through the shell: their page actions use its API (`code:lab/tagger/tagger/app/static/pages.js`), while Add link's restore and remove jobs invoke its commands (`code:lab/tagger/tagger/app/add.py`).

## Reading path

Start here, then [verification gates](verification-gates.md). Before changing a file, read its `//!` header: every `.rs` file under `src/` and `xtask/src/` names its slice and why it exists (`code:xtask/src/main.rs#parse_header`). `rg 'slice:.*triage'` finds every file in a slice.

## Vocabulary

- Archive: the root directory: `--root`, then `KMT_ROOT` (empty means unset), then `~/.knowmoretabs`, and `%LOCALAPPDATA%\knowmoretabs` on Windows (`code:src/platform.rs#resolve_root`, `code:src/platform.rs#default_root`).
- Snapshot: one dated directory under `snapshots/` holding `snapshot.json`, the source of truth, and `session.snss`, a verbatim copy of the browser's file (`code:src/archive.rs#SNAPSHOT_JSON`, `code:src/model.rs#SESSION_FILE_NAME`).
- Added page: a page in `pages/added.jsonl`, by address or, for a local document, by its key, written by `add` when new, with a later line if captured text supplies a missing title; the library reads the log as one in-memory snapshot, id `added` (`code:src/intake.rs#retitle`, `code:src/intake.rs#SNAPSHOT_ID`).
- Page: one URL as the library lists it, derived across snapshots; a sighting is one appearance of a page in a snapshot (`code:src/library.rs#Library`).
- Library state: `library.json`, per URL user state: forgotten pages, tags and the tag vocabulary (`code:src/library.rs#State`).
- Forget, restore: hide a page from the library, and bring it back; snapshots are untouched (`code:src/triage.rs`).
- Signals: what the browser's History database knows about a URL: visits, typed visits, first and last visit, foreground time, search, referrer (`code:src/history.rs`). First visit means the earliest visit History still retains, about 90 days (claimed by S5, unverified).
- Vocabulary, retire: the owner's tag names; retiring hides a tag everywhere and keeps it (`code:src/tags.rs`).
- Local document: a markdown, text or PDF file added with `add --file`, keyed `kmt-git:<repo>//<path>` for a committed file in a repository with a portable remote or `kmt-blob:sha256-<hex>` for a loose one (`code:src/source_key.rs#GIT`, `code:src/source_key.rs#BLOB`); its pin, in `pages/sources.jsonl`, names the bytes it was (`code:src/sources.rs#Pin`).
- Checkouts file: `checkouts.json`, per machine and outside the archive, naming each repository's checkout and the shelf loose documents are copied to (`code:src/sources.rs#CHECKOUTS_FILE`).
- Model service: the tagger lab's `tagger app`, which `serve --model-port` and `search --model-port` reach on 127.0.0.1 (`code:src/cli.rs#ModelArgs`).
- Suggestion: a tag an agent proposed, imported into `tags/suggested.jsonl` (`code:src/suggestions.rs#FILE`).
- Slice, capability: a vertical cut that ships user value end to end, and the user want it serves; `code:slices.toml` declares both, plus a future band of what is deliberately not built.

## Rules

- The invariants for the archive, capture, local documents, the network commands, `serve` and the model service, and the build: [rules](domain/rules.md).

## Components

none yet

## Interactions

none yet

## Operations

none yet

## Decisions

none yet

## Gaps

- Quick look map: no component, interaction or decision pages yet.
- Unmapped: SNSS command tables and pruning, the `serve` JSON API, the frontend state model, the tagger lab's internals, the release pipeline in `.github/workflows/release.yml`.

## Sources

| id | path | fingerprint | consulted | disposition | claims | watched |
| --- | --- | --- | --- | --- | --- | --- |
| S1 | README.md | 6547a40a3784 | 2026-10-06 | used | index.md#purpose-and-scope, index.md#rules | |
| S2 | docs/SLICES.md | c4e1c70a9f8d | 2026-10-06 | used | index.md#vocabulary | |
| S4 | docs/design-decision.md | d72a87609962 | 2026-10-06 | read, nothing kept | | |
| S5 | docs/briefs/slice-07b-history.md | e583b4ab1abf | 2026-10-06 | used | index.md#vocabulary, index.md#rules | |
