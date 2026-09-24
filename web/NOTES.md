# web · the knowmoretabs library, as shipped

slice: library (and triage, when the host is `serve`)

The production frontend. `knowmoretabs export` embeds a library into
`index.html` and copies the three assets beside it; `knowmoretabs serve`
serves the same files with the data element empty and answers the API in §5. Nothing here is built, bundled or fetched from anywhere.

To work on it without an archive: `python3 fixtures/generate.py` rebuilds the
synthetic library, the edge-case documents in `fixtures/cases/`, and re-embeds
the library into `index.html` so it opens from `file://` in export mode.
`knowmoretabs --root <archive> serve` runs serve mode with live forget,
restore, bulk select, tags and undo against an archive. The Python stand-in was
removed when the Rust server shipped.

```
web/
├── index.html              markup, CSP, the embedded data element
├── app.css                 styles
├── app.js                  renderer and interactions (unminified)
├── fixtures/generate.py    deterministic synthetic library (seed 2026) + cases
├── fixtures/library.json   2,166 pages · 41 snapshots · 163 sites · 8,478 sightings · 78 groups · 270 with History
├── fixtures/cases/         empty · no-pages · one-snapshot · degraded · export-forgotten
└── NOTES.md                this file
```

## 0. The ruled frame (September 2026 redesign)

The page is one centred column, 76 rem wide, ruled with hairlines on both
sides and between every cell. A sticky top bar holds the brand, the two
views, and the theme switch. The hero sets the headline in a field of faint
repeated letters with the library card underneath. The toolbar is a strip of
cells. Every row is a cell: a two-digit index on the left behind a hairline,
the sighting strip, a serif title over the URL, how long ago it was last seen
on the title's own baseline, and an arrow cell on the right that opens the
history drawer. Text aligns to text — title and age on one line, address and
group mark on the next — while the index, the strip and the arrow centre in
their cells, because they are furniture rather than words. Hover, the
keyboard cursor and an expanded row invert wholesale (paper on ink, or ink on
paper in dark mode); selection tints. Band headings are large and sticky under
the top bar.

**Theme.** Light and dark follow the OS until the switch is used; then
`html[data-theme]` pins it, `localStorage.theme` remembers it, and `t`
toggles it from the keyboard. The palette is written once with
`light-dark()`; `[data-theme]` only changes `color-scheme`. A remembered
theme is applied by `app.js` before first paint, and only a live switch
crossfades (the `switching` class), so a reload never flashes.

This is design B from the bake-off in `docs/design-decision.md`, with A's
headline, A's reassurance copy, the "Open now" fold that document asked for,
tab groups, and the fixes from the slice 2b brief. What follows is the design
as it stands, not a changelog.

## 1. The emotional job, and how the design answers it

The brief: 2,000 pages you never dealt with reads as a reproach. It should
read as a library you are glad to own. Copy cannot do that alone; structure
can. These are the devices, in order of how much work they do.

**Every row carries its own record.** The sighting strip is one mark per
snapshot, 4 px each, oldest first — twelve marks to a line, and the
thirteenth starts a second line under the first, so the strip grows downward
instead of sideways and 48 snapshots fit in 48 px of row. A single line keeps
the old tall ticks (8 px); two or more shrink to stacked bars (4 px on a 6 px
pitch), and the last line is padded out with the empty track, so the block
stays a rectangle. Past 48 marks each one stands for an equal run of
snapshots and a mark the page only part-fills is drawn faint: the block is
then a density, not a record, but it is still 48 × 22 px whether the archive
holds fifty snapshots or five hundred. A page kept open all summer is a solid
block. A page opened once is a single tick where it happened. A page you keep
coming back to is a rhythm. Scanning the list, the eye reads shapes before
titles, and the shapes say "this has a history", not "this is undone". The
tooltip always gives the exact count, which is what a mark standing for three
snapshots cannot.

**The default order is a diary, not a queue.** Sorted by last seen, the list
is banded: *Open now*, then *September 2026*, *August 2026*, back to the
start. Band headings are serif italic and stick while you scroll. The bands
only appear for the two time sorts.

**"Open now" answers the arriving question, then gets out of the way.** The
band shows its first ten rows and a "Show all 236 open pages" link. Ten rows
is enough to see the pinned, always-open head of the collection, and the
archive begins on the first screen at 1000 px instead of 236 rows down. Any
search, site, group or status filter unfolds the band, because a filter means
you are looking for something and a hidden match would be a lie; "Show fewer"
folds it again. A deep link to a page behind the fold unfolds it.

**The masthead is a library card.** "Every tab you ever had open." is the
headline, then one quiet line of provenance: "2,160 pages · 162 sites · 41
snapshots · Mar 14 to Sep 21, 2026". There is no "unread", no progress bar,
and the count that appears while a filter is set says "104 of 2,160 pages",
never "104 remaining"; with no filter set the line is empty, because the
masthead already says how many there are. An archive with nothing in it says
so and names the command that fills it.

**Forgetting is quiet, reversible, and explained where you hesitate.** No
confirmation dialog, no trash icon, and nothing red — but under the pointer a
Forget button fills with a burnt earth (`--forget`), the only colour on the
page with a job. The old hover emptied the button out to paper, which read as
"don't"; a button you are about to press should lean in. Restore keeps the
ink. The Forget buttons carry the
reassurance as their tooltip, and the `?` legend says it in full: *Hides it
from the library. The snapshots themselves are never touched.* Forget removes
the row and shows a toast with Undo; `u` undoes from the keyboard. In export mode the same key shows the exact CLI
command with a Copy button, so the page never pretends to a power it lacks.

**Tab groups are marks, not a palette.** A grouped page carries a small dot in
its group's colour and the group's name in the metadata grey, after the URL.
See §4 for the reasoning.

**The typography is one quiet sans.** The system sans (SF on a Mac, Segoe on
Windows) at a small scale with tabular figures for every count and date;
monospace only for the literal URL in the history and for commands. No serif.
Warm paper in light mode, warm near-black in dark, layered by three surfaces
and two hairline tones, one blue for links, the cursor, focus and deep-link
targets. No favicons, no thumbnails, no colour-coded status. The checkbox and
the index share one cell: the number is the resting state and the checkbox
takes its place on hover, on the cursor row and on a selected row, so
selecting costs the row no width at all (see §4).

## 2. Information architecture

```
Masthead        brand · "Every tab you ever had open." · library card · Pages | Snapshots
Pages (default)
  Toolbar       Search (title+URL, all words, any order; `/` hint inside)
                Site (text field with datalist, exact site or substring)
                Group (same, hidden when the archive has no groups)
                Show ▾ Everything | Open now | Closed | Forgotten (serve only)
                Sort ▾ Last seen | First seen | Times seen | Title | URL
                Clear (only while a filter is set) · ? (opens the legend)
  Bar           "104 of 2,160 pages", only while a filter is set
  List          bands → rows, "Open now" folded to ten + "Show all N"
    Row         01/[☐]  ▮▮▮░▮ strip  title over url · ●group · "3d ago"  ›
    History     inset panel: full URL · summary (incl. every group it has
    (expanded)  sat in) · Only site · Copy URL · Forget/Restore (serve) or the
                CLI command (export); browser history (found by searching,
                came from, visits, time on page, §10); every sighting (date →
                snapshot deep link, window, tab position, group)
Snapshots       table: captured, windows, tabs (bar, "incomplete: …" when the
                parse degraded), first seen here, last seen here → snapshot
Snapshot        header (tabs, windows, groups, degradation) · windows in order ·
                tabs in position order with group mark (and "collapsed"),
                pinned chip, and "41×" link back into the library
Tray / Toast    selection tray (bottom: ✕ · count · Select all shown · Preview
                selection · Forget) · undo toast above it
Help            `?` dialog listing every key
```

