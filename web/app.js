// knowmoretabs · library UI
// slice: library (triage when the host is `serve`)
// why:   One renderer, two hosts. The export page embeds its data; serve
//        fetches it and turns forget/restore into live buttons. Everything
//        below the host seam is host-agnostic.
'use strict';

// ---- 1. Host seam: the only place export and serve differ -----------------
const host = (() => {
  const blob = document.getElementById('library-data');
  const text = blob ? blob.textContent.trim() : '';
  if (text) return { mode: 'export', load: async () => JSON.parse(text) };
  const json = async (r) => { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); };
  const post = (path, urls) => fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ urls }) }).then(json);
  return {
    mode: 'serve',
    load: () => fetch('api/library').then(json),
    forget: (urls) => post('api/forget', urls),
    restore: (urls) => post('api/restore', urls),
  };
})();

// ---- 2. State -------------------------------------------------------------
const S = { pages: [], snaps: [], stats: {}, groups: new Map(), shown: [], rendered: [], q: '', domain: '', group: '', status: '', sort: 'last',
            openAll: false, cur: -1, anchor: -1, sel: new Set(), exp: new Set(), undo: null, view: 'pages' };
const FOLD = 10;                                   // rows of "Open now" shown before "show all"
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const num = new Intl.NumberFormat();
const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
const fmt = (opts) => new Intl.DateTimeFormat(undefined, { timeZone: 'UTC', ...opts });
const F = { day: fmt({ day: 'numeric', month: 'short' }), dayYear: fmt({ day: 'numeric', month: 'short', year: 'numeric' }),
            month: fmt({ month: 'long', year: 'numeric' }), long: fmt({ weekday: 'short', day: 'numeric', month: 'long', year: 'numeric' }),
            time: fmt({ hour: '2-digit', minute: '2-digit', hour12: false }), full: fmt({ day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) };
const isForm = (el) => el && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName);
const plural = (n, one, many = one + 's') => `${num.format(n)} ${n === 1 ? one : many}`;
const SORTS = {
  last: (a, b) => b.last - a.last || a.lw - b.lw || a.lp - b.lp,
  first: (a, b) => b.first - a.first || a.lw - b.lw || a.lp - b.lp,
  count: (a, b) => b.n - a.n || b.last - a.last || a.lw - b.lw,
  title: (a, b) => collator.compare(a.name, b.name) || b.last - a.last,
  url: (a, b) => collator.compare(a.addr, b.addr),
};

