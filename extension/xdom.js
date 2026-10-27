// What the extension knows of X's pages, plain JS with no browser API so tests can load it:
// the one table of X's markup, which post a bookmark click is for, and the health rule.
// Nothing else in the extension names an X attribute.
const X = {
  post: 'article[data-testid="tweet"]',
  author: '[data-testid="User-Name"]',
  time: 'time[datetime]',
  quote: 'div[role="link"][tabindex="0"]',
  add: '[data-testid="bookmark"]',
  remove: '[data-testid="removeBookmark"]',
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

// What a click means, from its event's composed path: `add` with the post it is for (or
// `missing` when the post cannot be named), `remove`, or `none` for any other click.
function clickIntent(path, base) {
  const control = path.find((node) => typeof node?.matches === 'function' && node.matches(`${X.add}, ${X.remove}`));
  if (!control) return { kind: 'none' };
  if (control.matches(X.remove)) return { kind: 'remove' };
  const named = postIdentity(control.closest(X.post), base);
  return named ? { kind: 'add', ...named } : { kind: 'add', missing: 'missing_identity' };
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

// The body `POST /api/x/health` takes: a state, a time, counts and the extension's version.
function healthReport(health, version) {
  const { state, since, views, posts, controls, intents, confirmed } = health;
  return { state, since, version, views, posts, controls, intents, confirmed };
}
