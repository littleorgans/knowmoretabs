// The content script on x.com and twitter.com. It hears X's own bookmark clicks before X
// handles them and tells the service worker which post, with what the page shows of its
// author's thread; it counts what each page shows so the worker can report health. It never
// fetches, clicks, scrolls or opens anything.
const CONFIRM_MS = 5000; // how long X may take to show a bookmark as saved
const POLL_MS = 250;
const SETTLE_MS = 2000; // a page view is counted once the page stops changing this long
const SETTLE_MAX_MS = 10000; // or this long after it first changed, on a page that never stops

// A worker that is not listening, or an extension reloaded under the page, costs nothing here.
function tell(message) {
  chrome.runtime.sendMessage(message).catch(() => {});
}

// Whether X shows post `id` as saved within CONFIRM_MS: health only, the save is already sent.
function watch(id) {
  const started = Date.now();
  const tick = () => {
    if (showsBookmarked(document, id, location.href)) return tell({ kind: 'intent', confirmed: true });
    if (Date.now() - started >= CONFIRM_MS) return tell({ kind: 'intent', confirmed: false });
    setTimeout(tick, POLL_MS);
  };
  setTimeout(tick, POLL_MS);
}

document.addEventListener(
  'click',
  (event) => {
    if (!event.isTrusted) return;
    const path = event.composedPath();
    const intent = clickIntent(path, location.href);
    if (intent.kind !== 'add') return;
    if (intent.missing) return tell({ kind: 'intent', confirmed: false });
    // Read now, before X draws the page again; the Library checks it and asks the public
    // service for what it lacks.
    tell({ kind: 'bookmark', post: intent.post, read: pageRead(path, location.href) });
    watch(intent.id);
  },
  true,
);

let counted = null; // the address whose page view was counted
let pending = 0;
let changing = 0;

function count() {
  pending = 0;
  if (location.href === counted) return;
  const counts = pageCounts(document);
  // The app shell can settle before X has loaded its posts. Count the view
  // when the feed arrives, so a slow load does not hide missing controls.
  if (!counts.posts) return;
  counted = location.href;
  tell({ kind: 'view', ...counts });
}

new MutationObserver(() => {
  if (location.href === counted) return;
  if (!pending) changing = Date.now();
  clearTimeout(pending);
  pending = setTimeout(count, Date.now() - changing >= SETTLE_MAX_MS ? 0 : SETTLE_MS);
}).observe(document.documentElement, { childList: true, subtree: true });
