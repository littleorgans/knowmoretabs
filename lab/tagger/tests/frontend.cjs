// Run the shipped scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const { setup, turn, plain, run } = require('./frontend_dom.cjs');

async function openControl() {
  const { K } = setup();
  const html = K.open({ url: 'https://example.test/a?b=1', host: 'example.test' });
  assert.match(html, /data-open href="https:\/\/example.test\/a\?b=1" target="_blank" rel="noopener noreferrer"/);
  assert.match(K.open({ url: 'http://example.test/', host: 'example.test' }), /data-open/);
  for (const url of ['javascript:void(0)', 'file:///tmp/a', 'data:text/html,a', 'not a url', undefined]) {
    assert.equal(K.open({ url, host: 'x' }), '', `${url} must get no open control`);
  }
}

/* ---- one screen: search, select, tag ---- */
const page = (row, tags = []) => ({ row, title: `Page ${row}`, host: 'example.test', url: `https://example.test/${row}`, image: false, tags });

const url = (row) => `https://example.test/${row}`;

/* The library's own API as the shell answers it: each page's tags by key, the forgotten keys and the names. A tag
   write answers every page it names with its tags afterwards and the exact record that undoes it; an undo answers
   with its own inverse. Every https://example.test/ page is in the library; any other key is not. */
function library(names = ['A']) {
  const held = new Map(), gone = new Set(), vocabulary = new Set(names);
  const known = (key) => key.startsWith('https://example.test/');
  const tags = (key) => [...(held.get(key) || [])].sort();
  const decide = (key, name, add) => { const t = new Set(held.get(key)); if (add) t.add(name); else t.delete(name); held.set(key, t); };
  const vocab = () => [...vocabulary].sort().map((name) => ({ name, created_at: '2026-10-01T00:00:00Z' }));
  const outcome = (urls, undo) => ({
    undo, urls: urls.filter(known), tags: Object.fromEntries(urls.filter(known).map((key) => [key, tags(key)])), vocabulary: vocab(),
    counts: { changed: urls.filter(known).length, unchanged: 0, unknown: urls.filter((key) => !known(key)).length },
  });
  return {
    held, gone, vocabulary,
    know(pages) { for (const p of pages) if (!held.has(p.url)) held.set(p.url, new Set(p.tags)); },
    '/api/tags'(body) {
      const undo = { tags: [], vocabulary: {} };
      if (body.undo) {
        for (const d of body.undo.tags) {
          undo.tags.push({ url: d.url, name: d.name, add: tags(d.url).includes(d.name), remove: !d.add });
          decide(d.url, d.name, d.add);
        }
        return outcome([...new Set(body.undo.tags.map((d) => d.url))], undo);
      }
      for (const key of body.urls.filter(known)) {
        for (const name of body.add) {
          vocabulary.add(name);
          if (!tags(key).includes(name)) { undo.tags.push({ url: key, name, add: false, remove: false }); decide(key, name, true); }
        }
        for (const name of body.remove) { undo.tags.push({ url: key, name, add: tags(key).includes(name), remove: false }); decide(key, name, false); }
      }
      return outcome(body.urls, undo);
    },
    '/api/forget'(body) { const urls = body.urls.filter((key) => known(key) && !gone.has(key)); urls.forEach((key) => gone.add(key)); return { urls, forgotten: urls, counts: {} }; },
    '/api/restore'(body) { const urls = body.urls.filter((key) => gone.has(key)); urls.forEach((key) => gone.delete(key)); return { urls, restored: urls, counts: {} }; },
    '/api/vocabulary'(body) {
      const resolved = body.create.map((n) => [...vocabulary].find((v) => v.toLowerCase() === n.trim().toLowerCase()) || n.trim());
      resolved.forEach((n) => vocabulary.add(n));
      return { vocabulary: vocab(), resolved };
    },
  };
}

/* the search "q" has found pages 1 (tagged A), 2 and 3 */
async function screenFixture(...more) {
  const f = setup('pages.js', 'views.js', 'strip.js', ...more);
  const { K, node } = f;
  K.changed = () => { K.views.render(); K.strip.render(); };
  K.lib = { sizes: [20, 50], images: true, pages: 9, app_tags: ['A'], recent: [], tags: [], forgotten: [] };
  await turn();   // a shell loaded for its keys finishes its unmocked startup before this fixture takes over
  const requests = [], sent = [];
  K.api = (route, body) => new Promise((resolve, reject) => {
    sent.push(route);
    requests.push({ route, body: body && plain(body), resolve, reject });
  });
  const loading = K.loadLists();   // the startup load of the selection and the pins, both empty
  for (let i = 0; i < 2; i++) { await turn(); requests.shift().resolve({ pages: [] }); }
  await loading;
  node('s-q').value = 'q';
  const searching = K.views.run();
  await turn();
  requests.shift().resolve({ hits: [page(1, ['A']), page(2), page(3)], offset: 0, total: 3 });
  await searching;
  const lib = library();
  lib.know([page(1, ['A']), page(2), page(3)]);
  const tile = (row) => ({ dataset: { row: String(row) }, querySelector: () => f.opened });
  Object.assign(f, { requests, sent, library: lib });
  f.opened = { clicks: 0, click() { this.clicks++; } };
  f.click = (row, at = {}) => {   // a click on tile `row`, on whatever `at` says is under it
    const e = { stopped: false, stopPropagation() { this.stopped = true; }, target: { closest: (s) => (s === '.tile[data-row]' ? tile(row) : at[s] || null) } };
    node('grid').click(e);
    return e;
  };
  f.key = (row, key) => K.views.key({ key, target: { closest: (s) => (s === '.tile[data-row]' && row !== null ? tile(row) : null) } });
  f.chip = (tag, act) => node('strip').click({ target: { closest: (s) => (s === 'button[data-act]' ? { dataset: { act }, closest: () => ({ dataset: { tag } }) } : null) } });
  /* what the library and the service answer, from what the page sent */
  f.answer = (r) => {
    if (lib[r.route]) return lib[r.route](r.body);
    if (r.route === 'api/library') {
      const forgotten = [...lib.gone].map((key) => Number(key.split('/').pop()));
      return { ...K.lib, pages: 9 - forgotten.length, forgotten };
    }
    return { rows: r.body.rows };
  };
  f.settle = async () => { await turn(); while (requests.length) { const r = requests.shift(); r.resolve(f.answer(r)); await turn(); } };
  f.next = (route) => {
    const i = requests.findIndex((r) => r.route === route);
    assert.ok(i >= 0, `a ${route} request among ${requests.map((r) => r.route)}`);
    return requests.splice(i, 1)[0];
  };
  f.shown = () => [...node('grid').innerHTML.matchAll(/data-row="(\d+)"/g)].map((m) => Number(m[1]));
  f.press = (key, mods = {}) => f.keys.keydown({ key, ...mods, target: { closest: () => null }, preventDefault() {} });
  return f;
}

