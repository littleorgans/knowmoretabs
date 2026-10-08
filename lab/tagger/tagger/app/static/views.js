/* knowmoretabs · tagger app · the views
   Search, then click pages to select them (space or x on the focused one).
   Over a tile's picture: Forget (f), Open (o) and Pin (p); none of them
   selects. A chip's × takes that tag off that page only. Four views share
   the grid: the search's results, ≈ (pages like a tag's without that tag),
   Show selection and Pinned. The last three sit over the search: pressing
   them again, Esc (≈, Pinned) or Back returns to the search as it was left,
   page and scroll included. Esc with neither open clears the selection.
   Search and ≈ come a page at a time: Previous and Next replace the hits
   and show them from the top. A forgotten page leaves every view at once
   (Undo puts it back in its place). Tagging, pinning and unpinning never
   change which pages a view shows, nor which tags the strip shows; a new
   search or another page does. lit-html renders the grid keyed by row, so
   a tile's node (its focus included) stays while its page is in view. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const { html, nothing, render, repeat, unsafeHTML } = K.lit;
  const asked = { search: 0, like: 0 };   // the latest request of each kind, whose reply wins
  /* a view: its pages, and the tags its strip has shown (kept, so a tag taken off every page stays there to put back) */
  const blank = (kind, extra) => ({ kind, rows: [], tags: new Set(), offset: 0, total: 0, loading: false, scroll: 0, ...extra });
  let base = blank("search");   // the search's results
  let over = null;              // the view over them: ≈ ("like"), "selection" or "pinned"
  let searched = false;         // a submitted search takes precedence over startup restoration
  const view = () => over || base;
  const per = () => (K.lib ? K.lib.sizes[1] : 50);   // hits on a page: the most the server sends
  const listed = (v) => v.kind === "selection" || v.kind === "pinned";   // a list the owner keeps, in one page
  /* what a view shows now: its pages less those forgotten since it came, and its count less them too */
  const shown = (v) => v.rows.filter((r) => !K.gone.has(r));
  const total = (v, rows) => v.total - (v.rows.length - rows.length);
  history.scrollRestoration = "manual";

  /* Each search page keeps its view and scroll in this tab's history. Page data lives in K.pages. */
  function remember(push = false) {
    const v = view();
    v.scroll = scrollY;
    if (!over) { S.offset = base.offset; S.scroll = base.scroll; }
    K.save();
    history[push ? "pushState" : "replaceState"]({ base, over, search: { ...S } }, "");
  }
  let settling = 0;   // once scrolling settles: browsers drop history calls past a rate (Chrome 200 in 10 s)
  addEventListener("scroll", () => {
    clearTimeout(settling);
    settling = setTimeout(() => { if (!view().loading) remember(); }, 150);
  });

  /* ---- views over the search: one entry, so again, Esc and Back all leave the entire view ---- */
  function enter(v) {
    const replacing = !!over;
    remember();
    over = v;
    scrollTo(0, 0);
    K.changed();
    remember(!replacing);
  }
  const leave = () => {
    if (!over) return;
    remember();
    history.back();
  };
  addEventListener("popstate", async (e) => {
    if (!e.state || !e.state.base) return;
    asked.search++; asked.like++;  // replies for the view being left cannot update the restored page
    ({ base, over } = e.state);
    Object.assign(S, e.state.search);
    K.save();
    const v = view(), scroll = v.scroll;
    if (v.rows.some((r) => !K.pages.has(r) && !K.gone.has(r))) await fetchPage(v, v.offset);
    K.changed();
    scrollTo(0, scroll);
  });
  /* one page of a search's or ≈'s hits, from rank `offset`; true when they replaced the view's */
  async function fetchPage(v, offset) {
    const mine = ++asked[v.kind];
    let done = false;
    v.loading = true;
    K.changed();
    const at = { offset, n: per(), images: S.images };
    try {
      await K.writes;  // rankings and tags must include decisions queued before this page was requested
      if (mine !== asked[v.kind]) return false;
      const out = await (v.kind === "like" ? K.api("/api/like", { tag: v.tag, ...at })
                                           : K.api("/api/search", { query: S.query, untagged: S.untagged, ...at }));
      if (mine !== asked[v.kind]) return false;
      Object.assign(v, { rows: K.know(out.hits), offset: out.offset, total: out.total, tags: new Set() });
      if (v.kind === "search") { S.offset = v.offset; K.save(); }
      done = true;
    } catch (err) { if (mine !== asked[v.kind]) return false; K.fail(err); }
    v.loading = false;
    K.changed();
    return done;
  }
  async function run(offset = 0, scroll = 0) {
    const q = K.$("s-q").value;
    if (!q.trim()) { K.$("s-q").focus(); return Promise.resolve(false); }
    searched = true;
    Object.assign(S, { query: q, images: K.$("s-img").checked, untagged: K.$("s-untag").checked, offset, scroll });
    K.save();
    over = null;  // a new search replaces the view over the old one
    base = blank("search");
    const v = base;
    if (!await fetchPage(v, offset) || v !== view()) return false;
    scrollTo(0, scroll);
    remember();
    return true;
  }
  /* Previous (-1) or Next (+1): the neighbouring page replaces this one, shown from the top. Next starts after the
     last hit shown, so a page forgotten here moves none of the rest past it */
  async function go(step) {
    const v = view();
    if (listed(v) || v.loading) return;
    const rows = shown(v);
    const offset = step > 0 ? v.offset + rows.length : Math.max(v.offset - per(), 0);
    if (offset === v.offset || offset >= total(v, rows)) return;
    remember();
    if (await fetchPage(v, offset) && v === view()) {
      scrollTo(0, 0);
      K.$("grid").querySelector(".tile")?.focus({ preventScroll: true });
      remember(!over);
    }
  }
  function like(tag) {
    if (over && over.kind === "like" && over.tag === tag) return leave();
    const v = blank("like", { tag });
    enter(v);
    return fetchPage(v, 0).then((done) => { if (done && v === view()) remember(); return done; });
  }
  /* Show selection and Pinned: the list as it is now, kept while the view is open (one taken off stays to put back) */
  function showList(kind, rows) {
    if (over && over.kind === kind) return leave();
    enter(blank(kind, { rows: rows.slice() }));
  }
  function clear() {
    K.clearSel();
    if (over && over.kind === "selection") leave();
  }

  /* ---- the grid: lit-html templates (text is escaped by lit; the picture and Open are core's escaped markup) ---- */
  const chip = (t) => html`<li class="tag">${t}<button type="button" class="x" data-untag="${t}" title="Take ${t} off this page" aria-label="Take ${t} off this page">×</button></li>`;
  const icon = (d) => html`<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="square" stroke-linejoin="miter"><path d="${d}"/></svg>`;
  const BIN = icon("M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v5M14 11v5");
  const PIN = icon("M9 3h6l-1 6 4 4H6l4-4-1-6zM12 13v8");
  function tile(p) {
    const on = K.sel.includes(p.row), pinned = K.pins.includes(p.row);
    const pin = pinned ? "Unpin this page (p)" : "Pin this page for later (p)";
    return html`<li class="hit tile${on ? " sel" : ""}${pinned ? " pinned" : ""}" data-row="${p.row}" tabindex="0">${unsafeHTML(K.thumb(p, "th"))}<div class="ctl">
      <button type="button" class="fg" data-forget title="Forget this page (f)" aria-label="Forget this page">${BIN}</button>${unsafeHTML(K.open(p))}<button
        type="button" class="pn" data-pin aria-pressed="${pinned}" title="${pin}" aria-label="${pin}">${PIN}</button></div><div class="hb">
      <h3 class="ht">${p.title || html`<i>Untitled page</i>`}</h3><p class="host">${p.host}</p>
      <ul class="tags">${p.tags.length ? p.tags.map(chip) : html`<li class="none">Untagged</li>`}</ul></div>${pinned ? html`<span class="pinmark" aria-hidden="true">${PIN}</span>` : nothing}${
      on || pinned ? html`<span class="sr">${[on && "Selected", pinned && "Pinned"].filter(Boolean).join(", ")}</span>` : nothing}</li>`;
  }
  const plural = (n, word) => `<b>${n}</b> ${word}${n === 1 ? "" : "s"}`;
  const range = (v, rows) => `<b>${v.offset + 1} to ${v.offset + rows.length}</b> of ${total(v, rows)}`;
  function position(v, rows) {
    const n = rows.length;
    if (v.kind === "like") return v.loading && !n ? `Finding pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b>…`
      : `Pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b> · ${n ? range(v, rows) : "<b>0</b>"}`;
    if (v.kind === "selection") return `Your selection · ${plural(n, "page")}`;
    if (v.kind === "pinned") return `Pinned · ${plural(n, "page")}`;
    if (v.loading) return "Searching…";
    if (S.query) return `${n ? range(v, rows) : plural(0, "result")} for “${K.esc(K.short(S.query))}”${S.untagged ? " · untagged only" : ""}`;
    return K.lib ? `${K.lib.pages} pages in your library` : "";
  }
  function empty(v) {
    if (v.loading) return "";
    if (v.kind === "like") return `No pages like ${v.tag} without that tag are left.`;
    if (v.kind === "selection") return "Nothing selected.";
    if (v.kind === "pinned") return "Nothing pinned.";
    return S.query ? `Nothing found${S.untagged ? " among untagged pages" : ""}.` : "";
  }

  K.views = {
    view,
    shown,
    run,
    like,
    showSelection: () => showList("selection", K.sel),
    showPinned: () => showList("pinned", K.pins),
    clear,
    render() {
      const v = view(), rows = shown(v);
      if (document.activeElement !== K.$("s-q")) K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-img").disabled = !!K.lib && !K.lib.images;
      K.$("s-untag").checked = !!S.untagged;
      render(rows.length ? repeat(rows, (r) => r, (r) => tile(K.pages.get(r))) : html`<li class="none">${empty(v)}</li>`, K.$("grid"));
      K.$("v-pos").innerHTML = position(v, rows);
      const end = v.offset + rows.length >= total(v, rows);
      K.$("pager").hidden = listed(v) || (v.offset === 0 && end);
      K.$("prev").disabled = v.loading || v.offset === 0;
      K.$("next").disabled = v.loading || end;
    },
    go,
    async restored(row) {
      const v = view();
      if (listed(v) || (!v.loading && v.rows.includes(row)) || !S.query) return;
      const scroll = scrollY;
      if (await fetchPage(v, v.offset) && v === view()) { scrollTo(0, scroll); remember(); }
    },
    restore() {   // the form is empty after a reload: fill it from the saved search
      if (!S.query || searched) return Promise.resolve();
      K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-untag").checked = !!S.untagged;
      return run(S.offset || 0, S.scroll || 0);
    },
    /* Esc leaves ≈ or Pinned first; with neither open it clears the selection (and leaves Show selection) */
    escape() {
      if (over && over.kind !== "selection") { leave(); return true; }
      const had = K.sel.length > 0 || !!over;
      K.clearSel();
      leave();
      return had;
    },
    key(e) {
      if (e.key === "Escape") return K.views.escape();
      const tile = e.target.closest && e.target.closest(".tile[data-row]");
      if (!tile || e.target.closest("a, button")) return false;
      const row = Number(tile.dataset.row);
      if (e.key === " " || e.key === "x") { K.select(row); return true; }
      if (e.key === "o") { K.openIn(tile); return true; }
      if (e.key === "p") { K.pin(row); return true; }
      if (e.key === "f") {   // the next tile takes the focus, so f again forgets on down the grid
        const next = tile.nextElementSibling || tile.previousElementSibling;
        K.forget(row);
        if (next && next.focus) next.focus();
        return true;
      }
      return false;
    }
  };

  K.$("s-form").addEventListener("submit", (e) => { e.preventDefault(); run(); K.$("s-q").blur(); });
  K.$("s-untag").addEventListener("change", () => { if (K.$("s-q").value.trim()) run(); });
  K.$("s-img").addEventListener("change", () => {
    if (over && over.kind === "like") { S.images = K.$("s-img").checked; K.save(); fetchPage(over, 0); }
    else if (K.$("s-q").value.trim()) run();
  });
  K.$("prev").addEventListener("click", () => go(-1));
  K.$("next").addEventListener("click", () => go(1));
  K.$("grid").addEventListener("click", (e) => {
    if (K.opens(e)) return;
    const tile = e.target.closest(".tile[data-row]");
    if (!tile) return;
    const row = Number(tile.dataset.row), x = e.target.closest("[data-untag]");
    if (x) K.untag(row, x.dataset.untag);
    else if (e.target.closest("[data-forget]")) K.forget(row);
    else if (e.target.closest("[data-pin]")) K.pin(row);
    else K.select(row);
  });
})();
