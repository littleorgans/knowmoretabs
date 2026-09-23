# Slice 7: Tags

**Status:** approved 2026-09-23. Nothing here is built yet. §9 records the owner's
decisions; §8 is for open questions. Of §7's `BRIEF.md` changes, 1 is applied; 3's
README wording lands with 7b, when `enrich` exists.

**Ships, in four parts:**

| Part | Adds | Network | Leaves the machine |
|---|---|---|---|
| 7a | Tags you set, and filtering by combining them | none | nothing |
| 7b | `knowmoretabs enrich`: page metadata | opt-in, to the pages' own sites | the URLs you fetch |
| 7c | `knowmoretabs tag`: suggested tags, via a prompt you run in your own agent | none | nothing: you hand the folder to your agent |
| 7d | Learning from your decisions | as 7c | as 7c |

Each part ships on its own. 7a is useful without the other three and breaks
none of the product's current promises.

## 1. Why now

`slices.toml` recorded `notes-tags` as waiting until "the shape of what people
want to record is known, rather than guessed". A lab experiment (outside the
repo, on one real library of 592 pages) established that shape.

- **Tags are flat facets that the owner controls.** The meaning is in the combination. Harness + Skills
  means harness skills; Skills alone may also surface a sports skill, and that
  is fine. Broad tags are wanted, not avoided. There is no hierarchy and there
  are no "X, not Y" rules.
- **Filtering is how tags are used.** Select one tag, and every other tag shows
  how many of those pages carry it (Harness → Skills 47, Orchestration 41,
  Factory 29 …). That count is how you find a combination you did not know to
  look for.
- **Most of a library can be tagged automatically, but not perfectly.** Two
  independent taggers agreed on 655 tag assignments and disagreed on 821.
  Suggestions therefore need a review step, and the owner's decisions must
  always win.

## 2. The model

- **Vocabulary.** A set of tag names the owner controls. The owner creates a tag
  by typing a new name and retires one by dropping it. A retired tag is
  recorded, not deleted, so history stays readable.
- **Page tags.** For each URL, the tags the owner added and the tags they removed.
  Stored as `add` and `remove` from the start so that 7c's suggestions need no
  migration. Before 7c, a page's tags are simply its `add` set.
- **Umbrella tags** (for example AI ⇐ any AI tag) are optional, derived, and
  never assigned. They are displayed but not stored per page.
- **Suggestions** (7c) are imported from the owner's agent, with their source (the model name),
  the date and the vocabulary version; they have no scores. A page's tags are
  then its suggestions (confirmed when two sources agree), plus the owner's
  additions, minus the owner's removals.

## 3. Part 7a: tags you set, offline

**In scope:**

- `library.json` gains
  - `tags: {url: {add: [...], remove: [...]}}`
  - `vocabulary: {name: {created_at, retired_at?}}`

  It uses the same atomic, lock-guarded writes as `forgotten`.
- `POST /api/tags` takes `{"urls": [...], "add": [...], "remove": [...]}` and
  mirrors `/api/forget` and `/api/restore`: the same request with the lists
  swapped is its undo.
- `POST /api/vocabulary` takes `{"create": [...], "retire": [...]}`.
- The serve shape of `GET /api/library` gives each page a `tags` array. The
  export shape does the same, read-only.
- **Frontend:**
  - a tag bar with co-occurrence counts, where selecting several tags means all of them;
  - tag chips on each row;
  - `+ tag` on a row and on the **selection tray**, so bulk tagging uses the
    existing multi-select;
  - typing a new name creates a tag, case-insensitively matched to existing ones;
  - × removes a tag, and the undo toast covers it.
- CLI: `knowmoretabs tag <URL>... --add NAME --remove NAME` and
  `knowmoretabs tags` (the vocabulary with counts).

**Out of scope for 7a:** anything automatic; notes; tag colours; renaming a tag
(retire one and create another).

**Acceptance:**

- tagging, untagging and bulk tagging work;
- undo symmetry holds;
- a retired tag disappears from the tag bar but stays in `library.json`;
- export shows tags;
- `library.json` written by 7a is still read by the current code, and a
  `library.json` without `tags` reads as untagged;
- the three serve defences (bind, `Host`, `Origin`) apply to the new endpoints.

## 4. Part 7b: `knowmoretabs enrich`, opt-in network

For each library URL that has no metadata, it fetches the page's `<head>`
without cookies:

- title, description, `og:*` and `twitter:*` tags, JSON-LD `@type`, `lang`
  and canonical URL;
- for public GitHub repositories, the README text and the repository's topics,
  from the repository page's embedded data.

It needs no token.

**Rules the lab established:**

- **Skip:**
  - loopback and private-network hosts;
  - search-result pages (the query is already known);
  - URLs whose query parameters look like tokens, since a GET can spend a one-time link;
  - pages the owner has forgotten.
- **Read until `</head>`, with a 3 MB cap.** YouTube's meta tags start about
  0.7 MB into the page.
- **Pace requests per host** (one per second per host, hosts in parallel).
- **Store per URL with a date, append-only, and never refetch** unless asked:
  `~/.knowmoretabs/pages/metadata.jsonl`.
