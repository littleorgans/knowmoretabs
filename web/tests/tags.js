// Run against a disposable serve archive: agent-browser eval --stdin < web/tests/tags.js
(async () => {
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  for (let n = 0; !S.rendered.length && n < 100; n++) await new Promise(r=>setTimeout(r,50));
  check(S.rendered.length, 'library has no rendered pages');
  const original = host.tag, originalLoad = host.load, originalVocab = host.vocab, requests = [];
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
    closeTagger();
    const page = S.pages[S.rendered[0].i], before = [...page.tags];
    const name = 'ReviewRefreshRegression', vocabulary = [...S.vocab.values(), name].map(name => ({name}));
    host.tag = async () => ({urls:[page.url], tags:{[page.url]:[...before, name]}, vocabulary});
    host.load = async () => { throw new Error('refresh failed'); };
    S.undo = null;
    await applyTags([page.i], [name], []);
    check(page.tags.includes(name), 'successful write was discarded after refresh failure');
    check(S.undo && $('toast').textContent.includes('saved, but'), 'refresh failure lost undo or claimed nothing changed');
    const retry = S.undo;
    host.tag = async () => { throw new Error('write failed'); };
    await undo();
    check(S.undo === retry, 'failed undo cannot be retried');
    openTagger(page.i); $('tg').value = 'Keep my input';
    $('tg').dispatchEvent(new Event('input'));
    $('tg').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true, cancelable:true}));
    await new Promise(r=>setTimeout(r,30));
    check($('tg').value === 'Keep my input', 'failed write erased the input');
    closeTagger();
    host.vocab = async () => ({vocabulary: vocabulary.filter(v => v.name !== name)});
    await retire(lc(name));
    host.vocab = async () => ({vocabulary});
    await retire(lc(name));
    check(S.vocab.has(lc(name)) && S.undo && $('toast').textContent.includes('saved, but'), 'revival refresh failure lost the successful write');
    setTags(page, before);
    return 'PASS: keyboard, names, successful writes with failed refresh, retryable undo and preserved input';
  } finally { host.tag = original; host.load = originalLoad; host.vocab = originalVocab; closeTagger(); await refetchTags(); render(); }
})()
