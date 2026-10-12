/* knowmoretabs · tagger app · core
   The server keeps every decision; this browser keeps only where you are:
   the last search (query, images, untagged only, size; the review's picked
   tags, open set and mode), theme and motion. Results are fetched again
   after a reload. */
(function () {
  const KEY = "kmt-tagger-app-v1";
  const K = (window.KMT = window.KMT || {});

  K.$ = (id) => document.getElementById(id);
  K.esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  K.pct = (p) => Math.round(p * 100) + "%";
  K.plural = (n, word) => `<b>${n}</b> ${word}${n === 1 ? "" : "s"}`;
  K.short = (t) => (t.length > 44 ? t.slice(0, 42).trimEnd() + "…" : t);
  K.title = (p) => p.title || "Untitled page";
  K.byName = (a, b) => a.localeCompare(b, undefined, { sensitivity: "base" });   // tag names, as every list orders them

  /* ---- where you are ---- */
  let saved = {};
  try { saved = JSON.parse(localStorage.getItem(KEY)) || {}; } catch { /* a fresh start */ }
  K.state = {
    search: saved.search || { query: "", images: false, untagged: false, n: 20, picked: [] },
    session: saved.session || null,
    mode: saved.mode || "grid",
    theme: saved.theme || null,
    motion: saved.motion !== undefined ? saved.motion : !matchMedia("(prefers-reduced-motion: reduce)").matches
  };
  K.save = () => { try { localStorage.setItem(KEY, JSON.stringify(K.state)); } catch { /* private mode: the session still works */ } };

  /* what this tab holds from the server: the library, the last results, the open set */
  K.lib = null;
  K.results = null;
  K.set = null;

  /* every path is relative: the Library serves this page and its API below its own /model/ */
  K.api = async (path, body) => {
    let r;
    try {
      r = await fetch(path, body === undefined ? {} : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    } catch (err) { if (err.name === "TypeError") K.outage(); throw err; }   // fetch's network failure
    if (OUTAGE.includes(r.status)) K.outage();
    const out = await r.json();
    if (!r.ok) throw new Error(out.error || `HTTP ${r.status}`);
    return out;
  };
  K.fail = (err) => { if (!K.down) K.toast(`Something went wrong: ${err.message}`); };   // the outage line says it once

  /* ---- the service stopped answering (the Library answers 502, 503 or 504 for it, or is gone itself): one line
     says so for good, both screens and Send stop taking actions, and only Reload goes on. Nothing is retried or
     queued; what was in flight may have been done ---- */
  const OUTAGE = [502, 503, 504];
  K.down = false;
  K.outage = () => {
    if (K.down) return;
    K.down = true;
    K.$("outage").hidden = false;
    K.$("tag-screen").inert = K.$("add-screen").inert = true;
    K.$("send-btn").disabled = true;
  };

  /* ---- a page's picture: the captured image, or the host's initial; `inner` goes on top ---- */
  K.thumb = (p, cls, inner = "") => {
    const initial = K.esc((p.host.replace(/^www\./, "")[0] || "?").toUpperCase());
    return p.image ? `<div class="${cls}"><img src="img/${p.image}" alt="" loading="lazy" decoding="async">${inner}</div>`
                   : `<div class="${cls} none" aria-label="No image captured"><b aria-hidden="true">${initial}</b>${inner}</div>`;
  };
  K.own = (p) => (p.own.length ? p.own.map((t) => `<li class="tag">${K.esc(t)}</li>`).join("") : `<li class="none">None yet</li>`);

  /* ---- open the page: a control for http and https pages only. Its click
     stays its own: no exclusion, drag or decision (the views call K.opens) ---- */
  const web = (url) => { try { return ["http:", "https:"].includes(new URL(url).protocol); } catch { return false; } };
  K.open = (p) => (web(p.url) ? `<a class="open" data-open href="${K.esc(p.url)}" target="_blank" rel="noopener noreferrer" draggable="false" ` +
    `title="Open the page in a new tab (o)" aria-label="Open the page in a new tab">Open ↗</a>` : "");
  K.opens = (e) => { if (!e.target.closest("[data-open]")) return false; e.stopPropagation(); return true; };
  K.openIn = (el) => { const a = el && el.querySelector("[data-open]"); if (a) a.click(); return !!a; };

  /* ---- toast: one message, with Undo when the action can be taken back (its button, u or Ctrl+Z) ---- */
  let toastTimer = 0;
  K.undo = null;   // the shown toast's Undo
  K.toast = (msg, onUndo) => {
    const t = K.$("toast");
    K.$("toast-msg").textContent = msg;
    K.$("toast-undo").hidden = !onUndo;
    K.undo = onUndo ? () => { hide(); onUndo(); } : null;
    K.$("toast-undo").onclick = K.undo;
    t.hidden = false;
    requestAnimationFrame(() => t.classList.add("in"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hide, onUndo ? 6000 : 4500);
    function hide() { K.undo = null; t.classList.remove("in"); setTimeout(() => { if (!t.classList.contains("in")) t.hidden = true; }, 200); }
  };

  /* every view re-renders through this hook (set by app.js) */
  K.changed = () => {};
})();