- **For logged-in apps, keep the tab's own title first.** A cookieless fetch
  sees a login page or a generic title ("Reddit"). Record that the page was
  behind a login and skip it; don't hide it.
- **Flags:** `--dry-run` lists what would be fetched; `--limit N` caps a run.

**Decided (§9):** `save` records the local History signals
for each tab: the search query that led to it, the referrer, the visit count
and the time in the foreground. These come from the browser's `History`
database, copied before reading. There is no network. Chrome keeps about
90 days of history; a snapshot keeps it for as long as the archive exists.

## 5. Part 7c: `knowmoretabs tag`, suggested tags through a prompt handoff

knowmoretabs contains no model, no provider code and no API keys. It writes a
prompt, and imports what comes back.

1. **Rules first**, local and certain, applied by the tool itself:
   - keywords in the title and description, never in README bodies (a passing
     mention is not a topic);
   - GitHub topics;
   - parent tags, only where the implication always holds (e.g. DPO ⇒ Training).
2. **`knowmoretabs tag --prompt DIR`** writes a work folder:
   - `prompt.md`: the owner's guidelines (tags are facets, "substantially
     about", favour recall), the vocabulary with definitions, the exact output
     format, and the validation the agent must run before it finishes;
   - `pages.jsonl`: one line per page not yet tagged, with its URL, title and
     the enriched text from 7b when there is some.
3. **The owner runs it in any agent** (Codex, Claude Code, anything), with any
   model: "Read `prompt.md` and carry it out."
   - The prompt tells the agent to tag by reading, in chunks, and to write
     only inside `DIR`.
   - The owner chooses where the data goes; knowmoretabs sends nothing.
4. **`knowmoretabs tag --import DIR/tags.jsonl [--source NAME]`** validates
   before storing anything:
   - every URL must be one the library knows;
   - every tag must be in the vocabulary, or be a new tag the owner accepts;
   - every line must be well formed.

   Imported tags become suggestions, stored with their source (the model
   name the agent reports, or `--source`), the date and the vocabulary version,
   in `~/.knowmoretabs/tags/suggested.jsonl`.

**Why this shape:**

- The lab proved it end to end. The same brief-in, `tags.jsonl`-out handoff
  gave 592 of 592 pages tagged, with every tag defined.
- It works with whichever model is best next year.
- It keeps the tool's promise literal: `tag` has no network code at all.

**Always:**

- the prompt covers only new or untagged pages;
- nothing runs automatically;
- the owner's decisions override suggestions.

**Review in `serve`:** each chip is one of suggested, suggested by several sources, or yours, with ✓ × ↺. There is a
"has suggested tags" view, and "forget page" is available beside the tags.

**Not in the product:** classifier.dev (Jev). It served the lab as a second,
reproducible opinion. A second source is still possible: the owner imports the
output of two different agents.

## 6. Part 7d: learning from decisions

- `knowmoretabs tag --eval`: each tag's precision and recall against the
  owner's decisions, per provider.
- `knowmoretabs tag --propose-vocabulary DIR`: the same handoff with a different
  prompt. The owner's agent proposes tags for pages that nothing matched; the owner
  accepts them, and they are imported into the vocabulary.
- Later: `prompt.md` includes balanced examples of the owner's confirmed and
  removed tags. Measured caution: examples shifted a classifier's scores about
  ten times more than noise, and made it stricter overall unless balanced
  across tags.

## 7. Changes to `docs/BRIEF.md` and the README (approved)

1. **Non-goals:** "No tag taxonomy" → "No tag hierarchy: tags are flat facets
   the owner controls".
2. **Non-goals:** "No full-text indexing of page contents" stays. 7b stores
   `<head>` metadata and a README excerpt, not page bodies. `fulltext` stays in
   the future band.
3. **README, "Nothing leaves your machine"** stays true for knowmoretabs:
   `save`, `serve` and `tag` send nothing. Add "unless you run `enrich`, which
   fetches the `<head>` of pages you already visited". Say plainly that a
   `tag --prompt` folder holds page text, and that where it goes is the owner's choice.
4. **Principle 6 ("One binary, no runtime")** holds. Only 7b needs an HTTP
   client with TLS in the binary (e.g. `ureq` with `rustls`), a new
   dependency. `tag` needs none.

## 8. Decisions for the owner

None open. New questions go here.

## 9. Already decided

- **Tagging is a prompt handoff.** `tag --prompt` writes `prompt.md` and
  `pages.jsonl`, the owner runs them in whatever harness and model they like on
  their own subscription, and `tag --import` validates and stores the result.
  The tool contains no model, provider code or API keys. Decided 2026-09-23.
- **`save` records History signals** (§4), from the local `History` database, with no network. Decided
  2026-09-23.
- **7a ships first** and is used for a while before 7b and 7c.
- **New users start with an empty vocabulary.** No starter set is shipped;
  `--propose-vocabulary` (7d) is the opt-in bootstrap.
- **Loopback tabs** are left out at `save` and hidden in the library. This is
  on the branch `feat/save-leaves-out-localhost`.
- **Login, signup and verification screens** are the owner's to forget. No
  heuristic hides them; 7b skips fetching them.
- **Tags are facets.** Broad tags, overlapping tags and no exclusions are the
  intended design.
