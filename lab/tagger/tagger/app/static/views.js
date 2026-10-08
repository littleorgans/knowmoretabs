/* knowmoretabs · tagger app · the views
   Search, then click pages to select them (space or x on the focused one).
   Open (o) shows a page in a new tab and selects nothing; a chip's × takes
   that tag off that page only. Three views share the grid: the search's
   results, ≈ (pages like a tag's without that tag) and Show selection. ≈ and Show
   selection sit over the search: pressing them again, Esc (≈) or Back
   returns to the search as it was left, page and scroll included. Esc
   with no ≈ open clears the selection. Search and ≈ come a page at a
   time: Previous and Next replace the hits and show them from the top.
   Tagging never changes which pages a view shows, nor which tags the
   strip shows; a new search, a filter or another page does. lit-html
   renders the grid keyed by row, so a tile's node (its focus included)
   stays while its page is in view. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const { html, nothing, render, repeat, unsafeHTML } = K.lit;
  const asked = { search: 0, like: 0 };   // the latest request of each kind, whose reply wins
  /* a view: its pages, the filter and the pages it leaves, and the tags its strip has shown (kept, so a tag
     taken off every page stays there to put back) */
  const blank = (kind, extra) => ({ kind, rows: [], filter: null, shown: null, tags: new Set(), offset: 0, total: 0, loading: false, scroll: 0, ...extra });
  let base = blank("search");   // the search's results
  let over = null;              // the view over them: ≈ ("like") or "selection"
  const view = () => over || base;
  const per = () => (K.lib ? K.lib.sizes[1] : 50);   // hits on a page: the most the server sends
  history.scrollRestoration = "manual";

  /* ---- views over the search: one history entry, so Back leaves them ---- */
  function enter(v) {
    if (over) history.replaceState({ over: v.kind }, "");
    else { base.scroll = scrollY; history.pushState({ over: v.kind }, ""); }
    over = v;
    scrollTo(0, 0);
    K.changed();
  }
  const leave = () => { if (over) history.back(); };
  addEventListener("popstate", () => {
    if (!over) return;
    over = null;
    K.changed();
    scrollTo(0, base.scroll);
  });
  function drop() {   // a new search replaces whatever view was over the old one
    if (!over) return;
    over = null;
    history.replaceState(null, "");
  }

  /* one page of a search's or ≈'s hits, from rank `offset`; true when they replaced the view's */
  async function fetchPage(v, offset) {
    const mine = ++asked[v.kind];
    let done = false;
    v.loading = true;
    K.changed();
    const at = { offset, n: per(), images: S.images };
    try {
      const out = await (v.kind === "like" ? K.api("/api/like", { tag: v.tag, ...at })
                                           : K.api("/api/search", { query: S.query, untagged: S.untagged, ...at }));
      if (mine !== asked[v.kind]) return false;
      Object.assign(v, { rows: K.know(out.hits), offset: out.offset, total: out.total, filter: null, shown: null, tags: new Set() });
      if (v.kind === "search") { S.offset = v.offset; K.save(); }
      done = true;
    } catch (err) { if (mine !== asked[v.kind]) return false; K.fail(err); }
    v.loading = false;
    K.changed();
    return done;
  }
  function run(offset = 0) {
    const q = K.$("s-q").value;
    if (!q.trim()) { K.$("s-q").focus(); return Promise.resolve(false); }
    Object.assign(S, { query: q, images: K.$("s-img").checked, untagged: K.$("s-untag").checked });
    K.save();
    drop();
    base = blank("search");
    return fetchPage(base, offset);
  }
  /* Previous (-1) or Next (+1): the neighbouring page replaces this one, shown from the top */
  async function go(step) {
    const v = view();
    if (v.kind === "selection" || v.loading) return;
    const offset = v.offset + step * per();
    if (offset < 0 || offset >= v.total) return;
    if (await fetchPage(v, offset) && v === view()) scrollTo(0, 0);
  }
  function like(tag) {
    if (over && over.kind === "like" && over.tag === tag) return leave();
    const v = blank("like", { tag });
    enter(v);
    return fetchPage(v, 0);
  }
  function showSelection() {
    if (over && over.kind === "selection") return leave();
    enter(blank("selection", { rows: K.sel.slice() }));
  }
  function clear() {
    K.clearSel();
    if (over && over.kind === "selection") leave();
  }

  /* the strip's count: only pages with the tag, then only pages without, then all */
  function filter(tag) {
    const v = view(), f = v.filter;
    v.filter = !f || f.tag !== tag ? { tag, has: true } : f.has ? { tag, has: false } : null;
    v.shown = v.filter ? v.rows.filter((r) => K.has(r, tag) === v.filter.has) : null;
    K.changed();
  }

  /* ---- the grid: lit-html templates (text is escaped by lit; the picture and Open are core's escaped markup) ---- */
  const chip = (t) => html`<li class="tag">${t}<button type="button" class="x" data-untag="${t}" title="Take ${t} off this page" aria-label="Take ${t} off this page">×</button></li>`;
  function tile(p) {
    const on = K.sel.includes(p.row);
    return html`<li class="hit tile${on ? " sel" : ""}" data-row="${p.row}" tabindex="0">${unsafeHTML(K.thumb(p, "th", K.open(p)))}<div class="hb">
      <h3 class="ht">${p.title || html`<i>Untitled page</i>`}</h3><p class="host">${p.host}</p>
      <ul class="tags">${p.tags.length ? p.tags.map(chip) : html`<li class="none">Untagged</li>`}</ul></div>${on ? html`<span class="sr">Selected</span>` : nothing}</li>`;
  }
  const plural = (n, word) => `<b>${n}</b> ${word}${n === 1 ? "" : "s"}`;
  const range = (v) => `<b>${v.offset + 1} to ${v.offset + v.rows.length}</b> of ${v.total}`;
  function position(v) {
    const n = v.rows.length;
    if (v.kind === "like") return v.loading && !n ? `Finding pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b>…`
      : `Pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b> · ${n ? range(v) : "<b>0</b>"}`;
    if (v.kind === "selection") return `Your selection · ${plural(n, "page")}`;
    if (v.loading) return "Searching…";
    if (S.query) return `${n ? range(v) : plural(0, "result")} for “${K.esc(K.short(S.query))}”${S.untagged ? " · untagged only" : ""}`;
    return K.lib ? `${K.lib.pages} pages in your library` : "";
  }
  function empty(v) {
    if (v.loading) return "";
    if (v.filter) return "No page in view matches this filter.";
    if (v.kind === "like") return `No pages like ${v.tag} without that tag are left.`;
    if (v.kind === "selection") return "Nothing selected.";
    return S.query ? `Nothing found${S.untagged ? " among untagged pages" : ""}.` : "";
  }

  K.views = {
    view,
    run,
    like,
    filter,
    showSelection,
    clear,
    render() {
      const v = view(), rows = v.shown || v.rows;
      if (document.activeElement !== K.$("s-q")) K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-img").disabled = !!K.lib && !K.lib.images;
      K.$("s-untag").checked = !!S.untagged;
      render(rows.length ? repeat(rows, (r) => r, (r) => tile(K.pages.get(r))) : html`<li class="none">${empty(v)}</li>`, K.$("grid"));
      K.$("v-pos").innerHTML = position(v);
      K.$("v-filter").innerHTML = v.filter ? `Only pages ${v.filter.has ? "tagged" : "without"} <b>${K.esc(v.filter.tag)}</b>` +
        `<button type="button" class="lnk" data-act="unfilter">Show all</button>` : "";
      K.$("pager").hidden = v.kind === "selection" || v.total <= per();
      K.$("prev").disabled = v.loading || v.offset === 0;
      K.$("next").disabled = v.loading || v.offset + per() >= v.total;
    },
    go,
    restore() {   // the form is empty after a reload: fill it from the saved search
      if (!S.query) return Promise.resolve();
      K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-untag").checked = !!S.untagged;
      return run(S.offset || 0);
    },
    /* Esc leaves ≈ first; with no ≈ open it clears the selection (and leaves Show selection) */
    escape() {
      if (over && over.kind === "like") { leave(); return true; }
      const had = K.sel.length > 0 || !!over;
      K.clearSel();
      leave();
      return had;
    },
    key(e) {
      if (e.key === "Escape") return K.views.escape();
      const tile = e.target.closest && e.target.closest(".tile[data-row]");
      if (!tile || e.target.closest("a, button")) return false;
      if (e.key === " " || e.key === "x") { K.select(Number(tile.dataset.row)); return true; }
      if (e.key === "o") { K.openIn(tile); return true; }
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
  K.$("v-filter").addEventListener("click", (e) => {
    if (!e.target.closest("[data-act=unfilter]")) return;
    const v = view();
    Object.assign(v, { filter: null, shown: null });
    K.changed();
  });
  K.$("grid").addEventListener("click", (e) => {
    if (K.opens(e)) return;
    const tile = e.target.closest(".tile[data-row]");
    if (!tile) return;
    const x = e.target.closest("[data-untag]");
    if (x) K.untag(Number(tile.dataset.row), x.dataset.untag);
    else K.select(Number(tile.dataset.row));
  });
})();
