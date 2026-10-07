/* knowmoretabs · tagger app · core
   The server keeps every decision; this browser keeps only where you are:
   the last search (query, images, size, picked tags), the open set, the
   review mode, theme and motion. Results are fetched again after a reload. */
(function () {
  const KEY = "kmt-tagger-app-v1";
  const K = (window.KMT = window.KMT || {});

  K.$ = (id) => document.getElementById(id);
  K.esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  K.pct = (p) => Math.round(p * 100) + "%";
  K.short = (t) => (t.length > 44 ? t.slice(0, 42).trimEnd() + "…" : t);
  K.title = (p) => p.title || "Untitled page";

  /* ---- where you are ---- */
  let saved = {};
  try { saved = JSON.parse(localStorage.getItem(KEY)) || {}; } catch { /* a fresh start */ }
  K.state = {
    search: saved.search || { query: "", images: false, n: 20, picked: [] },
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

  K.api = async (path, body) => {
    const r = await fetch(path, body === undefined ? {} : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    const out = await r.json();
    if (!r.ok) throw new Error(out.error || `HTTP ${r.status}`);
    return out;
  };
  K.fail = (err) => K.toast(`Something went wrong: ${err.message}`);

  /* ---- a page's picture: the captured image, or the host's initial ---- */
  K.thumb = (p, cls) => {
    const initial = K.esc((p.host.replace(/^www\./, "")[0] || "?").toUpperCase());
    return p.image ? `<div class="${cls}"><img src="/img/${p.row}" alt="" loading="lazy" decoding="async"></div>`
                   : `<div class="${cls} none" aria-label="No image captured"><b aria-hidden="true">${initial}</b></div>`;
  };
  K.own = (p) => (p.own.length ? p.own.map((t) => `<li class="tag">${K.esc(t)}</li>`).join("") : `<li class="none">None yet</li>`);
  K.link = (p) => `<a class="open" href="${K.esc(p.url)}" target="_blank" rel="noopener noreferrer" title="Open the page in a new tab">${K.esc(p.host)} ↗</a>`;

  /* ---- toast ---- */
  let toastTimer = 0;
  K.toast = (msg, onUndo) => {
    const t = K.$("toast");
    K.$("toast-msg").textContent = msg;
    K.$("toast-undo").hidden = !onUndo;
    K.$("toast-undo").onclick = () => { hide(); onUndo(); };
    t.hidden = false;
    requestAnimationFrame(() => t.classList.add("in"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hide, 4500);
    function hide() { t.classList.remove("in"); setTimeout(() => { if (!t.classList.contains("in")) t.hidden = true; }, 200); }
  };

  /* every view re-renders through this hook (set by app.js) */
  K.changed = () => {};
})();
