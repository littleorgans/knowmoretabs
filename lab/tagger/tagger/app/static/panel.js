/* knowmoretabs · tagger app · the tag panel (Add link)
   Suggested, one line whatever it holds, and Recently used, three lines
   held open (the tags last put on a page, latest first, as many as three
   rows of its columns fit, nine at most, so none sits clipped out of
   sight); a box
   that filters the tags or names a new one; then every tag,
   folded under one button, in letter rows ("#" first, then A to Z,
   letters without tags left out) of same width cells. A cell is one tag
   on one page: filled when the page has it, striped when picked and not
   yet written. Its place depends on its name alone, so no pick, write or
   stage moves it; only a new tag or the filter does. Typing opens the
   fold to the matching cells; Enter takes the exact match, else the only
   match, else makes the tag when nothing matches. What the panel acts on
   and what a pick does belong to its caller: `K.panel.render(view)`. */
(function () {
  const K = window.KMT;
  const { html, nothing, render, repeat } = K.lit;
  let view = null;     // { has(tag), pend(tag), pick(tag), sugg }: the caller's, as last rendered
  let open = false;    // the fold, as the owner left it
  let making = false;
  let target = 0;      // bumped for each new target: a tag made for an older one is not picked
  const box = K.$("a-tag"), tell = K.strip.sayIn("a-msg");

  /* the row a name sits in: its first letter, accents and case aside; digits and symbols share "#" */
  const lead = (name) => { const c = name.normalize("NFD").charAt(0).toUpperCase().charAt(0); return c >= "A" && c <= "Z" ? c : "#"; };
  function letters(names) {
    const rows = new Map();
    for (const t of [...names].sort(K.byName)) {
      const k = lead(t);
      if (!rows.has(k)) rows.set(k, []);
      rows.get(k).push(t);
    }
    return [...rows].sort(([a], [b]) => (a < b ? -1 : 1));   // "#" sorts before A
  }
  /* active names only, in their order, as many as `rows` rows of the line's columns: none clipped yet reachable by Tab */
  const fit = (names, line, rows = 1) => {
    const active = new Set(K.lib ? K.lib.app_tags : []);
    return names.filter((t) => active.has(t)).slice(0, rows * getComputedStyle(line).gridTemplateColumns.split(" ").filter((w) => w.endsWith("px")).length);
  };
  const RECENT_ROWS = 3, RECENT_MAX = 9;   // Recently used: the rows .rlines holds open (app.css), and its cap
  const matching = (q) => (K.lib ? K.lib.app_tags : []).filter((t) => t.toLowerCase().includes(q));

  const cell = (t) => {
    const on = view.has(t), k = on ? "on" : view.pend(t) ? "pend" : "off";
    return html`<li><button type="button" class="tagcell" data-tag="${t}" data-k="${k}" aria-pressed="${k !== "off"}" title="${t}"><span>${t}</span></button></li>`;
  };
  const cells = (names) => repeat(names, (t) => t, cell);
  const row = ([k, names]) => html`<div class="lr"><span class="ll" aria-hidden="true">${k}</span><ul class="cells">${cells(names)}</ul></div>`;

  function draw() {
    if (!view) return;
    const all = K.lib ? K.lib.app_tags : [], typed = box.value.trim(), q = typed.toLowerCase();
    const shown = q ? matching(q) : all, expanded = !!q || open;
    K.$("a-fold").textContent = `All ${all.length} tag${all.length === 1 ? "" : "s"}`;
    K.$("a-fold").setAttribute("aria-expanded", String(expanded));
    K.$("a-rows").hidden = !expanded;
    render(html`${cells(fit(view.sugg, K.$("a-sugg")))}`, K.$("a-sugg"));
    render(html`${cells(fit((K.lib && K.lib.recent) || [], K.$("a-recent"), RECENT_ROWS).slice(0, RECENT_MAX))}`, K.$("a-recent"));
    render(!expanded ? nothing : shown.length ? html`${repeat(letters(shown), ([k]) => k, row)}`
      : html`<p class="nil">No tag has “${typed}”. Enter makes it.</p>`, K.$("a-rows"));
  }
  const clear = () => { box.value = ""; tell(""); draw(); };

  K.panel = {
    lead,
    /* draw for `view`; Suggested lists the names it holds, best first */
    render(v) { view = v; draw(); },
    /* a new target: the filter empties and the fold closes */
    reset() { target++; open = false; box.value = ""; tell(""); },
  };

  const resize = new ResizeObserver(draw);   // a narrower line holds fewer; no laid out columns means no cells
  for (const id of ["a-sugg", "a-recent"]) resize.observe(K.$(id));
  K.$("a-panel").addEventListener("click", (e) => {
    const b = e.target.closest("button[data-tag]");
    if (b && view) view.pick(b.dataset.tag);
  });
  K.$("a-fold").addEventListener("click", () => {   // while filtering it shows open; a press closes it and clears
    open = !(open || box.value.trim());
    clear();
  });
  box.addEventListener("input", () => { tell(""); draw(); });
  box.addEventListener("keydown", (e) => {   // Esc clears the filter; the next leaves the box (app.js)
    if (e.key !== "Escape" || !box.value) return;
    e.preventDefault();
    e.stopPropagation();
    clear();
  });
  K.$("a-filter").addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = box.value.trim(), q = name.toLowerCase();
    if (!name || !view || making) return;
    const hits = matching(q), tag = hits.find((t) => t.toLowerCase() === q) || (hits.length === 1 ? hits[0] : null);
    if (tag) { clear(); view.pick(tag); return; }
    if (hits.length) return;   // several, none exact: keep typing
    making = true;
    const mine = target;
    try {
      const made = await K.newTag(name);
      if (mine === target) { clear(); view.pick(made); }
    } catch (err) { tell(err.message); }
    finally { making = false; }
  });
})();
