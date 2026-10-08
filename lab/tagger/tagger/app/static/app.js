/* knowmoretabs · tagger app · the shell
   One screen: search, select and tag. Loads the library and the saved
   selection, then the last search; owns the global keys, theme and
   motion, help and export. (Review, swipe and the tag picker keep their
   scripts but are not part of this screen.) */
(function () {
  const K = window.KMT;
  const root = document.documentElement;

  K.changed = () => { K.views.render(); K.strip.render(); };

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

  /* ---- export ---- */
  K.exportNow = async () => {
    try {
      await K.writes;
      const x = await K.api("/api/export", {});
      const cmd = `knowmoretabs --root '/path/to/archive-copy' tag --import '${x.answers.replace(/'/g, "'\\''")}' --accept-new --dry-run`;
      K.$("export-body").innerHTML = x.decided
        ? `<p class="lede">${x.decided} decisions: ${x.answer_tags} tags kept on ${x.answer_pages} pages.</p>` +
          `<dl><dt>Answers</dt><dd><code>${K.esc(x.answers)}</code></dd><dt>Log</dt><dd><code>${K.esc(x.decisions)}</code></dd></dl>` +
          `<p>Replace <code>/path/to/archive-copy</code> with a writable copy of your archive and check it. Drop <code>--dry-run</code> to import into that copy (source <code>${K.esc(x.source)}</code>):</p><pre>${K.esc(cmd)}</pre>`
        : `<p class="lede">Nothing tagged yet, so the files are empty. Select pages and tag them first.</p>`;
      K.$("export").showModal();
    } catch (err) { K.fail(err); }
  };

  K.$("export-btn").addEventListener("click", K.exportNow);
  K.$("help-btn").addEventListener("click", () => K.$("help").showModal());
  for (const d of ["help", "export"]) K.$(`${d}-close`).addEventListener("click", () => K.$(d).close());

  /* ---- keys ---- */
  document.addEventListener("keydown", (e) => {
    if (document.querySelector("dialog[open]")) return;   // the dialog's own keys; esc closes it
    if (e.key !== "Escape" && e.target.closest("input:not([type=checkbox]), textarea")) return;   // Esc also leaves a view while typing
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    let done = true;
    if (e.key === "?") K.$("help").showModal();
    else if (e.key === "t") flipTheme();
    else if (e.key === "/") { K.$("s-q").focus(); K.$("s-q").select(); }
    else if (e.key === "e") K.exportNow();
    else done = K.views.key(e);
    if (done) e.preventDefault();
  });

  applyTheme();
  (async () => {
    try {
      K.lib = await K.api("/api/library");
      await K.loadSel();
      K.changed();
      await K.views.restore();
    } catch (err) { K.fail(err); }
    K.changed();
  })();
})();
