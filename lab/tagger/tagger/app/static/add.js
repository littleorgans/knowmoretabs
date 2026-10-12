/* knowmoretabs · tagger app · Add link
   A link in, Add (or Enter, or a paste of a web address into the empty
   box), then four segments fill as the server's job reports knowmoretabs'
   stages, polled every 250 ms: Library, Content, Image, Search. From the
   moment the box holds a web address the page's tile (a placeholder until
   the library lists it) and its tag panel show, and stay put through every
   stage. Tags picked before the page has a row wait striped, go with the
   link when it is added and are written at Indexed, in one change with one
   Undo, as the library's own write; a newer link does not stop an older job
   writing its own, and a write that fails is said, never sent again. A
   refusal clears them. At Indexed the model's suggestions for the page fill the
   Suggested row, less the tags picked, and stay put while it is on screen.
   A failed stage shows its value and one action, which starts a job of its
   own: Retry runs only the stages that failed. It names the page by the key
   knowmoretabs reports holding it under, which may be a cleaned form of the link.
   `#add=<encoded link>&title=<encoded title>` fills the box and focuses
   Add, which waits for Enter; the title goes with that link. */
(function () {
  const K = window.KMT;
  const { html, nothing, render, unsafeHTML } = K.lit;
  const POLL = 250;
  const SEGS = ["library", "content", "image", "search"];

  /* ---- what a job's stages read as (design section 9) ---- */
  const RUNNING = { web: "Web", github: "GitHub", x: "X", youtube: "YouTube", headless: "Rendering", signed_in: "Signed in" };
  const TIER = { ...RUNNING, headless: "Headless" };
  const STATUS = {
    ok: "Ok", thin: "Thin", empty_shell: "Empty", blocked: "Blocked", behind_login: "Behind login", paywalled: "Paywalled",
    not_found: "Not found", error: "Error", not_html: "Not HTML", media: "Media", skipped: "Skipped", unavailable: "Unavailable",
    unknown: "Not recorded", off: "Chrome not reachable", not_running: "Chrome not reachable", not_allowed: "Not allowed",
  };
  const SOFT = ["thin", "empty_shell", "not_html", "media", "skipped", "unknown"];
  const WITH_TIER = ["ok", "thin"], WITH_CODE = ["blocked", "not_found", "error"];
  const SIGN_IN = ["blocked", "behind_login", "paywalled"];
  const REFUSED = ["thin", "empty_shell"];                          // signed in too after the site refused the render (403)
  const PUBLIC = ["web", "github", "x", "youtube", "headless"];   // the tiers a signed in read starts from
  const UNREACHED = ["off", "not_running", "not_allowed"];         // Chrome out of reach: Retry signed in
  const LISTED = ["added", "known"];
  const wait = (e) => e.state === "waiting" || e.state === "retrying";
  const pending = (e, running) => (e.state === "retrying" ? ["Retrying", "wait"] : e.state === "waiting" ? ["Waiting", "wait"] : [running, "run"]);

  function library(e, failed) {
    if (e.reason === "snapshot") return ["Read only snapshot", "bad"];
    if (failed && e.state !== "done") return ["Not added", "bad"];
    if (e.state !== "done") return wait(e) ? ["Waiting", "wait"] : ["Adding", "run"];
    if (e.reason === "not_in_library") return ["Not in library", "bad"];
    return { added: ["Added", "ok"], known: ["Already in library", "ok"], forgotten: ["Forgotten", "bad"] }[e.value] || ["Not a web page", "bad"];
  }
  function content(e) {
    if (e.state !== "done") return pending(e, RUNNING[e.tier] || "Fetching");
    if (e.status === "error" && e.reason === "timeout") return ["Timed out", "bad"];
    const name = STATUS[e.status] || e.status;
    const text = WITH_TIER.includes(e.status) && TIER[e.tier] ? `${name} · ${TIER[e.tier]}`
      : WITH_CODE.includes(e.status) && e.http_status ? `${name} · ${e.http_status}` : name;
    return [text, e.status === "ok" ? "ok" : SOFT.includes(e.status) ? "soft" : "bad"];
  }
  const IMAGE = { ok: ["Ok", "ok"], error: ["Error", "bad"], unavailable: ["Unavailable", "bad"] };
  function image(e) {   /* a failed image says the reason it recorded */
    if (e.state !== "done") return pending(e, "Fetching");
    const [name, look] = IMAGE[e.status] || ["No image", "soft"];
    return [look === "bad" && e.reason ? `${name} · ${e.reason}` : name, look];
  }
  const search = (e) => (e.state !== "done" ? ["Indexing", "run"] : e.value === "indexed" ? ["Indexed", "ok"] : ["Not indexed", "bad"]);

  /* the stages Retry runs again (the contract's lab mapping), all in one run: the text after an error or a
     timeout, or signed in again when Chrome was out of reach; the image after an error. Nothing else. */
  function failed(st, signed_in = false) {
    const text = st.content && st.content.status;
    signed_in ||= UNREACHED.includes(text);
    const retry = [...(text === "error" || signed_in ? ["content"] : []), ...(st.image && st.image.status === "error" ? ["image"] : [])];
    return retry.length ? { action: "add", retry, signed_in } : null;
  }
  /* what a job asked for; a failed one is asked again, after a Restore that held as an add */
  const request = (job) => (job.restored ? { action: "add" } : { action: job.action, retry: job.retry, signed_in: job.signed_in, title: job.title });

  /* a job as the screen shows it: per segment [value, look], the one action [label, request], and the tile's state */
  function say(job) {
    const st = job.stages, read = { library: (e) => library(e, job.failed), content, image, search };
    const segs = SEGS.map((k) => (st[k] ? read[k](st[k]) : ["", ""]));
    const lib = st.library || {}, got = st.content || {}, indexed = !!st.search && st.search.value === "indexed";
    const signed = PUBLIC.includes(got.tier)
      && (SIGN_IN.includes(got.status) || REFUSED.includes(got.status) && (got.reason || "").endsWith("not rendered: HTTP 403"));
    const again = failed(st, signed);
    let act = null;
    if (lib.value === "forgotten") act = ["Restore", { action: "restore" }];
    else if (job.failed) act = ["Retry", request(job)];
    else if (signed) act = ["Try signed in", again];
    else if (again) act = ["Retry", again];
    else if (got.status === "not_found") act = ["Remove", { action: "forget" }];
    else if (st.search && st.search.state === "done" && !indexed) act = ["Retry", { action: "index" }];   // lab only
    return { segs, act, framed: LISTED.includes(lib.value), indexed, title: (job.page && job.page.title) || got.title || "",
             picture: st.image ? st.image.state !== "done" ? "wait" : st.image.status === "ok" ? "ok" : "none" : indexed ? "none" : "" };
  }

  /* ---- the job shown, the polls, and the picks held for a page without a row ---- */
  let job = null;      // the job on screen
  let asked = 0;       // the latest job started, the one drawn
  let row = null;      // the on screen page's row (in K.pages), kept while the jobs drawn share its link
  let cleared = 0;     // the job whose link left the box once the library listed it
  let offer = null;    // the request the action button starts
  let titled = null;   // [link, title] from the deep link, sent with that link's Add
  let shown = null;    // the link the panel last showed: another one resets it
  let suggested = { url: null, tags: [] };   // the model's, for the page last indexed on screen, fixed then: a pick never moves them
  const draft = new Set();     // picks for the box's link, before Add
  const held = new Map();      // link -> picks for a submitted link whose row is not known yet
  const latest = new Map();    // link -> the newest job started for it: only that one writes its picks
  const pause = (ms) => new Promise((r) => setTimeout(r, ms));
  const web = (s) => { try { return ["http:", "https:"].includes(new URL(s).protocol); } catch { return false; } };

  /* a job polls while drawn, and after that while it is its link's newest and picks wait for its row */
  async function start(url, ask = { action: "add" }, link = url) {
    const mine = ++asked;
    latest.set(url, mine);
    latest.set(link, mine);   // a follow up supersedes the link's job too: that job's late poll drops none of its picks
    try {
      let j = await K.api("api/add", { url, ...ask });
      for (;;) {
        if (latest.get(url) !== mine) return;   // a reply already in flight may belong to a superseded job
        if (link !== url && held.has(link)) {   // the first reply moves picks, including ones made while it waited
          held.set(url, new Set([...(held.get(url) || []), ...held.get(link)]));
          held.delete(link);
        }
        const drawn = mine === asked;
        if (drawn && link !== url && box.value.trim() === link) box.value = url;   // the box names the key: a forgotten link's Restore is not its draft
        link = url;   // a background follow up owns its picks too, even when it is never shown
        if (drawn && (j.action !== "forget" || j.finished)) show(j);   // a Remove shows its outcome only
        track(j, drawn);
        if (j.finished) return settle(j);
        await pause(POLL);
        if (mine !== asked && !(latest.get(url) === mine && held.has(url))) return;
        j = await K.api(`api/add/${j.id}`);
      }
    } catch (err) { if (mine === asked) K.fail(err); }
  }
  async function settle(j) {
    if (j.stages.library && [...LISTED, "forgotten"].includes(j.stages.library.value)) await K.loadLibrary();
    K.changed();
  }
  /* every poll's job, drawn or not: picks held for its link are written once the page has a row, and dropped
     when it will not get one from this job */
  function track(j, drawn) {
    const picks = held.get(j.url), lib = j.stages.library;
    if (!picks) return;
    if (j.page) {
      held.delete(j.url);
      K.tagPage(K.know([j.page], false)[0], [...picks]);
    } else if (lib && lib.state === "done" && !LISTED.includes(lib.value)) held.delete(j.url);   // refused: the segment says why
    else if (j.finished && !drawn) {   // drawn, Retry is offered and its success writes them
      held.delete(j.url);
      if (!j.failed) K.toast(`Tags for ${K.short(j.url)} not added: the page is not indexed`);
    }
  }
  function show(j) {
    if (job && job.url !== j.url && job.key !== j.key) {   // another page: the last one's row and, once it is done, its picks go
      row = null;
      if (job.finished) track(job, false);
    }
    if (j.action === "forget" && !j.failed && row !== null) {   // removed: out of every view
      K.gone.add(row);
      row = null;
    } else if (j.page && row === null) {   // indexed: the page is known as Indexed shows
      row = K.know([j.page])[0];
      K.gone.delete(row);
    } else if (j.page) Object.assign(K.pages.get(row), { ...j.page, tags: K.pages.get(row).tags });   // its tags are this tab's
    if (j.page && suggested.url !== j.url) suggested = { url: j.url, tags: j.page.suggested.filter((t) => !waiting(j.url, t)) };
    job = j;
    const box = K.$("a-link");
    if (LISTED.includes((j.stages.library || {}).value) && cleared !== j.id) {   // listed: the box is ready for the next link
      cleared = j.id;
      if (box.value.trim() === j.url) {
        box.value = "";
        draft.clear();
        K.$("a-go").disabled = true;
        box.focus({ preventScroll: true });
      }
    }
    draw();
  }

  /* ---- what the panel acts on: the box's link before it is added, else the job on screen ---- */
  function target() {
    const link = K.$("a-link").value.trim();
    if (web(link) && (!job || link !== job.url)) return { url: link, draft: true };
    return job && { url: job.url, draft: false };
  }
  const waiting = (url, tag) => !!held.get(url) && held.get(url).has(tag);
  /* a pick: written at once for a page with a row (with Undo), else held for the row */
  function pick(tag) {
    const t = target();
    if (!t) return;
    if (!t.draft && row !== null) {
      if (K.has(row, tag)) K.untagSelection(tag, [row]); else K.tagSelection(tag, [row]);
      return;
    }
    const on = (t.draft && draft.has(tag)) || waiting(t.url, tag);
    draft.delete(tag);
    if (held.has(t.url)) held.get(t.url).delete(tag);
    if (held.has(t.url) && !held.get(t.url).size) held.delete(t.url);
    if (!on) {
      if (t.draft && latest.get(t.url) !== asked) draft.add(tag);
      else held.set(t.url, (held.get(t.url) || new Set()).add(tag));
    }
    draw();
  }

  /* ---- drawing ---- */
  const host = (url) => { try { return new URL(url).hostname.replace(/^www\./, ""); } catch { return ""; } };
  function thumb(s, page, url) {
    if (s && s.picture === "ok" && page) return unsafeHTML(K.thumb(page, "th"));
    if (s && s.picture === "none") return unsafeHTML(K.thumb({ host: host(url), image: null }, "th"));
    return html`<div class="th${s && s.picture === "wait" ? " wait" : ""}"></div>`;
  }
  /* the page's tile; for a link not yet added (`s` null) its placeholder, the same size */
  function tile(url, s) {
    const page = s && row !== null ? K.pages.get(row) : null;
    const tags = page ? page.tags : [], title = s && s.title;
    return html`<article class="hit tile${s && s.indexed ? " sel" : ""}">${thumb(s, page, url)}${page ? html`<div class="ctl">${unsafeHTML(K.open(page))}</div>` : nothing}<div class="hb">
      <h3 class="ht${title ? "" : " link"}">${title || url}</h3><p class="host">${host(url)}</p>
      <ul class="tags">${tags.length ? tags.map(K.views.chip) : html`<li class="none">Untagged</li>`}</ul></div></article>`;
  }
  function draw() {
    const t = target(), s = t && !t.draft ? say(job) : null, lib = job && job.stages.library;
    K.$("a-stages").hidden = !t;
    K.$("a-stages").classList.toggle("draft", !s);   // a link not yet added keeps the bar's room, empty
    (s ? s.segs : SEGS.map(() => ["", ""])).forEach(([value, look], i) => {
      const seg = K.$(`a-${SEGS[i]}`);
      seg.dataset.k = look;
      seg.querySelector(".val").textContent = value;
    });
    K.$("a-secs").textContent = s ? `${job.seconds.toFixed(1)} s` : "";
    const act = K.$("a-act");
    act.hidden = !(s && s.act);
    act.textContent = s && s.act ? s.act[0] : "";
    offer = s && s.act ? s.act[1] : null;
    /* shown from the link until the library refuses it, or knowmoretabs could not run */
    const live = !!t && (t.draft || s.framed || !job.failed && !(lib && lib.state === "done"));
    const result = K.$("a-result"), opening = result.hidden && live;
    result.hidden = !live;
    if (opening) { result.classList.remove("in"); void result.offsetWidth; result.classList.add("in"); }
    if (!live) return;
    if (shown !== t.url) { shown = t.url; K.panel.reset(); }
    render(tile(t.draft ? t.url : job.key, s), K.$("a-tile"));
    const has = (tag) => !t.draft && row !== null && K.has(row, tag);
    const sugg = !t.draft && suggested.url === t.url ? suggested.tags : [];
    K.panel.render({ has, pend: (tag) => (t.draft && draft.has(tag)) || waiting(t.url, tag), pick, sugg });
  }

  /* ---- the deep link: fill the box, focus Add, wait for Enter ---- */
  const decode = (s) => { try { return decodeURIComponent(s); } catch { return s; } };
  function enter() {
    const m = location.hash.match(/^#add=(.*?)(?:&title=(.*))?$/s);
    const link = m ? decode(m[1]) : null;
    titled = m && m[2] ? [link.trim(), decode(m[2])] : null;
    if (link !== null) {
      K.$("a-link").value = link;
      draft.clear();
      K.$("a-go").disabled = !link.trim();
      K.$("a-go").focus();
    } else K.$("a-link").focus();
    draw();
  }

  K.add = { say, start, enter, render: draw };

  /* ---- wiring ---- */
  const box = K.$("a-link");
  const submit = () => {   // the draft's picks go with the link, held for its row
    const url = box.value.trim();
    if (!url) return;
    if (draft.size) held.set(url, new Set([...(held.get(url) || []), ...draft]));
    draft.clear();
    start(url, { action: "add", title: titled && titled[0] === url ? titled[1] : null });
  };
  K.$("a-form").addEventListener("submit", (e) => { e.preventDefault(); submit(); });
  box.addEventListener("input", () => {   // a web address shows the panel at once
    K.$("a-go").disabled = !box.value.trim();
    if (box.value.trim() !== shown) draft.clear();   // a draft's picks belong to its exact address
    draw();
  });
  box.addEventListener("paste", (e) => {   // a web address pasted into the empty box starts at once
    const text = (e.clipboardData ? e.clipboardData.getData("text") : "").trim();
    if (box.value.trim() || !web(text)) return;
    e.preventDefault();
    box.value = text;
    K.$("a-go").disabled = false;
    submit();
  });
  K.$("a-act").addEventListener("click", () => {   // a follow up names the page's key
    if (!job || !offer) return;
    start(job.key, offer, job.url);
  });
  K.$("a-tile").addEventListener("click", (e) => {
    if (K.opens(e)) return;
    const x = e.target.closest("[data-untag]");
    if (x && row !== null) K.untag(row, x.dataset.untag);
  });
})();
