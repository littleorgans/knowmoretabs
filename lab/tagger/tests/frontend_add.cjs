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
    ['Image error, why', [L('added'), C('ok', 'web'), ev('image', 'done', { status: 'error', reason: 'HTTP 503' }), S('indexed')], ['Added', 'Ok · Web', 'Error · HTTP 503', 'Indexed'], 'Retry'],
    ['Image unavailable, why', [L('known'), C('ok', 'web'), ev('image', 'done', { status: 'unavailable', reason: 'HTTP 503; failed on 3 runs' }), S('indexed')], ['Already in library', 'Ok · Web', 'Unavailable · HTTP 503; failed on 3 runs', 'Indexed'], null],
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
  const { K, node, context } = setup({ lit, strip: { render() {}, sayIn: () => () => {} }, lib: { app_tags: [] } }, 'pages.js', 'panel.js', 'add.js');
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
  assert.deepEqual(plain(asked.pop()), ['api/add', { url: link, action: 'add', title: 'One & two · é' }], 'the title goes with its link');
  node('a-link').value = 'https://added.example/other';
  submit();
  assert.deepEqual(plain(asked.pop()), ['api/add', { url: 'https://added.example/other', action: 'add', title: null }], 'never with another link');
  context.location.hash = '#add=https://added.example/raw?a=1&b=2';
  K.add.enter();
  submit();
  assert.deepEqual(plain(asked.pop()), ['api/add', { url: 'https://added.example/raw?a=1&b=2', action: 'add', title: null }], 'a link without a title keeps its &');
  const ev = (stage, state, extra = {}) => ({ stage, state, ...extra });
  const done = { id: 1, url: link, key: link, action: 'add', retry: [], signed_in: false, title: null, restored: false, failed: false, finished: true, seconds: 1.2, page: null,
    stages: { library: ev('library', 'done', { value: 'known' }), content: ev('content', 'done', { status: 'off', tier: 'signed_in' }),
      image: ev('image', 'done', { status: 'error' }), search: ev('search', 'done', { value: 'indexed' }) } };
  reply = (path) => Promise.resolve(path === 'api/library' ? { app_tags: [] } : done);
  node('a-link').value = '';   // another link in the box would be its draft, and the bar would wait for it
  await K.add.start(link, { action: 'add', signed_in: true });
  await turn();
  assert.equal(K.failure, undefined);
  assert.equal(node('a-act').textContent, 'Retry');
  asked.length = 0;
  node('a-act').click();
  assert.deepEqual(plain(asked[0]), ['api/add', { url: link, action: 'add', retry: ['content', 'image'], signed_in: true }], 'Retry starts the failed stages');
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
  K.panel.render({ has: (t) => t === 'Alpha', pend: (t) => t === 'eagle', pick: (t) => picked.push(t), sugg: [] });
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
  K.panel.render({ has: () => false, pend: () => false, pick() {}, sugg: [] });
  assert.deepEqual([node('a-tag').value, ...folded()], ['', 'false', true], 'a new target: no filter, the fold closed');
}