Routes, all hash-based so they work from `file://`:

| hash | shows |
|---|---|
| `#pages` (or empty) | Pages view |
| `#pages/<index>` | Pages view with that page revealed, expanded and focused (clears filters and unfolds if it was hidden) |
| `#snapshots` | the snapshot table |
| `#snapshot/<id>` | one snapshot, by window |
| `#snapshot/<id>/<tab_id>` | same, scrolled to and highlighting that tab |

The two hosts differ in exactly one place: the `host` object at the top of
`app.js`. If the `#library-data` element has content, `host.mode` is `export`
and `host.forget` is undefined. Otherwise the data comes from `GET api/library`
and `host.forget` / `host.restore` POST to the API. Nothing else in the file
asks which host it is; it asks whether `host.forget` exists.

## 3. Keyboard

`/` search (then `↓` or `↵` jumps into the list) · `j` `k` or arrows move ·
`↵` opens in a new tab · `space` shows or hides history · `x` selects,
`shift`+`x` or `shift`+click extends a range (letter keys ignore case, so
Caps Lock does not silence them) · `f` forgets the selection or the current row
(restores, in the Forgotten view) · `+` or `=` tags the selection or the
current row · `u` undoes · `esc` steps back (below) · `?` opens the legend,
and while it is open the page's keys are off (`esc` closes it). The legend
lives only in that dialog, behind the `?` key and the `?` button at the end of
the toolbar; the search field shows a `/` hint.

**`esc` takes one step a press, the most local thing showing:**

1. an open dialog (the legend, Retire tags) closes;
2. an open menu (Site, Group, Show, Sort, the tag suggestions) closes;
3. text in the focused field clears (search, Site, Group, the tag field);
4. the tag editor, its field empty, closes and the key goes back to its row;
5. an open history closes;
6. the preview goes back to the full list, the selection kept;
7. the selection clears;
8. the filters clear, as Clear does;
9. a field with nothing left to clear hands the key to the cursor row.

Open things close before state is cleared, and the history comes before the
preview because it belongs to one row. Where the focus is never counts as a
step. The first version let a clicked row checkbox keep the focus, and since
the page read any `<input>` as a text field, `esc` first "cleared" the box's
value (`on`, invisibly), then blurred it to the body, and only the third press
cleared the selection; every other page key was dead in between. The row now
takes the focus when its box is clicked, as it does for any click, and a
checkbox is not a field. Nothing `esc` hides takes the key with it: when the
tray goes, the key goes back to the cursor row. The "Open now" fold and the
tag bar's "N more" are views you chose, not layers, and `esc` leaves them.

Rows are the tab stops. The list uses a roving tabindex: one row is
`tabindex="0"`, the title link and the buttons inside rows are `tabindex="-1"`.
The focused row is the cursor, so the focus ring and the cursor are the same
thing, and a mouse click on a row moves the cursor too. "Show all" moves the
cursor to the first newly shown row; "Show fewer" returns focus to the link.
A repaint rebuilds the rows, so `render()` notes whether it held the focus and
gives it back: to the same row, or if that row left (forgotten, untagged under
a tag filter), to the nearest row that stayed, after it and then before. It
does not scroll. So after `f` the key is on the next row, and after `u` it
stays where it was while the page comes back above it. The tray, the toast
and the tag editor hand the key to the cursor row as they go, never to the
body.

## 4. Decisions, and what was rejected

- **Tab groups: a dot and a name, on the row, from the latest sighting.** A
  page's row shows the group it sat in when it was last seen: for an open
  page that is Chrome's current organisation, for a closed page it is the
  project it belonged to. Chrome's nine colours are each dimmed for paper and
  lifted for dark, and the dot (0.55 em) is the only thing on the page that
  ever takes one, so 600 marks down a list still read as marks against the
  single accent. The name is a button: clicking it filters to that group,
  like "Only github.com" does for sites. The Group field in the toolbar is a
  text field with a datalist, exactly like Site, and it is hidden when the
  archive has no groups, so an archive without them looks as it did before.
  An unnamed group shows the dot alone (Chrome does the same) and is not a
  button, since there is no name to filter by. Groups are matched by name,
  case-insensitively, across all snapshots: "in Papers" means "was ever in a
  group called Papers", and the row still shows where it sat last. The
  expanded history marks every sighting's group and summarises the set, so
  a page that moved from Papers to Later to nowhere tells that story. The
  snapshot view shows the same mark per tab, plus "collapsed" when the
  contract says so.
  Rejected: colouring the row or title by group (nine competing colours,
  the accent loses); a coloured left rule per row (collides with the cursor
  rule and the amber deep-link target); a Group sort or band (a group is a
  fact about one snapshot, not about a page, so there is no canonical group
  to band on); group sub-headings inside windows in the snapshot view
  (assumes contiguity, which a degraded parse can break); Chrome's exact
  hex values (designed for white chrome, loud on paper); group chips as a
  legend in the "Open now" heading (a second filter mechanism for the same
  thing).
- **The checkbox lives in the index cell, not beside it.** The index and the
  checkbox are the same grid cell (`grid-area: 1 / 1` for both the `::before`
  counter and the `.pick` span); wherever the checkbox shows, the number goes
  `color: transparent` and the hairline stays. Selecting therefore costs no
  column, the title keeps the width, and the eye has one place to look instead
  of two. The whole cell picks, not just the 15 px box. The box is drawn
  rather than native — a 1.5 px square of `currentColor`, filled with a tick
  in `--surface` when checked — because a native checkbox on an inverted row
  renders as a white slab whichever `color-scheme` it is handed. Where there
  is no pointer (`hover: none`) the checkbox simply is the cell, since nothing
  would ever reveal it. Export mode has no checkbox, so the number never
  leaves.
- **The tray is a ruled strip, not a row of links.** ✕ and the count are cells
  with hairlines between them, matching the toolbar; "Select all shown" is the
  one word-button; Preview is an outlined secondary and Forget the filled
  primary, so the two things you can do read as buttons and the state reads as
  a label. "Deselect all" went: it said the same thing as the ✕ and the `esc`
  key, in more words, in the middle of the actions.
- **"Open now" folds to ten rows.** See §1. Ten rather than fewer because the
  first rows are the pinned head whose solid strips do the most emotional
  work; ten rather than more because the next band must start on screen at
  1000 px. Folding is off whenever any filter is set. "Select all shown"
  selects the rendered rows, not the folded remainder, so what the tray says
  is what you can see.