async function select() {
  const { K, node, requests, click, key, settle } = await screenFixture();
  click(2);
  assert.deepEqual(plain(K.sel), [2], 'a click selects the tile');
  assert.match(node('grid').innerHTML, /class="hit tile sel" data-row="2"/);
  assert.doesNotMatch(node('grid').innerHTML, /class="hit tile sel" data-row="[13]"/);
  assert.equal(node('n-sel').textContent, 1);
  await turn();
  assert.deepEqual(requests[0], { ...requests[0], route: 'api/selection', body: { rows: [2] } }, 'the selection is kept by the server');
  assert.equal(key(3, ' '), true, 'space on the focused tile is handled');
  assert.deepEqual(plain(K.sel), [2, 3], 'space selects the focused tile');
  assert.equal(key(2, 'x'), true);
  assert.deepEqual(plain(K.sel), [3], 'x unselects it');
  assert.equal(requests.length, 1, 'writes go one at a time');
  await settle();
  click(3);
  assert.deepEqual(plain(K.sel), [], 'a second click unselects');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [] }, 'each write carries the selection as it is when sent');
}

/* the strip chip [× | name s/n | ≈] acts on the selection only: the name adds, × removes, the count is coverage */

/* the strip chip [× | name s/n | ≈] acts on the selection only: the name adds, × removes, the count is coverage */
async function chipActsOnSelection() {
  const { K, node, requests, click, chip, settle } = await screenFixture();
  const strip = () => node('strip').innerHTML;
  assert.doesNotMatch(strip(), /class="ct"/, 'no selection: no count');
  assert.doesNotMatch(strip(), /data-act="remove"/, 'no selection: no ×');
  assert.doesNotMatch(strip(), /data-act="filter"/, 'the count filters nothing');
  chip('A', 'add');
  await turn();
  assert.equal(requests.length, 0, 'no selection, nothing tagged');
  assert.match(K.toasted, /Select pages first/);
  click(1); click(2); click(3); await settle();
  assert.match(strip(), /data-on="some" data-tag="A" style="--cover: 33.3%"><button type="button" class="rm" data-act="remove"/, 'partial fill, and × as one selected page has it');
  assert.match(strip(), /aria-pressed="mixed"[^>]*>A<span class="ct" title="1 of your 3 selected have A"[^>]*>1\/3<\/span>/, 'the count is selection coverage');
  chip('A', 'add');
  await turn();
  assert.deepEqual(requests[0].body, { urls: [url(2), url(3)], add: ['A'], remove: [] }, 'the name adds to the selected pages lacking it');
  assert.match(strip(), /data-on="some"/, 'shown as the library holds it until it answers');
  await settle();
  assert.match(strip(), /data-on="all"/, 'then as it answers');
  assert.match(K.toasted, /^Added A to 2 pages$/);
  chip('A', 'add');
  await turn();
  assert.equal(requests.length, 0, 'the name never takes a tag off, even when every selected page has it');
  assert.match(K.toasted, /Every selected page has A/);
  click(3); await settle();
  chip('A', 'remove');
  await turn();
  assert.deepEqual(requests[0].body, { urls: [url(1), url(2)], add: [], remove: ['A'] }, '× takes it off the selected pages only');
  await settle();
  assert.match(K.toasted, /^Removed A from 2 pages$/);
  assert.equal(K.has(3, 'A'), true, 'a page outside the selection keeps it');
  assert.doesNotMatch(strip(), /data-act="remove"/, 'no selected page has it: no ×');
  assert.match(strip(), /data-on="none"[^]*>0\/2<\/span>/, 'a tag on none of the selection stays to put back');
  K.clearSel(); await settle();
  assert.doesNotMatch(strip(), /class="ct"/, 'the count hides with no selection');
}

/* Undo after a strip action sends the library's own record back: each page exactly as it was */
async function bulkUndo() {
  const { K, requests, click, chip, settle, press } = await screenFixture('app.js');
  const has = () => [1, 2, 3].map((r) => K.has(r, 'A'));
  click(1); click(2); click(3); await settle();
  chip('A', 'add'); await settle();
  assert.deepEqual(has(), [true, true, true]);
  press('u');
  await turn();
  const undo = { tags: [{ url: url(2), name: 'A', add: false, remove: false }, { url: url(3), name: 'A', add: false, remove: false }], vocabulary: {} };
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/tags', { urls: [], undo }], 'Undo sends the record as the library made it');
  assert.deepEqual(has(), [true, true, true], 'shown as it is until the library answers');
  await settle();
  assert.deepEqual(has(), [true, false, false], 'Undo takes it off only the pages it was added to');
  chip('A', 'remove'); await settle();
  assert.deepEqual(has(), [false, false, false]);
  assert.equal(typeof K.undo, 'function');
  press('z', { ctrlKey: true });
  await turn();
  assert.deepEqual(requests[0].body.undo.tags.map((d) => [d.url, d.add]), [[url(1), true]], 'the page it came off');
  await settle();
  assert.deepEqual(has(), [true, false, false], 'Ctrl+Z undoes too: only the page that had it gets it back');
}

/* the tags picked for one page go in one write, one toast and one Undo; a failed write leaves the page as it was,
   says so, and is never sent again */
async function tagPageUndo() {
  const { K, requests, settle, press } = await screenFixture('app.js');
  let toasts = 0;
  const toast = K.toast;
  K.toast = (...args) => { toasts++; return toast.apply(K, args); };
  K.title = (p) => p.title;
  K.tagPage(1, ['C', 'A', 'B']);
  await turn();
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/tags', { urls: [url(1)], add: ['C', 'B'], remove: [] }], 'one write, the tags it lacks');
  assert.deepEqual(plain(K.pages.get(1).tags), ['A'], 'nothing shows before the library answers');
  await settle();
  assert.deepEqual(plain(K.pages.get(1).tags), ['A', 'B', 'C']);
  assert.equal(toasts, 1, 'one toast');
  assert.equal(K.toasted, 'Added “C”, “B” to “Page 1”');
  press('u');
  await settle();
  assert.deepEqual(plain(K.pages.get(1).tags), ['A'], 'Undo takes both off in one write');
  toasts = 0;
  K.tagPage(1, ['B']);
  await turn();
  requests.shift().reject(new Error('synthetic failure'));
  await turn(); await turn();
  assert.deepEqual(plain(K.pages.get(1).tags), ['A'], 'a failed write leaves the page as it was');
  assert.match(K.failure.message, /synthetic failure/, 'and says so');
  assert.equal(toasts, 0, 'no result toast, no Undo');
  await turn();
  assert.equal(requests.length, 0, 'nothing is sent again');
}

