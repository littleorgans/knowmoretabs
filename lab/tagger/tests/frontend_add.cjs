// The Add link screen's scripts with a small DOM and controlled API round trips.
const assert = require('node:assert/strict');
const { lit, setup, turn, plain, run } = require('./frontend_dom.cjs');

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
  const { K, node, context } = setup({ lit, strip: { render() {}, sayIn: () => () => {} }, lib: { app_tags: [] } }, 'panel.js', 'add.js');
  const asked = [];
  let reply = () => new Promise(() => {});
  K.api = (path, body) => { asked.push([path, body]); return reply(path, body); };
  for (const k of ['library', 'content', 'image', 'search']) node(`a-${k}`).querySelector = () => node(`a-${k}-val`);
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
  reply = (path) => Promise.resolve(path === '/api/library' ? { app_tags: [] } : done);
  node('a-link').value = '';   // another link in the box would be its draft, and the bar would wait for it
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

/* the panel's letter rows: "#" first, letters without tags left out, case and accents folded; typing filters every
   tag and opens the fold, Esc and the fold put it back */
async function panelLetters() {
  const { K, node } = setup({ lit, lib: { app_tags: ['zeta', 'Alpha', 'apple', 'Éclair', 'eagle', 'iOS', '3D print', '#hash', 'straße'] } },
    'strip.js', 'panel.js');
  node('a-fold').setAttribute = function (k, v) { this[k] = v; };
  const picked = [];
  K.panel.render({ has: (t) => t === 'Alpha', pend: (t) => t === 'eagle', pick: (t) => picked.push(t), sugg: [], recent: [] });
  const rows = () => [...node('a-rows').innerHTML.matchAll(/<span class="ll"[^>]*>([^<]*)<\/span><ul class="cells">(.*?)<\/ul>/g)]
    .map(([, k, cells]) => [k, [...cells.matchAll(/data-tag="([^"]*)"/g)].map((m) => m[1])]);
  const folded = () => [node('a-fold')['aria-expanded'], node('a-rows').hidden];
  const type = (value) => { node('a-tag').value = value; node('a-tag').input(); };
  const esc = () => { const e = { key: 'Escape', stopped: false, preventDefault() {}, stopPropagation() { this.stopped = true; } }; node('a-tag').keydown(e); return e.stopped; };
  assert.equal(node('a-fold').textContent, 'All 9 tags');
  assert.deepEqual(folded(), ['false', true], 'the fold starts closed');
  assert.equal(node('a-rows').innerHTML, '');
  node('a-fold').click();
  assert.deepEqual(folded(), ['true', false]);
  assert.deepEqual(rows(), [['#', ['#hash', '3D print']], ['A', ['Alpha', 'apple']], ['E', ['eagle', 'Éclair']], ['I', ['iOS']], ['S', ['straße']], ['Z', ['zeta']]],
    '"#" first, then A to Z by name; no B row');
  const html = node('a-rows').innerHTML;
  assert.match(html, /data-tag="Alpha" data-k="on" aria-pressed="true" title="Alpha"><span>Alpha<\/span>/, 'the page has it: on');
  assert.match(html, /data-tag="eagle" data-k="pend" aria-pressed="true"/, 'picked, not written: pending');
  assert.match(html, /data-tag="apple" data-k="off" aria-pressed="false"/);
  assert.doesNotMatch(html, /class="ct"|data-act|≈|×/, 'no count, no ×, no ≈');
  type('E');
  assert.deepEqual(rows(), [['A', ['apple']], ['E', ['eagle']], ['S', ['straße']], ['Z', ['zeta']]], 'the filter keeps the matching cells, any case');
  assert.equal(esc(), true, 'Esc clears the filter and stays in the box');
  assert.equal(node('a-tag').value, '');
  assert.equal(esc(), false, 'the next Esc is the shell\'s: it leaves the box');
  assert.equal(rows().length, 6, 'cleared: the fold as it was left, open');
  node('a-fold').click();
  assert.deepEqual(folded(), ['false', true]);
  type('ap');
  assert.deepEqual(folded(), ['true', false], 'typing opens the fold');
  assert.deepEqual(rows(), [['A', ['apple']]]);
  type('zz');
  assert.match(node('a-rows').innerHTML, /No tag has “zz”. Enter makes it./);
  node('a-fold').click();
  assert.deepEqual([node('a-tag').value, ...folded()], ['', 'false', true], 'the fold pressed while filtering clears and closes');
  node('a-fold').click();
  node('a-panel').click({ target: { closest: (s) => (s === 'button[data-tag]' ? { dataset: { tag: 'apple' } } : null) } });
  assert.deepEqual(picked, ['apple'], 'a cell picks its tag');
  type('ap');
  K.panel.reset();
  K.panel.render({ has: () => false, pend: () => false, pick() {}, sugg: [], recent: [] });
  assert.deepEqual([node('a-tag').value, ...folded()], ['', 'false', true], 'a new target: no filter, the fold closed');
}

/* ---- picks before the page has a row: the real pages, strip, panel and add scripts; the server is the test ---- */
const ev = (stage, state, extra = {}) => ({ stage, state, ...extra });
const STAGES = {
  running: { library: ev('library', 'running') },
  added: { library: ev('library', 'done', { value: 'added' }), content: ev('content', 'running', { tier: 'web' }) },
  image: { library: ev('library', 'done', { value: 'added' }), content: ev('content', 'done', { status: 'ok', tier: 'web' }), image: ev('image', 'running') },
  search: { library: ev('library', 'done', { value: 'added' }), content: ev('content', 'done', { status: 'ok', tier: 'web' }), image: ev('image', 'done', { status: 'ok' }),
    search: ev('search', 'running') },
};
const done = (value) => ({ ...STAGES.search, search: ev('search', 'done', { value }) });
const addPage = (row, url, tags = []) => ({ row, title: `Page ${row}`, host: 'added.example', url, image: null, own: [], tags });

function addFixture() {
  const f = setup({ lit, lib: { app_tags: ['Coffee', 'Night trains', 'Trains'], tags: [] }, views: { chip: (t) => `<li class="tag">${t}</li>` } },
    'pages.js', 'strip.js', 'panel.js', 'add.js');
  const { K, node, context } = f;
  K.changed = () => K.add.render();
  K.title = (p) => p.title;
  context.setTimeout = (fn) => setImmediate(fn);   // a poll's pause is a turn
  for (const k of ['library', 'content', 'image', 'search']) node(`a-${k}`).querySelector = () => node(`a-${k}-val`);
  const requests = (f.requests = []);
  K.api = (route, body) => new Promise((resolve, reject) => requests.push({ route, body: body && plain(body), resolve, reject }));
  f.next = async (route) => {   // the oldest request to `route`, once it is sent
    for (let i = 0; i < 50; i++) {
      const at = requests.findIndex((r) => r.route === route);
      if (at >= 0) return requests.splice(at, 1)[0];
      await turn();
    }
    throw new Error(`no request to ${route}; sent: ${requests.map((r) => r.route).join(', ')}`);
  };
  f.reply = async (route, out) => { const r = await f.next(route); r.resolve(out); for (let i = 0; i < 4; i++) await turn(); return r; };
  f.job = (id, url, stages, extra = {}) => ({ id, url, action: 'add', retry: [], signed_in: false, title: null, restored: false, failed: false,
    finished: false, seconds: 0.5, page: null, stages, ...extra });
  f.type = (link) => { node('a-link').value = link; node('a-link').input(); };
  f.submit = () => node('a-form').submit({ preventDefault() {} });
  f.click = (tag) => node('a-panel').click({ target: { closest: (s) => (s === 'button[data-tag]' ? { dataset: { tag } } : null) } });
  f.look = (tag) => (node('a-rows').innerHTML.match(new RegExp(`data-tag="${tag}" data-k="(\\w+)"`)) || [])[1];
  f.applies = () => requests.filter((r) => r.route === '/api/apply').map((r) => r.body);
  f.filter = (text) => { node('a-tag').value = text; node('a-tag').input(); node('a-filter').submit({ preventDefault() {} }); };
  let batch = 0;
  f.applied = async (row, tags) => {   // the server takes each apply in turn
    const out = [];
    for (const tag of tags) {
      const r = await f.next('/api/apply');
      out.push(r.body);
      r.resolve({ pages: [{ row, tags: K.pages.get(row).tags }], changed: 1, batch: ++batch, app_tags: K.lib.app_tags });
      await turn(); await turn();
    }
    return out;
  };
  return f;
}

/* picks on a link before Add travel with it; none is written before the page has a row, then one change: an apply a
   tag for that row, one toast, and its Undo retracts every batch */
async function addPicksHeld() {
  const f = addFixture(), { K, node, requests } = f, url = 'https://added.example/one';
  f.type(url);
  node('a-fold').click();
  assert.equal(node('a-result').hidden, false, 'a web address shows the panel');
  assert.match(node('a-tile').innerHTML, /class="ht link">https:\/\/added.example\/one<\/h3><p class="host">added.example/, 'beside a placeholder tile');
  f.click('Trains'); f.click('Coffee'); f.click('Night trains'); f.click('Night trains');
  assert.deepEqual([f.look('Trains'), f.look('Coffee'), f.look('Night trains')], ['pend', 'pend', 'off'], 'striped; a second click unpicks');
  assert.equal(requests.length, 0, 'nothing is sent for a link not yet added');
  f.submit();
  const add = await f.next('/api/add');
  assert.deepEqual(add.body, { url, action: 'add', title: null });
  add.resolve(f.job(1, url, STAGES.running));
  for (const stages of [STAGES.running, STAGES.added, STAGES.image, STAGES.search]) {
    await turn(); await turn();
    assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['pend', 'pend'], 'still striped');
    await f.reply('/api/add/1', f.job(1, url, stages));
    assert.deepEqual(f.applies(), [], 'no write during Library, Content, Image or Search');
  }
  assert.equal(node('a-link').value, '', 'listed: the box is ready for the next link');
  await f.reply('/api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['on', 'on'], 'Indexed: on at once');
  assert.deepEqual(await f.applied(7, ['Trains', 'Coffee']), [{ rows: [7], tag: 'Trains', value: true }, { rows: [7], tag: 'Coffee', value: true }], 'one apply a tag, for its row');
  await f.reply('/api/library', { ...K.lib });
  assert.equal(K.toasted, 'Added “Trains”, “Coffee” to “Page 7”');
  K.undo();
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['off', 'off'], 'Undo takes both off');
  const undone = [];
  for (let i = 0; i < 2; i++) {
    const r = await f.next('/api/undo');
    undone.push(r.body);
    r.resolve({ pages: [{ row: 7, tags: [] }], app_tags: K.lib.app_tags });
  }
  assert.deepEqual(undone, [{ batch: 1 }, { batch: 2 }], 'every batch retracted');
}

/* a second link does not stop the first job: it writes the first page's picks to the first page's row only */
async function addPicksBackground() {
  const f = addFixture(), { node } = f, one = 'https://added.example/one', two = 'https://added.example/two';
  f.type(one); f.click('Trains'); f.submit();
  await f.reply('/api/add', f.job(1, one, STAGES.running));
  await f.reply('/api/add/1', f.job(1, one, STAGES.added));
  f.type(two);
  assert.equal(f.look('Trains'), undefined, 'a new link: the fold closes for it');
  node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'and the first page\'s picks are not its');
  f.click('Coffee'); f.submit();
  await f.reply('/api/add', f.job(2, two, STAGES.running));
  await f.reply('/api/add/1', f.job(1, one, STAGES.search));
  await f.reply('/api/add/1', f.job(1, one, done('indexed'), { finished: true, page: addPage(7, one) }));
  assert.deepEqual(await f.applied(7, ['Trains']), [{ rows: [7], tag: 'Trains', value: true }], 'the first job writes its own picks');
  assert.match(node('a-tile').innerHTML, /added.example\/two/, 'the second page stays on screen');
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['off', 'pend'], 'its cells untouched');
  await f.reply('/api/add/2', f.job(2, two, done('indexed'), { finished: true, page: addPage(8, two) }));
  assert.deepEqual(await f.applied(8, ['Coffee']), [{ rows: [8], tag: 'Coffee', value: true }]);
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['off', 'on']);
}