- **Forgotten pages are counted by their flag, in both modes.** Export omits
  them from `pages[]` and reports the number in `stats.forgotten`; serve
  includes them flagged. The masthead and the count line count pages that are
  not flagged, which is right in both shapes; the earlier
  `pages.length − stats.forgotten` was right only in serve and short by
  `stats.forgotten` in export. `cases/export-forgotten.json` is the export
  shape and pins this.
- **An empty archive is a page, not a stack trace.** Zero snapshots and zero
  pages render a card that names `knowmoretabs save`, an empty list that says
  "No pages yet" without a "clear the search" that would do nothing, and a
  Snapshots view that says the same. `cases/empty.json` and
  `cases/no-pages.json` pin it. One snapshot says "taken Sep 20, 2026" rather
  than "Sep 20 to Sep 20" and skips the "present in every snapshot" clause.
- **Only `http(s)` titles are links.** `chrome://`, `data:` and anything else
  render the title as plain text: Chrome will not open them from a link
  anyway, and a `javascript:` URL from a hostile session file must never be
  an `href`. The address column shows the URL as it is, scheme and all: an
  address with the `https://` filed off is not the address, and the row that
  says `chrome://settings/` and the row that says `https://example.com` should
  be telling the same kind of truth. A page with no domain (`data:`) gets no
  "Only" button and no datalist entry.
- **Titles get `dir="auto"`.** A right-to-left title ellipsises on its left
  and reads correctly; the address stays left-to-right. Long titles and long
  URLs ellipsise in the row; the full URL and the CLI command wrap anywhere
  in the history.
- **A degraded snapshot says so.** From the snapshot's `stats` (see §5), the
  Snapshots table and the snapshot header append "incomplete: 3 tabs dropped,
  2 unknown records, 1,024 bytes truncated, no end marker" in the same italic
  serif as the band headings, no colour; when `degraded` is set and every
  named counter is zero (a navigation fallback, a group without metadata) it
  says "incomplete: parse degraded". A snapshot that lost tabs should not
  quietly report a low count. `cases/degraded.json` carries all three shapes.
- **Unreadable data is skipped and counted, never fatal.** The document is
  our own backend's, but a hand-edited or half-written one degrades the way
  the parser does. A tab row that is not a tuple naming a readable page is
  dropped and the Snapshots view says "incomplete: 2 unreadable tab rows"; a
  snapshot without a parseable `captured_at` is skipped and the footer says
  "1 snapshot in the data could not be read"; a page without a URL is a page
  with no sightings; a group with no title or colour is unnamed and grey;
  missing `stats`, `groups`, `tabs_total` or `windows` fall back to empty or
  to what the rows say. Anything derive() still cannot read shows "Could not
  load the library (…)" in the card instead of "Loading…" forever.
- **Two native selects, not nine chips.** Show and Sort are `<select>`
  elements dressed as a word and a chevron; every option is one keystroke
  away and the toolbar is one line. Site and Group stay text fields with a
  `<datalist>` because 163 buttons do not fit anywhere. (Earlier: segmented
  button groups with `aria-pressed`, which put nine labels at the same
  volume as the search field.)
- **The Site menu counts what the other filters leave.** It used to be built
  once at boot from the whole archive, so a search for "mcp" showing 16 pages
  still offered "youtube.com · 75 pages", and clicking it gave nothing: a
  count that is a lie about what the click will do. The menu is now built when
  it opens, from one predicate shared with the list (`matcher`), with its own
  filter skipped — skipped, or choosing a site would collapse the menu to the
  site you just chose and leave you nowhere to go. One pass over the pages,
  0.1 ms on 567 of them, and only for the picker you reached for: nothing is
  spent per keystroke. For the same reason, text in the field stops narrowing
  the menu once it exactly matches an option, since at that point it is the
  filter you applied and the menu is how you change it.
- **All rows in the DOM, no virtual scrolling.** 2,160 rows render in 22 to
  33 ms (§6). `content-visibility: auto` keeps layout and paint proportional
  to what is on screen. Virtualising was rejected because it breaks
  find-in-page, screen readers and `scrollIntoView`.
- **String rendering plus CSSOM for the strips.** Rows are built as one HTML
  string and set with `innerHTML`. The strip gradient cannot go in a
  `style=""` attribute because the CSP has no `'unsafe-inline'` for styles,
  so it is set with `style.setProperty` in a loop after insertion. Group
  colours are `data-c` attributes resolved by the stylesheet for the same
  reason.
  A stacked strip is one element still, not a line of children: each line of
  marks is a background layer, and since every page in one archive has the
  same geometry, the sizes and offsets (`--gs`, `--gp`) are written once on
  the root and only the gradients (`--g`) are per row. So a four-line strip
  costs the same one `setProperty` per row that a one-line strip did, and
  1,940 rows still render in 10 to 14 ms.
- **No view transitions.** `startViewTransition` defers the DOM swap to the
  next frame, which broke focusing a deep-link target inside a still-hidden
  section.
- **Sticky band headings, non-sticky toolbar.** `/` is the sticky search.
- **Sightings are derived in the browser** from per-snapshot tab tuples, in
  one pass; each sighting keeps a reference to its group object, which is how
  a page's group history costs nothing extra in the payload.
- **Deep links to pages use the array index**, stable only within one export.
  They exist to get from a snapshot row back into the library.
- **Times are shown in UTC and say so** — except the row, which says how long
  ago instead: "12m ago", "3d ago", "4mo ago". A row answers "is this still
  warm?", and an age answers it without arithmetic; the exact UTC stamp is the
  row's `title` and the history, the Snapshots table and the snapshot header
  still carry the date in full. `Intl.RelativeTimeFormat` does the wording and
  the locale, so the whole thing is a six-line table and no dependency: a
  package for this would be the first thing the page ever fetched or bundled.
- **The deep-linked tab keeps ink on paper.** A row arrived at from
  `#snapshot/<id>/<tab>` is the cursor, and the cursor inverts; inverting a row
  that is also tinted left paper text on a pale tint. The target keeps the
  tint, black text and a rule in the margin, and inverts only under the
  pointer, like any other row.
- **Open in a new tab from the row's own anchor.** `↵` clicks the title link;
  there is no `window.open`.
- **Rejected: favicons, thumbnails, colour by domain, progress indicators,
  confirm dialogs, "mark as read", a total-pages badge in the tab title.**

## 5. JSON contract

This is what `knowmoretabs export` embeds and `knowmoretabs serve` returns
from `GET api/library`. `fixtures/library.json` is a valid instance and the
exemplar for the Rust contract test, so it carries only the fields the backend
emits today.

