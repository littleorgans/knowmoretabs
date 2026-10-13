# Rules
<!-- lilo-page kind=focus schema=1 -->
<!-- lilo-covers Cargo.toml .github/workflows/ci.yml xtask/src/main.rs src/*.rs lab/tagger/tagger/app/service_auth.py tests/fixtures/service_auth.json -->
<!-- lilo-verified digest=d98ab6855276f395 commit=3a0159524068 date=2026-10-13 -->

Up: [knowmoretabs system map](../index.md)

The invariants the archive, the network commands, `serve` and the build hold. Terms are defined in the map's [Vocabulary](../index.md#vocabulary).

## Archive

- Archive writers hold the archive's advisory lock (`code:src/archive.rs#LOCK_FILE`).
- A snapshot is staged as a directory and renamed into place once, never over anything, and never written again (`code:src/archive.rs#publish`).
- State files such as `library.json` are replaced whole: staged beside the target, then renamed (`code:src/archive.rs#replace_file`).
- `pages/added.jsonl`, `pages/sources.jsonl`, `pages/metadata.jsonl`, `pages/content.jsonl` and `pages/images.jsonl` are append only: one whole line per write, under the lock, synced before the next (`code:src/jsonl.rs#Appender`).
- A content file or a page image is replaced whole, then its log line appended, under one hold of the lock, never across a request or a decode; an unchanged body or image leaves the file untouched (`code:src/jsonl.rs#Store`, `code:src/content_store.rs#Store`, `code:src/image_store.rs#Store`).
- Opening a log repairs its file's Unix permissions, and opening a file store repairs its own directory and files, under the archive lock, including existing images, without rewriting their bytes (`code:src/archive.rs#make_private`, `code:src/jsonl.rs#Appender`, `code:src/jsonl.rs#Store`).
- On Unix the archive root and its private directories are created mode 0700; on Windows they inherit the parent's ACL (`code:src/archive.rs#create_private_dir`).
- JSON is the archive's storage; SQLite, bundled, only reads a copy of the browser's `History` taken with its `-journal` and `-wal` (`code:src/history.rs#COMPANIONS`).

## Capture and library

- An unknown session command is skipped and counted, never fatal; a torn tail is reported as truncated bytes (`code:src/session.rs`, `code:src/snss.rs`).
- The encrypted-sessions preflight refuses a stale save with exit 3; `--force` cannot bypass it (`code:src/staleness.rs#check`, `code:src/error.rs#exit_code`).
- Tabs on this machine (`localhost`, its subdomains, loopback) are left out of `snapshot.json` and of change detection; `session.snss` stays verbatim and keeps them (`code:src/capture.rs#leave_out_this_machine`, `code:src/local.rs#is_this_machine_url`).
- Foreground time sums positive durations only; the search walk stops at `MAX_HOPS` hops (`code:src/history.rs#MAX_HOPS`).
- Export omits the search and referrer signals unless `--with-history`, and lists no forgotten page (`code:src/library.rs#Shape`).
- Imported suggestions never change the owner's tags; tag names match without regard to case (`code:src/suggestions.rs`, `code:src/library.rs#fold`).
- `add` keeps a new page under its cleaned address: trailing slashes on a non-root path, the fragment (a routing `#/` or `#!` stays) and tracking parameters dropped, and `/` supplied when the root path is absent; every other byte stays as written. An address that cleans the same as a page the library holds resolves to that page's key; no stored key is rewritten (`code:src/page_key.rs#clean`, `code:src/add.rs`).

## Local documents

- `add --file` keeps a committed file in a git repository with a portable remote as `kmt-git:<repo>//<path>`, pinned to its blob and commit; a file with changes or not committed is refused. Any other file, or text on stdin, is copied once to the shelf the checkouts file names and kept as `kmt-blob:sha256-<hex>`; with no shelf it is refused. The archive keeps no copy and no local path (`code:src/source_add.rs`, `code:src/source_key.rs`).
- The pin and the intake line are written under one hold of the lock, the pin first, so a listed document always has a pin; adding again writes a pin only when the bytes changed (`code:src/source_add.rs#command`).
- The checkouts file lives outside the archive, one per machine: `--checkouts`, else `knowmoretabs/checkouts.json` in the config folder; nothing prints it (`code:src/sources.rs#Checkouts`, `code:src/platform.rs#config_file`).
- Every read of a local document resolves its pinned bytes live and says `fresh`, `stale` (behind the checkout's `HEAD`) or `unresolved`, with why; text is read in memory and never written (`code:src/sources.rs#Resolver`).
- `search` reads only `pages/added.jsonl`, `pages/sources.jsonl`, `library.json`, the machine's checkouts file when resolving local documents, and each page's text, never a snapshot or the History record, takes no lock and writes nothing; forgotten pages stay hidden, and `read` answers the same pages, refusing a forgotten one (`code:src/search.rs#command`, `code:src/page_text.rs#read_command`).
- Git runs only through `code:src/tools.rs#run`, asking and never writing, with optional locks off and prompts off (`code:src/source_git.rs#ENV`).

## Network

- Only `enrich`, `content` and `add`, whose content stage is `content` narrowed to one page unless `--no-content`, send page addresses off the machine, and `content` sends a video's id, never its library address, to YouTube through yt-dlp, without cookies or yt-dlp's configuration files (`code:src/ytdlp.rs`); `content` also downloads one preview image per page from the address the page names, which may be on another host, and keeps it in the private archive (`code:src/image_fetch.rs`); `doctor` is offline by default; `--live` lets gh check its own sign in, asks the X post API for one fixed public post and starts the browser once with no page, sending nothing of the owner's. A local document is never sent. Production `ureq` use is confined to `code:src/fetch.rs`.
- A page `content` renders is loaded in the installed browser, headless, on a scratch profile, and every connection it makes goes through the loopback relay `code:src/socks.rs`, opened by `code:src/fetch.rs#connect_public` under the fetcher's public only rule; WebRTC is kept off UDP. Rendered text replaces kept text only when it is longer, and the headless tier never writes `error` (`code:src/content_headless.rs#settle`).
- `content --signed-in` reads only the remote debugging switch in `Local State` and `DevToolsActivePort` to find the owner's running Chrome, connects once per run, and only after the owner allows it; it creates hidden tabs of its own with a session each, never lists, attaches to or closes another tab and never sends `Browser.close` (`code:src/browser_tab.rs`, `code:src/cdp.rs#Command`). It opens only lines a public tier wrote as behind a login, paywalled or blocked, or rendered thin or empty after a 403 (`code:src/content_signed_in.rs#eligible`), never a personal app, and every line it writes says `signed_in` for tier and access. Names are checked before navigation and where a page ends (`code:src/fetch.rs#Fetcher`); Chrome's own redirects, name resolution and subresources are not guarded. `doctor` reads the same two files and never connects.
- Content's external tools run only through `code:src/tools.rs`, the browser only through `code:src/browser.rs`: `run` uses an argument vector, no shell, no input, a timeout, capped output and stderr cut to one line, its first `ERROR:` line when there is one; `System::exit_code` runs `gh auth status` with only its exit code read (`code:src/github_api.rs#Readiness`).
- Every program `content` and `doctor` run, and every tool scratch directory, goes through `code:src/process_tree.rs`: one registry lock covers start and register, retirement and the signal handler, which never takes the archive lock; a timeout or a signal ends the whole group (Unix `killpg`, Windows `taskkill /T /F`), and a signal exits 130. A signal inherited as ignored stays ignored and turns signal cleanup off, warned once.

## Serve and the model service

- `serve` binds 127.0.0.1 only, and every parsed request passes the Host and Origin checks before further body bytes are read; the header read may prefetch body bytes (`code:src/server.rs`, `code:src/http.rs#check_headers`).
- The Library's Content tab reads what `content` recorded, and a local document's pinned text through the checkouts file, and never fetches; it answers only a page the library lists (`code:src/content_view.rs#served`).
- `serve` forwards below `/model/` only the methods and paths the route table admits, each with its type and deadline, to the model service on 127.0.0.1 at `--model-port`; anything else is refused before a connection is made (`code:src/model_routes.rs#classify`, `code:src/model_service.rs#forward`).
- Without a usable `--model-secret-file` nothing is forwarded and search stays by keyword. Each forward carries a fresh nonce, the time and an HMAC of its framed transcript; a reply counts only when its HMAC binds that nonce, the request, status, type, archive identity and exact body, it names this archive and fits its route's type and bounds. Otherwise the shell answers with its own error or unavailable page, never the service's bytes (`code:src/service_auth.rs`, `code:src/model_service.rs#ARCHIVE_HEADER`). `code:lab/tagger/tagger/app/service_auth.py` is the service's side; `code:tests/fixtures/service_auth.json` holds both to the same bytes.
- The frontend is hand-written HTML, CSS and JS with no build step, embedded with `include_str!`; a debug build reads `web/` from the source checkout on every asset request and export, falling back to the embedded copy, so a frontend edit needs a browser refresh, not a rebuild (`code:src/assets.rs#load`); committed files are LF, and Windows CI fails on a checkout that rewrites them (`code:.github/workflows/ci.yml`).
- One nav, one asset table and the content security policy live in `code:src/assets.rs#nav`; an export writes only `app.css` and `app.js` beside its page, never the unavailable page (`code:src/assets.rs#EXPORTED`).

## Build and fixtures

- Unsafe code is forbidden; clippy `all` and `pedantic` deny; `print_stdout` and `print_stderr` are denied, so user output goes through `code:src/out.rs` (`code:Cargo.toml`).
- Committed fixtures are synthetic or redacted, never a real URL or title from a browser; `reference/` is vendored prior art and read-only (claimed by S1, unverified).
- Every `.rs` file under `src/` and `xtask/src/` carries a `slice:` naming a slice in `slices.toml` and a `why:`; the slice matrix doc is generated from `slices.toml` and never hand-edited (`code:xtask/src/main.rs#lint`).