/* a refusal clears the picks, Not indexed keeps them for Retry, a superseded unindexed job drops them with a notice */
async function addPicksOutcomes() {
  const f = addFixture(), { K, node, requests } = f;
  const refusals = [['forgotten', { library: ev('library', 'done', { value: 'forgotten' }) }],
    ['not a web page', { library: ev('library', 'done', { value: 'refused', reason: 'not_web' }) }],
    ['snapshot', { library: ev('library', 'done', { value: 'refused', reason: 'snapshot' }) }]];
  let id = 0;
  for (const [name, stages] of refusals) {
    const url = `https://added.example/${name.replace(/ /g, '-')}`;
    f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
    await f.reply('/api/add', f.job(++id, url, STAGES.running));
    assert.equal(f.look('Trains'), 'pend', `${name}: held`);
    await f.reply(`/api/add/${id}`, f.job(id, url, stages, { finished: true }));
    if (name === 'forgotten') await f.reply('/api/library', { ...K.lib });
    assert.equal(node('a-result').hidden, true, `${name}: no tile, no panel`);
    K.add.start(url, { action: 'restore' });
    await f.reply('/api/add', f.job(++id, url, STAGES.running, { action: 'restore' }));
    assert.equal(f.look('Trains'), 'off', `${name}: the picks cleared`);
    await f.reply(`/api/add/${id}`, f.job(id, url, { library: ev('library', 'done', { value: 'refused', reason: 'snapshot' }) }, { finished: true }));
  }
  node('a-link').value = '';
  const lone = 'https://added.example/lone';
  f.type(lone); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('/api/add', f.job(++id, lone, STAGES.running));
  await f.reply(`/api/add/${id}`, f.job(id, lone, done('not_indexed'), { finished: true }));
  await f.reply('/api/library', { ...K.lib });
  assert.equal(f.look('Trains'), 'pend', 'Not indexed: the picks wait');
  assert.equal(node('a-act').textContent, 'Retry');
  node('a-act').click();
  const index = await f.next('/api/add');
  assert.deepEqual(index.body, { url: lone, action: 'index' });
  index.resolve(f.job(++id, lone, { library: ev('library', 'done', { value: 'known' }), search: ev('search', 'done', { value: 'indexed' }) },
    { action: 'index', finished: true, page: addPage(9, lone) }));
  assert.deepEqual(await f.applied(9, ['Trains']), [{ rows: [9], tag: 'Trains', value: true }], 'its Retry writes them');
  await f.reply('/api/library', { ...K.lib });
  const failed = 'https://added.example/failed';
  f.type(failed); node('a-fold').click(); f.click('Coffee'); f.submit();
  await f.reply('/api/add', f.job(++id, failed, STAGES.running, { failed: true, finished: true }));
  K.add.start(failed, { action: 'add' });
  await f.reply('/api/add', f.job(++id, failed, STAGES.running));
  assert.equal(f.look('Coffee'), 'pend', 'knowmoretabs could not run: kept while Retry is offered');
  await f.reply(`/api/add/${id}`, f.job(id, failed, done('indexed'), { finished: true, page: addPage(10, failed) }));
  assert.deepEqual(await f.applied(10, ['Coffee']), [{ rows: [10], tag: 'Coffee', value: true }]);
  await f.reply('/api/library', { ...K.lib });
  const late = 'https://added.example/late', next = 'https://added.example/next';
  f.type(late); node('a-fold').click(); f.click('Coffee'); f.submit();
  const lateId = ++id;
  await f.reply('/api/add', f.job(lateId, late, STAGES.running));
  f.type(next); node('a-fold').click();
  f.click('Trains'); f.click('Trains');
  assert.equal(f.look('Trains'), 'off', 'a pending pick clicked again is gone');
  f.submit();
  await f.reply('/api/add', f.job(++id, next, STAGES.running));
  await f.reply(`/api/add/${lateId}`, f.job(lateId, late, done('not_indexed'), { finished: true }));
  assert.equal(K.toasted, 'Tags for https://added.example/late not added: the page is not indexed', 'the dropped picks are told');
  await f.reply('/api/library', { ...K.lib });
  assert.equal(requests.filter((r) => r.route === '/api/add/' + lateId).length, 0, 'its poll stops');
  await f.reply(`/api/add/${id}`, f.job(id, next, done('indexed'), { finished: true, page: addPage(11, next) }));
  await f.reply('/api/library', { ...K.lib });
  assert.deepEqual(f.applies(), [], 'nothing written for the dropped or unpicked tags');
}

