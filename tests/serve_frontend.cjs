// Run with: node --test tests/serve_frontend.cjs
// Exercise the shipped renderer without starting its DOM event wiring.
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

const source = readFileSync(join(__dirname, '../web/app.js'), 'utf8');
assert.match(source, /\nmain\(\);\s*$/);
const renderer = source.replace(/\nmain\(\);\s*$/, '\n');

function openPages(snapshots) {
  const context = vm.createContext({
    document: {
      getElementById: () => null,
      documentElement: { style: { setProperty() {} } },
    },
    IntersectionObserver: class {},
    library: {
      pages: ['Current', 'Earlier', 'Added'].map((title, i) => ({
        url: `https://synthetic.test/page-${i}`, domain: 'synthetic.test', title,
      })),
      snapshots,
    },
  });
  vm.runInContext(renderer, context);
  return Array.from(vm.runInContext('derive(library); S.pages.filter(p => p.open).map(p => p.title)', context));
}

function snapshot(id, captured_at, pages) {
  return { id, captured_at, tabs: pages.map((page, i) => [page, 1, i, i + 1, 0, null]) };
}

const earlier = snapshot('2026-01-01-000000Z', '2026-01-01T00:00:00Z', [1]);
const current = snapshot('2026-02-01-000000Z', '2026-02-01T00:00:00Z', [0]);

test('intake never displaces the latest browser capture, regardless of date', () => {
  for (const date of ['2025-12-01T00:00:00Z', '2026-02-01T00:00:00Z', '2026-03-01T00:00:00Z']) {
    const snapshots = [earlier, current, snapshot('added', date, [2])]
      .sort((a, b) => new Date(a.captured_at) - new Date(b.captured_at));
    assert.deepEqual(openPages(snapshots), ['Current']);
  }
});

test('an intake only library has no pages open now', () => {
  assert.deepEqual(openPages([snapshot('added', '2026-03-01T00:00:00Z', [2])]), []);
});

test('a later intake sighting preserves open membership only for captured pages', () => {
  assert.deepEqual(openPages([
    earlier, current, snapshot('added', '2026-03-01T00:00:00Z', [0, 1, 2]),
  ]), ['Current']);
});

// ---- Library search through the model ------------------------------------
// The renderer over a stub fetch whose replies the test gives, timers the
// test runs, and `render` replaced by a recorder of what `compute` shows.
function searchable(pages, { exported = false } = {}) {
  const timers = new Map(), requests = [];
  let next = 0;
  const context = vm.createContext({
    document: {
      getElementById: (id) => (id === 'library-data' ? (exported ? { textContent: '{}' } : null) : { firstElementChild: {}, classList: { toggle() {} } }),
      documentElement: { style: { setProperty() {} } },
    },
    IntersectionObserver: class {},
    setTimeout: (f) => { timers.set(++next, f); return next; },
    clearTimeout: (id) => timers.delete(id),
    requestAnimationFrame: (f) => { f(); return 0; },
    cancelAnimationFrame() {},
    fetch: (path, init) => new Promise((resolve, reject) => requests.push({ path, query: JSON.parse(init.body).query, resolve, reject })),
    library: {
      pages,
      // Three months of snapshots, a third of the pages in each, so date sorts have bands.
      snapshots: [0, 1, 2].map((k) => snapshot(`2026-0${k + 1}-01-000000Z`, `2026-0${k + 1}-01T00:00:00Z`, pages.flatMap((_, i) => (i % 3 === k ? [i] : [])))),
    },
  });
  vm.runInContext(renderer, context);
  vm.runInContext(`derive(library); DD.sort = { options: SORT_OPTIONS, set(o) { this.options = o; } };
    var renders = []; render = () => { compute(); renders.push(S.shown.map((p) => p.title)); };`, context);
  const run = (code) => vm.runInContext(code, context);
  const flush = () => new Promise((resolve) => setImmediate(resolve));
  return {
    requests,
    run,
    type: (q) => run(`S.q = ${JSON.stringify(q)}; ask();`),
    settle: () => { const due = [...timers.values()]; timers.clear(); for (const f of due) f(); },
    answer: async (i, body, ok = true) => { requests[i].resolve({ ok, status: ok ? 200 : 503, json: async () => body }); await flush(); },
    unreachable: async (i) => { requests[i].reject(new TypeError('Failed to fetch')); await flush(); },
    renders: () => run('renders.length'),
    shown: () => Array.from(run('S.shown.map((p) => p.title)')),
  };
}

const page = (i, domain = 'synthetic.test', title = `Page ${i}`) => ({ url: `https://${domain}/page-${i}`, domain, title });
const urls = (pages) => pages.map((p) => p.url);

test('the model orders a query, and relevance is the sort while it does', async () => {
  const pages = Array.from({ length: 10 }, (_, i) => page(i));
  const lib = searchable(pages);
  lib.type('alpha');
  lib.settle();
  assert.equal(lib.requests.length, 1);
  assert.equal(lib.requests[0].path, 'model/api/rank');
  assert.equal(lib.requests[0].query, 'alpha');
  assert.equal(lib.renders(), 0, 'the list holds until the model answers');
  await lib.answer(0, { keys: urls(pages).reverse() });
  assert.deepEqual(lib.shown(), pages.map((p) => p.title).reverse());
  assert.equal(lib.run('S.sort'), 'relevance');
  assert.equal(lib.run('DD.sort.options[0].value'), 'relevance');
  lib.type('');
  assert.equal(lib.shown().length, 10);
  assert.equal(lib.run('S.sort'), 'last');
  assert.equal(lib.run('DD.sort.options.some((o) => o.value === "relevance")'), false);
});