/* Recently used: the library's order, as many as the line has columns, each cell as its twin in the letter rows */
async function panelRecent() {
  const { K, node } = setup({ lit, lib: { app_tags: ['Alpha', 'apple', 'eagle', 'iOS', 'zeta'], recent: ['zeta', 'Alpha', 'eagle', 'apple', 'iOS'] } },
    'strip.js', 'panel.js');
  const picked = [];
  const recent = () => [...node('a-recent').innerHTML.matchAll(/data-tag="([^"]*)"/g)].map((m) => m[1]);
  const suggested = () => [...node('a-sugg').innerHTML.matchAll(/data-tag="([^"]*)"/g)].map((m) => m[1]);
  const button = (id, t) => node(id).innerHTML.match(new RegExp(`<button[^>]*data-tag="${t}"[^>]*>`))[0];
  node('a-recent').columns = '136px 136px 136px';
  K.panel.render({ has: (t) => t === 'Alpha', pend: (t) => t === 'eagle', pick: (t) => picked.push(t), sugg: K.lib.app_tags });
  assert.deepEqual(recent(), ['zeta', 'Alpha', 'eagle'], 'latest first, three columns, three cells');
  assert.deepEqual(suggested(), ['Alpha', 'apple', 'eagle'], 'Suggested shares the one line cap');
  node('a-fold').click();
  for (const t of ['zeta', 'Alpha', 'eagle']) assert.equal(button('a-recent', t), button('a-rows', t), `${t} reads as its letter row cell`);
  assert.match(button('a-recent', 'Alpha'), /data-k="on"/);
  assert.match(button('a-recent', 'eagle'), /data-k="pend"/);
  node('a-panel').click({ target: { closest: () => ({ dataset: { tag: 'zeta' } }) } });
  assert.deepEqual(picked, ['zeta'], 'a recent cell picks as any cell');
  node('a-recent').columns = '120px 120px 120px 120px 120px 120px';
  node('a-recent').resized();
  assert.deepEqual(recent(), ['zeta', 'Alpha', 'eagle', 'apple', 'iOS'], 'a wider line: more of them, all fitting');
  node('a-sugg').columns = 2;
  node('a-sugg').resized();
  assert.deepEqual(suggested(), ['Alpha', 'apple'], 'Suggested redraws on resize too');
  assert.deepEqual(recent(), ['zeta', 'Alpha', 'eagle', 'apple', 'iOS'], 'each row fits its own columns');
  node('a-sugg').columns = 'none';
  node('a-sugg').resized();
  assert.deepEqual(suggested(), [], 'Suggested not laid out: none');
  node('a-recent').columns = 'repeat(auto-fill, minmax(8.5rem, 1fr))';
  node('a-recent').resized();
  assert.deepEqual(recent(), [], 'not laid out: none');
  node('a-recent').columns = '136px 136px 136px';
  K.lib.recent = ['iOS'];
  K.panel.render({ has: () => false, pend: () => false, pick() {}, sugg: [] });
  assert.deepEqual(recent(), ['iOS'], 'the order as the library last said');
  K.lib.recent = [];
  K.panel.render({ has: () => false, pend: () => false, pick() {}, sugg: [] });
  assert.equal(node('a-recent').innerHTML, '', 'nothing used yet: the line stays, empty');
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
const addPage = (row, url, tags = [], suggested = []) => ({ row, title: `Page ${row}`, host: 'added.example', url, image: null, tags, suggested });

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
  f.job = (id, url, stages, extra = {}) => ({ id, url, key: url, action: 'add', retry: [], signed_in: false, title: null, restored: false, failed: false,
    finished: false, seconds: 0.5, page: null, stages, ...extra });
  f.type = (link) => { node('a-link').value = link; node('a-link').input(); };
  f.submit = () => node('a-form').submit({ preventDefault() {} });
  f.click = (tag) => node('a-panel').click({ target: { closest: (s) => (s === 'button[data-tag]' ? { dataset: { tag } } : null) } });
  f.look = (tag) => (node('a-rows').innerHTML.match(new RegExp(`data-tag="${tag}" data-k="(\\w+)"`)) || [])[1];
  f.applies = () => requests.filter((r) => r.route === '/api/tags' && !r.body.undo).map((r) => r.body);
  f.filter = (text) => { node('a-tag').value = text; node('a-tag').input(); node('a-filter').submit({ preventDefault() {} }); };
  /* the library's answer to a write on one page: the tags it holds afterwards and the record that undoes the write */
  const answer = (key, tags, undo) => ({ undo: { tags: undo, vocabulary: {} }, urls: [key], tags: { [key]: tags },
    vocabulary: K.lib.app_tags.map((name) => ({ name })), counts: { changed: 1, unchanged: 0, unknown: 0 } });
  f.applied = async (row, tags) => {   // the library takes the next tag write, for this row's page
    const r = await f.next('/api/tags'), key = K.pages.get(row).url;
    r.resolve(answer(key, [...new Set([...K.pages.get(row).tags, ...tags])].sort(), tags.map((name) => ({ url: key, name, add: false, remove: false }))));
    await turn(); await turn();
    return r.body;
  };
  f.undone = async (row, tags) => {   // the library takes the next Undo; the page holds `tags` afterwards
    const r = await f.next('/api/tags'), key = K.pages.get(row).url;
    r.resolve(answer(key, tags, r.body.undo.tags.map((d) => ({ ...d, add: true }))));
    for (let i = 0; i < 4; i++) await turn();
    return r.body;
  };
  return f;
}

