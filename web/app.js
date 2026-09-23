// knowmoretabs · library UI
// slice: library (triage when the host is `serve`)
// why:   One renderer, two hosts. The export page embeds its data; serve
//        fetches it and turns forget/restore and tagging into live buttons.
//        Everything below the host seam is host-agnostic.
'use strict';

// ---- 1. Host seam: the only place export and serve differ -----------------
const host = (() => {
  const blob = document.getElementById('library-data');
  const text = blob ? blob.textContent.trim() : '';
  if (text) return { mode: 'export', load: async () => JSON.parse(text) };
  const json = async (r) => { if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || 'HTTP ' + r.status); return r.json(); };
  const post = (path, body) => fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }).then(json);
  return {
    mode: 'serve',
    load: () => fetch('api/library').then(json),
    forget: (urls) => post('api/forget', { urls }),
    restore: (urls) => post('api/restore', { urls }),
    tag: (urls, add, remove) => post('api/tags', { urls, add, remove }),
    vocab: (create, retire) => post('api/vocabulary', { create, retire }),
  };
})();

// ---- 2. State -------------------------------------------------------------
const S = { pages: [], snaps: [], stats: {}, groups: new Map(), shown: [], rendered: [], q: '', domain: '', group: '', status: '', sort: 'last',
            openAll: false, preview: false, cur: -1, anchor: -1, sel: new Set(), exp: new Set(), undo: null, view: 'pages',
            vocab: new Map(), tags: [], tagsAll: false };   // vocab: lower-case name → name; tags: the tag filter, lower-case, all must match
