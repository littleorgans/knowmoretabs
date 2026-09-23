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
    check(tagName('One\tTwo') === 'One Two', 'whitespace changed the name');
    check(tagName('😀'.repeat(40)) === '😀'.repeat(40), 'Unicode name was truncated');
    check(tagName('x'.repeat(41)).length === 41, 'overlong name was silently truncated');
    check(!$('tg').hasAttribute('maxlength'), 'HTML limits code units instead of characters');
    const p = S.pages.find(p => p.i !== S.cur && !p.forgotten), saved = [...p.tags], vocab = new Map(S.vocab);
    try {
      S.vocab.set('reviewexact', 'ReviewExact'); S.vocab.set('reviewexactly', 'ReviewExactly');
      setTags(p, ['ReviewExactly']); $('tg').value = 'REVIEWEXACT';
      check(tagOptions()[0]?.value === 'ReviewExact', 'a popular prefix outranked the exact name');
    } finally { S.vocab = vocab; setTags(p, saved); }
    return 'PASS: keyboard, exact names, whitespace and Unicode';
  } finally { host.tag = original; closeTagger(); }
})()
