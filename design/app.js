/* knowmoretabs — library behaviour.
   slice: library
   why: One renderer, two hosts. Everything that knows whether the data came
        from an embedded blob or from the API lives in HOST below; the rest of
        this file only ever asks "is this host live?". */

'use strict';

const el = document.getElementById.bind(document);

/* ---- the mode seam ------------------------------------------------------ */

const BLOB = (() => {
  const node = el('library');
  try { return node ? JSON.parse(node.textContent) : null; } catch { return null; }
})();

async function api(path, body) {
  const res = await fetch(path, body && {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(path + ' → ' + res.status);
  return res.json();
}

const HOST = BLOB ? {
  mode: 'export',
  live: false,
  load: async () => BLOB,
} : {
  mode: 'serve',
  live: true,
  load: () => api('api/library'),
  forget: urls => api('api/forget', { urls }),
  restore: urls => api('api/restore', { urls }),
};

document.documentElement.dataset.mode = HOST.mode;

/* ---- state -------------------------------------------------------------- */

/* FIRST is sized to more than fill a tall window, so the first frame is always
   a complete screen; CHUNK is sized so a 2000-row library finishes in two more
   frames while a pathologically large one still yields between them. */
const FIRST = 80, CHUNK = 1500;
let DOC, SNAPS, PAGES, NOW, byUrl, bySnapshot = null, arrivals;
const view = { rows: [], cursor: -1, open: new Set(), picked: new Set(), undo: null, terms: [] };

const COLLATE = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
const utc = o => new Intl.DateTimeFormat(undefined, Object.assign({ timeZone: 'UTC' }, o));
const DATE = utc({ day: 'numeric', month: 'short', year: 'numeric' });
const SHORT = utc({ day: 'numeric', month: 'short' });
const CLOCK = utc({ hour: '2-digit', minute: '2-digit', hour12: false });

const ENT = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' };
const esc = s => String(s).replace(/[&<>"]/g, c => ENT[c]);
const num = n => n.toLocaleString();

function since(i) {
  const d = (NOW - SNAPS[i].t) / 864e5;
  if (d < 1.5) return 'today';
  if (d < 14) return Math.round(d) + 'd';
  if (d < 70) return Math.round(d / 7) + 'w';
  if (d < 730) return Math.round(d / 30.4) + 'mo';
  return (d / 365).toFixed(1) + 'y';
}

/* Highlight the longest search term so a half-remembered word is findable by
   eye as well as by filter. One occurrence is enough to aim at. */
function hl(text, terms) {
  if (!terms.length) return esc(text);
  let term = terms[0];
  for (const t of terms) if (t.length > term.length) term = t;
  const at = text.toLowerCase().indexOf(term);
  if (at < 0) return esc(text);
  return esc(text.slice(0, at)) + '<mark>' + esc(text.slice(at, at + term.length)) +
    '</mark>' + esc(text.slice(at + term.length));
}

/* ---- the barcode -------------------------------------------------------- */

/* Every page carries the same 40-snapshot axis, so the marks line up down the
   whole column: a solid bar is something you never close, a single tick on the
   left is something you opened once in March. Runs are merged, which keeps the
   gradient short for exactly the pages that would otherwise be longest. */
function barcode(p) {
  if (p.bc) return p.bc;
  const n = SNAPS.length, step = 100 / n, xs = [];
  for (let k = p.sightings.length - 1; k >= 0; k--) xs.push(n - 1 - p.sightings[k].snapshot);
  let out = '', at = 0;
  for (let k = 0; k < xs.length; k++) {
    const a = xs[k];
    let b = a + 1;
    while (k + 1 < xs.length && xs[k + 1] === b) { b++; k++; }
    if (a > at) out += 'transparent 0 ' + (a * step).toFixed(2) + '%,';
    out += 'var(--tick) 0 ' + (b * step).toFixed(2) + '%,';
    at = b;
  }
  if (at < n) out += 'transparent 0 100%,';
  return (p.bc = 'linear-gradient(90deg,' + out.slice(0, -1) + ')');
}

/* ---- rows --------------------------------------------------------------- */

const link = (url, body, cls) => /^https?:\/\//i.test(url)
  ? `<a${cls ? ` class="${cls}"` : ''} href="${esc(url)}" target="_blank" rel="noopener noreferrer">${body}</a>`
  : `<span${cls ? ` class="${cls}"` : ''}>${body}</span>`;

const shellQuote = url => esc("'" + url.replace(/'/g, "'\\''") + "'");

function rowHtml(p) {
  const name = p.title || p.addr, open = view.open.has(p.url);
  return `<li class="page${p.forgotten ? ' gone' : ''}" data-i="${p.i}" tabindex="-1"><div class="row">` +
    (HOST.live ? `<input class="pick" type="checkbox" aria-label="Select ${esc(name)}"${
      view.picked.has(p.url) ? ' checked' : ''}>` : '') +
    `<span class="seen"><span class="n">${p.n}×</span><span class="bar" role="img"` +
    ` aria-label="Seen in ${p.n} of ${SNAPS.length} snapshots"></span></span>` +
    `<span class="main">${link(p.url, hl(name, view.terms), 't')}` +
    `<span class="u">${hl(p.addr, view.terms)}</span></span>` +
    (p.open ? '<span class="when now">open</span>'
      : `<time class="when" datetime="${SNAPS[p.last].captured_at}">${since(p.last)}</time>`) +
    `<button class="disc" type="button" aria-expanded="${open}"` +
    ` aria-label="History of ${esc(name)}"></button></div>${open ? panelHtml(p) : ''}</li>`;
}

function panelHtml(p) {
  const hist = p.sightings.map(s => {
    const snap = SNAPS[s.snapshot];
    return `<li><a href="#snapshot=${snap.id}&tab=${s.tab}">${SHORT.format(snap.t)} ` +
      `${CLOCK.format(snap.t)}</a><span class="where">Window ${s.window} · tab ${s.position + 1}` +
      `</span>${s.pinned ? '<span class="pin">pinned</span>' : ''}</li>`;
  }).join('');
  const acts = HOST.live
    ? `<button class="ghost act" type="button">${p.forgotten ? 'Restore' : 'Forget'} this page</button>` +
      '<p class="why">Hides it from the library. The snapshots themselves are never touched.</p>'
    : `<code>knowmoretabs ${p.forgotten ? 'restore' : 'forget'} ${shellQuote(p.url)}</code>` +
      '<p class="why">This export is read-only. Run that, then export again.</p>';
  return `<div class="panel"><p class="full">${link(p.url, esc(p.url))}</p>` +
    `<h3>Seen ${p.n}× · first ${DATE.format(SNAPS[p.first].t)} ` +
    `· last ${DATE.format(SNAPS[p.last].t)}</h3>` +
    `<ol class="hist">${hist}</ol><div class="acts">${acts}</div></div>`;
}

/* ---- painting ----------------------------------------------------------- */

/* Rows go in as HTML in chunks, not as a virtual window. The first chunk lands
   in one frame so the page is never blank, the rest arrive over the next few,
   and `content-visibility: auto` means the browser skips layout and paint for
   everything below the fold. Keeping real elements keeps find-in-page, tab
   order, scroll anchoring and variable-height disclosures working for free. */
let token = 0;

function paint() {
  const mine = ++token, list = el('pages'), rows = view.rows;
  const t0 = performance.now();
  list.replaceChildren();
  let i = 0;

  let work = 0;
  const step = () => {
    if (mine !== token) return;
    const t1 = performance.now();
    const end = Math.min(rows.length, i + (i ? CHUNK : FIRST));
    let html = '';
    for (let k = i; k < end; k++) html += rowHtml(rows[k]);
    list.insertAdjacentHTML('beforeend', html);
    for (let k = i; k < end; k++) {
      list.children[k].querySelector('.bar').style.setProperty('--barcode', barcode(rows[k]));
    }
    work += performance.now() - t1;
    if (i === 0) window.__first = work;
    i = end;
    if (i < rows.length) requestAnimationFrame(step);
    else { window.__work = work; window.__wall = performance.now() - t0; markCursor(); }
  };
  step();
}

function markCursor(scroll) {
  const list = el('pages').matches('[hidden] *') ? el('snaps') : el('pages');
  for (const hit of document.querySelectorAll('.cursor')) hit.classList.remove('cursor');
  const node = view.cursor >= 0 && list.children[view.cursor];
  if (!node) return;
  node.classList.add('cursor');
  if (scroll) { node.focus({ preventScroll: true }); node.scrollIntoView({ block: 'nearest' }); }
}

/* ---- filter and sort ---------------------------------------------------- */

const ORDER = {
  last:  (a, b) => a.last - b.last || a.i - b.i,
  first: (a, b) => a.first - b.first || a.i - b.i,
  count: (a, b) => b.n - a.n || a.last - b.last,
  title: (a, b) => COLLATE.compare(a.title || a.addr, b.title || b.addr),
  url:   (a, b) => COLLATE.compare(a.addr, b.addr),
};

function apply(keepCursor) {
  const form = el('filters').elements;
  const q = el('q').value.trim().toLowerCase();
  view.terms = q ? q.split(/\s+/) : [];
  /* The site box is a typeahead over 150-odd hosts, so an exact pick from the
     datalist and a half-typed "wiki" both have to mean something. */
  const dom = form.domain.value.trim().toLowerCase();
  const state = form.state.value;
  const rows = [];

  outer: for (const p of PAGES) {
    if (state === 'forgotten') { if (!p.forgotten) continue; }
    else if (p.forgotten) continue;
    if (dom && !p.domain.includes(dom)) continue;
    if (state === 'open' && !p.open) continue;
    if (state === 'closed' && p.open) continue;
    if (state === 'once' && p.n !== 1) continue;
    if (state === 'often' && p.n < 5) continue;
    for (const t of view.terms) if (!p.hay.includes(t)) continue outer;
    rows.push(p);
  }

  rows.sort(ORDER[form.sort.value]);
  view.rows = rows;
  if (!keepCursor) view.cursor = -1;
  else if (view.cursor >= rows.length) view.cursor = rows.length - 1;

  const open = rows.reduce((n, p) => n + (p.open ? 1 : 0), 0);
  const all = state === 'forgotten' ? DOC.counts.forgotten : PAGES.length - DOC.counts.forgotten;
  el('count').innerHTML = `<b>${num(rows.length)}</b>${
    rows.length === all ? '' : ' of ' + num(all)} pages · ${num(open)} still open`;
  el('empty').hidden = rows.length > 0;
  el('pages').hidden = el('listhead').hidden = rows.length === 0;
  paint();
}

/* ---- forget, restore, undo ---------------------------------------------- */

function say(html, flags) {
  const bar = el('actionbar');
  bar.hidden = false;
  el('action-text').innerHTML = html;
  bar.classList.toggle('has-selection', view.picked.size > 0);
  bar.classList.toggle('has-undo', !!view.undo);
  if (flags === 'idle' && !view.picked.size && !view.undo && !HOST.live) bar.hidden = true;
}

function selectionSay() {
  if (!HOST.live) return;
  const n = view.picked.size;
  say(n ? '<b>' + n + '</b> selected' : 'Select rows with <kbd>x</kbd> or the checkboxes, then forget them together.');
}

async function change(verb, urls) {
  if (!HOST.live || !urls.length) return;
  try {
    const res = await HOST[verb](urls);
    for (const url of urls) byUrl.get(url).forgotten = verb === 'forget';
    if (res && res.counts) DOC.counts = res.counts;
    else DOC.counts.forgotten += (verb === 'forget' ? 1 : -1) * urls.length;
    view.undo = { verb: verb === 'forget' ? 'restore' : 'forget', urls };
    view.picked.clear();
    apply(true);
    tally();
    say(`${verb === 'forget' ? 'Forgot' : 'Restored'} <b>${urls.length}</b> page` +
      `${urls.length === 1 ? '' : 's'}. Nothing was deleted.`);
  } catch (err) {
    say(`Could not reach the archive: ${esc(err.message)}. Nothing changed.`);
  }
}

function forgetOne(p) {
  if (HOST.live) return change(p.forgotten ? 'restore' : 'forget', [p.url]);
  say(`Read-only export. Run <code>knowmoretabs forget ${shellQuote(p.url)}</code> to hide it.`);
}

/* ---- snapshots ---------------------------------------------------------- */

function index() {
  if (bySnapshot) return;
  bySnapshot = SNAPS.map(() => []);
  arrivals = SNAPS.map(() => 0);
  for (const p of PAGES) {
    arrivals[p.first]++;
    for (const s of p.sightings) bySnapshot[s.snapshot].push([s.window, s.position, s.tab, p]);
  }
  for (const list of bySnapshot) list.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
}

function paintSnaps() {
  index();
  const top = Math.max(...SNAPS.map(s => s.stats.tabs));
  const list = el('snaps');
  list.innerHTML = SNAPS.map((s, i) => {
    const st = s.stats, notes = [];
    if (arrivals[i]) notes.push(`<b>${arrivals[i]}</b> new`);
    /* Surfaced, not swallowed: a snapshot the parser only partly understood is
       still a good snapshot, and you should be able to see that it happened. */
    if (st.unknown_commands) notes.push(`<span class="warn">${st.unknown_commands} unknown</span>`);
    if (st.truncated_bytes) notes.push(`<span class="warn">torn tail</span>`);
    return `<li data-s="${i}" tabindex="-1"><div class="srow">` +
      `<span class="sdate"><time datetime="${s.captured_at}">${DATE.format(s.t)}</time>` +
      `<span>${CLOCK.format(s.t)} UTC · ${esc(s.browser)}</span></span>` +
      `<span class="gauge" data-w="${(st.tabs / top * 100).toFixed(1)}"><i></i></span>` +
      `<span class="snums"><b>${st.tabs}</b> tabs · <b>${st.windows}</b> windows` +
      `${notes.length ? ' · ' + notes.join(' · ') : ''}</span>` +
      `<button class="disc sdisc" type="button" aria-expanded="false"` +
      ` aria-label="Tabs in the ${DATE.format(s.t)} snapshot"></button></div></li>`;
  }).join('');
  for (const g of list.querySelectorAll('.gauge')) {
    g.firstElementChild.style.setProperty('--w', g.dataset.w + '%');
  }
}

function snapPanel(i) {
  const wins = new Map();
  for (const [w, , tab, p] of bySnapshot[i]) {
    if (!wins.has(w)) wins.set(w, []);
    wins.get(w).push(`<li data-tab="${tab}">${link(p.url, esc(p.title || p.addr))}</li>`);
  }
  return '<div class="windows">' + [...wins].map(([w, items]) =>
    `<div class="win"><h3>Window ${w} · ${items.length} tabs</h3><ol>${items.join('')}</ol></div>`
  ).join('') + '</div>';
}

function toggleSnap(li, force) {
  const btn = li.querySelector('.sdisc');
  const open = force !== undefined ? force : btn.getAttribute('aria-expanded') === 'false';
  btn.setAttribute('aria-expanded', open);
  const panel = li.querySelector('.windows');
  if (open && !panel) li.insertAdjacentHTML('beforeend', snapPanel(+li.dataset.s));
  else if (!open && panel) panel.remove();
}

/* ---- views and routing -------------------------------------------------- */

function show(name) {
  for (const which of ['pages', 'snapshots']) {
    const on = which === name;
    el('view-' + which).hidden = !on;
    const tab = el('tab-' + which);
    tab.setAttribute('aria-selected', on);
    tab.tabIndex = on ? 0 : -1;
  }
  view.cursor = -1;
  if (name === 'snapshots' && !el('snaps').children.length) paintSnaps();
}

function route() {
  const q = new URLSearchParams(location.hash.slice(1));
  const id = q.get('snapshot');
  if (!id) return;
  const i = SNAPS.findIndex(s => s.id === id);
  if (i < 0) return;
  show('snapshots');
  el('tab-snapshots').focus();
  const li = el('snaps').children[i];
  toggleSnap(li, true);
  const tab = q.get('tab');
  const hit = tab && li.querySelector('[data-tab="' + CSS.escape(tab) + '"]');
  for (const old of el('snaps').querySelectorAll('.target')) old.classList.remove('target');
  (hit || li).scrollIntoView({ block: 'center' });
  if (hit) hit.classList.add('target');
}

/* ---- tally -------------------------------------------------------------- */

function tally() {
  const open = PAGES.reduce((n, p) => n + (p.open && !p.forgotten ? 1 : 0), 0);
  el('tally').innerHTML = [
    ['Pages kept', PAGES.length - DOC.counts.forgotten],
    ['Still open', open, 'lit'],
    ['Snapshots', SNAPS.length],
    ['Sites', DOC.counts.domains],
    ['Forgotten', DOC.counts.forgotten],
  ].map(([label, n, cls]) =>
    `<div${cls ? ` class="${cls}"` : ''}><dt>${label}</dt><dd>${num(n)}</dd></div>`).join('');
}

/* ---- keyboard ----------------------------------------------------------- */

function rowAt(i) { return el('pages').children[i]; }

function move(delta, to) {
  const onPages = !el('view-pages').hidden;
  const len = onPages ? view.rows.length : SNAPS.length;
  if (!len) return;
  view.cursor = to !== undefined
    ? (to < 0 ? len - 1 : 0)
    : Math.max(0, Math.min(len - 1, view.cursor + delta));
  markCursor(true);
}

function current() {
  return !el('view-pages').hidden && view.cursor >= 0 ? view.rows[view.cursor] : null;
}

function toggleRow(p, node) {
  const li = node || rowAt(view.rows.indexOf(p));
  if (!li) return;
  const open = !view.open.has(p.url);
  li.querySelector('.disc').setAttribute('aria-expanded', open);
  if (open) { view.open.add(p.url); li.insertAdjacentHTML('beforeend', panelHtml(p)); }
  else { view.open.delete(p.url); li.querySelector('.panel').remove(); }
}

function togglePick(p, node) {
  if (!HOST.live) return;
  if (view.picked.has(p.url)) view.picked.delete(p.url); else view.picked.add(p.url);
  const box = (node || rowAt(view.rows.indexOf(p)))?.querySelector('.pick');
  if (box) box.checked = view.picked.has(p.url);
  selectionSay();
}

addEventListener('keydown', e => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const field = /^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName);
  const key = e.key;

  if (key === 'Escape') {
    if (el('q').value) { el('q').value = ''; apply(); }
    view.picked.clear();
    view.cursor = -1;
    markCursor();
    selectionSay();
    if (field) e.target.blur();
    return;
  }
  if (field || el('help').open) return;

  if (key === '/') { e.preventDefault(); el('q').focus(); el('q').select(); return; }
  if (key === '?') { e.preventDefault(); el('help').showModal(); return; }
  if (key === 'j' || key === 'ArrowDown') { e.preventDefault(); move(1); return; }
  if (key === 'k' || key === 'ArrowUp') { e.preventDefault(); move(-1); return; }
  if (key === 'g') { e.preventDefault(); move(0, 0); return; }
  if (key === 'G') { e.preventDefault(); move(0, -1); return; }
  if (key === 'u' && view.undo) { e.preventDefault(); change(view.undo.verb, view.undo.urls); return; }

  if (!el('view-pages').hidden) {
    const p = current();
    if (!p) return;
    if (key === 'Enter') { e.preventDefault(); if (p.safe) rowAt(view.cursor).querySelector('.t').click(); }
    else if (key === 'o') { e.preventDefault(); toggleRow(p); }
    else if (key === 'f') { e.preventDefault(); forgetOne(p); }
    else if (key === 'x') { e.preventDefault(); togglePick(p); }
  } else if (view.cursor >= 0 && (key === 'Enter' || key === 'o')) {
    e.preventDefault();
    toggleSnap(el('snaps').children[view.cursor]);
  }
});

/* ---- wiring ------------------------------------------------------------- */

function wire() {
  el('filters').addEventListener('input', () => apply());
  el('filters').addEventListener('reset', () => setTimeout(apply, 0));
  el('empty-clear').addEventListener('click', () => { el('filters').reset(); apply(); el('q').focus(); });

  el('pages').addEventListener('click', e => {
    const li = e.target.closest('li');
    if (!li) return;
    const p = PAGES[+li.dataset.i];
    view.cursor = [...el('pages').children].indexOf(li);
    if (e.target.closest('.disc')) { toggleRow(p, li); markCursor(); }
    else if (e.target.closest('.pick')) togglePick(p, li);
    else if (e.target.closest('.act')) forgetOne(p);
    else if (!e.target.closest('a')) markCursor();
  });

  el('snaps').addEventListener('click', e => {
    const li = e.target.closest('li[data-s]');
    if (li && e.target.closest('.sdisc')) toggleSnap(li);
  });

  for (const which of ['pages', 'snapshots']) {
    el('tab-' + which).addEventListener('click', () => show(which));
  }
  el('tab-pages').parentNode.addEventListener('keydown', e => {
    const dir = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!dir) return;
    const next = el(e.target.id === 'tab-pages' ? 'tab-snapshots' : 'tab-pages');
    show(next.id.slice(4));
    next.focus();
  });

  el('open-help').addEventListener('click', () => el('help').showModal());
  el('select-all').addEventListener('click', () => {
    for (const p of view.rows) view.picked.add(p.url);
    for (const box of el('pages').querySelectorAll('.pick')) box.checked = true;
    selectionSay();
  });
  el('deselect').addEventListener('click', () => {
    view.picked.clear();
    for (const box of el('pages').querySelectorAll('.pick')) box.checked = false;
    selectionSay();
  });
  el('bulk-forget').addEventListener('click', () => change('forget', [...view.picked]));
  el('undo').addEventListener('click', () => view.undo && change(view.undo.verb, view.undo.urls));

  const modes = ['auto', 'light', 'dark'];
  el('theme').addEventListener('click', e => {
    const next = modes[(modes.indexOf(localStorage.kmtTheme || 'auto') + 1) % 3];
    try { localStorage.kmtTheme = next; } catch (_) { /* private mode */ }
    theme(next);
    e.target.focus();
  });
  addEventListener('hashchange', route);
}

function theme(name) {
  document.documentElement.dataset.theme = name === 'auto' ? '' : name;
  el('theme').textContent = 'Theme: ' + name;
}

/* ---- boot --------------------------------------------------------------- */

async function boot() {
  try { theme(localStorage.kmtTheme || 'auto'); } catch (_) { theme('auto'); }

  try { DOC = await HOST.load(); } catch (err) {
    el('count').innerHTML = 'Could not load the library: ' + esc(err.message) +
      '. Is <code>knowmoretabs serve</code> still running?';
    return;
  }

  SNAPS = DOC.snapshots;
  PAGES = DOC.pages;
  NOW = Date.parse(DOC.generated_at);
  for (const s of SNAPS) s.t = Date.parse(s.captured_at);

  byUrl = new Map();
  PAGES.forEach((p, i) => {
    p.i = i;
    p.n = p.sightings.length;
    p.last = p.sightings[0].snapshot;
    p.first = p.sightings[p.n - 1].snapshot;
    p.open = p.last === 0;
    p.safe = /^https?:\/\//i.test(p.url);
    p.addr = p.url.replace(/^https?:\/\/(www\.)?/i, '');
    p.hay = (p.title + ' ' + p.url).toLowerCase();
    byUrl.set(p.url, p);
  });

  const counts = new Map();
  for (const p of PAGES) counts.set(p.domain, (counts.get(p.domain) || 0) + 1);
  el('domains').innerHTML = [...counts.keys()]
    .sort((a, b) => counts.get(b) - counts.get(a) || COLLATE.compare(a, b))
    .map(d => '<option value="' + esc(d) + '">' + counts.get(d) + ' pages</option>').join('');

  el('root').textContent = DOC.root || 'the archive';
  el('hint').innerHTML = 'Press <kbd>/</kbd> to search, <kbd>j</kbd> <kbd>k</kbd> to move, ' +
    '<kbd>?</kbd> for the rest.';
  el('cli-hint').textContent = HOST.live
    ? 'Serving from the API · knowmoretabs save  ·  knowmoretabs export'
    : 'Offline export · knowmoretabs serve for live forget and restore';
  el('key-f-note').textContent = HOST.live ? '' : '(shows the command — this export is read-only)';

  tally();
  wire();
  apply();
  if (HOST.live) { el('actionbar').hidden = false; selectionSay(); }
  if (location.hash) requestAnimationFrame(route);
}

boot();
