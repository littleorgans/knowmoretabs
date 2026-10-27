# knowmoretabs system map
<!-- lilo-page kind=index schema=1 -->
<!-- lilo-covers Cargo.toml Cargo.lock .cargo/config.toml xtask/Cargo.toml xtask/src/main.rs slices.toml src/*.rs web/index.html web/app.css web/app.js justfile scripts/clippy.py lab/tagger/tagger/paths.py lab/tagger/tagger/app/kmt.py lab/tagger/tagger/app/add.py lab/tagger/tagger/app/static/pages.js scripts/installation.py extension/manifest.json extension/*.js -->
<!-- lilo-verified digest=1b7d4ff6d08124ea commit=d4b973eff38b date=2026-10-27 -->

> One Rust binary that snapshots the open tabs of Chromium browsers from their on-disk session files and serves the snapshots as a local, searchable library.

## Purpose and scope

knowmoretabs saves a dated, immutable snapshot of every open window and tab by reading the browser's own session file, so capture installs nothing in the browser; an optional toolbar extension adds Add link and saving posts bookmarked on X. The snapshots become a library of every page ever open: search, history per page, forget and restore, tags. `save` and `serve` carry almost all the value; when a decision is close, keep those two excellent (claimed by S1, unverified).

Browsers: Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and Vivaldi on macOS, Linux and Windows (`code:src/platform.rs`). Arc is refused (`code:src/error.rs#ArcUnsupported`).

Non-goals: sync, accounts, cloud, telemetry, page text unless you ask for it (`content` is opt in), copies of local documents (`add --file` keeps a reference), tag hierarchy, touching or closing live tabs, modifying browser files (claimed by S1, unverified).

## Responsibilities

Each file's `//!` header says why it exists.

- CLI, output, errors: `code:src/cli.rs`, `code:src/main.rs`, `code:src/out.rs`, `code:src/error.rs`; shared lowercase hex and SHA-256 spelling for file names, keys and MACs `code:src/digest.rs`.
- Capture: `code:src/capture.rs`, `code:src/platform.rs`, `code:src/snss.rs`, `code:src/session.rs`, `code:src/staleness.rs` (encrypted sessions preflight), `code:src/history.rs`, `code:src/model.rs`, `code:src/local.rs` (what counts as this machine); the report a `save` ends with, as lines or JSON `code:src/save_report.rs`.
- Archive: root, advisory lock, staged writes `code:src/archive.rs`.
- Library, forget and restore: `code:src/library.rs`, `code:src/triage.rs`, `code:src/library_commands.rs`, `code:src/export.rs`, `code:src/library_history.rs`.
- Serve: HTTP wire `code:src/http.rs`, routes and JSON API `code:src/server.rs`, the Content tab `code:src/content_view.rs`, frontend `code:web/app.js` loaded by `code:src/assets.rs#load`.
- Model service, the tagger lab's `tagger app` on 127.0.0.1 at `--model-port`, forwarded under `/model/`: `code:src/model_service.rs`, `code:src/model_routes.rs`, `code:src/service_auth.rs`; ranked search replies and passage checks `code:src/model_rank.rs`.
- Add: `code:src/add.rs`, `code:src/page_key.rs`, `code:src/add_content.rs`, intake log `code:src/intake.rs`.
- Local documents: `code:src/source_add.rs`, `code:src/source_key.rs`, `code:src/source_git.rs`, `code:src/sources.rs`; the one page text reader and `read` `code:src/page_text.rs`; `search` `code:src/search.rs`.
- Tags and agent suggestions (`tags/suggested.jsonl`): `code:src/tags.rs`, `code:src/prompt.rs`, `code:src/suggestions.rs`.
- Enrich: `code:src/enrich.rs`, `code:src/head.rs`, the `metadata*` files, GitHub addresses `code:src/github.rs`.
- Network commands, shared: URL refusal `code:src/guard.rs`, guarded GET `code:src/fetch.rs`, per host workers `code:src/targets.rs`, logs and stores `code:src/jsonl.rs`, JSON-LD `code:src/jsonld.rs`.
- Content: entry `code:src/content.rs`, plus the `content_*`, `image_*`, `github_*`, `youtube*` and `ytdlp*` files; text extraction `code:src/extract.rs`; X post route `code:src/xpost.rs`; the headless and signed in tiers' browser `code:src/browser.rs`, `code:src/browser_tab.rs`, `code:src/cdp.rs`, relay `code:src/socks.rs`; tools and process groups `code:src/tools.rs`, `code:src/process_tree.rs`; readiness `code:src/doctor.rs`; parity tool `code:examples/content_parity.rs`.
- X bookmarks: `serve`'s save of a post bookmarked on X `code:src/xbookmark.rs`, the X log and its Library marks `code:src/xbookmark_log.rs`, the extension's health report and its `doctor` line `code:src/xhealth.rs`.
- Toolbar extension: the button points a Library tab at Add link (`code:extension/link.js`); a content script on X hears X's own bookmark clicks and counts page health (`code:extension/x.js`, X markup in `code:extension/xdom.js`); the worker `code:extension/background.js` is the only part that talks to the Library.
- Slice lint and matrix: `code:xtask/src/main.rs`.
- Recipes: `code:justfile`, `code:scripts/installation.py`, `code:scripts/clippy.py`.
- Tagger lab, a Python prototype outside the binary: `lab/tagger`; its archive and data folders `code:lab/tagger/tagger/paths.py`; it runs the binary `tagger app --kmt-bin` names, else the one on `PATH`, and embeds local document text from `read` (`code:lab/tagger/tagger/app/kmt.py`); Tag and Add link change the archive only through the shell's API and commands (`code:lab/tagger/tagger/app/static/pages.js`, `code:lab/tagger/tagger/app/add.py`).

## Reading path

Start here, then [verification gates](verification-gates.md). `rg 'slice:.*triage'` finds every file in a slice (`code:xtask/src/main.rs#parse_header`).

## Vocabulary

- Archive: the root directory: `--root`, then `KMT_ROOT` (empty means unset), then `~/.knowmoretabs`, and `%LOCALAPPDATA%\knowmoretabs` on Windows (`code:src/platform.rs#resolve_root`, `code:src/platform.rs#default_root`).
- Snapshot: one dated directory under `snapshots/` holding `snapshot.json`, the source of truth, and `session.snss`, a verbatim copy of the browser's file (`code:src/archive.rs#SNAPSHOT_JSON`, `code:src/model.rs#SESSION_FILE_NAME`).
- Added page: a page in `pages/added.jsonl`, by address or document key; the library reads the log as one in-memory snapshot, id `added` (`code:src/intake.rs#SNAPSHOT_ID`).
- Page: one URL as the library lists it, derived across snapshots; a sighting is one appearance in a snapshot (`code:src/library.rs#Library`).
- Library state: `library.json`: forgotten pages, tags and the tag vocabulary; retiring a tag hides it everywhere and keeps it (`code:src/library.rs#State`).
- Signals: what the browser's History knows about a URL (`code:src/history.rs`). First visit is the earliest History retains, about 90 days (claimed by S5, unverified).
- Local document: a file added with `add --file`, keyed `kmt-git:<repo>//<path>` or `kmt-blob:sha256-<hex>` (`code:src/source_key.rs#GIT`, `code:src/source_key.rs#BLOB`); its pin in `pages/sources.jsonl` names its bytes (`code:src/sources.rs#Pin`).
- X log: `pages/xbookmarks.jsonl`, a line per bookmark click on X naming the page it saved (`code:src/xbookmark_log.rs#FILE`); extension health: `extension.json` at the archive root (`code:src/xhealth.rs#FILE`).
- Slice, capability: a vertical cut that ships user value, and the want it serves; `code:slices.toml` declares both, plus a future band.

## Rules

- Invariants for the archive, capture, local documents, the network, `serve` and the model service, and the build: [rules](domain/rules.md).

## Components

none yet

## Interactions

none yet

## Operations

none yet

## Decisions

none yet

## Gaps

- Unmapped: SNSS command tables and pruning, the `serve` JSON API, the frontend state model, the tagger lab's internals, the release pipeline in `.github/workflows/release.yml`.

## Sources

| id | path | fingerprint | consulted | disposition | claims | watched |
| --- | --- | --- | --- | --- | --- | --- |
| S1 | README.md | 6547a40a3784 | 2026-10-06 | used | index.md#purpose-and-scope, index.md#rules | |
| S2 | docs/SLICES.md | c4e1c70a9f8d | 2026-10-06 | used | index.md#vocabulary | |
| S4 | docs/design-decision.md | d72a87609962 | 2026-10-06 | read, nothing kept | | |
| S5 | docs/briefs/slice-07b-history.md | e583b4ab1abf | 2026-10-06 | used | index.md#vocabulary, index.md#rules | |