```jsonc
{
  "schema_version": 1,
  "generated_at": "2026-09-21T16:01:52Z",      // ISO 8601 UTC
  "stats": {                                   // informational; UI recomputes what it shows
    "pages": 2166, "snapshots": 41, "domains": 163,
    "sightings": 8478,
    "forgotten": 6                             // count of forgotten pages, even when omitted
  },
  "snapshots": [                               // ASCENDING by captured_at; index is referenced below
    {
      "id": "2026-03-14-091202Z",              // the snapshot directory name
      "captured_at": "2026-03-14T09:12:02Z",
      "browser": "chrome", "profile": "Default",
      "windows": 4,                            // window count in this snapshot
      "tabs_total": 126,                       // rows in tabs[] (duplicates included)
      "stats": {                               // the parser's counters, always present
        "dropped_tabs": 0, "unknown_commands": 0, "malformed_commands": 0,
        "truncated_bytes": 0, "marker_ok": true,
        "degraded": false                      // true for degradation no counter above names
      },
      "groups": [ { "id": 0, "title": "Later", "colour": "purple", "collapsed": false } ],  // may be []; title may be ""
      "tabs": [
        // [page_index, window, position, tab_id, pinned, group_index_or_null]
        [1, 1, 0, 101, 1, null],
        [2, 1, 1, 106, 1, 0]
      ]
    }
  ],
  "vocabulary": [                              // active names; alphabetical ignoring case; always present
    { "name": "Harness", "created_at": "2026-09-24T10:00:00Z" }
  ],
  "pages": [                                   // one entry per distinct URL, any order
    {
      "url": "https://github.com/ogham/eza",
      "title": "GitHub - ogham/eza: A modern alternative to ls",   // latest known; may be ""
      "domain": "github.com",                  // host, lowercased, leading "www." removed; "" for data: etc.
      "forgotten": false,                      // optional; absent means false
      "tags": ["Harness"],                     // active names, vocabulary spelling, alphabetical ignoring case; [] if untagged
      "history": {                             // optional; absent when no snapshot recorded signals for the URL
        "visits": 12, "typed": 3,              // what the browser still kept, about 90 days
        "first_visit": "2026-07-01T09:12:44Z", "last_visit": "2026-09-23T23:49:42Z",
        "foreground_seconds": 1834,            // optional
        "search": { "term": "eza ls", "hops": 1 },                            // optional; serve, or export --with-history
        "referrer": { "url": "https://…", "title": "…" }                      // optional, as search; title only when the URL is a page here
      }
    }
  ]
}
```

Rules the backend must keep:

- `snapshots` is ascending by time. The client uses the array index as the
  strip column and as the sort key for first/last seen.
- Every `page_index` in a tab tuple is a valid index into `pages`. A page
  with no tab rows is allowed; the client never shows it.
- `tab_id` is unique within one snapshot (it is the deep-link target).
  `window` is 1-based. `position` is 0-based within the window. `pinned` is
  0 or 1. `group_index` indexes that snapshot's `groups` or is `null`.
