/* knowmoretabs · tagger app · pages, the selection, pins and tags
   Every page this tab has shown, by row, with its tags as the library holds
   them. The selection and the pins, which the server keeps across searches
   and restarts until changed; pinning parks a page for later and never
   selects it. A forgotten page stays in both, out of every view and count,
   and is back in its place once restored. Tags, new names, forget and
   restore are the library's own writes (the shell's API): one at a time in
   order, each shown once the library answers what it now holds, and Undo
   sends back the library's exact record of what a write changed. A failed
   write leaves every page as it was. The service's view of the library (tag
   counts, every name, Recently used) is read again after a write, apart from
   the writes, so neither waits on the other. */
(function () {
  const K = window.KMT;
  K.pages = new Map();
  K.sel = [];                      // selected rows, in the order selected; a forgotten one stays, unshown
  K.pins = [];                     // pinned rows, in the order pinned; likewise
  K.gone = new Set();              // forgotten rows: out of every view
  K.writes = Promise.resolve();    // the service's list writes, queued
  K.mutations = Promise.resolve(); // the library's writes, queued apart from the service
  K.generation = 0;                // bumped by every library write sent and every one answered
  const rows = new Map();          // page key -> row
  const confirmed = new Map();     // row -> the generation a library reply last set its tags at

  /* pages the service sent for a request made at generation `at`: a library reply since then keeps its tags */
  K.know = (pages, replace = true, at = K.generation) => {
    for (const p of pages) {
      const had = K.pages.get(p.row);
      if (!had || replace) K.pages.set(p.row, had && confirmed.get(p.row) > at ? { ...p, tags: had.tags } : p);
      rows.set(p.url, p.row);
    }
    return pages.map((p) => p.row);
  };
  K.has = (row, tag) => K.pages.get(row).tags.includes(tag);
  K.selected = () => K.sel.filter((r) => !K.gone.has(r));
  K.pinned = () => K.pins.filter((r) => !K.gone.has(r));
  const count = (n) => `${n} page${n === 1 ? "" : "s"}`;

  /* `body` may be a function, read when the write is sent, so it carries the latest state */
  function write(path, body) {
    const next = K.writes.then(() => K.api(path, typeof body === "function" ? body() : body));
    K.writes = next.catch(() => {});
    return next;
  }
  /* one library write, after the ones before it */
  function mutate(route, body) {
    K.generation++;
    const sent = K.mutations.then(() => K.shell(route, body));
    K.mutations = sent.catch(() => {});
    return sent.then((out) => { K.generation++; return out; });
  }

  /* The service's view of the library, once the library's writes before it are in. A reply a write overtook
     (sent or answered meanwhile) is read again; a call while one is in flight shares it. */
  let reading = null;
  K.loadLibrary = () => reading || (reading = (async () => {
    try {
      for (;;) {
        await K.mutations;
        const at = K.generation;
        const out = await K.api("api/library");
        if (at !== K.generation) continue;
        K.lib = out;
        K.gone = new Set(out.forgotten);
        return;
      }
    } finally { reading = null; }
  })());
  const refresh = () => K.loadLibrary().then(() => K.changed(), K.fail);   // a failed read leaves the write done
  const failed = (err) => { K.fail(err); K.changed(); };   // every page as the library holds it, said once

  /* ---- the selection and the pins: lists the server keeps, sent whole. A list is sent only once its
     startup load is in: a change made before then is made again on the loaded list, which is then sent ---- */
  const lists = { selection: () => K.sel, pins: () => K.pins };
  const early = { selection: [], pins: [] };   // changes made before the list loaded; null once it has
  const edit = (name, change) => { change(lists[name]()); if (early[name]) early[name].push(change); };
  const keep = (name) => (early[name] ? Promise.resolve() : write(`api/${name}`, () => ({ rows: lists[name]() })).catch(K.fail));
  const put = (row, on) => (list) => {   // `row` in (on) or out of the list; again changes nothing
    const i = list.indexOf(row);
    if (on && i < 0) list.push(row);
    if (!on && i >= 0) list.splice(i, 1);
  };
  K.select = (row) => { edit("selection", put(row, !K.sel.includes(row))); K.changed(); keep("selection"); };
  K.pin = (row) => { edit("pins", put(row, !K.pins.includes(row))); K.changed(); keep("pins"); };
  K.clearSel = () => {
    if (!K.sel.length) return;
    edit("selection", (list) => list.splice(0));
    K.changed();
    keep("selection");
  };
  K.loadLists = () => (K.listsReady = (async () => {
    for (const [name, field] of [["selection", "sel"], ["pins", "pins"]]) {
      const out = await K.api(`api/${name}`);
      K[field] = K.know(out.pages, false);   // startup data cannot replace a page already shown or changed
      const changes = early[name];
      early[name] = null;
      for (const change of changes) change(K[field]);
      if (changes.length) keep(name);
    }
  })());

  /* what a tag reply says the library holds now: each page it names that this tab has, and the active names */
  function adopt(out) {
    for (const [url, tags] of Object.entries(out.tags)) {
      const row = rows.get(url);
      if (row === undefined) continue;
      K.pages.get(row).tags = tags;
      confirmed.set(row, K.generation);
    }
    names(out.vocabulary);
    K.changed();
  }
  const names = (vocabulary) => { if (K.lib) K.lib.app_tags = vocabulary.map((v) => v.name).sort(K.byName); };
  const undoable = (undo) => undo.tags.length > 0 || Object.keys(undo.vocabulary).length > 0;
  /* the library's record of a write, sent back: each decision it changed as it was */
  const reverse = (undo) => mutate("tags", { urls: [], undo }).then((out) => { adopt(out); refresh(); }, failed);

  /* ---- forget: the page leaves every view and count once the library has forgotten it; Undo restores what
     this forget changed, and the page is back in its place in the selection and the pins ---- */
  function hide(urls, gone) {
    for (const url of urls) {
      const row = rows.get(url);
      if (row === undefined || K.gone.has(row) === gone) continue;
      K.gone[gone ? "add" : "delete"](row);
      K.lib.pages += gone ? -1 : 1;
    }
    K.changed();
  }
  K.forget = (row) => {
    if (K.gone.has(row)) return Promise.resolve();
    const url = K.pages.get(row).url;
    return mutate("forget", { urls: [url] }).then((out) => {
      hide([url], true);
      K.toast("Forgotten", out.urls.length ? () => mutate("restore", { urls: out.urls }).then((back) => {
        hide(back.urls, false);
        K.views.refresh(row);
        refresh();
      }, failed) : null);
      refresh();
    }, failed);
  };

  /* ---- tags: `add` and `remove` on `rows` in one library write, shown once it answers, with one toast and its
     Undo. `say()` reads the toast ---- */
  function retag(rows, add, remove, say) {
    const urls = rows.map((r) => K.pages.get(r).url);
    return mutate("tags", { urls, add, remove }).then((out) => {
      adopt(out);
      const lost = out.counts.unknown ? `. ${count(out.counts.unknown)} not in your library` : "";
      K.toast(say() + lost, undoable(out.undo) ? () => reverse(out.undo) : null);
      refresh();
      return out;
    }, failed);
  }
  const nothing = (msg) => { K.toast(msg); return Promise.resolve(); };

  /* the strip's name: add the tag to every selected page (or every page of `on`, the Add link tile's) that lacks it */
  K.tagSelection = (tag, on = K.selected()) => {
    if (!on.length) return nothing(`Select pages first, then click a tag to add it to them`);
    const rows = on.filter((r) => !K.has(r, tag));
    if (!rows.length) return nothing(`Every selected page has ${K.short(tag)}`);
    return retag(rows, [tag], [], () => `Added ${K.short(tag)} to ${count(rows.length)}`);
  };
  /* the strip's ×: take the tag off every selected page (or page of `on`) that has it */
  K.untagSelection = (tag, on = K.selected()) => {
    const rows = on.filter((r) => K.has(r, tag));
    if (!rows.length) return nothing(`No selected page has ${K.short(tag)}`);
    return retag(rows, [], [tag], () => `Removed ${K.short(tag)} from ${count(rows.length)}`);
  };
  const named = (row) => `“${K.short(K.title(K.pages.get(row)))}”`;
  /* a tile chip's ×: this page only */
  K.untag = (row, tag) => retag([row], [], [tag], () => `Took “${K.short(tag)}” off ${named(row)}`);
  /* the tags picked for one page before it had a row, those it lacks in one write: one toast, one Undo */
  K.tagPage = (row, tags) => {
    const lack = tags.filter((t) => !K.has(row, t));
    if (!lack.length) return Promise.resolve();
    return retag([row], lack, [], () => `Added ${lack.map((t) => `“${K.short(t)}”`).join(", ")} to ${named(row)}`);
  };
  /* a name typed as a new tag, in the library's vocabulary (made, brought back, or there in any case): its spelling */
  K.newTag = (name) => mutate("vocabulary", { create: [name] }).then((out) => {
    names(out.vocabulary);
    refresh();
    return out.resolved[0];
  });
})();
