// knowmoretabs · Tag and Add link while the model service is not answering
// slice: library
// why:   The shell serves this page at the service's own address when the
//        service cannot answer, so Tag and Add link stay in the nav and say
//        why. Try again reloads the same address, fragment and all; nothing
//        is retried on its own.
'use strict';
{
  // The Library's theme, as app.js keeps it.
  let theme = null; try { theme = localStorage.getItem('theme'); } catch { /* storage off */ }
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
  const current = () => {
    const screen = location.hash.startsWith('#add') ? 'add' : 'tag';
    for (const s of ['tag', 'add']) document.getElementById(`nav-${s}`)?.toggleAttribute('aria-current', s === screen);
  };
  addEventListener('hashchange', current);
  current();
  document.getElementById('retry').addEventListener('click', () => location.reload());
}
