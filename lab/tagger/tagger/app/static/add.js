/* knowmoretabs · tagger app · Add link
   A link in, Add (or Enter, or a paste of a web address into the empty
   box), then four segments fill as the server's job reports knowmoretabs'
   stages, polled every 250 ms: Library, Content, Image, Search. Once the
   library lists the page its tile is framed; title and picture fill in as
   they land; at Indexed the tile and the strip (every app tag, by name)
   open, acting on this page only. A failed stage shows its value and one
   action, which starts a job of its own. `#add=<encoded link>` fills the
   box and focuses Add, which waits for Enter. */
(function () {
  const K = window.KMT;
  const { html, nothing, render, unsafeHTML } = K.lit;
  const POLL = 250;
  const SEGS = ["library", "content", "image", "search"];

  /* ---- what a job's stages read as (design section 9) ---- */
  const RUNNING = { web: "Web", github: "GitHub", x: "X", youtube: "YouTube", pdf: "PDF", headless: "Rendering", signed_in: "Signed in" };
  const TIER = { ...RUNNING, headless: "Headless" };
  const STATUS = {
    ok: "Ok", thin: "Thin", empty_shell: "Empty", blocked: "Blocked", behind_login: "Behind login", paywalled: "Paywalled",
    not_found: "Not found", error: "Error", timeout: "Timed out", not_html: "Not HTML", media: "Media", skipped: "Skipped",
    unavailable: "Unavailable", chrome_not_running: "Chrome not reachable", off: "Chrome not reachable", not_allowed: "Not allowed",
  };
  const SOFT = ["thin", "empty_shell", "not_html", "media", "skipped"];
  const WITH_TIER = ["ok", "thin"], WITH_CODE = ["blocked", "not_found", "error"];
  const SIGN_IN = ["blocked", "behind_login", "paywalled"];
  const RETRY = ["timeout", "error", "chrome_not_running", "off", "not_allowed", "unavailable"];
  const LISTED = ["added", "known"];
  const wait = (e) => e.state === "waiting" || e.state === "retrying";

  function library(e, failed) {
    if (e.reason === "snapshot") return ["Read only snapshot", "bad"];
    if (failed && e.state !== "done") return ["Not added", "bad"];
    if (e.state !== "done") return wait(e) ? ["Waiting", "wait"] : ["Adding", "run"];
    return { added: ["Added", "ok"], known: ["Already in library", "ok"], forgotten: ["Forgotten", "bad"] }[e.value] || ["Not a web page", "bad"];
  }
  function content(e) {
    if (e.state === "retrying") return ["Retrying", "wait"];
    if (e.state === "waiting") return ["Waiting", "wait"];
    if (e.state !== "done") return [RUNNING[e.tier] || "Fetching", "run"];
    const name = STATUS[e.status] || e.status;
    const text = WITH_TIER.includes(e.status) && TIER[e.tier] ? `${name} · ${TIER[e.tier]}`
      : WITH_CODE.includes(e.status) && e.http_status ? `${name} · ${e.http_status}` : name;
    return [text, e.status === "ok" ? "ok" : SOFT.includes(e.status) ? "soft" : "bad"];
  }
  const image = (e) => (e.state !== "done" ? ["Fetching", "run"] : e.status === "ok" ? ["Ok", "ok"] : ["No image", "soft"]);
  const search = (e) => (e.state !== "done" ? ["Indexing", "run"] : e.value === "indexed" ? ["Indexed", "ok"] : ["Not indexed", "bad"]);

  /* a job as the screen shows it: per segment [value, look], the one action, and the tile's state */
  function say(job) {
    const st = job.stages, read = { library: (e) => library(e, job.failed), content, image, search };
    const segs = SEGS.map((k) => (st[k] ? read[k](st[k]) : ["", ""]));
    const lib = st.library || {}, got = st.content || {}, indexed = !!st.search && st.search.value === "indexed";
    const again = job.action;   // Retry repeats what failed
    let act = null;
    if (lib.value === "forgotten") act = ["Restore", "restore"];
    else if (job.failed) act = ["Retry", again];
    else if (st.search && st.search.state === "done" && !indexed) act = ["Retry", "index"];
    else if (SIGN_IN.includes(got.status)) act = ["Try signed in", "signed_in"];
    else if (got.status === "not_found") act = ["Remove", "forget"];
    else if (RETRY.includes(got.status)) act = ["Retry", again === "index" ? "add" : again];
    return { segs, act, framed: LISTED.includes(lib.value), indexed, title: (job.page && job.page.title) || got.title || "",
             picture: st.image ? st.image.state !== "done" ? "wait" : st.image.status === "ok" ? "ok" : "none" : indexed ? "none" : "" };
  }

  /* ---- the job shown, and the polls ---- */
  let job = null;      // the job on screen
  let asked = 0;       // the latest job started; a poll for an older one stops
  let row = null;      // the indexed page's row (in K.pages)
  let cleared = 0;     // the job whose link left the box once the library listed it
  const pause = (ms) => new Promise((r) => setTimeout(r, ms));
  const web = (s) => { try { return ["http:", "https:"].includes(new URL(s).protocol); } catch { return false; } };

  async function start(url, action = "add") {
    const mine = ++asked;
    try {
      let j = await K.api("/api/add", { url, action });
      while (mine === asked) {
        if (j.action !== "forget" || j.finished) show(j);   // a Remove shows its outcome only
        if (j.finished) return settle(j);
        await pause(POLL);
        if (mine !== asked) return;
        j = await K.api(`/api/add/${j.id}`);
      }
    } catch (err) { if (mine === asked) K.fail(err); }
  }
  async function settle(j) {
    if (j.page) {
      row = K.know([j.page])[0];
      K.gone.delete(row);
    }
    if (j.stages.library && [...LISTED, "forgotten"].includes(j.stages.library.value)) K.lib = await K.api("/api/library");
    K.changed();
  }
  function show(j) {
    if (!j.page) {
      if (j.action === "forget" && !j.failed && row !== null) K.gone.add(row);   // removed: out of every view
      row = null;
    }
    job = j;
    const box = K.$("a-link"), s = say(j);
    if (s.framed && cleared !== j.id) {   // listed: the box is ready for the next link
      cleared = j.id;
      if (box.value.trim() === j.url) {
        box.value = "";
        K.$("a-go").disabled = true;
        box.focus({ preventScroll: true });
      }
    }
    draw(s);
  }

  /* ---- drawing ---- */
  const host = (url) => { try { return new URL(url).hostname.replace(/^www\./, ""); } catch { return ""; } };
  function thumb(s, page) {
    if (s.picture === "ok" && page) return unsafeHTML(K.thumb(page, "th"));
    if (s.picture === "none") return unsafeHTML(K.thumb({ host: host(job.url), image: null }, "th"));
    return html`<div class="th${s.picture === "wait" ? " wait" : ""}"></div>`;
  }
  function tile(s) {
    const page = row !== null ? K.pages.get(row) : null;
    const tags = page ? page.tags : [];
    return html`<article class="hit tile${s.indexed ? " sel" : ""}">${thumb(s, page)}${page ? html`<div class="ctl">${unsafeHTML(K.open(page))}</div>` : nothing}<div class="hb">
      <h3 class="ht${s.title ? "" : " link"}">${s.title || job.url}</h3><p class="host">${host(job.url)}</p>
      <ul class="tags">${tags.length ? tags.map(K.views.chip) : html`<li class="none">Untagged</li>`}</ul></div></article>`;
  }
  function draw(s = job && say(job)) {
    K.$("a-stages").hidden = !s;
    if (!s) return;
    s.segs.forEach(([value, look], i) => {
      const seg = K.$(`a-${SEGS[i]}`);
      seg.dataset.k = look;
      seg.querySelector(".val").textContent = value;
    });
    K.$("a-secs").textContent = `${job.seconds.toFixed(1)} s`;
    const act = K.$("a-act");
    act.hidden = !s.act;
    act.textContent = s.act ? s.act[0] : "";
    act.dataset.action = s.act ? s.act[1] : "";
    const result = K.$("a-result"), opening = result.hidden && s.framed;
    result.hidden = !s.framed;
    if (opening) { result.classList.remove("in"); void result.offsetWidth; result.classList.add("in"); }
    if (!s.framed) return;
    render(tile(s), K.$("a-tile"));
    const open = s.indexed && row !== null;
    K.$("a-tagrow").classList.toggle("off", !open);
    K.$("a-tagrow").setAttribute("aria-disabled", String(!open));
    const rows = open ? [row] : [];
    K.$("a-strip").innerHTML = (K.lib ? K.lib.app_tags : []).map((t) => K.strip.chip(t, rows, false)).join("");
  }

  /* ---- the deep link: fill the box, focus Add, wait for Enter ---- */
  function enter() {
    const m = location.hash.match(/^#add=(.*)$/s);
    let link = null;
    if (m) { try { link = decodeURIComponent(m[1]); } catch { link = m[1]; } }
    if (link !== null) {
      K.$("a-link").value = link;
      K.$("a-go").disabled = !link.trim();
      K.$("a-go").focus();
    } else K.$("a-link").focus();
  }

  K.add = { say, start, enter, render: () => draw() };

  /* ---- wiring ---- */
  const box = K.$("a-link");
  const submit = () => { const url = box.value.trim(); if (url) start(url); };
  K.$("a-form").addEventListener("submit", (e) => { e.preventDefault(); submit(); });
  box.addEventListener("input", () => { K.$("a-go").disabled = !box.value.trim(); });
  box.addEventListener("paste", (e) => {   // a web address pasted into the empty box starts at once
    const text = (e.clipboardData ? e.clipboardData.getData("text") : "").trim();
    if (box.value.trim() || !web(text)) return;
    e.preventDefault();
    box.value = text;
    K.$("a-go").disabled = false;
    submit();
  });
  K.$("a-act").addEventListener("click", (e) => { if (job) start(job.url, e.currentTarget.dataset.action); });
  K.$("a-tile").addEventListener("click", (e) => {
    if (K.opens(e)) return;
    const x = e.target.closest("[data-untag]");
    if (x && row !== null) K.untag(row, x.dataset.untag);
  });
  K.$("a-strip").addEventListener("click", (e) => {
    const b = e.target.closest("button[data-act]");
    if (!b || row === null) return;
    const tag = b.closest("[data-tag]").dataset.tag;
    if (b.dataset.act === "add") K.tagSelection(tag, [row]);
    else if (b.dataset.act === "remove") K.untagSelection(tag, [row]);
    else { location.hash = "#tag"; K.views.like(tag); }
  });
  const tellNew = K.strip.sayIn("a-new-msg");
  const closeNew = () => { K.$("a-newtag").hidden = true; K.$("a-new").hidden = false; K.$("a-new-tag").value = ""; tellNew(""); };
  K.$("a-new").addEventListener("click", () => { K.$("a-new").hidden = true; K.$("a-newtag").hidden = false; K.$("a-new-tag").focus(); });
  K.$("a-newtag").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (row !== null && await K.strip.create(K.$("a-new-tag"), tellNew, [row])) closeNew();
  });
  K.$("a-new-tag").addEventListener("keydown", (e) => { if (e.key === "Escape") { e.preventDefault(); closeNew(); K.$("a-new").focus(); } });
  K.$("a-new-tag").addEventListener("input", () => tellNew(""));
})();
