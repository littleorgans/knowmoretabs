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

// --- X: manifest, id pin, xdom.js over the synthetic fixtures, background.js with a fake chrome.

const { createHash } = require('node:crypto');
const { parseHTML, composedPath } = require('./x_dom.cjs');

const manifest = JSON.parse(read('extension/manifest.json'));
const xdom = vm.runInNewContext(
  read('extension/xdom.js') + '\n({ X, canonicalPost, postIdentity, clickIntent, pageCounts, showsBookmarked, freshHealth, afterView, afterIntent, healthReport })',
  { URL },
);
const fixture = (name) => parseHTML(read(`tests/fixtures/x/${name}.html`));
const HOME = 'https://x.com/home';
const STATUS = 'https://x.com/example_author/status/102';
const article = (doc, name) => doc.querySelector(`article[data-fixture="${name}"]`);
const clickOn = (doc, name, control = 'bookmark', base = HOME) =>
  xdom.clickIntent(composedPath(article(doc, name).querySelector(`[data-testid="${control}"]`)), base);
const plain = (value) => JSON.parse(JSON.stringify(value));

test('the manifest asks for storage and X pages and nothing more', () => {
  assert.deepEqual(manifest.permissions, ['activeTab', 'storage']);
  assert.deepEqual(manifest.host_permissions, [`${LIBRARY}*`]);
  assert.deepEqual(manifest.content_scripts, [
    { matches: ['https://x.com/*', 'https://twitter.com/*'], js: ['xdom.js', 'x.js'], run_at: 'document_start' },
  ]);
});

test("the extension id the manifest key pins is the origin serve accepts", () => {
  const der = Buffer.from(manifest.key, 'base64');
  const id = [...createHash('sha256').update(der).digest('hex').slice(0, 32)].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join('');
  const declared = /^pub\(crate\) const EXTENSION_ORIGIN: &str = "chrome-extension:\/\/([a-p]{32})";$/m.exec(read('src/http.rs'));
  assert.ok(declared, 'EXTENSION_ORIGIN in src/http.rs');
  assert.equal(declared[1], id);
});

test('an add click names the post that owns the button, an inner icon included', () => {
  const doc = fixture('timeline');
  const icon = doc.querySelector('path[data-fixture="icon"]');
  assert.deepEqual(plain(xdom.clickIntent(composedPath(icon), HOME)), {
    kind: 'add', handle: 'example_author', id: '100', post: 'https://x.com/example_author/status/100',
  });
  assert.deepEqual(plain(xdom.clickIntent(composedPath(icon), 'https://twitter.com/home')).post, 'https://x.com/example_author/status/100');
});

test('a quote before the timestamp is not the post; a media suffix is dropped', () => {
  assert.deepEqual(plain(clickOn(fixture('timeline'), 'quote-first')), {
    kind: 'add', handle: 'example_writer', id: '200', post: 'https://x.com/example_writer/status/200',
  });
});

test('removing a bookmark, or any other click, is no intent', () => {
  const doc = fixture('timeline');
  assert.deepEqual(plain(clickOn(doc, 'bookmarked', 'removeBookmark')), { kind: 'remove' });
  assert.deepEqual(plain(clickOn(doc, 'plain', 'share')), { kind: 'none' });
  assert.deepEqual(plain(xdom.clickIntent(composedPath(doc.querySelector('[role="menuitem"]')), HOME)), { kind: 'none' });
});

test('a post whose permalink and author disagree, or that has no permalink, is not named', () => {
  const doc = fixture('timeline');
  for (const name of ['mismatch', 'no-time']) {
    assert.deepEqual(plain(clickOn(doc, name)), { kind: 'add', missing: 'missing_identity' }, name);
  }
});

test('a reply clicked while another post is focal is the reply', () => {
  const doc = fixture('status');
  assert.equal(clickOn(doc, 'reply', 'bookmark', STATUS).post, 'https://x.com/example_reader/status/104');
  assert.equal(clickOn(doc, 'self-reply', 'bookmark', STATUS).post, 'https://x.com/example_author/status/103');
  assert.deepEqual(plain(clickOn(doc, 'focal', 'removeBookmark', STATUS)), { kind: 'remove' });
});

