// Run the shipped scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function setup(...files) {
  const nodes = new Map();
  const events = new Map();
  const node = (id) => {
    if (!nodes.has(id)) nodes.set(id, {
      value: '', checked: false, disabled: false, innerHTML: '', textContent: '',
      dataset: {}, classList: { add() {}, remove() {}, toggle() {} },
      addEventListener(event, fn) { this[event] = fn; },
      querySelectorAll() { return []; }, querySelector() { return id === 'r-acts' ? node('action') : null; },
      getBoundingClientRect() { return {}; },
      focus() {}, setAttribute() {}, removeAttribute() {}, showModal() {},
    });
    return nodes.get(id);
  };
  const K = {
    $: node, esc: String, short: String, title: () => '', pct: String,
    own: () => '', thumb: () => '', link: () => '', save() {}, changed() {},
    fail(err) { this.failure = err; }, toast(msg) { this.toasted = msg; }, flyIn() {}, flyOut(c, d, done) { done(); }, refreshSets() {},
    state: { search: { query: 'old', images: false, n: 20, picked: ['A'] }, mode: 'grid' },
    lib: { sizes: [20, 50], images: true }, results: { hits: [{ row: 9 }] },
    search: { render() {}, restore: async () => {}, pending: Promise.resolve() }, pick: { render() {} }, review: { render() {}, left: () => 0 },
  };
  const context = vm.createContext({
    window: { KMT: K }, document: { activeElement: null, documentElement: node('root'), querySelector() { return null; }, addEventListener() {} },
    location: { hash: '' }, addEventListener(event, fn) { events.set(event, fn); }, matchMedia: () => ({ matches: false }),
    setTimeout, clearTimeout, console,
  });
  for (const file of files) vm.runInContext(fs.readFileSync(path.join(__dirname, '../tagger/app/static', file), 'utf8'), context);
  return { K, node, events, context };
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

async function tagAfterCut() {
  const { K, events, context } = setup('app.js');
  await turn();
  let settle, rendered = false;
  K.search.pending = new Promise((resolve) => { settle = resolve; });
  K.pick.render = () => { rendered = true; };
  context.location.hash = '#tag';
  const routing = events.get('hashchange')();
  await turn();
  assert.equal(rendered, false, 'tag suggestions must wait for pending exclusions');
  settle();
  await routing;
  assert.equal(rendered, true);
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
  K.review.pending = new Promise((resolve) => { finish = resolve; });
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
}

const cases = { search, restore, exactQuery, searchAfterCut, pickAfterCut, tagAfterCut, exclude, cut, pickIncluded, flips, switchedSet, acceptAfterFlip, confirmAllPending, exportWait, exportCommand };
cases[process.argv[2]]().catch((err) => { console.error(err); process.exitCode = 1; });
