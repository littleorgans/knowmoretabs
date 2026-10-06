# knowmoretabs system map
<!-- lilo-page kind=index schema=1 -->
<!-- lilo-covers Cargo.toml Cargo.lock .cargo/config.toml xtask/Cargo.toml xtask/src/main.rs slices.toml src/*.rs web/index.html web/app.css web/app.js -->
<!-- lilo-verified digest=15c21cf74a6ec1a0 commit=140f5db7d6a1 date=2026-10-06 -->

> One Rust binary that snapshots the open tabs of Chromium browsers from their on-disk session files and serves the snapshots as a local, searchable library.

## Purpose and scope

knowmoretabs saves a dated, immutable snapshot of every open window and tab by reading the browser's own session file, so nothing is installed in the browser. The snapshots become a library of every page ever open: search, history per page, forget and restore, tags. `save` and `serve` carry almost all the value; when a decision is close, keep those two excellent (claimed by S1, unverified).

Browsers: Chrome, Chrome Beta, Chrome Canary, Chromium, Brave, Edge and Vivaldi on macOS, Linux and Windows (`code:src/platform.rs`). Arc is refused (`code:src/error.rs#ArcUnsupported`).

Non-goals: sync, accounts, cloud, telemetry, page text unless you ask for it (`content` is opt in, and keyword search over what it stores comes later), tag hierarchy, touching or closing live tabs, modifying browser files (claimed by S1, unverified).

## Responsibilities

- CLI, output and errors: `code:src/cli.rs`, `code:src/main.rs`, `code:src/out.rs`, `code:src/error.rs`.
- Capture: browser and profile discovery, SNSS parsing, the encrypted-sessions preflight, History signals: `code:src/capture.rs`, `code:src/platform.rs`, `code:src/snss.rs`, `code:src/session.rs`, `code:src/staleness.rs`, `code:src/history.rs`, `code:src/model.rs`.
- Archive: root creation, advisory lock, staged writes: `code:src/archive.rs`.
- Library: pages derived across snapshots, user state, export, the library's History record: `code:src/library.rs`, `code:src/library_commands.rs`, `code:src/export.rs`, `code:src/library_history.rs`.
- Serve and triage: the HTTP server and JSON API, forget and restore: `code:src/server.rs`, `code:src/triage.rs`; the frontend `code:web/app.js`, embedded by `code:src/assets.rs`.
- Tags and suggestions: `code:src/tags.rs`, `code:src/prompt.rs`, `code:src/suggestions.rs`.
- Enrich, the opt-in head fetch: `code:src/enrich.rs`, `code:src/metadata_fetch.rs`, `code:src/head.rs`, `code:src/metadata.rs`, `code:src/metadata_writer.rs`; GitHub address parsing and the embedded repository data `code:src/github.rs`, shared with content.
- Shared by the network commands: the URL refusal rules `code:src/guard.rs`, the guarded GET and pacer `code:src/fetch.rs`, the planner and per-host workers `code:src/targets.rs`, the append-only logs `code:src/jsonl.rs`.
- Content, the opt-in page text capture: `code:src/content.rs`, `code:src/content_plan.rs`, `code:src/content_fetch.rs`, `code:src/content_store.rs`, `code:src/extract.rs`; the router `code:src/content_route.rs`, the X post route `code:src/xpost.rs`, the GitHub route through `gh api` `code:src/github_api.rs` with its markdown `code:src/github_page.rs`, and the YouTube route through yt-dlp `code:src/ytdlp.rs` with its video addresses `code:src/youtube.rs` and its caption choice, transcript and markdown `code:src/youtube_page.rs`; external tool discovery and bounded runs `code:src/tools.rs`; the readiness report `code:src/doctor.rs`; classifier parity tool `code:examples/content_parity.rs`.
- Slice lint and matrix generator: `code:xtask/src/main.rs`.

## Reading path

Start here, then [verification gates](verification-gates.md). Before changing a file, read its `//!` header: every `.rs` file under `src/` and `xtask/src/` names its slice and why it exists (`code:xtask/src/main.rs#parse_header`). `rg 'slice:.*triage'` finds every file in a slice.

## Vocabulary

- Archive: the root directory; default `~/.knowmoretabs`, and `%LOCALAPPDATA%\knowmoretabs` on Windows (`code:src/platform.rs#default_root`).
- Snapshot: one dated directory under `snapshots/` holding `snapshot.json`, the source of truth, and `session.snss`, a verbatim copy of the browser's file (`code:src/archive.rs#SNAPSHOT_JSON`, `code:src/model.rs#SESSION_FILE_NAME`).
- Page: one URL as the library lists it, derived across snapshots; a sighting is one appearance of a page in a snapshot (`code:src/library.rs#Library`).
- Library state: `library.json`, per URL user state: forgotten pages, tags and the tag vocabulary (`code:src/library.rs#State`).
- Forget, restore: hide a page from the library, and bring it back; snapshots are untouched (`code:src/triage.rs`).
- Signals: what the browser's History database knows about a URL: visits, typed visits, first and last visit, foreground time, search, referrer (`code:src/history.rs`). First visit means the earliest visit History still retains, about 90 days (claimed by S5, unverified).
- Vocabulary, retire: the owner's tag names; retiring hides a tag everywhere and keeps it (`code:src/tags.rs`).
- Suggestion: a tag an agent proposed, imported into `tags/suggested.jsonl` (`code:src/suggestions.rs#FILE`).
- Slice, capability: a vertical cut that ships user value end to end, and the user want it serves; `code:slices.toml` declares both, plus a future band of what is deliberately not built.

## Rules

- Archive writers hold the archive's advisory lock (`code:src/archive.rs#LOCK_FILE`).
- A snapshot is staged as a directory and renamed into place once, never over anything, and never written again (`code:src/archive.rs#publish`).
- State files such as `library.json` are replaced whole: staged beside the target, then renamed (`code:src/archive.rs#replace_file`).
- `pages/metadata.jsonl` and `pages/content.jsonl` are append only: one whole line per write, under the lock, synced before the next (`code:src/jsonl.rs#Appender`).
- A content file is replaced whole, then its log line appended, under one hold of the lock; an unchanged body leaves the file untouched (`code:src/content_store.rs#Store`).
- On Unix the archive root and its private directories are created mode 0700; on Windows they inherit the parent's ACL (`code:src/archive.rs#create_private_dir`).
- An unknown session command is skipped and counted, never fatal; a torn tail is reported as truncated bytes (`code:src/session.rs`, `code:src/snss.rs`).
- The encrypted-sessions preflight refuses a stale save with exit 3; `--force` cannot bypass it (`code:src/staleness.rs#check`, `code:src/error.rs#exit_code`).
- Tabs on this machine (`localhost`, its subdomains, loopback) are left out of `snapshot.json` and of change detection; `session.snss` stays verbatim and keeps them (`code:src/capture.rs#leave_out_this_machine`, `code:src/local.rs#is_this_machine_url`).
- Only `enrich` and `content` send page addresses off the machine, and `content` sends a video's id, never its library address, to YouTube through yt-dlp, without cookies or yt-dlp's configuration files (`code:src/ytdlp.rs`); `doctor` is offline by default; `--live` lets gh check its own sign in and asks the X post API for one fixed public post, sending nothing of the owner's. `ureq` appears only in `code:src/fetch.rs`.
- Content's external tools run only through `code:src/tools.rs`: `run` uses an argument vector, no shell, no input, a timeout, capped output and stderr cut to one line, its first `ERROR:` line when there is one; `System::exit_code` runs `gh auth status` with only its exit code read (`code:src/github_api.rs#Readiness`).
- `serve` binds 127.0.0.1 only (`code:src/server.rs`).
- JSON is the archive's storage; SQLite, bundled, only reads a copy of the browser's `History` taken with its `-journal` and `-wal` (`code:src/history.rs#COMPANIONS`).
- Foreground time sums positive durations only; the search walk stops at `MAX_HOPS` hops (`code:src/history.rs#MAX_HOPS`).
- Export omits the search and referrer signals unless `--with-history`, and lists no forgotten page (`code:src/library.rs#Shape`).
- Imported suggestions never change the owner's tags; tag names match without regard to case (`code:src/suggestions.rs`, `code:src/library.rs#fold`).
- The frontend is hand-written HTML, CSS and JS with no build step, embedded with `include_str!` (`code:src/assets.rs`); committed files are LF, and Windows CI fails on a checkout that rewrites them (`code:.github/workflows/ci.yml`).
- Unsafe code is forbidden; clippy `all` and `pedantic` deny; `print_stdout` and `print_stderr` are denied, so user output goes through `code:src/out.rs` (`code:Cargo.toml`).
- Committed fixtures are synthetic or redacted, never a real URL or title from a browser; `reference/` is vendored prior art and read-only (claimed by S1, unverified).
- Every `.rs` file under `src/` and `xtask/src/` carries a `slice:` naming a slice in `slices.toml` and a `why:`; the slice matrix doc is generated from `slices.toml` and never hand-edited (`code:xtask/src/main.rs#lint`).

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
- Unmapped: SNSS command tables and pruning, the `serve` JSON API, the frontend state model, `enrich` fetch safety (private address refusal), the release pipeline in `.github/workflows/release.yml`.

## Sources

| id | path | fingerprint | consulted | disposition | claims | watched |
| --- | --- | --- | --- | --- | --- | --- |
| S1 | README.md | 6547a40a3784 | 2026-10-06 | used | index.md#purpose-and-scope, index.md#rules | |
| S2 | docs/SLICES.md | c4e1c70a9f8d | 2026-10-06 | used | index.md#vocabulary | |
| S4 | docs/design-decision.md | d72a87609962 | 2026-10-06 | read, nothing kept | | |
| S5 | docs/briefs/slice-07b-history.md | e583b4ab1abf | 2026-10-06 | used | index.md#vocabulary, index.md#rules | |
