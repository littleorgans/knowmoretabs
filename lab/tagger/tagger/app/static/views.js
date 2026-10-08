/* knowmoretabs · tagger app · the views
   Search, then click pages to select them (space or x on the focused one).
   Open (o) shows a page in a new tab and selects nothing; a chip's × takes
   that tag off that page only. Three views share the grid: the search's
   results, ≈ (pages like a tag's without that tag) and Show selection. ≈ and Show
   selection sit over the search: pressing them again, Esc (≈) or Back
   returns to the search as it was left, scroll included. Esc with no ≈
   open clears the selection. Tagging never changes which pages a view
   shows, nor which tags the strip shows; a new search, a filter or Show
   more does. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const asked = { search: 0, like: 0 };   // the latest request of each kind, whose reply wins
  /* a view: its pages, the filter and the pages it leaves, and the tags its strip has shown (kept, so a tag
     taken off every page stays there to put back) */
  const blank = (kind, extra) => ({ kind, rows: [], filter: null, shown: null, tags: new Set(), n: 20, loading: false, scroll: 0, ...extra });
  let base = blank("search");   // the search's results
  let over = null;              // the view over them: ≈ ("like") or "selection"
  const view = () => over || base;
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

  async function run(n) {
    const q = K.$("s-q").value;
    if (!q.trim()) { K.$("s-q").focus(); return; }
    Object.assign(S, { query: q, images: K.$("s-img").checked, untagged: K.$("s-untag").checked, n: n || 20 });
    K.save();
    drop();
    const mine = ++asked.search;
    const v = (base = blank("search", { n: S.n, loading: true }));
    K.changed();
    try {
      const out = await K.api("/api/search", { query: S.query, images: S.images, untagged: S.untagged, n: S.n });
      if (mine !== asked.search) return;
      v.rows = K.know(out.hits);
    } catch (err) { if (mine !== asked.search) return; K.fail(err); }
    v.loading = false;
    K.changed();
  }

  async function fetchLike(v) {
    const mine = ++asked.like;
    Object.assign(v, { loading: true, filter: null, shown: null });
    K.changed();
    try {
      const out = await K.api("/api/like", { tag: v.tag, n: v.n, images: S.images });
      if (mine !== asked.like) return;
      v.rows = K.know(out.hits);
    } catch (err) { if (mine !== asked.like) return; K.fail(err); }
    v.loading = false;
    K.changed();
  }
  function like(tag) {
    if (over && over.kind === "like" && over.tag === tag) return leave();
    const v = blank("like", { tag });
    enter(v);
    return fetchLike(v);
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

  /* ---- the grid ---- */
  function tile(p) {
    const on = K.sel.includes(p.row);
    const tags = p.tags.length ? p.tags.map((t) => `<li class="tag">${K.esc(t)}<button type="button" class="x" data-untag="${K.esc(t)}" ` +
      `title="Take ${K.esc(t)} off this page" aria-label="Take ${K.esc(t)} off this page">×</button></li>`).join("") : `<li class="none">Untagged</li>`;
    return `<li class="hit tile${on ? " sel" : ""}" data-row="${p.row}" tabindex="0">${K.thumb(p, "th", K.open(p))}<div class="hb">` +
      `<h3 class="ht">${p.title ? K.esc(p.title) : "<i>Untitled page</i>"}</h3><p class="host">${K.esc(p.host)}</p>` +
      `<ul class="tags">${tags}</ul></div>${on ? `<span class="sr">Selected</span>` : ""}</li>`;
  }
  const plural = (n, word) => `<b>${n}</b> ${word}${n === 1 ? "" : "s"}`;
  function position(v) {
    const n = v.rows.length;
    if (v.kind === "like") return v.loading && !n ? `Finding pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b>…`
      : `Pages like <b>${K.esc(v.tag)}</b>, not tagged <b>${K.esc(v.tag)}</b> · ${n}<span class="wide"> · ≈, Esc or Back returns to your search</span>`;
    if (v.kind === "selection") return `Your selection · ${plural(n, "page")}<span class="wide"> · Show selection or Back returns to your search</span>`;
    if (v.loading) return "Searching…";
    if (S.query) return `${plural(n, "result")} for “${K.esc(K.short(S.query))}”${S.untagged ? " · untagged only" : ""}`;
    return K.lib ? `${K.lib.pages} pages in your library` : "";
  }
  function empty(v) {
    if (v.loading) return "";
    if (v.filter) return "No page in view matches this filter.";
    if (v.kind === "like") return `No pages like ${K.esc(v.tag)} without that tag are left.`;
    if (v.kind === "selection") return "Nothing selected.";
    return S.query ? `Nothing found${S.untagged ? " among untagged pages" : ""}.` : "Search your library, then click pages to select them.";
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
      const focused = document.activeElement && document.activeElement.closest ? document.activeElement.closest(".tile[data-row]") : null;
      K.$("grid").innerHTML = rows.length ? rows.map((r) => tile(K.pages.get(r))).join("") : `<li class="none">${empty(v)}</li>`;
      if (focused) { const again = K.$("grid").querySelector(`.tile[data-row="${focused.dataset.row}"]`); if (again) again.focus(); }
      K.$("v-pos").innerHTML = position(v);
      K.$("v-filter").innerHTML = v.filter ? `Only pages ${v.filter.has ? "tagged" : "without"} <b>${K.esc(v.filter.tag)}</b>` +
        `<button type="button" class="lnk" data-act="unfilter">Show all</button>` : "";
      const max = K.lib ? K.lib.sizes[1] : 50;
      K.$("more").hidden = v.kind === "selection" || v.loading || v.rows.length < v.n || v.n >= max;
      K.$("more").textContent = `Show more (${max})`;
    },
    more() {
      const max = K.lib.sizes[1];
      if (over && over.kind === "like") { over.n = max; return fetchLike(over); }
      return run(max);
    },
    restore() {   // the form is empty after a reload: fill it from the saved search
      if (!S.query) return Promise.resolve();
      K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-untag").checked = !!S.untagged;
      return run(S.n);
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
  K.$("s-untag").addEventListener("change", () => { if (K.$("s-q").value.trim()) run(S.n); });
  K.$("s-img").addEventListener("change", () => {
    if (over && over.kind === "like") { S.images = K.$("s-img").checked; K.save(); fetchLike(over); }
    else if (K.$("s-q").value.trim()) run(S.n);
  });
  K.$("more").addEventListener("click", () => K.views.more());
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