/* picks on a link before Add travel with it; none is written before the page has a row, then one library write for
   the key Indexed names, one toast, and its Undo sends that write's record back */
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
  const add = await f.next('api/add');
  assert.deepEqual(add.body, { url, action: 'add', title: null });
  add.resolve(f.job(1, url, STAGES.running));
  for (const stages of [STAGES.running, STAGES.added, STAGES.image, STAGES.search]) {
    await turn(); await turn();
    assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['pend', 'pend'], 'still striped');
    await f.reply('api/add/1', f.job(1, url, stages));
    assert.deepEqual(f.applies(), [], 'no write during Library, Content, Image or Search');
  }
  assert.equal(node('a-link').value, '', 'listed: the box is ready for the next link');
  await f.reply('api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['pend', 'pend'], 'Indexed: striped until the library answers');
  assert.deepEqual(await f.applied(7, ['Trains', 'Coffee']), { urls: [url], add: ['Trains', 'Coffee'], remove: [] }, 'one write, for its key');
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['on', 'on'], 'then on');
  await f.reply('api/library', { ...K.lib });
  assert.equal(K.toasted, 'Added “Trains”, “Coffee” to “Page 7”');
  K.undo();
  const undone = await f.undone(7, []);
  assert.deepEqual(undone.undo.tags.map((d) => [d.url, d.name]), [[url, 'Trains'], [url, 'Coffee']], 'one Undo: the write\'s own record');
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['off', 'off'], 'Undo takes both off');
}

/* a write the library refuses or never answers leaves the page as it was, says so, and is not sent again */
async function addPicksFailure() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.running));
  await f.reply('api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  (await f.next('/api/tags')).reject(new Error('the library is busy'));
  for (let i = 0; i < 4; i++) await turn();
  assert.match(K.failure.message, /the library is busy/, 'said');
  assert.equal(f.look('Trains'), 'off', 'the page as the library holds it, the pick not kept');
  await f.reply('api/library', { ...K.lib });
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(f.applies(), [], 'and never sent again');
}

/* An Indexed payload captured before a Tag write answers cannot replace that write's confirmed tags. */
async function addKeepsNewerTags() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one';
  K.know([addPage(7, url)]);
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.search));
  const poll = await f.next('api/add/1');
  const tagging = K.tagPage(7, ['Trains']);
  await f.applied(7, ['Trains']);
  await tagging;
  await f.reply('api/library', { ...K.lib, forgotten: [] });
  poll.resolve(f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(plain(K.pages.get(7).tags), ['Trains'], 'the library reply still owns the shown tags');
  assert.equal(f.look('Trains'), 'on', 'Indexed keeps the acknowledged tag');
  assert.deepEqual(f.applies(), [], 'held picks already acknowledged are never written again');
  await f.reply('api/library', { ...K.lib, forgotten: [] });
}

