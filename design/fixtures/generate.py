#!/usr/bin/env python3
"""Builds a synthetic library.json the frontend can be designed against.

slice: library
why: The UI has to be judged at the size it will really be used at. 2000 pages
     with a long tail of one-offs and a hard core of permanent residents is the
     shape the real archive takes; a hand-written fixture of twelve rows would
     hide every density and performance problem worth designing for.

Everything here is invented. No URL, title or timestamp comes from a real
browser profile. Deterministic: same seed, same file.

Usage:
    python3 fixtures/generate.py            # write library.json, inject into index.html
    python3 fixtures/generate.py --no-inject
"""

import hashlib
import json
import random
import re
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "library.json"
INDEX = HERE.parent / "index.html"

SEED = 20260920
SNAPSHOT_COUNT = 40
TARGET_DOMAINS = 155
RESIDENTS = 46          # pages open in the very first snapshot that never close
FORGOTTEN_SHARE = 0.03
LAST_CAPTURE = datetime(2026, 9, 19, 8, 44, 15, tzinfo=timezone.utc)

# --- vocabulary --------------------------------------------------------------

NOUNS = """archive buffer cache cursor daemon fixture gateway handle index journal
kernel lattice manifest nonce octet parser queue registry schema token undo vector
window yield zone allocator arena binding closure digest entropy frame grammar heap
invariant latency mutex namespace opcode pointer quota render socket tuple unicode
viewport widget checksum drift epoch fold glyph hash""".split()

ADJS = """ambient brittle careful dense eager frugal graceful hidden idle jagged
keen lazy modest narrow opaque patient quiet rugged shallow tidy unusual vivid warm
exact final gentle humble inert loose minimal nested odd plain rare solid true""".split()

VERBS = """building debugging designing fixing measuring porting reading refactoring
rewriting shipping sketching testing tracing tuning understanding profiling
benchmarking documenting migrating deprecating""".split()

TOPICS = [
    "session files", "sqlite", "the borrow checker", "css grid", "container queries",
    "colour schemes", "tab groups", "snss parsing", "atomic writes", "file locking",
    "error handling", "content security policy", "virtual scrolling",
    "accessibility trees", "keyboard navigation", "text rendering", "font stacks",
    "json schemas", "backpressure", "retry budgets", "crash recovery", "cold starts",
    "binary size", "trait objects", "zero-copy parsing", "string interning",
    "focus management", "reduced motion", "subpixel layout", "cache invalidation",
]

SITE_NAMES = {
    "doc.rust-lang.org": "Rust", "rust-lang.github.io": "Rust",
    "docs.python.org": "Python", "docs.rs": "docs.rs", "man7.org": "Linux man-pages",
    "developer.mozilla.org": "MDN", "developer.chrome.com": "Chrome for Developers",
    "sqlite.org": "SQLite", "www.postgresql.org": "PostgreSQL", "go.dev": "Go",
    "clang.llvm.org": "Clang", "wiki.archlinux.org": "ArchWiki",
    "web.dev": "web.dev", "caniuse.com": "Can I use",
    "news.ycombinator.com": "Hacker News", "lobste.rs": "Lobsters",
    "tildes.net": "Tildes", "pypi.org": "PyPI", "www.npmjs.com": "npm",
    "crates.io": "crates.io", "stackoverflow.com": "Stack Overflow",
    "www.theguardian.com": "the Guardian", "arstechnica.com": "Ars Technica",
    "apnews.com": "AP News", "www.bbc.co.uk": "BBC",
}


def site_name(host):
    if host in SITE_NAMES:
        return SITE_NAMES[host]
    label = host.split(".")[0]
    if label in ("www", "blog", "docs", "doc", "dev") and host.count(".") > 1:
        label = host.split(".")[1]
    return title_case(label)

THINGS = """Event_loop Garbage_collection Memory_model Hash_table B-tree Bloom_filter
Consistent_hashing Copy-on-write Write-ahead_logging Unicode_normalization
Quadtree Levenshtein_distance Reservoir_sampling Zipf%27s_law Amdahl%27s_law
Bus_factor Cache_coherence Dead_reckoning Elliptic_curve Fenwick_tree""".split()

ORGS = """oxidize northfold tinybird quietlabs paperclip lanternfish slowweb
corvid-io hollowpoint ninefold basalt driftwood shortwave inkpot meridian
foxglove halfmoon riverbend stillwater tanglefoot""".split()

