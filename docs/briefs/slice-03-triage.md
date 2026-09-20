# Slice 3 — Serve and triage

**Ships:** `knowmoretabs serve`, `knowmoretabs forget`, `knowmoretabs restore`.
The Forget button becomes real. This is the reason the project stopped being a
script.

## Required reading

1. `docs/BRIEF.md` — all of it, especially §2 principles and §5 storage.
2. `slices.toml` — slice `triage`, and the `future` band.
3. **`web/fixtures/serve.py`** — a working Python stand-in for the server you
   are about to write. It defines the API surface exactly, and the shipped
   frontend already drives it. **Treat it as the specification.**
4. `web/NOTES.md` §5 — the JSON contract.
5. `docs/research/rust-stack.md` §5 and §6 — the HTTP crate decision and asset
   embedding.
6. `src/library.rs`, `src/export.rs`, `src/assets.rs` — what already exists.
   `export` already renders the same assets; `serve` is the other host.

## In scope

- `knowmoretabs serve [--port N] [--open]`, binding **127.0.0.1 only**.
- `GET /` and the static assets, from the same `web/` files `export` embeds.
- `GET /api/library` — the contract document in its **serve shape**: forgotten
  pages present and flagged, not omitted.
- `POST /api/forget` and `POST /api/restore` — both take `{"urls":[...]}` and
  return `{"urls":[...],"counts":{...}}`. Same call with the sign flipped;
  that symmetry is what makes the frontend's undo four lines. Do not break it.
- `knowmoretabs forget <URL>...` and `restore <URL>...` — the same operation
  headlessly. These write `library.json`.
- `library.json` gains its write path. Slice 2a reads it already.

## Out of scope

Tags, notes, search on the server, any second page, authentication beyond §
Security below, SQLite, TLS, a daemon or background mode, opening or closing
tabs in the real browser, any new browser or platform. `slices.toml`'s
`notes-tags` and `scale-index` rows are recorded decisions — do not pre-build
for them.

## Security — the part most likely to be done badly

A server on localhost is reachable by **any web page the user has open**. A
page on the public internet can `fetch('http://127.0.0.1:7878/api/forget', …)`,
and with DNS rebinding it can do so while passing a naive origin check. Our
POST endpoints mutate the user's library, and `GET /api/library` discloses
every URL they have ever had open. That is the whole threat model and it is
not hypothetical.

Required, all three:

1. **Bind `127.0.0.1` explicitly.** Never `0.0.0.0`, never a hostname.
2. **Validate the `Host` header** against `127.0.0.1:<port>` and
   `localhost:<port>`. Anything else gets a 403. This is what stops DNS
   rebinding, and it must apply to `GET /api/library` too, not just the POSTs.
3. **Reject cross-origin requests.** If an `Origin` header is present and is
   not our own origin, 403. A same-origin `fetch` from our own page sends no
   `Origin` on GET and our own origin on POST, so this costs us nothing.

Also: no CORS headers, ever. `Access-Control-Allow-Origin` is the opposite of
what we want. Requests with a body larger than a sane cap are rejected before
allocation. Malformed JSON gets a 400, not a panic. Write a test for each of
the three rules above — a security control with no test is a comment.

Print the URL on startup and say plainly that it is local-only.

## Durability

`library.json` is user state and the same rules apply as to snapshots: atomic
write via temp-and-rename, and the archive lock held across read-modify-write
so two concurrent forgets cannot lose one another. Slice 1's `src/archive.rs`
already has both; reuse them rather than writing a second implementation.

**Forgetting never touches a snapshot.** It is a filter over the library, and
that is the promise the interface makes to the user at the point of action.

Forgetting a URL that is not in the library is an error for the CLI — the
reference implementation got this right and its message was good. Through the
API, decide what the frontend actually needs and say why in your report.

## The HTTP crate

`docs/research/rust-stack.md` §5 chose `tiny_http`: 5 dependency packages
against 48 for `axum` + `tokio`, blocking, no async runtime. It also recorded
that **`tiny_http` is dormant** — no recent releases.

Before you build on it, spend a few minutes confirming that is still the right
call now that the code is real rather than hypothetical. The alternative worth
weighing is writing the HTTP/1.1 handling directly on `std::net::TcpListener`:
for six routes on localhost that is perhaps 200 lines with zero dependencies,
against a dormant crate parsing untrusted-ish input. Either answer is
defensible. **Make the call, implement it, and justify it in one paragraph in
your report.** Do not add `axum`.

## Tests

- Each of the three security rules, rejected with the right status.
- Round trip: serve a real archive, `GET /api/library`, assert the serve shape
  (forgotten present and flagged), POST forget, GET again, assert it flipped.
- Forget then restore returns to the original state exactly.
- Forgetting the same URL twice is idempotent.
- Two concurrent forgets of different URLs both survive.
- A malformed `library.json` behaves as slice 2a decided — read
  `src/library.rs` and be consistent with it rather than inventing a second
  policy.
- Oversized body, malformed JSON, unknown route, wrong method — all handled,
  none panic.
- The CLI `forget`/`restore` commands, including the not-in-library error.
- Assets are served with correct content types and no network-implying headers.

## Verification beyond tests

`web/fixtures/serve.py` exists so the frontend could be exercised before the
real server existed. **Point the real frontend at your real server and drive
it**: forget three pages, undo, bulk select, restore from the Forgotten view,
and confirm the server-down revert path still shows its toast. If the Rust
server and the Python stand-in disagree anywhere, the frontend is the arbiter —
it shipped first.

Then delete `web/fixtures/serve.py`. Its job is done, and leaving a second
implementation of the API around is how the two drift.

## Fences

- You own: `src/**`, `tests/**`, `Cargo.toml`, `Cargo.lock`, `README.md`, and
  deleting `web/fixtures/serve.py`.
- Do not touch: `web/index.html`, `web/app.css`, `web/app.js` — the frontend
  shipped and was reviewed; if you believe it is wrong, report it and leave it.
  Also off limits: `reference/**`, `slices.toml`, `docs/**`, `.github/**`,
  `xtask/**`, `src/session.rs`, `src/snss.rs`.
- Every new source file carries its `slice:` / `why:` header;
  `cargo xtask slices --check` must pass.
- **Run no git commands at all.**

## Report

1. Files created or changed, with line counts.
2. The four gate outputs.
3. Your HTTP crate decision and the paragraph justifying it.
4. The three security rules, how each is enforced, and the test that proves it.
5. What the real frontend did against the real server, item by item.
6. Judgment calls, and what a reviewer should attack first.