/* a second link does not stop the first job: it writes the first page's picks to the first page's row only */
async function addPicksBackground() {
  const f = addFixture(), { K, node } = f, one = 'https://added.example/one', two = 'https://added.example/two';
  f.type(one); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, one, STAGES.running));
  await f.reply('api/add/1', f.job(1, one, STAGES.added));
  f.type(two);
  assert.equal(f.look('Trains'), undefined, 'a new link: the fold closes for it');
  node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'and the first page\'s picks are not its');
  f.click('Coffee'); f.submit();
  await f.reply('api/add', f.job(2, two, STAGES.running));
  await f.reply('api/add/1', f.job(1, one, STAGES.search));
  await f.reply('api/add/1', f.job(1, one, done('indexed'), { finished: true, page: addPage(7, one) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'the first job writes its own picks');
  await f.reply('api/library', { ...K.lib });
  assert.match(node('a-tile').innerHTML, /added.example\/two/, 'the second page stays on screen');
  assert.deepEqual([f.look('Trains'), f.look('Coffee')], ['off', 'pend'], 'its cells untouched');
  await f.reply('api/add/2', f.job(2, two, done('indexed'), { finished: true, page: addPage(8, two) }));
  assert.deepEqual((await f.applied(8, ['Coffee'])).add, ['Coffee']);
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
    await f.reply('api/add', f.job(++id, url, STAGES.running));
    assert.equal(f.look('Trains'), 'pend', `${name}: held`);
    await f.reply(`api/add/${id}`, f.job(id, url, stages, { finished: true }));
    if (name === 'forgotten') await f.reply('api/library', { ...K.lib });
    assert.equal(node('a-result').hidden, true, `${name}: no tile, no panel`);
    K.add.start(url, { action: 'restore' });
    await f.reply('api/add', f.job(++id, url, STAGES.running, { action: 'restore' }));
    assert.equal(f.look('Trains'), 'off', `${name}: the picks cleared`);
    await f.reply(`api/add/${id}`, f.job(id, url, { library: ev('library', 'done', { value: 'refused', reason: 'snapshot' }) }, { finished: true }));
  }
  node('a-link').value = '';
  const lone = 'https://added.example/lone';
  f.type(lone); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(++id, lone, STAGES.running));
  await f.reply(`api/add/${id}`, f.job(id, lone, done('not_indexed'), { finished: true }));
  await f.reply('api/library', { ...K.lib });
  assert.equal(f.look('Trains'), 'pend', 'Not indexed: the picks wait');
  assert.equal(node('a-act').textContent, 'Retry');
  node('a-act').click();
  const index = await f.next('api/add');
  assert.deepEqual(index.body, { url: lone, action: 'index' });
  index.resolve(f.job(++id, lone, { library: ev('library', 'done', { value: 'known' }), search: ev('search', 'done', { value: 'indexed' }) },
    { action: 'index', finished: true, page: addPage(9, lone) }));
  assert.deepEqual((await f.applied(9, ['Trains'])).add, ['Trains'], 'its Retry writes them');
  await f.reply('api/library', { ...K.lib });
  const failed = 'https://added.example/failed';
  f.type(failed); node('a-fold').click(); f.click('Coffee'); f.submit();
  await f.reply('api/add', f.job(++id, failed, STAGES.running, { failed: true, finished: true }));
  K.add.start(failed, { action: 'add' });
  await f.reply('api/add', f.job(++id, failed, STAGES.running));
  assert.equal(f.look('Coffee'), 'pend', 'knowmoretabs could not run: kept while Retry is offered');
  await f.reply(`api/add/${id}`, f.job(id, failed, done('indexed'), { finished: true, page: addPage(10, failed) }));
  assert.deepEqual((await f.applied(10, ['Coffee'])).add, ['Coffee']);
  await f.reply('api/library', { ...K.lib });
  const late = 'https://added.example/late', next = 'https://added.example/next';
  f.type(late); node('a-fold').click(); f.click('Coffee'); f.submit();
  const lateId = ++id;
  await f.reply('api/add', f.job(lateId, late, STAGES.running));
  f.type(next); node('a-fold').click();
  f.click('Trains'); f.click('Trains');
  assert.equal(f.look('Trains'), 'off', 'a pending pick clicked again is gone');
  f.submit();
  await f.reply('api/add', f.job(++id, next, STAGES.running));
  await f.reply(`api/add/${lateId}`, f.job(lateId, late, done('not_indexed'), { finished: true }));
  assert.equal(K.toasted, 'Tags for https://added.example/late not added: the page is not indexed', 'the dropped picks are told');
  await f.reply('api/library', { ...K.lib });
  assert.equal(requests.filter((r) => r.route === 'api/add/' + lateId).length, 0, 'its poll stops');
  await f.reply(`api/add/${id}`, f.job(id, next, done('indexed'), { finished: true, page: addPage(11, next) }));
  await f.reply('api/library', { ...K.lib });
  assert.deepEqual(f.applies(), [], 'nothing written for the dropped or unpicked tags');
}

/* Enter in the filter: the exact match in any case, else the only match, else a new tag made and picked; with several
   matches and none exact, nothing */
