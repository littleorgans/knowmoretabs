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
  snapshots already saved. It stays in `snapshot.json`. `save` reads a copy made inside the archive and never opens the
  browser's file; a History it cannot read costs the signals, with one
  warning, never the snapshot. `save --no-history` skips it, and
  `save --json` reports how many tabs it found.
- **See how you got to a page.** A page's history in `serve` and `export`
  now has a "Browser history" section: the search that found it, the page you
  came from (a link to that page's own row when it is in your library), its
  visits and typed visits over the dates History still kept, and its time on
  page. The signals come from the library's own record when it has the page,
  and otherwise from the newest snapshot that recorded them. `export`
  leaves out the search and the page you came from unless you pass
  `export --with-history`, and never names a forgotten page as a referrer.
- **History signals for every page, not just the open ones.** The library
  keeps `pages/history.json`, what History last said about every page it
  lists. Each `save` that writes a snapshot refreshes it from the same copy
  of History, and `knowmoretabs history --refresh` does it on demand;
  `knowmoretabs history` shows how many pages it covers and how current it
  is. Forgotten pages, pages on this machine and non-web pages are not looked
  up. A page History has forgotten keeps its earlier signals, with the date
  History last knew it. A skipped save and `save --no-history` read nothing
  and leave the record alone.
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
  and keeps it in `library.json`, and `--create` brings it back. `tag
  <URL>... --clear NAME` returns a tag to undecided on those pages: neither
  yours nor dismissed, so a suggestion for it shows again. `serve`
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
  agent. Tags are flat: each is judged on the page alone and no tag follows
  from another; a `library.json` holding parent rules from a development
  build loads as before, the rules are ignored, and the next change to the
  vocabulary drops them. `knowmoretabs` still contains no model and sends
  nothing.
- **Review suggested tags in the library.** Each row shows its suggestions
  after your own tags, dashed and a step quieter, with one mark per source
  that suggested them, so a tag two agents agree on reads apart from a
  single guess. They count in the tag bar and match its filters until you
  decide. In `serve`, clicking a suggestion or pressing `+` opens the tag
  editor, where each one has ✓ to confirm and × to dismiss, a page with
  several has "Confirm all", and "Forget page" sits beside them for pages not
  worth keeping. On a selection the tray's editor confirms or dismisses a
  name on every selected page that carries it. The toast's Undo and `u`
  bring a suggestion back. Show ▸ "Has suggested tags" lists the pages still
  waiting. `GET /api/library` now gives each tag its `definition`, which the
  page shows on the tag bar, in the retire dialog and on suggestions. An
  export shows suggestions but cannot decide them.
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
  the rest would not, `--limit N` caps a run, and `--refetch` fetches again,
  including failed attempts. It and `content` are the only commands that
  send anything, and the README says what.
- **Page text, if you ask for it.** `knowmoretabs content` fetches each
  library page not captured before, without cookies, and keeps its main text
  as markdown, private to you, in `pages/content/`, one file per page, with
  one line per attempt in `pages/content.jsonl`. Each page is recorded as
  `ok`, `thin` (under 1,500 characters, text kept), `empty_shell`,
  `behind_login`, `blocked`, `paywalled`, `not_found`, `not_html`,
  `skipped`, `error` or `unavailable`. It never fetches what `enrich` never
  fetches, and records X profiles and YouTube channels and playlists as not
  a document, without a request. A timeout, a 429 or an HTTP 502, 503 or 504
  is retried twice in the run, honouring `Retry-After` up to a minute; a
  page still failing is retried on the next run and is `unavailable` after
  three. A page known by addresses that differ only after `#` is fetched
  once. `--dry-run`, `--limit N`, `--refetch` and `--url URL`; never part of
  `save`.
- **X posts as text.** `content` reads an X post (`x.com` or `twitter.com`,
  `/<user>/status/<id>`) from the public X post API at `api.fxtwitter.com`,
  sending only the post's number, and keeps its text, the post it quotes, an
  article's body with its code, a link to each post it embeds and an
  `[Image]` or `[Video]` placeholder, with any description, where it shows
  one, the author, the date and the descriptions of its images. One post,
  not its thread. A missing, deleted or suspended post is `not_found`, a
  private post or protected account `behind_login`, and a post the API
  reports as blocked `blocked`.
- **GitHub as text.** With gh installed and signed in, `content` reads a
  GitHub repository, issue, pull request or discussion through `gh api`,
  never seeing the token: a repository's description, topics and README as
  written, and a thread's title, state, opening post and first 10 comments,
  with the total. At most four gh calls at once, each stopped after 20
  seconds. A private repository is `behind_login`, a page GitHub does not
  show is `not_found`, and a repository with no README or a thread with only
  a title is `thin`. Without gh, GitHub pages are read as web pages, and the
  report says so.
- **YouTube videos as text.** With yt-dlp and deno (or node) installed,
  `content` reads a video (`/watch?v=`, `/shorts/`, `youtu.be`) through
  yt-dlp: its title, channel, upload date, duration, chapters and
  description, then a transcript from one caption track, English first
  (made by a person, then YouTube's own), else the video's own language the
  same way; a machine translation is never chosen. Rolling automatic
  captions are read once each, and the transcript is grouped by chapter.
  yt-dlp is given only the video's id, with no cookies and no configuration
  file, one video at a time and a second between its requests. A video
  without captions is `thin`, keeping its description; a private, removed
  or missing video is `not_found`; an age check, members only or sign in
  is `behind_login`; a bot check or rate limit is `blocked`. Without yt-dlp
  or a JavaScript runtime, videos wait, unrecorded, and the report says how
  many. The front matter records the caption language and whether the
  captions are manual or automatic.
- **One preview image per page.** `content` also keeps the image that best
  shows each captured page: an X post's photo, video thumbnail or article
  cover, a YouTube thumbnail, a repository's own social preview or its README's
  first picture before GitHub's generated card, else the page's `og:image`,
  `twitter:image`, JSON-LD or `itemprop` image, else the largest image in
  its main text; an image a site shows on three or more of your pages comes
  after the page's own. Main text and README candidates exclude logos,
  icons, avatars, badges, ads, tracking pixels and SVGs. Pages that are
  images are kept too.
  It downloads one preview image per page from the address the page names,
  which may be on another host; images stay in the private archive, as a
  JPEG of at most 768 pixels, quality 80, upright, in sRGB and without the
  original's metadata, in `pages/images/` with one line per attempt in
  `pages/images.jsonl`. Images are fetched like pages (no cookies, one
  request a second per site, at most 8 MiB, three candidates per page),
  checked by their own bytes and refused by their dimensions before
  decoding. A failure that may pass is `error` and retried next run from
  the candidates the line kept; after three runs it is `unavailable`.
  `--no-images` skips images for a run, and `--dry-run` counts them.
  `serve` shows each kept image as a small thumbnail beside its page,
  served from the archive on `127.0.0.1`; nothing is loaded from the
  network, and an export carries no images.
- **Check what content can use.** `knowmoretabs doctor` reports whether each
  way of reading pages is ready, missing or degraded, with how to fix it:
  the web tier, gh and its version, the X post API, yt-dlp with deno or node
  for YouTube, and the `--browser` binary; then whether the archive is private and its
  pages by status. Offline by default; `--live` checks gh sign in (exit code
  only, output never read) and asks the X post API once for a fixed public
  post. `--json` for scripts. Exits 0 when the web tier is ready.

### Changed

- **Private page metadata.** On Unix, `pages/metadata.jsonl` is now created with mode `0600`, and an existing file's mode is corrected when opened for enrichment.

### Fixed

- **Programs `content` runs end with everything they started.** When gh or
  yt-dlp passes its deadline, the program and every process it started (such
  as yt-dlp's deno) are ended, not the program alone. Ctrl-C, SIGTERM or
  SIGHUP during `content` or `doctor` ends them too, removes yt-dlp's scratch
  folder and exits 130. When one of those signals was inherited as ignored
  (`nohup`, or a background job in a script), it stays ignored, a signal
  cleans up nothing, and the run says so once.
- **Pacing holds after a late wakeup.** `enrich` and `content` no longer send two requests to one host less than the pace apart when the system wakes a waiting request late.

[0.1.0]: https://github.com/littleorgans/knowmoretabs/releases/tag/v0.1.0