test('ranked, the list is the best fifty that pass the facets, and the pickers count it', async () => {
  const pages = Array.from({ length: 120 }, (_, i) => page(i, i % 2 ? 'b.test' : 'a.test'));
  const lib = searchable(pages);
  lib.run('S.domain = "b.test"');
  lib.type('x');
  lib.settle();
  await lib.answer(0, { keys: urls(pages) });
  const odd = pages.filter((_, i) => i % 2).slice(0, 50).map((p) => p.title);
  assert.deepEqual(lib.shown(), odd);
  // The Site picker ignores its own filter: the best fifty of every site.
  assert.deepEqual(JSON.parse(lib.run('JSON.stringify(facet("domain", (p) => [p.domain]))')), [['a.test', 25], ['b.test', 25]]);
  // Any other picker counts the pages shown.
  assert.deepEqual(JSON.parse(lib.run('JSON.stringify(facet("group", (p) => [p.domain]))')), [['b.test', 50]]);
});

test('pages the model has not indexed join a ranked list only when the words match', async () => {
  const indexed = Array.from({ length: 5 }, (_, i) => page(i));
  const pages = [...indexed, page(5, 'synthetic.test', 'Night train, saved since'), page(6, 'synthetic.test', 'Saved since, elsewhere')];
  const lib = searchable(pages);
  lib.type('night');
  lib.settle();
  await lib.answer(0, { keys: urls(indexed).reverse() });
  assert.deepEqual(lib.shown(), [...indexed.map((p) => p.title).reverse(), 'Night train, saved since']);
});

test('a failed or malformed reply leaves the keyword search, and the next query asks again', async () => {
  const pages = Array.from({ length: 9 }, (_, i) => page(i, 'synthetic.test', i % 2 ? `Night ${i}` : `Day ${i}`));
  const keyword = (word) => Array.from(lib.run(`S.pages.filter((p) => p.hay.includes(${JSON.stringify(word)})).sort(SORTS.last).map((p) => p.title)`));
  const lib = searchable(pages);
  lib.type('night');
  lib.settle();
  await lib.unreachable(0);
  assert.deepEqual(lib.shown(), keyword('night'));
  assert.equal(lib.run('S.sort'), 'last');
  // Known to have failed: keyword results at once, and the model is asked again.
  const fails = [(i) => lib.answer(i, { error: 'busy' }, false), (i) => lib.answer(i, { keys: [1, 2] })];
  for (const [i, fail] of fails.entries()) {
    const before = lib.renders(), word = i ? 'night' : 'day';
    lib.type(word);
    assert.equal(lib.renders(), before + 1);
    assert.deepEqual(lib.shown(), keyword(word));
    lib.settle();
    await fail(i + 1);
    assert.deepEqual(lib.shown(), keyword(word));
  }
  lib.type('day');
  lib.settle();
  await lib.answer(3, { keys: urls(pages) });
  assert.deepEqual(lib.shown(), pages.map((p) => p.title));
});

test('only the newest reply orders the list', async () => {
  const pages = Array.from({ length: 4 }, (_, i) => page(i));
  const lib = searchable(pages);
  lib.type('a');
  lib.settle();
  lib.type('ab');
  lib.settle();
  assert.deepEqual(lib.requests.map((r) => r.query), ['a', 'ab']);
  await lib.answer(1, { keys: urls(pages).reverse() });
  const renders = lib.renders();
  await lib.answer(0, { keys: urls(pages) });
  assert.equal(lib.renders(), renders, 'the older reply is dropped');
  assert.deepEqual(lib.shown(), ['Page 3', 'Page 2', 'Page 1', 'Page 0']);
});

test('a burst of typing asks once', () => {
  const lib = searchable([page(0)]);
  for (const q of ['n', 'ni', 'nig', 'nigh', 'night']) lib.type(q);
  lib.settle();
  assert.deepEqual(lib.requests.map((r) => r.query), ['night']);
});

test('a date sort reorders the ranked pages in date bands, and an explicit sort stays', async () => {
  const pages = Array.from({ length: 120 }, (_, i) => page(i));
  const lib = searchable(pages);
  lib.type('x');
  lib.settle();
  await lib.answer(0, { keys: urls(pages).reverse() });
  const ranked = lib.shown();
  lib.run('setSeg("sort", "first"); render();');
  assert.deepEqual([...lib.shown()].sort(), [...ranked].sort());
  assert.notDeepEqual(lib.shown(), ranked);
  assert.equal(lib.run('S.shown.every((p) => bandOf(p) !== "")'), true);
  assert.equal(lib.run('new Set(S.shown.map(bandOf)).size'), 3);
  lib.type('');
  assert.equal(lib.run('S.sort'), 'first');
});

test('an export never asks the model', () => {
  const pages = [page(0, 'synthetic.test', 'Night train'), page(1)];
  const lib = searchable(pages, { exported: true });
  assert.equal(lib.run('host.mode'), 'export');
  lib.type('night');
  lib.settle();
  assert.equal(lib.requests.length, 0);
  assert.deepEqual(lib.shown(), ['Night train']);
});
