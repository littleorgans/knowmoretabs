// Run the shipped scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function setup(...files) {
  const nodes = new Map();
  const node = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: '', checked: false, disabled: false, innerHTML: '', textContent: '',
      dataset: {}, classList: { add() {}, remove() {}, toggle() {} },
      addEventListener(event, fn) { this[event] = fn; },
      querySelectorAll() { return []; }, querySelector() {
        if (id === 't-sugg') return node('new-tag-control');
        return id === 'r-acts' ? node('action') : null;
      },
      getBoundingClientRect() { return {}; },
      focus() { this.focusCalls = (this.focusCalls || 0) + 1; }, setAttribute() {}, removeAttribute() {}, showModal() {},
    });
    return nodes.get(id);
  };
  const K = {};
  const mocks = {
    $: node, esc: String, short: String, title: () => '', pct: String,
    own: () => '', thumb: () => '', save() {}, changed() {},
    fail(err) { this.failure = err; }, toast(msg) { this.toasted = msg; }, flyIn() {}, flyOut(c, d, done) { done(); }, refreshSets() {},
    state: { search: { query: 'old', images: false, n: 20, picked: ['A'] }, mode: 'grid' },
    lib: { sizes: [20, 50], images: true }, results: { hits: [{ row: 9 }] },
    search: { render() {}, restore: async () => {} }, pick: { render() {} }, review: { render() {}, left: () => 0 },
    views: { render() {}, key: () => false, restore: async () => {} }, strip: { render() {} },
  };
  const listeners = {}, keys = {};   // the window's and the document's, by event
  const history = {       // one tab's entries; back() delivers popstate a turn later, as browsers do
    entries: [null], pushes: 0,
    get state() { return this.entries[this.entries.length - 1]; },
    pushState(state) { this.entries.push(state); this.pushes++; },
    replaceState(state) { this.entries[this.entries.length - 1] = state; },
    back() { if (this.entries.length > 1) this.entries.pop(); setImmediate(() => listeners.popstate && listeners.popstate({ state: this.state })); },
  };
  const context = vm.createContext({
    window: { KMT: K }, document: { activeElement: null, documentElement: node('root'), querySelector() { return null; }, addEventListener(event, fn) { keys[event] = fn; } },
    location: { hash: '' }, addEventListener(event, fn) { listeners[event] = fn; }, matchMedia: () => ({ matches: false }),
    history, scrollY: 0, setTimeout, clearTimeout, console, performance, URL,
  });
  context.scrollTo = (x, y) => { context.scrollY = y; };
  for (const file of ['core.js', ...files]) {   // the real open helpers, everything else mocked
    vm.runInContext(fs.readFileSync(path.join(__dirname, '../tagger/app/static', file), 'utf8'), context);
    if (file === 'core.js') Object.assign(K, mocks);
  }
  return { K, node, history, context, keys };
}
const turn = () => new Promise((resolve) => setImmediate(resolve));
const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made inside the scripts' realm

async function search() {
  const { K, node } = setup('search.js');
  const requests = [];
  K.api = () => new Promise((resolve) => requests.push(resolve));
  node('s-q').value = 'first';
  const first = K.search.run();
  assert.equal(K.results, null, 'pending search must hide old results');
  K.search.render();
  assert.equal(node('s-tag').disabled, true);
  await turn();
  node('s-q').value = 'second';
  const second = K.search.run();
  await turn();
  requests[1]({ hits: [{ row: 2 }] });
  await second;
  requests[0]({ hits: [{ row: 1 }] });
  await first;
  assert.equal(K.results.hits[0].row, 2, 'stale search reply must not replace current results');
}

function resultsFixture(...files) {
  const { K, node } = setup(...files);
  K.state.search.query = 'q';
  K.results = { hits: [1, 2, 3].map((row) => ({ row, fused: 0.1, sources: {}, own: [] })), excluded: [], cut: null, suggested: [], ms: {} };
  const requests = [];
  K.api = (route, body) => new Promise((resolve) => requests.push({ route, body, resolve }));
  const tile = (row) => ({ dataset: { row: String(row) } });
  const click = (row, act) => node('view-search').click({ target: { closest(selector) {
    if (selector === '[data-act]') return act ? { dataset: { act }, disabled: false, closest: () => tile(row) } : null;
    if (selector === '.hit[data-row]') return tile(row);
    return null;
  } } });
  const key = (row, k) => K.search.key({ key: k, target: { closest(selector) { return selector === '.hit[data-row]' ? tile(row) : null; } } });
  return { K, node, requests, click, key };
}

