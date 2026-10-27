// The service worker, the only file that speaks to the Library. The toolbar button points an
// open Library tab, else a new one, at the link link.js builds. The content script on X names
// a bookmarked post; the worker saves it, keeping it while the Library is not running, and
// reports what the content script sees there. Every request is built from link.js's constants.
importScripts('link.js', 'xdom.js');

const QUEUE_MAX = 50; // saves kept while the Library is not running
const QUEUE_MS = 7 * 24 * 60 * 60 * 1000; // for at most this long
const REPORT_MS = 24 * 60 * 60 * 1000; // an unchanged health is reported at most this often
const X_PAGES = ['https://x.com/', 'https://twitter.com/'];
const VIEW_MAX = 10000; // more posts than any page shows

async function openAddLink(tab) {
  const url = addLink(tab.url, tab.title);
  const open = await chrome.tabs.query({ url: LIBRARY + '*' });
  const library = open.find((t) => t.windowId === tab.windowId) ?? open[0];
  if (!library) return chrome.tabs.create({ url });
  await chrome.tabs.update(library.id, { url, active: true });
  await chrome.windows.update(library.windowId, { focused: true });
}

chrome.action.onClicked.addListener(openAddLink);

// Keep storage operations in order without waiting for a network request. The
// network has its own order, so every click can be stored while a save is pending.
const work = { storage: Promise.resolve(), network: Promise.resolve() };
function serial(kind, step) {
  const next = work[kind].then(step);
  work[kind] = next.catch(() => {});
  return next;
}

// A message only from this extension's own content script, in a tab on X.
function fromX(sender) {
  return sender.id === chrome.runtime.id && Boolean(sender.tab) && typeof sender.url === 'string' && X_PAGES.some((page) => sender.url.startsWith(page));
}

const isCount = (n) => Number.isInteger(n) && n >= 0 && n <= VIEW_MAX;

chrome.runtime.onMessage.addListener((message, sender) => {
  if (!fromX(sender) || typeof message !== 'object' || message === null) return;
  const now = Date.now();
  if (message.kind === 'bookmark' && typeof message.post === 'string' && canonicalPost(message.post)?.post === message.post) {
    serial('storage', () => save(message.post, now));
    serial('network', () => deliver());
  } else if (message.kind === 'view' && isCount(message.posts) && isCount(message.controls) && message.controls <= message.posts) {
    const counts = { posts: message.posts, controls: message.controls };
    serial('network', () => viewed(counts, now));
  } else if (message.kind === 'intent' && typeof message.confirmed === 'boolean') {
    serial('network', () => health((h) => afterIntent(h, message.confirmed, iso(now)), now));
  }
});

const iso = (ms) => new Date(ms).toISOString();

// Whether the Library answered at all; what it answered is its own to say.
async function sent({ url, init }) {
  try {
    await fetch(url, init);
    return true;
  } catch {
    return false;
  }
}

// The saves kept while the Library was not running, oldest first, within their bounds.
function bound(queue, now) {
  return queue.filter((item) => now - item.at < QUEUE_MS).slice(-QUEUE_MAX);
}

async function queued(now) {
  const { queue } = await chrome.storage.local.get('queue');
  return bound(Array.isArray(queue) ? queue : [], now);
}

// Sends the oldest pending save, then removes it from the current queue, including
// clicks stored during the request. A stopped worker resumes with the pending saves.
async function deliver() {
  for (;;) {
    const item = await serial('storage', async () => {
      const queue = await queued(Date.now());
      await chrome.storage.local.set({ queue });
      return queue[0];
    });
    if (!item || !(await sent(bookmarkRequest(item.post)))) return;
    await serial('storage', async () => {
      const queue = await queued(Date.now());
      const index = queue.findIndex((pending) => pending.post === item.post && pending.at === item.at);
      if (index !== -1) queue.splice(index, 1);
      await chrome.storage.local.set({ queue });
    });
  }
}

async function save(post, now) {
  await chrome.storage.local.set({ queue: bound([...(await queued(now)), { post, at: now }], now) });
}

async function viewed(counts, now) {
  await health((h) => afterView(h, counts, iso(now)), now);
  await deliver();
}

// Steps the stored health; reports it when it changed, or once a day while it holds.
async function health(step, now) {
  const stored = await chrome.storage.local.get(['health', 'reported']);
  const next = step(stored.health ?? freshHealth(iso(now)));
  await chrome.storage.local.set({ health: next });
  const last = stored.reported;
  if (last && last.state === next.state && last.since === next.since && now - last.at < REPORT_MS) return;
  if (await sent(healthRequest(healthReport(next, chrome.runtime.getManifest().version)))) {
    await chrome.storage.local.set({ reported: { state: next.state, since: next.since, at: now } });
  }
}
