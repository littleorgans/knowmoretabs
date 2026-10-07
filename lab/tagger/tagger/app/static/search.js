/* knowmoretabs · tagger app · step 1, search
   Type anything; EG2 ranks the library, fused with a keyword score (and the
   image cosine when asked). Each result shows where its rank came from, for
   debugging: every source's rank and score, a dash where it did not rank.
   A click or tap on a result (x on the focused one) marks it not relevant,
   so it stays out of tagging; again brings it back. Cut here (c) excludes
   every result below one, as ranked lists go bad past a point. Refine ranks
   again toward the results kept and away from those excluded. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const SOURCES = { text: "text", keyword: "keyword", image: "image" };
  let request = 0;
  let searching = false;
  let judging = Promise.resolve();   // exclusion requests, sent one at a time in order
  let judgment = 0;                  // the latest one, whose reply wins

  function source(h) {
    return Object.entries(h.sources).map(([k, v]) =>
      `<span title="${SOURCES[k]} score ${v.score.toFixed(3)}">${SOURCES[k]} ${v.rank ? `#${v.rank} · ${v.score.toFixed(2)}` : "–"}</span>`).join("");
  }
  const out = (h) => K.results.excluded.includes(h.row);
  const included = () => (K.results ? K.results.hits.filter((h) => !out(h)) : []);

  function hit(h, i) {
    const x = out(h), cut = K.results.cut === h.row;
    return `<li class="hit${x ? " out" : ""}${cut ? " cut" : ""}" data-row="${h.row}" tabindex="0" ` +
      `title="${x ? "Not relevant: click (x) to bring it back" : "Click (x) if this is not relevant"}">${K.thumb(h, "th")}<div class="hb">` +
      `<p class="kick">#${i + 1} · fused ${h.fused.toFixed(4)}${x ? ` · <b class="mark">Not relevant</b>` : ""}</p>` +
      `<h3 class="ht">${h.title ? K.esc(h.title) : "<i>Untitled page</i>"}</h3>` +
      `<p class="host">${K.link(h)}</p>` +
      `<ul class="tags">${K.own(h)}</ul><p class="src">${source(h)}</p>` +
      `<p class="hit-btns">${cut ? `<b class="mark">Everything below is cut</b><button type="button" data-act="uncut">Undo cut</button>`
                                : `<button type="button" data-act="cut" title="Exclude every result below this one (c)">Cut here</button>`}</p></div></li>`;
  }

  /* one exclusion action; the tile changes at once and the server's reply settles it */
  function judge(action, row) {
    const r = K.results;
    if (action === "exclude") r.excluded = r.excluded.concat(row);
    if (action === "include") r.excluded = r.excluded.filter((x) => x !== row);
    const mine = ++judgment;
    K.changed();
    const body = { query: S.query, rows: r.hits.map((h) => h.row), action, row };
    const next = judging.then(() => K.api("/api/exclusions", body));
    judging = next.catch(() => {});
    return next.then((reply) => {
      if (K.results === r && mine === judgment) { Object.assign(r, reply); K.changed(); }
      return reply;
    });
  }
  const toggle = (row) => judge(K.results.excluded.includes(row) ? "include" : "exclude", row).catch(K.fail);
  function cut(row) {
    if (K.results.cut === row) return judge("uncut").then(() => K.toast("Cut undone")).catch(K.fail);
    const rank = K.results.hits.findIndex((h) => h.row === row) + 1, all = K.results.hits.length;
    return judge("cut", row).then((r) => {
      K.toast(`Cut below #${rank}: ${all - r.excluded.length} of ${all} going to tagging`, () => judge("uncut").catch(K.fail));
    }).catch(K.fail);
  }

  async function run(n) {
    const q = K.$("s-q").value;
    if (!q.trim()) { K.$("s-q").focus(); return; }
    const changed = q !== S.query || K.$("s-img").checked !== S.images;
    Object.assign(S, { query: q, images: K.$("s-img").checked, n: n || 20 });
    if (changed) Object.assign(S, { picked: [], refine: false });
    K.save();
    const current = ++request;
    searching = true;
    K.results = null;
    K.changed();
    try {
      await judging;
      if (current !== request) return;
      const results = await K.api("/api/search", { query: S.query, images: S.images, n: S.n, refine: !!S.refine });
      if (current !== request) return;
      K.results = results;
    } catch (err) { if (current !== request) return; K.fail(err); }
    searching = false;
    K.changed();
  }

  K.search = {
    render() {
      const r = K.results;
      if (document.activeElement !== K.$("s-q")) K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-img").disabled = K.lib && !K.lib.images;
      const focused = document.activeElement && document.activeElement.closest ? document.activeElement.closest(".hit[data-row]") : null;
      K.$("s-grid").innerHTML = r ? r.hits.map(hit).join("") : "";
      if (focused) { const again = K.$("s-grid").querySelector(`.hit[data-row="${focused.dataset.row}"]`); if (again) again.focus(); }
      const kept = included().length;
      K.$("s-pos").innerHTML = searching ? "Searching…" : r ? `<b>${kept}</b> of ${r.hits.length} going to tagging${S.refine ? " · refined" : ""}` : S.query ? "" : `${K.lib ? K.lib.pages : ""} pages in your library`;
      K.$("s-ms").textContent = r ? `query ${r.ms.encode} ms · rank ${r.ms.rank} ms` : "";
      const max = K.lib ? K.lib.sizes[1] : 50;
      K.$("s-more").hidden = !r || S.n >= max || r.hits.length < S.n;
      K.$("s-more").textContent = `Show more (${max})`;
      K.$("s-tag").disabled = !kept;
      K.$("s-refine").disabled = !r || !r.excluded.length;
    },
    run,
    get pending() { return judging; },
    included,
    more: () => run(K.lib.sizes[1]),
    restore() {   // the form is empty after a reload: fill it from the saved search, so run sees no change
      if (!S.query) return Promise.resolve();
      K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      return run(S.n);
    },
    key(e) {
      if (e.key === "Enter" && included().length && !e.target.closest("button, a")) { location.hash = "tag"; return true; }
      const tile = K.results && e.target.closest(".hit[data-row]");
      if (tile && e.key === "x") { toggle(Number(tile.dataset.row)); return true; }
      if (tile && e.key === "c") { cut(Number(tile.dataset.row)); return true; }
      return false;
    }
  };

  K.$("s-form").addEventListener("submit", (e) => { e.preventDefault(); run(); K.$("s-q").blur(); });
  K.$("s-img").addEventListener("change", () => { if (K.$("s-q").value.trim()) run(S.n); });
  K.$("view-search").addEventListener("click", (e) => {
    const a = e.target.closest("[data-act]");
    if (a && !a.disabled) {
      const tile = a.closest(".hit[data-row]");
      if (a.dataset.act === "more") K.search.more();
      if (a.dataset.act === "refine") { S.refine = true; K.save(); run(S.n); }
      if (a.dataset.act === "tag") location.hash = "tag";
      if (a.dataset.act === "cut" || a.dataset.act === "uncut") cut(Number(tile.dataset.row));
      return;
    }
    const tile = e.target.closest(".hit[data-row]");
    if (tile && K.results && !e.target.closest("a, button")) toggle(Number(tile.dataset.row));
  });
})();