test('a permalink is a post on X with its handle, kept on x.com', () => {
  assert.equal(xdom.canonicalPost('/Example_Author/status/7/video/1', HOME).post, 'https://x.com/Example_Author/status/7');
  for (const href of ['/i/status/7', '/example_author', '/example_author/status/x', 'https://example.org/a/status/7', 'http://x.com/a/status/7', '/sixteen_letters_/status/7', '/a/status/12345678901234567890']) {
    assert.equal(xdom.canonicalPost(href, HOME), null, href);
  }
});

test('a page counts its posts and those holding a bookmark button; a flip shows as saved', () => {
  assert.deepEqual(plain(xdom.pageCounts(fixture('timeline'))), { posts: 5, controls: 5 });
  const status = fixture('status');
  assert.deepEqual(plain(xdom.pageCounts(status)), { posts: 5, controls: 4 });
  assert.equal(xdom.showsBookmarked(status, '102', STATUS), true);
  assert.equal(xdom.showsBookmarked(status, '101', STATUS), false);
});

test('health turns blind after three busy views without a button, and back on seeing one', () => {
  const t = (n) => `2026-10-27T09:00:0${n}Z`;
  let h = xdom.freshHealth(t(0));
  h = xdom.afterView(h, { posts: 10, controls: 0 }, t(1));
  h = xdom.afterView(h, { posts: 1, controls: 0 }, t(2)); // too few posts to say anything
  h = xdom.afterView(h, { posts: 10, controls: 0 }, t(3));
  assert.equal(h.state, 'ok');
  h = xdom.afterView(h, { posts: 10, controls: 0 }, t(4));
  assert.deepEqual([h.state, h.since], ['blind', t(4)]);
  h = xdom.afterView(h, { posts: 10, controls: 0 }, t(5));
  assert.equal(h.since, t(4), 'a held state keeps its time');
  h = xdom.afterView(h, { posts: 10, controls: 9 }, t(6));
  assert.deepEqual([h.state, h.since], ['ok', t(6)]);
});

test('health turns unconfirmed after three add clicks X never showed as saved', () => {
  const now = '2026-10-27T09:00:00Z';
  let h = xdom.freshHealth(now);
  for (const confirmed of [false, false, true, false, false]) h = xdom.afterIntent(h, confirmed, now);
  assert.equal(h.state, 'ok');
  h = xdom.afterIntent(h, false, now);
  assert.equal(h.state, 'unconfirmed');
  h = xdom.afterIntent(h, true, now);
  assert.equal(h.state, 'ok');
  assert.deepEqual(Object.keys(plain(xdom.healthReport(h, '0.2.0'))), ['state', 'since', 'version', 'views', 'posts', 'controls', 'intents', 'confirmed']);
});