/* The library's writes go to the shell's own API by key; its answer is the truth for every page it names */
async function writesAreTheLibrarys() {
  const { K, next } = await screenFixture();
  K.title = (p) => p.title;
  const brought = K.tagSelection('A', [2]);
  await turn();
  const r = next('/api/tags');
  assert.equal(r.route, '/api/tags', "the shell's own API, at its root");
  assert.deepEqual(r.body, { urls: [url(2)], add: ['A'], remove: [] }, 'pages by key, never by row');
  r.resolve({ undo: { tags: [{ url: url(2), name: 'A', add: false, remove: false }], vocabulary: { A: '2026-10-01T00:00:00Z' } },
    urls: [url(2)], tags: { [url(2)]: ['A'], [url(3)]: ['A'] }, vocabulary: [{ name: 'A' }, { name: 'b' }],
    counts: { changed: 1, unchanged: 0, unknown: 0 } });
  await brought;
  assert.equal(K.has(3, 'A'), true, 'every page the answer names follows it, as a name brought back shows on all');
  assert.deepEqual(plain(K.lib.app_tags), ['A', 'b'], 'the active names, as the library answers them');
  const unknown = K.tagSelection('B', [1, 2]);
  K.pages.get(2).url = 'https://elsewhere.example/2';   // a key the library does not hold
  await turn();
  next('/api/tags').resolve({ undo: { tags: [{ url: url(1), name: 'B', add: false, remove: false }], vocabulary: {} }, urls: [url(1)],
    tags: { [url(1)]: ['A', 'B'] }, vocabulary: [{ name: 'A' }, { name: 'B' }], counts: { changed: 1, unchanged: 0, unknown: 1 } });
  await unknown;
  assert.equal(K.has(2, 'B'), false, 'a page the answer leaves out stays as it was');
  assert.equal(K.toasted, 'Added B to 2 pages. 1 page not in your library');
  const dismissed = K.untag(1, 'B');
  await turn();
  next('/api/tags').resolve({ undo: { tags: [{ url: url(1), name: 'B', add: false, remove: false }], vocabulary: {} }, urls: [],
    tags: { [url(1)]: ['A'] }, vocabulary: [{ name: 'A' }, { name: 'B' }], counts: { changed: 0, unchanged: 1, unknown: 0 } });
  await dismissed;
  assert.equal(typeof K.undo, 'function', 'a write that changed no shown tag still recorded a decision: Undo');
  const nothing = K.tagSelection('C', [1]);
  await turn();
  next('/api/tags').resolve({ undo: { tags: [], vocabulary: {} }, urls: [], tags: { [url(1)]: ['A', 'C'] }, vocabulary: [{ name: 'A' }, { name: 'C' }],
    counts: { changed: 0, unchanged: 1, unknown: 0 } });
  await nothing;
  assert.equal(K.undo, null, 'nothing recorded, nothing to undo');
}

/* After a library write the service's view is read again for Recently used and the tag counts. A read the write
   overtook is read again, so an older order never comes back, and a later refresh shares the read in flight */
async function tagRecent() {
  const { K, requests, library } = await screenFixture();
  K.lib.recent = ['A'];
  const reading = K.loadLibrary();   // one in flight already, as Add link's
  await turn();
  const old = requests.shift();
  assert.equal(old.route, 'api/library');
  const write = K.tagSelection('B', [1]);
  await turn();
  const tag = requests.shift();
  assert.equal(tag.route, '/api/tags', 'the write goes at once, never behind the service read');
  tag.resolve(library['/api/tags'](tag.body));
  await write;
  old.resolve({ ...K.lib, recent: ['A'], forgotten: [] });   // what the service held before the write
  await turn(); await turn(); await turn();
  const fresh = requests.shift();
  assert.equal(fresh && fresh.route, 'api/library', 'a read the write overtook is read again');
  fresh.resolve({ ...K.lib, recent: ['B', 'A'], forgotten: [] });
  await reading;
  assert.deepEqual(plain(K.lib.recent), ['B', 'A'], 'the newer order stands');
  assert.equal(requests.length, 0, "the write's own refresh shared that read");
}

/* A held service, its list writes and its failed reads never hold up a library write or take one back */
async function writesNeverWaitOnTheService() {
  const { K, requests, click, library, next } = await screenFixture();
  click(2);   // the selection's write to the service, held
  await turn();
  const list = next('api/selection');
  const read = K.loadLibrary().catch((err) => err);
  await turn();
  const held = next('api/library');
  const first = K.tagSelection('A', [2]);
  await turn();
  const one = next('/api/tags');
  one.resolve(library['/api/tags'](one.body));
  await first;
  const second = K.untag(2, 'A');
  await turn();
  const two = next('/api/tags');
  two.resolve(library['/api/tags'](two.body));
  await second;
  assert.equal(K.has(2, 'A'), false, 'both writes answered while the service held its own');
  held.reject(new Error('the model service took too long'));
  assert.match((await read).message, /took too long/);
  await turn();
  assert.equal(K.has(2, 'A'), false, 'a failed read takes back no write');
  assert.equal(typeof K.undo, 'function', 'nor its Undo');
  list.resolve({ rows: [2] });
}

/* A search the service ranked before a library write answered cannot put back the tags the write replaced; one
   asked after it shows the service's */
async function searchKeepsNewerTags() {
  const { K, requests, library, next } = await screenFixture();
  const searching = K.views.run();
  await turn();
  const search = next('api/search');
  const write = K.tagSelection('B', [1]);
  await turn();
  const tag = next('/api/tags');
  tag.resolve(library['/api/tags'](tag.body));
  await write;
  search.resolve({ hits: [page(1, ['A']), page(2)], offset: 0, total: 2 });
  await searching;
  assert.deepEqual(plain(K.pages.get(1).tags), ['A', 'B'], 'the page as the library answered');
  const again = K.views.run();
  await turn(); await turn();
  next('api/search').resolve({ hits: [page(1, ['A', 'B', 'C'])], offset: 0, total: 1 });
  await again;
  assert.deepEqual(plain(K.pages.get(1).tags), ['A', 'B', 'C'], "a later search's tags are the service's");
  assert.ok(requests.every((r) => r.route === 'api/library'), 'nothing else waits');
}