- `colour` is one of Chrome's nine names (`grey blue red yellow green pink
  purple cyan orange`); anything else renders as grey. `collapsed` is the
  group's state when the snapshot was taken.
- `stats` and `collapsed` are emitted on every snapshot and every group, zeroed
  and `false` when the parse was clean, so their shape is constant. `degraded`
  is the parser's own verdict and covers degradation the five counters do not
  name (a navigation fallback, a group with no metadata), which the Snapshots
  view reports as "parse degraded".
- Export omits forgotten pages and their tab rows, and reports the number in
  `stats.forgotten`. Serve includes them with `"forgotten": true`.
- Each page's `tags` and the top-level `vocabulary` are always present, even
  when empty. Every page tag is an active vocabulary name. A retired name
  stays in the state file; bringing it back restores it on every page that
  had it. An older document lacking both keys reads as untagged.
- A page's `history` comes from the newest snapshot that recorded signals for
  its URL, and every field in it is optional. Export leaves out `search` and
  `referrer` unless `export --with-history` is given. A referrer is never a
  page on this machine, and an export never names a forgotten page as one.
  `referrer.title` is the library's own title for that URL, when it is a page.
- Unknown fields anywhere are ignored by the client. Add, never rename.
  Rows the client cannot read are dropped and counted, not fatal (§4).
- The export host writes the JSON into
  `<script id="library-data" type="application/json">` between the two
  `library-data` marker comments, with every `</` escaped as `<\/` (still
  valid JSON). The serve host serves the same `index.html` with that element
  empty.

Serve API, all same-origin, all JSON:

| method | path | body | response |
|---|---|---|---|
| GET | `api/library` | | the document above |
| POST | `api/forget` | `{ "urls": ["…"] }` | `{ "urls": ["…"], "counts": {…}, "forgotten": ["…"] }` |
| POST | `api/restore` | `{ "urls": ["…"] }` | `{ "urls": ["…"], "counts": {…}, "restored": ["…"] }` |
| POST | `api/tags` | `{ "urls": ["…"], "add": ["…"], "remove": ["…"] }` | `{ "urls": ["…"], "tags": { "<url>": ["…"] }, "vocabulary": […], "counts": {…}, "undo": {…} }` |
| POST | `api/tags` | `{ "urls": [], "undo": {…} }` | the same response, including an inverse `undo` |
| POST | `api/vocabulary` | `{ "create": ["…"], "retire": ["…"] }` | `{ "vocabulary": [ {"name": "…", "created_at": "…"} ] }` |

For forget and restore, `urls` contains only URLs whose flag actually
changed, in request order with repeats removed. The compatibility key
(`forgotten` or `restored`) contains the same array. `counts` contains
`changed`, `unchanged`, `unknown` (counts of distinct requested URLs), and
`forgotten` (the total forgotten pages still present in the archive
afterwards, matching `GET api/library`’s `stats.forgotten`). Unknown URLs are counted and skipped; known URLs in the
same request still change. Repeated requests succeed with an empty `urls`
array when nothing changes.

`api/tags` takes either list empty or omitted, but not both. Names are trimmed,
inner whitespace is collapsed, and the result must have 1–40 Unicode characters
and no control characters. Matching ignores case and keeps the vocabulary's
spelling. Adding an unknown name creates it; adding a retired one revives it
on every page that had it. A bad name or a name in both lists returns 400
without writing. Unknown URLs are counted, never stored; forgotten pages
are still known URLs.

`tags` contains every known requested URL with its visible tags afterwards.
`urls` lists the distinct requested URLs whose visible tags changed, in request
order. `counts` has `changed`, `unchanged` and `unknown`. `vocabulary` is the
full active list. A newly visible name might have been revived, so the client
refetches `api/library` to learn about affected pages outside the request.

**Undo is exact.** Swapping `add` and `remove` undoes a request for one
active name, but not one that names several tags over a mixed selection, and
not one that revived a retired name. So the response also carries `undo`, the
previous decisions for just the pages and names it changed:

```json
{
  "tags": [{"url": "https://example.test/", "name": "Harness", "add": false, "remove": false}],
  "vocabulary": {"Old": "2026-09-24T10:00:00Z"}
}
```

Send this object back as `{"urls": [], "undo": <object>}` to `api/tags`.
It cannot be combined with normal edits. The two booleans restore that name's
membership in the page's `add` and `remove` lists; both cannot be true.
`vocabulary` maps a touched name to its old `retired_at`, or `null` for active.
The server restores these decisions under one lock and atomic write, preserving
unrelated names and pages. New vocabulary entries remain as zero-count tags.
Duplicate decisions and invalid names are rejected before writing. Unknown URLs
are skipped and counted. The response includes a new inverse, so undo can be
undone. When retirement changes, the returned `tags` covers every known page;
its changed URLs are sorted by URL.

`api/vocabulary` takes `create` and `retire` lists, not both empty. Creating a
retired name brings it back; retiring an unknown name is ignored. Retirement
records a time and never alters a page's decisions.

All POSTs require `application/json` and use the same loopback bind, Host and
Origin defenses. Forget and restore change the page at once, and a non-2xx
makes the client revert it and say so in the toast. Tagging and the vocabulary
wait for the answer instead (§9). Paths are relative, so the UI can be mounted
under any prefix.

## 6. Measurements

Chrome 153 headless (new), 1400×1000, this Mac, embedded fixture, no
virtual time, driven over the DevTools protocol. Numbers are from
`performance.now()` around `render()` and from
`document.documentElement.dataset`, which the app sets for exactly this
purpose. The timings are the redesign's, on that Chrome; the September tweaks
(strip, index/checkbox cell, age, tray) were not re-measured against them, and
nothing in them changed the shape of a render. The byte count is current.

| what | measured |
|---|---|
| first render, folded (1,940 rows, 612 group marks) | 25 to 26 ms |
| 7 consecutive re-renders, folded | 22.1 to 32.6 ms, typically 22 to 23 ms |
| full render, unfolded, 2,160 rows | 27.5 to 28.3 ms |
| search "rust" re-render (135 rows) | 3.6 ms |
| boot: parse 510 KB JSON + derive 8,478 sightings + snapshots table + first render | 61 to 66 ms |
| serve mode boot, over `fetch` | 72 ms |
| CSS + JS | 31,295 + 41,167 = 72,462 bytes (the September tweaks added 7,548) |

Keystroke rendering is coalesced with `requestAnimationFrame`.

**The byte ceiling for this slice was ~38 KB and this is 41.8 KB.** B was
33.0 KB. The slice-2 review added 1.4 KB: the unreadable-data handling in
derive() and the counts it reports (§4), the key guard while the help dialog
is open, and the wording for a snapshot without a known browser. The 7.4 KB
before that went to: tab groups end to end, marks, the history, the
filter field, the datalist and the nine colours (about 3.0 KB); the "Open
now" fold with its cursor handling and the rendered/shown split it needs
(about 0.9 KB); the empty, one-snapshot and degraded states (about 1.2 KB);
the real-data edges, non-link titles, `dir="auto"`, empty domains, wrapping
(about 0.7 KB); the reassurance copy in two places and the help text (about
0.4 KB); comments explaining the above (about 0.6 KB); the phone layout for
the group mark and the snapshots table (about 0.3 KB). Groups alone would
have fit under the ceiling; groups plus the rest do not, and nothing here is
padding. Everything is unminified.

## 7. Accessibility and safety, what was checked

- Real elements: `<header>`, `<nav aria-label>`, `<main>`, `<section
  aria-label>`, `<form role="search">`, `<ol>`/`<li>` rows, `<table>` with
  `<th>`, `<time datetime>`, `<dialog>`, `<kbd>`, `<output aria-live>`.
- Buttons carry `aria-pressed` (segments) and `aria-expanded` (history).
  Group marks that filter are buttons with an `aria-label`; the ones that do
  not are spans with a `title`. Checkboxes have labels. The strip has a
  `title` with the count in words.
- Focus is always visible: a 2 px amber outline, inset on rows so it is not
  clipped, and the cursor row also has a blue edge for mouse users.
- Light and dark from `color-scheme: light dark` and `light-dark()` tokens;
  `<meta name="color-scheme">` so form controls follow. Both verified by
  emulating `prefers-color-scheme` explicitly (the host Mac is in dark mode,
  so "not emulating" is not "light").
- 390 px phone: no horizontal overflow on Pages, Snapshots and a snapshot
  (`scrollWidth 390/390`); strip hidden; title stacked over URL with the
  group mark beside the URL; segments wrap into chips; the snapshots table
  wraps. 700 px (200% zoom of a 1400 px window): no overflow, everything
  wraps.
- `prefers-reduced-motion` disables the only transition.
- CSP: `default-src 'none'; script-src 'self'; style-src 'self';
  connect-src 'self'; base-uri 'none'; form-action 'none'`. No
  `'unsafe-inline'` anywhere. `<meta name="referrer" content="no-referrer">`.
  Every external link is `target="_blank" rel="noopener noreferrer"`. Under
  `file://` the page makes zero requests; under serve it makes exactly the
  ones in §5. No CSP report and no console error in either mode, on the
  fixture, on every case, and on a real export from this machine.

Not verified: a screen reader pass with VoiceOver, and Safari and Firefox
rendering (only Chrome was scriptable here). `light-dark()`, container
queries, `:has()` and `content-visibility` are all in Baseline 2024 or
earlier.

## 8. What is still open

**The group filter matches by name, not identity.** Chrome gives every group
a token, but a token is per session and the contract does not carry it, so
two groups both called "Later" a month apart are one filter. That is
probably what a person means. If it is not, the contract needs a stable
group identity, and `saved_guid` from slice 1 is the candidate.

**Ten rows is a guess.** The fold answers "what do I have open" with the
pinned head and a link. Someone whose open set is the interesting part of
their archive will click "Show all" every time; if that turns out to be most
people, the number should grow or the fold should remember its state.

**`list` and the Snapshots table count tabs differently.** `knowmoretabs
list` reports every tab in the snapshot; `tabs_total` is the rows in
`tabs[]`, after local and forgotten pages are removed, so one snapshot can
say 129 tabs in the terminal and 126 on the page. The contract chose the
second. Whether the page should also say how many rows were left out, and
why, is open.

## 9. Tags: a ruled index, a line of chips, one editor

Tags are flat facets the owner makes by typing a name (brief §2, §3). The
page uses them in two ways: to **filter** by combining them, and to **tag** a
page or a selection. There is one piece of furniture for each: the tag bar
filters, and the tagger edits. Neither adds a colour, a badge or any motion
beyond disclosure.

**The tag bar is a ruled index under the toolbar.** It is a strip with a
`TAGS` label cell the width of the index column, so it lines up with the `?`
cell above it and the row numbers below. Next to the label is a grid of equal
cells, each holding a name and its count, ruled like the toolbar. Collapsed,
the bar is one line. The grid reports its own column count, and the last cell
of the line becomes "36 more". Opening it discloses the whole vocabulary as a
table of ruled cells, which ends with "Fewer" and (serve only) "Retire tags…".
At 1400 px the line holds seven tags. On a phone the cells narrow to 6.5 rem
and it holds two.

- **Selecting several tags means all of them.** A picked tag moves to the
  front of the bar and inverts, like the cursor row. Its count becomes a ×,
  and clicking it again drops it. Two picked tags stay two cells, with a
  hairline between them. The key stays on the tag as the bar reorders; if
  dropping it sends it past the end of the line, it goes to "36 more".
