/* knowmoretabs · tagger app · the shell
   Two screens, by the address's fragment: Tag (search, select and tag)
   and Add link (`#add`, or `#add=<link>` from the bookmarklet). Loads the
   library, the saved selection and pins, then the last search; owns the
   global keys (Undo included), theme and motion, help and Send to library. (Review,
   swipe and the tag picker keep their scripts but are not on a screen.) */
(function () {
  const K = window.KMT;
  const root = document.documentElement;

  K.changed = () => { K.views.render(); K.strip.render(); K.add.render(); };
  /* the shell also takes the forgotten rows on startup and after Send */
  const loadLibrary = async () => {
    await K.loadLibrary();
    K.gone = new Set(K.lib.forgotten);
  };

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

  /* ---- Send to library: what knowmoretabs would change to make the library match, then, once confirmed, the change ---- */
  const on = (tags, pages) => `${K.plural(tags, "tag")} on ${K.plural(pages, "page")}`;
  const summary = (x) => {
    const rows = [
      x.tags_on && ["Tags added", on(x.tags_on, x.pages_on)],
      x.tags_off && ["Tags removed", on(x.tags_off, x.pages_off)],
      x.never_tags && ["Removed, never tagged here", on(x.never_tags, x.never_pages)],
      x.cleared && ["Pages cleared", K.plural(x.cleared, "page")],
      x.dismiss_tags && ["Dismissed", on(x.dismiss_tags, x.dismiss_pages)],
      x.new.length && ["New tags", K.esc(x.new.join(", "))],
      x.revived.length && ["Back from retired", K.esc(x.revived.join(", "))],
      x.forget && ["Forget", K.plural(x.forget, "page")],
      x.kept_forgotten && ["Forgotten in the library only", K.plural(x.kept_forgotten, "page")],
      x.missing && ["Not in library", K.plural(x.missing, "page")],
    ].filter(Boolean);
    return rows.length ? `<dl>${rows.map(([t, d]) => `<dt>${t}</dt><dd>${d}</dd>`).join("")}</dl>` : "";
  };
  const details = (commands, record) => `<details><summary>Details</summary>` +
    (record ? `<dl><dt>Answers</dt><dd><code>${K.esc(record.answers)}</code></dd><dt>Log</dt><dd><code>${K.esc(record.decisions)}</code></dd>` +
      (record.forget ? `<dt>Forget</dt><dd><code>${K.esc(record.forget)}</code></dd>` : "") + `</dl>` : "") +
    (commands.length ? `<pre>${K.esc(commands.join("\n\n"))}</pre>` : "") + `</details>`;
  const show = (html, go) => {
    K.$("send-body").innerHTML = html;
    K.$("send-go").hidden = !go;
    K.$("send-go").disabled = K.down;
    K.$("send-close").textContent = go ? "Cancel" : "Done";
  };
  let preview;
  K.sendNow = async () => {
    try {
      await K.writes;
      const x = await K.api("api/send/preview", {});
      preview = x.preview;
      show((x.ready ? "" : `<p class="lede">Nothing to send.</p>`) + summary(x) + (x.commands.length ? details(x.commands) : ""), x.ready);
    } catch (err) { show(`<p class="lede">${K.esc(err.message)}</p>`, false); }
    K.$("send").showModal();
    K.$(K.$("send-go").hidden ? "send-close" : "send-go").focus();
  };
  K.$("send-go").addEventListener("click", async () => {
    K.$("send-go").disabled = true;
    try {
      const x = await K.api("api/send", { preview });
      const done = x.steps.filter((s) => s.ok).map((s) => s.command);
      if (x.forget) {
        try {
          await loadLibrary();
          K.changed();
          await K.views.refresh();
        } catch (err) { K.fail(err); }
      }
      show(x.ok
        ? `<p class="lede">Sent.</p>` + summary(x) + details(done, x.record)
        : `<p class="lede">Stopped. ${done.length ? "The commands under Details completed successfully." : "No commands completed successfully."}</p><pre>${K.esc(x.error)}</pre>` + details(done, x.record), false);
    } catch (err) { show(`<p class="lede">${K.esc(err.message)}</p>`, false); }
    K.$("send-close").focus();
  });

  K.$("send-btn").addEventListener("click", K.sendNow);
  K.$("outage-reload").addEventListener("click", () => location.reload());
  K.$("help-btn").addEventListener("click", () => K.$("help").showModal());
  for (const d of ["help", "send"]) K.$(`${d}-close`).addEventListener("click", () => K.$(d).close());

  /* ---- keys ---- */
  document.addEventListener("keydown", (e) => {
    if (K.down) return;   // the service stopped answering: no key acts, as no control does
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
    else if (e.key === "e") K.sendNow();
    else if (K.screen === "add") done = false;   // the rest act on the Tag screen
    else if (e.key === "/") { K.$("s-q").focus(); K.$("s-q").select(); }
    else done = K.views.key(e);
    if (done) e.preventDefault();
  });

  applyTheme();
  route();
  K.ready = (async () => {
    try {
      await loadLibrary();
      await K.loadLists();
      K.changed();
      await K.views.restore();
    } catch (err) { K.fail(err); }
    K.changed();
  })();
})();