/* Enter in the filter: the exact match in any case, else the only match, else a new tag made and picked; with several
   matches and none exact, nothing */
async function addFilterEnter() {
  const f = addFixture(), { K, node, requests } = f, url = 'https://added.example/one';
  f.type(url); node('a-fold').click(); f.submit();
  await f.reply('/api/add', f.job(1, url, STAGES.running));
  f.filter('trains');
  assert.deepEqual([f.look('Trains'), node('a-tag').value], ['pend', ''], 'exact, any case: picked, the filter clears');
  f.filter('coff');
  assert.deepEqual([f.look('Coffee'), node('a-tag').value], ['pend', ''], 'the only match');
  f.filter('rain');
  assert.equal(node('a-tag').value, 'rain', 'several, none exact: nothing');
  assert.equal(f.look('Night trains'), 'off');
  f.filter('  Sleeper cars ');
  const made = await f.next('/api/tags');
  assert.deepEqual(made.body, { name: 'Sleeper cars' }, 'no match: the tag is made');
  made.resolve({ tag: 'Sleeper cars', created: true, tags: [] });
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(plain(K.lib.app_tags), ['Coffee', 'Night trains', 'Sleeper cars', 'Trains'], 'listed while the page waits for its row');
  assert.deepEqual([f.look('Sleeper cars'), node('a-tag').value], ['pend', ''], 'and picked');
  f.filter('trains');
  assert.equal(f.look('Trains'), 'off', 'Enter on a picked tag unpicks it');
  await f.reply('/api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Coffee', 'Sleeper cars'])).map((b) => b.tag), ['Coffee', 'Sleeper cars']);
  assert.equal(requests.filter((r) => r.route === '/api/tags').length, 0);
}

