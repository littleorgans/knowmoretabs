# Slice 7b, part one: History signals in `save`

**Status:** approved 2026-09-24. Nothing here is built yet. It settles *how*
to build the decision recorded in `slice-07-tags.md` §4 and §9 ("`save`
records History signals"), with measurements. §12 records the owner's
decisions; §11 is for open questions. None of §10's changes is applied yet.

**Ships:** each tab in `snapshot.json` carries what the browser's own
`History` database knows about its URL: the search that led to it, the
referrer, how often it was visited and typed, when it was first and last
visited, and how long it was in the foreground. The database is copied into
the archive's staging area and read there. No network, nothing shown yet:
display is a later step.

It ships on its own, before `enrich`. It needs no network code, and `enrich`
needs no SQLite, so neither should wait for the other.

## 1. The signals

Measured on the owner's Chrome 153 (`Default` profile, `History` schema 70),
for the 73 tabs open at the time and for all 649 URLs in `~/.knowmoretabs`.
The commands are in §13.

| Field | Source | Rule | Open tabs | Library |
|---|---|---|---|---|
| `visits` | `urls.visit_count` | as stored | 73 of 73 found; 28 visited more than once | 649 of 649; 202 |
| `typed` | `urls.typed_count` | as stored | 10 non-zero | 41 non-zero |
| `last_visit` | `urls.last_visit_time` | as stored | 73 | 649 |
| `first_visit` | `min(visits.visit_time)` | earliest visit Chrome still holds | 73 | 649 |
| `foreground_seconds` | `context_annotations.total_foreground_duration` | sum of **positive** values over the URL's visits; absent when none | 36 | 484 |
| `search` | `keyword_search_terms.term` | nearest search results page within 3 hops back through `from_visit`/`opener_visit`, with `hops` (0 = the tab is the results page) | 13 | 185 |
| `referrer` | `visits.from_visit`/`opener_visit` → `urls.url`, else `visits.external_referrer_url` | latest visit to the URL that has a referring visit other than itself; else its latest external referrer | 72 | 598 |

**Include `typed`, `first_visit` and `last_visit`.** `typed` and
`last_visit` come from the same row as `visits`, so they cost nothing.
`first_visit` is one indexed aggregate (55 ms for 300 URLs). A typed count
separates the pages someone navigates to on purpose from pages they only land
on. The two visit times date a page's place in someone's attention, which a
snapshot's own timestamp cannot. All of these count only what Chrome still
keeps, about 90 days (the oldest visit here is 2026-06-25): `first_visit`
means "earliest retained", and the field's doc comment must say so.

**Four things the lab prototype got wrong,** found while measuring:

1. **It copies `History` without `History-journal`.** The journal was hot in
   7.5% of live samples (§3). A copy made then holds a half-written
   transaction, and on a synthetic database that read as a torn B-tree.
2. **It sums `total_foreground_duration` including Chrome's "unknown" value,
   −1,000,000 µs.** That value is in 39,141 of 57,163 `context_annotations`
   rows (68%), spread over 17,429 URLs. It also sits on the current visit of
   every open tab (41 of the 41 checked), because Chrome writes the duration
   when the visit ends. Sum positive values only. The visit in progress is
   therefore never counted.
3. **One hop finds half the searches.** From the 649 library URLs, walking
   back through `from_visit`/`opener_visit` finds the nearest search at depth
   1 for 99, at depth 2 for 43 more and at depth 3 for 17 more (plus 8, 4, 1,
   4 and 6 at depths 4 to 8). Walking redirect hops alone adds 3. A random 12
   at depth 2 or more, judged by term and host only, were on topic for 11.
   Three hops keeps most of the gain; recording `hops` lets display decide
   how far to trust it.
4. **`content_annotations.search_terms` adds one URL** of 649 that
   `keyword_search_terms` did not already give. It is not read.

**Join by URL, not by tab id.** `context_annotations.tab_id` and `window_id`
are the same SessionIDs that `save` records from the session file, and 41 of
the 73 open tabs have visits under their own `tab_id`. The newest of those
visits had the tab's URL in all 41. But for 12 of those 41 the tab's latest
visit was a reload, a typed URL or a link with no source, so it has no
referrer. The URL join finds the referrer on an earlier visit, and it covers
all 73 tabs rather than 41. Only 1 tab of 41 had a truly different referrer.
The tab-id join costs a concept and loses more than it gains.

## 2. How to read SQLite: the dependency decision

Options, measured with a copy of this repository under `/tmp`, with a History
reader (copy, schema probe, the §1 queries) wired into `main`. Toolchain
`rustc` 1.89.0, the MSRV, on an Apple M2 Max with 12 cores. Clean target
directory for each build.

| | Baseline (`main`) | + `rusqlite` 0.40.2, `bundled` |
|---|---|---|
| Clean `cargo build --release` (LTO, 1 CGU) | 33.8 s | 47.2 s (+13.4 s, +40%) |
| Clean `cargo build` (debug, what CI tests compile) | 8.5 s | 8.8 s |
| Release binary, `aarch64-apple-darwin` | 2,588,096 B | 4,334,736 B (+1.75 MB, +67%) |
| Packages in `cargo tree -e normal` | 75 | 82 (83 for `x86_64-pc-windows-msvc`) |
| Dynamic libraries (`otool -L`) | libiconv, libSystem | the same two: SQLite is static |

The added runtime packages are `rusqlite`, `libsqlite3-sys`, `hashlink`,
`hashbrown`, `foldhash`, `fallible-iterator` and
`fallible-streaming-iterator`. The build adds `cc`, `find-msvc-tools`,
`shlex`, `pkg-config` and `vcpkg`. `Cargo.lock` grows by 189 lines.
`libsqlite3-sys` 0.38.2 bundles SQLite 3.53.2 (a 9.5 MB C amalgamation).
Neither crate declares a `rust-version`, but both built on 1.89.0 here. Both
were released 2026-08-08 and have about 36 M and 68 M recent downloads.

**CI matrix.** `bundled` needs a C compiler at build time. Every runner in
`ci.yml` and `release.yml` already has one, because Rust links through it:
`cc` on Linux, Xcode's `clang` on macOS, MSVC `cl` on Windows. The same holds
for anyone running `cargo install`. rusqlite's own CI builds `bundled` on
`windows-latest` (MSVC and GNU), `ubuntu-latest` and `macos-latest`, and
`aarch64-apple-darwin` built here. **`ubuntu-24.04-arm` has not been
exercised by anyone we can point to.** It is plain C through `cc` with `gcc`,
so the risk is low, but the implementing PR's CI run is the first proof.

**Hand-written read-only reader, honestly sized:**

| Part | Lines, est. | Why this History needs it |
|---|---|---|
| Page access, header | 40 | |
| Hot-journal rollback: headers per sector, checksummed records, truncate to the original size | 120 | the journal was hot in 7.5% of samples (§3) |
| WAL replay: frame checksums, salts, last commit frame | 130 | behind a Chrome feature flag today (§3); a flip makes it mandatory |
| Table B-tree walk with overflow chains | 180 | 9 `urls` rows overflow a 4 KiB page; the longest URL is 7,100 B |
| Record and varint decoding, `INTEGER PRIMARY KEY` as rowid | 100 | |
| Schema: parse `CREATE TABLE`, apply `DEFAULT`s to rows written before an `ALTER TABLE ADD COLUMN` | 130 | `external_referrer_url` arrived by `ALTER`; older `visits` rows are shorter |
| The §1 queries as Rust over full scans of four tables (~150k rows) | 180 | no SQL engine |
| **Total** | **~900** | `snss.rs` + `session.rs` is 1,490 |

On top of that come tests of similar size. The fixtures still have to be
written by a real SQLite, either as committed binaries or through a
dev-dependency on rusqlite. So the dependency is moved, not avoided.

**Other options:**

- **System SQLite** (rusqlite without `bundled`): macOS ships
  `libsqlite3.dylib`, Windows 10+ ships `winsqlite3.dll`, and Linux usually,
  but not always, has `libsqlite3.so.0`. Taking this route means three build
  configurations, each with a different SQLite version, to save 1.75 MB.
- **Shelling out to `sqlite3`:** not installed on Windows, which breaks
  principle 6.
- **turso 0.7.2** (a pure-Rust SQLite rewrite): 241 packages in its normal
  tree, pre-1.0, async API.
- **`sq3_parser` 0.3.3 and `sqlite-parser-nom` 1.0.0:** last released 2024
  and 2023, with about 70 recent downloads each, and no journal or WAL
  handling.

**Recommendation: `rusqlite` with `bundled`.** Slice 3 hand-wrote its HTTP
layer because the alternative was a dormant crate parsing untrusted input
across a security boundary, and owning it cost about 200 lines. Here every one
of those facts is reversed. The hand-written path is about 900 lines of file
format plus journal and WAL recovery, which is exactly the code SQLite itself
has spent twenty years testing. The crate is active, and its bundled C *is*
the reference implementation of the format. The input is not attacker
controlled: it is a private copy of the browser's own file. The
portability problem it solves is concrete. Windows has no SQLite the binary
can rely on linking, and linking the system library elsewhere would make the
"one binary" depend on a shared library whose version varies by machine.
`bundled` compiles one pinned SQLite into the executable on all five targets.
The costs are real and stated above: +1.75 MB, +13 s on a clean release
build, and 269k lines of C (the amalgamation) inside a crate that forbids
`unsafe` in its own code. The owner accepted them (§12).

**This is not SQLite as storage.** `BRIEF.md`'s "Deliberately not built:
SQLite" is about the archive, and it stands. Snapshots stay JSON. Nothing is
written with SQLite except a scratch copy that is deleted within the same
run. Having the crate in the tree does not bring the `scale-index` row
forward; that row keeps its own trigger.

## 3. Copying History: the journal, the WAL and locks

**What Chrome does.** The evidence is the files on this machine and
Chromium's source (`main`, read 2026-09-24):

- `History` runs in rollback-journal mode, not WAL. All nine local
  profiles (Chrome ×5, Brave, Edge, Vivaldi, Arc) have header bytes 18–19 =
  1/1, and none has a `History-wal`. In `history_database.cc`, WAL is
  `set_wal_mode(kHistoryDatabaseWriteAheadLogging)`, and `features.cc` has
  that feature `FEATURE_DISABLED_BY_DEFAULT`.
- Chrome holds the database with `exclusive_locking` (the `sql::DatabaseOptions`
  default) and keeps a transaction open, committing every 10 s
  (`kCommitIntervalSeconds = 10` in `history_backend.cc`). The Windows-only
  `exclusive_database_file_lock`, which would stop any other process opening
  the file, defaults to false, and History does not set it.
- With a 4 MB page cache, an uncommitted transaction spills pages into
  `History` itself, and their originals sit in `History-journal`. Sampled
  every 0.2 s for 60 s while Chrome ran, the journal's header was **hot in
  20 of 265 samples (7.5%)**, with sizes between 0 and 115 KB.

**What that means for a copy.** It was tested on a synthetic database, where
a child process spilled an uncommitted update and exited without committing:

| Copy | Result |
|---|---|
| `db` + `db-journal`, opened read-write | SQLite rolls the hot journal back: 20,000 rows, 0 half-changed, `integrity_check` ok |
| `db` only (what the lab does) | 10,338 rows, 9,269 of them half-changed, `integrity_check`: "Rowid out of order" on tree 2 |
| `db` + `db-journal`, opened read-only | `attempt to write a readonly database`: the rollback needs write access |
| WAL mode, `db` + `db-wal` | 3 rows committed only to the WAL are seen |
| WAL mode, `db` only | 0 rows |

**The rule:**

1. Copy `History-journal` and `History-wal` if present, then `History`, with
   `std::fs::copy`. Never copy `-shm`; SQLite rebuilds it. Never open the
   live file, not even read-only: an SQLite open of the live file could take
   locks Chrome does not expect, and principle 2 forbids touching browser
   files beyond reading bytes.
2. Stat all three before and after, and retry up to three times if length or
   mtime moved, as `read_stable` already does for the session file. A
   10-second commit interval makes a collision rare. The check makes it
   detected rather than silent.
3. Copy into a fresh `archive.stage()` directory, a `.staging-` sibling under
   `snapshots/` that is separate from the snapshot's own staging directory.
   It sits inside the 0700 root on the same volume, and
   `clean_stale_staging` already removes it if the process dies. The whole
   browsing history must never be left behind in the system temp directory.
4. Open the copy read-write, so SQLite rolls back a hot journal or replays a
   WAL, and drop the directory when done.
5. Do not run `PRAGMA quick_check` on every save: it took 0.83 s cold and
   0.08 s warm. Rules 1 and 2 are the guarantee. Any SQLite error while
   reading is recorded as "History unavailable" (§5).

**Freshness, against the running Chrome.** Copies were taken every 5 s for
33 minutes while the owner browsed. Each of the 4 visits made in that time
first appeared in a copy **4.8, 8.0, 7.2 and 13.2 s** after its
`visit_time`. That fits a 10 s commit interval plus a 5 s poll. No copy failed
to open (391 rounds). In those rounds, a copy of `History` alone never
differed from a copy with its journal in visit count or newest visit. So the
harm of skipping the journal was reproduced only synthetically, above, and
the rule is there because it costs nothing. Visits in Chrome's still-open
transaction are in memory and in no file, so a tab opened seconds before
`save` may have no `history`. It never gets one later, because snapshots are
immutable.

## 4. Schema facts, per browser and version

Columns are found by name with `pragma_table_info`, once per save. The
schema version is recorded but never gated on. A newer version with the same
columns just works.

| Column | In Chromium since | If missing |
|---|---|---|
| `urls.url`, `visit_count`, `typed_count`, `last_visit_time`; `visits.url`, `visit_time`, `from_visit` | before schema 15, the oldest Chromium opens without razing (`kMinimalVersionNumber`) | **required**: no History signals for this snapshot; the reason is recorded; `save` succeeds |
| `keyword_search_terms.url_id`, `term` | created at open whenever missing (`InitKeywordSearchTermsTable`) | no `search` |
| `visits.opener_visit` | schema 49 (`MigrateVisitsWithoutOpenerVisitColumn…`) | walk `from_visit` only |
| `context_annotations.total_foreground_duration` | schema 51 (`MigrateContextAnnotationsAddTotalForegroundDuration`) | no `foreground_seconds` |
| `visits.external_referrer_url` | schema 66 (`MigrateVisitsAddExternalReferrerUrlColumn`) | no external fallback for `referrer` |

**Browsers `save` supports** (`platform::BROWSERS`: Chrome, Beta, Canary,
Chromium, Brave, Edge, Vivaldi) all use Chromium's history backend, and the
file is `<profile>/History` on every OS. Checked here, from copies:

| Browser | Profile | Schema | Needed columns | Visits |
|---|---|---|---|---|
| Chrome 153 | Default, Profiles 1, 3, 4, 8 | 70 | all | 59,551; 0; 17; 179; 120 |
| Brave | Default | 70 | all | 7 |
| Edge | Default (last used 2025-08) | 70 | all | 21,956 |
| Vivaldi | Default | 70 | all | 89 |
| Arc (unsupported) | Default | 69 | all | 143,736 |

Edge's file was last written in August 2025 and is already at 70, so every
column above has been in shipping browsers for over a year. Chromium, Beta
and Canary are not installed here. A browser that migrates History when it
starts cannot leave an older schema behind while it is in use. An empty
History, such as Brave's "clear on exit" or a new profile, simply gives tabs
no `history`.

**Where History comes from:** `Located::profile_dir`, which `save` already
computes for the staleness check. That also covers `--session` when the file
sits in a `Sessions/` directory. A `--session` file elsewhere has no profile,
so it gets no History and says so.

## 5. Where it is stored

**Optional fields in `snapshot.json`, no schema bump, no sidecar file.**

```json
"tabs": [{
  "tab_id": 17, "window": 1, "position": 0, "url": "https://example.test/a",
  "…": "…",
  "history": {
    "visits": 12, "typed": 3,
    "first_visit": "2026-07-01T09:12:44.123456Z",
    "last_visit": "2026-09-23T23:49:42.000000Z",
    "foreground_seconds": 1834,
    "search": {"term": "example query", "hops": 1},
    "referrer": "https://example.test/list"
  }
}],
"history": {
  "path": "/Users/…/Default/History", "bytes": 148930560, "schema_version": 70,
  "newest_visit": "2026-09-23T23:49:42.000000Z", "tabs_found": 73,
  "unavailable": [], "error": null
}
```

- `Tab.history: Option<TabHistory>` and `Snapshot.history:
  Option<HistorySource>`, both `#[serde(default, skip_serializing_if =
  "Option::is_none")]`. Inside `TabHistory` every field except `visits`,
  `typed` and `last_visit` is optional and omitted when absent. A tab whose
  URL History does not know has no `history` key. `unavailable` lists
  optional columns the database lacked, so that an absent `search` can be
  read as "not recorded" or "not available". `error` carries the one-line
  reason when History could not be read at all.
- **No `schema_version` bump.** `SCHEMA_VERSION`'s rule is "bump when a
  reader of the previous version could misread the new one". No current
  reader can misread these fields: serde ignores unknown fields
  (`model::tests::snapshot_round_trips_and_tolerates_unknown_fields` pins
  that), and missing `Option` fields read as `None`. A bump would be
  actively harmful: `library::usable` treats any version other than 1 as
  unreadable, so every build before this one would hide every new snapshot.
- **Why not a sidecar `history.json`:** it would be written and published in
  the same atomic rename, so immutability gives no reason to split. It would
  leave one snapshot described by two files, which cuts against "`snapshot.json`
  is THE source of truth". It would buy nothing for privacy either: forgetting
  is a filter and never touches a snapshot, and removing a file from a
  published snapshot directory breaks "never written to again" just as much.
- **Dedupe is unaffected.** `Snapshot::layout()` compares window, position,
  URL, pinned state and group only. History fields sit outside it, like
  titles and `last_active`. So a History change never produces a new
  snapshot. `save` should also **read History only after the unchanged check**:
  a skipped run costs nothing more than it does today, and gains nothing,
  because a skipped run writes nothing. `--force` always reads it.
- **Signals change over time and the archive keeps each snapshot's view.** The
  same URL in later snapshots carries later counts. That is the point:
  §4 of the tags brief notes that a snapshot keeps what Chrome discards after
  90 days.
- **Archive size.** Measured with the §1 rules in pretty JSON: 288 B per tab
  for the 73 open tabs and 363 B for the 649 library URLs. Today's
  `snapshot.json` is about 420 B per tab (30,728 B for 73 tabs), so it
  roughly doubles. The verbatim `session.snss` beside it is 1,031,751 B, so a
  300-tab snapshot directory grows by about 110 KB, around 10%. `serve`
  parses every `snapshot.json` per library load. Fifty 300-tab snapshots add
  about 5.5 MB of JSON, well inside the in-memory budget `BRIEF.md` sets.

## 6. Cost per save

This machine: `History` is 148,930,560 B (36,360 pages of 4 KiB, 17,732 of
them free), with 27,633 URLs and 59,552 visits from 2026-06-25 to
2026-09-23.

| Step | Time |
|---|---|
| Copy `History` + `-journal` with `std::fs::copy` (APFS clones) | 1.4 ms (8.4 ms on the first run) |
| The same 149 MB copied byte for byte (`cat >`, warm cache): the Linux ext4 and Windows NTFS case | 0.04–0.05 s |
| Queries, 300 URLs (release build, prepared statements, six queries per URL) | 0.20–0.30 s |
| Queries, 73 / 649 URLs | 0.26 s (cold) / 0.53 s |
| Remove the copy | < 1 ms |

That is **about 0.3 s per written snapshot**, held under the archive lock,
and nothing for a skipped one. Per-query split for 300 URLs: `urls` lookup
27 ms, first/last visit 55 ms, foreground 47 ms, search walk 28 ms,
referrer 26 ms, external referrer 9 ms. Nothing dominates, so no batching is
needed. A cold byte copy on a spinning disk would add about a second. That
was not measured.

## 7. Privacy

Search queries are the most sensitive thing this feature records. The facts
and the proposed defaults:

- **The archive already holds some of them.** `session.snss` is kept verbatim,
  and it contains every tab's back and forward navigations, including search
  results URLs with their `q=`. What is new is searches that reached a tab
  from *another* tab or an earlier session, the referrer, and all of it in a
  plain, greppable field.
- **Referrers are as sensitive as URLs.** Of 11,350 external referrers here,
  37% carry a query string. Apply `local::is_this_machine_url` to referrers as
  to tabs. Otherwise store them verbatim, as tab URLs are.
- **Where it lives:** in `snapshot.json` under the 0700 root. The scratch
  copy of `History` stays inside that root (§3) and is gone by the end of the
  run.
- **What `serve` and `export` expose in this step: nothing new.** Both are
  built from `library::build`, an explicit projection (`LibrarySnapshot` and
  a six-field `Tab` tuple), not from `snapshot.json`. `save --json` prints
  counts, not tabs. The integration test in §9 pins this so it stays true
  until display is designed on purpose.
- **Forgetting** a page hides its signals once they are shown, the same filter
  as today, and never touches a snapshot.
- **Chrome's "Clear browsing data" does not reach the archive.** Signals
  recorded before the clear stay. The README must say this next to the line
  that says what `save` records.
- **Defaults for later, when signals are shown:**
  - `serve` shows them: it is the same trust boundary that already shows every URL.
  - `export` leaves out `search` and `referrer` unless asked for: an exported
    folder is the artefact most likely to be copied somewhere else. Decided
    (§12): `--with-history` puts them back.
  - Proposed, for 7c: `tag --prompt` puts search terms into `pages.jsonl`
    only when asked for, because that folder goes to the owner's agent and its model provider.
    The lab found "found by searching" useful for tagging, so it needs a flag
    rather than a ban.

## 8. Tests

**Build the History fixture at test time. Commit no binary.** With rusqlite a
normal dependency, a test creates `History` exactly as it builds session files
today (`tests/common/session_builder.rs`).

- `tests/fixtures/history-v70.sql`: the `CREATE TABLE` and `CREATE INDEX`
  statements for `urls`, `visits`, `keyword_search_terms`,
  `context_annotations` and `meta`, taken from a real v70 `sqlite_schema`.
  It holds schema text and no rows, so it contains no personal data and is
  reviewable in a diff. Rows are synthetic (`example.test`).
- An older shape is the same file with `opener_visit`,
  `external_referrer_url` and `context_annotations` removed. It is built by
  the test, not committed.
- A committed `.sqlite` is rejected. It is opaque in review, cannot be
  regenerated from anything in the repository, and would drift from Chrome's
  DDL without anyone noticing.

## 9. Scope and acceptance

**In scope:**

- A `history` module (`slice: capture`): locate, copy with the §3 rule, probe
  columns, read the §1 signals per URL, and return per-tab values plus
  provenance. It never returns an error that fails `save`.
- `Tab.history` and `Snapshot.history` in `model.rs`, as in §5.
- `capture::save` reads History after the unchanged check and before
  staging, warns once (not per tab) when History is unavailable, and records
  why.
- `rusqlite` with `bundled` in `Cargo.toml`, with a comment that names the
  portability problem it solves (slice 5's rule for new dependencies).
- README: what `save` now records, that it stays local, and that clearing the
  browser's history does not clear the archive.
- `save --no-history` (§12), which skips History and records that it did.

**Out of scope:** showing the signals anywhere (`serve`, `export`, `list`),
`enrich` and all network code, 7c's prompt, the tab-id join,
`content_annotations` (`search_terms`, categories, entities), clusters,
`visit_duration`, segments, downloads, back-filling old snapshots (they are
immutable), non-Chromium browsers, and any SQLite in the archive.

**Acceptance:**

1. `save` on a profile with a `History` writes `history` on each tab whose URL
   History knows, with the §1 fields and rules, and a snapshot-level
   `history` with path, bytes, schema version, newest visit, tabs found and
   `unavailable`.
2. `search` is the nearest results page within 3 hops, `hops` 0 when the tab
   is the results page. A search 4 hops back is not recorded.
3. `referrer` prefers a referring visit other than the page itself, falls back
   to `external_referrer_url`, and is never a this-machine URL.
4. `foreground_seconds` sums positive values only. A URL whose only durations
   are −1,000,000 has no `foreground_seconds`.
5. A copy taken during a spilled, uncommitted transaction (the test holds one
   open) reads the committed state. The same test shows a copy without the
   journal reads different data, so it can fail.
6. A WAL-mode `History` whose newest rows are only in `-wal` is read with them.
7. The browser's `History`, `-journal` and `-wal` have the same bytes and
   mtime after `save` as before. Nothing named `History*` is left under the
   archive root or in the system temp directory, and a History scratch
   directory left by a killed run (the test plants one) is removed by the
   next `save`.
8. A database without `opener_visit`, `external_referrer_url` or
   `context_annotations` still yields the other signals and lists what was
   unavailable. A database without `urls` yields no signals, an `error`, a
   warning and a successful `save`. A missing `History` or an unreadable copy
   behaves the same way.
9. A save whose layout is unchanged but whose History changed is skipped, and
   reads no History (under `-v`, a written save notes that History was read;
   the skipped one does not).
10. Snapshots written before this slice read as before, and a snapshot written
    by it is read by the current `main` build with the fields ignored (the
    model round-trip test gains a `history` case).
11. `GET /api/library` and `export` output are byte-identical with and without
    `history` in the snapshots.
12. `save --no-history` copies nothing, writes no per-tab `history`, and the
    snapshot-level `history` says it was skipped by request.
13. CI is green on all four test runners and the five release targets with
    `bundled` SQLite, and `cargo xtask slices --check` passes.

## 10. Changes to `docs/BRIEF.md` and the research docs (approved)

1. **Deliberately not built, SQLite:** "SQLite *as the archive's storage*."
   Add: "`save` reads the browser's own `History` database through a bundled
   SQLite (slice 7b). That is reading a browser file, not a storage choice,
   and it does not bring `scale-index` forward."
2. **§5 data model:** `Tab` gains `history?`, and `Snapshot` gains `history?`.
3. **`docs/research/rust-stack.md`, "Deferred, with triggers":** note on the
   SQLite row that `rusqlite` is present for reading only.
4. **`docs/research/encrypted-sessions.md` §4** says History has "SQLite
   locking/copy-consistency concerns". Point it at §3 here, which settles
   them.

## 11. Decisions for the owner

None open. New questions go here.

## 12. Already decided

- **`rusqlite` with `bundled` SQLite** reads History, for the §2 reasons:
  +1.75 MB (+67%) on the binary, +13 s on a clean release build, 7 runtime
  packages and 269k lines of C, against about 900 lines of hand-written
  file-format and recovery code that would still need rusqlite to test.
  Decided 2026-09-24.
- **`save --no-history`** skips History entirely. Recording stays the
  default, so the §4 decision in the tags brief stands; the flag is the only
  way to keep search terms out of an archive nobody can edit. Decided
  2026-09-24.
- **Once the signals are shown, `export` leaves out `search` and `referrer`
  unless `--with-history` is given** (§7). Decided 2026-09-24.
- **`search` looks up to 3 hops back, and records `hops`,** rather than the
  lab's 1: 159 of the library's searches rather than 99, and the extra finds
  were on topic in 11 of 12 checked. Decided 2026-09-24.

## 13. Measurements, and how to repeat them

All reads were of copies under `/tmp/kmt-hist` (mode 0700). The live
profile's files were copied, never opened. Only counts were written down; no
URL, title or search term from the owner's data is in this document.

```sh
P="$HOME/Library/Application Support/Google/Chrome/Default"
# Journal mode, page counts, schema version (header bytes 18-19: 1/1 = rollback journal)
ls -la "$P"/History*
mkdir -m 700 /tmp/kmt-hist
cp "$P/History-journal" /tmp/kmt-hist/History-journal; cp "$P/History" /tmp/kmt-hist/History
sqlite3 /tmp/kmt-hist/History "select value from meta where key='version'; pragma page_count; pragma freelist_count;"
# Foreground: the unknown value and how common it is
sqlite3 /tmp/kmt-hist/History "select sum(total_foreground_duration<0), sum(total_foreground_duration>0), count(*) from context_annotations"
# Nearest search within 3 hops for one URL id
sqlite3 /tmp/kmt-hist/History "with recursive anc(id, depth) as (
  select id, 0 from visits where url = :id
  union select pv.id, depth + 1 from anc join visits cv on cv.id = anc.id
        join visits pv on pv.id in (cv.from_visit, cv.opener_visit) where depth < 3)
  select k.term, anc.depth from anc join visits v on v.id = anc.id
  join keyword_search_terms k on k.url_id = v.url order by anc.depth, v.visit_time desc limit 1"
# Dependency cost: baseline vs + rusqlite (a copy of the repo under /tmp, with a reader wired into main)
cargo add rusqlite@0.40 --features bundled
cargo tree -e normal -p knowmoretabs --target aarch64-apple-darwin --prefix none | sort -u | wc -l
rustup run 1.89.0 cargo build --release --locked --target-dir /tmp/fresh   # clean each time
otool -L target/release/knowmoretabs
```

The journal was sampled every 0.2 s for 60 s, reading its first 8 bytes and
its size, and copying `History` and `-journal` every 10 s. Freshness was
watched with copies every 5 s for 8 minutes and then for 25 minutes. Each
round compared the newest `visit_time` with the time of the copy, and a copy
of both files with a copy of `History` alone. Results: 95 and 296 rounds, no
errors, no difference between the two copies, and 4 new visits seen
4.8–13.2 s after they happened.

Chromium sources: `components/history/core/browser/history_database.cc`
(`kCurrentVersionNumber = 70`, the migration steps cited in §4, the WAL
option), `history_backend.cc` (`kCommitIntervalSeconds = 10`), `features.cc`
(`kHistoryDatabaseWriteAheadLogging`, disabled by default) and
`sql/database.h` (`exclusive_locking_ = true`,
`exclusive_database_file_lock_ = false`), all from
`chromium.googlesource.com/chromium/src/+/refs/heads/main`, read 2026-09-24.

## 14. Not verified

- **Linux and Windows.** No `History` was copied on either. `std::fs::copy`
  uses `copy_file_range` on Linux and `CopyFileExW` on Windows. Chrome's
  SQLite opens with read and write sharing, and its lock bytes sit at the
  1 GiB offset, so copies of files under 1 GiB should succeed while Chrome
  runs. A `History` over 1 GiB on Windows could hit the byte-range lock. That
  is untested.
- **`ubuntu-24.04-arm` building `bundled`** (§2).
- **Chromium, Chrome Beta and Chrome Canary** are not installed here.
- **The version-to-milestone mapping** for schemas 49, 51 and 66 was not
  looked up. The Edge profile shows all three had shipped by August 2025.
- **A cold byte copy on slow storage.**
