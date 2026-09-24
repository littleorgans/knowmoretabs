# Changelog

What changed for someone using `knowmoretabs`, newest first. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from its section here, so the heading must
carry a date before the tag is pushed.

## [0.1.0] — Unreleased

The first release. It was built in slices, and each one is listed below
as what you can do that you could not before.

### Added

- **Save what is open right now.** `knowmoretabs save` (or just
  `knowmoretabs`) reads Chrome's own session file and writes a dated snapshot
  under `~/.knowmoretabs/snapshots/`: every window, tab, pinned state, tab
  group (name, colour, collapsed) and last-active time as JSON, plus a
  verbatim copy of the session file. A snapshot is never overwritten, an
  interrupted run leaves nothing behind, and two runs at once both succeed.
  If nothing has changed since the last snapshot, nothing is saved unless you
  say `--force`. Tabs on this machine (`localhost`, `127.0.0.1` and the other
  loopback spellings) are left out of the snapshot and counted, so opening a
  development server is never a change; the library already hid them. A session record the parser does not recognise, a
  half-written tail, or a tab with no usable page are counted and reported,
  never fatal. If Chrome's encrypted session files are newer than the
  cleartext ones this tool reads, `save` refuses with exit status 3 rather
  than saving stale tabs as current. `--json`, `-v`, `-q`, `--root`,
  `--session` and `--profile` are all here from the start.
- **Remember how you got to a page.** `save` records, for each tab, what the
  browser's own History knows about its page: visits, typed visits, first and
  last visit, time in the foreground, the search that led to it (up to three
  links back) and the page you came from. Chrome forgets after about 90 days;
  a snapshot keeps it, and clearing the browser's history does not reach
  snapshots already saved. It stays in `snapshot.json` and nothing shows it
  yet. `save` reads a copy made inside the archive and never opens the
  browser's file; a History it cannot read costs the signals, with one
  warning, never the snapshot. `save --no-history` skips it, and
  `save --json` reports how many tabs it found.
- **Read your archive offline.** `knowmoretabs export` writes a static site
  that opens from `file://` with no server and no network: every page you
  have ever had open, listed once, with search, a site filter, an open-or-not
  filter, five sort orders, tab groups, and a per-page history of the
  snapshots and windows it appeared in. `knowmoretabs list` shows your
  snapshots from the terminal. Light and dark follow the system; `/`, `j`,
  `k` and `f` work from the keyboard. Local pages (`file://`, `localhost`)
  are kept in snapshots but left out of the library.
- **Triage from a page instead of a terminal.** `knowmoretabs serve` runs the
  same library on `127.0.0.1` with live Forget and Restore buttons, bulk
  selection and undo. `knowmoretabs forget <URL>...` and `restore <URL>...`
  do the same headlessly. Forgetting hides a page from the library and never
  touches a snapshot. The server answers only this machine and only its own
  page; `--port` picks a port and `--open` opens your browser.
- **Use the browser you actually use.** Chrome Beta, Chrome Canary, Chromium,
  Brave, Edge and Vivaldi alongside Chrome. With no flags, the browser with
  the newest session wins and the others found are named; `--browser` picks
  one, `--profile` accepts a directory name or a display name, and
  `--user-data-dir` handles a relocated profile. Arc is refused with an
  explanation rather than misread.
- **Linux and Windows.** Session discovery on Linux, including Snap and
  Flatpak installs and Chrome's own `CHROME_CONFIG_HOME` and
  `CHROME_USER_DATA_DIR`; on Windows, `%LOCALAPPDATA%` for both browsers and
  the archive, with names Windows rejects refused up front and long paths
  handled. `--open` uses `open`, `xdg-open` or `start` as appropriate. The
  test suite runs on all three platforms in CI.
- **Install it in one line.** Prebuilt binaries for macOS (Intel and Apple
  silicon), Linux (x86-64 and ARM64) and Windows (x86-64) attached to each
  tagged release with SHA-256 checksums, and the crate ready for
  `cargo install`.
- **Tag pages yourself.** `knowmoretabs tag <URL>... --add NAME --remove
  NAME` tags pages or untags them, and `knowmoretabs tags` lists your tags
  with how many pages carry each. A new name joins your vocabulary; names
  match without regard to case. `tags --retire NAME` hides a tag everywhere
  and keeps it in `library.json`, and `--create` brings it back. `serve`
  gains `POST /api/tags` and `POST /api/vocabulary` behind the same
  loopback, `Host` and `Origin` checks as forget, and every page in `serve`
  and `export` carries its tags. A `library.json` from before tags reads as
  untagged. Nothing is tagged automatically and nothing leaves the machine.
- **Suggested tags, from an agent you choose.** `knowmoretabs tag --prompt
  DIR` writes a work folder for any agent (Claude Code, Codex, anything):
  `prompt.md` carries how you tag (flat facets, broad tags, no exclusions,
  "substantially about", favour recall), what each tag means, the exact
  answer format and a check the agent runs before it finishes; `pages.jsonl`
  lists the pages that show none of your tags and have no answer yet,
  never a forgotten one, with searches and referrers only under
  `--with-history`. `knowmoretabs tag --import DIR/tags.jsonl` checks every
  line (known pages, known tags, one line per page, one source) and stores
  nothing unless the whole file passes; `--dry-run` only checks,
  `--accept-new` creates tags the vocabulary lacks, `--partial` allows
  unanswered pages and `--source` names the model. Answers are kept in
  `tags/suggested.jsonl` with their source, date and vocabulary version, and
  every page in `serve` and `export` gains `suggested`: the tags suggested
  and not yet added or removed by you, each with its sources. `tags --define
  NAME TEXT` gives a tag a definition, shown by `tags` and given to the
  agent, and `tags --imply CHILD PARENT` records a parent rule the import
  applies. `knowmoretabs` still contains no model and sends nothing.
- **Page metadata, if you ask for it.** `knowmoretabs enrich` fetches the
  `<head>` of each library page not fetched before, without cookies, and
  appends what it says about itself to `pages/metadata.jsonl`: title,
  description, `og:` and `twitter:` tags, JSON-LD types, language and
  canonical address, and for a public GitHub repository its topics and the
  start of its README. It never fetches forgotten pages, this machine or the
  private network (checked on every redirect and at the connection), search
  results, or addresses carrying a token; sign-in, sign-up and verification
  screens, and pages that redirect to one, are recorded as behind a login and
  stay in the library. One request a second per site, a few sites at once,
  reading no more than 3 MB; `--dry-run` lists what would be fetched and why
  the rest would not, `--limit N` caps a run, `--refetch` fetches again, and
  a failed page is retried next run. It is the only command that sends
  anything, and the README says what.

[0.1.0]: https://github.com/littleorgans/knowmoretabs/releases/tag/v0.1.0