REPOS = """knowmoretabs sessionkit snss-rs tabledeck quietfs ledgerline hoist
plumb rasterbox colourway gridlock marginalia typeset scrollport keymap
undobuffer atomicwrite forgetful indexcard papertrail""".split()

# --- domains -----------------------------------------------------------------

NAMED = [
    ("news.ycombinator.com", "forum", 9.0),
    ("github.com", "code", 9.0),
    ("developer.mozilla.org", "docs", 7.0),
    ("en.wikipedia.org", "wiki", 6.0),
    ("stackoverflow.com", "qa", 5.5),
    ("doc.rust-lang.org", "docs", 5.0),
    ("lobste.rs", "forum", 4.0),
    ("arxiv.org", "paper", 4.0),
    ("docs.python.org", "docs", 3.5),
    ("crates.io", "code", 3.5),
    ("developer.chrome.com", "docs", 3.0),
    ("web.dev", "ref", 3.0),
    ("caniuse.com", "ref", 2.5),
    ("css-tricks.com", "ref", 2.5),
    ("www.theguardian.com", "news", 3.0),
    ("arstechnica.com", "news", 2.5),
    ("apnews.com", "news", 2.0),
    ("www.bbc.co.uk", "news", 2.0),
    ("blog.cloudflare.com", "blog", 2.0),
    ("rust-lang.github.io", "docs", 2.0),
    ("www.youtube.com", "video", 3.0),
    ("gitlab.com", "code", 1.5),
    ("pypi.org", "code", 1.5),
    ("www.npmjs.com", "code", 1.5),
    ("docs.rs", "docs", 3.0),
    ("html.spec.whatwg.org", "spec", 2.0),
    ("drafts.csswg.org", "spec", 2.0),
    ("www.w3.org", "spec", 1.5),
    ("infra.spec.whatwg.org", "spec", 1.0),
    ("wiki.archlinux.org", "wiki", 1.5),
    ("man7.org", "docs", 1.2),
    ("sqlite.org", "docs", 1.5),
    ("www.postgresql.org", "docs", 1.2),
    ("go.dev", "docs", 1.2),
    ("clang.llvm.org", "docs", 1.0),
    ("chromium.googlesource.com", "code", 1.5),
    ("bugs.chromium.org", "code", 1.2),
    ("fonts.google.com", "ref", 1.0),
    ("typo.social", "social", 1.5),
    ("tildes.net", "forum", 1.2),
    ("www.baldurbjarnason.com", "blog", 1.2),
    ("rachelbythebay.com", "blog", 1.5),
    ("danluu.com", "blog", 1.5),
    ("jvns.ca", "blog", 1.8),
    ("fasterthanli.me", "blog", 1.5),
    ("matklad.github.io", "blog", 1.5),
    ("without.boats", "blog", 1.0),
    ("nullprogram.com", "blog", 1.0),
    ("eev.ee", "blog", 0.8),
    ("ciechanow.ski", "blog", 1.0),
    ("www.joshwcomeau.com", "blog", 1.2),
    ("piccalil.li", "blog", 1.0),
    ("adactio.com", "blog", 1.0),
    ("daverupert.com", "blog", 0.8),
    ("set.gd", "ref", 0.6),
    ("excalidraw.com", "tool", 1.2),
    ("regex101.com", "tool", 1.0),
    ("jsonformatter.org", "tool", 0.6),
    ("www.diffchecker.com", "tool", 0.6),
    ("archive.org", "ref", 1.0),
]

TLDS = [".com", ".dev", ".io", ".net", ".org", ".blog", ".page", ".sh", ".me", ".xyz"]


def build_domains(rng):
    domains = list(NAMED)
    seen = {d for d, _, _ in domains}
    while len(domains) < TARGET_DOMAINS:
        style = rng.random()
        if style < 0.45:
            host = rng.choice(ADJS) + rng.choice(NOUNS) + rng.choice(TLDS)
        elif style < 0.7:
            host = "%s-%s%s" % (rng.choice(NOUNS), rng.choice(NOUNS), rng.choice(TLDS))
        elif style < 0.85:
            host = "blog.%s%s" % (rng.choice(ORGS), rng.choice(TLDS))
        else:
            host = "%s.github.io" % rng.choice(ORGS)
        if host in seen:
            continue
        seen.add(host)
        kind = rng.choice(["blog", "blog", "blog", "docs", "ref", "tool"])
        domains.append((host, kind, round(rng.uniform(0.16, 0.7), 2)))
    return domains