const FOLD = 10;                                   // rows of "Open now" shown before "show all"
const EAGER = 300, CHUNK = 200;                    // rows rendered up front; rows per lazy placeholder after that
const COLS = 12, COL = 4, SROWS = 4;               // the sighting strip: marks to a row, px per mark (--col), rows
const TAGCH = 80, TAGMAX = 8;                      // a row's chips stop at this many characters or chips, then "+N"
const $ = (id) => document.getElementById(id);
// ---- theme: light or dark, the OS choice until the switch is used, then remembered here.
// Runs before first paint (the script is parser-blocking at the end of body) so there is no flash.
const THEME = {
  get: () => { try { return localStorage.getItem('theme'); } catch { return null; } },
  system: () => (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'),
  current: () => document.documentElement.dataset.theme || THEME.system(),
  set(t, remember) {
    document.documentElement.dataset.theme = t;
    const b = $('theme'); if (b) { const to = t === 'dark' ? 'light' : 'dark'; b.setAttribute('aria-label', `Switch to ${to} mode`); b.title = `Switch to ${to} mode (t)`; }
    if (remember) { try { localStorage.setItem('theme', t); } catch { /* file:// without storage */ } }
  },
  flip() {
    const h = document.documentElement; h.classList.add('switching'); clearTimeout(THEME.t); THEME.t = setTimeout(() => h.classList.remove('switching'), 300);
    THEME.set(THEME.current() === 'dark' ? 'light' : 'dark', true);
  },
};
{ const saved = THEME.get(); if (saved === 'light' || saved === 'dark') THEME.set(saved, false); }
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const num = new Intl.NumberFormat();
const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
const fmt = (opts) => new Intl.DateTimeFormat(undefined, { timeZone: 'UTC', ...opts });
const F = { day: fmt({ day: 'numeric', month: 'short' }), dayYear: fmt({ day: 'numeric', month: 'short', year: 'numeric' }),
            month: fmt({ month: 'long', year: 'numeric' }), long: fmt({ weekday: 'short', day: 'numeric', month: 'long', year: 'numeric' }),
            time: fmt({ hour: '2-digit', minute: '2-digit', hour12: false }), full: fmt({ day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) };
// How old, in the largest unit that still reads true: "12m ago", "3d ago",
// "4mo ago". Intl does the wording and the locale, so the table is all it
// costs and nothing is fetched to say it.
const rel = new Intl.RelativeTimeFormat(undefined, { numeric: 'always', style: 'narrow' });
const AGO = [[60, 'minute'], [3600, 'hour'], [86400, 'day'], [604800, 'week'], [2629800, 'month'], [31557600, 'year']];
function ago(date) {
  const s = (Date.now() - date) / 1000;
  if (s < 60) return 'now';                        // also covers a clock that is behind the archive
  let i = 0; while (i + 1 < AGO.length && s >= AGO[i + 1][0]) i++;
  return rel.format(-Math.floor(s / AGO[i][0]), AGO[i][1]);
}
const lc = (t) => t.toLowerCase();
// A tag name as the contract takes it: no control characters, spaces collapsed, at most 40.
const tagName = (s) => String(s).replace(/[\u0000-\u001f\u007f]/g, '').replace(/\s+/g, ' ').trim().slice(0, 40);
// A page's tags in the vocabulary's spelling, plus the lower-case set the filter counts with.
function setTags(p, names) {
  const m = new Map();
  for (const t of names) { const k = lc(t); if (!S.vocab.has(k)) S.vocab.set(k, t); m.set(k, S.vocab.get(k)); }
  p.tags = [...m.values()].sort(collator.compare); p.tk = new Set(m.keys());
}
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
// The document comes from our own backend, but a hand-edited or half-written
// one must degrade the way the parser does: skip what cannot be read, count
// it, say so. A page keeps its index (tab rows point at it), so an unreadable
// page is one with no sightings; a tab row that is not a tuple naming a
// readable page is dropped and counted on its snapshot (§7 shows it as
// "unreadable tab rows"); a snapshot without a parseable time is skipped and
// counted in the footer.
function derive(lib) {
  S.stats = lib.stats || {};
  const P = lib.pages || [];
  S.pages = P.map((p, i) => { p = typeof p?.url === 'string' ? p : { url: '' }; return { ...p, i, seen: [], title: p.title || '', domain: p.domain || '', forgotten: !!p.forgotten,
    hay: ((p.title || '') + ' ' + p.url).toLowerCase(), addr: p.url,
    name: (p.title || p.url).replace(/^[^\p{L}\p{N}]+/u, ''), link: /^https?:\/\//i.test(p.url) }; });
  // Tags: a library from before 7a has neither key and reads as untagged. A
  // page tag the vocabulary does not list is still shown, and joins it.
  setVocab(Array.isArray(lib.vocabulary) ? lib.vocabulary : []);
  for (const p of S.pages) setTags(p, Array.isArray(p.tags) ? p.tags.filter((t) => typeof t === 'string' && t) : []);
  S.byUrl = new Map(S.pages.map((p) => [p.url, p]));
  S.skipped = 0;
  S.snaps = (lib.snapshots || []).filter((s) => { const ok = s && !isNaN(new Date(s.captured_at)); if (!ok) S.skipped++; return ok; }).map((s, k) => {
    const rows = Array.isArray(s.tabs) ? s.tabs : [], tabs = rows.filter((t) => Array.isArray(t) && t.length > 3 && typeof P[t[0]]?.url === 'string');
    return { ...s, k, date: new Date(s.captured_at), groups: (s.groups || []).map((g) => ({ ...g, title: String(g?.title ?? ''), colour: String(g?.colour ?? 'grey') })),
      tabs, tabs_total: (rows.length && +s.tabs_total) || tabs.length, windows: +s.windows || 0, bad: rows.length - tabs.length, fresh: 0, gone: 0 };
  });
  // Each sighting keeps the group object it sat in, so a page can be in "Papers" in
  // March, "Later" in June and no group today without any per-page bookkeeping.
  for (const s of S.snaps) for (const [pi, w, pos, tid, , g] of s.tabs) S.pages[pi]?.seen.push([s.k, w, tid, pos, g == null ? null : s.groups[g] || null]);
  // One mark per snapshot, COLS of them to a line, wrapping downward: the
  // strip grows in lines instead of length, so 48 snapshots fit in 48 px of
  // row. A line on its own keeps the old tall ticks; two or more shrink to
  // stacked bars. Past COLS × SROWS marks each one stands for an equal run of
  // snapshots and a mark the page only part-fills is drawn faint, so the block
  // never outgrows the row however long the archive runs.
  const latest = S.snaps.length - 1, n = S.snaps.length;
  const cells = Math.min(n, COLS * SROWS) || 1;
  const cols = Math.min(cells, COLS), lines = Math.ceil(cells / COLS);
  const bar = lines > 1 ? 4 : 8, pitch = lines > 1 ? 6 : 8;
  const cellOf = (k) => Math.floor((k * cells) / n);
  const cap = new Array(cells).fill(0);
  for (let k = 0; k < n; k++) cap[cellOf(k)]++;
  const ink = (l) => (l === 1 ? 'var(--dot)' : `color-mix(in srgb,var(--dot) ${Math.round(45 + 55 * l)}%,transparent)`);
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
    const hit = new Array(cells).fill(0);
    for (const k of ks) hit[cellOf(k)]++;
    const layers = [];                                 // one gradient per line; runs of equal fill → stops
    for (let r = 0; r < lines; r++) {
      const end = Math.min((r + 1) * cols, cells), stops = [];
      for (let i = r * cols, j; i < end; i = j) {
        const l = hit[i] / cap[i];
        for (j = i + 1; j < end && hit[j] / cap[j] === l; j++);
        if (l) { const a = (i - r * cols) * COL, b = (j - r * cols) * COL;
          stops.push(`transparent ${a}px,${ink(l)} ${a}px ${b}px,transparent ${b}px`); }
      }
      layers.push(stops.length ? `linear-gradient(90deg,${stops.join(',')})` : 'none');
    }
    p.g = layers.join(',');
  }
  // the geometry is the archive's, not the page's, so the sheet reads it once
  const root = document.documentElement.style;
  root.setProperty('--sw', `${cols * COL}px`);
  root.setProperty('--sh', `${lines * pitch - (pitch - bar)}px`);
  root.setProperty('--sbar', `${bar}px`);
  root.setProperty('--spitch', `${pitch}px`);
  root.setProperty('--gs', Array.from({ length: lines }, () => `${cols * COL}px ${bar}px`).join(','));
  root.setProperty('--gp', Array.from({ length: lines }, (_, r) => `0 ${r * pitch}px`).join(','));
}
// The parser's per-snapshot counters (§5, always emitted, zeroed when clean),
// plus the rows derive() could not read: a snapshot that lost tabs says so
// instead of quietly reporting fewer.
function degraded(s) {
  const t = s.stats || {}, parts = [];
  if (t.dropped_tabs) parts.push(plural(t.dropped_tabs, 'tab') + ' dropped');
  if (t.unknown_commands) parts.push(plural(t.unknown_commands, 'unknown record'));
  if (t.malformed_commands) parts.push(plural(t.malformed_commands, 'malformed record'));
  if (t.truncated_bytes) parts.push(plural(t.truncated_bytes, 'byte') + ' truncated');
  if (t.marker_ok === false) parts.push('no end marker');
  if (s.bad) parts.push(plural(s.bad, 'unreadable tab row'));
  if (!parts.length && t.degraded) parts.push('parse degraded');
  return parts.join(', ');
}

// ---- 4. Pages view ---------------------------------------------------------
// One predicate for the list and for the pickers, so they cannot disagree.
// `skip` names the filter a picker must ignore: the Site menu counts what the
// *other* filters leave, which is the only count that is true when you click
// it. Its own filter is skipped, or choosing a site would collapse the menu
// to the site you just chose.
function matcher(skip) {
  const words = S.q.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const wantForgotten = S.status === 'forgotten';
  const d = skip === 'domain' ? '' : S.domain.trim().toLowerCase(), exact = d && S.pages.some((p) => p.domain === d);
  const g = skip === 'group' ? '' : S.group.trim().toLowerCase(), gexact = g && S.groups.has(g);
  return (p) => p.n && p.forgotten === wantForgotten && (!d || (exact ? p.domain === d : p.domain.includes(d)))
    && (!g || p.gs.some((t) => (gexact ? t === g : t.includes(g))))
    && (S.status !== 'open' || p.open) && (S.status !== 'closed' || !p.open)
    && (!S.preview || S.sel.has(p.i)) && S.tags.every((t) => p.tk.has(t))
    && words.every((w) => p.hay.includes(w));
}
function compute() { S.shown = S.pages.filter(matcher('')).sort(SORTS[S.sort]); }
// Built when a menu opens, not on every keystroke: one pass over the pages,
// and only for the picker you actually reached for.
function facet(skip, key) {
  const ok = matcher(skip), n = new Map();
  for (const p of S.pages) if (ok(p)) for (const t of key(p)) n.set(t, (n.get(t) || 0) + 1);
  return [...n].sort((a, b) => b[1] - a[1] || collator.compare(a[0], b[0]));
}
const siteOptions = () => facet('domain', (p) => (p.domain ? [p.domain] : []))
  .map(([d, k]) => ({ value: d, label: d, meta: plural(k, 'page') }));
const groupOptions = () => facet('group', (p) => p.gs)
  .map(([t, k]) => ({ value: S.groups.get(t).title, label: S.groups.get(t).title, meta: plural(k, 'page') }));
// Pages per tag. Over the shown list it is co-occurrence: with Harness picked,
// "Skills 47" is 47 Harness pages that are also Skills.
function tagCounts(pages) { const n = new Map(); for (const p of pages) for (const k of p.tk) n.set(k, (n.get(k) || 0) + 1); return n; }
const libraryCounts = () => tagCounts(S.pages.filter((p) => p.n && !p.forgotten));
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
// Chips get a line of their own under the address: up to TAGMAX, or TAGCH
// characters, then "+N" (the history lists all). The filter's own tags go
// last, since every row shown has them.
function tagsHTML(p) {
  const on = new Set(S.tags), list = [...p.tags].sort((a, b) => on.has(lc(a)) - on.has(lc(b)));
  let k = 0, used = 0;
  while (k < list.length && k < TAGMAX && (k === 0 || used + list[k].length <= TAGCH)) used += list[k++].length;
  const chips = list.slice(0, k).map((t) => `<button type="button" class="tag${on.has(lc(t)) ? ' on' : ''}" tabindex="-1" data-act="tagf" data-t="${esc(lc(t))}" title="${on.has(lc(t)) ? 'Stop filtering by' : 'Only pages tagged'} ${esc(t)}">${esc(t)}</button>`).join('');
  const more = k < list.length ? `<span class="tag n" title="${esc(list.slice(k).join(', '))}">+${list.length - k}</span>` : '';
  const add = host.tag ? `<button type="button" class="tag add" tabindex="-1" data-act="tag" aria-label="Add a tag" title="Add or remove tags (+)">${list.length ? '+' : '+ tag'}</button>` : '';
  return `<span class="tags${list.length ? ' line' : ''}">${chips}${more}${add}</span>`;
}
function rowHTML(p) {
  const s = S.snaps[p.last];
  return `<li class="row${p.open ? ' open' : ''}${p.tags.length ? ' tall' : ''}${S.sel.has(p.i) ? ' sel' : ''}${p.i === S.cur ? ' cur' : ''}${S.exp.has(p.i) ? ' exp' : ''}" data-i="${p.i}" tabindex="${p.i === S.cur ? 0 : -1}">` +
    `<span class="pick"><input type="checkbox" tabindex="-1" aria-label="Select"${S.sel.has(p.i) ? ' checked' : ''}></span>` +
    `<span class="strip" title="Seen in ${p.n} of ${S.snaps.length} snapshots"></span>` +
    `<span class="body">${titleHTML(p)}<span class="u">${esc(p.addr)}</span>${tagsHTML(p)}${grpHTML(p.grp, true)}</span>` +
    `<time class="last" datetime="${s.captured_at}" title="Last seen ${F.full.format(s.date)} UTC">${ago(s.date)}</time>` +
    `<button class="more" type="button" tabindex="-1" aria-label="History" aria-expanded="${S.exp.has(p.i)}">›</button>` +
    (S.exp.has(p.i) ? histHTML(p) : '') + '</li>';
}
function histHTML(p, cls = 'hist in') {
  const sightings = p.seen.map(([k, w, tid, pos, g]) => { const s = S.snaps[k];
    return `<li><a href="#snapshot/${esc(s.id)}/${tid}">${F.full.format(s.date)}</a><span>window ${w} · tab ${pos + 1}</span>${grpHTML(g)}</li>`; }).reverse();
  const tagged = p.tags.length ? ` · tagged ${p.tags.map(esc).join(', ')}` : '';
  const groups = p.gs.length ? ` · in ${p.gs.length === 1 ? 'group' : 'groups'} ${p.gs.map((t) => esc(S.groups.get(t).title)).join(', ')}` : '';
  const act = p.forgotten ? '<button type="button" data-act="restore" title="Puts it back in the library.">Restore</button>'
    : '<button type="button" data-act="forget" title="Hides it from the library. The snapshots themselves are never touched.">Forget</button>';
  const ro = `<span class="ro">Read-only export · to hide this page: <code>knowmoretabs forget '${esc(p.url)}'</code></span>`;
  const first = F.dayYear.format(S.snaps[p.first].date), last = F.dayYear.format(S.snaps[p.last].date);
  const only = S.domain.toLowerCase() === p.domain ? 'Clear site filter' : `Filter to ${esc(p.domain)}`;
  const full = p.link ? `<a class="full" href="${esc(p.url)}" target="_blank" rel="noopener noreferrer" title="${esc(p.url)}">${esc(p.url)}</a>` : `<p class="full" title="${esc(p.url)}">${esc(p.url)}</p>`;
  return `<div class="${cls}"><div class="hist-in">${full}` +
    `<p class="sum">Seen in ${p.n} of ${plural(S.snaps.length, 'snapshot')} · ${first === last ? first : `${first} to ${last}`}${p.open ? ' · open now' : ''}${groups}${tagged}</p>` +
    `<p class="acts">${p.domain ? `<button type="button" data-act="domain">${only}</button>` : ''}<button type="button" data-act="copy">Copy URL</button>${host.forget ? act : ro}</p>` +
    `<div class="sights"><h4>${plural(p.seen.length, 'sighting')}</h4><ol>${sightings.join('')}</ol></div></div></div>`;
}
function render() {
  const t0 = performance.now();
  if (T.on?.row != null) closeTagger(false, false);   // the row it sits in is about to be rebuilt
  compute();
  tagbar();
  // "Open now" folds to its first rows when nothing is filtered, so the archive
  // starts on screen. Any filter, or "show all", unfolds it.
  const plain = S.sort === 'last' && S.status === '' && !S.preview && !S.q.trim() && !S.domain.trim() && !S.group.trim() && !S.tags.length;
  const fold = (n, all) => `<li class="fold"><button type="button" id="fold">${all ? `Show all ${plural(n, 'open page')}` : 'Show fewer'}</button></li>`;
  // Large archives: the first EAGER rows are real; after that each run of up
  // to CHUNK rows is a placeholder of the right height that turns into rows
  // when it scrolls near (§4, lazy bands). S.rendered still lists every row.
  let html = '', band = null, inBand = 0, tail = '', pending = 0;
  const flush = () => { if (pending) { html += `<li class="ph" data-a="${S.rendered.length - pending}" data-b="${S.rendered.length}"></li>`; pending = 0; } };
  const nOpen = plain ? S.shown.filter((x) => x.open).length : 0;
  S.rendered = [];
  for (const p of S.shown) {
    const b = bandOf(p);
    if (b !== band) { flush(); html += (band === null ? '' : tail + '</ol></section>') + `<section class="band">${b ? `<h3>${esc(b)}<small>${num.format(S.shown.filter((x) => bandOf(x) === b).length)}</small></h3>` : ''}<ol>`; band = b; inBand = 0; tail = ''; }
    if (plain && p.open && ++inBand > FOLD) { tail = fold(nOpen, !S.openAll); if (!S.openAll) continue; }
    S.rendered.push(p);
    if (S.rendered.length > EAGER) { if (++pending === CHUNK) flush(); } else html += rowHTML(p);
  }
  flush();
  const list = $('list');
  list.innerHTML = html + (band === null ? '' : tail + '</ol></section>');
  strips(list);
  for (const ph of list.querySelectorAll('.ph')) { ph.style.setProperty('--rows', ph.dataset.b - ph.dataset.a); ph.style.setProperty('--tall', S.rendered.slice(+ph.dataset.a, +ph.dataset.b).filter((p) => p.tags.length).length); lazy.observe(ph); }
  if (S.rendered.length && !list.querySelector('.row.cur')) list.querySelector('.row').tabIndex = 0;
  const total = S.pages.filter((p) => p.n && !p.forgotten).length, n = S.shown.length;
  // The masthead already says how many pages there are; this line speaks only when a filter narrows them.
  const filtered = !!(S.q.trim() || S.domain.trim() || S.group.trim() || S.status || S.preview || S.tags.length);
  $('count').textContent = S.preview ? `Previewing ${plural(n, 'selected page')}` : S.status === 'forgotten' ? `${plural(n, 'forgotten page')}` : !filtered ? '' : `${num.format(n)} of ${plural(total, 'page')}`;
  $('reset').hidden = !filtered;
  $('empty').hidden = n > 0;
  $('empty-msg').textContent = total ? 'Nothing matches.' : 'No pages yet.';
  $('empty-clear').hidden = !total;
  $('sel-all').hidden = !host.forget || !n || S.preview;
  tray();
  const ms = performance.now() - t0;
  document.documentElement.dataset.renderMs = ms.toFixed(1);
  console.info(`render ${S.rendered.length} rows in ${ms.toFixed(1)} ms`);
}
// Strip gradients go through the CSSOM because the CSP forbids style attributes.
function strips(root) { for (const el of root.querySelectorAll('.row')) { const st = el.querySelector('.strip'); if (st && !st.style.getPropertyValue('--g')) st.style.setProperty('--g', S.pages[+el.dataset.i].g); } }
const lazy = new IntersectionObserver((es) => { for (const e of es) if (e.isIntersecting) materialise(e.target); }, { rootMargin: '1200px 0px' });
function materialise(ph) {
  lazy.unobserve(ph); const ol = ph.parentElement;
  ph.outerHTML = S.rendered.slice(+ph.dataset.a, +ph.dataset.b).map(rowHTML).join('');
  strips(ol);
}
const materialiseAll = () => { for (const ph of [...$('list').querySelectorAll('.ph')]) materialise(ph); };
const DEFAULTS = { status: '', sort: 'last' };
function setSeg(id, v) { S[id] = v; const o = DD[id].options.find((x) => x.value === v); $(id).firstElementChild.textContent = o ? o.label : v; $(id).classList.toggle('set', v !== DEFAULTS[id]); }

// ---- 4b. Dropdowns ----------------------------------------------------------
// One menu for all four pickers. Site and Group are typeahead comboboxes (type
// to narrow, arrows to move, enter to pick); Show and Sort are the same menu
// under a button. The native datalist and select popups looked like three
// different products, could not scroll, and could not be styled.
const DD = {};
function dropdown(id, { typeahead = false, own = false, none = 'No matches', onPick, options }) {
  const root = $(id + '-dd'), ctl = $(id), menu = $(id + '-menu');
  const d = { options: [], shown: [], open: false, hi: -1 };
  d.render = () => {
    // What is typed narrows the list — unless it is exactly one of the options,
    // in which case it is the filter already applied and the menu is how you
    // change it: offering only the site you are already on is a dead end.
    const v = typeahead ? ctl.value.trim().toLowerCase() : '';
    const q = own || d.options.some((o) => o.value.toLowerCase() === v) ? '' : v;   // own: the options already answer what is typed
    d.shown = q ? d.options.filter((o) => o.label.toLowerCase().includes(q)) : d.options;
    menu.innerHTML = d.shown.length ? d.shown.map((o, i) => `<li role="option" id="${id}-o${i}" aria-selected="${i === d.hi}" data-i="${i}"><span>${esc(o.label)}</span>${o.meta ? `<small>${esc(o.meta)}</small>` : ''}</li>`).join('') : `<li class="none">${none}</li>`;
    ctl.setAttribute('aria-activedescendant', d.hi >= 0 ? `${id}-o${d.hi}` : '');
    menu.children[d.hi]?.scrollIntoView({ block: 'nearest' });
    menu.classList.remove('flip'); menu.classList.toggle('flip', menu.getBoundingClientRect().right > innerWidth - 12);   // keep it on screen
  };
  d.show = () => { if (d.open) return; d.open = true; if (options) d.options = options();   // the list is of this moment, not of boot
    d.hi = typeahead ? -1 : Math.max(0, d.options.findIndex((o) => o.value === S[id])); menu.hidden = false; ctl.setAttribute('aria-expanded', 'true'); d.render(); showEl(menu, true); };
  d.hide = () => { if (!d.open) return; d.open = false; d.hi = -1; showEl(menu, false); ctl.setAttribute('aria-expanded', 'false'); };
  d.pick = (o) => { d.hide(); onPick(o); };
  d.set = (options) => { d.options = options; if (d.open) d.render(); };
  ctl.addEventListener('keydown', (e) => {
    const k = e.key;
    if (k === 'ArrowDown' || k === 'ArrowUp') { e.preventDefault(); e.stopPropagation(); if (!d.open) d.show(); d.hi = Math.max(0, Math.min(d.shown.length - 1, d.hi + (k === 'ArrowDown' ? 1 : -1))); d.render(); }
    else if (k === 'Enter' && d.open) { e.preventDefault(); e.stopPropagation(); if (d.shown[d.hi]) d.pick(d.shown[d.hi]); else d.hide(); }
    else if (k === 'Escape' && d.open) { e.preventDefault(); e.stopPropagation(); d.hide(); }
    else if ((k === ' ' || k === 'Enter') && !typeahead) { e.preventDefault(); e.stopPropagation(); d.open ? d.hide() : d.show(); }
    else if (k === 'Tab') d.hide();
  });
  if (typeahead) { ctl.addEventListener('input', () => { if (own) d.options = options(); d.hi = own && ctl.value.trim() ? 0 : -1; d.show(); d.render(); }); ctl.addEventListener('focus', d.show); ctl.addEventListener('click', d.show); }
  else ctl.addEventListener('click', () => (d.open ? d.hide() : d.show()));
  ctl.addEventListener('blur', d.hide);
  menu.addEventListener('mousedown', (e) => e.preventDefault());   // keep focus in the control
  menu.addEventListener('click', (e) => { const li = e.target.closest('[data-i]'); if (li) d.pick(d.shown[+li.dataset.i]); });
  document.addEventListener('pointerdown', (e) => { if (!root.contains(e.target)) d.hide(); });
  DD[id] = d; return d;
}
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
  let all = rows(); if (!all.length) return;
  let at = all.findIndex((r) => r.classList.contains('cur'));
  if (delta > 0 && S.view === 'pages') { if (delta > 1e8) materialiseAll(); else if (at >= all.length - 1) { const ph = $('list').querySelector('.ph'); if (ph) materialise(ph); } all = rows(); at = all.findIndex((r) => r.classList.contains('cur')); }
  const next = Math.max(0, Math.min(all.length - 1, at < 0 ? (delta > 0 ? 0 : all.length - 1) : at + delta));
  setCursor(all[next]);
}
// One history open at a time: opening a row closes the other. The panel is
// inserted collapsed and given `in` a frame later so the grid row can
// transition open; on close `in` comes off and the node goes once it has
// closed (or after the transition would have ended, for reduced motion).
function toggle(i, force) {
  const el = rowOf(i); if (!el) return;
  const open = force ?? !S.exp.has(i);
  let h = el.querySelector('.hist');
  if (open) {
    for (const j of [...S.exp]) if (j !== i) toggle(j, false);
    S.exp.add(i);
    if (!h) { el.insertAdjacentHTML('beforeend', histHTML(S.pages[i], 'hist')); h = el.lastElementChild; h.getBoundingClientRect(); }
    h.classList.add('in');
  } else {
    S.exp.delete(i);
    if (h) { h.classList.remove('in'); setTimeout(() => { if (!h.classList.contains('in')) h.remove(); }, 260); }
  }
  el.querySelector('.more').setAttribute('aria-expanded', open); el.classList.toggle('exp', open);
}
function select(i, on, shift) {
  if (shift && S.anchor >= 0) {
    const order = S.rendered.map((p) => p.i), a = order.indexOf(S.anchor), b = order.indexOf(i);
    for (const j of order.slice(Math.min(a, b), Math.max(a, b) + 1)) on ? S.sel.add(j) : S.sel.delete(j);
  } else { on ? S.sel.add(i) : S.sel.delete(i); S.anchor = i; }
  for (const el of $('list').querySelectorAll('.row')) { const k = +el.dataset.i, sel = S.sel.has(k); el.classList.toggle('sel', sel); el.querySelector('.pick input').checked = sel; }
  if (!S.sel.size && T.on?.sel) closeTagger(false);
  if (S.preview) { if (!S.sel.size) S.preview = false; render(); return; }
  tray();
}
function preview(on) { S.preview = on && S.sel.size > 0; render(); }
// Show or hide with a short fade: unhide, then add `in` a frame later so the
// transition runs; on hide take `in` off and hide once it has faded.
function showEl(el, on) {
  if (on) { el.hidden = false; el.getBoundingClientRect(); el.classList.add('in'); }
  else { el.classList.remove('in'); setTimeout(() => { if (!el.classList.contains('in')) el.hidden = true; }, 200); }
}
function tray() {
  const t = $('tray'); showEl(t, !!S.sel.size);
  $('selcount').textContent = `${num.format(S.sel.size)} selected`;
  $('preview-sel').textContent = S.preview ? 'Show everything' : 'Preview selection';
  $('preview-sel').setAttribute('aria-pressed', S.preview);
  $('preview-sel').title = S.preview ? 'Back to the full list; the selection stays.' : 'Show only the selected pages, so you can check them before forgetting.';
  $('forget-sel').textContent = S.status === 'forgotten' ? 'Restore' : 'Forget';
  $('forget-sel').title = S.status === 'forgotten' ? 'Puts them back in the library.' : 'Hides them from the library. The snapshots themselves are never touched.';
  $('forget-sel').classList.toggle('forget', S.status !== 'forgotten');   // the one coloured hover; putting a page back is not it
  $('tag-sel').hidden = !host.tag;
}

// ---- 6. Forget / restore, with undo ---------------------------------------
async function apply(idxs, restore) {
  if (!idxs.length) return;
  if (!host.forget) { const p = S.pages[idxs[0]]; return toast(`Read-only export. In a terminal: knowmoretabs forget '${p.url}'`, 'Copy', () => navigator.clipboard.writeText(`knowmoretabs forget '${p.url}'`)); }
  for (const i of idxs) S.pages[i].forgotten = !restore;
  S.sel.clear(); S.exp.clear(); S.preview = false; render();
  const all = rows(); if (all.length) setCursor(all[Math.min(all.length - 1, Math.max(0, S.rendered.findIndex((p) => p.i >= idxs[0])))], false);
  S.undo = () => apply(idxs, !restore);
  toast(`${restore ? 'Restored' : 'Forgot'} ${plural(idxs.length, 'page')}`, 'Undo', undo);
  try { await (restore ? host.restore : host.forget)(idxs.map((i) => S.pages[i].url)); }
  catch (e) { for (const i of idxs) S.pages[i].forgotten = restore; render(); toast(`Could not reach the server (${e.message}); nothing changed.`); }
}
// The last change, as the call that reverses it.
function undo() { const f = S.undo; S.undo = null; if (f) f(); }
let toastTimer = 0;
function toast(msg, label, act) {
  const t = $('toast'); t.textContent = msg;
  if (label) { const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = () => { act(); showEl(t, false); }; t.append(b); }
  showEl(t, true); clearTimeout(toastTimer); toastTimer = setTimeout(() => showEl(t, false), 9000);
}
const targets = () => (S.sel.size ? [...S.sel] : S.cur >= 0 ? [S.cur] : []);

// ---- 6b. Tags --------------------------------------------------------------
// The bar is a ruled grid, one line of it until "N more". Picked tags lead;
// every other count is over what is shown, so it is what a click will give.
function tagbar() {
  const bar = $('tagbar'); bar.hidden = !S.vocab.size && !host.tag; if (bar.hidden) return;
  const n = tagCounts(S.shown), cell = (k, c) => { const name = esc(S.vocab.get(k) || k), on = c < 0;
    return `<li><button type="button" data-t="${esc(k)}" aria-pressed="${on}" title="${on ? 'Stop filtering by' : plural(c, 'page') + ' tagged'} ${name}"><span>${name}</span><small>${on ? '×' : num.format(c)}</small></button></li>`; };
  const rest = [...n].filter(([k]) => !S.tags.includes(k) && S.vocab.has(k)).sort((a, b) => b[1] - a[1] || collator.compare(S.vocab.get(a[0]), S.vocab.get(b[0])));
  $('tb').innerHTML = !S.vocab.size ? '<li class="none wide">No tags yet. Press <kbd>+</kbd> on any row to add one.</li>'
    : S.tags.map((k) => cell(k, -1)).join('') + rest.map(([k, c]) => cell(k, c)).join('') + (rest.length || S.tags.length ? '' : `<li class="none${host.tag ? '' : ' wide'}">None of these pages is tagged.</li>`) +
      `<li class="tb-end"><button type="button" id="tb-more" aria-expanded="${S.tagsAll}"></button></li>` + (host.tag ? '<li class="tb-end"><button type="button" id="tb-manage">Retire tags…</button></li>' : '');
  fitTags();
}
// Collapsed, the line holds what the grid's own columns allow, the last cell saying how many more.
function fitTags() {
  const more = $('tb-more'); if (!more || $('tagbar').hidden) return;
  const cells = [...$('tb').querySelectorAll('li:not(.tb-end)')], manage = $('tb-manage')?.parentElement;
  const cols = getComputedStyle($('tb')).gridTemplateColumns.split(' ').length, fits = cells.length + !!manage <= cols;
  cells.forEach((c, j) => { c.hidden = !fits && !S.tagsAll && j >= cols - 1; });
  more.parentElement.hidden = fits; if (manage) manage.hidden = !fits && !S.tagsAll;
  more.textContent = fits || S.tagsAll ? 'Fewer' : `${num.format(cells.length - cols + 1)} more`;
}
function filterTag(k) { S.tags = S.tags.includes(k) ? S.tags.filter((t) => t !== k) : [...S.tags, k]; render(); }

// The tagger: one editor, moved onto a row's address line or above the tray's
// actions, its field a dropdown() like Site. While it is open only chips and
// counts repaint, so the list does not move under the pointer.
const T = { on: null, dirty: false };                // on: { row: i } or { sel: true }
const tagIdxs = () => (T.on?.sel ? [...S.sel] : T.on ? [T.on.row] : []);
function openTagger(where) {
  if (!host.tag) return readOnlyTag(where);
  closeTagger(false);
  const el = $('tg-dd'), row = where !== 'sel' && rowOf(where);
  if (row) { setCursor(row, false); row.classList.add('tagging'); row.querySelector('.body').append(el); }
  else if (S.sel.size) $('tray').prepend(el); else return;
  T.on = row ? { row: where } : { sel: true }; T.dirty = false;
  el.hidden = false; $('tg').value = ''; paintTagger();
  if (row) { const short = row.getBoundingClientRect().bottom + 360 - innerHeight; if (short > 0) scrollBy(0, short); }   // room for the menu
  el.classList.toggle('up', !row || el.getBoundingClientRect().bottom + 350 > innerHeight);   // the tray, or the end of the list
  $('tg').focus({ preventScroll: true });
}
function closeTagger(refocus = true, rerender = true) {
  if (!T.on) return;
  const row = rowOf(T.on.row); T.on = null;
  $('tg-dd').hidden = true; document.body.append($('tg-dd')); row?.classList.remove('tagging');
  if (T.dirty && rerender) render();
  if (refocus && rowOf(S.cur)) setCursor(rowOf(S.cur));
}
function paintTagger() {
  const idxs = tagIdxs(), n = tagCounts(idxs.map((i) => S.pages[i]));
  $('tg-chips').innerHTML = [...n].sort((a, b) => collator.compare(S.vocab.get(a[0]), S.vocab.get(b[0]))).map(([k, c]) => { const name = esc(S.vocab.get(k));
    return `<span class="tag x">${name}${c < idxs.length ? `<small title="On ${c} of ${idxs.length} selected pages">${c}</small>` : ''}<button type="button" data-rm="${esc(k)}" aria-label="Remove ${name}" title="Remove">×</button></span>`; }).join('');
}
// Names that start with what is typed, then names that contain it, busiest
// first, less what every target has. A new name comes last, so ↵ on a
// fragment finds the tag you meant instead of making one.
function tagOptions() {
  const typed = tagName($('tg').value), q = lc(typed), idxs = tagIdxs(), all = libraryCounts();
  const o = [...S.vocab].filter(([k]) => k.includes(q) && !idxs.every((i) => S.pages[i].tk.has(k)))
    .map(([k, name]) => ({ value: name, label: name, n: all.get(k) || 0, pre: k.startsWith(q), meta: plural(all.get(k) || 0, 'page') }))
    .sort((a, b) => b.pre - a.pre || b.n - a.n || collator.compare(a.label, b.label));
  const back = S.retired?.get(q);                    // retired on this visit: the one retired name the page knows
  if (q && !S.vocab.has(q)) o.push({ value: back || typed, label: back || typed, meta: back ? 'retired · brings it back' : 'new tag' });
  return o;
}
function readOnlyTag(i) {
  const p = S.pages[i], cmd = p && `knowmoretabs tag '${p.url}' --add NAME`;
  if (p) toast(`Read-only export. In a terminal: ${cmd}`, 'Copy', () => navigator.clipboard.writeText(cmd));
}
// One name on some pages. Like retiring it waits for the server, which has
// the final word on spelling and on which pages changed; undo reverses those.
async function applyTags(idxs, add, remove) {
  if (!host.tag) return readOnlyTag(idxs[0]);
  const had = new Set(S.vocab.keys());
  let changed;
  try {
    const r = await host.tag(idxs.map((i) => S.pages[i].url), add, remove);
    if (add.some((t) => !had.has(lc(t)))) await refetchTags();   // a new name may be a retired one, back on every page that had it
    else { setVocab(r.vocabulary || []); for (const [u, ts] of Object.entries(r.tags || {})) if (S.byUrl.has(u)) setTags(S.byUrl.get(u), ts); }
    changed = (r.urls || []).map((u) => S.byUrl.get(u)?.i).filter((i) => i != null);
  } catch (e) { return toast(`Nothing changed (${e.message}).`); }
  if (changed.length) {
    S.undo = () => applyTags(changed, remove, add);
    toast(`${add.length ? 'Added' : 'Removed'} ${(add.length ? add : remove).map((t) => S.vocab.get(lc(t)) || t).join(', ')} ${add.length ? 'to' : 'from'} ${plural(changed.length, 'page')}`, 'Undo', undo);
  }
  retagged(idxs);
}
function setVocab(list) { S.vocab = new Map(list.filter((v) => typeof v?.name === 'string' && v.name).map((v) => [lc(v.name), v.name])); }
async function refetchTags() {
  const lib = await host.load();
  setVocab(lib.vocabulary || []);
  for (const q of lib.pages || []) if (S.byUrl.has(q.url)) setTags(S.byUrl.get(q.url), Array.isArray(q.tags) ? q.tags : []);
}
function retagged(idxs) {
  if (!T.on) return render();
  T.dirty = true;
  for (const i of idxs) { const t = rowOf(i)?.querySelector('.tags'); if (t) t.outerHTML = tagsHTML(S.pages[i]); }
  compute(); tagbar(); paintTagger();
}
// Retiring waits for the server (it is this machine), then hides the tag
// everywhere. The dialog keeps it for the visit with "Bring back", since the
// undo toast sits behind the modal.
function vocabDialog() {
  const all = libraryCounts(), rows = [...S.vocab].concat([...(S.retired || [])].filter(([k]) => !S.vocab.has(k)).map(([k, name]) => [k, name, 1]));
  $('vocab-list').innerHTML = rows.sort((a, b) => collator.compare(a[1], b[1])).map(([k, name, off]) => `<li${off ? ' class="off"' : ''}><span>${esc(name)}</span>` +
    `<small>${off ? 'retired' : plural(all.get(k) || 0, 'page')}</small><button type="button" data-k="${esc(k)}">${off ? 'Bring back' : 'Retire'}</button></li>`).join('');
}
async function retire(k) {
  S.retired ||= new Map();
  const back = !S.vocab.has(k), name = back ? S.retired.get(k) : S.vocab.get(k), n = libraryCounts().get(k) || 0;
  try {
    const r = await host.vocab(back ? [name] : [], back ? [] : [name]);
    if (back) await refetchTags();
    else { setVocab(r.vocabulary || []); S.retired.set(k, name); S.tags = S.tags.filter((t) => t !== k); for (const p of S.pages) if (p.tk.has(k)) setTags(p, p.tags.filter((t) => lc(t) !== k)); }
  } catch (e) { return toast(`Nothing changed (${e.message}).`); }
  S.undo = () => retire(k);
  toast(back ? `${name} is back on ${plural(libraryCounts().get(k) || 0, 'page')}` : `Retired ${name}; ${plural(n, 'page')} no longer show it`, 'Undo', undo);
  render(); if ($('vocab').open) vocabDialog();
}

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
  $('snap-meta').textContent = `${plural(s.tabs_total, 'tab')} in ${plural(s.windows || wins.size, 'window')}${s.groups.length ? ', ' + plural(s.groups.length, 'group') : ''} · ${s.browser || 'unknown browser'} / ${s.profile || 'unknown profile'} · ${s.fresh} pages first seen here${d ? ' · incomplete: ' + d : ''}`;
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
function clearFilters(status = '') { S.preview = false; S.tags = []; S.q = $('q').value = ''; S.domain = $('domain').value = ''; S.group = $('group').value = ''; setSeg('status', status); }
function reveal(i) {
  const p = S.pages[i];
  if (!S.shown.includes(p)) { clearFilters(p.forgotten ? 'forgotten' : ''); render(); }
  if (!rowOf(i)) { S.openAll = true; render(); }   // it was behind the fold
  if (!rowOf(i)) materialiseAll();                  // or inside a lazy placeholder
  toggle(i, true); setCursor(rowOf(i)); rowOf(i).scrollIntoView({ block: 'center' });
}

// ---- 9. Wiring -------------------------------------------------------------
function keys(e) {
  const t = e.target, k = e.key;
  if (document.querySelector('dialog[open]')) return;   // dialogs are modal; esc closes them natively
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
    case '+': case '=': if (S.view === 'pages') { e.preventDefault(); if (S.sel.size) openTagger('sel'); else if (rowOf(S.cur)) openTagger(S.cur); } break;
    case 'u': undo(); break;
    case 't': THEME.flip(); break;
    case '?': $('help').showModal(); break;
    case 'Escape':
      if (!$('help').open && S.preview) { preview(false); }
      else if (!$('help').open && S.sel.size) { S.sel.clear(); select(-1, false); }
      else if (S.exp.size) { for (const i of [...S.exp]) toggle(i, false); }
      else if (S.view === 'pages') { $('reset').click(); }
      break;
  }
}
function wire() {
  document.body.dataset.mode = host.mode;
  for (const id of ['q', 'domain', 'group']) $(id).addEventListener('input', (e) => { S[id] = e.target.value; schedule(); });
  dropdown('domain', { typeahead: true, options: siteOptions, onPick: (o) => { S.domain = $('domain').value = o.value; render(); } });
  dropdown('group', { typeahead: true, options: groupOptions, onPick: (o) => { S.group = $('group').value = o.value; render(); } });
  for (const id of ['status', 'sort']) dropdown(id, { onPick: (o) => { setSeg(id, o.value); render(); } });
  DD.status.set([{ value: '', label: 'Everything' }, { value: 'open', label: 'Open now' }, { value: 'closed', label: 'Closed' }]);
  DD.sort.set([{ value: 'last', label: 'Last seen' }, { value: 'first', label: 'First seen' }, { value: 'count', label: 'Times seen' }, { value: 'title', label: 'Title' }, { value: 'url', label: 'URL' }]);
  $('help-btn').addEventListener('click', () => $('help').showModal());
  THEME.set(THEME.current(), false);
  $('theme').addEventListener('click', THEME.flip);
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => { if (!THEME.get()) THEME.set(THEME.system(), false); });
  // Motion is the page's own setting, not the OS's: on by default, remembered here.
  const motion = (on) => { document.documentElement.dataset.motion = on ? 'on' : 'off'; $('motion').checked = on; };
  let saved = null; try { saved = localStorage.getItem('motion'); } catch { /* file:// without storage */ }
  motion(saved !== 'off');
  $('motion').addEventListener('change', (e) => { motion(e.target.checked); try { localStorage.setItem('motion', e.target.checked ? 'on' : 'off'); } catch { /* ignore */ } });
  $('reset').addEventListener('click', () => { clearFilters(); render(); });
  $('empty-clear').addEventListener('click', () => $('reset').click());
  let down = null;
  $('list').addEventListener('pointerdown', (e) => { down = [e.clientX, e.clientY]; });
  $('list').addEventListener('click', (e) => {
    const dragged = down && Math.hypot(e.clientX - down[0], e.clientY - down[1]) > 4; down = null;
    if (e.target.id === 'fold') { S.openAll = !S.openAll; render(); if (S.openAll) setCursor(rows()[FOLD]); else $('fold').focus(); return; }
    if (e.target.closest('.tagger')) return;          // the editor handles its own clicks
    const row = e.target.closest('.row'); if (!row) return; const i = +row.dataset.i;
    // the index cell is the checkbox once it shows, so the whole cell picks
    const pick = e.target.closest('.pick');
    if (pick) { const box = pick.querySelector('input'); if (e.target !== box) box.checked = !box.checked; return select(i, box.checked, e.shiftKey); }
    if (e.target.closest('a')) return;
    setCursor(row, false); row.focus({ preventScroll: true });
    const act = e.target.closest('[data-act]');
    if (!act) { if (!dragged && !e.target.closest('.hist')) toggle(i); return; }   // the panel itself is not a toggle
    const p = S.pages[i];
    if (act.dataset.act === 'domain') { S.domain = $('domain').value = S.domain === p.domain ? '' : p.domain; render(); }
    if (act.dataset.act === 'group') { const t = p.grp.title; S.group = $('group').value = S.group.toLowerCase() === t.toLowerCase() ? '' : t; render(); }
    if (act.dataset.act === 'tagf') filterTag(act.dataset.t);
    if (act.dataset.act === 'tag') openTagger(i);
    if (act.dataset.act === 'copy') navigator.clipboard.writeText(p.url).then(() => toast('URL copied'));
    if (act.dataset.act === 'forget') apply([i], false);
    if (act.dataset.act === 'restore') apply([i], true);
  });
  $('list').addEventListener('focusin', (e) => { const row = e.target.closest('.row'); if (row && !row.classList.contains('cur')) setCursor(row, false); });
  $('view-snapshot').addEventListener('focusin', (e) => { const row = e.target.closest('.row'); if (row) setCursor(row, false); });
  $('forget-sel').addEventListener('click', () => apply([...S.sel], S.status === 'forgotten'));
  $('clear-sel').addEventListener('click', () => { S.sel.clear(); select(-1, false); });
  $('preview-sel').addEventListener('click', () => preview(!S.preview));
  $('sel-all').addEventListener('click', () => { for (const p of S.rendered) S.sel.add(p.i); select(-1, false); });
  $('help-close').addEventListener('click', () => $('help').close());
  $('tb').addEventListener('click', (e) => { const b = e.target.closest('button'); if (!b) return;
    if (b.id === 'tb-more') { S.tagsAll = !S.tagsAll; b.setAttribute('aria-expanded', S.tagsAll); fitTags(); }
    else if (b.id === 'tb-manage') { vocabDialog(); $('vocab').showModal(); }
    else filterTag(b.dataset.t); });
  new ResizeObserver(fitTags).observe($('tagbar'));
  $('tag-sel').addEventListener('click', () => openTagger('sel'));
  dropdown('tg', { typeahead: true, own: true, none: 'Type a name to make a tag', options: tagOptions, onPick: (o) => { $('tg').value = ''; applyTags(tagIdxs(), [o.value], []); } });
  $('tg').addEventListener('keydown', (e) => {         // after the menu's keys: esc clears, then closes; ↵ on nothing closes
    e.stopPropagation(); const v = $('tg').value;
    if (e.defaultPrevented || !(e.key === 'Escape' || (e.key === 'Enter' && !v.trim()))) return;
    e.preventDefault(); if (v) $('tg').value = ''; else closeTagger();
  });
  $('tg-dd').addEventListener('mousedown', (e) => { if (e.target !== $('tg')) e.preventDefault(); });   // keep focus in the field
  $('tg-dd').addEventListener('click', (e) => { const rm = e.target.closest('[data-rm]'); if (rm) applyTags(tagIdxs(), [], [S.vocab.get(rm.dataset.rm)]); });
  $('tg-dd').addEventListener('focusout', (e) => { if (!$('tg-dd').contains(e.relatedTarget)) closeTagger(false); });
  $('vocab-list').addEventListener('click', (e) => { const b = e.target.closest('button[data-k]'); if (b) retire(b.dataset.k); });
  $('vocab-close').addEventListener('click', () => $('vocab').close());
  // the toast sits above the tray, however tall the tray grows
  new ResizeObserver(() => document.documentElement.style.setProperty('--tray-h', `${$('tray').offsetHeight}px`)).observe($('tray'));
  document.addEventListener('keydown', keys);
  window.addEventListener('hashchange', route);
}
async function main() {
  wire();
  let lib;
  try { lib = await host.load(); derive(lib); } catch (e) { $('card').textContent = `Could not load the library (${e.message}).`; return; }
  // Forgotten pages are counted by their flag: export omits them from pages[],
  // serve includes them flagged, and either way this is the number on the shelf.
  const n = S.snaps.length, total = S.pages.filter((p) => p.n && !p.forgotten).length, domains = new Map();
  for (const p of S.pages) if (p.n && !p.forgotten && p.domain) domains.set(p.domain, (domains.get(p.domain) || 0) + 1);
  if (!n) {
    $('card').innerHTML = 'Nothing here yet. Run <code>knowmoretabs save</code> to capture what is open now; the library builds itself from there.';
  } else {
    const first = S.snaps[0].date, last = S.snaps.at(-1).date, sameDay = F.dayYear.format(first) === F.dayYear.format(last);
    const sameYear = first.getUTCFullYear() === last.getUTCFullYear();
    const span = sameDay ? F.dayYear.format(first) : `${(sameYear ? F.day : F.dayYear).format(first)} to ${F.dayYear.format(last)}`;
    $('card').innerHTML = `<b>${plural(total, 'page')}</b> · ${plural(domains.size, 'site')} · ${plural(n, 'snapshot')} · ${span}`;
  }
  if (host.forget) {
    DD.status.set(DD.status.options.concat({ value: 'forgotten', label: 'Forgotten' }));
    $('mode-note').textContent = 'Live — served by knowmoretabs on this machine. Forgetting hides a page; snapshots are never changed.';
  } else {
    const f = S.stats.forgotten || 0;
    $('mode-note').textContent = `Offline copy, exported ${F.full.format(new Date(lib.generated_at || Date.now()))} UTC.` + (f ? ` ${f} forgotten ${f === 1 ? 'page is' : 'pages are'} not included.` : '');
  }
  if (S.skipped) $('mode-note').textContent += ` ${plural(S.skipped, 'snapshot')} in the data could not be read.`;
  renderSnapshots();
  render();
  route();
  document.documentElement.dataset.readyMs = performance.now().toFixed(0);   // parse + derive + first paint-ready
}
main();
