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
  read('extension/xdom.js') + '\n({ X, canonicalPost, postIdentity, clickIntent, pageRead, pageCounts, showsBookmarked, freshHealth, afterView, afterIntent, healthReport })',
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

// The page read for a bookmark click on post `name` of fixture `doc`, on the page at `base`.
const readOn = (doc, name, base) => plain(xdom.pageRead(composedPath(article(doc, name).querySelector('[data-testid="bookmark"]')), base));
const ids = (read) => read.posts.map((post) => post.id);
const THREAD = 'https://x.com/example_author/status/301';

// A conversation of cells: `example_author` posts by number, `@<handle>:<id>` another's post,
// `heading` and `more` the boundaries a page shows.
function conversation(cells) {
  const cell = (spec) => {
    if (spec === 'heading') return '<div data-testid="cellInnerDiv"><h2 role="heading">More From This Author</h2></div>';
    if (spec === 'more') return '<div data-testid="cellInnerDiv"><div role="button" tabindex="0">Show more replies</div></div>';
    const [handle, id] = String(spec).startsWith('@') ? spec.slice(1).split(':') : ['example_author', spec];
    return `<div data-testid="cellInnerDiv"><article data-testid="tweet" data-fixture="p${id}">
      <div data-testid="User-Name"><a href="/${handle}">Example</a></div>
      <a href="/${handle}/status/${id}"><time datetime="2026-01-01T10:00:00Z">10:00</time></a>
      <div data-testid="tweetText" lang="en">Post ${id}.</div>
      <button data-testid="bookmark">Bookmark</button></article></div>`;
  };
  return parseHTML(`<main><section>${cells.map(cell).join('')}</section></main>`);
}

test("a read on a post's own page is its author's run: ancestors kept, another author stops it", () => {
  const read = readOn(fixture('status'), 'ancestor', 'https://x.com/example_author/status/102');
  assert.equal(read.where, 'status');
  assert.equal(read.clicked, '101');
  assert.deepEqual(ids(read), ['101', '102', '103'], 'the reply by someone else and the recommendation are not in it');
  assert.equal(read.posts[2].text_complete, false);
  assert.equal(read.posts[2].text, 'A visible preview of a longer self reply…');
  assert.deepEqual([read.complete, read.reasons], [false, ['text_truncated']], 'truncated text marks it incomplete');
});

test('a read keeps text as shown, image descriptions, a quote apart, and only full link targets', () => {
  const read = readOn(fixture('thread'), 'focal', THREAD);
  assert.deepEqual(ids(read), ['300', '301', '302'], 'the recommendation heading stops the run');
  assert.deepEqual(read.reasons, ['recommendation_boundary', 'unresolved_link']);
  assert.equal(read.complete, false);
  const [first, , last] = read.posts;
  assert.deepEqual(
    { name: first.name, at: first.at, lang: first.lang, text: first.text },
    { name: 'Example Author', at: '2026-01-02T10:00:00Z', lang: 'en', text: 'A thread in three posts.\nSecond line 🧵 and a link: example.org/full… and @example_quote' },
  );
  assert.deepEqual(first.alts, ['A synthetic diagram'], 'a generic "Image" and the quote\'s image are not its descriptions');
  assert.deepEqual(first.media, ['https://example.org/image.png', 'https://example.org/second.png']);
  assert.deepEqual(first.links, [{ href: 'https://t.co/EXAMPLE1', label: 'example.org/full…', target: 'https://example.org/full-path' }], 'a mention is not a link off X');
  assert.deepEqual(first.quote, { url: 'https://x.com/example_quote/status/900', handle: 'example_quote', text: 'Quoted text, separate from the owning post.' });
  assert.ok(!first.text.includes('Quoted text'));
  assert.deepEqual(last.links, [{ href: 'https://t.co/EXAMPLE2', label: 'example.org/another…', target: null }], 'a label cut short is no target');
});

test('a read in Home is the clicked post alone, said to be from a timeline', () => {
  const read = readOn(fixture('timeline'), 'plain', HOME);
  assert.deepEqual([read.where, read.complete, read.reasons, ids(read)], ['timeline', false, ['timeline'], ['100']]);
  assert.equal(read.posts[0].links[0].target, 'https://example.org/full-path');
  assert.equal(read.posts[0].quote.handle, 'example_quote');
});

