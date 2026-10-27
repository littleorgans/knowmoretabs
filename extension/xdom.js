// What the extension knows of X's pages, plain JS with no browser API so tests can load it:
// the one table of X's markup, which post a bookmark click is for, what the page shows of its
// author's thread, and the health rule. Nothing else in the extension names an X attribute.
const X = {
  post: 'article[data-testid="tweet"]',
  author: '[data-testid="User-Name"]',
  time: 'time[datetime]',
  text: '[data-testid="tweetText"]',
  textMore: '[data-testid="tweet-text-show-more-link"]',
  photo: '[data-testid="tweetPhoto"] img',
  quote: 'div[role="link"][tabindex="0"]',
  add: '[data-testid="bookmark"]',
  remove: '[data-testid="removeBookmark"]',
  cell: '[data-testid="cellInnerDiv"]',
  heading: '[role="heading"]',
};
const X_HOSTS = ['x.com', 'twitter.com'];
const HANDLE = '([A-Za-z0-9_]{1,15})';
const PERMALINK = new RegExp(`^/${HANDLE}/status/(\\d{1,19})(?:/(?:photo|video)/\\d+)?/?$`);
const PROFILE = new RegExp(`^/${HANDLE}/?$`);

// Health: posts on this many page views in a row (each with at least BLIND_POSTS) and no
// bookmark button is blind; this many add clicks in a row X never shows as saved is unconfirmed.
const BLIND_VIEWS = 3;
const BLIND_POSTS = 3;
const MISSED_INTENTS = 3;
const COUNT_MAX = 1000000;

// The path of `href` against `base` when it is on X, else null.
function xPath(href, base) {
  let url;
  try {
    url = new URL(href, base);
  } catch {
    return null;
  }
  return url.protocol === 'https:' && X_HOSTS.includes(url.hostname) ? url.pathname : null;
}

// The post a permalink names, kept as the Library keeps it, or null; `/i/status/<id>` names no author.
function canonicalPost(href, base) {
  const match = PERMALINK.exec(xPath(href, base) ?? '');
  if (!match || match[1].toLowerCase() === 'i') return null;
  return { handle: match[1], id: match[2], post: `https://x.com/${match[1]}/status/${match[2]}` };
}

// `node` belongs to post `article` itself: not to a post nested in it, nor to a quote card in it.
function owned(article, node) {
  const quote = node.closest(X.quote);
  return node.closest(X.post) === article && !(quote && quote !== article && article.contains(quote));
}

// The post `article` is: the permalink around its own time, when its handle is the one its
// author's profile link names, case folded; null when either is missing or they disagree.
function postIdentity(article, base) {
  if (!article) return null;
  const time = [...article.querySelectorAll(X.time)].find((node) => owned(article, node));
  const link = time && time.closest('a[href]');
  const named = link && canonicalPost(link.getAttribute('href'), base);
  if (!named) return null;
  const author = [...article.querySelectorAll(X.author)].find((node) => owned(article, node));
  const profile = author && [...author.querySelectorAll('a[href]')]
    .map((anchor) => PROFILE.exec(xPath(anchor.getAttribute('href'), base) ?? '')?.[1])
    .find(Boolean);
  return profile && profile.toLowerCase() === named.handle.toLowerCase() ? named : null;
}

// The bookmark button a click's composed path went through, or undefined.
function controlIn(path) {
  return path.find((node) => typeof node?.matches === 'function' && node.matches(`${X.add}, ${X.remove}`));
}

// What a click means, from its event's composed path: `add` with the post it is for (or
// `missing` when the post cannot be named), `remove`, or `none` for any other click.
function clickIntent(path, base) {
  const control = controlIn(path);
  if (!control) return { kind: 'none' };
  if (control.matches(X.remove)) return { kind: 'remove' };
  const named = postIdentity(control.closest(X.post), base);
  return named ? { kind: 'add', ...named } : { kind: 'add', missing: 'missing_identity' };
}

// --- The page read: what the page shows of the clicked post's author's thread, read at the
// click and never by clicking, scrolling or opening anything. Membership is a heuristic: the run
// of the author's posts around the clicked one, in display order.

const READ_POSTS = 50; // the most posts a read sends
const PER_POST = { alts: 8, media: 4, links: 10 };
const TEXT_NODE = 3;

// The first element under `article` matching `selector` that is its own, not a quote's.
const ownFirst = (article, selector) => [...article.querySelectorAll(selector)].find((node) => owned(article, node));

