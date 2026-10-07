/* knowmoretabs · tagger app · step 1, search
   Type anything; EG2 ranks the library, fused with a keyword score (and the
   image cosine when asked). Each result shows where its rank came from, for
   debugging: every source's rank and score, a dash where it did not rank. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const SOURCES = { text: "text", keyword: "keyword", image: "image" };

  function source(h) {
    return Object.entries(h.sources).map(([k, v]) =>
      `<span title="${SOURCES[k]} score ${v.score.toFixed(3)}">${SOURCES[k]} ${v.rank ? `#${v.rank} · ${v.score.toFixed(2)}` : "–"}</span>`).join("");
  }
  function hit(h, i) {
    return `<li class="hit">${K.thumb(h, "th")}<div class="hb">` +
      `<p class="kick">#${i + 1} · fused ${h.fused.toFixed(4)}</p>` +
      `<h3 class="ht">${h.title ? K.esc(h.title) : "<i>Untitled page</i>"}</h3>` +
      `<p class="host">${K.link(h)}</p>` +
      `<ul class="tags">${K.own(h)}</ul><p class="src">${source(h)}</p></div></li>`;
  }

  async function run(n) {
    const q = K.$("s-q").value.trim();
    if (!q) { K.$("s-q").focus(); return; }
    const changed = q !== S.query || K.$("s-img").checked !== S.images;
    Object.assign(S, { query: q, images: K.$("s-img").checked, n: n || 20 });
    if (changed) S.picked = [];
    K.save();
    K.$("s-pos").textContent = "Searching…";
    try {
      K.results = await K.api("/api/search", { query: S.query, images: S.images, n: S.n });
    } catch (err) { K.results = null; K.fail(err); }
    K.changed();
  }

  K.search = {
    render() {
      const r = K.results;
      if (document.activeElement !== K.$("s-q")) K.$("s-q").value = S.query;
      K.$("s-img").checked = S.images;
      K.$("s-img").disabled = K.lib && !K.lib.images;
      K.$("s-grid").innerHTML = r ? r.hits.map(hit).join("") : "";
      K.$("s-pos").innerHTML = r ? `<b>${r.hits.length}</b> results` : S.query ? "" : `${K.lib ? K.lib.pages : ""} pages in your library`;
      K.$("s-ms").textContent = r ? `query ${r.ms.encode} ms · rank ${r.ms.rank} ms` : "";
      const max = K.lib ? K.lib.sizes[1] : 50;
      K.$("s-more").hidden = !r || S.n >= max || r.hits.length < S.n;
      K.$("s-more").textContent = `Show more (${max})`;
      K.$("s-tag").disabled = !r || !r.hits.length;
    },
    run,
    more: () => run(K.lib.sizes[1]),
    restore: () => (S.query ? run(S.n) : Promise.resolve()),
    key(e) {
      if (e.key === "Enter" && K.results && !e.target.closest("button, a")) { location.hash = "tag"; return true; }
      return false;
    }
  };

  K.$("s-form").addEventListener("submit", (e) => { e.preventDefault(); run(); K.$("s-q").blur(); });
  K.$("s-img").addEventListener("change", () => { if (K.$("s-q").value.trim()) run(S.n); });
  K.$("view-search").addEventListener("click", (e) => {
    const a = e.target.closest("[data-act]");
    if (!a || a.disabled) return;
    if (a.dataset.act === "more") K.search.more();
    if (a.dataset.act === "tag") location.hash = "tag";
  });
})();
