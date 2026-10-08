// Run the shipped scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

/* lit-html's API over this string DOM: a template renders as its markup with the values in place */
const nothing = Symbol('nothing');
const markup = (v) => (v === nothing || v == null ? '' : Array.isArray(v) ? v.map(markup).join('')
  : v.strings ? v.strings.reduce((out, s, i) => out + markup(v.values[i - 1]) + s) : String(v));
const lit = { nothing, html: (strings, ...values) => ({ strings, values }), repeat: (items, key, each) => items.map(each),
  unsafeHTML: String, render(v, el) { el.innerHTML = markup(v); } };

function setup(...files) {
  const overrides = typeof files[0] === 'object' ? files.shift() : {};
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
    fail(err) { this.failure = err; }, toast(msg, undo) { this.toasted = msg; this.undo = undo || null; }, flyIn() {}, flyOut(c, d, done) { done(); }, refreshSets() {},
    state: { search: { query: 'old', images: false, n: 20, picked: ['A'] }, mode: 'grid' },
    lib: { sizes: [20, 50], images: true }, results: { hits: [{ row: 9 }] },
    search: { render() {}, restore: async () => {} }, pick: { render() {} }, review: { render() {}, left: () => 0 },
    views: { render() {}, key: () => false, restore: async () => {} }, strip: { render() {} }, add: { render() {}, enter() {} }, lit, ...overrides,
  };
  const listeners = {}, keys = {};   // the window's and the document's, by event
  const history = {       // one tab's entries; back() delivers popstate a turn later, as browsers do
    entries: [null], pushes: 0,
    get state() { return this.entries[this.entries.length - 1]; },
    pushState(state) { this.entries.push(structuredClone(state)); this.pushes++; },
    replaceState(state) { this.entries[this.entries.length - 1] = structuredClone(state); },
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
  assert.ok(html.includes("<pre>knowmoretabs tag --import"), 'import command must target the owner default library');
  assert.ok(!html.includes('--root'), 'the app snapshot must never be the export target');
  assert.ok(html.includes("--import '/tmp/owner'\\''s data/answers.jsonl'"), 'answer path must be shell quoted');
  assert.ok(html.includes('--accept-new'), 'created tags must validate without an existing archive tag');
  assert.ok(!html.includes('xargs'), 'nothing forgotten, no forget command');
}

async function exportForget() {
  const { K, node } = setup('app.js');
  await turn();
  K.api = async () => ({ decided: 0, answers: '/a', decisions: '/d', forget: "/data/owner's exports/forget.urls", forgotten: 2, source: 'test' });
  await K.exportNow();
  const html = node('export-body').innerHTML;
  assert.ok(html.includes("<pre>xargs -0 knowmoretabs forget -- < '/data/owner'\\''s exports/forget.urls'</pre>"),
    'the manifest goes through xargs -0 to the owner default library, its path one shell word');
  assert.ok(html.includes('Forget the 2 forgotten pages'));
  assert.ok(!html.includes('tag --import'), 'nothing tagged, no import command');
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
async function screenFixture(...more) {
  const f = setup('pages.js', 'views.js', 'strip.js', ...more);
  const { K, node } = f;
  K.changed = () => { K.views.render(); K.strip.render(); };
  K.lib = { sizes: [20, 50], images: true, pages: 9, app_tags: ['A'] };
  const requests = [];
  K.api = (route, body) => new Promise((resolve, reject) => requests.push({ route, body: body && plain(body), resolve, reject }));
  const loading = K.loadLists();   // the startup load of the selection and the pins, both empty
  for (let i = 0; i < 2; i++) { await turn(); requests.shift().resolve({ pages: [] }); }
  await loading;
  node('s-q').value = 'q';
  const searching = K.views.run();
  await turn();
  requests.shift().resolve({ hits: [page(1, ['A']), page(2), page(3)], offset: 0, total: 3 });
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
  const batches = [];
  const tags = (rows) => rows.map((row) => ({ row, tags: K.pages.get(row).tags }));
  const reply = (r) => {
    if (r.route === '/api/apply') return { pages: tags(r.body.rows), changed: r.body.rows.length, batch: batches.push(r.body.rows), app_tags: ['A'] };
    if (r.route === '/api/undo') return { pages: tags(batches[r.body.batch - 1]), app_tags: ['A'] };
    if (r.route === '/api/forget') return { pages: K.lib.pages };
    return { rows: r.body.rows };
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
  assert.match(strip(), /data-on="all"/, 'the chip fills at once');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [2, 3], tag: 'A', value: true }, 'the name adds to the selected pages lacking it');
  await settle();
  assert.match(K.toasted, /^Added A to 2 pages$/);
  chip('A', 'add');
  await turn();
  assert.equal(requests.length, 0, 'the name never takes a tag off, even when every selected page has it');
  assert.match(K.toasted, /Every selected page has A/);
  click(3); await settle();
  chip('A', 'remove');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [1, 2], tag: 'A', value: false }, '× takes it off the selected pages only');
  await settle();
  assert.match(K.toasted, /^Removed A from 2 pages$/);
  assert.equal(K.has(3, 'A'), true, 'a page outside the selection keeps it');
  assert.doesNotMatch(strip(), /data-act="remove"/, 'no selected page has it: no ×');
  assert.match(strip(), /data-on="none"[^]*>0\/2<\/span>/, 'a tag on none of the selection stays to put back');
  K.clearSel(); await settle();
  assert.doesNotMatch(strip(), /class="ct"/, 'the count hides with no selection');
}

