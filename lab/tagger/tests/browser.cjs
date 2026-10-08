// The one screen in headless Chrome, on a running app: the real DOM, lit-html and the server's CSP.
// usage: node browser.cjs <chrome> <profile dir> <app url> <case>
// Chrome speaks the DevTools protocol over a pipe (fds 3 and 4), so no port and no package is needed.
// Any console error, page exception or CSP report fails the case.
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');

const [chrome, profile, url, name] = process.argv.slice(2);

function launch() {
  const proc = spawn(chrome, ['--headless=new', '--remote-debugging-pipe', `--user-data-dir=${profile}`, '--no-first-run',
    '--no-default-browser-check', '--window-size=1280,900', 'about:blank'], { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] });
  const waiting = new Map(), errors = [];
  let id = 0, buffer = '', loaded = null;
  proc.stdio[4].on('data', (chunk) => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf('\0')) >= 0) {
      const msg = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      if (msg.id && waiting.has(msg.id)) {
        const { resolve, reject } = waiting.get(msg.id);
        waiting.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message)); else resolve(msg.result);
      } else if (msg.method === 'Page.loadEventFired' && loaded) loaded();
      else if (msg.method === 'Runtime.exceptionThrown') errors.push(msg.params.exceptionDetails.exception?.description || 'exception');
      else if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') errors.push(msg.params.args.map((a) => a.value).join(' '));
      else if (msg.method === 'Log.entryAdded' && msg.params.entry.level === 'error') {
        const { source, text, url: at } = msg.params.entry, path = at ? new URL(at).pathname : '';
        if (path !== '/favicon.ico') errors.push(`${source}: ${text} ${path}`);   // the app has no icon
      }
    }
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    waiting.set(++id, { resolve, reject });
    proc.stdio[3].write(JSON.stringify({ id, method, params, sessionId }) + '\0');
  });
  return { proc, send, errors, load: () => new Promise((resolve) => { loaded = resolve; }) };
}

/* ---- in the page: each case is a function run there, given helpers; it throws on a failed check ---- */
function inPage() {
  const $ = (s) => document.querySelector(s);
  const tiles = () => [...document.querySelectorAll('#grid .tile')];
  const tileOf = (row) => $(`#grid .tile[data-row="${row}"]`);
  const pos = () => $('#v-pos').textContent;
  const until = async (ok, what) => {
    for (let i = 0; i < 400; i++) { if (ok()) return; await new Promise((r) => setTimeout(r, 25)); }
    throw new Error(`timed out waiting for ${what}`);
  };
  const check = (ok, what) => { if (!ok) throw new Error(what); };
  const search = async (q) => {
    $('#s-q').value = q;
    $('#s-form').requestSubmit();
    await until(() => tiles().length === 50 && /^1 to 50 of/.test(pos()), 'the first page');
  };
  const page = async (button, first) => {
    scrollTo(0, 1500);
    check(scrollY > 0, 'the grid scrolls');
    $(button).click();
    await until(() => new RegExp(`(?:^| · )${first} to `).test(pos()), `the page from ${first}`);
  };
  return { $, tiles, tileOf, pos, until, check, search, page };
}