async function addFilterEnter() {
  const f = addFixture(), { K, node, requests } = f, url = 'https://added.example/one';
  f.type(url); node('a-fold').click(); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.running));
  f.filter('trains');
  assert.deepEqual([f.look('Trains'), node('a-tag').value], ['pend', ''], 'exact, any case: picked, the filter clears');
  f.filter('coff');
  assert.deepEqual([f.look('Coffee'), node('a-tag').value], ['pend', ''], 'the only match');
  f.filter('rain');
  assert.equal(node('a-tag').value, 'rain', 'several, none exact: nothing');
  assert.equal(f.look('Night trains'), 'off');
  f.filter('  sleeper  CARS ');
  const made = await f.next('/api/vocabulary');
  assert.deepEqual(made.body, { create: ['sleeper  CARS'] }, 'no match: the library makes the name');
  made.resolve({ vocabulary: ['Coffee', 'Night trains', 'Sleeper cars', 'Trains'].map((name) => ({ name })), resolved: ['Sleeper cars'] });
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(plain(K.lib.app_tags), ['Coffee', 'Night trains', 'Sleeper cars', 'Trains'], 'listed while the page waits for its row');
  assert.deepEqual([f.look('Sleeper cars'), node('a-tag').value], ['pend', ''], "and picked, in the library's spelling");
  f.filter('trains');
  assert.equal(f.look('Trains'), 'off', 'Enter on a picked tag unpicks it');
  await f.reply('api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Coffee', 'Sleeper cars'])).add, ['Coffee', 'Sleeper cars']);
  assert.equal(requests.filter((r) => r.route === '/api/vocabulary').length, 0, 'made once');
}

/* Draft picks belong to one address, including while the initial Add reply is delayed. */
async function addPicksAddress() {
  const f = addFixture(), { K, node } = f;
  const one = 'https://added.example/one', two = 'https://added.example/two';
  f.type(one); node('a-fold').click(); f.click('Trains');
  f.type(two); node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'editing to another address drops the old draft picks');
  f.click('Coffee'); f.submit();
  await f.reply('api/add', f.job(1, two, done('indexed'), { finished: true, page: addPage(8, two) }));
  assert.deepEqual((await f.applied(8, ['Coffee'])).add, ['Coffee']);
  await f.reply('api/library', { ...K.lib });
  assert.equal(K.has(8, 'Trains'), false, 'the other address never inherits the old picks');
}

async function addPicksBeforeReply() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/delayed';
  f.type(url); node('a-fold').click(); f.submit();
  const add = await f.next('api/add');
  f.click('Trains');
  assert.equal(f.look('Trains'), 'pend', 'a pick while Add is waiting stays visible');
  f.type('https://added.example/next');
  add.resolve(f.job(1, url, STAGES.added));
  for (let i = 0; i < 4; i++) await turn();
  await f.reply('api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'the submitted address keeps the pick');
  await f.reply('api/library', { ...K.lib });
  node('a-fold').click();
  assert.equal(f.look('Trains'), 'off', 'the next draft does not inherit it');
}