/* Draft picks belong to one address, including while the initial Add reply is delayed. */
async function addPicksAddress() {
  const f = addFixture(), { K, node } = f;
  const one = 'https://added.example/one', two = 'https://added.example/two';
  f.type(one); node('a-fold').click(); f.click('Trains');
  f.type(two); node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'editing to another address drops the old draft picks');
  f.click('Coffee'); f.submit();
  await f.reply('/api/add', f.job(1, two, done('indexed'), { finished: true, page: addPage(8, two) }));
  assert.deepEqual((await f.applied(8, ['Coffee'])).map((b) => b.tag), ['Coffee']);
  await f.reply('/api/library', { ...K.lib });
  assert.equal(K.has(8, 'Trains'), false, 'the other address never inherits the old picks');
}

async function addPicksBeforeReply() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/delayed';
  f.type(url); node('a-fold').click(); f.submit();
  const add = await f.next('/api/add');
  f.click('Trains');
  assert.equal(f.look('Trains'), 'pend', 'a pick while Add is waiting stays visible');
  f.type('https://added.example/next');
  add.resolve(f.job(1, url, STAGES.added));
  for (let i = 0; i < 4; i++) await turn();
  await f.reply('/api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).map((b) => b.tag), ['Trains'], 'the submitted address keeps the pick');
  await f.reply('/api/library', { ...K.lib });
  node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'the next draft does not inherit it');
}

/* An already sent poll must not consume picks owned by a newer job for the same address. */
async function addPicksStalePoll() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/retry';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('/api/add', f.job(1, url, STAGES.added));
  const old = await f.next('/api/add/1');
  K.add.start(url, { action: 'index' });
  await f.reply('/api/add', f.job(2, url, STAGES.added, { action: 'index' }));
  old.resolve(f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(f.applies(), [], 'the obsolete poll cannot write the newer job\'s picks');
  assert.equal(f.look('Trains'), 'pend', 'the newest job keeps its picks until Indexed');
  await f.reply('/api/add/2', f.job(2, url, done('indexed'), { action: 'index', finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).map((b) => b.tag), ['Trains'], 'only one write, from the latest job');
  await f.reply('/api/library', { ...K.lib });
  assert.deepEqual(f.applies(), []);
}

run({ addStates, addRequests, addSignedInStages, panelLetters, addPicksHeld, addPicksBackground, addPicksOutcomes, addFilterEnter, addPicksAddress, addPicksBeforeReply, addPicksStalePoll });