async function restore() {
  const { K, node } = setup('search.js');
  Object.assign(K.state.search, { query: 'q', images: true, n: 50, refine: true, picked: ['A'] });
  const sent = [];
  K.api = async (route, body) => { sent.push(body); return { hits: [], excluded: [], cut: null, suggested: [], ms: {} }; };
  await K.search.restore();
  assert.deepEqual(plain(sent), [{ query: 'q', images: true, n: 50, refine: true }], 'a reload must search again as it was left');
  assert.deepEqual(plain(K.state.search.picked), ['A'], 'a reload must keep the picked tags');
}

async function exactQuery() {
  const { K, node } = setup('search.js');
  const query = ' q ';
  node('s-q').value = query;
  let sent;
  K.api = async (route, body) => { sent = body; return { hits: [], excluded: [], cut: null, suggested: [], ms: {} }; };
  await K.search.run();
  assert.equal(sent.query, query, 'search must preserve exact query text');
  assert.equal(K.state.search.query, query);
}

async function searchAfterCut() {
  const { K, node, requests, click } = resultsFixture('search.js');
  node('s-q').value = 'q';
  click(1, 'cut');
  await turn();
  const searching = K.search.more();
  await turn();
  assert.equal(requests.length, 1, 'Show more must wait for the pending cut');
  requests[0].resolve({ excluded: [2, 3], cut: 1, suggested: [] });
  await turn(); await turn();
  assert.equal(requests[1].route, '/api/search');
  assert.equal(requests[1].body.n, 50);
  requests[1].resolve({ hits: [], excluded: [], cut: null, suggested: [], ms: {} });
  await searching;
}

async function pickAfterCut() {
  const { K, node, requests, click } = resultsFixture('search.js', 'pick.js');
  click(1, 'cut');
  node('view-tag').click({ target: { closest(selector) { return selector === '[data-act]' ? { dataset: { act: 'review' }, disabled: false } : null; } } });
  await turn();
  assert.equal(requests.length, 1, 'review must wait for the pending cut');
  requests[0].resolve({ excluded: [2, 3], cut: 1, suggested: [] });
  await turn(); await turn();
  assert.equal(requests[1].route, '/api/sessions');
  assert.deepEqual(plain(requests[1].body.rows), [1]);
}

async function exclude() {
  const { K, node, requests, click, key } = resultsFixture('search.js');
  K.search.render();
  assert.match(node('s-pos').innerHTML, /<b>3<\/b> of 3 going to tagging/);
  click(2);
  assert.deepEqual(K.results.excluded, [2], 'a click must exclude the tile at once');
  K.search.render();
  assert.match(node('s-pos').innerHTML, /<b>2<\/b> of 3 going to tagging/);
  assert.match(node('s-grid').innerHTML, /class="hit out" data-row="2"/);
  assert.equal(key(2, 'x'), true, 'x on the focused tile must be handled');
  assert.deepEqual(K.results.excluded, [], 'x must bring the excluded tile back');
  await turn();
  assert.equal(requests.length, 1, 'exclusion requests must go one at a time');
  assert.deepEqual(plain(requests[0].body), { query: 'q', rows: [1, 2, 3], action: 'exclude', row: 2 });
  requests[0].resolve({ excluded: [2], cut: null, suggested: [] }); await turn(); await turn();
  assert.deepEqual(K.results.excluded, [], 'an earlier reply must not undo a later click');
  assert.equal(requests[1].body.action, 'include');
  requests[1].resolve({ excluded: [], cut: null, suggested: [{ tag: 'Kept only', z: 1 }] }); await turn(); await turn();
  assert.equal(K.results.suggested[0].tag, 'Kept only', 'suggestions must follow the kept results');
  for (const row of [1, 2, 3]) click(row);
  K.search.render();
  assert.equal(node('s-tag').disabled, true, 'nothing kept, nothing to tag');
}

