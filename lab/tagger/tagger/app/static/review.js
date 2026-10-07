/* knowmoretabs · tagger app · step 3, review
   Each result page with the picked tags it does not hold yet, checked where
   the tag's head (or its zero shot query, under 10 labels) says yes. Flip the
   wrong ones. Grid: every page at once, confirm one or all. Swipe: one page
   per screen as in the mockup; right keeps the checked tags and rejects the
   rest, left rejects all, up skips, u undoes. Every change is saved at once. */
(function () {
  const K = window.KMT;
  let busy = false;
  let front = null;   // the page undo brought back, shown first

  const set = () => K.set;
  const open = (p) => p.status !== "decided";
  const decidable = () => set().pages.filter((p) => p.sugg.length);
  const kept = (p) => p.sugg.filter((s) => s.checked);

  /* the swipe queue: open pages in rank order, then skipped ones in the order they were skipped */
  function queue() {
    const ps = decidable();
    const q = ps.filter((p) => p.status === "open").concat(ps.filter((p) => p.status === "skipped").sort((a, b) => a.at - b.at));
    const f = q.findIndex((p) => p.index === front);
    if (f > 0) q.unshift(...q.splice(f, 1));
    return q;
  }

  function put(p, body, s = set()) {
    const next = K.review.pending.then(async () => {
      const i = s.pages.findIndex((x) => x.index === p.index);
      const saved = await K.api(`/api/sessions/${s.id}/pages/${p.index}`, typeof body === "function" ? body(s.pages[i]) : body);
      s.pages[i] = saved;
      return saved;
    });
    K.review.pending = next.catch(() => {});
    return next;
  }
  const marks = (p, f) => Object.fromEntries(p.sugg.map((s) => [s.tag, f ? f(s) : s.checked]));

  function rows(p, keys) {
    return p.sugg.map((s, i) => {
      const flipped = s.checked !== s.model ? " · flipped" : "";
      return `<li><button type="button" class="sg${keys ? "" : " nokbd"}" data-tag="${K.esc(s.tag)}" aria-pressed="${s.checked}">` +
        (keys ? `<kbd>${i + 1}</kbd>` : "") + `<span class="box" aria-hidden="true"></span>` +
        `<span class="nm"><b>${K.esc(s.tag)}</b><small>${s.source}${flipped}</small></span>` +
        `<span class="conf" style="--p:${s.p}" aria-hidden="true"><i></i></span><span class="pct" title="model confidence">${K.pct(s.p)}</span></button></li>`;
    }).join("");
  }
  const STATUS = { open: "Open", skipped: "Skipped", decided: "Decided" };

  /* ---- grid ---- */
  function gridCard(p) {
    const body = p.sugg.length
      ? `<ol class="sugg">${rows(p, false)}</ol><p class="row-btns"><button type="button" data-act="confirm"${open(p) ? "" : " disabled"}>${open(p) ? "Confirm" : "Confirmed"}</button></p>`
      : `<p class="ex"><i>You already have every picked tag here.</i></p>`;
    return `<li class="hit rcard st-${p.sugg.length ? p.status : "none"}" data-i="${p.index}">${K.thumb(p, "th")}<div class="hb">` +
      `<p class="kick">${p.sugg.length ? STATUS[p.status] : "Nothing to decide"}</p>` +
      `<h3 class="ht">${p.title ? K.esc(p.title) : "<i>Untitled page</i>"}</h3><p class="host">${K.link(p)}</p>` +
      `<ul class="tags">${K.own(p)}</ul>${body}</div></li>`;
  }

  async function flipInGrid(p, tag) {
    await put(p, (latest) => ({ marks: { [tag]: !latest.sugg.find((x) => x.tag === tag).checked } }));
    K.changed();
  }
  async function confirmAll() {
    const todo = decidable().filter(open);
    await Promise.all(todo.map((p) => put(p, { status: "decided" })));  // queued at once, so export waits for all
    K.toast(`Confirmed ${todo.length} page${todo.length === 1 ? "" : "s"}`);
    K.changed();
  }

  /* ---- swipe ---- */
  function acceptLabel(p) {
    const all = p.sugg.length, k = kept(p).length;
    if (k === 0) return "None apply";
    if (k === all) return all === 1 ? "Accept" : `Accept all ${all}`;
    return `Accept ${k}, reject ${all - k}`;
  }
  function swipeCard(p) {
    const near = p.sugg.filter((s) => Math.abs(s.p - 0.5) < 0.15).length;
    const kick = p.status === "skipped" ? "Skipped before · back for a decision" : near ? `<b>${near} of ${p.sugg.length}</b> near a coin toss` : "The model leans one way";
    const shot = p.image ? `<div class="shot"><img src="/img/${p.row}" alt=""></div>`
                         : `<div class="shot none" aria-label="No image captured"><b aria-hidden="true">${K.esc((p.host[0] || "?").toUpperCase())}</b><span>No image captured</span></div>`;
    return `<div class="page">${shot}<div class="cbody"><p class="kick">${kick}</p>` +
      `<h2 class="ptitle">${p.title ? K.esc(p.title) : "<i>Untitled page</i>"}</h2><p class="host">${K.link(p)}</p>` +
      `<p class="lbl">Your tags</p><ul class="tags">${K.own(p)}</ul>` +
      `<p class="lbl">Picked tags</p><ol class="sugg">${rows(p, true)}</ol></div></div>`;
  }

  const card = () => K.$("r-deck").querySelector(".card[data-i]");
  const current = () => queue()[0];

  async function flip(p, tag) {
    await put(p, (latest) => ({ marks: tag === null ? marks(latest, () => true) : { [tag]: !latest.sugg.find((x) => x.tag === tag).checked } }));
    K.changed();
  }

  function commit(p, body, dir, say) {
    if (busy || !p) return;
    busy = true;
    const s = set();
    const b = K.$("r-acts").querySelector(`[data-act="${dir === "up" ? "skip" : dir === "no" ? "reject" : "accept"}"]`);
    b.classList.remove("flash"); b.getBoundingClientRect(); b.classList.add("flash");
    const saving = put(p, body, s);
    saving.catch(() => {}); // handled after the animation, even if the request fails first
    K.flyOut(card(), dir, async () => {
      try {
        const next = await saving;
        if (set() === s) {
          if (front === p.index) front = null;
          K.toast(say(next), undo);
        }
      } catch (err) { K.fail(err); }
      busy = false;
      K.changed();
    });
  }
  const accept = () => { const p = current(); if (p) commit(p, { status: "decided" }, kept(p).length ? "yes" : "no", sayDecided); };
  const reject = () => { const p = current(); if (p) commit(p, { marks: marks(p, () => false), status: "decided" }, "no", sayDecided); };
  const skip = () => { const p = current(); if (p) commit(p, { status: "skipped" }, "up", (x) => `Skipped “${K.short(K.title(x))}”. It comes back at the end.`); };
  function sayDecided(p) {
    const k = kept(p).map((s) => s.tag);
    if (!k.length) return `Rejected ${p.sugg.length === 1 ? "the tag" : `all ${p.sugg.length}`} on “${K.short(K.title(p))}”`;
    return `Kept ${k.join(", ")}${k.length < p.sugg.length ? `, rejected ${p.sugg.length - k.length}` : ""} on “${K.short(K.title(p))}”`;
  }

  /* undo: the latest decided or skipped page in this set opens again, first in the queue */
  async function undo() {
    if (busy) return;
    const last = decidable().filter((p) => p.status !== "open").sort((a, b) => b.at - a.at)[0];
    if (!last) { K.toast("Nothing to undo here."); return; }
    const was = last.status === "skipped" ? "up" : kept(last).length ? "yes" : "no";
    await put(last, { status: "open" });
    front = last.index;
    K.changed();
    K.flyIn(card(), was);
    K.toast(`Back for another look: “${K.short(K.title(last))}”`);
  }

  function renderSwipe() {
    const ps = decidable(), q = queue(), cur = q[0];
    const deck = K.$("r-deck");
    deck.classList.toggle("has-more", q.length > 1);
    if (cur) {
      deck.innerHTML = `<article class="card" data-i="${cur.index}" tabindex="-1" aria-label="${K.esc(K.title(cur))}">` +
        `<span class="stamp yes">${acceptLabel(cur)}</span><span class="stamp no">Reject all</span>${swipeCard(cur)}</article>`;
      const c = card();
      K.swipe(c, { enabled: () => !busy, commit: (dir) => (dir === "yes" ? accept() : reject()) });
      c.querySelectorAll(".sg").forEach((b) => b.addEventListener("click", () => flip(cur, b.dataset.tag).catch(K.fail)));
    } else {
      const n = ps.filter((p) => p.status === "decided").length;
      deck.innerHTML = `<div class="card static"><div class="done"><h2>All decided.</h2><p>${n} pages decided in this set. Undo brings the last one back; export writes the answers file.</p>` +
        `<p class="row-btns"><button type="button" data-go="undo">Undo the last one</button><button type="button" data-go="export">Export</button><button type="button" data-go="search">New search</button></p></div></div>`;
    }
    K.$("r-accept").textContent = cur ? acceptLabel(cur) : "Accept";
    K.$("r-acts").querySelectorAll("button").forEach((b) => {
      b.disabled = b.dataset.act === "undo" ? !ps.some((p) => p.status !== "open") : !cur;
    });
  }

  K.review = {
    pending: Promise.resolve(),
    render() {
      const s = set();
      const swipe = K.state.mode === "swipe";
      K.$("view-review").querySelectorAll("[data-mode]").forEach((b) => b.setAttribute("aria-pressed", b.dataset.mode === K.state.mode));
      if (!s) {
        K.$("r-pos").textContent = "No set yet";
        K.$("r-strip").innerHTML = "";
        K.$("r-grid-view").hidden = false;
        K.$("r-swipe-view").hidden = true;
        K.$("r-grid").innerHTML = `<li class="none">Search, pick tags, then review them here.</li>`;
        K.$("r-confirm-all").disabled = true;
        return;
      }
      const ps = decidable(), q = queue(), cur = swipe ? q[0] : null;
      const n = ps.filter((p) => p.status === "decided").length;
      K.$("r-strip").innerHTML = ps.map((p) => `<li class="${cur && p.index === cur.index ? "cur" : p.status === "skipped" ? "s" : p.status === "decided" ? (kept(p).length ? "a" : "r") : ""}"></li>`).join("");
      K.$("r-pos").innerHTML = `“${K.esc(K.short(s.query))}” · ${K.esc(s.picked.join(", "))} · <b>${n}</b> of ${ps.length} decided`;
      K.$("r-grid-view").hidden = swipe;
      K.$("r-swipe-view").hidden = !swipe;
      if (swipe) renderSwipe();
      else {
        K.$("r-grid").innerHTML = s.pages.map(gridCard).join("");
        K.$("r-confirm-all").disabled = !ps.some(open);
        K.$("r-confirm-all").textContent = `Confirm ${ps.filter(open).length} open page${ps.filter(open).length === 1 ? "" : "s"}`;
      }
    },
    left: () => (set() ? decidable().filter(open).length : 0),
    async load(id) {
      if (set() && set().id === id) return;
      K.set = await K.api(`/api/sessions/${id}`);
      front = null;
    },
    mode(m) { K.state.mode = m; K.save(); K.changed(); },
    key(e) {
      const k = e.key;
      if (k === "g") return K.review.mode(K.state.mode === "grid" ? "swipe" : "grid"), true;
      if (K.state.mode !== "swipe" || !set()) return false;
      if (k === "ArrowRight" || k === "l" || (k === "Enter" && !e.target.closest("button, a"))) return accept(), true;
      if (k === "ArrowLeft" || k === "h") return reject(), true;
      if (k === "ArrowUp" || k === "s") return skip(), true;
      if (k === "u" || (k === "z" && (e.metaKey || e.ctrlKey))) return undo().catch(K.fail), true;
      const p = current();
      if (!p || busy) return false;
      if (k === "a") return flip(p, null).catch(K.fail), true;
      if (/^[1-9]$/.test(k)) {
        const s = p.sugg[Number(k) - 1];
        if (s) flip(p, s.tag).catch(K.fail);
        return true;
      }
      return false;
    }
  };

  K.$("view-review").addEventListener("click", (e) => {
    const m = e.target.closest("[data-mode]");
    if (m) { K.review.mode(m.dataset.mode); return; }
    const a = e.target.closest("[data-act]");
    if (a && !a.disabled) {
      const act = a.dataset.act;
      if (act === "confirm-all") confirmAll().catch(K.fail);
      else if (act === "accept") accept();
      else if (act === "reject") reject();
      else if (act === "skip") skip();
      else if (act === "undo") undo().catch(K.fail);
      else if (act === "confirm") {
        const p = set().pages.find((x) => x.index === Number(a.closest("[data-i]").dataset.i));
        put(p, { status: "decided" }).then(K.changed, K.fail);
      }
      return;
    }
    const sg = e.target.closest(".rcard .sg");
    if (sg) {
      const p = set().pages.find((x) => x.index === Number(sg.closest("[data-i]").dataset.i));
      flipInGrid(p, sg.dataset.tag).catch(K.fail);
      return;
    }
    const g = e.target.closest("[data-go]");
    if (!g) return;
    if (g.dataset.go === "undo") undo().catch(K.fail);
    else if (g.dataset.go === "export") K.exportNow();
    else location.hash = g.dataset.go;
  });
})();
