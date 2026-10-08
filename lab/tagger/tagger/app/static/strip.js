/* knowmoretabs · tagger app · the selection bar and the tag strip
   Above the grid and sticky: how many pages are selected (Show selection,
   Clear) and how many are pinned (Pinned shows them), then the app tags on
   the pages in view and the selection (and any shown before in this view,
   so one taken off can go back), each with up to three targets acting on
   the selection only. × takes the tag off every selected page that has it
   (shown when one does). The name adds it to every selected page; filled
   when they all have it, filled in part when some do, beside how many of
   the selection have it. ≈ shows pages without this tag like the tag's,
   and pressed again goes back. "+ New tag" makes a tag, or finds yours in
   any case, and puts it on every selected page. */
(function () {
  const K = window.KMT;
  const byName = (a, b) => a.localeCompare(b, undefined, { sensitivity: "base" });
  let making = false;

  function chip(t, v) {
    const n = K.sel.length, s = K.sel.filter((r) => K.has(r, t)).length;
    const on = n && s === n ? "all" : s ? "some" : "none";
    const like = v.kind === "like" && v.tag === t;
    const name = K.esc(t);
    const add = !n ? `Select pages first, then click to add ${name} to them` : on === "all" ? `Every selected page has ${name}`
      : `Add ${name} to the ${n - s} selected without it`;
    const cover = `${s} of your ${n} selected have ${name}`;
    const find = like ? `Back to your search (Esc)` : `Find pages like ${name}, not tagged ${name}`;
    const remove = `Remove ${name} from the ${s} selected with it`;
    return `<li class="tc" data-on="${on}" data-tag="${name}"${on === "some" ? ` style="--cover: ${(100 * s / n).toFixed(1)}%"` : ""}>` +
      (s ? `<button type="button" class="rm" data-act="remove" title="${remove}" aria-label="${remove}">×</button>` : "") +
      `<button type="button" class="nm" data-act="add" aria-pressed="${on === "all" ? "true" : on === "some" ? "mixed" : "false"}" title="${add}">${name}` +
      (n ? `<span class="ct" title="${cover}" aria-label="${cover}">${s}/${n}</span>` : "") + `</button>` +
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
      const v = K.views.view(), n = K.sel.length;
      K.$("n-sel").textContent = n;
      K.$("n-pin").textContent = K.pins.length;
      K.$("selbar").classList.toggle("has", n > 0);
      K.$("clear").disabled = !n;
      for (const [id, kind, count] of [["show-sel", "selection", n], ["show-pins", "pinned", K.pins.length]]) {
        K.$(id).setAttribute("aria-pressed", v.kind === kind);
        K.$(id).disabled = !count && v.kind !== kind;
      }
      const names = v.tags;
      for (const r of K.views.shown(v).concat(K.sel)) for (const t of K.pages.get(r).tags) names.add(t);
      if (v.kind === "like") names.add(v.tag);
      K.$("strip").innerHTML = [...names].sort(byName).map((t) => chip(t, v)).join("");
      K.$("app-tags").innerHTML = (K.lib ? K.lib.app_tags : []).map((t) => `<option value="${K.esc(t)}"></option>`).join("");
    }
  };

  K.$("strip").addEventListener("click", (e) => {
    const b = e.target.closest("button[data-act]");
    if (!b) return;
    const tag = b.closest("[data-tag]").dataset.tag;
    if (b.dataset.act === "add") K.tagSelection(tag);
    else if (b.dataset.act === "remove") K.untagSelection(tag);
    else K.views.like(tag);
  });
  K.$("show-sel").addEventListener("click", () => K.views.showSelection());
  K.$("show-pins").addEventListener("click", () => K.views.showPinned());
  K.$("clear").addEventListener("click", () => K.views.clear());
  K.$("newtag").addEventListener("submit", (e) => { e.preventDefault(); create(); });
  K.$("new-tag").addEventListener("input", () => say(""));
  K.$("new-tag").addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.target.value = "";
    say("");
  });
})();
