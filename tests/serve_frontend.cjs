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