const cases = {
  newSearch: async ({ $, pos, check, search, page }) => {
    await search('night train');
    await page('#next', 51);
    scrollTo(0, 900);
    await search('espresso');
    check(pos().startsWith('1 to 50 of'), 'a new search starts on page one');
    check(scrollY === 0, 'a new search shows the top');
  },
  pageFocus: async ({ $, tiles, check, search, page }) => {
    await search('night train');
    $('#next').focus();
    await page('#next', 51);
    check(document.activeElement === tiles()[0], 'Next focuses the first replacement tile');
    $('#next').focus();
    await page('#next', 101);
    check($('#next').disabled && document.activeElement === tiles()[0], 'last page retains useful keyboard focus');
    $('#prev').focus();
    await page('#prev', 51);
    check(document.activeElement === tiles()[0], 'Previous focuses the first replacement tile');
  },
  pageHistory: async ({ $, tiles, pos, until, check, search, page }) => {
    await search('night train');
    scrollTo(0, 700);
    $('#next').click();
    await until(() => pos().startsWith('51 to '), 'page two');
    scrollTo(0, 900);
    $('#next').click();
    await until(() => pos().startsWith('101 to '), 'page three');
    history.back();
    await until(() => pos().startsWith('51 to '), 'Back to page two');
    check(scrollY === 900, 'Back restores page two scroll');
    history.back();
    await until(() => pos().startsWith('1 to '), 'Back to page one');
    check(scrollY === 700, 'Back restores page one scroll');
    history.forward();
    await until(() => pos().startsWith('51 to '), 'Forward to page two');
    check(scrollY === 900, 'Forward restores page two scroll');
  },
  reloadScroll: async ({ search, page, check }) => {
    await search('night train');
    await page('#next', 51);
    scrollTo(0, 900);
    await new Promise((r) => setTimeout(r, 100));
    check(scrollY === 900, 'scroll before reload');
  },
  /* a selection or tag update reuses the tile's node, its picture and its focus */
  identity: async ({ $, tiles, tileOf, until, check, search }) => {
    await search('night train');
    check(window.KMT.lit && typeof window.KMT.lit.render === 'function', 'lit-html loaded under the CSP');
    const tile = tiles()[1], row = tile.dataset.row, picture = tile.querySelector('.th'), other = tiles()[2];
    tile.focus();
    tile.click();
    await until(() => tile.classList.contains('sel'), 'the tile selected');
    check(tileOf(row) === tile, 'selecting keeps the tile node');
    check(tile.querySelector('.th') === picture, 'and its picture');
    check(document.activeElement === tile, 'and its focus');
    check(tiles()[2] === other, 'and its neighbours');
    $('#new-tag').value = 'Sleeper cars';
    $('#newtag').requestSubmit();
    await until(() => tile.querySelector('[data-untag="Sleeper cars"]'), 'the tag on the tile');
    check(tileOf(row) === tile && tile.querySelector('.th') === picture, 'tagging keeps the tile node and its picture');
    $('#strip [data-tag="Sleeper cars"] [data-act=filter]').click();
    await until(() => tiles().length === 1, 'only the tagged tile');
    check(tiles()[0] === tile, 'filtering keeps the node of a tile still in view (keyed by page, not by place)');
    $('#v-filter [data-act=unfilter]').click();
    await until(() => tiles().length === 50, 'every tile back');
    tile.querySelector('[data-untag="Sleeper cars"]').click();
    await until(() => !tile.querySelector('[data-untag]'), 'the tag off the tile');
    check(tileOf(row) === tile && tile.classList.contains('sel'), '× keeps the tile node, still selected');
    $('#clear').click();
    await until(() => !tile.classList.contains('sel'), 'the selection cleared');
    check(tileOf(row) === tile, 'clearing keeps the tile node');
  },

  /* Next and Previous replace the hits and show the top; the selection stays */
  pages: async ({ $, tiles, tileOf, pos, check, search, page }) => {
    await search('night train');
    const total = Number(pos().match(/of (\d+)/)[1]);
    check(total > 100 && total <= 150, `the synthetic library ranks three pages (${total})`);
    check($('#prev').disabled && !$('#next').disabled, 'no Previous on the first page');
    const first = tiles().map((t) => t.dataset.row), chosen = tiles()[3];
    chosen.click();
    await page('#next', 51);
    check(scrollY === 0, 'Next shows the top');
    check(tiles().length === 50 && !tiles().some((t) => first.includes(t.dataset.row)), 'Next replaces the hits');
    check(!chosen.isConnected, 'the old tiles are gone');
    check(/^51 to 100 of \d+ for “night train”$/.test(pos()), `the position reads ${pos()}`);
    check($('#n-sel').textContent === '1', 'the selection stays');
    await page('#next', 101);
    check(pos().startsWith(`101 to ${total} of ${total}`) && $('#next').disabled, 'no Next on the last page');
    await page('#prev', 51);
    check(scrollY === 0 && !$('#next').disabled, 'Previous shows the top');
    await page('#prev', 1);
    check($('#prev').disabled, 'no Previous on the first page again');
    check(tileOf(chosen.dataset.row).classList.contains('sel'), 'back on the first page, the selected tile shows selected');
  },

  /* ≈ and back (again, Esc, Back) return to the same page and scroll */
  likeBack: async ({ $, tiles, pos, until, check, search, page }) => {
    await search('night train');
    tiles()[0].click();
    $('#new-tag').value = 'Sleeper cars';
    $('#newtag').requestSubmit();
    await until(() => $('#strip [data-tag="Sleeper cars"]'), 'the tag in the strip');
    await page('#next', 51);
    const here = tiles().map((t) => t.dataset.row).join(), at = pos();
    const ways = {
      again: () => $('#strip [data-tag="Sleeper cars"] [data-act=like]').click(),
      Esc: () => document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })),
      Back: () => history.back(),
    };
    for (const [how, leave] of Object.entries(ways)) for (const offset of [0, 50]) {
      scrollTo(0, 900);
      $('#strip [data-tag="Sleeper cars"] [data-act=like]').click();
      await until(() => pos().startsWith('Pages like') && /1 to 50 of/.test(pos()), 'the ≈ view');
      check(scrollY === 0, '≈ starts at the top');
      if (offset) await page('#next', 51);
      leave();
      await until(() => pos() === at, `${how} back`);
      check(tiles().map((t) => t.dataset.row).join() === here, `${how} returns to the same page`);
      check(scrollY === 900, `${how} restores the scroll (${scrollY})`);
    }
  },
};

async function main() {
  const run = cases[name];
  assert.ok(run, `no case ${name}`);
  const { proc, send, errors, load } = launch();
  try {
    const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
    for (const domain of ['Page', 'Runtime', 'Log']) await send(`${domain}.enable`, {}, sessionId);
    const loaded = load();
    await send('Page.navigate', { url }, sessionId);
    await loaded;
    const expression = `(${run})((${inPage})())`;
    const out = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId);
    if (out.exceptionDetails) throw new Error(`${out.exceptionDetails.exception?.description || out.exceptionDetails.text}\n${errors.join('\n')}`);
    if (name === 'reloadScroll') {
      const reloaded = load();
      await send('Page.reload', {}, sessionId);
      await reloaded;
      const restored = await send('Runtime.evaluate', { expression: `(async () => {
        const { pos, until, check } = (${inPage})();
        await until(() => pos().startsWith('51 to '), 'the saved page after reload');
        check(scrollY === 900, 'reload restores the saved scroll');
      })()`, awaitPromise: true, returnByValue: true }, sessionId);
      if (restored.exceptionDetails) throw new Error(restored.exceptionDetails.exception?.description || restored.exceptionDetails.text);
    }
    assert.deepEqual(errors, [], `no console error, exception or CSP report: ${errors.join(' | ')}`);
  } finally {
    proc.kill();
  }
}
main().catch((err) => { console.error(err.message); process.exitCode = 1; });