# --- pages -------------------------------------------------------------------


def slug(rng, words=3):
    return "-".join(rng.sample(ADJS + NOUNS + VERBS, words))


def title_case(s):
    return s[0].upper() + s[1:]


def make_page(rng, host, kind, n):
    """Invent one plausible public page on `host`."""
    topic = rng.choice(TOPICS)
    noun = rng.choice(NOUNS)
    adj = rng.choice(ADJS)
    if kind == "code" and host in ("crates.io", "pypi.org", "www.npmjs.com"):
        repo = rng.choice(REPOS)
        prefix = {"crates.io": "/crates/", "pypi.org": "/project/",
                  "www.npmjs.com": "/package/"}[host]
        path = "%s%s/%d.%d.%d" % (prefix, repo, rng.randint(0, 4),
                                  rng.randint(0, 30), rng.randint(0, 12))
        title = "%s — %s %s" % (repo, site_name(host),
                                rng.choice(["crate", "package", "release"]))
    elif kind == "code":
        org, repo = rng.choice(ORGS), rng.choice(REPOS)
        roll = rng.random()
        number = rng.randint(12, 4800)
        if roll < 0.35:
            path = "/%s/%s/issues/%d" % (org, repo, number)
            title = "%s: %s %s · Issue #%d · %s/%s" % (
                title_case(rng.choice(VERBS)), adj, noun, number, org, repo)
        elif roll < 0.6:
            path = "/%s/%s/pull/%d" % (org, repo, number)
            title = "%s the %s %s · Pull Request #%d · %s/%s" % (
                title_case(rng.choice(VERBS)), adj, noun, number, org, repo)
        elif roll < 0.8:
            path = "/%s/%s/blob/main/src/%s.rs" % (org, repo, noun)
            title = "%s/%s/src/%s.rs at main · %s/%s" % (org, repo, noun, org, repo)
        else:
            path = "/%s/%s" % (org, repo)
            title = "%s/%s: %s %s for %s" % (org, repo, title_case(adj), noun, topic)
    elif kind == "docs":
        seg = rng.choice(["guide", "reference", "book", "std", "api", "manual"])
        leaf = rng.choice(NOUNS)
        path = "/%s/%s/%s.html" % (seg, noun, leaf)
        title = "%s::%s — %s %s" % (
            noun, leaf, site_name(host),
            rng.choice(["documentation", "reference", "guide", "manual"]))
    elif kind == "wiki":
        thing = rng.choice(THINGS)
        path = "/wiki/%s" % thing
        title = "%s - %s" % (thing.replace("_", " ").replace("%27", "'"),
                             "Wikipedia" if "wikipedia" in host else site_name(host))
    elif kind == "forum":
        if host == "news.ycombinator.com":
            path = "/item?id=%d" % rng.randint(38000000, 49999999)
        else:
            path = "/s/%s/%s" % (
                "".join(rng.choice("abcdefghijklmnopqrstuvwxyz0123456789") for _ in range(6)),
                slug(rng, 3))
        title = "%s %s (%s) | %s" % (
            title_case(adj), noun, topic, site_name(host))
    elif kind == "qa":
        path = "/questions/%d/%s" % (rng.randint(1200000, 79000000), slug(rng, 5))
        title = "%s — how do I %s a %s %s?" % (
            rng.choice(["rust", "python", "css", "javascript", "sql", "bash"]),
            rng.choice(VERBS)[:-3] if rng.choice(VERBS).endswith("ing") else "fix",
            adj, noun)
    elif kind == "paper":
        path = "/abs/26%02d.%05d" % (rng.randint(1, 9), rng.randint(1000, 99999))
        title = "%s %s: %s for %s" % (
            title_case(adj), title_case(noun), title_case(rng.choice(VERBS)), topic)
    elif kind == "news":
        d = LAST_CAPTURE - timedelta(days=rng.randint(1, 420))
        path = "/%s/%s/%02d/%s" % (
            rng.choice(["technology", "science", "world", "culture", "environment"]),
            d.strftime("%Y/%b").lower(), d.day, slug(rng, 4))
        title = "%s %s %s — and what it means for %s" % (
            title_case(adj), noun, rng.choice(["ruling", "report", "outage", "study", "launch"]),
            topic)
    elif kind == "spec":
        path = "/#%s-%s" % (noun, rng.choice(NOUNS))
        title = "%s Standard — §%d.%d %s" % (
            host.split(".")[0].upper(), rng.randint(2, 14), rng.randint(1, 9), title_case(noun))
    elif kind == "video":
        path = "/watch?v=%s" % "".join(
            rng.choice("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-")
            for _ in range(11))
        title = "%s %s in %d minutes - YouTube" % (
            title_case(rng.choice(VERBS)), topic, rng.choice([7, 12, 18, 24, 43]))
    elif kind == "tool":
        path = "/%s" % "".join(rng.choice("abcdefghijkmnpqrstuvwxyz23456789") for _ in range(10))
        title = "%s — %s" % (title_case(noun), site_name(host))
    elif kind == "social":
        path = "/@%s/%d" % (rng.choice(ORGS), rng.randint(10**16, 10**17 - 1))
        title = "%s: “%s %s, again.”" % (rng.choice(ORGS), title_case(adj), noun)
    else:  # blog, ref
        year = rng.choice([2023, 2024, 2025, 2025, 2026, 2026])
        path = "/%d/%02d/%s/" % (year, rng.randint(1, 12), slug(rng, 3))
        title = "%s %s: %s" % (title_case(adj), noun, topic)
    frag = ""
    if rng.random() < 0.06:
        frag = "#" + rng.choice(NOUNS)
    return "https://%s%s%s" % (host, path, frag), title


