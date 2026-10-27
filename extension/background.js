// The service worker, the only file that speaks to the Library. The toolbar button points an
// open Library tab, else a new one, at the link link.js builds. The content script on X names
// a bookmarked post; the worker sends it to the Library once and reports what the content
// script sees there. A save the Library does not take is dropped and only counted, so the
// health report can say so. Every request is built from link.js's constants.
importScripts('link.js', 'xdom.js');

const REPORT_MS = 24 * 60 * 60 * 1000; // an unchanged health is reported at most this often
const DROPS_MS = 14 * 24 * 60 * 60 * 1000; // a drop this long after the last starts a new count, as doctor forgets them
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

// Discard any saves kept by the previous worker before processing new messages.
// One step at a time, so each reads the storage the last one left.
let work = chrome.storage.local.remove('queue');
function serial(step) {
  const next = work.then(step);
  work = next.catch(() => {});
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
    serial(() => deliver(message.post, now));
  } else if (message.kind === 'view' && isCount(message.posts) && isCount(message.controls) && message.controls <= message.posts) {
    const counts = { posts: message.posts, controls: message.controls };
    serial(() => health((h) => afterView(h, counts, iso(now)), now));
  } else if (message.kind === 'intent' && typeof message.confirmed === 'boolean') {
    serial(() => health((h) => afterIntent(h, message.confirmed, iso(now)), now));
  }
});

const iso = (ms) => new Date(ms).toISOString();

// The Library's reply, or null when it did not answer.
async function reply({ url, init }) {
  try {
    return await fetch(url, init);
  } catch {
    return null;
  }
}

// Sends the post once. When the Library does not answer, or answers that its archive failed,
// the save is dropped: only the count of drops and the time of the last are kept, never the
// post, and the health report carries them. Any other reply, a refusal included, ends it.
async function deliver(post, now) {
  const answer = await reply(bookmarkRequest(post));
  if (answer && answer.status < 500) return;
  const { drops } = await chrome.storage.local.get('drops');
  const counting = drops && now - Date.parse(drops.last) < DROPS_MS;
  await chrome.storage.local.set({ drops: { count: bounded(counting ? drops.count + 1 : 1), last: iso(now) } });
  await health((h) => h, now);
}

// Steps the stored health; reports it with the drops when either changed, or once a day while
// both hold.
async function health(step, now) {
  const stored = await chrome.storage.local.get(['health', 'reported', 'drops']);
  const next = step(stored.health ?? freshHealth(iso(now)));
  await chrome.storage.local.set({ health: next });
  const lastDropped = stored.drops?.last ?? null;
  const last = stored.reported;
  if (last && last.state === next.state && last.since === next.since && last.lastDropped === lastDropped && now - last.at < REPORT_MS) return;
  const answer = await reply(healthRequest(healthReport(next, chrome.runtime.getManifest().version, stored.drops)));
  if (answer?.ok) {
    await chrome.storage.local.set({ reported: { state: next.state, since: next.since, lastDropped, at: now } });
  }
}
