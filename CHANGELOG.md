# Changelog

What changed for someone using `knowmoretabs`, newest first. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from its section here, so the heading must
carry a date before the tag is pushed.

## [0.1.0] — Unreleased

The first release. It was built in six slices, and each one is listed below
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

[0.1.0]: https://github.com/littleorgans/knowmoretabs/releases/tag/v0.1.0