// Text as a reader sees it: line breaks kept, an emoji image as its description.
function shownText(node) {
  if (!node) return '';
  if (node.nodeType === TEXT_NODE) return node.textContent;
  if (node.localName === 'br') return '\n';
  if (node.localName === 'img') return node.getAttribute('alt') ?? '';
  return [...node.childNodes].map(shownText).join('');
}

const isWeb = (raw) => {
  try {
    return ['http:', 'https:'].includes(new URL(raw).protocol);
  } catch {
    return false;
  }
};

// A link in a post's text that leaves X: its address, its label and, when the page gives it in
// full, where it goes. A label cut short with `…` is never where it goes.
function linkOf(anchor, base) {
  let href;
  try {
    href = new URL(anchor.getAttribute('href') ?? '', base);
  } catch {
    return null;
  }
  if (!['http:', 'https:'].includes(href.protocol) || X_HOSTS.includes(href.hostname)) return null;
  const label = shownText(anchor).trim();
  const title = anchor.getAttribute('title') ?? '';
  const target = isWeb(title) ? title : !label.endsWith('…') && isWeb(label) ? label : null;
  return { href: href.href, label, target };
}

// The quote card a post shows, as its own post: address, handle and text; null when none or unnamed.
function quoteOf(article, base) {
  const card = [...article.querySelectorAll(X.quote)].find((node) => node.closest(X.post) === article);
  if (!card) return null;
  const time = card.querySelector(X.time);
  const named = time && time.closest('a[href]') && canonicalPost(time.closest('a[href]').getAttribute('href'), base);
  if (!named) return null;
  return { url: named.post, handle: named.handle, text: shownText(card.querySelector(X.text)) };
}

// One post as the page shows it, by `named`, its identity.
function postRead(article, named, base) {
  const author = ownFirst(article, X.author);
  const profile = author && [...author.querySelectorAll('a[href]')].find((anchor) => PROFILE.test(xPath(anchor.getAttribute('href'), base) ?? ''));
  const text = ownFirst(article, X.text);
  const photos = [...article.querySelectorAll(X.photo)].filter((img) => owned(article, img));
  const alts = photos.map((img) => (img.getAttribute('alt') ?? '').trim()).filter((alt) => alt && alt !== 'Image');
  const media = photos.map((img) => img.getAttribute('src') ?? '').filter((src) => src.startsWith('https://'));
  const links = text ? [...text.querySelectorAll('a[href]')].map((anchor) => linkOf(anchor, base)).filter(Boolean) : [];
  const read = {
    id: named.id,
    handle: named.handle,
    name: profile ? shownText(profile).trim() : '',
    at: ownFirst(article, X.time)?.getAttribute('datetime') ?? null,
    lang: text?.getAttribute('lang') || null,
    text: shownText(text),
    text_complete: !ownFirst(article, X.textMore),
    alts: alts.slice(0, PER_POST.alts),
    media: media.slice(0, PER_POST.media),
    links: links.slice(0, PER_POST.links),
  };
  const quote = quoteOf(article, base);
  if (quote) read.quote = quote;
  return read;
}

// What a cell of the conversation is to the run of `handle`'s posts: one of them, another
// author's post, or a boundary (a heading, an expander, a post the page does not name).
function cellKind(cell, handle, base) {
  if (cell.matches(X.heading) || cell.querySelector(X.heading)) return { kind: 'heading' };
  const article = cell.querySelector(X.post);
  if (!article) return { kind: 'expander' };
  const named = postIdentity(article, base);
  if (!named) return { kind: 'unnamed' };
  if (named.handle.toLowerCase() !== handle.toLowerCase()) return { kind: 'other' };
  return { kind: 'post', article, named };
}

