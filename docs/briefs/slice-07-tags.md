# Slice 7: Tags (draft for review)

**Status:** draft. Nothing here is built. §9 records what the owner has
decided and §8 what is still open. §7 proposes changes to `docs/BRIEF.md` that
need the owner's approval before any code.

**Ships, in four parts:**

| Part | Adds | Network | Leaves the machine |
|---|---|---|---|
| 7a | Tags you set, and filtering by combining them | none | nothing |
| 7b | `knowmoretabs enrich`: page metadata | opt-in, to the pages' own sites | the URLs you fetch |
| 7c | `knowmoretabs tag`: suggested tags | opt-in | page text, to the provider you name |
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
- **Suggestions** (7c) come with provenance: source, model, score, date and
  vocabulary version. A page's tags are then its confirmed suggestions, plus
  single-source suggestions shown as unconfirmed, plus the owner's additions,
  minus the owner's removals.

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

## 5. Part 7c: `knowmoretabs tag`, suggested tags

**Rules first**, local and certain:

- keywords in the title and description, never in README bodies (a passing
  mention is not a topic);
- GitHub topics;
- parent tags, only where the implication always holds (e.g. DPO ⇒ Training).

**Then the providers the owner names on the command line.** None needs an API key:

- `--with codex` or `--with claude`: the agent CLI the owner already has, on the
  owner's existing subscription.
  - Invocations:
    - Codex: `codex exec -m <model> -s read-only --output-schema <file> -o <out> --ephemeral`
    - Claude: `claude -p --model <model> --tools "" --output-format json --json-schema <schema> --no-session-persistence`
  - Each call is headless, with no tools or filesystem access, and returns JSON
    validated against a schema.
  - It receives a batch of pages, the vocabulary with definitions, and the
    guidelines, and returns tags per URL.
  - The owner's plan limits the calls; there is no per-call bill.
  - The tool detects which CLIs are installed and names the one it uses. The
    model is chosen with `--model` and defaults to the CLI's own default.
  - In the lab, an LLM reading each page against written guidelines built the
    better vocabulary and did better on pages with little text.
- `--with classifier.dev`: zero-shot, keyless, free tier.
  - Send **one page per request**. Batching pages changed scores by up to 0.48.
  - Scores are cached per page and phrase, because with one page per request
    each label scores independently.

**Always:**

- only new or changed pages are processed;
- nothing runs automatically;
- every suggestion is stored with its provenance in
  `~/.knowmoretabs/tags/suggested.jsonl`;
- the owner's decisions override suggestions.

**Review in `serve`:** each chip is one of confirmed, unconfirmed or yours, with ✓ × ↺. There is a
"has unconfirmed tags" view, and "forget page" is available beside the tags.

## 6. Part 7d: learning from decisions

- `knowmoretabs tag --eval`: each tag's precision and recall against the
  owner's decisions, per provider.
- `knowmoretabs tag --propose-vocabulary`: an LLM suggests tags for pages that
  nothing matched.
- Later: balanced examples of confirmed decisions sent with each request.
  Measured caution: examples shifted scores about ten times more than noise, and
  they made the model stricter overall unless balanced across tags.

## 7. Proposed changes to `docs/BRIEF.md` (need approval)

1. **Non-goals:** "No tag taxonomy" → "No tag hierarchy: tags are flat facets
   the owner controls".
2. **Non-goals:** "No full-text indexing of page contents" stays. 7b stores
   `<head>` metadata and a README excerpt, not page bodies. `fulltext` stays in
   the future band.
3. **README, "Nothing leaves your machine"** → "Nothing leaves your machine
   unless you run `enrich` or `tag --with …`, which say what they send and to
   whom (your own Codex or Claude subscription, or classifier.dev)". `save` and
   `serve` stay offline.
4. **Principle 6 ("One binary, no runtime")** holds. 7b and classifier.dev need an
   HTTP client with TLS in the binary (e.g. `ureq` with `rustls`), a new dependency.
   The `codex` and `claude` providers call CLIs the owner already has installed;
   knowmoretabs never requires them.

## 8. Decisions for the owner

1. **Which model is the default for `--with claude`,** and does Opus tag better
   than Luna? Measure both through the headless path in §5 before choosing.

## 9. Already decided

- **Model providers are the agent CLIs the owner already subscribes to** (`codex`, `claude`),
  plus keyless classifier.dev. There are no API keys and no per-call billing. It is opt-in: the command
  names the provider and says what it sends. Decided 2026-09-23.
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