/* Forget: the library forgets the page, then it leaves the grid and every count; the selection and the pins keep
   it, out of sight, and Undo restores what the forget changed, back in its place */
async function forgetAndUndo() {
  const { K, node, requests, sent, click, key, settle, press, shown } = await screenFixture('app.js');
  click(1); click(2); key(2, 'p'); await settle();
  sent.length = 0;
  const e = click(2, { '[data-forget]': {} });
  assert.equal(e.stopped, false);
  assert.deepEqual(shown(), [1, 2, 3], 'shown until the library has forgotten it');
  await turn();
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/forget', { urls: [url(2)] }]);
  await settle();
  assert.deepEqual(shown(), [1, 3], 'then out of the grid');
  assert.deepEqual(plain(K.sel), [1, 2], 'the selection keeps its place');
  assert.deepEqual(plain(K.pins), [2], 'and the pins');
  assert.equal(node('n-sel').textContent, 1, 'counted in neither');
  assert.equal(node('n-pin').textContent, 0);
  assert.match(node('v-pos').innerHTML, /^<b>1 to 2<\/b> of 2 for “q”$/, 'the counts drop it');
  assert.equal(K.lib.pages, 8);
  assert.match(node('strip').innerHTML, />1\/1<\/span>/);
  assert.equal(K.toasted, 'Forgotten');
  assert.deepEqual(sent, ['/api/forget', 'api/library'], 'no list is written');
  press('u');
  await turn();
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/restore', { urls: [url(2)] }], 'Undo restores what it forgot');
  await settle();
  assert.deepEqual(shown(), [1, 2, 3], 'back in its place');
  assert.deepEqual(plain(K.selected()), [1, 2], 'selected as it was');
  assert.deepEqual(plain(K.pinned()), [2], 'and pinned');
  assert.equal(K.lib.pages, 9);
  assert.equal(key(3, 'f'), true, 'f forgets the focused page');
  assert.deepEqual(plain(K.sel), [1, 2], 'and selects nothing');
  await turn();
  assert.deepEqual(requests[0].body, { urls: [url(3)] });
  await settle();
  assert.deepEqual(shown(), [1, 2]);
  K.forget(1);
  await turn();
  requests.shift().reject(new Error('synthetic failure'));
  await turn(); await turn();
  assert.deepEqual(shown(), [1, 2], 'a failed forget leaves the page shown');
  assert.match(K.failure.message, /synthetic failure/);
}

/* Next after a forget starts at the first hit not shown, so nothing is skipped */
async function forgetThenNext() {
  const { K, node, requests, click, settle, shown, turnTo } = await pagedFixture();
  click(10, { '[data-forget]': {} }); await settle();
  assert.equal(shown().length, 49);
  assert.match(node('v-pos').innerHTML, /^<b>1 to 49<\/b> of 119 for “q”$/);
  await turnTo('next', 49, rows(51, 99));
  assert.equal(K.views.view().offset, 49);
  assert.equal(requests.length, 0);
}

/* Undo after navigation ranks the current view again, including the restored page and its count. */

/* Undo after navigation ranks the current view again, including the restored page and its count. */
async function forgetUndoAfterNavigation() {
  for (const navigation of ['search', 'next']) {
    const { K, node, requests, click, settle, shown, turnTo, next } = await pagedFixture();
    click(10, { '[data-forget]': {} }); await settle();
    const undo = K.undo;
    if (navigation === 'next') await turnTo('next', 49, rows(51, 99));
    else {
      node('s-q').value = 'another query';
      const searching = K.views.run();
      await turn();
      requests.shift().resolve({ hits: rows(1, 51).filter((p) => p.row !== 10), offset: 0, total: 119 });
      await searching;
    }
    const offset = K.views.view().offset;
    undo();
    await turn();
    next('/api/restore').resolve({ urls: [url(10)], restored: [url(10)], counts: {} });
    await turn(); await turn();
    const search = next('api/search');
    assert.equal(search.body.offset, offset, 'Undo after navigation must rank the current view again, on its page');
    const hits = rows(offset + 1, offset + 50);
    search.resolve({ hits, offset, total: 120 });
    await turn(); await turn();
    assert.deepEqual(shown(), hits.map((p) => p.row));
    assert.match(node('v-pos').innerHTML, /of 120 for/, 'the total includes the restored page');
  }
}

/* Startup finishing during Next must not replay the submitted search. */
async function startupDuringPage() {
  const requests = [];
  const { K, node } = setup({ api: (route, body) => new Promise((resolve) => requests.push({ route, body, resolve })) },
    'pages.js', 'views.js', 'strip.js', 'app.js');
  await turn();
  requests.shift().resolve({ sizes: [20, 50], images: true, pages: 120, app_tags: [], forgotten: [] });
  await turn();
  assert.equal(requests[0].route, 'api/selection');
  node('s-q').value = 'q';
  const searching = K.views.run();
  await turn();
  requests.splice(1, 1)[0].resolve({ hits: rows(1, 50), offset: 0, total: 120 });
  await searching;
  node('next').click();
  await turn();
  requests.shift().resolve({ pages: [] });
  await turn();
  requests.splice(1, 1)[0].resolve({ pages: [] });
  await turn(); await turn();
  assert.equal(requests.length, 1, 'startup must not submit a second search that cancels Next');
  assert.equal(requests[0].body.offset, 50);
  requests.shift().resolve({ hits: rows(51, 100), offset: 50, total: 120 });
  await turn(); await turn();
  assert.equal(K.views.view().offset, 50, 'Next finishes on page two');
}

/* Pin parks a page without touching the selection; Pinned shows them, and leaving returns to the results */