/* A poll already sent for a superseded job must not drop the picks the newer job for its address will write. */
async function addPicksStalePoll() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/retry';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.added));
  const old = await f.next('api/add/1');
  K.add.start(url, { action: 'index' });
  await f.reply('api/add', f.job(2, url, STAGES.added, { action: 'index' }));
  old.resolve(f.job(1, url, done('not_indexed'), { finished: true }));
  for (let i = 0; i < 4; i++) await turn();
  assert.equal(K.toasted, undefined, 'the obsolete poll reports no dropped picks');
  assert.equal(f.look('Trains'), 'pend', 'the newest job keeps its picks until Indexed');
  await f.reply('api/add/2', f.job(2, url, done('indexed'), { action: 'index', finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'only one write, from the latest job');
  await f.reply('api/library', { ...K.lib });
  assert.deepEqual(f.applies(), []);
}

/* A finished unindexed page keeps its picks while the box holds another link: emptying it brings back Retry, which writes them */
async function addPicksDetour() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/unindexed';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.added));
  await f.reply('api/add/1', f.job(1, url, done('not_indexed'), { finished: true }));
  await f.reply('api/library', { ...K.lib });
  f.type('https://a');
  assert.equal(K.toasted, undefined, 'a link being typed drops nothing');
  f.type('');
  node('a-fold').click();
  assert.equal(f.look('Trains'), 'pend', 'back on the page, its picks wait');
  assert.equal(node('a-act').textContent, 'Retry');
  node('a-act').click();
  await f.reply('api/add', f.job(2, url, done('indexed'), { action: 'index', finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'its Retry writes them');
}

/* At Indexed the Suggested row fills with the model's tags for the page, best first, less those picked and as many as its
   line holds; it stays put while the page is on screen. A pick there writes as any pick and reads as its cell in the list */
async function addSuggested() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one';
  K.lib.app_tags.push('Canals', 'Timetables');
  const row = () => [...node('a-sugg').innerHTML.matchAll(/data-tag="([^"]*)" data-k="(\w+)"/g)].map(([, t, k]) => `${t} ${k}`);
  f.type(url); node('a-fold').click(); f.click('Coffee'); f.submit();
  await f.reply('api/add', f.job(1, url, STAGES.running));
  for (const stages of [STAGES.added, STAGES.image, STAGES.search]) {
    await f.reply('api/add/1', f.job(1, url, stages));
    assert.deepEqual(row(), [], 'empty before Indexed');
  }
  const model = ['Trains', 'Coffee', 'Timetables', 'Night trains', 'Canals'];
  await f.reply('api/add/1', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url, [], model) }));
  assert.deepEqual(row(), ['Trains off', 'Timetables off', 'Night trains off'], 'best first, less the pick, as many as its line holds');
  await f.applied(7, ['Coffee']);
  await f.reply('api/library', { ...K.lib });
  assert.deepEqual(row(), ['Trains off', 'Timetables off', 'Night trains off'], 'the pick written: nothing moves');
  f.click('Timetables');
  assert.deepEqual((await f.applied(7, ['Timetables'])).add, ['Timetables'], 'written as any pick');
  assert.deepEqual([row(), f.look('Timetables')], [['Trains off', 'Timetables on', 'Night trains off'], 'on'], 'on in place, as its cell');
  assert.equal(K.toasted, 'Added Timetables to 1 page');
  K.undo();
  const undone = await f.undone(7, ['Coffee']);
  assert.deepEqual([undone.undo.tags.map((d) => d.name), f.look('Timetables'), row()], [['Timetables'], 'off', ['Trains off', 'Timetables off', 'Night trains off']], 'Undo, in place');
  f.type('https://added.example/two');
  assert.deepEqual(row(), [], 'another link: its own row, empty');
  f.type('');
  node('a-sugg').columns = 2;
  K.add.render();
  assert.deepEqual(row(), ['Trains off', 'Timetables off'], 'back on the page: its suggestions, as many as a narrower line holds');
}

/* The final library refresh never holds up a pick: a pick or its Undo during the refresh goes at once, and the read
   it overtook is read again, so an earlier snapshot never replaces a newer recent order. An undone add was a use. */
async function addLibraryWrites() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/recent';
  K.lib.recent = ['Coffee'];
  node('a-recent').columns = '136px 136px';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, done('indexed'), { finished: true, page: addPage(7, url) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'held picks first');
  const refresh = await f.next('api/library');
  f.click('Coffee');
  assert.deepEqual((await f.applied(7, ['Coffee'])).add, ['Coffee'], 'a pick during the refresh goes at once');
  refresh.resolve({ ...K.lib, recent: ['Trains', 'Coffee'], forgotten: [] });   // read before that pick
  (await f.next('api/library')).resolve({ ...K.lib, recent: ['Coffee', 'Trains'], forgotten: [] });
  for (let i = 0; i < 4; i++) await turn();
  assert.deepEqual(plain(K.lib.recent), ['Coffee', 'Trains'], 'the overtaken read is read again: the later pick stays first');
  assert.deepEqual([f.look('Coffee'), f.look('Trains')], ['on', 'on']);
  const loading = K.loadLibrary();
  const beforeUndo = await f.next('api/library');
  K.undo();
  const undone = await f.undone(7, ['Trains']);
  assert.deepEqual(undone.urls, [], 'Undo goes at once, with its record');
  beforeUndo.resolve({ ...K.lib, recent: ['Coffee', 'Trains'], forgotten: [] });
  (await f.next('api/library')).resolve({ ...K.lib, recent: ['Coffee', 'Trains'], forgotten: [] });
  await loading;
  assert.deepEqual(plain(K.lib.recent), ['Coffee', 'Trains'], 'an undone add was still a use');
  assert.deepEqual([f.look('Coffee'), f.look('Trains')], ['off', 'on']);
}