- **Every count is a co-occurrence count.** A count is how many of the pages
  on show also carry that tag, after search, site, group, status and the
  picked tags. So with Harness picked, "Skills 47" means 47 Harness pages are
  also Skills. The counts use the same `matcher` as the list and the Site
  menu, so a count never lies about what a click will give.
- **Tags with no pages on show drop out.** A tag no shown page carries has no
  cell, since clicking it would give nothing. When the view has no tagged
  pages at all, the bar says "None of these pages is tagged."
- **Order.** The bar is sorted by count, then by name, so the combination you
  did not know to look for rises to the front.
- **An empty vocabulary.** This is where every new user starts (§9 of the
  brief). In serve mode the strip stays, and the cells give way to one quiet
  line: *No tags yet. Press + on any row to add one.* It is the page's only
  pointer to a feature you have not used yet, and it sits where the tags will
  appear. Export shows no strip until there are tags, because it cannot make
  any. A tag that no page carries any more (an undone first tag, say) leaves
  "None of these pages is tagged." with "Retire tags…" in the last cell.

The tag filter is not in the URL, because no filter is. The hash carries
views and deep links only. A deep link that has to reveal a hidden page
clears the tags along with the other filters. Clear and `esc` clear them too,
and a tag filter unfolds "Open now", like any other filter.

**Chips get a line of their own under the address.** A tagged row has three
lines: the title (with the age on its baseline), the URL (with the group mark
at its end, where it always was), and then the chips. The chips are aligned
with the title and URL text, 0.45 rem below the URL, so they read as a line
and not as part of the address. An untagged row keeps its two lines and its
height, 61 px at 1400; a tagged row is 88 px. The first version put the chips
at the end of the URL line. The owner found that crowded, and the owner was
right: the URL, chips and group mark were competing for one line.

- **The cap is 8 chips or 80 characters, then "+N".** The old cap of 4 chips or
  30 characters was set by the width left over on the URL line, and a line of
  their own is about 890 px at 1400. On the owner's archive, 97% of tagged
  pages have 8 tags or fewer, so nearly every row shows its whole set. A full
  line of 80 characters is about 640 px, so it never wraps at desktop widths
  and the row keeps air at the right. The rare 14-tag page shows 8 and "+6",
  not a wall. The "+N" tooltip names the rest, and the history drawer's
  summary lists them all ("… · tagged Agent, MCP, Skills").
- **Order.** Chips are alphabetical, except that the picked tags go last:
  every row on show carries them, so they are the least news. They are drawn
  in ink, so the row still shows why it matched.
- **A chip filters.** Clicking one picks that tag, the way a group mark picks
  its group. It never opens the drawer.
- **Narrow screens.** The chip line wraps, so on a phone a heavily tagged row
  can take a fourth line.
- **Offscreen rows and placeholders.** Tagged rows carry a `tall` class, so
  `content-visibility` estimates them at their real height. The lazy
  placeholders count the tagged rows they stand for (`--tall` × `--tagline`),
  so scrolling into them does not jump.

**`+ tag` and the tagger: one editor, moved to where it is needed.** On a
tagged row, a dashed `+` ends the chip line. An untagged row has no chip line,
and the owner asked that it not reserve one, so its `+ tag` waits at the end
of the URL line, before the group mark. It shows under the pointer and on the
cursor row. The rest of the time it is transparent but keeps its place, so a
hover never moves anything; at most a very long URL ellipsises a few
characters earlier. A chip line that appeared on hover would make the row
jump under the pointer, which is exactly what hover must not do. Clicking
`+ tag`, or pressing `+`, opens the editor, and that does give the row its
third line. That growth is the answer to an action you took, the same kind of
disclosure as the history drawer, not motion on hover.

`+` (or `=`, the same key unshifted) opens the editor on the selection if
there is one, otherwise on the cursor row: the same targets `f` uses.

- **On a row,** the editor takes the chip line: the page's chips with ×, then
  a field. The title, the URL and the group mark stay where they are.
- **On the tray,** it adds a line above the tray's actions and edits the whole
  selection. A chip on only some of the selected pages shows how many carry
  it ("Agent 2"). Its × removes the tag from all of them, and adding a tag
  adds it to all of them.
- **While it is open,** only the chips and the bar's counts repaint, so the
  list does not move under the pointer. The list catches up when the editor
  closes. A row that no longer matches the filter leaves then, not while you
  are typing into it.
- **The field is the page's own combobox.** It is `dropdown()`, the same one
  Site and Group use, with an `own` flag meaning "the caller filters and
  orders the options". So the keys, the menu, the highlight and the ARIA are
  shared code, not a second copy.

**Suggestions and creating a tag.** The field suggests from the vocabulary as
you type:

- **What is offered.** The name you typed exactly, in any case, comes first,
  then names that start with it, then names that contain it. Each group is
  sorted busiest first, with the library count beside each name. Tags every
  target already has are left out.
- **Creating.** A name the vocabulary lacks (matched case-insensitively) is
  offered last, as "new tag". So `↵` on a fragment finds the tag
  you meant rather than minting "Ag". Typing the exact name of an existing
  tag in any case picks that tag, in its spelling.
- **Names.** Runs of whitespace become one space, as the server does it. The
  field has no `maxlength` and cuts nothing: the server counts the 40
  characters (a `maxlength` counts UTF-16 units, which cuts an emoji short),
  and a name it refuses stays in the field with its reason in the toast.
- **Keys.** `↑` `↓` move, and `↵` adds and keeps the field open for the next
  tag; the menu comes back on typing or `↓`. `esc` closes the menu, then clears
  the field, then closes the editor and returns to the row (steps 2 to 4 of
  §3). `↵` on an empty field closes it too. With the menu shut by `esc`, `↵`
  adds the name as typed, since the suggestions were refused: "Ag" makes Ag,
  an existing name in any case is that tag, and a retired one comes back.
  (It used to do nothing: the menu took `↵` only while open.)
- **Placement.** The menu opens under the field. When the row is too near the
  bottom of the window, the page first scrolls to make room. At the very end
  of the list the menu opens upward instead, clear of the title and URL. A tag
  retired on this visit is offered as "retired · brings it back", not as a new
  tag.

**Undo covers every change.** Each change is one name on some pages, and the
toast says so: "Added Voice to 4 pages · Undo", "Removed MCP from 1 page ·
Undo". `u` works from the keyboard, once per press. Tagging and retiring wait
for the server before they change the page. It is this machine and answers in
milliseconds, and its answer settles the spelling and which pages changed, so
the page needs no local diff and no rollback. The answer also carries the
decisions it replaced (§5), and undo sends those back. So undoing an add on a
mixed selection leaves alone the pages that already had the tag, and undoing a
revival retires the name again. A revival is not an edit of the pages you
tagged, so the toast does not count them: typing a retired name says
"Security is back on 24 pages", counted over the library once it is reloaded,
and its undo says "Retired Security; 24 pages no longer show it", as the
dialog does. The page knows a name came back because the answer's `undo`
keeps an old retirement time for it (and `null` for one it retired); a name
that is new to the page was not necessarily a revival, and only a revival
needs the reload. An answer with nothing to undo (another tab got there
first) says nothing, as forget does with nothing to forget: the page takes
the tags it reports, the field clears, and the last undo still stands. A
failed request changes nothing, the toast
gives the server's reason, and what you typed or could undo is still there to
try again. If the write stands but the library cannot be reloaded after it,
the toast says it was saved and asks for a reload.

