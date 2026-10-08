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
  startupDuringPage: async ({ $, pos, until, check, search }) => {
    await until(() => window.pinsWaiting, 'startup waiting for pins');
    await search('night train');
    $('#next').click();
    await until(() => window.nextWaiting, 'Next waiting for its response');
    window.releasePins();
    await until(() => window.pinsRead, 'startup pin response read');
    await new Promise((resolve) => requestAnimationFrame(resolve));
    check(window.searchRequests === 2, 'startup must not replay the search while Next is pending');
    window.releaseNext();
    await until(() => pos().startsWith('51 to '), 'page two after startup completes');
    check(window.KMT.views.view().offset === 50, 'startup must preserve the submitted search and Next');
  },
  forgetUndoAfterNavigation: async ({ $, tiles, tileOf, pos, until, check, search, page }) => {
    await search('night train');
    const row = tiles()[2].dataset.row, all = Number(pos().match(/of (\d+)/)[1]);
    tileOf(row).click();
    tileOf(row).querySelector('[data-pin]').click();
    tileOf(row).querySelector('[data-forget]').click();
    await window.KMT.writes;
    await search('espresso');
    $('#toast-undo').click();
    await until(() => Number(pos().match(/of (\d+)/)?.[1]) === all, 'Undo updates the new search count');
    await window.KMT.writes;
    check($('#n-sel').textContent === '1' && $('#n-pin').textContent === '1', 'Undo restores selection and pin after a new search');
    await search('night train');
    const victim = tiles()[2].dataset.row;
    tileOf(victim).querySelector('[data-forget]').click();
    await window.KMT.writes;
    await page('#next', 50);
    $('#toast-undo').click();
    await until(() => Number(pos().match(/of (\d+)/)?.[1]) === all, 'Undo updates page two count');
    const expected = await window.KMT.api('/api/search', { query: 'night train', offset: 49, n: 50 });
    check(tiles().map((t) => Number(t.dataset.row)).join() === expected.hits.map((p) => p.row).join(), 'Undo refreshes page two to the real ranking');
  },
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

  /* ---- Add link, against a fake knowmoretabs that paces its stage lines ---- */
  addFlow: async ({ $, tiles, until, check }) => {
    const seg = (k) => $(`#a-${k} .val`).textContent, look = (k) => $(`#a-${k}`).dataset.k;
    check(!$('#add-screen').hidden && $('#tag-screen').hidden, 'the fragment opens Add link');
    check($('#nav-add').getAttribute('aria-current') === 'page' && !$('#nav-tag').hasAttribute('aria-current'), 'the nav marks it');
    check(document.activeElement === $('#a-link') && $('#a-go').disabled && $('#a-stages').hidden, 'empty: Add off, no bar');
    check(!$('#a-link').placeholder, 'no placeholder');
    const url = 'https://added.example/notes/one';
    $('#a-link').value = url;
    $('#a-link').dispatchEvent(new Event('input'));
    check(!$('#a-go').disabled, 'a link turns Add on');
    $('#a-form').requestSubmit();
    const seen = new Set();
    await until(() => { for (const k of ['library', 'content', 'image', 'search']) seen.add(`${k}:${seg(k)}:${look(k)}`); return seg('search') === 'Indexed'; }, 'Indexed');
    for (const s of ['library:Adding:run', 'library:Added:ok', 'content:Web:run', 'content:Ok · Web:ok', 'image:Fetching:run', 'image:Ok:ok'])
      check(seen.has(s), `the bar showed ${s} (${[...seen].join(', ')})`);
    check(/^\d+\.\d s$/.test($('#a-secs').textContent) && $('#a-act').hidden, 'elapsed, and no action');
    check($('#a-link').value === '' && document.activeElement === $('#a-link'), 'the box is empty and keeps the focus');
    const tile = $('#a-tile .tile');
    check(tile.classList.contains('sel') && !$('#a-tagrow').classList.contains('off'), 'Indexed opens the tile and the strip');
    check($('#a-tile .ht').textContent === 'An invented page on zeppelin timetables' && $('#a-tile .host').textContent === 'added.example', 'title and host');
    const img = $('#a-tile .th img');
    await until(() => img && img.complete && img.naturalWidth > 0, 'the picture');
    check(/^\/img\/[0-9a-f]{64}$/.test(new URL(img.src).pathname), 'addressed by page');
    $('#a-new').click();
    check(document.activeElement === $('#a-new-tag') && !$('#a-new-tag').placeholder, 'New tag opens a box');
    $('#a-new-tag').value = 'Sleeper cars';
    $('#a-newtag').requestSubmit();
    await until(() => $('#a-tile [data-untag="Sleeper cars"]'), 'the new tag on the tile');
    await until(() => $('#a-strip [data-tag="Sleeper cars"]')?.dataset.on === 'all' && !$('#a-new').hidden, 'and in the strip, held');
    check($('#a-tile .tile') === tile, 'tagging keeps the tile node');
    $('#a-strip [data-tag="Sleeper cars"] [data-act=remove]').click();
    await until(() => $('#a-strip [data-tag="Sleeper cars"]')?.dataset.on === 'none' && !$('#a-tile [data-untag]'), 'the strip takes it off');
    await until(() => $('#toast-msg').textContent === 'Removed Sleeper cars from 1 page' && !$('#toast-undo').hidden, 'the toast');
    $('#toast-undo').click();
    await until(() => $('#a-tile [data-untag="Sleeper cars"]'), 'Undo puts it back');
    await window.KMT.writes;
    $('#nav-tag').click();
    await until(() => !$('#tag-screen').hidden && $('#add-screen').hidden, 'the Tag screen');
    $('#s-q').value = 'zeppelin timetables';
    $('#s-form').requestSubmit();
    await until(() => tiles().length > 0, 'results');
    check(tiles()[0].querySelector('.ht').textContent === 'An invented page on zeppelin timetables', 'the added page is searchable');
    check(tiles()[0].querySelector('[data-untag="Sleeper cars"]'), 'with its tag');
  },
  /* #add=<link> fills the box and focuses Add, which waits; a later fragment refills it */
  addDeepLink: async ({ $, until, check }) => {
    const first = 'https://added.example/a b?x=1&y=é';
    check($('#a-link').value === first && document.activeElement === $('#a-go') && !$('#a-go').disabled, 'filled, Add focused');
    check(!document.body.innerText.includes('A deep linked title'), 'the title is not shown before the add');
    await new Promise((r) => setTimeout(r, 600));
    check($('#a-stages').hidden, 'nothing starts before Enter');
    $('#a-go').click();
    await until(() => $('#a-search .val').textContent === 'Indexed', 'Indexed');
    check($('#a-tile .ht').textContent === 'A deep linked title', `the tile has the link's title (${$('#a-tile .ht').textContent})`);
    const second = 'https://added.example/notes/two';
    location.hash = '#add=' + encodeURIComponent(second);
    await until(() => $('#a-link').value === second, 'the new fragment');
    check(document.activeElement === $('#a-go'), 'Add focused again');
    $('#a-go').click();
    await until(() => $('#a-search .val').textContent === 'Indexed' && $('#a-tile .host').textContent === 'added.example' && $('#a-tile .ht').textContent !== 'A deep linked title', 'Indexed');
    check(document.activeElement === $('#a-link') && $('#a-link').value === '', 'ready for the next link');
  },
  /* each failure shows its value and its one action; the action runs and the bar follows it */
  addFailures: async ({ $, until, check }) => {
    const seg = (k) => $(`#a-${k} .val`).textContent;
    const add = async (url) => {   // its first state shows for a whole poll
      $('#a-link').value = url;
      $('#a-link').dispatchEvent(new Event('input'));
      $('#a-form').requestSubmit();
      await until(() => seg('library') === 'Adding' && seg('search') === '', 'the new job');
    };
    await add('ftp://added.example/one');
    await until(() => seg('library') === 'Not a web page', 'refused');
    check($('#a-link').value === 'ftp://added.example/one' && $('#a-result').hidden && $('#a-act').hidden, 'the text stays, no tile, no action');
    check($('#a-library').dataset.k === 'bad', 'shown as a failure');
    await add('https://added.example/blocked');
    await until(() => seg('search') === 'Indexed', 'blocked, indexed');
    check(seg('content') === 'Blocked · 403' && seg('image') === 'No image' && $('#a-image').dataset.k === 'soft', `blocked reads ${seg('content')}`);
    check(!$('#a-result').hidden && $('#a-tile .th.none'), 'the tile, with no picture');
    check($('#a-act').textContent === 'Try signed in', 'Try signed in');
    $('#a-act').click();
    await until(() => seg('content') === 'Ok · Signed in' && seg('search') === 'Indexed', 'signed in');
    check(seg('library') === 'Already in library' && $('#a-act').hidden, 'known now; no action');
    await add('https://added.example/missing');
    await until(() => seg('search') === 'Indexed', 'not found, indexed');
    check(seg('content') === 'Not found · 404' && $('#a-act').textContent === 'Remove', 'Remove');
    $('#a-act').click();
    await until(() => seg('library') === 'Forgotten', 'removed');
    check($('#a-result').hidden && $('#a-act').textContent === 'Restore' && seg('content') === '', 'Forgotten offers Restore');
    $('#a-act').click();
    await until(() => seg('library') === 'Already in library' && seg('search') === 'Indexed', 'restored');
    check(!$('#a-result').hidden && $('#a-act').textContent === 'Remove', 'the tile is back');
    const actual = await window.KMT.api('/api/library');
    await until(() => window.KMT.lib.pages === actual.pages, 'the library count after Restore');
    await add('https://added.example/timeout');
    await until(() => seg('search') === 'Indexed', 'timed out');
    check(seg('content') === 'Timed out' && seg('image') === 'No image' && $('#a-act').textContent === 'Retry', 'Retry');
    $('#a-act').click();
    await until(() => seg('content') === 'Ok · Web' && seg('image') === 'Ok' && seg('search') === 'Indexed', 'the text, then its image');
    check(seg('library') === 'Already in library' && $('#a-act').hidden, 'no action');
  },
  /* Retry runs the failed stages only, row by row of the contract's lab mapping; terminal states offer nothing */
  addRetry: async ({ $, until, check }) => {
    const seg = (k) => $(`#a-${k} .val`).textContent, act = () => ($('#a-act').hidden ? '' : $('#a-act').textContent);
    const settled = async (what) => { await until(() => seg('search') === 'Indexed', what); };
    const press = async (label, what) => {
      check(act() === label, `${what}: ${label} (${act()})`);
      $('#a-act').click();
      await until(() => seg('search') === '', `${what}: a new job`);
      await settled(what);
    };
    const add = async (url) => {
      $('#a-link').value = url;
      $('#a-link').dispatchEvent(new Event('input'));
      $('#a-form').requestSubmit();
      await until(() => seg('library') === 'Adding' && seg('search') === '', 'the new job');
      await settled(url);
    };
    await add('https://added.example/error');
    check(seg('content') === 'Error · 503' && $('#a-content').dataset.k === 'bad', `error reads ${seg('content')}`);
    await press('Retry', 'error');
    check(seg('content') === 'Ok · Web' && act() === '', 'read again');
    for (const [name, value] of [['offline', 'Chrome not reachable'], ['asleep', 'Chrome not reachable'], ['refused', 'Not allowed']]) {
      await add(`https://added.example/${name}`);
      await press('Try signed in', name);
      check(seg('content') === value, `${name} reads ${seg('content')}`);
      await press('Retry', name);
      check(seg('content') === 'Ok · Signed in' && act() === '', `${name}: read signed in`);
    }
    await add('https://added.example/imagefail');
    check(seg('content') === 'Ok · Web' && seg('image') === 'Error' && $('#a-image').dataset.k === 'bad', `image reads ${seg('image')}`);
    await press('Retry', 'image');
    check(seg('image') === 'Ok' && act() === '', 'the image');
    await until(() => $('#a-tile .th img'), 'the picture on the tile');
    await add('https://added.example/bothfail');
    check(seg('content') === 'Error · 503' && seg('image') === 'Error', 'both failed');
    await press('Retry', 'both');
    check(seg('content') === 'Ok · Web' && seg('image') === 'Ok' && act() === '', 'both in one run');
    await add('https://added.example/stuck');
    await press('Retry', 'stuck');
    await press('Retry', 'stuck again');
    check(seg('content') === 'Unavailable' && act() === '', `terminal: ${seg('content')}, ${act()}`);
    await add('https://added.example/video');
    check(seg('content') === 'Not recorded' && seg('image') === 'No image' && act() === '', 'nothing recorded, nothing offered');
    await window.KMT.add.start('https://added.example/never', { action: 'add', retry: ['content'] });
    check(seg('library') === 'Not in library' && act() === '' && $('#a-result').hidden, 'a Retry never adds a page');
    const url = 'https://added.example/relapse';
    await add(url);
    await window.KMT.add.start(url, { action: 'forget' });
    check(act() === 'Restore', 'Forgotten offers Restore');
    $('#a-act').click();
    await until(() => seg('library') === 'Not added', 'the add after the restore failed');
    check(act() === 'Retry', 'Retry');
    $('#a-act').click();
    await until(() => seg('library') === 'Already in library' && seg('search') === 'Indexed', 'added, not restored again');
  },
  /* a web address pasted into the empty box starts at once */
  addPaste: async ({ $, until, check }) => {
    const data = new DataTransfer();
    data.setData('text/plain', ' https://added.example/pasted ');
    $('#a-link').dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
    await until(() => $('#a-search .val').textContent === 'Indexed', 'Indexed');
    check($('#a-link').value === '', 'the box is ready for the next link');
  },
  addSignedStages: async ({ $, until, check }) => {
    const seg = (k) => $(`#a-${k} .val`).textContent;
    for (const [name, status] of [['thin403', 'Ok · Signed in'], ['offlineimage', 'Chrome not reachable']]) {
      await window.KMT.add.start(`https://added.example/${name}`);
      check($('#a-act').textContent === 'Try signed in' && seg('image') === 'Error', 'signed in and image action');
      $('#a-act').click();
      await until(() => seg('content') === status && seg('image') === 'Ok' && seg('search') === 'Indexed', 'both selected stages settled');
      check($('#a-act').hidden === (name === 'thin403'), 'only unreachable Chrome offers Retry');
    }
  },
  addSnapshot: async ({ $, until, check }) => {
    for (const ask of [{ action: 'add' }, { action: 'add', signed_in: true }, { action: 'add', retry: ['content'] }, { action: 'forget' }, { action: 'restore' }]) {
      // Stale output from a previous job must leave with the refusal.
      for (const stage of ['library', 'content', 'image', 'search']) $(`#a-${stage} .val`).textContent = 'Old state';
      $('#a-result').hidden = false;
      await window.KMT.add.start('https://added.example/protected', ask);
      check($('#a-library .val').textContent === 'Read only snapshot', 'the bar explains the snapshot refusal');
      for (const stage of ['content', 'image', 'search']) check($(`#a-${stage} .val`).textContent === '', 'old stages cleared');
      check($('#a-result').hidden && $('#a-act').hidden, 'no tile or retry action');
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
    if (name === 'startupDuringPage') await send('Page.addScriptToEvaluateOnNewDocument', { source: `
      const fetchNormally = window.fetch;
      window.searchRequests = 0;
      window.fetch = async (...args) => {
        if (args[0] === '/api/search') {
          window.searchRequests++;
          if (JSON.parse(args[1].body).offset === 50) {
            window.nextWaiting = true;
            await new Promise((resolve) => { window.releaseNext = resolve; });
          }
        }
        if (args[0] === '/api/pins') {
          window.pinsWaiting = true;
          await new Promise((resolve) => { window.releasePins = resolve; });
          const response = await fetchNormally(...args);
          const read = response.json.bind(response);
          response.json = async () => { const out = await read(); window.pinsRead = true; return out; };
          return response;
        }
        return fetchNormally(...args);
      };
    ` }, sessionId);
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