# --- simulation --------------------------------------------------------------


def main(argv):
    rng = random.Random(SEED)
    domains = build_domains(rng)
    hosts = [d[0] for d in domains]
    kinds = {d[0]: d[1] for d in domains}
    weights = [d[2] for d in domains]

    # Snapshot timeline: roughly every 4-5 days over ~6 months, newest last here.
    times = []
    t = LAST_CAPTURE
    for _ in range(SNAPSHOT_COUNT):
        times.append(t)
        t -= timedelta(days=rng.randint(3, 7), hours=rng.randint(0, 20),
                       minutes=rng.randint(0, 59))
    times.reverse()

    pages = {}           # url -> page record
    order = []           # urls in first-seen order
    open_tabs = {}       # url -> {"window": n, "pos": n}
    sightings = {}       # url -> list of (snapshot_index, window, pos, tab_id)
    snapshots = []

    def new_page():
        while True:
            host = rng.choices(hosts, weights=weights, k=1)[0]
            url, title = make_page(rng, host, kinds[host], len(pages))
            if url not in pages:
                return url, title, host

    for i, when in enumerate(times):
        windows = min(12, max(3, int(rng.gauss(7, 2.2))))
        # 1. survival: what stays open from last time
        survivors = {}
        for url, place in open_tabs.items():
            if rng.random() < pages[url]["stick"]:
                survivors[url] = dict(place)
                if rng.random() < 0.08:                 # tabs get dragged between windows
                    survivors[url]["window"] = rng.randint(1, windows)
        # 2. arrivals
        want = int(rng.triangular(30, 90, 52))
        if i == 0:
            want = 190
        for n in range(want):
            url, title, host = new_page()
            # Stickiness decides the shape of the whole library: a small head of
            # permanent residents, a broad middle of things that linger for weeks,
            # and a long tail of pages opened once and closed the same day.
            r = rng.random()
            if i == 0 and n < RESIDENTS:
                stick = 1.0                             # never closed, ever
            elif r < 0.03:
                stick = rng.uniform(0.94, 0.995)
            elif r < 0.22:
                stick = rng.uniform(0.82, 0.94)
            elif r < 0.54:
                stick = rng.uniform(0.50, 0.80)
            else:
                stick = rng.uniform(0.02, 0.35)         # read once, closed
            pages[url] = {"url": url, "title": title, "domain": host, "stick": stick}
            order.append(url)
            survivors[url] = {"window": rng.randint(1, windows),
                              "pos": 999 + rng.random()}
        # 3. cap a snapshot at a believable size, dropping the least sticky first
        cap = min(320, max(90, int(rng.gauss(250, 45))))
        if len(survivors) > cap:
            ranked = sorted(survivors, key=lambda u: (pages[u]["stick"], rng.random()))
            for url in ranked[: len(survivors) - cap]:
                del survivors[url]
        # 4. lay out windows, assign tab ids
        by_window = {}
        for url, place in survivors.items():
            by_window.setdefault(place["window"], []).append(url)
        tab_seq = i * 100000 + 1000
        used_windows = sorted(by_window)
        for w in used_windows:
            urls = sorted(by_window[w], key=lambda u: survivors[u]["pos"])
            for pos, url in enumerate(urls):
                survivors[url]["pos"] = pos
                tab_seq += rng.randint(1, 4)
                sightings.setdefault(url, []).append((i, w, pos, tab_seq))
        pinned_here = set(rng.sample(list(survivors), min(len(survivors), rng.randint(2, 7))))
        for url in pinned_here:
            pages[url].setdefault("pinned_in", set()).add(i)
        open_tabs = survivors

        sid = when.strftime("%Y-%m-%d-%H%M%SZ")
        digest = hashlib.sha256(sid.encode()).hexdigest()
        snapshots.append({
            "id": sid,
            "captured_at": when.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "browser": "chrome",
            "profile": "Default",
            "source_bytes": rng.randint(741 * 1024, 4300 * 1024),
            "sha256": digest,
            "stats": {
                "tabs": len(survivors),
                "windows": len(used_windows),
                "commands": len(survivors) * rng.randint(9, 16),
                "unknown_commands": rng.choice([0, 0, 0, 0, 1, 2, 5, 11]),
                "truncated_bytes": rng.choice([0, 0, 0, 0, 0, 0, 37, 212]),
            },
        })

    # Newest first everywhere the UI reads it.
    snapshots.reverse()
    last = SNAPSHOT_COUNT - 1

    # Only pages that actually made it into a snapshot can be forgotten; a page
    # the cap evicted before it was ever written has no row to hide.
    forget_pool = [u for u in order if 0 < len(sightings.get(u, ())) <= 3]
    forgotten = set(rng.sample(forget_pool, int(len(forget_pool) * FORGOTTEN_SHARE)))

    out_pages = []
    for url in order:
        seen = sightings.get(url)
        if not seen:
            continue
        p = pages[url]
        pin = p.get("pinned_in", set())
        out = {
            "url": url,
            "title": p["title"],
            "domain": p["domain"],
            "sightings": [
                {
                    "snapshot": last - i,          # index into snapshots[], newest first
                    "window": w,
                    "position": pos,
                    "tab": tab,
                }
                for (i, w, pos, tab) in reversed(seen)
            ],
        }
        for s, (i, _w, _p, _t) in zip(out["sightings"], reversed(seen)):
            if i in pin:
                s["pinned"] = True
        if url in forgotten:
            out["forgotten"] = True
        out_pages.append(out)

    # Newest-seen first is the natural order for the backend to emit.
    out_pages.sort(key=lambda p: (p["sightings"][0]["snapshot"], -len(p["sightings"])))

    doc = {
        "schema_version": 1,
        "generated_at": LAST_CAPTURE.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "root": "~/.knowmoretabs",
        "counts": {
            "pages": len(out_pages),
            "forgotten": sum(1 for p in out_pages if p.get("forgotten")),
            "snapshots": len(snapshots),
            "domains": len({p["domain"] for p in out_pages}),
            "sightings": sum(len(p["sightings"]) for p in out_pages),
        },
        "snapshots": snapshots,
        "pages": out_pages,
    }

    blob = json.dumps(doc, separators=(",", ":"), ensure_ascii=False)
    OUT.write_text(blob + "\n", encoding="utf-8")

    if "--no-inject" not in argv:
        html = INDEX.read_text(encoding="utf-8")
        new, n = re.subn(
            r'(<script id="library" type="application/json">).*?(</script>)',
            lambda m: m.group(1) + blob + m.group(2),
            html, count=1, flags=re.S)
        if not n:
            print("warning: no <script id=\"library\"> element in index.html", file=sys.stderr)
        else:
            INDEX.write_text(new, encoding="utf-8")

    c = doc["counts"]
    once = sum(1 for p in out_pages if len(p["sightings"]) == 1)
    always = sum(1 for p in out_pages if len(p["sightings"]) == SNAPSHOT_COUNT)
    still_open = sum(1 for p in out_pages if p["sightings"][0]["snapshot"] == 0)
    print("pages %d  snapshots %d  domains %d  sightings %d" %
          (c["pages"], c["snapshots"], c["domains"], c["sightings"]))
    print("seen once %d (%.0f%%)  seen in every snapshot %d  still open %d  forgotten %d" %
          (once, 100 * once / c["pages"], always, still_open, c["forgotten"]))
    print("tabs per snapshot %d-%d   library.json %.1f KB" % (
        min(s["stats"]["tabs"] for s in snapshots),
        max(s["stats"]["tabs"] for s in snapshots),
        len(blob) / 1024))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