/* Pin parks a page without touching the selection; Pinned shows them, and leaving returns to the results */
async function pinView() {
  const { K, node, requests, click, key, settle, context, history, shown } = await screenFixture();
  const e = click(2, { '[data-pin]': {} });
  assert.equal(e.stopped, false);
  assert.deepEqual(plain(K.sel), [], 'a pin click never selects');
  assert.deepEqual(plain(K.pins), [2]);
  assert.match(node('grid').innerHTML, /class="hit tile pinned" data-row="2"/);
  assert.match(node('grid').innerHTML, /data-pin aria-pressed="true"/);
  assert.equal(node('n-pin').textContent, 1);
  await turn();
  assert.deepEqual([requests[0].route, requests[0].body], ['api/pins', { rows: [2] }], 'the pins are kept by the server');
  await settle();
  click(1);
  assert.equal(key(3, 'p'), true, 'p pins the focused page');
  assert.deepEqual(plain(K.sel), [1], 'and selects nothing');
  assert.deepEqual(plain(K.pins), [2, 3]);
  await settle();
  context.scrollY = 640;
  node('show-pins').click();
  assert.deepEqual(shown(), [2, 3], 'Pinned shows the pinned pages');
  assert.match(node('v-pos').innerHTML, /^Pinned · <b>2<\/b> pages$/);
  assert.equal(context.scrollY, 0);
  assert.equal(node('pager').hidden, true);
  assert.equal(key(2, 'p'), true);
  assert.deepEqual(plain(K.pins), [3], 'unpinned in the view');
  assert.deepEqual(shown(), [2, 3], 'and still shown, to pin again');
  assert.equal(key(null, 'Escape'), true);
  await turn(); await turn();
  assert.deepEqual(shown(), [1, 2, 3], 'Esc returns to the results');
  assert.equal(context.scrollY, 640, 'at the scroll it left');
  assert.deepEqual(plain(K.sel), [1], 'with the selection kept');
  assert.equal(history.entries.length, 1);
  node('show-pins').click(); node('show-pins').click();
  await turn(); await turn();
  assert.deepEqual(shown(), [1, 2, 3], 'Pinned again leaves the view');
}

async function untagAndOpenDoNotSelect() {
  const { K, node, requests, click, key, opened, settle } = await screenFixture();
  const e = click(1, { '[data-untag]': { dataset: { untag: 'A' } } });
  assert.deepEqual(plain(K.sel), [], 'a tag chip × does not select');
  await turn();
  assert.deepEqual(requests[0].body, { urls: [url(1)], add: [], remove: ['A'] }, '× takes the tag off that page only');
  await settle();
  assert.doesNotMatch(node('grid').innerHTML, /data-untag="A"/, 'the tag leaves the tile once the library answers');
  const open = click(2, { '[data-open]': {}, 'a, button': {} });
  assert.equal(open.stopped, true, 'a click on Open stops at the control');
  assert.deepEqual(plain(K.sel), [], 'Open does not select');
  assert.equal(key(3, 'o'), true);
  assert.equal(opened.clicks, 1, 'o opens the focused page');
  assert.deepEqual(plain(K.sel), [], 'o does not select');
  await turn();
  assert.equal(requests.length, 0, 'opening writes nothing');
  assert.equal(e.stopped, false);
}