async function cut() {
  const { K, requests, click } = resultsFixture('search.js');
  click(1, 'cut');
  await turn();
  assert.deepEqual(plain(requests[0].body), { query: 'q', rows: [1, 2, 3], action: 'cut', row: 1 });
  requests[0].resolve({ excluded: [2, 3], cut: 1, suggested: [] }); await turn(); await turn();
  assert.match(K.toasted, /Cut below #1: 1 of 3 going to tagging/);
  click(1, 'uncut');
  await turn(); await turn();
  assert.equal(requests[1].body.action, 'uncut');
}

async function pickIncluded() {
  const { K, node, requests } = resultsFixture('search.js', 'pick.js');
  K.results.excluded = [2];
  node('view-tag').click({ target: { closest(selector) { return selector === '[data-act]' ? { dataset: { act: 'review' }, disabled: false } : null; } } });
  await turn();
  assert.equal(requests[0].route, '/api/sessions');
  assert.deepEqual(plain(requests[0].body.rows), [1, 3], 'excluded results must never reach review');
}

/* ---- "+ New tag": type a name, Enter picks it, made new by the server only when you lack it ---- */
function newTagFixture() {
  const f = resultsFixture('search.js', 'pick.js');
  f.K.lib.tags = [{ name: 'Coffee', positives: 12, source: 'head' }, { name: 'Trains', positives: 11, source: 'head' }];
  f.K.results.suggested = [{ tag: 'Coffee', z: 1.5 }];
  f.K.state.search.picked = [];
  f.node('t-new').id = 't-new';
  f.K.pick.render();
  f.open = () => f.node('view-tag').click({ target: { closest(selector) {
    return selector === '[data-act]' ? { dataset: { act: 'new-tag' }, disabled: false } : null;
  } } });
  f.type = (value, key = 'Enter') => { f.node('t-new').value = value; f.node('view-tag').keydown({ key, preventDefault() {}, target: f.node('t-new') }); };
  f.row = () => f.node('t-sugg').innerHTML;
  return f;
}

async function newTag() {
  const { K, requests, open, type, row } = newTagFixture();
  assert.match(row(), /data-act="new-tag"><b>\+ New tag<\/b>/, 'the chip is always in the suggestions row');
  open();
  assert.match(row(), /<input id="t-new"/, 'a click opens an inline input');
  type('  Night trains ');
  await turn();
  assert.equal(requests[0].route, '/api/tags');
  assert.deepEqual(plain(requests[0].body), { name: 'Night trains' }, 'the name goes trimmed');
  requests[0].resolve({ tag: 'Night trains', created: true, tags: [...K.lib.tags, { name: 'Night trains', positives: 0, source: 'zero shot' }] });
  await turn(); await turn();
  assert.deepEqual(plain(K.state.search.picked), ['Night trains']);
  assert.match(row(), /data-tag="Night trains" aria-pressed="true"><b>Night trains<\/b><small>zero shot/, 'a filled chip in the row');
  assert.doesNotMatch(row(), /t-new/, 'the input closes');
  assert.match(row(), /\+ New tag/);
}

async function newTagExisting() {
  const { K, requests, open, type, row } = newTagFixture();
  open(); type('trains');
  open(); type(' COFFEE ');
  open(); type('Trains');
  await turn();
  assert.equal(requests.length, 0, 'a name you have makes no tag');
  assert.deepEqual(plain(K.state.search.picked), ['Trains', 'Coffee'], 'it picks your tag once, as you spell it');
  assert.match(row(), /data-tag="Coffee" aria-pressed="true"/);
  assert.match(row(), /data-tag="Trains" aria-pressed="true"/);
}

async function newTagCancelAndRefusal() {
  const { K, node, open, type, row } = newTagFixture();
  open(); type('Half typed', 'Escape');
  assert.doesNotMatch(row(), /t-new/, 'Escape cancels');
  open(); type('   ');
  assert.doesNotMatch(row(), /t-new/, 'an empty name cancels');
  K.api = async () => { throw new Error('cannot use that name as a tag: it is longer than 40 characters'); };
  open(); type('x'.repeat(41));
  await turn(); await turn();
  assert.match(row(), /<li class="msg" role="alert">cannot use that name as a tag: it is longer than 40 characters<\/li>/);
  assert.match(row(), new RegExp(`<input id="t-new"[^>]*value="${'x'.repeat(41)}"`), 'the name stays to fix');
  assert.deepEqual(plain(K.state.search.picked), [], 'nothing picked');
  assert.equal(node('t-filter').value, '', 'the filter box is untouched');
}

async function newTagKeyboardFocus() {
  const { K, node, requests, open, type } = newTagFixture();
  open(); type('Sleeper cars');
  requests[0].resolve({ tag: 'Sleeper cars', tags: [...K.lib.tags, { name: 'Sleeper cars', positives: 0, source: 'zero shot' }] });
  await turn(); await turn();
  assert.equal(node('new-tag-control').focusCalls, 1, 'Enter returns focus to + New tag');
  open(); type('COFFEE');
  assert.equal(node('new-tag-control').focusCalls, 2, 'picking an existing tag also returns focus');
  open(); type('Half typed', 'Escape');
  assert.equal(node('new-tag-control').focusCalls, 3, 'Escape returns focus to + New tag');
  open(); type('   ');
  assert.equal(node('new-tag-control').focusCalls, 4, 'empty Enter returns focus to + New tag');
}

function reviewFixture() {
  const { K, node } = setup('review.js');
  K.set = { id: 1, pages: [{ index: 0, status: 'open', sugg: [
    { tag: 'A', checked: true }, { tag: 'B', checked: true },
  ] }] };
  const requests = [];
  const saved = { A: true, B: true };
  K.api = (route, body) => new Promise((resolve) => requests.push(() => {
    Object.assign(saved, body.marks);
    resolve({ index: 0, status: body.status || 'open', sugg: Object.entries(saved).map(([tag, checked]) => ({ tag, checked })) });
  }));
  const flip = (tag) => node('view-review').click({ target: { closest(selector) {
    return selector === '.rcard .sg' ? { dataset: { tag }, closest() { return { dataset: { i: '0' } }; } } : null;
  } } });
  return { K, requests, saved, flip, node };
}

async function flips() {
  const { requests, saved, flip } = reviewFixture();
  flip('A'); flip('B');
  await turn();
  requests.shift()(); await turn();
  requests.shift()(); await turn();
  assert.deepEqual(saved, { A: false, B: false }, 'rapid flips must retain both changes');
}

async function switchedSet() {
  const { K, requests, flip } = reviewFixture();
  flip('A');
  await turn();
  const other = { id: 2, pages: [{ index: 0, status: 'open', sugg: [{ tag: 'A', checked: true }] }] };
  K.set = other;
  requests.shift()(); await turn();
  assert.equal(other.pages[0].sugg[0].checked, true, 'reply for earlier set must not mutate current set');
}

async function acceptAfterFlip() {
  const { K, requests, saved, flip } = reviewFixture();
  K.state.mode = 'swipe';
  flip('A');
  K.review.key({ key: 'ArrowRight', target: { closest() { return null; } } });
  await turn(); requests.shift()();
  await turn(); requests.shift()(); await turn();
  assert.equal(saved.A, false, 'accept must preserve a pending flip');
  assert.equal(K.set.pages[0].status, 'decided');
}

async function confirmAllPending() {
  const { K, node } = setup('review.js');
  K.set = { id: 1, pages: [0, 1].map((index) => ({ index, status: 'open', sugg: [{ tag: 'A', checked: true }] })) };
  const requests = [];
  K.api = (route, body) => new Promise((resolve) => requests.push(() => resolve({
    index: Number(route.split('/').pop()), status: body.status, sugg: [{ tag: 'A', checked: true }],
  })));
  node('view-review').click({ target: { closest(selector) {
    return selector === '[data-act]' ? { dataset: { act: 'confirm-all' }, disabled: false } : null;
  } } });
  let settled = false;
  K.review.pending.then(() => { settled = true; }); // what an export pressed now waits for
  await turn(); requests.shift()(); await turn(); await turn();
  assert.equal(settled, false, 'export must wait for every page Confirm all is saving');
  requests.shift()(); await turn(); await turn();
  assert.deepEqual(K.set.pages.map((p) => p.status), ['decided', 'decided']);
}

async function exportWait() {
  const { K } = setup('app.js');
  await turn();
  let finish, exports = 0;
  K.writes = new Promise((resolve) => { finish = resolve; });
  K.api = async () => { exports++; return { decided: 0, answers: '' }; };
  const exporting = K.exportNow();
  await turn();
  assert.equal(exports, 0, 'export must wait for pending saves');
  finish(); await exporting;
  assert.equal(exports, 1);
}

async function exportCommand() {
  const { K, node } = setup('app.js');
  // Startup's API is intentionally absent; its error is observed before export.
  await turn();
  K.api = async () => ({ decided: 1, flipped: 0, answer_tags: 1, answer_pages: 1, answers: "/tmp/owner's data/answers.jsonl", decisions: '/tmp/log', source: 'test' });
  await K.exportNow();
  const html = node('export-body').innerHTML;
  assert.ok(html.includes("--root '/path/to/archive-copy'"), 'import command must select a copy explicitly');
  assert.ok(html.includes("--import '/tmp/owner'\\''s data/answers.jsonl'"), 'answer path must be shell quoted');
  assert.ok(html.includes('--accept-new'), 'created tags must validate without an existing archive tag');
}

/* ---- open the page: a control on http(s) pages whose click and key change nothing ---- */
const control = () => ({ clicks: 0, click() { this.clicks++; } });
const clickOn = (a, at) => {   // a click on the open control inside the element `at` returns for that selector
  const e = { stopped: false, stopPropagation() { this.stopped = true; }, target: { closest(selector) {
    if (selector === '[data-open]' || selector === 'a, button') return a;
    return at[selector] || null;
  } } };
  return e;
};

async function openControl() {
  const { K } = setup();
  const html = K.open({ url: 'https://example.test/a?b=1', host: 'example.test' });
  assert.match(html, /data-open href="https:\/\/example.test\/a\?b=1" target="_blank" rel="noopener noreferrer"/);
  assert.match(K.open({ url: 'http://example.test/', host: 'example.test' }), /data-open/);
  for (const url of ['javascript:void(0)', 'file:///tmp/a', 'data:text/html,a', 'not a url', undefined]) {
    assert.equal(K.open({ url, host: 'x' }), '', `${url} must get no open control`);
  }
}

async function openSearch() {
  const { K, node, requests, key } = resultsFixture('search.js');
  const tile = { dataset: { row: '2' } };
  const e = clickOn(control(), { '.hit[data-row]': tile });
  node('view-search').click(e);
  assert.equal(e.stopped, true, 'a click on Open must stop at the control');
  assert.deepEqual(K.results.excluded, [], 'a click on Open must not exclude the result');
  const a = control();
  assert.equal(K.search.key({ key: 'o', target: { closest: (s) => (s === '.hit[data-row]' ? { ...tile, querySelector: () => a } : null) } }), true);
  assert.equal(a.clicks, 1, 'o must open the focused result');
  assert.deepEqual(K.results.excluded, [], 'o must not exclude the result');
  await turn();
  assert.equal(requests.length, 0, 'opening must send no exclusion');
  assert.equal(key(2, 'x'), true, 'the tile still excludes');
}

async function openReview() {
  const { K, node, requests } = reviewFixture();
  const row = { dataset: { i: '0' } };
  const e = clickOn(control(), { '[data-i]': row, '.rcard': row });
  node('view-review').click(e);
  assert.equal(e.stopped, true, 'a click on Open must stop at the control');
  K.state.mode = 'swipe';
  const a = control();
  node('r-deck').querySelector = () => ({ querySelector: () => a });
  assert.equal(K.review.key({ key: 'o', target: { closest() { return null; } } }), true);
  assert.equal(a.clicks, 1, 'o must open the swipe card');
  await turn();
  assert.equal(requests.length, 0, 'opening must not count as a decision');
  assert.equal(K.set.pages[0].status, 'open');
}

async function openNoDrag() {
  const { K } = setup('swipe.js');
  const card = { offsetWidth: 400, dataset: {}, style: { setProperty() {} }, classList: { add() {}, remove() {} },
    setPointerCapture() {}, addEventListener(event, fn) { this[event] = this[event] || fn; }, removeEventListener() {} };
  const decided = [];
  K.swipe(card, { enabled: () => true, commit: (dir) => decided.push(dir) });
  const on = (sel) => ({ closest: (s) => (s === sel ? {} : null) });
  const drag = (target) => {
    card.pointerdown({ button: 0, pointerId: 1, clientX: 0, clientY: 0, target });
    card.pointermove({ pointerId: 1, clientX: 300, clientY: 0 });
    card.pointerup({ pointerId: 1, type: 'pointerup' });
  };
  drag({ closest: (s) => (s.includes('[data-open]') ? {} : null) });
  assert.deepEqual(decided, [], 'a press on Open must never drag the card into a decision');
  drag(on('nothing'));
  assert.deepEqual(decided, ['yes'], 'the card itself still swipes');
}

/* ---- one screen: search, select, tag ---- */
const page = (row, tags = []) => ({ row, title: `Page ${row}`, host: 'example.test', url: `https://example.test/${row}`, image: false, tags });

/* the search "q" has found pages 1 (tagged A), 2 and 3 */
async function screenFixture() {
  const f = setup('pages.js', 'views.js', 'strip.js');
  const { K, node } = f;
  K.changed = () => { K.views.render(); K.strip.render(); };
  K.lib = { sizes: [20, 50], images: true, pages: 9, app_tags: ['A'] };
  const requests = [];
  K.api = (route, body) => new Promise((resolve, reject) => requests.push({ route, body: plain(body), resolve, reject }));
  node('s-q').value = 'q';
  const searching = K.views.run();
  requests.shift().resolve({ hits: [page(1, ['A']), page(2), page(3)] });
  await searching;
  const tile = (row) => ({ dataset: { row: String(row) }, querySelector: () => f.opened });
  f.requests = requests;
  f.opened = { clicks: 0, click() { this.clicks++; } };
  f.click = (row, at = {}) => {   // a click on tile `row`, on whatever `at` says is under it
    const e = { stopped: false, stopPropagation() { this.stopped = true; }, target: { closest: (s) => (s === '.tile[data-row]' ? tile(row) : at[s] || null) } };
    node('grid').click(e);
    return e;
  };
  f.key = (row, key) => K.views.key({ key, target: { closest: (s) => (s === '.tile[data-row]' && row !== null ? tile(row) : null) } });
  f.chip = (tag, act) => node('strip').click({ target: { closest: (s) => (s === 'button[data-act]' ? { dataset: { act }, closest: () => ({ dataset: { tag } }) } : null) } });
  f.settle = async () => { await turn(); while (requests.length) { const r = requests.shift(); r.resolve(reply(r)); await turn(); } };
  /* what the server answers, from what the page sent */
  const reply = (r) => (r.route === '/api/apply'
    ? { pages: r.body.rows.map((row) => ({ row, tags: K.pages.get(row).tags })), changed: r.body.rows.length, app_tags: ['A'] }
    : { rows: r.body.rows });
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
  assert.deepEqual(requests[0], { ...requests[0], route: '/api/selection', body: { rows: [2] } }, 'the selection is kept by the server');
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

async function tagToggle() {
  const { K, node, requests, click, chip, settle } = await screenFixture();
  chip('A', 'toggle');
  await turn();
  assert.equal(requests.length, 0, 'no selection, nothing tagged');
  assert.match(K.toasted, /Select pages first/);
  click(1); click(2); await settle();
  assert.match(node('strip').innerHTML, /<li class="tc" data-on="some" data-tag="A"><button type="button" class="nm" data-act="toggle" aria-pressed="mixed"/, 'some selected have it: partial');
  assert.match(node('strip').innerHTML, />1\/3<\/button>/, 'the count: pages in view with it, of all in view');
  chip('A', 'toggle');
  assert.match(node('strip').innerHTML, /data-on="all"/, 'the chip fills at once');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [2], tag: 'A', value: true }, 'partial: only the pages lacking it are tagged');
  await settle();
  assert.match(K.toasted, /Tagged 1 page “A”/);
  chip('A', 'toggle');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [1, 2], tag: 'A', value: false }, 'all have it: it comes off them all');
  await settle();
  assert.match(node('strip').innerHTML, /data-on="none"[^]*aria-pressed="false"/);
  assert.equal((node('grid').innerHTML.match(/Untagged/g) || []).length, 3, 'tiles show it gone');
  assert.match(node('strip').innerHTML, />0\/3<\/button>/, 'a tag taken off every page stays in the strip to put back');
}

