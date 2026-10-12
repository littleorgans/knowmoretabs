// The toolbar button, the only file that speaks to Chrome: it reads the clicked tab and
// points an open Library tab, else a new one, at the link link.js builds.
importScripts('link.js');

async function openAddLink(tab) {
  const url = addLink(tab.url, tab.title);
  const open = await chrome.tabs.query({ url: LIBRARY + '*' });
  const library = open.find((t) => t.windowId === tab.windowId) ?? open[0];
  if (!library) return chrome.tabs.create({ url });
  await chrome.tabs.update(library.id, { url, active: true });
  await chrome.windows.update(library.windowId, { focused: true });
}

chrome.action.onClicked.addListener(openAddLink);
