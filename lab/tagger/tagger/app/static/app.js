/* knowmoretabs · tagger app · the shell
   Two screens, by the address's fragment: Tag (search, select and tag)
   and Add link (`#add`, or `#add=<link>` from the bookmarklet). Loads the
   library, the saved selection and pins, then the last search; owns the
   global keys (Undo included), theme and motion, help and export. (Review,
   swipe and the tag picker keep their scripts but are not on a screen.) */
(function () {
  const K = window.KMT;
  const root = document.documentElement;

  K.changed = () => { K.views.render(); K.strip.render(); K.add.render(); };

  /* ---- the screen the fragment names; Tag comes back at the scroll it was left ---- */
  K.screen = null;
  function route() {
    const screen = location.hash.startsWith("#add") ? "add" : "tag";
    const was = K.screen;
    K.screen = screen;
    K.$("tag-screen").hidden = screen !== "tag";
    K.$("add-screen").hidden = screen !== "add";
    for (const s of ["tag", "add"]) {
      if (s === screen) K.$(`nav-${s}`).setAttribute("aria-current", "page"); else K.$(`nav-${s}`).removeAttribute("aria-current");
    }
    if (screen === "add") K.add.enter();
    else if (was === "add") scrollTo(0, K.views.view().scroll);
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

  /* ---- export: the answers to import and, when pages were forgotten, their addresses to forget (one per NUL,
     so xargs passes each as one argument whatever it holds) ---- */
  const word = (s) => `'${s.replace(/'/g, "'\\''")}'`;   // one shell word
  const ROOT = "knowmoretabs";
  K.exportNow = async () => {
    try {
      await K.writes;
      const x = await K.api("/api/export", {});
      const files = (x.decided ? `<dt>Answers</dt><dd><code>${K.esc(x.answers)}</code></dd><dt>Log</dt><dd><code>${K.esc(x.decisions)}</code></dd>` : "") +
        (x.forgotten ? `<dt>Forget</dt><dd><code>${K.esc(x.forget)}</code></dd>` : "");
      const tagged = `<p class="lede">${x.decided} decisions: ${x.answer_tags} tags kept on ${x.answer_pages} pages.</p>`;
      const imports = `<p>Check these tags against your library. Drop <code>--dry-run</code> to import into your library (source <code>${K.esc(x.source)}</code>):</p>` +
        `<pre>${K.esc(`${ROOT} tag --import ${word(x.answers)} --accept-new --dry-run`)}</pre>`;
      const forgets = `<p>Forget the ${x.forgotten} forgotten page${x.forgotten === 1 ? "" : "s"} in your library (<code>restore</code> in place of <code>forget</code> brings them back):</p>` +
        `<pre>${K.esc(`xargs -0 ${ROOT} forget -- < ${word(x.forget || "")}`)}</pre>`;
      K.$("export-body").innerHTML = x.decided || x.forgotten
        ? (x.decided ? tagged : "") + `<dl>${files}</dl>` + (x.decided ? imports : "") + (x.forgotten ? forgets : "")
        : `<p class="lede">Nothing tagged or forgotten yet.</p>`;
      K.$("export").showModal();
    } catch (err) { K.fail(err); }
  };

  K.$("export-btn").addEventListener("click", K.exportNow);
  K.$("help-btn").addEventListener("click", () => K.$("help").showModal());
  for (const d of ["help", "export"]) K.$(`${d}-close`).addEventListener("click", () => K.$(d).close());

  /* ---- keys ---- */
  document.addEventListener("keydown", (e) => {
    if (document.querySelector("dialog[open]")) return;   // the dialog's own keys; esc closes it
    if (e.target.closest("input:not([type=checkbox]), textarea")) {   // typing; a ticked box keeps the keys
      if (e.key === "Escape") e.target.blur();   // Esc leaves the box only; the next one acts on the view
      return;
    }
    const z = (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "z";
    if (K.undo && (z || (e.key === "u" && !e.altKey && !e.metaKey && !e.ctrlKey))) { K.undo(); e.preventDefault(); return; }
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    let done = true;
    if (e.key === "?") K.$("help").showModal();
    else if (e.key === "t") flipTheme();
    else if (e.key === "e") K.exportNow();
    else if (K.screen === "add") done = false;   // the rest act on the Tag screen
    else if (e.key === "/") { K.$("s-q").focus(); K.$("s-q").select(); }
    else done = K.views.key(e);
    if (done) e.preventDefault();
  });

  applyTheme();
  route();
  K.ready = (async () => {
    try {
      K.lib = await K.api("/api/library");
      K.gone = new Set(K.lib.forgotten);
      await K.loadLists();
      K.changed();
      await K.views.restore();
    } catch (err) { K.fail(err); }
    K.changed();
  })();
})();
