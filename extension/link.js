// The link the toolbar button opens and the requests the service worker sends the Library,
// plain JS with no browser API so tests can load it.
// The Library hands `#add=<link>&title=<title>` to Add link whole (web/app.js handOff),
// the form the bookmarklet in lab/tagger/README.md opens too.
const PORT = 41878; // the Library's port under `just up`
const LIBRARY = `http://127.0.0.1:${PORT}/`;
const BOOKMARK_API = LIBRARY + 'api/x/bookmark';
const HEALTH_API = LIBRARY + 'api/x/health';

// Add link with the tab's address and title filled in; a page that is not on the web opens it empty.
function addLink(url, title) {
  if (!/^https?:/i.test(url || '')) return LIBRARY + '#add';
  return LIBRARY + '#add=' + encodeURIComponent(url) + (title ? '&title=' + encodeURIComponent(title) : '');
}

// A JSON POST to one of the Library's fixed routes, as fetch takes it.
function jsonPost(url, body) {
  return { url, init: { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) } };
}

// Saves the post a content script named; the post's address is all it carries.
function bookmarkRequest(post) {
  return jsonPost(BOOKMARK_API, { post });
}

// The extension's report on what it sees on X.
function healthRequest(report) {
  return jsonPost(HEALTH_API, report);
}
