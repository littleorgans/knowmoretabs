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
    undoTags: (undo) => post('api/tags', { urls: [], undo }),
    vocab: (create, retire) => post('api/vocabulary', { create, retire }),
  };
})();

// ---- 2. State -------------------------------------------------------------
const S = { pages: [], snaps: [], stats: {}, groups: new Map(), shown: [], rendered: [], q: '', domain: '', group: '', status: '', sort: 'last',
            openAll: false, preview: false, cur: -1, anchor: -1, sel: new Set(), exp: new Set(), undo: null, view: 'pages',
            vocab: new Map(), defs: new Map(), tags: [], tagsAll: false };   // vocab: lower-case name → name; tags: the tag filter, lower-case, all must match
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
const shellQuote = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'";
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
// Collapse whitespace like the backend; it validates length and control characters.
const tagName = (s) => String(s).replace(/\p{White_Space}+/gu, ' ').trim();
// A page's tags in the vocabulary's spelling, plus the lower-case set the filter counts with.
function setTags(p, names) {
  const m = new Map();
  for (const t of names) { const k = lc(t); if (!S.vocab.has(k)) S.vocab.set(k, t); m.set(k, S.vocab.get(k)); }
  p.tags = [...m.values()].sort(collator.compare); p.tk = new Set(m.keys());
  if (p.sg) setSug(p);
}
// Suggestions as loaded, each name with its sources; `gone` is what was decided
// on this visit. A page shows the ones still active and undecided, several
// sources first, and filters and counts by them with its own tags (`all`).
function loadSug(p, list) {
  p.sg = new Map(); p.gone = new Set();
  for (const s of Array.isArray(list) ? list : []) if (typeof s?.name === 'string' && s.name) p.sg.set(lc(s.name), Array.isArray(s.sources) ? s.sources.filter((x) => typeof x === 'string') : []);
  setSug(p);
}
function setSug(p) {
  p.sug = [...p.sg].filter(([k]) => S.vocab.has(k) && !p.tk.has(k) && !p.gone.has(k)).map(([k, src]) => ({ k, name: S.vocab.get(k), src }))
    .sort((a, b) => (b.src.length > 1) - (a.src.length > 1) || collator.compare(a.name, b.name));
  p.all = p.sug.length ? new Set([...p.tk, ...p.sug.map((s) => s.k)]) : p.tk;
}
const and = new Intl.ListFormat(undefined, { type: 'conjunction' });
const def = (k) => (S.defs.get(k) ? `\n${S.defs.get(k)}` : '');
const sugTitle = (k, src, then = '') => `Suggested${src.length ? ` by ${and.format(src)}` : ''}${then}${def(k)}`;
const TICKS = '<i class="src" aria-hidden="true"></i>';
const isForm = (el) => el && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName) && el.type !== 'checkbox';   // a box is not typed into
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
  for (const p of S.pages) { setTags(p, Array.isArray(p.tags) ? p.tags.filter((t) => typeof t === 'string' && t) : []); loadSug(p, p.suggested); }
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
    && (S.status !== 'open' || p.open) && (S.status !== 'closed' || !p.open) && (S.status !== 'suggested' || p.sug.length)
    && (!S.preview || S.sel.has(p.i)) && S.tags.every((t) => p.all.has(t))
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
// Pages per tag, suggestions included (`own`: the owner's alone). Over the
// shown list it is co-occurrence: with Harness picked, "Skills 47" is 47
// Harness pages that are also Skills.
function tagCounts(pages, own) { const n = new Map(); for (const p of pages) for (const k of own ? p.tk : p.all) n.set(k, (n.get(k) || 0) + 1); return n; }
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
// characters, then "+N" (the history lists all). The owner's tags come
// first, then suggestions, dashed, each with a mark per source. The filter's
// own tags go last, since every row shown has them. In serve a suggestion
// opens the editor, where it is confirmed or dismissed; in export it filters.
function chipsOf(p) {
  const on = new Set(S.tags), list = [...p.tags.map((name) => ({ k: lc(name), name })), ...p.sug].sort((a, b) => on.has(a.k) - on.has(b.k));
  let k = 0, used = 0;
  while (k < list.length && k < TAGMAX && (k === 0 || used + list[k].name.length <= TAGCH)) used += list[k++].name.length;
  return { on, list, k };
}
// Every row has the line, so rows share a height and a hover moves nothing: an
// untagged row's holds only "+ tag", at its start.
function tagsHTML(p) {
  const { on, list, k } = chipsOf(p);
  const chips = list.slice(0, k).map(({ k: t, name, src }) => { const f = on.has(t) ? 'Stop filtering by' : 'Only pages tagged', rev = src && host.tag && !on.has(t);
    return `<button type="button" class="tag${src ? ` s s${Math.min(3, src.length) || 1}` : ''}${on.has(t) ? ' on' : ''}" tabindex="-1" data-act="${rev ? 'tag' : 'tagf'}" data-t="${esc(t)}" ` +
      `title="${esc(src ? sugTitle(t, src, rev ? '. Click to confirm or dismiss it' : `. ${f} ${name}`) : `${f} ${name}${def(t)}`)}">${esc(name)}${src ? TICKS : ''}</button>`; }).join('');
  const more = k < list.length ? `<span class="tag n" title="${esc(list.slice(k).map((t) => t.name + (t.src ? ' (suggested)' : '')).join(', '))}">+${list.length - k}</span>` : '';
  const add = host.tag ? `<button type="button" class="tag add" tabindex="-1" data-act="tag" aria-label="Add a tag" title="Add or remove tags (+)">${list.length ? '+' : '+ tag'}</button>` : '';
  return `<span class="tags">${chips}${more}${add}</span>`;
}
function rowHTML(p) {
  const s = S.snaps[p.last];
  return `<li class="row${p.open ? ' open' : ''}${S.sel.has(p.i) ? ' sel' : ''}${p.i === S.cur ? ' cur' : ''}${S.exp.has(p.i) ? ' exp' : ''}" data-i="${p.i}" tabindex="${p.i === S.cur ? 0 : -1}">` +
    `<span class="pick"><input type="checkbox" tabindex="-1" aria-label="Select"${S.sel.has(p.i) ? ' checked' : ''}></span>` +
    `<span class="strip" title="Seen in ${p.n} of ${S.snaps.length} snapshots"></span>` +
    `<span class="body">${titleHTML(p)}<span class="u">${esc(p.addr)}</span>${tagsHTML(p)}${grpHTML(p.grp, true)}</span>` +
    `<time class="last" datetime="${s.captured_at}" title="Last seen ${F.full.format(s.date)} UTC">${ago(s.date)}</time>` +
    `<button class="more" type="button" tabindex="-1" aria-label="History" aria-expanded="${S.exp.has(p.i)}">›</button>` +
    (S.exp.has(p.i) ? histHTML(p) : '') + '</li>';
}
// How long, in the largest unit and the one under it if it is not zero:
// "7 days 4 hours", "12 minutes", "32 seconds".
const SPAN = [[86400, 'day'], [3600, 'hour'], [60, 'minute'], [1, 'second']].map(([n, unit]) => [n, new Intl.NumberFormat(undefined, { style: 'unit', unit, unitDisplay: 'long' })]);
function span(s) {
  const parts = [];
  for (const [n, f] of SPAN) { const k = Math.floor(s / n); if (k) { parts.push(f.format(k)); s -= k * n; } else if (parts.length) break; if (parts.length === 2) break; }
  return parts.join(' ') || 'under a second';
}
const norm = (t) => String(t).toLowerCase().replace(/\s+/g, ' ').trim();
// What the browser's own History said about the page when it was last saved
// (NOTES §10). Absent for pages seen only before it was recorded, and an
// export leaves out the search and the referrer unless asked for them.
function browserHTML(p) {
  const h = p.history; if (!h || typeof h !== 'object') return '';
  const line = (dt, dd, title = '') => `<dt>${dt}</dt><dd${title ? ` title="${esc(title)}"` : ''}>${dd}</dd>`;
  const ext = (url, text) => (/^https?:\/\//i.test(url) ? `<a class="x" href="${esc(url)}" target="_blank" rel="noopener noreferrer">${text}</a>` : `<span class="x">${text}</span>`);
  const date = (t) => { const d = new Date(t); return t == null || isNaN(d) ? '' : F.dayYear.format(d); };
  let ref = h.referrer && typeof h.referrer.url === 'string' && h.referrer.url !== p.url ? h.referrer : null, out = '';
  const term = typeof h.search?.term === 'string' && h.search.term.trim() ? h.search.term : '', hops = +h.search?.hops || 0;
  if (term) {
    // One link from a results page that is also the referrer: say it once, as "on google.com".
    let on = '';
    if (hops === 1 && ref) { try { const u = new URL(ref.url); if ([...u.searchParams.values()].some((v) => norm(v) === norm(term))) { on = ` on ${ext(ref.url, esc(u.hostname.replace(/^www\./, '')))}`; ref = null; } } catch { /* not a URL */ } }
    const where = hops === 0 ? ' · this page is the results' : hops > 1 ? ` · ${hops} links away` : '';
    out += line('Found by searching', `<q>${esc(term)}</q>${on}${where}`, `${term}\nThe nearest search results page${hops ? `, ${plural(hops, 'link')} before this page` : ''}.`);
  }
  if (ref) {
    const q = S.byUrl.get(ref.url), title = ref.title || q?.title;
    const shown = ref.url.replace(/^https?:\/\/(www\.)?/i, '');
    out += line('Came from', (q?.n && title ? `<a href="#pages/${q.i}" title="Show it in the library">${esc(title)}</a> ` : '') + ext(ref.url, `<code>${esc(shown)}</code>`), ref.url);
  }
  if (h.visits) {
    const f = new Date(h.first_visit), l = new Date(h.last_visit), last = date(h.last_visit);
    const first = date(h.first_visit) && (f.getUTCFullYear() === l.getUTCFullYear() ? F.day : F.dayYear).format(f);
    const when = first && last && date(h.first_visit) !== last ? ` between ${first} and ${last}` : !last ? '' : first || h.visits === 1 ? ` on ${last}` : `, the last on ${last}`;
    const typed = !h.typed ? '' : h.typed >= h.visits ? (h.visits === 1 ? ' · typed' : ' · all typed') : ` · typed ${h.typed === 1 ? 'once' : plural(h.typed, 'time')}`;
    out += line('Visits', `${num.format(h.visits)}${when}${typed}`, 'Visits the browser still keeps, about 90 days of them. Typed means typed or picked in the address bar.');
  }
  if (Number.isFinite(h.foreground_seconds) && h.foreground_seconds >= 0) out += line('Time on page', span(h.foreground_seconds), 'Time in the foreground, over the visits the browser timed. The visit in progress is not counted.');
  return out ? `<div class="hx"><h4>Browser history</h4><dl>${out}</dl></div>` : '';
}
function histHTML(p, cls = 'hist in') {
  const sightings = p.seen.map(([k, w, tid, pos, g]) => { const s = S.snaps[k];
    return `<li><a href="#snapshot/${esc(s.id)}/${tid}">${F.full.format(s.date)}</a><span>window ${w} · tab ${pos + 1}</span>${grpHTML(g)}</li>`; }).reverse();
  const tagged = (p.tags.length ? ` · tagged ${p.tags.map(esc).join(', ')}` : '') + (p.sug.length ? ` · suggested ${p.sug.map((s) => esc(s.name)).join(', ')}` : '');
  const groups = p.gs.length ? ` · in ${p.gs.length === 1 ? 'group' : 'groups'} ${p.gs.map((t) => esc(S.groups.get(t).title)).join(', ')}` : '';
  const act = p.forgotten ? '<button type="button" data-act="restore" title="Puts it back in the library.">Restore</button>'
    : '<button type="button" data-act="forget" title="Hides it from the library. The snapshots themselves are never touched.">Forget</button>';
  const ro = `<span class="ro">Read-only export · to hide this page: <code>knowmoretabs forget ${esc(shellQuote(p.url))}</code></span>`;
  const first = F.dayYear.format(S.snaps[p.first].date), last = F.dayYear.format(S.snaps[p.last].date);
  const only = S.domain.toLowerCase() === p.domain ? 'Clear site filter' : `Filter to ${esc(p.domain)}`;
  const full = p.link ? `<a class="full" href="${esc(p.url)}" target="_blank" rel="noopener noreferrer" title="${esc(p.url)}">${esc(p.url)}</a>` : `<p class="full" title="${esc(p.url)}">${esc(p.url)}</p>`;
  return `<div class="${cls}"><div class="hist-in">${full}` +
    `<p class="sum">Seen in ${p.n} of ${plural(S.snaps.length, 'snapshot')} · ${first === last ? first : `${first} to ${last}`}${p.open ? ' · open now' : ''}${groups}${tagged}</p>` +
    `<p class="acts">${p.domain ? `<button type="button" data-act="domain">${only}</button>` : ''}<button type="button" data-act="copy">Copy URL</button>${host.forget ? act : ro}</p>` +
    browserHTML(p) + `<div class="sights"><h4>${plural(p.seen.length, 'sighting')}</h4><ol>${sightings.join('')}</ol></div></div></div>`;
}
function render() {
  const t0 = performance.now();
  // The rebuild drops the focused row: note if the key was in the list (or a going tray).
  const was = S.rendered, a = document.activeElement, held = $('list').contains(a) || (!S.sel.size && $('tray').contains(a));
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
  for (const ph of list.querySelectorAll('.ph')) { ph.style.setProperty('--rows', ph.dataset.b - ph.dataset.a); lazy.observe(ph); }
  fitRows();
  if (S.rendered.length && !list.querySelector('.row.cur')) list.querySelector('.row').tabIndex = 0;
  // back to the same row, or the nearest that stayed: after it, then before
  if (held) { const live = new Set(S.rendered.map((p) => p.i)), at = was.findIndex((p) => p.i === S.cur);
    home(live.has(S.cur) ? S.cur : [...was.slice(at + 1), ...was.slice(0, Math.max(at, 0)).reverse()].find((p) => live.has(p.i))?.i); }
  const total = S.pages.filter((p) => p.n && !p.forgotten).length, n = S.shown.length;
  // The masthead already says how many pages there are; this line speaks only when a filter narrows them.
  const filtered = !!(S.q.trim() || S.domain.trim() || S.group.trim() || S.status || S.preview || S.tags.length);
  $('count').textContent = S.preview ? `Previewing ${plural(n, 'selected page')}` : S.status === 'forgotten' ? `${plural(n, 'forgotten page')}` : S.status === 'suggested' ? `${plural(n, 'page')} with suggested tags` : !filtered ? '' : `${num.format(n)} of ${plural(total, 'page')}`;
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
  strips(ol); fitRows(ol);
}
// Offscreen rows are laid out at an estimate (content-visibility), and a row
// that renders taller than its estimate moves the page under the reader.
// Every row has one line of chips, so one rendered row gives the height of
// all of them (--h1) and a chip gives each further line (--hl). What varies
// is how many lines a row's chips wrap to: that comes from their text widths
// (measured once per name, on a canvas, in the chips' font) at the chip
// line's width, as `data-x` on a row and `--x` summed on a placeholder.
// Measured again on each render, and when the list or a row changes size.
const FIT = { w: 0, words: new Map() };
function measure() {
  const t = [...$('list').querySelectorAll('.row:not(.exp):not(.tagging) .tags')].find((x) => x.checkVisibility({ contentVisibilityAuto: true }) && x.offsetHeight < 1.5 * x.firstElementChild?.offsetHeight);
  if (!t) return;
  const c = t.firstElementChild, cs = getComputedStyle(c), ts = getComputedStyle(t), px = (v) => parseFloat(v) || 0;
  if (FIT.font !== cs.font) { FIT.font = cs.font; FIT.words.clear(); (FIT.ctx ||= document.createElement('canvas').getContext('2d')).font = cs.font; }
  const sep = $('list').querySelector('.tag:not(.s) + .tag.s');
  Object.assign(FIT, { w: t.getBoundingClientRect().width, gap: px(ts.columnGap), in: px(cs.columnGap), sep: sep ? px(getComputedStyle(sep).marginLeft) : FIT.sep || 0,
    pad: px(cs.paddingLeft) + px(cs.paddingRight) + px(cs.borderLeftWidth) + px(cs.borderRightWidth) });
  const row = t.closest('.row'), root = document.documentElement.style;
  // A further line from a row on screen that wraps (lines snap to whole pixels), else from a chip and the gap.
  const lines = (x) => new Set([...x.children].map((e) => e.offsetTop)).size;
  const t2 = [...$('list').querySelectorAll('.row[data-x]:not(.exp):not(.tagging) .tags')].find((x) => x.checkVisibility({ contentVisibilityAuto: true }) && lines(x) > 1);
  FIT.h1 = row.clientHeight; root.setProperty('--h1', `${FIT.h1}px`);
  root.setProperty('--hl', `${t2 ? (t2.offsetHeight - t.offsetHeight) / (lines(t2) - 1) : c.getBoundingClientRect().height + px(ts.rowGap)}px`);
  // The row it measured is watched: a face that finishes loading later (the serif titles) changes every row.
  FIT.ro ||= new ResizeObserver(([e]) => { if (e.target.clientHeight !== FIT.h1 && e.target.isConnected) fitRows(); });
  FIT.ro.disconnect(); FIT.ro.observe(row);
}
const textW = (s) => FIT.words.get(s) ?? FIT.words.set(s, FIT.ctx.measureText(s).width).get(s);
function extra(p) {   // the lines of chips past the first, as tagsHTML lays them out
  const { list, k } = chipsOf(p), ws = list.slice(0, k).map((t, j) => textW(t.name) + FIT.pad + (t.src ? FIT.in + 4 * (Math.min(3, t.src.length) || 1) - 2 + (j && !list[j - 1].src ? FIT.sep : 0) : 0));
  if (k < list.length) ws.push(textW(`+${list.length - k}`) + FIT.pad);
  if (host.tag) ws.push(textW(list.length ? '+' : '+ tag') + FIT.pad);
  let x = 0, n = 0;
  for (const w of ws) if (x && x + FIT.gap + w > FIT.w) { n++; x = w; } else x += (x && FIT.gap) + w;
  return Math.min(n, 8);
}
function fitRows(root = $('list')) {
  if (root === $('list')) measure();
  if (!FIT.w) return;
  for (const el of root.querySelectorAll('.band .row')) { const x = extra(S.pages[+el.dataset.i]); if (x) el.dataset.x = x; else delete el.dataset.x; }
  for (const ph of root.querySelectorAll('.ph')) ph.style.setProperty('--x', S.rendered.slice(+ph.dataset.a, +ph.dataset.b).reduce((n, p) => n + extra(p), 0));
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
    else if (k === 'Enter' && d.open) { if (own && !ctl.value.trim() && d.hi < 0) { d.hide(); return; } e.preventDefault(); e.stopPropagation(); if (d.shown[d.hi]) d.pick(d.shown[d.hi]); else d.hide(); }
    else if (k === 'Escape' && d.open) { e.preventDefault(); e.stopPropagation(); d.hide(); }
    else if ((k === ' ' || k === 'Enter') && !typeahead) { e.preventDefault(); e.stopPropagation(); d.open ? d.hide() : d.show(); }
    else if (k === 'Tab') d.hide();
  });
  if (typeahead) { ctl.addEventListener('input', () => { if (own) d.options = options(); d.show(); d.hi = own && ctl.value.trim() ? 0 : -1; d.render(); }); ctl.addEventListener('focus', d.show); ctl.addEventListener('click', d.show); }
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
// Back to the list when what held the key goes: row i (the cursor), out of
// its placeholder if need be, or the first row. It never scrolls.
function home(i = S.cur) {
  const k = S.rendered.findIndex((p) => p.i === i), ph = [...$('list').querySelectorAll('.ph')].find((x) => k >= +x.dataset.a && k < +x.dataset.b);
  if (ph) materialise(ph);
  (rowOf(i) || $('list').querySelector('.row'))?.focus({ preventScroll: true });
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
    h.inert = false; h.classList.add('in');
  } else {
    S.exp.delete(i);
    if (h) {
      if (h.contains(document.activeElement)) home(i);
      h.inert = true; h.classList.remove('in'); setTimeout(() => { if (!h.classList.contains('in')) h.remove(); }, 260);
    }
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
  if (!S.sel.size && t.contains(document.activeElement)) home();   // the tray is going, and the key with it
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
  if (!host.forget) return readOnly(`knowmoretabs forget ${shellQuote(S.pages[idxs[0]].url)}`);
  for (const i of idxs) S.pages[i].forgotten = !restore;
  S.sel.clear(); S.exp.clear(); S.preview = false; render();
  S.undo = () => apply(idxs, !restore);
  toast(`${restore ? 'Restored' : 'Forgot'} ${plural(idxs.length, 'page')}`, 'Undo', undo);
  try { await (restore ? host.restore : host.forget)(idxs.map((i) => S.pages[i].url)); }
  catch (e) { for (const i of idxs) S.pages[i].forgotten = restore; render(); toast(`Could not reach the server (${e.message}); nothing changed.`); }
}
// The last change, as the call that reverses it. It is taken while it runs, so
// a second u cannot send it twice, and put back if the server did not take it.
async function undo() { const f = S.undo; S.undo = null; if (f && (await f()) === false) S.undo ||= f; }
let toastTimer = 0;
function toast(msg, label, act) {
  const t = $('toast'); t.textContent = msg;
  if (label) { const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = () => { home(); act(); showEl(t, false); }; t.append(b); }   // the key stays in the list
  showEl(t, true); clearTimeout(toastTimer); toastTimer = setTimeout(() => showEl(t, false), 9000);
}
function readOnly(cmd) { toast(`Read-only export. In a terminal: ${cmd}`, 'Copy', () => navigator.clipboard.writeText(cmd)); }
const targets = () => (S.sel.size ? [...S.sel] : S.cur >= 0 ? [S.cur] : []);

