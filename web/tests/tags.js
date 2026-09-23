// Run against a disposable serve archive: agent-browser eval --stdin < web/tests/tags.js
(async () => {
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  for (let n = 0; !S.rendered.length && n < 100; n++) await new Promise(r=>setTimeout(r,50));
  check(S.rendered.length, 'library has no rendered pages');
  const original = host.tag, requests = [];
  host.tag = async (urls, add, remove) => { requests.push({urls, add, remove}); return {urls: [], tags: {}, vocabulary: [...S.vocab.values()].map(name=>({name}))}; };
  try {
    closeTagger(); openTagger(S.rendered[0].i);
    for (const name of ['KeyboardOne', 'KeyboardTwo']) {
      $('tg').value = name; $('tg').dispatchEvent(new Event('input'));
      $('tg').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true, cancelable:true}));
      await new Promise(r=>setTimeout(r,30));
      check(requests.at(-1)?.add[0] === name, `Enter did not submit ${name}`);
      check(document.activeElement === $('tg'), 'focus left editor');
    }
    return 'PASS: successive tags submit with Enter and retain focus';
  } finally { host.tag = original; closeTagger(); }
})()