/* Undo after a strip action: each page exactly as it was */
async function bulkUndo() {
  const { K, requests, click, chip, settle, press } = await screenFixture('app.js');
  const has = () => [1, 2, 3].map((r) => K.has(r, 'A'));
  click(1); click(2); click(3); await settle();
  chip('A', 'add'); await settle();
  assert.deepEqual(has(), [true, true, true]);
  press('u');
  assert.deepEqual(has(), [true, false, false], 'Undo takes it off only the pages it was added to');
  await turn();
  assert.deepEqual(requests[0].body, { batch: 1 }, 'the server retracts that batch');
  assert.equal(requests[0].route, '/api/undo');
  await settle();
  assert.deepEqual(has(), [true, false, false], 'and its reply agrees');
  chip('A', 'remove'); await settle();
  assert.deepEqual(has(), [false, false, false]);
  assert.equal(typeof K.undo, 'function');
  press('z', { ctrlKey: true });
  assert.deepEqual(has(), [true, false, false], 'Ctrl+Z undoes too: only the page that had it gets it back');
  await turn();
  assert.deepEqual(requests[0].body, { batch: 2 });
  await settle();
}

/* Forget: out of the grid, the selection, the pins and every count at once; Undo puts it back as it was */
async function forgetAndUndo() {
  const { K, node, requests, click, key, settle, press, shown } = await screenFixture('app.js');
  click(1); click(2); key(2, 'p'); await settle();
  const e = click(2, { '[data-forget]': {} });
  assert.equal(e.stopped, false);
  assert.deepEqual(plain(K.sel), [1], 'the page leaves the selection');
  assert.deepEqual(plain(K.pins), [], 'and the pins');
  assert.deepEqual(shown(), [1, 3], 'and the grid');
  assert.equal(node('n-sel').textContent, 1);
  assert.equal(node('n-pin').textContent, 0);
  assert.match(node('v-pos').innerHTML, /^<b>1 to 2<\/b> of 2 for “q”$/, 'the counts drop it');
  assert.equal(K.lib.pages, 8);
  assert.match(node('strip').innerHTML, />1\/1<\/span>/);
  assert.equal(K.toasted, 'Forgotten');
  await turn();
  assert.deepEqual(requests[0].body, { rows: [2], value: true });
  assert.equal(requests[0].route, '/api/forget');
  await settle();
  press('u');
  assert.deepEqual(shown(), [1, 2, 3], 'Undo puts it back in its place');
  assert.deepEqual(plain(K.sel), [1, 2], 'selected as it was');
  assert.deepEqual(plain(K.pins), [2], 'and pinned');
  assert.equal(K.lib.pages, 9);
  await turn();
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/forget', { rows: [2], value: false }]);
  await settle();
  assert.equal(key(3, 'f'), true, 'f forgets the focused page');
  assert.deepEqual(plain(K.sel), [1, 2], 'and selects nothing');
  assert.deepEqual(shown(), [1, 2]);
  await turn();
  assert.deepEqual(requests[0].body, { rows: [3], value: true });
}