/* a link the library keeps under a cleaned key: the tile shows the key, and the action names it, its picks with it */
async function addFollowUpNamesTheKey(background = false) {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one/#top', key = 'https://added.example/one';
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  const listed = { library: ev('library', 'done', { value: 'added', url: key }) };
  await f.reply('api/add', f.job(1, url, listed, { key }));
  assert.match(node('a-tile').innerHTML, /class="ht link">https:\/\/added.example\/one<\/h3>/, 'the tile names the key');
  await f.reply('api/add/1', f.job(1, url, { ...done('not_indexed'), ...listed }, { key, finished: true }));
  await f.reply('api/library', { ...K.lib });
  assert.equal(f.look('Trains'), 'pend');
  assert.equal(node('a-act').textContent, 'Retry');
  node('a-act').click();
  const retry = await f.next('api/add');
  assert.deepEqual(retry.body, { url: key, action: 'index' }, 'the action names the key');
  K.add.render();
  assert.equal(f.look('Trains'), 'pend', 'the pick stays held while the follow up reply is delayed');
  f.click('Coffee');
  assert.equal(f.look('Coffee'), 'pend', 'another pick during the follow up waits with the first');
  if (background) { f.type('https://added.example/next'); f.submit(); }
  retry.resolve(f.job(2, key, done('indexed'), { action: 'index', finished: true, page: addPage(7, key) }));
  for (let i = 0; i < 4; i++) await turn();
  const written = await f.applied(7, ['Trains', 'Coffee']);
  assert.deepEqual([written.urls, written.add], [[key], ['Trains', 'Coffee']], 'the link\'s picks follow the key');
  assert.equal(K.has(7, 'Trains'), true, 'the follow up writes its picks even when another link is now drawn');
  await f.reply('api/library', { ...K.lib });
  if (background) return;
  f.type(url); f.submit();
  await f.reply('api/add', f.job(3, url, {
    ...done('indexed'), ...listed, content: ev('content', 'done', { status: 'not_found', http_status: 404 }),
  }, { key, finished: true, page: addPage(7, key) }));
  await f.reply('api/library', { ...K.lib });
  assert.equal(node('a-act').textContent, 'Remove');
  node('a-act').click();
  const remove = await f.next('api/add');
  assert.deepEqual(remove.body, { url: key, action: 'forget' }, 'Remove names the key too');
  remove.resolve(f.job(4, key, { library: ev('library', 'done', { value: 'forgotten' }) }, { action: 'forget', finished: true }));
  await f.reply('api/library', { ...K.lib, forgotten: [7] });
  assert.equal(K.gone.has(7), true, 'Remove hides the indexed row when the link and key differ');
}

