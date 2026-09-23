// Run against a disposable serve archive: agent-browser eval --stdin < web/tests/tags.js
(async () => {
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  for (let n = 0; !S.rendered.length && n < 100; n++) await new Promise(r=>setTimeout(r,50));
  check(S.rendered.length, 'library has no rendered pages');
  const original = host.tag, originalLoad = host.load, originalVocab = host.vocab, originalUndo = host.undoTags, requests = [];
  host.tag = async (urls, add, remove) => { requests.push({urls, add, remove}); return {urls: [], tags: {}, vocabulary: [...S.vocab.values()].map(name=>({name}))}; };
  try {
    closeTagger();
    const row = rowOf(S.rendered[0].i); setCursor(row);
    row.dispatchEvent(new KeyboardEvent('keydown', {key:'=', bubbles:true, cancelable:true}));
    check(T.on && document.activeElement === $('tg'), '= did not open the row editor');
    $('tg').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true, cancelable:true}));
    check(!T.on && document.activeElement === row, 'empty Enter did not close and refocus the row');
    row.dispatchEvent(new KeyboardEvent('keydown', {key:'+', bubbles:true, cancelable:true}));
    for (const name of ['KeyboardOne', 'KeyboardTwo']) {
      $('tg').value = name; $('tg').dispatchEvent(new Event('input'));
      $('tg').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true, cancelable:true}));
      await new Promise(r=>setTimeout(r,30));
      check(requests.at(-1)?.add[0] === name, `Enter did not submit ${name}`);
      check(document.activeElement === $('tg'), 'focus left editor');
    }
    check(shellQuote("https://test.invalid/a'b") === "'https://test.invalid/a'\\''b'", 'CLI hint did not quote an apostrophe');
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
    closeTagger(); clearFilters(); S.tagsAll = true; render();
    const filter = $('tb').querySelector('[data-t]');
    filter.focus(); filter.click();
    check(document.activeElement.dataset.t === filter.dataset.t, 'tag filter lost keyboard focus');
    clearFilters(); render();
    const last = [...$('tb').querySelectorAll('[data-t]')].at(-1).dataset.t;
    filterTag(last); S.tagsAll = false; fitTags();
    const picked = [...$('tb').querySelectorAll('[data-t]')].find(b => b.dataset.t === last);
    picked.focus(); picked.click();
    check(document.activeElement.id === 'tb-more', 'collapsed tag lost focus when it returned below the fold');
    clearFilters(); render(); vocabDialog(); $('vocab').showModal();
    const button = $('vocab-list').querySelector('button'); button.focus(); vocabDialog();
    check(document.activeElement.dataset.k === button.dataset.k, 'vocabulary update lost keyboard focus');
    $('vocab').close();
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
    host.load = originalLoad;
    const payload = {tags: [{url:page.url, name, add:false, remove:false}], vocabulary:{}};
    host.tag = async () => ({urls:[page.url], tags:{[page.url]:[...before, name]}, vocabulary, undo:payload});
    let restored = false;
    host.undoTags = async (received) => {
      check(received === payload, 'client did not use the exact undo payload'); restored = true;
      return {urls:[page.url], tags:{[page.url]:before}, vocabulary};
    };
    await applyTags([page.i], [name], []); await undo();
    check(restored && !page.tags.includes(name), 'client fell back to a lossy swapped request');
    return 'PASS: keyboard, names, focus, successful writes with failed refresh, retryable undo and preserved input';
  } finally { S.undo = null; $('vocab').close(); host.tag = original; host.load = originalLoad; host.vocab = originalVocab; host.undoTags = originalUndo; closeTagger(); await refetchTags(); render(); }
})()
