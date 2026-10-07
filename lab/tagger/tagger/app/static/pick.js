/* knowmoretabs · tagger app · step 2, pick tags
   The model's suggestions for this result set come first (tags ranked by
   their mean z score over the results kept), then every tag you have. Pick a
   few; the review offers only those, on kept pages that do not hold them yet. */
(function () {
  const K = window.KMT;
  const S = K.state.search;
  const info = (t) => K.lib.tags.find((x) => x.name === t);

  function chip(t, extra) {
    const on = S.picked.includes(t), x = info(t);
    const meta = x.source === "head" ? `${x.positives} labels` : x.positives ? `${x.positives} labels · zero shot` : "zero shot";
    return `<li><button type="button" class="chip" data-tag="${K.esc(t)}" aria-pressed="${on}"><b>${K.esc(t)}</b><small>${extra || meta}</small></button></li>`;
  }

  function toggle(t) {
    const i = S.picked.indexOf(t);
    if (i >= 0) S.picked.splice(i, 1); else S.picked.push(t);
    K.save();
    K.pick.render();
  }

  async function review() {
    if (!K.results || !S.picked.length) return;
    const b = K.$("t-go");
    b.disabled = true;
    try {
      const set = await K.api("/api/sessions", { query: S.query, images: S.images, rows: K.search.included().map((h) => h.row), picked: S.picked });
      K.set = set;
      K.state.session = set.id;
      K.save();
      K.refreshSets();
      location.hash = "review";
    } catch (err) { K.fail(err); b.disabled = false; }
  }

  K.pick = {
    render() {
      const r = K.results, kept = K.search.included().length;
      if (!r || !K.lib) {
        K.$("t-pos").textContent = "Search first";
        K.$("t-sugg").innerHTML = `<li class="none">Search for something, then pick tags for what it finds.</li>`;
        K.$("t-all").innerHTML = "";
        K.$("t-go").disabled = true;
        return;
      }
      K.$("t-pos").innerHTML = `<b>${kept}</b> results for “${K.esc(K.short(S.query))}”${kept < r.hits.length ? ` (${r.hits.length - kept} excluded)` : ""}`;
      K.$("t-picked").textContent = S.picked.length ? `${S.picked.length} picked` : "none picked";
      K.$("t-sugg").innerHTML = r.suggested.map((s) => chip(s.tag, `z ${s.z.toFixed(2)}`)).join("");
      const f = K.$("t-filter").value.trim().toLowerCase();
      const all = K.lib.tags.filter((t) => !f || t.name.toLowerCase().includes(f));
      K.$("t-all").innerHTML = all.length ? all.map((t) => chip(t.name)).join("") : `<li class="none">No tag matches.</li>`;
      const go = K.$("t-go");
      go.disabled = !S.picked.length || !kept;
      go.innerHTML = S.picked.length ? `<span>Review ${kept} pages with ${S.picked.length} tag${S.picked.length === 1 ? "" : "s"}</span><kbd>↵</kbd>`
                                      : "<span>Pick at least one tag</span>";
    },
    key(e) {
      if (e.key === "Enter" && !e.target.closest("button")) { review(); return true; }
      if (e.key === "ArrowLeft") { location.hash = "search"; return true; }
      return false;
    }
  };

  K.$("view-tag").addEventListener("click", (e) => {
    const c = e.target.closest(".chip");
    if (c) { toggle(c.dataset.tag); return; }
    const a = e.target.closest("[data-act]");
    if (!a || a.disabled) return;
    if (a.dataset.act === "back") location.hash = "search";
    if (a.dataset.act === "review") review();
  });
  K.$("t-filter").addEventListener("input", () => K.pick.render());
})();
