# The library interface: two designs, one chosen

Two designers built the library interface independently and blind — neither saw
the other's work, both worked from `docs/BRIEF.md` §8 and the same synthetic
2,000-page fixture. The point of doing it twice was to get two genuine shapes
rather than one shape and one revision of it.

Both are in git history. **Design B is what we build.**

## Where they agreed

Convergence between two blind attempts is the strongest signal available, so
these are settled and not worth revisiting:

- Pages is the default view; Snapshots is secondary. A snapshot is a fact about
  a moment, a page is a fact about you, and the second is what you came for.
- One row per page, with an expandable history of sightings underneath.
- A small graphic showing how often a page was seen, alongside the numeric count.
- Search plus a site filter plus an open/closed filter plus five sort orders,
  all as controls that are visible rather than hidden in a menu.
- Forgetting is reversible, and the interface says so at the point of action.
- No badges, no unread counts, no red. The tally is a set of facts, not a score.
- A single accent colour, mono type for metadata, and no motion beyond disclosure.

## Why B

| | A | B |
|---|---|---|
| Row | title and address stacked | title and address on one line |
| Rows on a 1400px screen | ~13 | ~17 |
| Data payload for 2,000 pages | 810 KB | 487 KB |
| Tab groups in the contract | absent | present |
| CSS + JS | 37.8 KB | 33.0 KB |
| Full render, 2,000 rows | 17 ms script / 55 ms wall | 21–27 ms |
| Verification | manual, in-browser | scripted over the DevTools protocol, both modes |

Three of those decided it:

1. **Density.** This interface exists to make a large archive legible. B's
   single-line row fits about 40% more of it on screen, and at this row height
   the address is still comfortably readable.
2. **The contract carries tab groups.** We decided in slice 1 that groups are
   part of the data model. A's contract has no place for them, so choosing A
   would have meant redesigning the contract before slice 2 could start.
3. **B derives sightings on the client** from per-snapshot tab tuples rather
   than repeating a snapshot reference on every sighting. Same information,
   40% smaller payload, and it scales the right way as snapshots accumulate.

B's expanded page history is also the single best element either design
produced: every sighting as a dated deep link in a dense grid, with the exact
`knowmoretabs forget '…'` command shown inline when the page is read-only.

## What we take from A

- **The masthead.** A opens with "Every tab you ever had open." against B's
  "Library". A's does the emotional work the brief asked for — the archive as
  something you are glad to own — and B's subhead is the better line underneath
  it. Use A's headline over B's subhead.
- **The reassurance copy** on the forget control: *"Hides it from the library.
  The snapshots themselves are never touched."* B says this only in export
  mode's inline command hint; it belongs on the live button too, where the
  hesitation actually happens.

## The one thing to fix in B

The **"Open now" band is 236 rows tall**, so the rest of the archive begins
below the fold — B's own designer flagged it as the decision to argue with, and
they were right to. The band also duplicates the "Open now" filter that sits
directly above it.

Fix: keep the band, show its first rows, and collapse the remainder behind a
"show all 236" control. The arriving question — *what do I have open* — still
gets answered immediately, and the archive stays reachable by scrolling rather
than by filtering first.

## Not carried over from either

Neither design's markup inherits from `reference/b_tabs_export.py`. Its
information architecture — the pages/snapshots split and the per-page history
disclosure — survived into both designs independently, which is the best
evidence that part of it was right.