**Retiring is in a dialog, reached from the open bar.** "Retire tags…" opens
a small dialog, like the `?` legend. It lists the vocabulary with page counts
and a Retire button per tag, quiet until the pointer is on its line. The copy
under the title is the reassurance, in the place where the hesitation happens:
*Retiring a tag takes it off the bar and off every page. Nothing is deleted:
the library keeps it, and adding it to a page again brings it back.* Retiring
hides the tag everywhere at once. The dialog keeps the retired tag for the
rest of the visit, struck through, with "Bring back" on the button that had
focus, because the undo toast sits behind the modal. Bringing a tag back
(from the dialog, or by typing its name on a page) returns it to every page
that had it. Since the response covers only the pages in the request, the
page asks for the library again whenever a name new to it comes back.

**Export mode shows tags and edits nothing.** The bar, the counts, the
multi-tag filter and the chips all work from `file://`. There is no `+ tag`,
no tray button and no Retire cell. `+` shows the exact command with a Copy
button, the way `f` does for forget:
`knowmoretabs tag '<url>' --add NAME`. The URL is shell-quoted, so one with
an apostrophe still pastes as a single argument.

**The host seam grows by three calls.** `host.tag(urls, add, remove)`,
`host.undoTags(undo)` and `host.vocab(create, retire)` exist only in serve.
The page asks whether `host.tag` exists, never which host it is.

### Decisions, and what was rejected

- **A ruled grid for the bar, not a line of words.** A flowing line of
  "Agent 173 · Harness 132 · …" fits about fourteen tags where the grid fits
  seven, but it is the one part of the page that would not be ruled. The grid
  is the toolbar's own language, its counts align in a column, and opened, it
  reads as an index of the library rather than a cloud. Rejected: a tag cloud
  sized by count (loud, and it reorders under the pointer); a Tags dropdown
  like Site (a combination you can only build one menu-open at a time, with
  no counts in view); tags as a sidebar (the frame is one column).
- **Chips on their own line, not the address or title line.** The first
  version put them on the address line, which the owner found
  crowded. The title line is the serif and the age, and chips there would
  compete with both.
- **A budget of characters, not a measured fit.** Measuring every row's free
  width would cost a layout per row. A character budget is exact enough,
  since names are short, and the drawer has the full list.
- **Edit in place, not × on hover.** × on every chip under the pointer would
  change chip widths on hover (the row would shift) or cost a × of width
  always. The tagger makes × part of an explicit edit, in the same place.
- **The tagger is one DOM node, moved.** Moving one node means one set of
  listeners and one menu. Rows use `content-visibility: auto`, which contains
  paint and would clip the menu, so the tagging row switches to `visible`
  while the editor is open.
- **`+` as the key.** The page's keys are mnemonic (`f` forget, `u` undo, `x`
  select, `t` theme), and `t` was taken. `+` is the label on the button it
  opens.
- **Retire lives in a dialog, not on the bar.** Retiring is rare and affects
  the whole library, and a filter bar is clicked constantly. Keeping it one
  deliberate step away means it is never hit by accident.
- **No umbrella tags in 7a.** They are derived and optional (brief §2).

### Follow-ups, left out of 7a by decision

- **An "untagged" filter.** A view of the pages with no tags, for tagging
  work. It belongs with 7c's review views ("has suggested tags"), where
  tagging a backlog becomes a job.
- **Tags retired on earlier visits.** The dialog lists what you retired on
  this visit, with "Bring back", but `GET api/library` returns only active
  tags, so older retirements are invisible to the page. Showing them needs the
  API to return retired entries (flagged); `knowmoretabs tags --all` lists
  them today.

### Fixture

`fixtures/generate.py` gives the synthetic library a vocabulary of 40
facets plus one tag that no page carries ("Prompt", to exercise a 0-count
tag). It uses its own seeded RNG, so the rest of the fixture is byte-for-byte
unchanged. Tags are drawn from themes (agents, models, engineering, design,
life) chosen by the kind of page, which gives the co-occurrence counts a real
shape. 922 pages have no tags, most tagged pages have one to four, and 38
have eight or eleven. The one-snapshot case stays untagged, with no `tags`
and no `vocabulary` at all, to pin that a library from before 7a reads as
untagged. Since `library.json` is the contract test's exemplar, the export now
has to carry `pages[].tags` and `vocabulary`, and on the merged branch it
does.

### Measurements at the design handoff (Chrome headless, 1400×1000, embedded fixture)

| what | measured |
|---|---|
| re-render, folded (1,940 rows) | 13.4 to 16.6 ms (first version; the chip line changed no render path) |
| render with one tag picked | 5.6 ms |
| boot to first render | 73 ms |
| app.js | 41.2 KB before tags → 60.1 KB first version → 55.5 KB now |
| app.css | 31.3 KB before tags → 37.8 KB now |

The trim from 60.1 to 55.5 KB came from three changes. The editor is built on
`dropdown()` instead of its own menu. Tagging and retiring wait for the server
instead of diffing and rolling back locally. And the comments were cut back to
the file's density: 15.4% of the bytes, against 14.7% before tags. The
remaining ~11 KB of code is the feature set: the bar and its fitting, chips,
the editor, the retire dialog with bring-back, the export path, and the
wiring. Going further would mean cutting a feature or minifying, and the page
ships unminified on purpose.

### What was checked after review

By hand, in Chrome headless at 1400, 700 and 390 px in both schemes, against
`knowmoretabs serve` on a copy of a real archive (43 tags on 390 pages). Like
§6 and §7 this is a record, not a suite: the page has no browser test runner,
and a script that reaches into `S` and `host` breaks with every refactor.

- The bar, chips, row editor and tray editor are pixel-identical to the
  design handoff.
- `↵` adds a second and third name without an arrow key; `↵` on the empty
  field closes it and returns to the row. The tray editor, the tag bar
  (including a picked tag folding past "N more") and the retire dialog work
  from the keyboard alone.
- Undoing an add or a remove on a mixed selection, and undoing a revival,
  leaves the tag state in `library.json` as it was before the change.
- With the server stopped, an add keeps the typed name, a × keeps the chip,
  retiring changes nothing, and `u` works once the server is back. A held `u`
  sends one request.
- A reload that fails after a revival leaves the change saved and undoable.
- Export: `+` and `f` offer the shell-quoted command with Copy. No console
  errors in either mode.

app.js is 56.9 KB with these fixes.

### Rough edges after the merge

