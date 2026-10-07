// Run the shipped scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function setup(file) {
  const nodes = new Map();
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
    fail(err) { this.failure = err; }, toast() {}, flyIn() {}, flyOut(c, d, done) { done(); },
    state: { search: { query: 'old', images: false, n: 20, picked: ['A'] }, mode: 'grid' },
    lib: { sizes: [20, 50], images: true }, results: { hits: [{ row: 9 }] },
    search: { render() {}, restore: async () => {} }, pick: { render() {} }, review: { render() {}, left: () => 0 },
  };
  const context = vm.createContext({
    window: { KMT: K }, document: { activeElement: null, documentElement: node('root'), querySelector() { return null; }, addEventListener() {} },
    location: { hash: '' }, addEventListener() {}, matchMedia: () => ({ matches: false }),
    setTimeout, clearTimeout, console,
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../tagger/app/static', file), 'utf8'), context);
  return { K, node };
}
const turn = () => new Promise((resolve) => setImmediate(resolve));

async function search() {
  const { K, node } = setup('search.js');
  const requests = [];
  K.api = () => new Promise((resolve) => requests.push(resolve));
  node('s-q').value = 'first';
  const first = K.search.run();
  assert.equal(K.results, null, 'pending search must hide old results');
  K.search.render();
  assert.equal(node('s-tag').disabled, true);
  node('s-q').value = 'second';
  const second = K.search.run();
  requests[1]({ hits: [{ row: 2 }] });
  await second;
  requests[0]({ hits: [{ row: 1 }] });
  await first;
  assert.equal(K.results.hits[0].row, 2, 'stale search reply must not replace current results');
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

const cases = { search, flips, switchedSet, acceptAfterFlip, exportWait, exportCommand };
cases[process.argv[2]]().catch((err) => { console.error(err); process.exitCode = 1; });
