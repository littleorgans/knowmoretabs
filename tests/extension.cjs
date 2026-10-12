// Run with: node --test tests/extension.cjs
// The toolbar extension's link building, loaded the way its service worker loads it.
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

const read = (path) => readFileSync(join(__dirname, '..', path), 'utf8');
const { addLink, LIBRARY, PORT } = vm.runInNewContext(read('extension/link.js') + '\n({ addLink, LIBRARY, PORT })');

test('a web page opens Add link with its address and title encoded', () => {
  assert.equal(
    addLink('https://synthetic.test/a b?x=1&y=2#top', 'Tea & toast: 50% off'),
    'http://127.0.0.1:41878/#add=https%3A%2F%2Fsynthetic.test%2Fa%20b%3Fx%3D1%26y%3D2%23top&title=Tea%20%26%20toast%3A%2050%25%20off',
  );
  assert.equal(addLink('http://synthetic.test/', 'Ünïcode ✓'), `${LIBRARY}#add=http%3A%2F%2Fsynthetic.test%2F&title=%C3%9Cn%C3%AFcode%20%E2%9C%93`);
});

test('a page without a title sends no title', () => {
  assert.equal(addLink('https://synthetic.test/', ''), `${LIBRARY}#add=https%3A%2F%2Fsynthetic.test%2F`);
  assert.equal(addLink('https://synthetic.test/'), `${LIBRARY}#add=https%3A%2F%2Fsynthetic.test%2F`);
});

test('a page that is not on the web opens Add link empty', () => {
  for (const url of ['chrome://newtab/', 'file:///synthetic/page.html', 'about:blank', 'data:text/plain,x', '', undefined]) {
    assert.equal(addLink(url, 'Synthetic'), `${LIBRARY}#add`, String(url));
  }
});

test("the Library's port is the one `just up` serves and the manifest may reach", () => {
  assert.equal(LIBRARY, `http://127.0.0.1:${PORT}/`);
  assert.match(read('justfile'), new RegExp(`^lib_port := "${PORT}"$`, 'm'));
  assert.deepEqual(JSON.parse(read('extension/manifest.json')).host_permissions, [`${LIBRARY}*`]);
});