// What the page shows of the author's thread for a bookmark click, from its composed path, on
// the page at `base`: the C3 read the Library checks, or null when the post cannot be named.
function pageRead(path, base) {
  const control = controlIn(path);
  const clicked = control && control.closest(X.post);
  const named = clicked && postIdentity(clicked, base);
  if (!named) return null;
  const onStatus = PERMALINK.test(xPath(base, base) ?? '');
  const reasons = new Set();
  let run = [{ article: clicked, named }];
  const cell = clicked.closest(X.cell);
  const cells = cell?.parentElement ? [...cell.parentElement.children] : [];
  if (!onStatus) {
    reasons.add('timeline');
  } else if (cell && cellKind(cell, named.handle, base).kind === 'heading') {
    reasons.add('recommendation_boundary');
  } else if (cells.length) {
    const at = cells.indexOf(cell);
    const before = [];
    for (let i = at - 1; i >= 0; i -= 1) {
      const seen = cellKind(cells[i], named.handle, base);
      if (seen.kind !== 'post') break;
      before.unshift(seen);
    }
    const after = [];
    let end = { kind: 'end' };
    for (let i = at + 1; i < cells.length; i += 1) {
      const seen = cellKind(cells[i], named.handle, base);
      if (seen.kind !== 'post') {
        end = seen;
        break;
      }
      after.push(seen);
    }
    if (end.kind === 'heading') reasons.add('recommendation_boundary');
    if (end.kind === 'expander') reasons.add('unexpanded_replies');
    if (end.kind === 'unnamed') reasons.add('missing_identity');
    run = [...before, run[0], ...after];
  }
  const ids = new Set();
  run = run.filter(({ named: post }) => !ids.has(post.id) && ids.add(post.id));
  const at = run.findIndex((seen) => seen.named.id === named.id);
  const start = Math.max(0, at - (READ_POSTS - 1));
  const capped = run.length > READ_POSTS;
  const posts = run.slice(start, start + READ_POSTS).map((seen) => postRead(seen.article, seen.named, base));
  if (posts.some((post) => !post.text_complete)) reasons.add('text_truncated');
  if (posts.some((post) => post.links.some((link) => !link.target))) reasons.add('unresolved_link');
  return {
    where: onStatus ? 'status' : 'timeline',
    clicked: named.id,
    author: named.handle,
    complete: onStatus && cells.length > 0 && !capped && reasons.size === 0,
    reasons: [...reasons],
    posts,
  };
}

// What a page shows: its posts, and how many hold a bookmark button.
function pageCounts(root) {
  const posts = [...root.querySelectorAll(X.post)];
  const controls = posts.filter((post) => post.querySelector(X.add) || post.querySelector(X.remove));
  return { posts: posts.length, controls: controls.length };
}

// X shows post `id` as bookmarked: one of its articles holds the remove button.
function showsBookmarked(root, id, base) {
  return [...root.querySelectorAll(X.post)].some((article) => postIdentity(article, base)?.id === id && article.querySelector(X.remove));
}

// Health starts `ok`, counting from `now`, an RFC 3339 time.
function freshHealth(now) {
  return { state: 'ok', since: now, views: 0, posts: 0, controls: 0, intents: 0, confirmed: 0, blindRun: 0, missRun: 0 };
}

const bounded = (count) => Math.min(COUNT_MAX, count);

// A changed state counts afresh from `now`; the runs that led to it carry on.
function moved(health, state, now) {
  if (health.state === state) return health;
  return { ...freshHealth(now), state, blindRun: health.blindRun, missRun: health.missRun };
}

// After a page view with `posts` posts, `controls` of them holding a bookmark button. A view
// with few posts and no button says nothing; seeing a button again ends `blind`.
function afterView(health, { posts, controls }, now) {
  const next = { ...health, views: bounded(health.views + 1), posts: bounded(health.posts + posts), controls: bounded(health.controls + controls) };
  if (controls > 0) next.blindRun = 0;
  else if (posts >= BLIND_POSTS) next.blindRun = health.blindRun + 1;
  if (next.blindRun >= BLIND_VIEWS) return moved(next, 'blind', now);
  if (health.state === 'blind' && controls > 0) return moved(next, 'ok', now);
  return next;
}

// After an add click X did, or did not, show as saved; a confirmed one ends `unconfirmed`.
function afterIntent(health, confirmed, now) {
  const next = {
    ...health,
    intents: bounded(health.intents + 1),
    confirmed: bounded(health.confirmed + (confirmed ? 1 : 0)),
    missRun: confirmed ? 0 : health.missRun + 1,
  };
  if (next.missRun >= MISSED_INTENTS) return moved(next, 'unconfirmed', now);
  if (health.state === 'unconfirmed' && confirmed) return moved(next, 'ok', now);
  return next;
}

// The body `POST /api/x/health` takes: a state, a time, counts, the extension's version, and
// how many saves the Library did not take with the time of the last, from `drops`.
function healthReport(health, version, drops) {
  const { state, since, views, posts, controls, intents, confirmed } = health;
  return { state, since, version, views, posts, controls, intents, confirmed, dropped: drops?.count ?? 0, last_dropped: drops?.last ?? null };
}
