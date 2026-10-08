/* knowmoretabs · tagger app · the shell
   Routes between the three steps (#search, #tag, #review/<set>), owns the
   global keys, theme and motion, the list of earlier sets and export. */
(function () {
  const K = window.KMT;
  const views = { search: K.search, tag: K.pick, review: K.review };
  const NAMES = { search: "Search", tag: "Pick tags", review: "Review" };
  const root = document.documentElement;
  let view = "search";

  function render() {
    views[view].render();
    K.$("n-review").textContent = K.review.left() || "";
  }
  K.changed = render;

  async function route() {
    const [name, id] = location.hash.slice(1).split("/");
    view = views[name] ? name : "search";
    if (view === "review") {
      const sid = Number(id) || K.state.session;
      if (sid) {
        try { await K.review.load(sid); K.state.session = sid; K.save(); } catch (err) { K.fail(err); }
      }
    }
    for (const v in views) {
      K.$(`view-${v}`).hidden = v !== view;
      if (v === view) K.$(`nav-${v}`).setAttribute("aria-current", "page"); else K.$(`nav-${v}`).removeAttribute("aria-current");
    }
    document.title = `${NAMES[view]} · knowmoretabs tagger`;
    render();
  }
  addEventListener("hashchange", route);

  /* ---- settings ---- */
  function applyTheme() {
    if (K.state.theme) root.dataset.theme = K.state.theme; else delete root.dataset.theme;
    root.dataset.motion = K.state.motion ? "on" : "off";
    K.$("motion").checked = K.state.motion;
  }
  function flipTheme() {
    const dark = K.state.theme ? K.state.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
    K.state.theme = dark ? "light" : "dark";
    K.save();
    applyTheme();
  }
  K.$("theme").addEventListener("click", flipTheme);
  K.$("motion").addEventListener("change", (e) => { K.state.motion = e.target.checked; K.save(); applyTheme(); });

  /* ---- earlier sets ---- */
  K.refreshSets = async () => {
    const sets = await K.api("/api/sessions");
    K.$("n-sets").textContent = sets.length;
    K.$("sets-word").textContent = sets.length === 1 ? "set" : "sets";
    return sets;
  };
  async function openSets() {
    const sets = await K.refreshSets();
    K.$("sets-list").innerHTML = sets.length ? sets.map((s) =>
      `<li><b>“${K.esc(K.short(s.query))}”</b><small>${K.esc(s.picked.join(", "))} · ${s.decided} of ${s.pages} pages decided</small>` +
      `<button type="button" data-set="${s.id}">Open</button></li>`).join("") : `<li class="nil">No sets yet.</li>`;
    K.$("sets").showModal();
  }
  K.$("sets-list").addEventListener("click", (e) => {
    const b = e.target.closest("button[data-set]");
    if (!b) return;
    K.$("sets").close();
    location.hash = `review/${b.dataset.set}`;
  });

  /* ---- export ---- */
  K.exportNow = async () => {
    try {
      await K.review.pending;
      const x = await K.api("/api/export", {});
      const cmd = `knowmoretabs --root '/path/to/archive-copy' tag --import '${x.answers.replace(/'/g, "'\\''")}' --accept-new --dry-run`;
      K.$("export-body").innerHTML = x.decided
        ? `<p class="lede">${x.decided} decisions (${x.flipped} flipped from the model): ${x.answer_tags} tags kept on ${x.answer_pages} pages.</p>` +
          `<dl><dt>Answers</dt><dd><code>${K.esc(x.answers)}</code></dd><dt>Log</dt><dd><code>${K.esc(x.decisions)}</code></dd></dl>` +
          `<p>Replace <code>/path/to/archive-copy</code> with a writable copy of your archive and check it. Drop <code>--dry-run</code> to import into that copy (source <code>${K.esc(x.source)}</code>):</p><pre>${K.esc(cmd)}</pre>`
        : `<p class="lede">Nothing decided yet, so the files are empty. Review a set first.</p>`;
      K.$("export").showModal();
    } catch (err) { K.fail(err); }
  };

  K.$("sets-btn").addEventListener("click", () => openSets().catch(K.fail));
  K.$("export-btn").addEventListener("click", K.exportNow);
  K.$("help-btn").addEventListener("click", () => K.$("help").showModal());
  for (const d of ["help", "sets", "export"]) K.$(`${d}-close`).addEventListener("click", () => K.$(d).close());

  /* ---- keys ---- */
  document.addEventListener("keydown", (e) => {
    if (document.querySelector("dialog[open]")) return;   // the dialog's own keys; esc closes it
    if (e.target.closest("input, textarea")) return;
    if (e.altKey || ((e.metaKey || e.ctrlKey) && e.key !== "z")) return;
    let done = true;
    if (e.key === "?") K.$("help").showModal();
    else if (e.key === "t") flipTheme();
    else if (e.key === "/") { location.hash = "search"; K.$("s-q").focus(); }
    else if (e.key === "e") K.exportNow();
    else if (!(done = views[view].key(e)) && e.key === "o") { done = true; openSets().catch(K.fail); }   // o opens a focused page first
    if (done) e.preventDefault();
  });

  applyTheme();
  (async () => {
    try {
      K.lib = await K.api("/api/library");
      await K.refreshSets();
      await K.search.restore();
    } catch (err) { K.fail(err); }
    route();
  })();
})();
