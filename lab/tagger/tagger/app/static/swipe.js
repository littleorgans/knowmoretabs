/* knowmoretabs · tagger app · the gesture (from the swipe review mockup)
   A horizontal drag moves and tilts the card
   and reports which way it leans; letting go past the threshold (or flicking)
   commits, short of it the card settles back. A press that never travelled
   stays a click, so the rows inside the card still toggle. Vertical drags are
   left to the page, which keeps scrolling on touch. */
(function () {
  const K = window.KMT;
  const START = 8;          // px before a press becomes a drag
  const FLICK = 0.6;        // px per ms over the last 100 ms that commits whatever the distance

  const animated = () => K.state.motion;

  K.lean = (card, dx) => {
    const t = Math.min(card.offsetWidth * 0.25, 160);
    card.dataset.lean = dx > 0 ? "yes" : dx < 0 ? "no" : "";
    card.style.setProperty("--lean", Math.min(1, Math.abs(dx) / t).toFixed(2));
    card.style.transform = dx ? `translateX(${dx}px) rotate(${dx / 28}deg)` : "";
    return t;
  };

  /* opts: enabled() -> bool, commit(dir) where dir is "yes" or "no" */
  K.swipe = (card, opts) => {
    let x0 = 0, y0 = 0, dx = 0, drag = false, id = null, trail = [];
    card.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || !opts.enabled() || e.target.closest("input")) return;
      x0 = e.clientX; y0 = e.clientY; dx = 0; drag = false; id = e.pointerId; trail = [];
    });
    card.addEventListener("pointermove", (e) => {
      if (e.pointerId !== id) return;
      dx = e.clientX - x0;
      const now = performance.now();
      trail.push([now, dx]);
      while (trail.length > 2 && now - trail[0][0] > 100) trail.shift();
      if (!drag) {
        if (Math.abs(dx) < START || Math.abs(dx) < Math.abs(e.clientY - y0)) return;
        drag = true;
        try { card.setPointerCapture(id); } catch { /* a pointer the browser no longer tracks: the drag still works within the card */ }
        card.classList.remove("settle");
        card.classList.add("dragging");
      }
      K.lean(card, dx);
    });
    const end = (e) => {
      if (e.pointerId !== id) return;
      id = null;
      if (!drag) return;
      card.classList.remove("dragging");
      /* the click that follows a drag must not toggle the row it ended on;
         touch sends no such click, so the guard lasts one turn only */
      const eat = (c) => { c.stopPropagation(); c.preventDefault(); };
      card.addEventListener("click", eat, { capture: true, once: true });
      setTimeout(() => card.removeEventListener("click", eat, { capture: true }), 0);
      const t = K.lean(card, dx);
      const [t0, d0] = trail[0] || [0, dx];
      const v = (dx - d0) / Math.max(1, performance.now() - t0);
      const flick = Math.abs(v) > FLICK && Math.sign(v) === Math.sign(dx) && Math.abs(dx) > START * 4;
      if (e.type === "pointerup" && (Math.abs(dx) > t || flick)) opts.commit(dx > 0 ? "yes" : "no");
      else { card.classList.add("settle"); K.lean(card, 0); }
    };
    card.addEventListener("pointerup", end);
    card.addEventListener("pointercancel", end);
  };

  /* the card leaves the way it was decided: right, left or up for a skip */
  K.flyOut = (card, dir, done) => {
    if (!card || !animated()) { done(); return; }
    const w = window.innerWidth;
    const to = { yes: `translateX(${w}px) rotate(18deg)`, no: `translateX(${-w}px) rotate(-18deg)`, up: "translateY(-60vh) scale(0.9)" }[dir];
    card.dataset.lean = dir === "up" ? "" : dir;
    card.style.setProperty("--lean", "1");
    card.classList.add("anim");
    requestAnimationFrame(() => { card.style.transform = to; card.style.opacity = "0"; });
    let called = false;
    const fin = () => { if (!called) { called = true; done(); } };
    card.addEventListener("transitionend", fin, { once: true });
    setTimeout(fin, 400);   // headless and hidden tabs may not deliver transitionend
  };

  /* undo brings a card back from the side it left by */
  K.flyIn = (card, dir) => {
    if (!card || !animated()) return;
    const from = { yes: "translateX(70vw) rotate(14deg)", no: "translateX(-70vw) rotate(-14deg)", up: "translateY(-40vh) scale(0.95)" }[dir];
    if (!from) return;
    card.style.transform = from;
    card.style.opacity = "0";
    card.getBoundingClientRect();
    card.classList.add("anim");
    card.style.transform = "";
    card.style.opacity = "";
    card.addEventListener("transitionend", () => card.classList.remove("anim"), { once: true });
  };
})();