async function likeToggle() {
  const { K, node, requests, click, chip, key, settle, history, context } = await screenFixture();
  click(2); await settle();
  const enter = async (scroll) => {
    context.scrollY = scroll;
    chip('A', 'like');
    assert.equal(context.scrollY, 0, 'the ≈ view starts at the top');
    await turn();
    assert.deepEqual(requests[0].body, { tag: 'A', offset: 0, n: 50, images: false });
    requests.shift().resolve({ tag: 'A', hits: [page(7), page(8)], offset: 0, total: 2 });
    await turn(); await turn();
    assert.match(node('grid').innerHTML, /data-row="7"[^]*data-row="8"/);
    assert.doesNotMatch(node('grid').innerHTML, /data-row="[123]"/);
    assert.match(node('v-pos').innerHTML, /Pages like <b>A<\/b>, not tagged <b>A<\/b>/, 'the view is named');
    assert.match(node('strip').innerHTML, /data-act="like" aria-pressed="true" title="Back to your search/, '≈ shows as active');
  };
  const back = async (scroll, how) => {
    await turn(); await turn();
    assert.match(node('grid').innerHTML, /data-row="1"[^]*data-row="2"[^]*data-row="3"/, `${how} returns to the search's results`);
    assert.equal(context.scrollY, scroll, `${how} restores the scroll position`);
    assert.equal(history.entries.length, 1, `${how} leaves no ≈ entry behind`);
    assert.match(node('grid').innerHTML, /class="hit tile sel" data-row="2"/, `${how} keeps the selection`);
  };
  await enter(480);
  assert.equal(history.pushes, 1, '≈ is a history entry, so Back leaves it');
  click(7); chip('A', 'add'); await settle();
  assert.match(node('grid').innerHTML, /data-row="7"/, 'a page tagged in the view stays until you leave it');
  chip('A', 'like');
  await back(480, '≈ again');
  assert.deepEqual(plain(K.sel), [2, 7]);
  await enter(300);
  assert.equal(key(null, 'Escape'), true);
  await back(300, 'Esc');
  assert.deepEqual(plain(K.sel), [2, 7], 'Esc leaves ≈ before it clears anything');
  await enter(120);
  history.back();   // the browser's Back
  await back(120, 'Back');
  assert.equal(key(null, 'Escape'), true);
  assert.deepEqual(plain(K.sel), [], 'with no ≈ open, Esc clears the selection');
  assert.equal(key(null, 'Escape'), false, 'nothing left for Esc');
}

async function newTagApplies() {
  const { node, requests, click, settle, next, library, answer } = await screenFixture();
  library.vocabulary.add('Night trains');
  const submit = (name) => { node('new-tag').value = name; node('newtag').submit({ preventDefault() {} }); };
  submit('Night trains');
  await turn();
  assert.equal(requests.length, 0, 'no selection, no tag made');
  assert.match(node('new-msg').textContent, /Select pages first/);
  click(2); click(3); await settle();
  submit('  night  TRAINS ');
  await turn();
  const made = next('/api/vocabulary');
  assert.deepEqual(made.body, { create: ['night  TRAINS'] }, 'the name goes trimmed, for the library to spell');
  made.resolve({ vocabulary: [{ name: 'A' }, { name: 'Night trains' }], resolved: ['Night trains'] });
  await turn(); await turn(); await turn();
  const tag = next('/api/tags');
  assert.deepEqual(tag.body, { urls: [url(2), url(3)], add: ['Night trains'], remove: [] }, "it goes on every selected page, in the library's spelling");
  assert.equal(node('new-tag').value, '', 'the box empties for the next one');
  tag.resolve(answer(tag));
  await settle();
  submit('a');
  await turn();
  next('/api/vocabulary').reject(new Error('cannot use "a" as a tag name: it is empty'));
  await turn(); await turn();
  assert.match(node('new-msg').textContent, /cannot use "a" as a tag name/, 'a refusal shows inline');
  assert.equal(node('new-tag').value, 'a', 'the name stays to fix');
}

/* the shell's keys: a ticked box keeps them; a text box does not */
async function keysBesideCheckbox() {
  const { K, keys } = setup('app.js');
  await turn();
  const seen = [];
  K.views.key = (e) => { seen.push(e.key); return true; };
  const element = (...matches) => ({ closest: (s) => (s.split(',').some((part) => matches.includes(part.trim())) ? {} : null) });
  const press = (target, key = 'Escape') => keys.keydown({ key, target, preventDefault() {} });
  press(element('input', 'input[type=checkbox]'));
  assert.deepEqual(seen, ['Escape'], 'Esc on a focused checkbox reaches the views');
  press(element('input', 'input:not([type=checkbox])'), 'x');
  press(element('textarea'), '/');
  assert.deepEqual(seen, ['Escape'], 'keys typed into a text box stay there');
}

async function escapeFromTextInputs() {
  const { K, node, keys, click, chip, settle } = await screenFixture('app.js');
  click(2);
  chip('A', 'like');
  await settle();
  const typing = (id) => Object.assign(node(id), { blurs: 0, blur() { this.blurs++; }, closest: (s) => (s.startsWith('input') ? {} : null) });
  const press = (target) => {   // the box's own listener, then the document's, as the event bubbles
    const e = { key: 'Escape', target, preventDefault() {} };
    if (target.keydown) target.keydown(e);
    keys.keydown(e);
  };
  const box = typing('new-tag');
  box.value = 'Half typed';
  press(box);
  await turn();
  assert.equal(box.value, '', 'Esc in + New tag cancels it');
  assert.equal(box.blurs, 1, 'and leaves it');
  assert.match(node('v-pos').innerHTML, /Pages like <b>A<\/b>/, 'and stays in the like view');
  assert.deepEqual(plain(K.sel), [2], 'and keeps the selection');
  press(typing('s-q'));
  await turn();
  assert.equal(node('s-q').blurs, 1, 'Esc in the search box leaves it');
  assert.match(node('v-pos').innerHTML, /Pages like <b>A<\/b>/, 'and stays in the like view');
  assert.deepEqual(plain(K.sel), [2], 'and keeps the selection');
  const page = { closest: () => null };
  press(page);
  await turn();
  assert.doesNotMatch(node('v-pos').innerHTML, /Pages like/, 'the next Esc leaves the like view');
  assert.deepEqual(plain(K.sel), [2]);
  press(page);
  assert.deepEqual(plain(K.sel), [], 'and the one after clears the selection');
}

async function likeNaming() {
  const { K, node, requests, chip } = await screenFixture();
  assert.match(node('strip').innerHTML, /Find pages like A, not tagged A/);
  chip('A', 'like');
  assert.match(node('v-pos').innerHTML, /Finding pages like <b>A<\/b>, not tagged <b>A<\/b>/);
  await turn();
  requests.shift().resolve({ tag: 'A', hits: [], offset: 0, total: 0 });
  await turn();
  assert.match(node('v-pos').innerHTML, /Pages like <b>A<\/b>, not tagged <b>A<\/b> · <b>0<\/b>/);
  assert.match(node('grid').innerHTML, /No pages like A without that tag are left/);
}

/* ---- pages of hits: Previous and Next replace them; the search "q" ranks 120 pages, 50 at a time ---- */

/* ---- pages of hits: Previous and Next replace them; the search "q" ranks 120 pages, 50 at a time ---- */
const rows = (from, to) => Array.from({ length: to - from + 1 }, (_, i) => page(from + i));
async function pagedFixture() {
  const f = await screenFixture();
  const { K, node, requests, context } = f;
  K.views.run();
  await turn();
  requests.shift().resolve({ hits: rows(1, 50), offset: 0, total: 120 });
  await turn(); await turn();
  f.shown = () => [...node('grid').innerHTML.matchAll(/data-row="(\d+)"/g)].map((m) => Number(m[1]));
  f.turnTo = async (button, offset, hits, scroll = 900) => {   // press Previous or Next from `scroll` down; the server answers
    context.scrollY = scroll;
    node(button).click();
    assert.equal(node('prev').disabled && node('next').disabled, true, 'both wait for the page');
    await turn();
    assert.deepEqual(requests[0].body, { query: 'q', untagged: false, offset, n: 50, images: false });
    requests.shift().resolve({ hits, offset, total: 120 });
    await turn(); await turn();
  };
  return f;
}

async function pages() {
  const { K, node, click, settle, context, shown, turnTo } = await pagedFixture();
  assert.deepEqual(shown(), rows(1, 50).map((p) => p.row));
  assert.match(node('v-pos').innerHTML, /^<b>1 to 50<\/b> of 120 for “q”$/, 'the position: which hits, of how many');
  assert.equal(node('pager').hidden, false);
  assert.equal(node('prev').disabled, true, 'no Previous on the first page');
  assert.equal(node('next').disabled, false);
  click(2); await settle();
  await turnTo('next', 50, rows(51, 100));
  assert.deepEqual(shown(), rows(51, 100).map((p) => p.row), 'Next replaces the hits');
  assert.equal(context.scrollY, 0, 'and shows them from the top');
  assert.match(node('v-pos').innerHTML, /^<b>51 to 100<\/b> of 120 for “q”$/);
  assert.deepEqual(plain(K.sel), [2], 'the selection stays');
  assert.equal(node('n-sel').textContent, 1);
  click(60); await settle();
  assert.match(node('grid').innerHTML, /class="hit tile sel" data-row="60"/);
  await turnTo('next', 100, rows(101, 120));
  assert.match(node('v-pos').innerHTML, /^<b>101 to 120<\/b> of 120 for “q”$/);
  assert.equal(node('next').disabled, true, 'no Next on the last page');
  assert.equal(node('prev').disabled, false);
  await turnTo('prev', 50, rows(51, 100), 400);
  assert.deepEqual(shown(), rows(51, 100).map((p) => p.row), 'Previous goes back');
  assert.equal(context.scrollY, 0);
  await turnTo('prev', 0, rows(1, 50));
  assert.match(node('grid').innerHTML, /class="hit tile sel" data-row="2"/, 'a page selected before shows selected');
  assert.deepEqual(plain(K.sel), [2, 60]);
  assert.equal(K.state.search.offset, 0, 'a reload comes back to this page');
}

/* Clearing the search box lists every page, newest first, and pages through them as a search does. */

/* Clearing the search box lists every page, newest first, and pages through them as a search does. */
async function clearShowsAll() {
  const { K, node, requests, click, settle, shown } = await screenFixture();
  click(2); await settle();
  node('s-q').value = '';
  node('s-q').input();
  await turn();
  assert.deepEqual(requests[0].body, { query: '', untagged: false, offset: 0, n: 50, images: false }, 'cleared: every page');
  requests.shift().resolve({ hits: rows(1, 50), offset: 0, total: 120 });
  await turn(); await turn();
  assert.deepEqual(shown(), rows(1, 50).map((p) => p.row));
  assert.match(node('v-pos').innerHTML, /^<b>1 to 50<\/b> of 120 in your library · newest first$/, 'the heading names no query');
  assert.equal(node('next').disabled, false);
  node('next').click();
  await turn();
  assert.deepEqual(requests[0].body, { query: '', untagged: false, offset: 50, n: 50, images: false });
  requests.shift().resolve({ hits: rows(51, 100), offset: 50, total: 120 });
  await turn(); await turn();
  assert.match(node('v-pos').innerHTML, /^<b>51 to 100<\/b> of 120 in your library · newest first$/);
  assert.deepEqual(plain(K.sel), [2], 'the selection stays');
  node('s-q').input();
  assert.equal(requests.length, 0, 'clearing an empty box asks again for nothing');
}

/* Searching for nothing, or only spaces, lists every page too. */

/* Searching for nothing, or only spaces, lists every page too. */
async function emptySearchShowsAll() {
  const { K, node, requests } = await screenFixture();
  node('s-q').value = '  ';
  node('s-q').blur = () => {};
  node('s-form').submit({ preventDefault() {} });
  await turn();
  assert.deepEqual(requests[0].body, { query: '', untagged: false, offset: 0, n: 50, images: false });
  requests.shift().resolve({ hits: [page(4), page(5)], offset: 0, total: 2 });
  await turn(); await turn();
  assert.equal(K.state.search.query, '');
  assert.match(node('v-pos').innerHTML, /^<b>1 to 2<\/b> of 2 in your library · newest first$/);
}

async function restorePage() {
  const requests = [];
  const { K, node } = setup('pages.js', 'views.js', 'strip.js');
  K.api = (route, body) => new Promise((resolve) => requests.push({ route, body, resolve }));
  Object.assign(K.state.search, { query: 'q', offset: 50 });
  node('s-q').value = '';
  K.views.restore();
  await turn();
  assert.equal(node('s-q').value, 'q');
  assert.deepEqual(plain(requests[0].body), { query: 'q', untagged: false, offset: 50, n: 50, images: false }, 'a reload asks for the page it left');
}

async function likeBackToPage() {
  const { node, requests, chip, key, history, context, shown, turnTo } = await pagedFixture();
  await turnTo('next', 50, rows(51, 100));
  const away = async (scroll) => {
    context.scrollY = scroll;
    chip('A', 'like');
    await turn();
    requests.shift().resolve({ tag: 'A', hits: [page(7), page(8)], offset: 0, total: 2 });
    await turn(); await turn();
    assert.deepEqual(shown(), [7, 8]);
    assert.equal(node('pager').hidden, true, 'one page of ≈: no pager');
  };
  const back = async (how) => {
    await turn(); await turn();
    assert.deepEqual(shown(), rows(51, 100).map((p) => p.row), `${how} returns to the same page`);
    assert.match(node('v-pos').innerHTML, /^<b>51 to 100<\/b> of 120/, `${how} keeps its position`);
    assert.equal(node('pager').hidden, false);
    assert.equal(node('prev').disabled, false);
    assert.equal(context.scrollY, 700, `${how} restores the scroll`);
  };
  await away(700); chip('A', 'like'); await back('≈ again');
  await away(700); assert.equal(key(null, 'Escape'), true); await back('Esc');
  await away(700); history.back(); await back('Back');
  assert.equal(requests.length, 0, 'going back asks the server for nothing');
}

async function likeWaitsForTag() {
  const { K, requests, click, chip, settle, next, library } = await screenFixture();
  click(2); await settle();
  chip('A', 'add');
  chip('A', 'like');
  await turn();
  assert.equal(requests.length, 1, 'the ranking waits for the pending library write');
  const tag = next('/api/tags');
  tag.resolve(library['/api/tags'](tag.body));
  await turn(); await turn();
  next('api/like').resolve({ hits: [page(3)], offset: 0, total: 1 });
  await turn();
  assert.deepEqual(plain(K.views.view().rows), [3], 'the newly tagged page is absent from the ranking');
  assert.equal(K.has(2, 'A'), true);
}

async function backDuringPage() {
  const { K, node, requests, history, shown, turnTo } = await pagedFixture();
  await turnTo('next', 50, rows(51, 100));
  node('next').click();
  await turn();
  history.back();
  await turn();
  assert.deepEqual(shown(), rows(1, 50).map((p) => p.row));
  requests.shift().resolve({ hits: rows(101, 120), offset: 100, total: 120 });
  await turn();
  assert.equal(K.state.search.offset, 0, 'an abandoned page reply must not change the page saved for reload');
}

/* Startup GETs snapshot the saved lists, but their replies are held. Writes replace whole lists and forgetting
   prunes both lists, as server.py does; forgotten rows cannot be selected or pinned. */

/* Startup GETs snapshot the saved lists, but their replies are held. Writes replace whole lists; the library's
   forget and restore leave them as they are, the forgotten page out of sight. */
function startupFixture(saved = [1]) {
  const f = setup('pages.js', 'views.js', 'strip.js');
  const { K } = f;
  const server = { selection: [...saved], pins: [...saved] }, loaded = new Set(), late = [], sent = [];
  const lib = library();
  K.lib = { pages: 9, sizes: [20, 50], app_tags: ['A'], forgotten: [] };
  K.state.search.query = '';
  K.know([page(1), page(2), page(3)]);
  K.changed = () => { K.views.render(); K.strip.render(); };
  K.api = (route, body) => {
    if (lib[route]) { sent.push([route, plain(body)]); return Promise.resolve(lib[route](body)); }
    const name = route.replace('api/', '');
    if (name === 'library') {
      const forgotten = [...lib.gone].map((key) => Number(key.split('/').pop()));
      return Promise.resolve({ ...K.lib, pages: 9 - forgotten.length, forgotten });
    }
    if (body === undefined) {
      const pages = server[name].map((row) => page(row, [...(lib.held.get(url(row)) || [])]));
      return new Promise((resolve) => late.push(() => { loaded.add(name); resolve({ pages }); }));
    }
    sent.push([name, plain(body)]);
    server[name] = [...body.rows];
    return Promise.resolve({ rows: body.rows });
  };
  f.finish = async () => {
    while (late.length) { late.shift()(); await turn(); }
    await K.writes;
    await K.mutations;
    await turn(); await turn();
  };
  return Object.assign(f, { server, loaded, late, sent });
}

async function startupEdits() {
  for (const [name, actions, expected] of [
    ['selection', (K) => { K.select(2); K.clearSel(); K.select(3); }, [3]],
    ['selection', (K) => { K.select(2); K.select(2); }, [1]],
    ['pins', (K) => { K.pin(2); K.pin(2); }, [1]],
  ]) {
    for (const duringPins of name === 'pins' ? [false, true] : [false]) {
      const { K, server, late, sent, finish } = startupFixture();
      const loading = K.loadLists();
      if (duringPins && name === 'pins') { late.shift()(); await turn(); }
      actions(K);
      await turn();
      assert.deepEqual(sent, [], 'early edits send nothing');
      await finish(); await loading;
      assert.deepEqual(server[name], expected, 'ordered edits replay on the saved list');
      assert.deepEqual(plain(name === 'selection' ? K.sel : K.pins), expected, 'late replies keep decisions');
      assert.deepEqual(sent, [[name, { rows: expected }]], 'one merged write, with the untouched list unsent');
    }
  }
}

async function startupForgetAndUndo() {
  for (const undoAt of ['never', 'early', 'loaded']) {
    const { K, server, sent, finish } = startupFixture([1, 2, 3]);
    const loading = K.loadLists();
    const forgotten = K.forget(2);
    if (undoAt === 'early') { await forgotten; K.undo(); }
    await turn();
    await finish(); await loading;
    if (undoAt === 'loaded') { K.undo(); await K.mutations; await turn(); await turn(); }
    const shown = undoAt === 'never' ? [1, 3] : [1, 2, 3];
    assert.deepEqual(plain(K.sel), [1, 2, 3], `${undoAt}: the selection keeps its saved membership and order`);
    assert.deepEqual(plain(K.pins), [1, 2, 3], `${undoAt}: the pins too`);
    assert.deepEqual(plain(K.selected()), shown, `${undoAt}: the forgotten page counted nowhere`);
    assert.deepEqual(server, { selection: [1, 2, 3], pins: [1, 2, 3] }, `${undoAt}: persisted lists untouched`);
    assert.deepEqual(sent.map(([name]) => name), undoAt === 'never' ? ['/api/forget'] : ['/api/forget', '/api/restore'], `${undoAt}: no list write`);
    assert.equal(K.failure, undefined, `${undoAt}: no rejected writes`);
    K.views.showPinned();
    assert.deepEqual(plain(K.views.shown(K.views.view())), shown, `${undoAt}: Pinned shows what is not forgotten`);
  }
}

async function startupTagsKeepChanges() {
  const { K, server, finish } = startupFixture();
  server.pins = [2];   // the later pins reply cannot repair page 1's tags after the stale selection reply
  const loading = K.loadLists();
  K.select(1);
  K.pin(1);
  await K.tagSelection('A');
  assert.equal(K.has(1, 'A'), true);
  await finish(); await loading;
  assert.equal(K.has(1, 'A'), true, 'startup replies cannot replace newer page tags');
}

/* The server keeps the selection [1] and the pins [1] and replaces a list whole on a write. The user selects and
   pins page 2 before the startup load of that list is in (pins: before, or while its load is out): the server
   keeps both pages, and no list is sent before its load is in. */
async function startupListsKeepChanges() {
  for (const pinWhileLoading of [false, true]) {
    const { K, server, loaded, late, sent, finish } = startupFixture();
    const loading = K.loadLists();
    K.select(2);
    if (pinWhileLoading) { late.shift()(); await turn(); }
    K.pin(2);
    await turn();
    assert.ok(sent.every(([name]) => loaded.has(name)), 'no list is sent before its startup load');
    await finish(); await loading;
    assert.deepEqual(sent, [['selection', { rows: [1, 2] }], ['pins', { rows: [1, 2] }]], 'each list sends once');
    assert.deepEqual(server, { selection: [1, 2], pins: [1, 2] }, 'the server keeps both');
    assert.deepEqual({ sel: plain(K.sel), pins: plain(K.pins) }, { sel: [1, 2], pins: [1, 2] }, 'the page keeps both');
    K.select(3);
    await K.writes;
    assert.deepEqual(server.selection, [1, 2, 3]);
  }
}

/* the service stops answering: one line says so for good, both screens and Send stop taking actions, and
   nothing is sent again; a refusal the service itself makes is not an outage */

/* the service stops answering: one line says so for good, both screens stop taking actions, and nothing is sent
   again, the library's writes included; a refusal the service itself makes is not an outage */
async function outage() {
  const reply = (status, body) => () => Promise.resolve({ status, ok: status < 300, json: async () => body });
  {
    const { K, context } = setup();
    context.fetch = reply(404, { error: 'not found' });
    await assert.rejects(K.api('api/add/9'), /not found/);
    assert.equal(K.down, false, 'a refusal is not an outage');
  }
  const failures = {
    503: reply(503, { error: 'the model service is not answering' }),
    502: reply(502, { error: 'the model service did not prove its answer' }),
    504: reply(504, { error: 'the model service took too long' }),
    gone: () => Promise.reject(new TypeError('Failed to fetch')),
  };
  for (const [name, failure] of Object.entries(failures)) {
    const { K, node, context, coreFail } = setup();
    const sent = [];
    context.fetch = (path, init) => { sent.push([path, init.method]); return failure(); };
    await assert.rejects(K.api('api/search', { query: 'q' }));
    assert.equal(K.down, true, name);
    assert.equal(node('outage').hidden, false, `${name}: the line shows`);
    assert.equal(node('tag-screen').inert && node('add-screen').inert, true, `${name}: no control acts`);
    assert.equal(node('toast-undo').disabled, true, `${name}: no Undo outside the inert screens`);
    await assert.rejects(K.shell('tags', { urls: [], undo: { tags: [], vocabulary: {} } }), /Reload/);
    await turn(); await turn();
    assert.deepEqual(plain(sent), [['api/search', 'POST']], `${name}: sent once, never again`);
    coreFail(new Error('the model service is not answering'));
    assert.equal(K.toasted, undefined, `${name}: the line says it, not a toast as well`);
  }
}

run({ clearShowsAll, emptySearchShowsAll, tagRecent, writesNeverWaitOnTheService, searchKeepsNewerTags, writesAreTheLibrarys, tagPageUndo, startupEdits, startupForgetAndUndo, startupTagsKeepChanges, startupListsKeepChanges, forgetUndoAfterNavigation, startupDuringPage, chipActsOnSelection, bulkUndo, forgetAndUndo, forgetThenNext, pinView, backDuringPage, likeWaitsForTag, pages, restorePage, likeBackToPage, escapeFromTextInputs, likeNaming, keysBesideCheckbox, select, untagAndOpenDoNotSelect, likeToggle, newTagApplies, openControl, outage });