test('a run ending at another author or the conversation end is complete; a fold is not', () => {
  const base = 'https://x.com/example_author/status/2';
  const read = (cells) => readOn(conversation(cells), 'p2', base);
  for (const cells of [[1, 2, 3, '@example_reader:9', 4], [1, 2, 3]]) {
    const whole = read(cells);
    assert.deepEqual([whole.complete, whole.reasons, ids(whole)], [true, [], ['1', '2', '3']], String(cells));
  }
  assert.deepEqual(read([1, 2, 'more', 3]).reasons, ['unexpanded_replies']);
  assert.deepEqual(read(['heading', 2]).reasons, [], 'a boundary before the run is not after it');
  assert.equal(xdom.pageRead(composedPath(conversation([1]).querySelector('article')), base), null, 'no button, no read');
});

test('a read sends at most 50 posts around the clicked one', () => {
  const many = Array.from({ length: 60 }, (_, i) => i + 1);
  const read = readOn(conversation(many), 'p55', 'https://x.com/example_author/status/55');
  assert.equal(read.posts.length, 50);
  assert.deepEqual([ids(read)[0], ids(read).at(-1), read.complete], ['6', '55', false]);
});

test('a recommendation heading beside a post stops the run in both directions', () => {
  const doc = conversation([1, 2, 3, 4]);
  const cell = article(doc, 'p3').closest('[data-testid="cellInnerDiv"]');
  cell.append(conversation(['heading']).querySelector('[role="heading"]'));
  const before = readOn(doc, 'p2', THREAD);
  assert.deepEqual(ids(before), ['1', '2']);
  assert.deepEqual(before.reasons, ['recommendation_boundary']);
  assert.equal(before.complete, false);
  const after = readOn(doc, 'p4', THREAD);
  assert.deepEqual(ids(after), ['4']);
  const clicked = readOn(doc, 'p3', THREAD);
  assert.deepEqual(ids(clicked), ['3'], 'a click beside the heading never joins the preceding thread');
  assert.deepEqual(clicked.reasons, ['recommendation_boundary']);
});

test('a heading outside a cell still separates the conversation from recommendations', () => {
  const doc = conversation([1, 2]);
  const rows = article(doc, 'p1').closest('[data-testid="cellInnerDiv"]').parentElement;
  rows.append(conversation(['heading']).querySelector('[role="heading"]'));
  rows.append(article(conversation([3]), 'p3').closest('[data-testid="cellInnerDiv"]'));
  const read = readOn(doc, 'p2', THREAD);
  assert.deepEqual(ids(read), ['1', '2']);
  assert.deepEqual(read.reasons, ['recommendation_boundary']);
  assert.deepEqual(ids(readOn(doc, 'p3', THREAD)), ['3']);
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
  const report = plain(xdom.healthReport(h, '0.2.0'));
  assert.deepEqual(Object.keys(report), ['state', 'since', 'version', 'views', 'posts', 'controls', 'intents', 'confirmed', 'dropped', 'last_dropped']);
  assert.deepEqual([report.dropped, report.last_dropped], [0, null], 'no drops');
  const drops = { count: 2, last: now };
  assert.deepEqual(plain(xdom.healthReport(h, '0.2.0', drops)).dropped, 2);
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
  assert.deepEqual(script.messages.map(({ kind, post, read }) => [kind, post, read.clicked]), [['bookmark', 'https://x.com/example_author/status/100', '100']]);
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
    return { ok: context.status < 400, status: context.status };
  };
  context.down = down;
  context.status = 200;
  context.importScripts = (...files) => files.forEach((file) => vm.runInContext(read(`extension/${file}`), context));
  vm.runInContext(read('extension/background.js'), context);
  const tab = { id: 1 };
  return {
    store, fetched, context,
    send: async (message, sender = { id: 'self', tab, url: HOME }) => {
      onMessage(message, sender);
      await vm.runInContext('work', context);
    },
  };
}

const POST = 'https://x.com/example_author/status/100';

test('the worker posts a named post to the Library and nothing else', async () => {
  const w = worker();
  await w.send({ kind: 'bookmark', post: POST });
  assert.deepEqual(w.fetched, [{ url: `${LIBRARY}api/x/bookmark`, body: { post: POST }, type: 'application/json' }]);
});