// ---- 6b. Tags --------------------------------------------------------------
// The bar is a ruled grid, one line of it until "N more". Picked tags lead;
// every other count is over what is shown, so it is what a click will give.
function tagbar() {
  const bar = $('tagbar'); bar.hidden = !S.vocab.size && !host.tag; if (bar.hidden) return;
  const n = tagCounts(S.shown), sug = new Map();
  for (const p of S.shown) for (const s of p.sug) sug.set(s.k, (sug.get(s.k) || 0) + 1);
  const cell = (k, c) => { const name = S.vocab.get(k) || k, on = c < 0, s = sug.get(k);
    const title = `${on ? 'Stop filtering by' : plural(c, 'page') + ' tagged'} ${name}${!on && s ? `, ${s === c ? (c === 1 ? 'suggested' : 'all suggested') : `${num.format(s)} of them suggested`}` : ''}${def(k)}`;
    return `<li><button type="button" data-t="${esc(k)}" aria-pressed="${on}" title="${esc(title)}"><span>${esc(name)}</span><small>${on ? '×' : num.format(c)}</small></button></li>`; };
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
function filterTag(k) {
  const focused = $('tb').contains(document.activeElement);
  S.tags = S.tags.includes(k) ? S.tags.filter((t) => t !== k) : [...S.tags, k]; render();
  const b = focused && $('tb').querySelector(`[data-t="${CSS.escape(k)}"]`);   // keep the key on it, or on "N more" if it folded away
  if (b) (b.parentElement.hidden ? $('tb-more') : b).focus();
}

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
  if (tagIdxs().some((i) => S.pages[i].sug.length)) DD.tg.hide();   // reviewing comes first; typing or ↓ brings the menu
}
function closeTagger(refocus = true, rerender = true) {
  if (!T.on) return;
  const row = rowOf(T.on.row); T.on = null;
  if (refocus && rowOf(S.cur)) setCursor(rowOf(S.cur));   // first, so the render below finds the key in the list
  $('tg-dd').hidden = true; document.body.append($('tg-dd')); row?.classList.remove('tagging');
  if (T.dirty && rerender) render();
}
// The owner's tags with ×, then the suggestions with ✓ and ×: on the tray,
// each counted over the selected pages that carry it.
function paintTagger() {
  const idxs = tagIdxs(), pages = idxs.map((i) => S.pages[i]), n = tagCounts(pages, true), sg = new Map();
  for (const p of pages) for (const s of p.sug) { const e = sg.get(s.k) || { n: 0, src: new Set() }; e.n++; s.src.forEach((x) => e.src.add(x)); sg.set(s.k, e); }
  const on = (c) => (c < idxs.length ? `<small title="On ${c} of ${idxs.length} selected pages">${c}</small>` : '');
  const mine = [...n].sort((a, b) => collator.compare(S.vocab.get(a[0]), S.vocab.get(b[0]))).map(([k, c]) => { const name = esc(S.vocab.get(k));
    return `<span class="tag x">${name}${on(c)}<button type="button" data-rm="${esc(k)}" aria-label="Remove ${name}" title="Remove">×</button></span>`; });
  const sugs = [...sg].sort((a, b) => b[1].n - a[1].n || (b[1].src.size > 1) - (a[1].src.size > 1) || collator.compare(S.vocab.get(a[0]), S.vocab.get(b[0]))).map(([k, e]) => { const name = esc(S.vocab.get(k)), src = [...e.src].sort(collator.compare);
    return `<span class="tag x s s${Math.min(3, src.length) || 1}" title="${esc(sugTitle(k, src))}">${name}${TICKS}${on(e.n)}<button type="button" data-ok="${esc(k)}" aria-label="Confirm ${name}" title="Confirm: make it yours">✓</button><button type="button" data-no="${esc(k)}" aria-label="Dismiss ${name}" title="Dismiss: not this tag">×</button></span>`; });
  const all = T.on?.row != null && sg.size > 1 ? '<button type="button" class="tag all" data-all title="Confirm every suggestion on this page">Confirm all</button>' : '';
  // Reviewing turns up pages not worth keeping (a login screen), so forgetting is beside the tags.
  const off = S.pages[T.on?.row]?.forgotten, fg = T.on?.row == null ? '' : `<button type="button" class="tag all${off ? '' : ' fg'}" data-fg title="${off ? 'Puts it back in the library.' : 'Hides it from the library. The snapshots themselves are never touched.'}">${off ? 'Restore page' : 'Forget page'}</button>`;
  if ($('tg-chips').contains(document.activeElement)) { $('tg').focus({ preventScroll: true }); DD.tg.hide(); }   // the key stays in the editor as its chips go
  $('tg-chips').innerHTML = mine.join('') + sugs.join('') + all + fg;
}
// ✓ or × on suggestions: add or remove, on just the pages that carry them.
// From the keyboard the key goes on to the next suggestion's same button.
async function review(ks, ok, kb) {
  const idxs = tagIdxs().filter((i) => S.pages[i].sug.some((s) => ks.includes(s.k))), names = ks.map((k) => S.vocab.get(k));
  if (!idxs.length) return;
  const at = [...$('tg-chips').querySelectorAll('[data-ok]')].findIndex((b) => b.dataset.ok === ks[0]);
  await applyTags(idxs, ok ? names : [], ok ? [] : names, null, ok ? 'confirm' : 'dismiss');
  if (!kb || !T.on) return;
  const bs = $('tg-chips').querySelectorAll(ok ? '[data-ok]' : '[data-no]');
  (ks.length === 1 && bs[Math.min(at, bs.length - 1)] || $('tg')).focus({ preventScroll: true });
}
// Names that start with what is typed, then names that contain it, busiest
// first, less what every target has. A new name comes last, so ↵ on a
// fragment finds the tag you meant instead of making one.
function tagOptions() {
  const typed = tagName($('tg').value), q = lc(typed), idxs = tagIdxs(), all = libraryCounts();
  const o = [...S.vocab].filter(([k]) => k.includes(q) && !idxs.every((i) => S.pages[i].tk.has(k)))
    .map(([k, name]) => ({ value: name, label: name, n: all.get(k) || 0, pre: k.startsWith(q), meta: plural(all.get(k) || 0, 'page') }))
    .sort((a, b) => (lc(b.value) === q) - (lc(a.value) === q) || b.pre - a.pre || b.n - a.n || collator.compare(a.label, b.label));
  const back = S.retired?.get(q);                    // retired on this visit: the one retired name the page knows
  if (q && !S.vocab.has(q)) o.push({ value: back || typed, label: back || typed, meta: back ? 'retired · brings it back' : 'new tag' });
  return o;
}
function readOnlyTag(i) { const p = S.pages[i]; if (p) readOnly(`knowmoretabs tag ${shellQuote(p.url)} --add NAME`); }
// One name on some pages. Like retiring it waits for the server, which has
// the final word on spelling and on which pages changed. Its answer carries
// the decisions it replaced, and undo sends them back, exact on any selection.
// `how` names a review ('confirm', 'dismiss') or its undo ('confirm-undo'), for the toast.
async function applyTags(idxs, add, remove, reverse, how) {
  if (!host.tag) return readOnlyTag(idxs[0]);
  const before = libraryCounts();
  let r;
  try { r = await (reverse ? host.undoTags(reverse) : host.tag(idxs.map((i) => S.pages[i].url), add, remove)); }
  catch (e) { toast(`Could not update tags (${e.message}).`); return false; }
  setVocab(r.vocabulary || []);
  const idxsOf = (urls) => urls.map((u) => S.byUrl.get(u)).filter(Boolean).map((p) => p.i);
  const inv = r.undo;
  // Each decision touched is now made, or by an undo unmade, which shows a suggestion again.
  for (const d of reverse ? reverse.tags : inv?.tags || []) S.byUrl.get(d.url)?.gone[reverse && !d.add && !d.remove ? 'delete' : 'add'](lc(d.name));
  for (const [u, ts] of Object.entries(r.tags || {})) if (S.byUrl.has(u)) setTags(S.byUrl.get(u), ts);
  const changed = idxsOf(r.urls || []), tagged = idxsOf(Object.keys(r.tags || {}));
  // nothing to undo is nothing done (another tab got there first): quiet, as forget is
  if (!inv || !(inv.tags.length || Object.keys(inv.vocabulary).length)) return retagged(tagged);
  S.undo = () => applyTags(changed, remove, add, inv, how && (how.endsWith('-undo') ? how.slice(0, -5) : how + '-undo'));
  // The inverse keeps a revived name's old retirement (null for one it retired),
  // and that happened on every page with the name, so it is said as the dialog does.
  const vs = Object.entries(inv?.vocabulary || {}), back = vs.find((v) => v[1])?.[0], gone = vs.find((v) => v[1] === null)?.[0];
  const note = back && !reverse ? await refetchTags() : '';
  const names = (add.length ? add : remove).map((t) => S.vocab.get(lc(t)) || t), on = `on ${plural(new Set(inv.tags.map((d) => d.url)).size, 'page')}`;
  toast(how ? (how.endsWith('undo') ? `${and.format(names)} ${names.length > 1 ? 'are' : 'is'} suggested again ${on}` : `${how === 'confirm' ? 'Confirmed' : 'Dismissed'} ${names.join(', ')} ${on}`)
    : back ? `${back} is back${note || ` on ${plural(libraryCounts().get(lc(back)) || 0, 'page')}`}`
    : gone ? `Retired ${gone}; ${plural(before.get(lc(gone)) || 0, 'page')} no longer show it`
    : `${add.length ? 'Added' : 'Removed'} ${names.join(', ')} ${add.length ? 'to' : 'from'} ${plural(changed.length, 'page')}`, 'Undo', undo);
  retagged(tagged);
}
function setVocab(list) {
  list = list.filter((v) => typeof v?.name === 'string' && v.name);
  S.vocab = new Map(list.map((v) => [lc(v.name), v.name]));
  S.defs = new Map(list.filter((v) => typeof v.definition === 'string' && v.definition.trim()).map((v) => [lc(v.name), v.definition.trim()]));
}
// After a name comes back: '' or, if the reload fails, the toast's note that the write stood.
async function refetchTags() {
  try {
    const lib = await host.load();
    setVocab(lib.vocabulary || []);
    for (const q of lib.pages || []) if (S.byUrl.has(q.url)) { const p = S.byUrl.get(q.url); setTags(p, Array.isArray(q.tags) ? q.tags : []); loadSug(p, q.suggested); }
    return '';
  } catch (e) { return `; saved, but could not refresh the library (${e.message}). Reload to see all pages.`; }
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
  const focused = document.activeElement?.dataset.k;
  const all = libraryCounts(), rows = [...S.vocab].concat([...(S.retired || [])].filter(([k]) => !S.vocab.has(k)).map(([k, name]) => [k, name, 1]));
  $('vocab-list').innerHTML = rows.sort((a, b) => collator.compare(a[1], b[1])).map(([k, name, off]) => `<li${off ? ' class="off"' : ''}><span${S.defs.get(k) ? ` title="${esc(S.defs.get(k))}"` : ''}>${esc(name)}${S.defs.get(k) ? `<small>${esc(S.defs.get(k))}</small>` : ''}</span>` +
    `<small>${off ? 'retired' : plural(all.get(k) || 0, 'page')}</small><button type="button" data-k="${esc(k)}">${off ? 'Bring back' : 'Retire'}</button></li>`).join('');
  if (focused) $('vocab-list').querySelector(`[data-k="${CSS.escape(focused)}"]`)?.focus();
}
async function retire(k) {
  S.retired ||= new Map();
  const back = !S.vocab.has(k), name = back ? S.retired.get(k) : S.vocab.get(k), n = libraryCounts().get(k) || 0;
  let r;
  try { r = await host.vocab(back ? [name] : [], back ? [] : [name]); }
  catch (e) { toast(`Could not update tags (${e.message}).`); return false; }
  setVocab(r.vocabulary || []);
  if (!back) { S.retired.set(k, name); S.tags = S.tags.filter((t) => t !== k); for (const p of S.pages) if (p.tk.has(k)) setTags(p, p.tags.filter((t) => lc(t) !== k)); else if (p.sg.has(k)) setSug(p); }
  S.undo = () => retire(k);
  const note = back ? await refetchTags() : '';
  toast(back ? `${name} is back${note || ` on ${plural(libraryCounts().get(k) || 0, 'page')}`}` : `Retired ${name}; ${plural(n, 'page')} no longer show it`, 'Undo', undo);
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
  const t = e.target, k = e.key.length === 1 ? e.key.toLowerCase() : e.key;   // shift or caps lock sends "X" for x
  if (document.querySelector('dialog[open]')) return;   // dialogs are modal; esc closes them natively
  // On the editor's buttons: arrows move along them, esc closes it, u undoes; the rest is theirs.
  if (t.closest?.('#tg-chips')) {
    if (k === 'Escape') closeTagger(); else if (k === 'u' && !e.repeat) undo();
    else if (k === 'ArrowLeft' || k === 'ArrowRight') { e.preventDefault(); const bs = [...$('tg-chips').querySelectorAll('button')]; (bs[bs.indexOf(t) + (k === 'ArrowLeft' ? -1 : 1)] || (k === 'ArrowLeft' ? t : $('tg'))).focus(); }
    return;
  }
  if (k === 'Escape') return stepBack(t);
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
    case 'u': if (!e.repeat) undo(); break;
    case 't': THEME.flip(); break;
    case '?': $('help').showModal(); break;
  }
}
// Esc takes one step a press, the most local showing (NOTES §3): dialog, menu
// and tagger first, then a field's text, history, preview, selection, filters.
// Focus is never a step: a field with nothing left hands the key to the list.
function stepBack(t) {
  if (isForm(t) && t.value) { t.value = ''; t.dispatchEvent(new Event('input')); DD[t.id]?.hide(); }   // the input reopens a picker's menu
  else if (S.exp.size) for (const i of [...S.exp]) toggle(i, false);
  else if (S.preview) preview(false);
  else if (S.sel.size) { S.sel.clear(); select(-1, false); }
  else if (S.view === 'pages' && !$('reset').hidden) $('reset').click();
  else if (isForm(t)) home();
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
    // and the row keeps the key; a focused box would swallow the page's keys
    if (pick) { const box = pick.querySelector('input'); if (e.target !== box) box.checked = !box.checked; row.focus({ preventScroll: true }); return select(i, box.checked, e.shiftKey); }
    const link = e.target.closest('a');
    if (link) {
      // Back from a drawer's deep link returns to the row that opened it.
      if (link.getAttribute('href')?.startsWith('#') && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) history.replaceState(null, '', `#pages/${i}`);
      return;
    }
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
  new ResizeObserver(([e]) => { if (e.contentRect.width !== FIT.lw) { FIT.lw = e.contentRect.width; fitRows(); } }).observe($('list'));
  $('tag-sel').addEventListener('click', () => openTagger('sel'));
  const addTag = async (name) => { const v = $('tg').value; if ((await applyTags(tagIdxs(), [name], [])) !== false && $('tg').value === v) $('tg').value = ''; };   // a failed add keeps what was typed
  dropdown('tg', { typeahead: true, own: true, none: 'Type a name to make a tag', options: tagOptions, onPick: (o) => addTag(o.value) });
  // After the menu's keys: esc clears, then closes; ↵ on nothing closes, and with the menu shut adds what is typed.
  $('tg').addEventListener('keydown', (e) => {
    e.stopPropagation(); const v = $('tg').value, last = [...$('tg-chips').querySelectorAll('button')].at(-1);
    if (e.key === 'ArrowLeft' && !v && last) { e.preventDefault(); last.focus(); return; }   // back along the chips
    if (e.defaultPrevented || (e.key !== 'Escape' && e.key !== 'Enter')) return;
    e.preventDefault(); if (e.key === 'Enter' && v.trim()) addTag(tagName(v)); else if (v) $('tg').value = ''; else closeTagger();
  });
  $('tg-dd').addEventListener('mousedown', (e) => { if (e.target !== $('tg')) e.preventDefault(); });   // keep focus in the field
  $('tg-dd').addEventListener('click', (e) => {
    const b = e.target.closest('#tg-chips button'), kb = b === document.activeElement;   // the mouse leaves the key in the field
    if (!b) return;
    if (b.dataset.rm) applyTags(tagIdxs(), [], [S.vocab.get(b.dataset.rm)]);
    else if (b.dataset.ok || b.dataset.no) review([b.dataset.ok || b.dataset.no], !!b.dataset.ok, kb);
    else if ('all' in b.dataset) review(S.pages[T.on.row].sug.map((s) => s.k), true, kb);
    else if ('fg' in b.dataset) { const i = T.on.row; closeTagger(); apply([i], S.pages[i].forgotten); }
  });
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
  if (S.pages.some((p) => p.sg.size)) DD.status.set(DD.status.options.concat({ value: 'suggested', label: 'Has suggested tags' }));
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
