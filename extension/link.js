// The link the toolbar button opens, plain JS with no browser API so tests can load it.
// The Library hands `#add=<link>&title=<title>` to Add link whole (web/app.js handOff),
// the form the bookmarklet in lab/tagger/README.md opens too.
const PORT = 41878; // the Library's port under `just up`
const LIBRARY = `http://127.0.0.1:${PORT}/`;

// Add link with the tab's address and title filled in; a page that is not on the web opens it empty.
function addLink(url, title) {
  if (!/^https?:/i.test(url || '')) return LIBRARY + '#add';
  return LIBRARY + '#add=' + encodeURIComponent(url) + (title ? '&title=' + encodeURIComponent(title) : '');
}