/* A follow up on the key supersedes the link's job: that job's poll resolving first drops none of the picks. */
async function addFollowUpStalePoll() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one/#top', key = 'https://added.example/one';
  const blocked = { library: ev('library', 'done', { value: 'added', url: key }), content: ev('content', 'done', { status: 'blocked', tier: 'web', http_status: 403 }) };
  f.type(url); node('a-fold').click(); f.click('Trains'); f.submit();
  await f.reply('api/add', f.job(1, url, { ...blocked, search: ev('search', 'running') }, { key }));
  const old = await f.next('api/add/1');
  assert.equal(node('a-act').textContent, 'Try signed in');
  node('a-act').click();
  const signed = await f.next('api/add');
  old.resolve(f.job(1, url, { ...blocked, search: ev('search', 'done', { value: 'not_indexed' }) }, { key, finished: true }));
  for (let i = 0; i < 4; i++) await turn();
  assert.equal(K.toasted, undefined, 'the superseded poll reports no dropped picks');
  signed.resolve(f.job(2, key, done('indexed'), { signed_in: true, retry: ['content'], finished: true, page: addPage(7, key) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'the follow up writes them');
}

/* Restore on a forgotten variant follows the key: its progress shows, its picks are written once at Indexed, one Undo */
async function addRestoreFollowsTheKey() {
  const f = addFixture(), { K, node } = f, url = 'https://added.example/one/#top', key = 'https://added.example/one';
  f.type(url); f.submit();
  await f.reply('api/add', f.job(1, url, { library: ev('library', 'done', { value: 'forgotten', url: key }) }, { key, finished: true }));
  await f.reply('api/library', { ...K.lib });
  assert.equal(node('a-act').textContent, 'Restore');
  node('a-act').click();
  const restore = await f.next('api/add');
  assert.deepEqual(restore.body, { url: key, action: 'restore' }, 'Restore names the key');
  restore.resolve(f.job(2, key, STAGES.running, { action: 'restore' }));
  for (let i = 0; i < 4; i++) await turn();
  assert.equal(node('a-link').value, key, 'the box names the key, not a draft of the link');
  assert.equal(node('a-library-val').textContent, 'Adding', 'the Restore\'s progress shows');
  assert.match(node('a-tile').innerHTML, /class="ht link">https:\/\/added.example\/one<\/h3>/, 'the tile names the key');
  node('a-fold').click(); f.click('Trains');
  assert.equal(f.look('Trains'), 'pend', 'a pick waits for the key\'s row');
  const known = { library: ev('library', 'done', { value: 'known', url: key }) };
  await f.reply('api/add/2', f.job(2, key, { ...STAGES.search, ...known }, { action: 'restore', restored: true }));
  assert.equal(node('a-link').value, '', 'listed: the box is ready for the next link');
  await f.reply('api/add/2', f.job(2, key, { ...done('indexed'), ...known }, { action: 'restore', restored: true, finished: true, page: addPage(7, key) }));
  assert.deepEqual((await f.applied(7, ['Trains'])).add, ['Trains'], 'written once, at Indexed');
  await f.reply('api/library', { ...K.lib });
  assert.equal(K.toasted, 'Added “Trains” to “Page 7”');
  K.undo();
  const undo = await f.undone(7, []);
  assert.deepEqual(undo.undo.tags, [{ url: key, name: 'Trains', add: false, remove: false }], 'its record, for the key');
  assert.equal(f.requests.filter((r) => r.route !== 'api/library').length, 0, 'one write, one Undo');
}

/* a poll the Library cannot answer is not a finished job: its stages stay as last seen, nothing is asked again,
   and the library is not reloaded as if the page were added */
async function addPollFails() {
  const { K, node } = setup({ lit, strip: { render() {}, sayIn: () => () => {} }, lib: { app_tags: [] } }, 'panel.js', 'add.js');
  for (const k of ['library', 'content', 'image', 'search']) node(`a-${k}`).querySelector = () => node(`a-${k}-val`);
  const ev = (stage, state, extra = {}) => ({ stage, state, ...extra });
  const link = 'https://added.example/polled';
  const running = { id: 1, url: link, action: 'add', retry: [], signed_in: false, title: null, restored: false, failed: false,
    finished: false, seconds: 0.4, page: null, stages: { library: ev('library', 'done', { value: 'added' }), search: ev('search', 'running') } };
  const asked = [];
  K.api = (path) => {
    asked.push(path);
    return path === 'api/add' ? Promise.resolve(running) : Promise.reject(new Error('the model service is not answering'));
  };
  const lib = K.lib;
  await K.add.start(link);
  assert.deepEqual(asked, ['api/add', 'api/add/1'], 'one failed poll, then nothing');
  assert.equal(K.failure.message, 'the model service is not answering');
  assert.equal(node('a-library-val').textContent, 'Added');
  assert.equal(node('a-search-val').textContent, 'Indexing', 'the job is shown as last seen, not finished');
  assert.equal(K.lib, lib, 'no reload as if it had finished');
}

run({ addKeepsNewerTags, addLibraryWrites, addPicksFailure, addStates, addRequests, addSignedInStages, panelLetters, panelRecent, addPicksHeld, addPicksBackground, addPicksOutcomes, addFilterEnter, addPicksAddress, addPicksBeforeReply, addPicksStalePoll, addPicksDetour, addSuggested, addFollowUpNamesTheKey, addFollowUpPicksBackground: () => addFollowUpNamesTheKey(true), addFollowUpStalePoll, addRestoreFollowsTheKey, addPollFails });