Six fixes, each its own commit: `esc` as one ordered list (§3), focus kept on
a row through a repaint (§3), a revival that says it is back on every page, a
tag request that changed nothing staying quiet, `↵` adding the typed name
after `esc` has shut the menu (all §9), and "tags" rather than "vocabulary"
in the retire error. Checked in Chrome headless at 1400 px in both schemes
against `knowmoretabs serve` on a fresh copy of the lab archive (43 tags on
390 pages), with single trusted key presses sent over the DevTools protocol
(the harness's own key command repeats each press hundreds of times) and
real mouse clicks: every `esc` use in §3 by mouse and by keyboard alone, `f`
and `u` in the middle, at the end and past the lazy rows, Forget from the
history and the tray, the toast's Undo, and the three tag toasts. The list,
the row editor and the tray are pixel-identical to the merge. Export mode
still offers the commands, and no console errors.

app.js is 58.9 KB (56.9 KB at the merge); comments are 16.7% of it (15.5%),
most of the growth being the reasons behind where the key goes.

## 10. History signals: the drawer, not the row

Since slice 7b, `save` records what the browser's own History knows about each
tab's page (brief `slice-07b-history.md`). The library shows it in one place:
a **Browser history** section in the history drawer, between the actions and
the sightings. The row does not change.

```
BROWSER HISTORY
Found by searching   “github ecc” on google.com
Came from            Introducing System One Models – TypeSafe Blog   typesafe.ai/blog/introducing-…
Visits               1,437 between Jul 1 and Sep 24, 2026 · typed 12 times
Time on page         7 days 5 hours
```

It is a two-column list, labels in the metadata grey and values in ink, one
line per signal, cut with an ellipsis rather than wrapped. Its heading is the
same small caps as "12 sightings", so the drawer now holds two records of the
page: the browser's and the library's. A line with nothing to say is left
out, and a page with no signals has no section at all, so its drawer is
exactly what it was. Below 44 rem the labels stack over their values, and a
referrer's title wraps, with its address cut on a line of its own.

- **Nothing on the row.** Signals exist only for pages seen in a snapshot
  saved since 7b: 71 of 626 pages in the owner's archive today. A mark on the
  row would sit on one row in nine and read as a badge ("this one has
  something"), which the page has rejected since the bake-off, and the owner
  had just moved the chips to their own line because the rows felt crowded.
  The drawer is one key away (`space`). Rejected: the search term under the
  address as a fourth line (the most useful signal, but a line on 2% of rows
  makes the list uneven for little gain); a "typed" or visits count beside the
  age (a score, and the age already answers "is this warm?"); a sort by
  visits or time on page (with signals on one page in nine, the sort would
  mostly order the pages that have none).
- **"Found by searching", with how far away.** The term is quoted and in ink.
  `hops` decides the rest: 0 says "this page is the results", 2 and 3 say
  "2 links away", so a search found through a tweet two clicks back reads as
  what it is. 1 says nothing extra. The tooltip says what the line means.
- **The results page is said once.** When the search is one link back and the
  referrer is that results page (its query string holds the term), the
  referrer line goes and the search line ends "on google.com", which links to
  the results page. On the owner's data that is 4 of 10 searches; showing
  both said the same thing twice, the second time as a 300-character URL.
- **Came from: a title that jumps, an address that leaves.** When the
  referrer is itself a page in the library, its title is a link to that
  page's row (`#pages/<i>`, the existing deep link: it reveals, opens and
  focuses the row, and Back returns). Beside it the address is mono, grey and
  scheme-less, and opens the page in a new tab; with no title the address is
  the value, in ink. A referrer equal to the page itself is not shown (one in
  the owner's archive). Only `http(s)` addresses are links.
- **No filters from here.** A search term is not a filter: the page's search
  matches all words in titles and URLs, and a natural-language query mostly
  matches nothing, not even the page it found. A referrer site is not a
  filter either, though "everything t.co sent me to" is a real question: it
  would be a new filter with no place in the toolbar, and with one page in
  nine carrying a referrer it would mostly return one or two rows. Both are
  under Open, below, to revisit when more snapshots carry signals.
- **Visits in one line.** The count, the dates History still kept ("between
  Jul 1 and Sep 24, 2026", the year once when both share it, "on Sep 23" for
  one day), and "typed 12 times", "typed once" or "all typed". The tooltip says
  typed means typed or picked in the address bar, and that the browser keeps
  about 90 days. `last_visit` is only used here: the row's age already says
  when the library last saw the page.
- **Time on page, not "foreground".** It is the sum of the timed visits' time
  in front, which is what anyone calls time on page. Two units at most, in
  words ("7 days 5 hours", "4 minutes 51 seconds"), from
  `Intl.NumberFormat`'s unit style; `Intl.DurationFormat` is newer than the
  Baseline the page holds to. The tooltip says the visit in progress is not
  counted.
- **Export.** Without `--with-history` the section shows Visits and Time on
  page and nothing says what was left out. Serve shows everything, forgotten
  pages included (in the Forgotten view), since it is the same trust boundary
  that already shows every URL.

The keyboard is unchanged: `space` opens the drawer, and `esc` closes it at
step 5 of §3. The links in the section are ordinary links inside the drawer,
like the sighting links, so they take no key of their own. The `?` legend's
`space` line now says "Show or hide its history: where it was seen and how you
found it."

**Fixture.** `generate.py` gives pages seen in the last three snapshots
synthetic signals (270 of 2,166), with its own seeded RNG so nothing else
moves: a Google results page as referrer for a fifth of them, another page in
the library or a made-up link elsewhere for most of the rest, and searches 0,
2 and 3 links away. Page 0 has none, because the contract test reads the first
page as the shape every exported page must have. `cases/export-forgotten.json`
drops `search` and `referrer`, as an export does. `library.json` grew from
566,869 to 623,264 bytes.

**Size.** app.js 59,016 → 62,646 bytes (+3.6 KB); app.css 37,849 → 39,037
bytes (+1.2 KB).

**Checked** in Chrome headless against `serve` on a copy of the owner's archive
with a fresh snapshot (71 pages with signals, 10 searches, 70 referrers):
light and dark at 1400, 700 and 390 px with no horizontal overflow; a page
with no signals; a search 2 links away, one merged with its Google referrer,
and one where the page is the results; a referrer with and without a title,
and one equal to its page; the title link jumping to the referrer's row and
Back returning; forget from a drawer, the Forgotten view's drawer, restore;
`export` with and without `--with-history` from `file://`; the §3 `esc` order
around an open drawer, with single trusted key presses over the DevTools
protocol; the site filter, a chip, the tray, preview, and tagging and undo on
a row with signals. No console errors.

### Open

- **Say what an export left out.** An export without `--with-history` drops
  searches and referrers silently. Saying so ("searches and referrers are not
  in this export") needs the document to carry the choice, a top-level field
  the contract does not have yet.
- **Search the search terms.** Putting the term (and the referrer's title) into
  what `/` matches would find "that page I found by searching X", but a row
  could then match with nothing visible explaining why.
- **Filter by where pages came from.** See above; worth it once most pages
  carry a referrer.
- **The drawer grows by up to four lines** on pages with signals, and the
  Forget button stays where it was. If the section is read more than the
  sightings, it could sit beside them at wide widths instead of above.