// ---- 3. Derive pages ⇄ snapshots ------------------------------------------
function derive(lib) {
  S.stats = lib.stats || {};
  S.snaps = (lib.snapshots || []).map((s, k) => ({ ...s, k, date: new Date(s.captured_at), groups: s.groups || [], fresh: 0, gone: 0 }));
  S.pages = (lib.pages || []).map((p, i) => ({ ...p, i, seen: [], title: p.title || '', forgotten: !!p.forgotten,
    hay: ((p.title || '') + ' ' + p.url).toLowerCase(), addr: p.url.replace(/^https?:\/\/(www\.)?/i, ''),
    name: (p.title || p.url).replace(/^[^\p{L}\p{N}]+/u, ''), link: /^https?:\/\//i.test(p.url) }));
  // Each sighting keeps the group object it sat in, so a page can be in "Papers" in
  // March, "Later" in June and no group today without any per-page bookkeeping.
  for (const s of S.snaps) for (const [pi, w, pos, tid, , g] of s.tabs) S.pages[pi]?.seen.push([s.k, w, tid, pos, g == null ? null : s.groups[g] || null]);
  const latest = S.snaps.length - 1, col = 4;
  S.groups = new Map();
  for (const p of S.pages) {
    if (!p.seen.length) { p.n = 0; continue; }      // allowed by the contract, never shown
    const ks = [...new Set(p.seen.map((x) => x[0]))];
    p.n = ks.length; p.first = ks[0]; p.last = ks.at(-1); p.open = p.last === latest;
    const l = p.seen.at(-1); p.lw = l[1]; p.lp = l[3];
    p.grp = (p.seen.find((x) => x[0] === p.last && x[4]) || l)[4];   // the group it sat in when last seen
    p.gs = [...new Set(p.seen.map((x) => x[4] && x[4].title.toLowerCase()).filter(Boolean))];
    for (const t of p.gs) { const e = S.groups.get(t) || { title: p.seen.find((x) => x[4] && x[4].title.toLowerCase() === t)[4].title, n: 0 }; e.n++; S.groups.set(t, e); }
    S.snaps[p.first].fresh++; if (!p.open) S.snaps[p.last].gone++;
    const stops = []; let a = ks[0], b = ks[0];        // runs of consecutive sightings → gradient stops
    for (const k of ks.slice(1).concat(NaN)) {
      if (k === b + 1) { b = k; continue; }
      stops.push(`transparent ${a * col}px,var(--dot) ${a * col}px ${(b + 1) * col}px,transparent ${(b + 1) * col}px`);
      a = b = k;
    }
    p.g = `linear-gradient(90deg,${stops.join(',')})`;
  }
  document.documentElement.style.setProperty('--n', S.snaps.length);
}
// Optional per-snapshot parser counters. Absent from the contract today; when
// present, a snapshot that lost tabs says so instead of quietly reporting fewer.
function degraded(s) {
  const t = s.stats; if (!t) return '';
  const parts = [];
  if (t.dropped_tabs) parts.push(plural(t.dropped_tabs, 'tab') + ' dropped');
  if (t.unknown_commands) parts.push(plural(t.unknown_commands, 'unknown record'));
  if (t.malformed_commands) parts.push(plural(t.malformed_commands, 'malformed record'));
  if (t.truncated_bytes) parts.push(plural(t.truncated_bytes, 'byte') + ' truncated');
  if (t.marker_ok === false) parts.push('no end marker');
  if (!parts.length && t.degraded) parts.push('parse degraded');
  return parts.join(', ');
}

// ---- 4. Pages view ---------------------------------------------------------
function compute() {
  const words = S.q.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const wantForgotten = S.status === 'forgotten';
  const d = S.domain.trim().toLowerCase(), exact = d && S.pages.some((p) => p.domain === d);
  const g = S.group.trim().toLowerCase(), gexact = g && S.groups.has(g);
  S.shown = S.pages.filter((p) => p.n && p.forgotten === wantForgotten && (!d || (exact ? p.domain === d : p.domain.includes(d)))
    && (!g || p.gs.some((t) => (gexact ? t === g : t.includes(g))))
    && (S.status !== 'open' || p.open) && (S.status !== 'closed' || !p.open)
    && words.every((w) => p.hay.includes(w))).sort(SORTS[S.sort]);
}
function bandOf(p) {
  if (S.sort === 'last') return p.open ? 'Open now' : F.month.format(S.snaps[p.last].date);
  if (S.sort === 'first') return F.month.format(S.snaps[p.first].date);
  return '';
}
// A group mark: a dot in the group's colour and its name. Nothing else on the
// page takes the colour, so nine of them can coexist with the one accent.
function grpHTML(g, btn, state) {
  if (!g) return '';
  const label = g.title ? `Group: ${esc(g.title)}` : `Unnamed ${esc(g.colour)} group`;
  btn = btn && !!g.title;                         // an unnamed group cannot be typed into the filter
  const tag = btn ? `button type="button" tabindex="-1" data-act="group" aria-label="Only ${label}"` : 'span';
  return `<${tag} class="grp" data-c="${esc(g.colour)}" title="${label}">${esc(g.title)}${state && g.collapsed ? '<small>collapsed</small>' : ''}</${btn ? 'button' : 'span'}>`;
}
function titleHTML(p) {
  const inner = p.title ? esc(p.title) : '<i>untitled</i>';
  return p.link ? `<a class="t" dir="auto" href="${esc(p.url)}" target="_blank" rel="noopener noreferrer" tabindex="-1">${inner}</a>` : `<span class="t" dir="auto">${inner}</span>`;
}
function rowHTML(p) {
  const s = S.snaps[p.last], y = s.date.getUTCFullYear() !== S.snaps.at(-1).date.getUTCFullYear();
  return `<li class="row${p.open ? ' open' : ''}${S.sel.has(p.i) ? ' sel' : ''}${p.i === S.cur ? ' cur' : ''}" data-i="${p.i}" tabindex="${p.i === S.cur ? 0 : -1}">` +
    `<span class="pick"><input type="checkbox" tabindex="-1" aria-label="Select"${S.sel.has(p.i) ? ' checked' : ''}></span>` +
    `<span class="body">${titleHTML(p)}<span class="u">${esc(p.addr)}</span>${grpHTML(p.grp, true)}</span>` +
    `<span class="strip" title="Seen in ${p.n} of ${S.snaps.length} snapshots"></span><span class="n">${p.n}</span>` +
    `<time class="last" datetime="${s.captured_at}">${(y ? F.dayYear : F.day).format(s.date)}</time>` +
    `<button class="more" type="button" tabindex="-1" aria-label="History" aria-expanded="${S.exp.has(p.i)}">›</button>` +
    (S.exp.has(p.i) ? histHTML(p) : '') + '</li>';
}
function histHTML(p) {
  const sightings = p.seen.map(([k, w, tid, pos, g]) => { const s = S.snaps[k];
    return `<li><a href="#snapshot/${esc(s.id)}/${tid}">${F.full.format(s.date)}</a><span>window ${w} · tab ${pos + 1}</span>${grpHTML(g)}</li>`; }).reverse();
  const groups = p.gs.length ? ` · in ${p.gs.length === 1 ? 'group' : 'groups'} ${p.gs.map((t) => esc(S.groups.get(t).title)).join(', ')}` : '';
  const act = p.forgotten ? '<button type="button" data-act="restore">Restore</button><span class="ro">Puts it back in the library.</span>'
    : '<button type="button" data-act="forget">Forget</button><span class="ro">Hides it from the library. The snapshots themselves are never touched.</span>';
  const ro = `<span class="ro">Read-only export · to hide this page: <code>knowmoretabs forget '${esc(p.url)}'</code></span>`;
  return `<div class="hist"><p class="full">${esc(p.url)}</p>` +
    `<p class="sum">Seen in ${p.n} of ${S.snaps.length} snapshots · first ${F.dayYear.format(S.snaps[p.first].date)} · last ${F.dayYear.format(S.snaps[p.last].date)}${p.open ? ' · open now' : ''}${groups}</p>` +
    `<ol>${sightings.join('')}</ol><p class="acts">${p.domain ? `<button type="button" data-act="domain">Only ${esc(p.domain)}</button>` : ''}<button type="button" data-act="copy">Copy URL</button>${host.forget ? act : ro}</p></div>`;
}
function render() {
  const t0 = performance.now();
  compute();
  // "Open now" folds to its first rows when nothing is filtered, so the archive
  // starts on screen. Any filter, or "show all", unfolds it.
  const plain = S.sort === 'last' && S.status === '' && !S.q.trim() && !S.domain.trim() && !S.group.trim();
  const fold = (n, all) => `<li class="fold"><button type="button" id="fold">${all ? `Show all ${plural(n, 'open page')}` : 'Show fewer'}</button></li>`;
  let html = '', band = null, inBand = 0, tail = '';
  const nOpen = plain ? S.shown.filter((x) => x.open).length : 0;
  S.rendered = [];
  for (const p of S.shown) {
    const b = bandOf(p);
    if (b !== band) { html += (band === null ? '' : tail + '</ol></section>') + `<section class="band">${b ? `<h3>${esc(b)}<small>${num.format(S.shown.filter((x) => bandOf(x) === b).length)}</small></h3>` : ''}<ol>`; band = b; inBand = 0; tail = ''; }
    if (plain && p.open && ++inBand > FOLD) { tail = fold(nOpen, !S.openAll); if (!S.openAll) continue; }
    S.rendered.push(p); html += rowHTML(p);
  }
  const list = $('list');
  list.innerHTML = html + (band === null ? '' : tail + '</ol></section>');
  const strips = list.querySelectorAll('.strip');                 // CSSOM, because CSP forbids style attributes
  S.rendered.forEach((p, i) => strips[i].style.setProperty('--g', p.g));
  if (S.rendered.length && !list.querySelector('.row.cur')) list.querySelector('.row').tabIndex = 0;
  const total = S.pages.filter((p) => p.n && !p.forgotten).length, n = S.shown.length;
  $('count').textContent = S.status === 'forgotten' ? `${plural(n, 'forgotten page')}` : n === total ? `All ${plural(total, 'page')}` : `${num.format(n)} of ${plural(total, 'page')}`;
  $('empty').hidden = n > 0;
  $('empty-msg').textContent = total ? 'Nothing matches.' : 'No pages yet.';
  $('empty-clear').hidden = !total;
  $('sel-all').hidden = !host.forget || !n;
  tray();
  const ms = performance.now() - t0;
  document.documentElement.dataset.renderMs = ms.toFixed(1);
  console.info(`render ${S.rendered.length} rows in ${ms.toFixed(1)} ms`);
}
function setSeg(id, v) { S[id] = v; for (const b of $(id).querySelectorAll('button')) b.setAttribute('aria-pressed', b.dataset.v === v); }
let raf = 0;
const schedule = () => { cancelAnimationFrame(raf); raf = requestAnimationFrame(render); };

// ---- 5. Cursor, expand, select ---------------------------------------------
const rows = () => [...$(S.view === 'pages' ? 'list' : 'view-snapshot').querySelectorAll('.row')];
const rowOf = (i) => $('list').querySelector(`.row[data-i="${i}"]`);
function setCursor(el, focus = true) {
  const prev = document.querySelector('.row.cur'); if (prev && prev !== el) { prev.classList.remove('cur'); prev.tabIndex = -1; }
  if (!el) { S.cur = -1; return; }
  el.classList.add('cur'); el.tabIndex = 0; S.cur = +el.dataset.i;
  if (focus) { el.focus({ preventScroll: true }); el.scrollIntoView({ block: 'nearest' }); }
}
function move(delta) {
  const all = rows(); if (!all.length) return;
  const at = all.findIndex((r) => r.classList.contains('cur'));
  const next = Math.max(0, Math.min(all.length - 1, at < 0 ? (delta > 0 ? 0 : all.length - 1) : at + delta));
  setCursor(all[next]);
}
function toggle(i, force) {
  const el = rowOf(i); if (!el) return;
  const open = force ?? !S.exp.has(i);
  if (open) { S.exp.add(i); if (!el.querySelector('.hist')) el.insertAdjacentHTML('beforeend', histHTML(S.pages[i])); }
  else { S.exp.delete(i); el.querySelector('.hist')?.remove(); }
  el.querySelector('.more').setAttribute('aria-expanded', open);
}
function select(i, on, shift) {
  if (shift && S.anchor >= 0) {
    const order = S.rendered.map((p) => p.i), a = order.indexOf(S.anchor), b = order.indexOf(i);
    for (const j of order.slice(Math.min(a, b), Math.max(a, b) + 1)) on ? S.sel.add(j) : S.sel.delete(j);
  } else { on ? S.sel.add(i) : S.sel.delete(i); S.anchor = i; }
  for (const el of $('list').querySelectorAll('.row')) { const k = +el.dataset.i, sel = S.sel.has(k); el.classList.toggle('sel', sel); el.querySelector('.pick input').checked = sel; }
  tray();
}
function tray() {
  const t = $('tray'); t.hidden = !S.sel.size;
  $('selcount').textContent = `${num.format(S.sel.size)} selected`;
  $('forget-sel').textContent = S.status === 'forgotten' ? 'Restore' : 'Forget';
  $('tray-why').textContent = S.status === 'forgotten' ? 'Puts them back in the library.' : 'Hides them from the library. The snapshots themselves are never touched.';
}

// ---- 6. Forget / restore, with undo ---------------------------------------
async function apply(idxs, restore) {
  if (!idxs.length) return;
  if (!host.forget) { const p = S.pages[idxs[0]]; return toast(`Read-only export. In a terminal: knowmoretabs forget '${p.url}'`, 'Copy', () => navigator.clipboard.writeText(`knowmoretabs forget '${p.url}'`)); }
  for (const i of idxs) S.pages[i].forgotten = !restore;
  S.sel.clear(); S.exp.clear(); render();
  const all = rows(); if (all.length) setCursor(all[Math.min(all.length - 1, Math.max(0, S.rendered.findIndex((p) => p.i >= idxs[0])))], false);
  S.undo = { idxs, restore };
  toast(`${restore ? 'Restored' : 'Forgot'} ${plural(idxs.length, 'page')}`, 'Undo', undo);
  try { await (restore ? host.restore : host.forget)(idxs.map((i) => S.pages[i].url)); }
  catch (e) { for (const i of idxs) S.pages[i].forgotten = restore; render(); toast(`Could not reach the server (${e.message}); nothing changed.`); }
}
function undo() { if (!S.undo) return; const { idxs, restore } = S.undo; S.undo = null; apply(idxs, !restore); }
let toastTimer = 0;
function toast(msg, label, act) {
  const t = $('toast'); t.textContent = msg;
  if (label) { const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = () => { act(); t.hidden = true; }; t.append(b); }
  t.hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => { t.hidden = true; }, 9000);
}
const targets = () => (S.sel.size ? [...S.sel] : S.cur >= 0 ? [S.cur] : []);

// ---- 7. Snapshots view -----------------------------------------------------
function renderSnapshots() {
  const max = Math.max(1, ...S.snaps.map((s) => s.tabs_total));
  $('snaps-empty').hidden = S.snaps.length > 0;
  $('snaps-body').innerHTML = S.snaps.slice().reverse().map((s) => { const d = degraded(s);
    return `<tr><td><a href="#snapshot/${esc(s.id)}">${F.long.format(s.date)}</a> <span class="u">${F.time.format(s.date)} UTC</span></td>` +
    `<td class="num">${s.windows}</td><td class="bar-cell"><span class="num">${s.tabs_total}</span><span class="tabs-bar" data-w="${Math.round(100 * s.tabs_total / max)}"></span>${d ? `<span class="warn">incomplete: ${d}</span>` : ''}</td>` +
    `<td class="num">${s.fresh}</td><td class="num">${s.gone}</td></tr>`; }).join('');
  for (const b of $('snaps-body').querySelectorAll('.tabs-bar')) b.style.width = `${b.dataset.w * 0.6}%`;
}
function renderSnapshot(id, tab) {
  const s = S.snaps.find((x) => x.id === id); if (!s) return showView('snapshots');
  const wins = new Map();
  for (const t of s.tabs) { if (!wins.has(t[1])) wins.set(t[1], []); wins.get(t[1]).push(t); }
  const d = degraded(s);
  $('snap-title').textContent = F.long.format(s.date) + ', ' + F.time.format(s.date) + ' UTC';
  $('snap-meta').textContent = `${plural(s.tabs_total, 'tab')} in ${plural(wins.size, 'window')}${s.groups.length ? ', ' + plural(s.groups.length, 'group') : ''} · ${s.browser} / ${s.profile} · ${s.fresh} pages first seen here${d ? ' · incomplete: ' + d : ''}`;
  $('snap-body').innerHTML = [...wins].sort((a, b) => a[0] - b[0]).map(([w, tabs]) => `<section class="win"><h3>Window ${w}<small>${plural(tabs.length, 'tab')}</small></h3><ol>` +
    tabs.sort((a, b) => a[2] - b[2]).map(([pi, , pos, tid, pin, g]) => { const p = S.pages[pi];
      return `<li class="row${tid === +tab ? ' target' : ''}" id="t-${tid}" data-i="${pi}" tabindex="-1"><span class="pos">${pos + 1}</span>` +
        `<span class="body">${titleHTML(p)}<span class="u">${esc(p.addr)}</span></span>` +
        `<span>${grpHTML(g == null ? null : s.groups[g], false, true)}</span><span>${pin ? '<span class="chip pin">pinned</span>' : ''}</span>` +
        `<a class="more" href="#pages/${pi}" tabindex="-1" aria-label="Show in library">${p.n}×</a></li>`; }).join('') + '</ol></section>').join('');
  const target = tab && $('t-' + tab);
  if (target) { setCursor(target); target.scrollIntoView({ block: 'center' }); }
}

// ---- 8. Views and routing --------------------------------------------------
function showView(name) {
  S.view = name;
  for (const v of ['pages', 'snapshots', 'snapshot']) $('view-' + v).hidden = v !== name;
  for (const a of document.querySelectorAll('.views a')) a.toggleAttribute('aria-current', a.hash === '#' + (name === 'snapshot' ? 'snapshots' : name));
  if (name !== 'snapshot') scrollTo(0, 0);
}
function route() {
  const [view, a, b] = location.hash.slice(1).split('/');
  if (view === 'snapshot' && a) { showView('snapshot'); renderSnapshot(decodeURIComponent(a), b); }
  else if (view === 'snapshots') showView('snapshots');
  else { showView('pages'); if (a !== undefined && S.pages[a]?.n) reveal(+a); }
}
function clearFilters(status = '') { S.q = $('q').value = ''; S.domain = $('domain').value = ''; S.group = $('group').value = ''; setSeg('status', status); }
function reveal(i) {
  const p = S.pages[i];
  if (!S.shown.includes(p)) { clearFilters(p.forgotten ? 'forgotten' : ''); render(); }
  if (!rowOf(i)) { S.openAll = true; render(); }   // it was behind the fold
  toggle(i, true); setCursor(rowOf(i)); rowOf(i).scrollIntoView({ block: 'center' });
}

// ---- 9. Wiring -------------------------------------------------------------
function keys(e) {
  const t = e.target, k = e.key;
  if (k === 'Escape' && isForm(t)) { if (t.value) { t.value = ''; t.dispatchEvent(new Event('input')); } else t.blur(); return; }
  if (isForm(t)) { if ((k === 'ArrowDown' || k === 'Enter') && t.id === 'q') { e.preventDefault(); move(1); } return; }
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const inRow = t.closest && t.closest('.row');
  switch (k) {
    case '/': e.preventDefault(); $('q').focus(); $('q').select(); break;
    case 'j': case 'ArrowDown': e.preventDefault(); move(1); break;
    case 'k': case 'ArrowUp': e.preventDefault(); move(-1); break;
    case 'Home': e.preventDefault(); move(-1e9); break;
    case 'End': e.preventDefault(); move(1e9); break;
    case 'Enter': if (inRow && t === inRow) { e.preventDefault(); inRow.querySelector('.t').click(); } break;
    case ' ': case 'h': if (inRow && S.view === 'pages') { e.preventDefault(); toggle(+inRow.dataset.i); } break;
    case 'x': if (inRow && S.view === 'pages' && host.forget) select(+inRow.dataset.i, !S.sel.has(+inRow.dataset.i), e.shiftKey); break;
    case 'f': if (S.view === 'pages') apply(targets(), S.status === 'forgotten'); break;
    case 'u': undo(); break;
    case '?': $('help').showModal(); break;
    case 'Escape':
      if (!$('help').open && S.sel.size) { S.sel.clear(); select(-1, false); }
      else if (S.exp.size) { for (const i of [...S.exp]) toggle(i, false); }
      else if (S.view === 'pages') { $('reset').click(); }
      break;
  }
}
function wire() {
  document.body.dataset.mode = host.mode;
  for (const id of ['q', 'domain', 'group']) $(id).addEventListener('input', (e) => { S[id] = e.target.value; schedule(); });
  for (const id of ['status', 'sort']) $(id).addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) { setSeg(id, b.dataset.v); render(); } });
  $('reset').addEventListener('click', () => { clearFilters(); render(); });
  $('empty-clear').addEventListener('click', () => $('reset').click());
  $('list').addEventListener('click', (e) => {
    if (e.target.id === 'fold') { S.openAll = !S.openAll; render(); if (S.openAll) setCursor(rows()[FOLD]); else $('fold').focus(); return; }
    const row = e.target.closest('.row'); if (!row) return; const i = +row.dataset.i;
    if (e.target.matches('.pick input')) return select(i, e.target.checked, e.shiftKey);
    if (e.target.closest('a')) return;
    setCursor(row, false); row.focus({ preventScroll: true });
    const act = e.target.closest('[data-act]');
    if (!act) { if (!window.getSelection().toString()) toggle(i); return; }
    const p = S.pages[i];
    if (act.dataset.act === 'domain') { S.domain = $('domain').value = S.domain === p.domain ? '' : p.domain; render(); }
    if (act.dataset.act === 'group') { const t = p.grp.title; S.group = $('group').value = S.group.toLowerCase() === t.toLowerCase() ? '' : t; render(); }
    if (act.dataset.act === 'copy') navigator.clipboard.writeText(p.url).then(() => toast('URL copied'));
    if (act.dataset.act === 'forget') apply([i], false);
    if (act.dataset.act === 'restore') apply([i], true);
  });
  $('list').addEventListener('focusin', (e) => { const row = e.target.closest('.row'); if (row && !row.classList.contains('cur')) setCursor(row, false); });
  $('view-snapshot').addEventListener('focusin', (e) => { const row = e.target.closest('.row'); if (row) setCursor(row, false); });
  $('forget-sel').addEventListener('click', () => apply([...S.sel], S.status === 'forgotten'));
  $('clear-sel').addEventListener('click', () => { S.sel.clear(); select(-1, false); });
  $('sel-all').addEventListener('click', () => { for (const p of S.rendered) S.sel.add(p.i); select(-1, false); });
  $('help-close').addEventListener('click', () => $('help').close());
  document.addEventListener('keydown', keys);
  window.addEventListener('hashchange', route);
}
async function main() {
  wire();
  let lib;
  try { lib = await host.load(); } catch (e) { $('card').textContent = `Could not load the library (${e.message}).`; return; }
  derive(lib);
  // Forgotten pages are counted by their flag: export omits them from pages[],
  // serve includes them flagged, and either way this is the number on the shelf.
  const n = S.snaps.length, total = S.pages.filter((p) => p.n && !p.forgotten).length, domains = new Map();
  for (const p of S.pages) if (p.n && !p.forgotten && p.domain) domains.set(p.domain, (domains.get(p.domain) || 0) + 1);
  if (!n) {
    $('card').innerHTML = 'Nothing here yet. Run <code>knowmoretabs save</code> to capture what is open now; the library builds itself from there.';
  } else {
    const first = S.snaps[0].date, last = S.snaps.at(-1).date, sameDay = F.dayYear.format(first) === F.dayYear.format(last);
    const every = S.pages.filter((p) => p.n === n && !p.forgotten).length;
    $('card').innerHTML = `<b>${plural(total, 'page')}</b> across <b>${plural(n, 'snapshot')}</b>, ${sameDay ? 'taken ' : ''}${F.dayYear.format(first)}${sameDay ? '' : ' to ' + F.dayYear.format(last)}. ` +
      `${plural(domains.size, 'site')}${n > 1 && total ? `; ${num.format(every)} ${every === 1 ? 'page' : 'pages'} present in every snapshot` : ''}.`;
  }
  $('domains').innerHTML = [...domains].sort((a, b) => b[1] - a[1]).map(([d, k]) => `<option value="${esc(d)}">${plural(k, 'page')}</option>`).join('');
  $('group-l').hidden = !S.groups.size;
  $('groups').innerHTML = [...S.groups.values()].sort((a, b) => b.n - a.n).map((g) => `<option value="${esc(g.title)}">${plural(g.n, 'page')}</option>`).join('');
  if (host.forget) {
    $('status').insertAdjacentHTML('beforeend', '<button type="button" data-v="forgotten" aria-pressed="false">Forgotten</button>');
    $('mode-note').textContent = 'Live — served by knowmoretabs on this machine. Forgetting hides a page; snapshots are never changed.';
  } else {
    const f = S.stats.forgotten || 0;
    $('mode-note').textContent = `Offline copy, exported ${F.full.format(new Date(lib.generated_at || Date.now()))} UTC.` + (f ? ` ${f} forgotten ${f === 1 ? 'page is' : 'pages are'} not included.` : '');
  }
  renderSnapshots();
  render();
  route();
  document.documentElement.dataset.readyMs = performance.now().toFixed(0);   // parse + derive + first paint-ready
}
main();