/* the undo of a forget writes the page back before the lists that hold it */
async function forgetUndoOrder() {
  const { K, requests, click, key, settle, press } = await screenFixture('app.js');
  click(2); key(2, 'p'); await settle();
  click(2, { '[data-forget]': {} });
  press('u');
  const sent = [];
  for (let i = 0; i < 8; i++) {
    await turn();
    const r = requests.shift();
    if (!r) continue;
    sent.push([r.route, r.body]);
    r.resolve(r.route === '/api/forget' ? { pages: K.lib.pages } : { rows: r.body.rows });
  }
  assert.deepEqual(sent, [['/api/forget', { rows: [2], value: true }], ['/api/forget', { rows: [2], value: false }],
    ['/api/selection', { rows: [2] }], ['/api/pins', { rows: [2] }]]);
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
async function forgetUndoAfterNavigation() {
  for (const navigation of ['search', 'next']) {
    const { K, node, requests, click, settle, shown, turnTo } = await pagedFixture();
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
    requests.shift().resolve({ pages: 9 });
    await turn(); await turn();
    assert.equal(requests[0]?.route, '/api/search', 'Undo after navigation must rank the current view again');
    assert.equal(requests[0].body.offset, offset, 'Undo keeps the current result page');
    const hits = rows(offset + 1, offset + 50);
    requests.shift().resolve({ hits, offset, total: 120 });
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
  requests.shift().resolve({ sizes: [20, 50], images: true, pages: 120, app_tags: [], forgotten: [] });
  await turn();
  assert.equal(requests[0].route, '/api/selection');
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
  assert.deepEqual([requests[0].route, requests[0].body], ['/api/pins', { rows: [2] }], 'the pins are kept by the server');
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
  const { K, requests, click, chip, settle } = await screenFixture();
  click(2); await settle();
  chip('A', 'add');
  chip('A', 'like');
  await turn();
  assert.equal(requests.length, 1, 'the ranking must wait for the pending tag decision');
  assert.equal(requests[0].route, '/api/apply');
  requests.shift().resolve({ pages: [{ row: 2, tags: ['A'] }], app_tags: ['A'] });
  await turn();
  assert.equal(requests[0].route, '/api/like');
  requests.shift().resolve({ hits: [page(3)], offset: 0, total: 1 });
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

/* ---- Add link: every state of design section 9, read off the job the server reports ---- */
async function addStates() {
  const { K } = setup({ lit, strip: { render() {}, sayIn: () => () => {} } }, 'add.js');
  const ev = (stage, state, extra = {}) => ({ stage, state, ...extra });
  const L = (value, extra) => ev('library', 'done', { value, ...extra }), C = (status, tier, extra) => ev('content', 'done', { status, tier, ...extra });
  const I = (status) => ev('image', 'done', { status }), S = (value) => ev('search', 'done', { value });
  const job = (stages, extra = {}) => ({ action: 'add', failed: false, page: null, stages: Object.fromEntries(stages.map((e) => [e.stage, e])), ...extra });
  const cases = [
    ['Adding', [ev('library', 'running')], ['Adding', '', '', ''], null],
    ['Content, web', [L('added'), ev('content', 'running', { tier: 'web' })], ['Added', 'Web', '', ''], null],
    ['Content, rendering', [L('added'), ev('content', 'running', { tier: 'headless' })], ['Added', 'Rendering', '', ''], null],
    ['Image', [L('added'), C('ok', 'headless'), ev('image', 'running')], ['Added', 'Ok · Headless', 'Fetching', ''], null],
    ['Search', [L('added'), C('ok', 'headless'), I('ok'), ev('search', 'running')], ['Added', 'Ok · Headless', 'Ok', 'Indexing'], null],
    ['Done', [L('added'), C('ok', 'headless'), I('ok'), S('indexed')], ['Added', 'Ok · Headless', 'Ok', 'Indexed'], null],
    ['Already in library', [L('known'), C('ok', 'web'), I('ok'), S('indexed')], ['Already in library', 'Ok · Web', 'Ok', 'Indexed'], null],
    ['Forgotten', [L('forgotten')], ['Forgotten', '', '', ''], 'Restore'],
    ['Not a web page', [L('refused', { reason: 'not_web' })], ['Not a web page', '', '', ''], null],
    ['Waiting', [ev('library', 'waiting')], ['Waiting', '', '', ''], null],
    ['Retrying', [L('added'), ev('content', 'retrying', { after_s: 2 })], ['Added', 'Retrying', '', ''], null],
    ['Blocked', [L('added'), C('blocked', 'web', { http_status: 403 }), I('none'), S('indexed')], ['Added', 'Blocked · 403', 'No image', 'Indexed'], 'Try signed in'],
    ['Behind login', [L('added'), C('behind_login', 'web', { http_status: 401 }), I('none'), S('indexed')], ['Added', 'Behind login', 'No image', 'Indexed'], 'Try signed in'],
    ['Paywalled', [L('added'), C('paywalled', 'web'), I('ok'), S('indexed')], ['Added', 'Paywalled', 'Ok', 'Indexed'], 'Try signed in'],
    ['Signed in, running', [L('known'), ev('content', 'running', { tier: 'signed_in' })], ['Already in library', 'Signed in', '', ''], null],
    ['Signed in, off', [L('known'), C('off', 'signed_in'), I('none'), S('indexed')], ['Already in library', 'Chrome not reachable', 'No image', 'Indexed'], 'Retry'],
    ['Signed in, not running', [L('known'), C('not_running', 'signed_in'), I('none'), S('indexed')], ['Already in library', 'Chrome not reachable', 'No image', 'Indexed'], 'Retry'],
    ['Signed in, not allowed', [L('known'), C('not_allowed', 'signed_in'), I('none'), S('indexed')], ['Already in library', 'Not allowed', 'No image', 'Indexed'], 'Retry'],
    ['Timed out', [L('added'), C('error', 'web', { reason: 'timeout' }), I('unknown'), S('indexed')], ['Added', 'Timed out', 'No image', 'Indexed'], 'Retry'],
    ['HTTP error', [L('added'), C('error', 'web', { http_status: 503 }), I('unknown'), S('indexed')], ['Added', 'Error · 503', 'No image', 'Indexed'], 'Retry'],
    ['Unavailable', [L('known'), C('unavailable', 'web', { reason: 'failed on 3 runs' }), I('unknown'), S('indexed')], ['Already in library', 'Unavailable', 'No image', 'Indexed'], null],
    ['Not recorded', [L('added'), C('unknown', null, { reason: 'not_recorded' }), I('unknown'), S('indexed')], ['Added', 'Not recorded', 'No image', 'Indexed'], null],
    ['Image retrying', [L('added'), C('ok', 'web'), ev('image', 'retrying', { after_s: 2 })], ['Added', 'Ok · Web', 'Retrying', ''], null],
    ['Image error', [L('added'), C('ok', 'web'), I('error'), S('indexed')], ['Added', 'Ok · Web', 'Error', 'Indexed'], 'Retry'],
    ['Image unavailable', [L('known'), C('ok', 'web'), I('unavailable'), S('indexed')], ['Already in library', 'Ok · Web', 'Unavailable', 'Indexed'], null],
    ['Not in library', [L('refused', { reason: 'not_in_library' })], ['Not in library', '', '', ''], null],
    ['Other tier', [L('known'), C('ok', 'other'), I('ok'), S('indexed')], ['Already in library', 'Ok', 'Ok', 'Indexed'], null],
    ['Not found', [L('added'), C('not_found', 'web', { http_status: 404 }), I('none'), S('indexed')], ['Added', 'Not found · 404', 'No image', 'Indexed'], 'Remove'],
    ['Thin', [L('added'), C('thin', 'headless'), I('ok'), S('indexed')], ['Added', 'Thin · Headless', 'Ok', 'Indexed'], null],
    ['Skipped', [L('added'), C('skipped', 'web'), I('none'), S('indexed')], ['Added', 'Skipped', 'No image', 'Indexed'], null],
    ['No image', [L('added'), C('ok', 'web'), I('none'), S('indexed')], ['Added', 'Ok · Web', 'No image', 'Indexed'], null],
    ['Not indexed', [L('added'), C('ok', 'web'), I('ok'), S('not_indexed')], ['Added', 'Ok · Web', 'Ok', 'Not indexed'], 'Retry'],
  ];
  for (const [name, stages, values, act] of cases) {
    const s = K.add.say(job(stages));
    assert.deepEqual(plain(s.segs.map(([v]) => v)), values, name);
    assert.equal(s.act && s.act[0], act, `${name}: action`);
  }
  const looks = (stages) => plain(K.add.say(job(stages)).segs.map(([, k]) => k));
  assert.deepEqual(looks([ev('library', 'running')]), ['run', '', '', '']);
  assert.deepEqual(looks([L('added'), C('thin', 'headless'), I('none'), S('not_indexed')]), ['ok', 'soft', 'soft', 'bad']);
  assert.deepEqual(looks([L('forgotten')]), ['bad', '', '', '']);
  assert.deepEqual(looks([L('added'), C('unknown', null, { reason: 'not_recorded' }), I('error')]), ['ok', 'soft', 'bad', '']);
  assert.deepEqual(looks([L('known'), C('unavailable', 'web'), I('unavailable')]), ['ok', 'bad', 'bad', '']);
  const retry = (action, stages, extra) => K.add.say(job(stages, { action, ...extra })).act;
  const again = (retry, signed_in = false) => ['Retry', { action: 'add', retry, signed_in }];
  /* the contract's lab mapping, row by row: only the failed stages, every one in one run */
  assert.deepEqual(plain(retry('add', [L('added'), C('error', 'web', { http_status: 503 }), I('unknown'), S('indexed')])), again(['content']), 'an error');
  assert.deepEqual(plain(retry('add', [L('added'), C('error', 'web', { reason: 'timeout' }), I('ok'), S('indexed')])), again(['content']), 'a timeout');
  for (const status of ['off', 'not_running', 'not_allowed'])
    assert.deepEqual(plain(retry('add', [L('known'), C(status, 'signed_in'), I('ok'), S('indexed')], { signed_in: true })), again(['content'], true), status);
  assert.deepEqual(plain(retry('add', [L('known'), C('ok', 'web'), I('error'), S('indexed')])), again(['image']), 'an image error');
  assert.deepEqual(plain(retry('add', [L('known'), C('error', 'web'), I('error'), S('indexed')])), again(['content', 'image']), 'both');
  assert.deepEqual(plain(retry('add', [L('known'), C('off', 'signed_in'), I('error'), S('indexed')])), again(['content', 'image'], true), 'signed in and the image');
  assert.deepEqual(plain(retry('add', [L('known'), C('not_found', 'web'), I('error'), S('indexed')])), again(['image']), 'a failed image still retries after Not found');
  assert.deepEqual(plain(retry('add', [L('known'), C('unavailable', 'web'), I('error'), S('indexed')])), again(['image']), 'a failed image even after unavailable');
  for (const [status, tier] of [['unavailable', 'web'], ['unknown', null], ['thin', 'headless'], ['skipped', null], ['media', 'web'], ['not_html', 'web'], ['empty_shell', 'headless']])
    for (const img of ['ok', 'none', 'unknown', 'unavailable'])
      assert.equal(retry('add', [L('known'), C(status, tier), I(img), S('indexed')]), null, `no Retry for ${status} and image ${img}`);
  /* Try signed in: a public baseline that was blocked, behind a login or paywalled */
  for (const status of ['blocked', 'behind_login', 'paywalled'])
    for (const tier of ['web', 'github', 'x', 'youtube', 'headless'])
      assert.deepEqual(plain(retry('add', [L('added'), C(status, tier), S('indexed')])), ['Try signed in', { action: 'add', retry: ['content'], signed_in: true }], `${status} · ${tier}`);
  for (const tier of ['signed_in', null, 'other'])
    assert.equal(retry('add', [L('known'), C('blocked', tier), I('none'), S('indexed')]), null, `no signed in read from ${tier}`);
  assert.deepEqual(plain(retry('add', [L('known'), C('behind_login', null), I('error'), S('indexed')])), again(['image']), 'its image still retries');
  assert.deepEqual(plain(retry('add', [L('added'), C('not_found', 'web'), S('indexed')])), ['Remove', { action: 'forget' }]);
  assert.deepEqual(plain(retry('add', [L('forgotten')])), ['Restore', { action: 'restore' }]);
  assert.deepEqual(plain(retry('add', [L('added'), C('ok', 'web'), I('ok'), S('not_indexed')])), ['Retry', { action: 'index' }], 'an index failure retries the lab only');
  assert.deepEqual(plain(retry('add', [L('added'), C('blocked', 'web'), S('not_indexed')])), ['Try signed in', { action: 'add', retry: ['content'], signed_in: true }], 'a content action also indexes');
  assert.deepEqual(plain(retry('add', [L('added'), C('ok', 'web'), I('error'), S('not_indexed')])), again(['image']), 'an image Retry also indexes');
  /* a job that ended without a result is asked again, a Restore that held as an add */
  const ask = { retry: [], signed_in: false, title: null };
  assert.deepEqual(plain(retry('forget', [], { failed: true, ...ask })), ['Retry', { action: 'forget', ...ask }], 'Retry repeats a failed Remove');
  assert.deepEqual(plain(retry('add', [L('known'), C('error', 'web')], { failed: true, ...ask, retry: ['content', 'image'], signed_in: true })),
    ['Retry', { action: 'add', ...ask, retry: ['content', 'image'], signed_in: true }], 'a failed Retry runs the same stages');
  assert.deepEqual(plain(retry('restore', [ev('library', 'running')], { failed: true, ...ask })), ['Retry', { action: 'restore', ...ask }], 'a failed Restore');
  assert.deepEqual(plain(retry('restore', [ev('library', 'running')], { failed: true, restored: true, ...ask })), ['Retry', { action: 'add' }], 'never a second Restore');
  const failed = K.add.say(job([ev('library', 'running')], { failed: true, ...ask, title: 'One note' }));
  assert.deepEqual([failed.segs[0][0], plain(failed.act)], ['Not added', ['Retry', { action: 'add', ...ask, title: 'One note' }]], 'knowmoretabs could not run');
  const tile = (stages, page) => { const s = K.add.say(job(stages, { page })); return plain([s.framed, s.indexed, s.title, s.picture]); };
  assert.deepEqual(tile([ev('library', 'running')]), [false, false, '', '']);
  assert.deepEqual(tile([L('added'), C('ok', 'web', { title: 'One note' }), ev('image', 'running')]), [true, false, 'One note', 'wait'], 'framed after Library, title as it lands');
  assert.deepEqual(tile([L('known'), C('not_running', 'signed_in'), S('indexed')], { title: 'Kept' }), [true, true, 'Kept', 'none'], 'settled with no image');
}

/* what the screen asks the server: the deep link's title with its own link only, and the action's request */
async function addRequests() {
  const { K, node, context } = setup({ lit, strip: { render() {}, sayIn: () => () => {} }, lib: { app_tags: [] } }, 'add.js');
  const asked = [];
  let reply = () => new Promise(() => {});
  K.api = (path, body) => { asked.push([path, body]); return reply(path, body); };
  const submit = () => node('a-form').submit({ preventDefault() {} });
  const link = 'https://added.example/a b?x=1&y=2';
  context.location.hash = `#add=${encodeURIComponent(link)}&title=${encodeURIComponent('One & two · é')}`;
  K.add.enter();
  assert.equal(node('a-link').value, link);
  submit();
  assert.deepEqual(plain(asked.pop()), ['/api/add', { url: link, action: 'add', title: 'One & two · é' }], 'the title goes with its link');
  node('a-link').value = 'https://added.example/other';
  submit();
  assert.deepEqual(plain(asked.pop()), ['/api/add', { url: 'https://added.example/other', action: 'add', title: null }], 'never with another link');
  context.location.hash = '#add=https://added.example/raw?a=1&b=2';
  K.add.enter();
  submit();
  assert.deepEqual(plain(asked.pop()), ['/api/add', { url: 'https://added.example/raw?a=1&b=2', action: 'add', title: null }], 'a link without a title keeps its &');
  const ev = (stage, state, extra = {}) => ({ stage, state, ...extra });
  const done = { id: 1, url: link, action: 'add', retry: [], signed_in: false, title: null, restored: false, failed: false, finished: true, seconds: 1.2, page: null,
    stages: { library: ev('library', 'done', { value: 'known' }), content: ev('content', 'done', { status: 'off', tier: 'signed_in' }),
      image: ev('image', 'done', { status: 'error' }), search: ev('search', 'done', { value: 'indexed' }) } };
  for (const k of ['library', 'content', 'image', 'search']) node(`a-${k}`).querySelector = () => node(`a-${k}-val`);
  reply = (path) => Promise.resolve(path === '/api/library' ? { app_tags: [] } : done);
  await K.add.start(link, { action: 'add', signed_in: true });
  await turn();
  assert.equal(K.failure, undefined);
  assert.equal(node('a-act').textContent, 'Retry');
  asked.length = 0;
  node('a-act').click();
  assert.deepEqual(plain(asked[0]), ['/api/add', { url: link, action: 'add', retry: ['content', 'image'], signed_in: true }], 'Retry starts the failed stages');
}

async function addSignedInStages() {
  const { K } = setup({ lit, strip: { render() {}, sayIn: () => () => {} } }, 'add.js');
  const refused = { http_status: 200, reason: 'short text; not rendered: HTTP 403' };   // the baseline's code, the render's refusal
  for (const value of ['added', 'known']) for (const tier of ['web', 'github', 'x', 'youtube', 'headless'])
    for (const status of ['blocked', 'behind_login', 'paywalled', 'thin', 'empty_shell']) for (const image of ['error', 'ok', 'none', 'unavailable', 'unknown']) {
      const stages = { library: { state: 'done', value }, content: { state: 'done', status, tier, ...(['thin', 'empty_shell'].includes(status) ? refused : { http_status: 403 }) }, image: { state: 'done', status: image } };
      const action = K.add.say({ stages }).act;
      assert.deepEqual(plain(action), ['Try signed in', { action: 'add', retry: image === 'error' ? ['content', 'image'] : ['content'], signed_in: true }]);
    }
  for (const tier of ['signed_in', 'other', null])
    assert.equal(K.add.say({ stages: { library: { value: 'known' }, content: { state: 'done', status: 'thin', tier, reason: 'not rendered: HTTP 403' } } }).act, null);
  for (const reason of ['short text', 'short text; not rendered: HTTP 404', undefined])
    assert.equal(K.add.say({ stages: { library: { value: 'known' }, content: { state: 'done', status: 'thin', tier: 'web', http_status: 403, reason } } }).act, null, `${reason}`);
}

/* Startup GETs snapshot the saved lists, but their replies are held. Writes replace whole lists and forgetting
   prunes both lists, as server.py does; forgotten rows cannot be selected or pinned. */
function startupFixture(saved = [1]) {
  const f = setup('pages.js', 'views.js', 'strip.js');
  const { K } = f;
  const server = { selection: [...saved], pins: [...saved] }, loaded = new Set(), late = [], sent = [];
  const gone = new Set(), tags = new Map();
  K.lib = { pages: 9, sizes: [20, 50], app_tags: ['A'] };
  K.state.search.query = '';
  K.know([page(1), page(2), page(3)]);
  K.changed = () => { K.views.render(); K.strip.render(); };
  K.api = (route, body) => {
    const name = route.replace('/api/', '');
    if (body === undefined) {
      const pages = server[name].map((row) => page(row, tags.get(row) || []));
      return new Promise((resolve) => late.push(() => { loaded.add(name); resolve({ pages }); }));
    }
    sent.push([name, plain(body)]);
    if (name === 'apply') {
      for (const row of body.rows) tags.set(row, [body.tag]);
      return Promise.resolve({ pages: body.rows.map((row) => ({ row, tags: tags.get(row) })),
        app_tags: [body.tag], changed: body.rows.length, batch: 1 });
    }
    if (name === 'forget') {
      for (const row of body.rows) {
        if (body.value) {
          gone.add(row);
          for (const list of Object.keys(server)) server[list] = server[list].filter((r) => r !== row);
        } else gone.delete(row);
      }
      return Promise.resolve({ pages: 9 - gone.size });
    }
    if (body.rows.some((row) => gone.has(row))) return Promise.reject(new Error('forgotten row'));
    server[name] = [...body.rows];
    return Promise.resolve({ rows: body.rows });
  };
  f.finish = async () => {
    while (late.length) { late.shift()(); await turn(); }
    await K.writes;
    await turn();
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
  for (const local of [false, true]) {
    for (const undoAt of ['never', 'early', 'loaded']) {
      const { K, server, sent, finish } = startupFixture([1, 2, 3]);
      const loading = K.loadLists();
      if (local) { K.select(2); K.pin(2); }
      K.forget(2);
      const undo = K.undo;
      if (undoAt === 'early') undo();
      await turn();
      await finish(); await loading;
      if (undoAt === 'loaded') { undo(); await K.writes; await turn(); }
      const expected = undoAt === 'never' ? [1, 3] : [1, 2, 3];
      assert.deepEqual(plain(K.sel), expected, `${local}/${undoAt}: selection, including saved membership and order`);
      assert.deepEqual(plain(K.pins), expected, `${local}/${undoAt}: pins, including saved membership and order`);
      assert.deepEqual(server, { selection: expected, pins: expected }, `${local}/${undoAt}: persisted lists`);
      assert.equal(K.failure, undefined, `${local}/${undoAt}: no rejected writes`);
      for (const name of ['selection', 'pins']) {
        const writes = sent.filter(([n]) => n === name);
        assert.ok(writes.length <= (undoAt === 'loaded' ? 2 : 1), 'each startup list sends once');
      }
      K.views.showPinned();
      assert.deepEqual(plain(K.views.shown(K.views.view())), expected, 'Pinned shows the restored list');
    }
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

const cases = { startupEdits, startupForgetAndUndo, startupTagsKeepChanges, addSignedInStages, startupListsKeepChanges, addStates, addRequests, forgetUndoAfterNavigation, startupDuringPage, chipActsOnSelection, bulkUndo, forgetAndUndo, forgetUndoOrder, forgetThenNext, pinView, exportForget, backDuringPage, likeWaitsForTag, pages, restorePage, likeBackToPage, escapeFromTextInputs, likeNaming, keysBesideCheckbox, select, untagAndOpenDoNotSelect, likeToggle, newTagApplies, newTagKeyboardFocus, newTag, newTagExisting, newTagCancelAndRefusal, openControl, openSearch, openReview, openNoDrag, search, restore, exactQuery, searchAfterCut, pickAfterCut, exclude, cut, pickIncluded, flips, switchedSet, acceptAfterFlip, confirmAllPending, exportWait, exportCommand };
const timeout = setTimeout(() => { console.error('frontend round trip did not settle'); process.exit(1); }, 5000);
cases[process.argv[2]]().catch((err) => { console.error(err); process.exitCode = 1; }).finally(() => clearTimeout(timeout));