// The content script with the fixture DOM, a mutation observer and a deterministic clock.
function contentScript(doc) {
  const messages = [];
  const listeners = {};
  const timers = new Map();
  let now = 0;
  let timer = 0;
  let changed;
  doc.documentElement = doc.children[0];
  doc.addEventListener = (kind, listener) => (listeners[kind] = listener);
  vm.runInNewContext(read('extension/xdom.js') + '\n' + read('extension/x.js'), {
    URL,
    document: doc,
    location: { href: HOME },
    Date: { now: () => now },
    chrome: { runtime: { sendMessage: async (message) => messages.push(plain(message)) } },
    MutationObserver: class {
      constructor(callback) { changed = callback; }
      observe() {}
    },
    setTimeout: (callback, delay) => { timers.set(++timer, { callback, at: now + delay }); return timer; },
    clearTimeout: (id) => timers.delete(id),
  });
  return {
    messages,
    changed: () => changed(),
    click: (node, isTrusted) => listeners.click({ isTrusted, composedPath: () => composedPath(node) }),
    advance: (ms) => {
      const end = now + ms;
      for (;;) {
        const next = [...timers].filter(([, task]) => task.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        const [id, task] = next;
        now = task.at;
        timers.delete(id);
        task.callback();
      }
      now = end;
    },
  };
}

test('health waits for posts when the page shell settles before the feed arrives', () => {
  const doc = parseHTML('<main></main>');
  const script = contentScript(doc);
  script.changed();
  script.advance(2500);
  doc.querySelector('main').append(article(fixture('timeline'), 'plain'));
  script.changed();
  script.advance(2000);
  assert.deepEqual(script.messages, [{ kind: 'view', posts: 1, controls: 1 }]);
  script.changed();
  script.advance(2000);
  assert.equal(script.messages.length, 1, 'a rendered view is counted once');
});

test('the content script saves trusted add clicks and watches the same post for confirmation', () => {
  const doc = fixture('timeline');
  const script = contentScript(doc);
  const button = article(doc, 'plain').querySelector('[data-testid="bookmark"]');
  script.click(button, false);
  script.click(article(doc, 'bookmarked').querySelector('[data-testid="removeBookmark"]'), true);
  assert.deepEqual(script.messages, []);
  script.click(button, true);
  assert.deepEqual(script.messages, [{ kind: 'bookmark', post: 'https://x.com/example_author/status/100' }]);
  script.advance(250);
  assert.equal(script.messages.length, 1, 'another post already bookmarked does not confirm this one');
  button.attributes.set('data-testid', 'removeBookmark');
  script.advance(250);
  assert.deepEqual(script.messages[1], { kind: 'intent', confirmed: true });
});

// background.js in its own context with a fake chrome: storage in memory, fetch recorded.
function worker({ down = false, store = {}, fetchRequest } = {}) {
  const fetched = [];
  let onMessage;
  const chrome = {
    runtime: { id: 'self', onMessage: { addListener: (f) => (onMessage = f) }, getManifest: () => ({ version: manifest.version }) },
    action: { onClicked: { addListener() {} } },
    storage: {
      local: {
        get: async (keys) => Object.fromEntries([keys].flat().filter((k) => k in store).map((k) => [k, structuredClone(store[k])])),
        set: async (items) => Object.assign(store, structuredClone(items)),
      },
    },
  };
  const context = vm.createContext({ chrome, URL, structuredClone, console });
  context.fetch = async (url, init) => {
    if (fetchRequest) return fetchRequest(url, init);
    if (context.down) throw new TypeError('Failed to fetch');
    fetched.push({ url, body: JSON.parse(init.body), type: init.headers['Content-Type'] });
    return { ok: true };
  };
  context.down = down;
  context.importScripts = (...files) => files.forEach((file) => vm.runInContext(read(`extension/${file}`), context));
  vm.runInContext(read('extension/background.js'), context);
  const tab = { id: 1 };
  return {
    store, fetched, context,
    send: async (message, sender = { id: 'self', tab, url: HOME }) => {
      onMessage(message, sender);
      await vm.runInContext("typeof work === 'undefined' ? chain : work.storage", context);
      await vm.runInContext("typeof work === 'undefined' ? chain : work.network", context);
    },
  };
}

const POST = 'https://x.com/example_author/status/100';

test('the worker posts a named post to the Library and nothing else', async () => {
  const w = worker();
  await w.send({ kind: 'bookmark', post: POST });
  assert.deepEqual(w.fetched, [{ url: `${LIBRARY}api/x/bookmark`, body: { post: POST }, type: 'application/json' }]);
});

test('a message from another extension, a page off X or of the wrong shape is dropped', async () => {
  const w = worker();
  for (const sender of [
    { id: 'other', tab: { id: 1 }, url: HOME },
    { id: 'self', url: HOME },
    { id: 'self', tab: { id: 1 }, url: 'https://example.org/' },
    { id: 'self', tab: { id: 1 }, url: 'https://x.com.example.org/' },
  ]) {
    await w.send({ kind: 'bookmark', post: POST }, sender);
  }
  for (const message of [
    null, 'bookmark', { kind: 'bookmark' }, { kind: 'bookmark', post: 'https://example.org/a/status/1' },
    { kind: 'bookmark', post: `${POST}?s=20` }, { kind: 'bookmark', post: 'https://twitter.com/example_author/status/100' },
    { kind: 'view', posts: -1, controls: 0 }, { kind: 'view', posts: 1, controls: 2 }, { kind: 'intent', confirmed: 'yes' }, { kind: 'fetch', url: 'https://example.org/' },
  ]) {
    await w.send(message);
  }
  assert.deepEqual(w.fetched, []);
  assert.deepEqual(w.store, {});
});

test('saves wait while the Library is down, at most 50 for 7 days, and go with the next', async () => {
  const w = worker({ down: true });
  for (let i = 1; i <= 52; i += 1) await w.send({ kind: 'bookmark', post: `https://x.com/example_author/status/${i}` });
  assert.equal(w.store.queue.length, 50);
  assert.equal(w.store.queue[0].post, 'https://x.com/example_author/status/3', 'the oldest go first');
  w.store.queue[0].at -= 8 * 24 * 60 * 60 * 1000;
  w.context.down = false;
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  const saved = w.fetched.filter((f) => f.url.endsWith('/api/x/bookmark')).map((f) => f.body.post);
  assert.equal(saved.length, 49, 'one was older than 7 days');
  assert.equal(saved[0], 'https://x.com/example_author/status/4');
  assert.deepEqual(w.store.queue, []);
});

test('a save survives a worker restart while its request is pending', async () => {
  let started;
  const fetching = new Promise((resolve) => (started = resolve));
  let finish;
  const w = worker({ fetchRequest: () => {
    started();
    return new Promise((resolve) => (finish = resolve));
  } });
  const sending = w.send({ kind: 'bookmark', post: POST });
  await fetching;
  const stored = structuredClone(w.store);
  finish({ ok: true });
  await sending;

  assert.equal(stored.queue?.length, 1, 'the click is stored before the request finishes');
  const restarted = worker({ store: stored });
  await restarted.send({ kind: 'view', posts: 4, controls: 4 });
  assert.equal(restarted.fetched.filter((f) => f.url.endsWith('/api/x/bookmark')).length, 1);
  assert.deepEqual(restarted.store.queue, []);
});

test('new clicks reach storage while a previous save waits for the Library', async () => {
  let started;
  const fetching = new Promise((resolve) => (started = resolve));
  let finish;
  let requests = 0;
  const w = worker({ fetchRequest: () => {
    if (++requests !== 1) return { ok: true };
    started();
    return new Promise((resolve) => (finish = resolve));
  } });
  const first = w.send({ kind: 'bookmark', post: POST });
  await fetching;
  const second = w.send({ kind: 'bookmark', post: 'https://x.com/example_author/status/101' });
  // Allow storage operations to finish without waiting for the network.
  await new Promise((resolve) => setImmediate(resolve));
  const queued = structuredClone(w.store.queue);
  finish({ ok: true });
  await Promise.all([first, second]);
  assert.deepEqual(queued.map((item) => item.post), [POST, 'https://x.com/example_author/status/101']);
  assert.deepEqual(w.store.queue, []);
});

test('each answered save is removed before the next request starts', async () => {
  const queued = [POST, 'https://x.com/example_author/status/101'];
  const store = { queue: queued.map((post) => ({ post, at: Date.now() })) };
  let started;
  const fetching = new Promise((resolve) => (started = resolve));
  let finish;
  let requests = 0;
  const w = worker({ store, fetchRequest: () => {
    requests += 1;
    if (requests !== 2) return { ok: true };
    started();
    return new Promise((resolve) => (finish = resolve));
  } });
  const sending = w.send({ kind: 'bookmark', post: 'https://x.com/example_author/status/102' });
  await fetching;
  const kept = structuredClone(w.store.queue);
  finish({ ok: true });
  await sending;
  assert.deepEqual(kept.map((item) => item.post), [queued[1], 'https://x.com/example_author/status/102']);
});

test('health is reported on a change and at most once a day while it holds', async () => {
  const w = worker();
  const reports = () => w.fetched.filter((f) => f.url.endsWith('/api/x/health')).map((f) => f.body);
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.deepEqual(reports().map((r) => [r.state, r.version, r.views]), [['ok', manifest.version, 1]]);
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.equal(reports().length, 1, 'unchanged within the day');
  for (let i = 0; i < 3; i += 1) await w.send({ kind: 'view', posts: 5, controls: 0 });
  assert.deepEqual(reports().map((r) => r.state), ['ok', 'blind']);
  for (const report of reports()) assert.deepEqual(Object.keys(report), ['state', 'since', 'version', 'views', 'posts', 'controls', 'intents', 'confirmed']);
});
