// A small DOM for the shipped scripts and controlled API round trips: what every frontend case file shares.
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
    views: { render() {}, key: () => false, restore: async () => {}, refresh: async () => {} }, strip: { render() {} }, add: { render() {}, enter() {} }, lit, ...overrides,
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
    getComputedStyle: (el) => ({ gridTemplateColumns: typeof el.columns === 'string' ? el.columns : Array(el.columns || 3).fill('10rem').join(' ') }),   // numeric tracks or an explicit layout
    ResizeObserver: class { constructor(fn) { this.fn = fn; } observe(el) { el.resized = this.fn; } },   // a case calls `node(id).resized()`
  });
  context.scrollTo = (x, y) => { context.scrollY = y; };
  let coreFail;
  for (const file of ['core.js', ...files]) {   // the real open helpers, everything else mocked
    vm.runInContext(fs.readFileSync(path.join(__dirname, '../tagger/app/static', file), 'utf8'), context);
    if (file === 'core.js') { coreFail = K.fail; Object.assign(K, mocks); }
  }
  return { K, node, history, context, keys, coreFail };
}
const turn = () => new Promise((resolve) => setImmediate(resolve));
const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made inside the scripts' realm

/* run the case the command line names; a case that never settles fails */
function run(cases) {
  const timeout = setTimeout(() => { console.error('frontend round trip did not settle'); process.exit(1); }, 5000);
  cases[process.argv[2]]().catch((err) => { console.error(err); process.exitCode = 1; }).finally(() => clearTimeout(timeout));
}

module.exports = { lit, setup, turn, plain, run };
