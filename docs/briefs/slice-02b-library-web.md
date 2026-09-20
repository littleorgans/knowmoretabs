# Slice 2b — The library, frontend

**Ships:** the production web assets at `web/`, consuming real data from
`knowmoretabs export`. This is the face of the product.

## Required reading

1. `docs/BRIEF.md` §1, §2, §8.
2. `docs/design-decision.md` — **the whole thing.** It records a bake-off
   between two blind designs, why B won, and the three changes B needs.
3. `design-b/NOTES.md` — the design you are promoting. Its §5 is the JSON
   contract, which slice 2a has now implemented against.
4. `slices.toml` — slice `library`, and the `future` band.

## What this slice is

`design-b/` is a prototype that works against a fixture. Slice 2a built
`knowmoretabs export`, which emits the same contract from real snapshots. Your
job is to promote the prototype to production, apply the changes
`docs/design-decision.md` calls for, and make it show the things a real archive
has that the fixture did not.

This is not a rewrite. The design won on its merits; most of it should survive
unchanged. Resist the urge to redesign what you are promoting.

## Deliverables

```
web/
├── index.html     the template; export embeds the data blob into it
├── app.css
├── app.js
├── NOTES.md       promoted from design-b/NOTES.md, updated to match reality
└── fixtures/
    ├── generate.py    kept — it is how the frontend is developed without an archive
    ├── library.json
    └── serve.py       kept — a stand-in for slice 3's server
```

Then **delete `design/` and `design-b/`.** Both are preserved in git history and
`docs/design-decision.md` records what each contributed. Two prototype
directories beside a production one is exactly the clutter `docs/BRIEF.md` §2.7
is against.

Slice 2a pointed its `include_str!` at `design-b/` with a comment marking the
seam. Move it to `web/` and remove the comment.

## The changes to make

### 1. The masthead — take A's line

B opens with "Library". A opened with **"Every tab you ever had open."** and A's
line does the emotional work `docs/BRIEF.md` §8 asks for: the archive as
something you are glad to own, not a backlog. Use A's headline with B's subhead
underneath it — B's subhead ("2,113 pages across 41 snapshots, Mar 14 2026 to
Sep 21 2026. 155 sites; 65 pages present in every snapshot.") is genuinely
informative and stays.

### 2. The "Open now" band must stop burying the archive

B's own designer flagged this and was right: the band is 236 rows tall, so
everything else starts below the fold, and it duplicates the "Open now" filter
sitting directly above it. Show the band's first rows and collapse the rest
behind a "show all 236" control. The arriving question still gets answered
immediately; the archive stays reachable by scrolling rather than by filtering
first.

### 3. Reassurance on the forget control

A put it at the point of action: *"Hides it from the library. The snapshots
themselves are never touched."* B says this only in export mode's inline command
hint. It belongs on the live control too, where the hesitation actually happens.

### 4. Tab groups — new, and the fixture never had them

The contract carries `snapshots[].groups` and a group slot in each tab tuple,
and slice 1 captures real Chrome tab groups with their title, colour and
collapsed state. **Neither prototype displayed them, because neither fixture
contained them.** Groups are how people actually organise a hundred tabs, so
this is real signal and it is currently invisible.

Design this yourself — it is the one genuinely open design question in the
slice. Constraints: Chrome's eight group colours must be recognisable but must
not overwhelm a page that already spends its single accent colour carefully; a
page can be in different groups in different snapshots, or in none; and most
archives will have few groups, so it must degrade to nothing gracefully.
Extend `fixtures/generate.py` to produce groups so it can be seen.

### 5. Real data, real edges

The fixture was clean. Real archives are not. Handle, and add fixture cases for:
a page with an empty title; a very long title and a very long URL; non-ASCII and
RTL titles; a `data:` or `chrome://` URL; an archive with one snapshot; an
archive with zero pages; a snapshot where parsing degraded, whose `stats` carry
non-zero counters. That last one should be visible in the Snapshots view — a
snapshot that dropped tabs should say so rather than quietly reporting a low
count.

## Hold the line on

Everything `docs/design-decision.md` lists under "Where they agreed" is settled
by two independent designs converging. Do not revisit it.

Also unchanged: no build step, no framework, no npm, no network of any kind, the
CSP meta tag, `referrer: no-referrer`, `rel="noopener noreferrer"`, light and
dark via `color-scheme`, keyboard-first with the shortcuts visible, and the
single-line row that won B the density argument.

The byte budget was ~25 KB for CSS + JS and B landed at 33 KB. Do not let this
slice push it past ~38 KB. If groups cost more than that, say so rather than
compressing the code.

## Verification

The prototype was verified by driving headless Chrome over the DevTools
protocol, and that bar stands. Check both modes, and specifically:

- A real export from a real archive opens from `file://` and works.
- `fixtures/serve.py` still exercises serve mode, so slice 3 has a target.
- Render time for ~2,000 rows, and the CSS + JS byte count — report both.
- 390 px phone, 700 px (≈200% zoom), no horizontal overflow, dark and light.
- No CSP violations and no console errors in either mode.
- Keyboard: `/`, `j`/`k`, `Enter`, `space`, `f`, `u`, `x`, `esc`, `?`.

## Fences

- You own: `web/**`, and deleting `design/**` and `design-b/**`.
- Do not touch: `src/**`, `tests/**`, `Cargo.toml`, `xtask/**`, `.github/**`,
  `reference/**`, `slices.toml`, `docs/**` (other than reading).
- **One exception:** the `include_str!` path in `src/` that slice 2a pointed at
  `design-b/`. Change that path and nothing else in `src/`.
- `cargo xtask slices --check` must still pass.
- **Run no git commands at all.** Version control is handled outside your run.

## Report

1. Files created, moved and deleted.
2. How you chose to show tab groups, and what you rejected.
3. Measured render time for ~2,000 rows; CSS + JS byte count.
4. The verification list above, with results.
5. What you changed in the promoted design beyond the five items here, and why.
6. What you could not verify.
