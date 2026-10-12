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
  const { K, node, context } = setup({ lit, strip: { render() {}, sayIn: () => () => {} }, lib: { app_tags: [] } }, 'add.js');
  const asked = [];
  let reply = () => new Promise(() => {});
  K.api = (path, body) => { asked.push([path, body]); return reply(path, body); };
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
  for (const k of ['library', 'content', 'image', 'search']) node(`a-${k}`).querySelector = () => node(`a-${k}-val`);
  reply = (path) => Promise.resolve(path === '/api/library' ? { app_tags: [] } : done);
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

run({ addStates, addRequests, addSignedInStages });
