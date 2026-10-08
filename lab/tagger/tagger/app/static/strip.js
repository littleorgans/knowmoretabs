/* knowmoretabs · tagger app · the selection bar and the tag strip
   Above the grid and sticky: how many pages are selected (Show selection,
   Clear), then the app tags on the pages in view and the selection (and
   any shown before in this view, so one taken off can go back), each
   with three targets. The name tags every selected page, or takes the tag
   off them all when they all have it (partial when only some do). The
   count, pages in view with the tag of all in view, shows only those
   pages, then only the others, then all. ≈ shows untagged pages like the
   tag's, and pressed again goes back. "+ New tag" makes a tag, or finds
   yours in any case, and puts it on every selected page. */
(function () {
  const K = window.KMT;
  const byName = (a, b) => a.localeCompare(b, undefined, { sensitivity: "base" });
  let making = false;

  function chip(t, v) {
    const n = K.sel.length, s = K.sel.filter((r) => K.has(r, t)).length;
    const on = n && s === n ? "all" : s ? "some" : "none";
    const c = v.rows.filter((r) => K.has(r, t)).length;
    const f = v.filter && v.filter.tag === t ? (v.filter.has ? "has" : "not") : "";
    const like = v.kind === "like" && v.tag === t;
    const name = K.esc(t);
    const tip = !n ? "Select pages first, then click to tag them" : on === "all" ? `Take ${name} off the ${n} selected`
      : on === "some" ? `On ${s} of the ${n} selected: click to tag the rest` : `Tag the ${n} selected ${name}`;
    const count = f === "has" ? `Showing pages tagged ${name}; click for pages without it` : f === "not" ? `Showing pages without ${name}; click to show all`
      : `Show only pages tagged ${name}`;
    const find = like ? `Back to your search (Esc)` : `Find untagged pages like ${name}`;
    return `<li class="tc" data-on="${on}" data-tag="${name}">` +
      `<button type="button" class="nm" data-act="toggle" aria-pressed="${on === "all" ? "true" : on === "some" ? "mixed" : "false"}" title="${tip}">${name}</button>` +
      `<button type="button" class="ct" data-act="filter"${f ? ` data-f="${f}"` : ""} title="${count}" aria-label="${c} of ${v.rows.length} in view. ${count}">${c}/${v.rows.length}</button>` +
      `<button type="button" class="ml" data-act="like" aria-pressed="${like}" title="${find}" aria-label="${find}">≈</button></li>`;
  }

  const say = (msg) => { K.$("new-msg").textContent = msg; K.$("new-msg").hidden = !msg; };
  async function create() {
    const box = K.$("new-tag"), name = box.value.trim();
    if (!name || making) return;
    if (!K.sel.length) { say("Select pages first: a new tag goes on the selected pages."); return; }
    making = true;
    try {
      const made = await K.api("/api/tags", { name });
      K.lib.tags = made.tags;
      box.value = "";
      say("");
      await K.tagSelection(made.tag);
    } catch (err) { say(err.message); }
    finally { making = false; }
  }

  K.strip = {
    render() {
      const v = K.views.view(), n = K.sel.length, showing = v.kind === "selection";
      K.$("n-sel").textContent = n;
      K.$("selbar").classList.toggle("has", n > 0);
      K.$("clear").disabled = !n;
      K.$("show-sel").setAttribute("aria-pressed", showing);
      K.$("show-sel").disabled = !n && !showing;
      const names = v.tags;
      for (const r of v.rows.concat(K.sel)) for (const t of K.pages.get(r).tags) names.add(t);
      if (v.kind === "like") names.add(v.tag);
      K.$("strip").innerHTML = names.size ? [...names].sort(byName).map((t) => chip(t, v)).join("")
        : `<li class="none">No tags here yet: select pages, then make a new tag.</li>`;
      K.$("app-tags").innerHTML = (K.lib ? K.lib.app_tags : []).map((t) => `<option value="${K.esc(t)}"></option>`).join("");
      K.$("hint").textContent = n ? `A tag's name tags the ${n} selected, or takes it off when they all have it. Its count filters the view; ≈ finds untagged pages like it.`
        : "Click pages to select them. The selection stays across searches until you clear it.";
    }
  };

  K.$("strip").addEventListener("click", (e) => {
    const b = e.target.closest("button[data-act]");
    if (!b) return;
    const tag = b.closest("[data-tag]").dataset.tag;
    if (b.dataset.act === "toggle") K.toggleTag(tag);
    else if (b.dataset.act === "filter") K.views.filter(tag);
    else K.views.like(tag);
  });
  K.$("show-sel").addEventListener("click", () => K.views.showSelection());
  K.$("clear").addEventListener("click", () => K.views.clear());
  K.$("newtag").addEventListener("submit", (e) => { e.preventDefault(); create(); });
  K.$("new-tag").addEventListener("input", () => say(""));
  K.$("new-tag").addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.target.value = "";
    say("");
    e.target.blur();
  });
})();
