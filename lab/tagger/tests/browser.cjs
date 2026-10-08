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
  /* reading a page scrolls hundreds of times; the browser caps history calls (Chrome 200 in 10 s) */
  longScroll: async ({ $, pos, until, check, search }) => {
    await search('night train');
    for (let i = 1; i <= 250; i++) {
      scrollTo(0, 4 * i);
      await new Promise((r) => requestAnimationFrame(r));
    }
    $('#next').click();
    await until(() => pos().startsWith('51 to '), 'page two');
    history.back();
    await until(() => pos().startsWith('1 to '), 'Back to page one');
    check(scrollY === 1000, 'Back restores where the reading stopped');
  },
  reloadScroll: async ({ search, page, check }) => {
    await search('night train');
    await page('#next', 51);
    scrollTo(0, 900);
    await new Promise((r) => setTimeout(r, 300));
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
    const before = tiles()[0], gone = before.dataset.row;
    before.querySelector('[data-forget]').click();
    await until(() => !tileOf(gone), 'the neighbour forgotten');
    check(tiles()[0] === tile, 'forgetting a neighbour keeps the node of a tile still in view (keyed by page, not by place)');
    $('#toast-undo').click();
    await until(() => tileOf(gone), 'the neighbour back');
    check(tiles()[1] === tile && tileOf(row) === tile, 'and Undo keeps it too');
    tile.querySelector('[data-untag="Sleeper cars"]').click();
    await until(() => !tile.querySelector('[data-untag]'), 'the tag off the tile');
    check(tileOf(row) === tile && tile.classList.contains('sel'), '× keeps the tile node, still selected');
    $('#clear').click();
    await until(() => !tile.classList.contains('sel'), 'the selection cleared');
    check(tileOf(row) === tile, 'clearing keeps the tile node');
  },

  /* f forgets the focused page, u brings it back in its place; Pin never selects; Pinned is a view over the results */
  forgetPin: async ({ $, tiles, tileOf, pos, until, check, search }) => {
    await search('night train');
    const total = () => Number(pos().match(/of (\d+)/)[1]), all = total();
    const victim = tiles()[2], row = victim.dataset.row, next = tiles()[3];
    victim.focus();
    victim.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', bubbles: true }));
    await until(() => !tileOf(row), 'the forgotten tile gone');
    check(total() === all - 1 && tiles().length === 49, 'the count and the grid drop it');
    check($('#n-sel').textContent === '0', 'f selects nothing');
    check(document.activeElement === next, 'the next tile takes the focus');
    check(!$('#toast').hidden && $('#toast-msg').textContent === 'Forgotten' && !$('#toast-undo').hidden, 'the toast offers Undo');
    next.dispatchEvent(new KeyboardEvent('keydown', { key: 'u', bubbles: true }));
    await until(() => tileOf(row), 'Undo brings it back');
    check(tiles()[2].dataset.row === row && total() === all, 'in its place');
    const pinned = tiles()[4];
    pinned.querySelector('[data-pin]').click();
    await until(() => pinned.classList.contains('pinned'), 'pinned');
    check(!pinned.classList.contains('sel') && $('#n-sel').textContent === '0' && $('#n-pin').textContent === '1', 'pinning selects nothing');
    tileOf(row).querySelector('[data-forget]').click();
    await until(() => !tileOf(row), 'forgotten by its button');
    check($('#n-sel').textContent === '0', 'Forget selects nothing');
    scrollTo(0, 600);
    $('#show-pins').click();
    await until(() => pos().startsWith('Pinned'), 'the pinned view');
    check(tiles().length === 1 && tiles()[0].dataset.row === pinned.dataset.row && scrollY === 0, 'only the pinned page, from the top');
    $('#show-pins').click();
    await until(() => /^1 to 49 of/.test(pos()), 'back to the results');
    check(scrollY === 600, `the results at the scroll they were left (${scrollY})`);
    await window.KMT.writes;
    sessionStorage.setItem('p9', JSON.stringify({ row, pinned: pinned.dataset.row, all }));
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

/* the checks a case makes once the page has reloaded */
const afterReload = {
  reloadScroll: async ({ pos, until, check }) => {
    await until(() => pos().startsWith('51 to '), 'the saved page after reload');
    check(scrollY === 900, 'reload restores the saved scroll');
  },
  forgetPin: async ({ $, tileOf, pos, until, check }) => {
    const { row, pinned, all } = JSON.parse(sessionStorage.getItem('p9'));
    await until(() => /^1 to 50 of/.test(pos()), 'the results after reload, a full page from the server');
    check(!tileOf(row) && Number(pos().match(/of (\d+)/)[1]) === all - 1, 'still forgotten after a reload');
    check($('#n-pin').textContent === '1' && tileOf(pinned).classList.contains('pinned'), 'still pinned after a reload');
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
    if (afterReload[name]) {
      const reloaded = load();
      await send('Page.reload', {}, sessionId);
      await reloaded;
      const restored = await send('Runtime.evaluate', { expression: `(${afterReload[name]})((${inPage})())`,
        awaitPromise: true, returnByValue: true }, sessionId);
      if (restored.exceptionDetails) throw new Error(restored.exceptionDetails.exception?.description || restored.exceptionDetails.text);
    }
    assert.deepEqual(errors, [], `no console error, exception or CSP report: ${errors.join(' | ')}`);
  } finally {
    proc.kill();
  }
}
main().catch((err) => { console.error(err.message); process.exitCode = 1; });
