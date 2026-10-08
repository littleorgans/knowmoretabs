/* knowmoretabs · tagger app · pages, the selection and app tags
   Every page this tab has shown, by row, with its app tags (the tags made
   in this app; archive tags stay out of this screen). The selection, which
   the server keeps across searches and restarts until it is cleared. And
   the writes that change either, sent one at a time in order: a change
   shows at once and the server's reply to the latest one settles it. */
(function () {
  const K = window.KMT;
  K.pages = new Map();
  K.sel = [];                      // selected rows, in the order selected
  K.writes = Promise.resolve();    // every write, queued; export waits for them
  let tagging = 0;                 // the latest tag change, whose reply wins

  K.know = (pages) => { for (const p of pages) K.pages.set(p.row, p); return pages.map((p) => p.row); };
  K.has = (row, tag) => K.pages.get(row).tags.includes(tag);
  const count = (n) => `${n} page${n === 1 ? "" : "s"}`;

  /* `body` may be a function, read when the write is sent, so it carries the latest state */
  function write(path, body) {
    const next = K.writes.then(() => K.api(path, typeof body === "function" ? body() : body));
    K.writes = next.catch(() => {});
    return next;
  }

  /* ---- the selection ---- */
  const keep = () => write("/api/selection", () => ({ rows: K.sel })).catch(K.fail);
  K.select = (row) => {
    const i = K.sel.indexOf(row);
    if (i >= 0) K.sel.splice(i, 1); else K.sel.push(row);
    K.changed();
    keep();
  };
  K.clearSel = () => {
    if (!K.sel.length) return;
    K.sel = [];
    K.changed();
    keep();
  };
  K.loadSel = async () => { K.sel = K.know((await K.api("/api/selection")).pages); };

  /* ---- app tags: add `tag` to (value true) or take it off `rows` ---- */
  function change(tag, value, rows, say) {
    const before = new Map(rows.map((r) => [r, K.pages.get(r).tags]));
    for (const r of rows) {
      const tags = before.get(r).filter((t) => t !== tag);
      K.pages.get(r).tags = value ? tags.concat(tag).sort() : tags;
    }
    const mine = ++tagging;
    K.changed();
    return write("/api/apply", { rows, tag, value }).then((out) => {
      if (mine === tagging) for (const p of out.pages) K.pages.get(p.row).tags = p.tags;
      K.lib.app_tags = out.app_tags;
      K.toast(say);
      K.changed();
    }, (err) => {
      if (mine === tagging) for (const [r, tags] of before) K.pages.get(r).tags = tags;
      K.fail(err);
      K.changed();
    });
  }
  const nothing = (msg) => { K.toast(msg); return Promise.resolve(); };

  /* the strip's name: tag every selected page, or take the tag off them all when they all have it */
  K.toggleTag = (tag) => {
    const sel = K.sel.slice();
    if (!sel.length) return nothing("Select pages first, then click a tag to tag them");
    const all = sel.every((r) => K.has(r, tag));
    if (all) return change(tag, false, sel, `Took “${K.short(tag)}” off ${count(sel.length)}`);
    const rows = sel.filter((r) => !K.has(r, tag));
    return change(tag, true, rows, `Tagged ${count(rows.length)} “${K.short(tag)}”`);
  };
  /* "+ New tag": tag every selected page that lacks it */
  K.tagSelection = (tag) => {
    const rows = K.sel.filter((r) => !K.has(r, tag));
    if (!rows.length) return nothing(`Every selected page has “${K.short(tag)}”`);
    return change(tag, true, rows, `Tagged ${count(rows.length)} “${K.short(tag)}”`);
  };
  /* a tile chip's ×: this page only */
  K.untag = (row, tag) => change(tag, false, [row], `Took “${K.short(tag)}” off “${K.short(K.title(K.pages.get(row)))}”`);
})();