async function untagAndOpenDoNotSelect() {
  const { K, node, requests, click, key, opened, settle } = await screenFixture();
  const e = click(1, { '[data-untag]': { dataset: { untag: 'A' } } });
  assert.deepEqual(plain(K.sel), [], 'a tag chip × does not select');
  assert.doesNotMatch(node('grid').innerHTML, /data-untag="A"/, 'the tag leaves the tile at once');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [1], tag: 'A', value: false }, '× takes the tag off that page only');
  await settle();
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
    assert.deepEqual(requests[0].body, { tag: 'A', n: 20, images: false });
    requests.shift().resolve({ tag: 'A', hits: [page(7), page(8)] });
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
  click(7); chip('A', 'toggle'); await settle();
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
  const { K, node, requests, click, settle } = await screenFixture();
  const submit = (name) => { node('new-tag').value = name; node('newtag').submit({ preventDefault() {} }); };
  submit('Night trains');
  await turn();
  assert.equal(requests.length, 0, 'no selection, no tag made');
  assert.match(node('new-msg').textContent, /Select pages first/);
  click(2); click(3); await settle();
  submit('  Night trains ');
  await turn();
  assert.deepEqual(requests[0].body, { name: 'Night trains' }, 'the name goes trimmed, through the tag rules');
  requests.shift().resolve({ tag: 'Night trains', created: true, tags: [] });
  await turn(); await turn();
  assert.deepEqual(requests[0].body, { rows: [2, 3], tag: 'Night trains', value: true }, 'it goes on every selected page');
  assert.equal(node('new-tag').value, '', 'the box empties for the next one');
  await settle();
  submit('a');
  await turn();
  requests.shift().reject(new Error('cannot use that name as a tag: it is empty'));
  await turn(); await turn();
  assert.match(node('new-msg').textContent, /cannot use that name as a tag/, 'a refusal shows inline');
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
  const { K, keys, context } = setup('app.js');
  await turn();
  let overlay = true;
  K.sel = [2, 7];
  K.views.key = (e) => {
    assert.equal(e.key, 'Escape');
    if (overlay) overlay = false;
    else K.sel = [];
    return true;
  };
  const target = { closest: () => ({}) };  // focused search, new tag or textarea
  let prevented = 0;
  const press = () => keys.keydown({ key: 'Escape', target, preventDefault() { prevented++; } });
  press();
  assert.equal(overlay, false, 'Esc from a text input leaves the like view first');
  assert.deepEqual(K.sel, [2, 7], 'leaving the like view keeps the selection');
  press();
  assert.deepEqual(K.sel, [], 'Esc from a text input with no like view clears selection');
  assert.equal(prevented, 2);
  context.document.querySelector = () => ({});  // an open dialog owns Escape
  press();
  assert.equal(prevented, 2, 'an open dialog keeps its native Escape handling');
}

async function likeNaming() {
  const { K, node, requests, chip } = await screenFixture();
  assert.match(node('strip').innerHTML, /Find pages like A, not tagged A/);
  chip('A', 'like');
  assert.match(node('v-pos').innerHTML, /Finding pages like <b>A<\/b>, not tagged <b>A<\/b>/);
  requests.shift().resolve({ tag: 'A', hits: [] });
  await turn();
  assert.match(node('v-pos').innerHTML, /Pages like <b>A<\/b>, not tagged <b>A<\/b>/);
  assert.match(node('grid').innerHTML, /No pages like A without that tag are left/);
}

const cases = { escapeFromTextInputs, likeNaming, keysBesideCheckbox, select, tagToggle, untagAndOpenDoNotSelect, likeToggle, newTagApplies, newTagKeyboardFocus, newTag, newTagExisting, newTagCancelAndRefusal, openControl, openSearch, openReview, openNoDrag, search, restore, exactQuery, searchAfterCut, pickAfterCut, exclude, cut, pickIncluded, flips, switchedSet, acceptAfterFlip, confirmAllPending, exportWait, exportCommand };
cases[process.argv[2]]().catch((err) => { console.error(err); process.exitCode = 1; });
