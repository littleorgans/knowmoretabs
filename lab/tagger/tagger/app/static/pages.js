/* knowmoretabs · tagger app · pages, the selection, pins and app tags
   Every page this tab has shown, by row, with its app tags (the tags made
   in this app; archive tags stay out of this screen). The selection and
   the pins, which the server keeps across searches and restarts until
   changed; pinning parks a page for later and never selects it. Forgotten
   pages, out of every view at once. And the writes that change any of
   these, sent one at a time in order: a change shows at once and the
   server's reply to the latest one settles it. A tag change or a forget
   offers Undo, which puts every page back exactly as it was. */
(function () {
  const K = window.KMT;
  K.pages = new Map();
  K.sel = [];                      // selected rows, in the order selected
  K.pins = [];                     // pinned rows, in the order pinned
  K.gone = new Set();              // forgotten rows (the server's, then this tab's): out of every view
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

  /* ---- the selection and the pins: lists the server keeps, sent whole ---- */
  const lists = { selection: () => K.sel, pins: () => K.pins };
  const keep = (name) => write(`/api/${name}`, () => ({ rows: lists[name]() })).catch(K.fail);
  const flip = (list, row) => { const i = list.indexOf(row); if (i >= 0) list.splice(i, 1); else list.push(row); };
  K.select = (row) => { flip(K.sel, row); K.changed(); keep("selection"); };
  K.pin = (row) => { flip(K.pins, row); K.changed(); keep("pins"); };
  K.clearSel = () => {
    if (!K.sel.length) return;
    K.sel = [];
    K.changed();
    keep("selection");
  };
  K.loadLists = async () => {
    K.sel = K.know((await K.api("/api/selection")).pages);
    K.pins = K.know((await K.api("/api/pins")).pages);
  };

  /* ---- forget: the page leaves every view, the selection, the pins and the counts at once.
     Undo puts it back in its place, selected and pinned as it was ---- */
  K.forget = (row) => {
    if (K.gone.has(row)) return;
    const was = { selection: K.sel.indexOf(row), pins: K.pins.indexOf(row) };
    const out = () => {
      K.gone.add(row);
      for (const name in was) { const list = lists[name](), i = list.indexOf(row); if (i >= 0) list.splice(i, 1); }
      K.lib.pages--;
      K.changed();
    };
    const back = () => {
      K.gone.delete(row);
      for (const name in was) {
        const list = lists[name]();
        if (was[name] >= 0 && !list.includes(row)) list.splice(Math.min(was[name], list.length), 0, row);
      }
      K.lib.pages++;
      K.changed();
    };
    const send = (value) => write("/api/forget", { rows: [row], value }).then((reply) => { K.lib.pages = reply.pages; K.changed(); });
    out();
    send(true).catch((err) => { back(); K.fail(err); });
    K.toast("Forgotten", () => {
      if (!K.gone.has(row)) return;
      back();
      send(false).then(() => K.views.restored(row)).catch((err) => { out(); K.fail(err); });
      for (const name in was) if (was[name] >= 0) keep(name);
    });
  };

  /* ---- app tags: add `tag` to (value true) or take it off `rows`; Undo retracts the server's batch ---- */
  const set = (rows, tag, value) => {
    for (const r of rows) {
      const tags = K.pages.get(r).tags.filter((t) => t !== tag);
      K.pages.get(r).tags = value ? tags.concat(tag).sort() : tags;
    }
  };
  function settle(path, body, mine, onError) {
    return write(path, body).then((out) => {
      if (mine === tagging) for (const p of out.pages) K.pages.get(p.row).tags = p.tags;
      K.lib.app_tags = out.app_tags;
      K.changed();
      return out;
    }, (err) => { onError(); K.fail(err); K.changed(); });
  }
  function change(tag, value, rows, say) {
    const before = new Map(rows.map((r) => [r, K.pages.get(r).tags]));
    set(rows, tag, value);
    const mine = ++tagging;
    K.changed();
    return settle("/api/apply", { rows, tag, value }, mine, () => {
      if (mine === tagging) for (const [r, tags] of before) K.pages.get(r).tags = tags;
    }).then((out) => { if (out) K.toast(say, out.batch ? () => undo(out.batch, rows, tag, value) : null); });
  }
  function undo(batch, rows, tag, value) {
    set(rows, tag, !value);
    const mine = ++tagging;
    K.changed();
    return settle("/api/undo", { batch }, mine, () => { if (mine === tagging) set(rows, tag, value); });
  }
  const nothing = (msg) => { K.toast(msg); return Promise.resolve(); };

  /* the strip's name: add the tag to every selected page that lacks it */
  K.tagSelection = (tag) => {
    if (!K.sel.length) return nothing(`Select pages first, then click a tag to add it to them`);
    const rows = K.sel.filter((r) => !K.has(r, tag));
    if (!rows.length) return nothing(`Every selected page has ${K.short(tag)}`);
    return change(tag, true, rows, `Added ${K.short(tag)} to ${count(rows.length)}`);
  };
  /* the strip's ×: take the tag off every selected page that has it */
  K.untagSelection = (tag) => {
    const rows = K.sel.filter((r) => K.has(r, tag));
    if (!rows.length) return nothing(`No selected page has ${K.short(tag)}`);
    return change(tag, false, rows, `Removed ${K.short(tag)} from ${count(rows.length)}`);
  };
  /* a tile chip's ×: this page only */
  K.untag = (row, tag) => change(tag, false, [row], `Took “${K.short(tag)}” off “${K.short(K.title(K.pages.get(row)))}”`);
})();