test('the worker sends the page read with the post, and leaves out a read of the wrong shape', async () => {
  const w = worker();
  const read = { where: 'status', clicked: '100', posts: [] };
  await w.send({ kind: 'bookmark', post: POST, read });
  await w.send({ kind: 'bookmark', post: POST, read: ['not', 'a', 'read'] });
  assert.deepEqual(w.fetched.map((f) => f.body), [{ post: POST, read }, { post: POST }]);
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

const saved = (w) => w.fetched.filter((f) => f.url.endsWith('/api/x/bookmark')).map((f) => f.body.post);
const reports = (w) => w.fetched.filter((f) => f.url.endsWith('/api/x/health')).map((f) => f.body);
const DAY = 24 * 60 * 60 * 1000;

test('a save the Library does not answer is dropped and counted, and nothing of the post is kept', async () => {
  const w = worker({ down: true });
  await w.send({ kind: 'bookmark', post: POST });
  await w.send({ kind: 'bookmark', post: 'https://x.com/example_author/status/101' });
  assert.equal(w.store.drops.count, 2);
  assert.ok(Date.now() - Date.parse(w.store.drops.last) < 1000, 'the time of the last drop');
  assert.deepEqual(Object.keys(w.store).sort(), ['drops', 'health']);
  assert.doesNotMatch(JSON.stringify(w.store), /example_author|status/, 'no address of a post');
  w.context.down = false;
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.deepEqual(saved(w), [], 'a dropped save is never sent again');
  w.store.drops.last = new Date(Date.now() - 15 * DAY).toISOString();
  w.context.down = true;
  await w.send({ kind: 'bookmark', post: POST });
  assert.equal(w.store.drops.count, 1, 'a drop long after the last starts a new count');
});

test('a 5xx drops the save and counts it; a refusal ends it uncounted', async () => {
  const w = worker();
  w.context.status = 500;
  await w.send({ kind: 'bookmark', post: POST });
  assert.equal(w.store.drops.count, 1);
  w.context.status = 400;
  await w.send({ kind: 'bookmark', post: 'https://x.com/example_author/status/101' });
  w.context.status = 200;
  await w.send({ kind: 'bookmark', post: 'https://x.com/example_author/status/102' });
  assert.equal(w.store.drops.count, 1, 'only the 5xx counts');
  assert.equal(saved(w).length, 3, 'each save is sent once');
});

test('health is reported on a change and at most once a day while it holds', async () => {
  const w = worker();
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.deepEqual(reports(w).map((r) => [r.state, r.version, r.views]), [['ok', manifest.version, 1]]);
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.equal(reports(w).length, 1, 'unchanged within the day');
  for (let i = 0; i < 3; i += 1) await w.send({ kind: 'view', posts: 5, controls: 0 });
  assert.deepEqual(reports(w).map((r) => r.state), ['ok', 'blind']);
  for (const report of reports(w)) {
    assert.deepEqual(Object.keys(report), ['state', 'since', 'version', 'views', 'posts', 'controls', 'intents', 'confirmed', 'dropped', 'last_dropped']);
  }
});

test('the next health report carries the drops, sent again when they change', async () => {
  const w = worker();
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.deepEqual([reports(w)[0].dropped, reports(w)[0].last_dropped], [0, null]);
  w.context.down = true;
  await w.send({ kind: 'bookmark', post: POST });
  w.context.down = false;
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  const report = reports(w).at(-1);
  assert.equal(reports(w).length, 2, 'a drop is a change');
  assert.deepEqual([report.state, report.dropped, report.last_dropped], ['ok', 1, w.store.drops.last]);
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.equal(reports(w).length, 2, 'unchanged within the day');
});

test('a refused health report leaves drops to report on the next page view', async () => {
  const w = worker();
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  const accepted = structuredClone(w.store.reported);
  w.context.status = 503;
  await w.send({ kind: 'bookmark', post: POST });
  assert.equal(w.store.drops.count, 1);
  assert.deepEqual(w.store.reported, accepted, 'only an accepted report updates reported');
  w.context.status = 200;
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.deepEqual(reports(w).map((report) => report.dropped), [0, 1, 1]);
  assert.equal(w.store.reported.lastDropped, w.store.drops.last);
  await w.send({ kind: 'view', posts: 4, controls: 4 });
  assert.equal(reports(w).length, 3, 'accepted reports are suppressed until the next change');
  assert.equal(saved(w).length, 1, 'only the health report is attempted again');
});
