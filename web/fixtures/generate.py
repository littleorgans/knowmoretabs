#!/usr/bin/env python3
"""Synthetic library fixture for the knowmoretabs frontend prototype.

slice: library
why:   The UI has to be designed against a realistic shape — a head of pages
       present in every snapshot, a long tail seen once, ~150 domains — and
       committed fixtures must never contain anyone's real browsing. Every
       URL and title here is invented. Deterministic: same seed, same file.

Writes fixtures/library.json and re-embeds the blob into ../index.html
between the two marker comments, so the page opens from file://. Also writes
fixtures/cases/*.json: the small documents the edge cases are checked against
(an empty archive, one snapshot, a degraded parse, the export shape).

    python3 generate.py            # ~2000 pages, ~41 snapshots, plus the cases
    python3 generate.py --seed 7   # a different but equally plausible world
"""
import argparse, json, random, re, sys
from urllib.parse import urlencode, urlsplit
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent

# --- vocabulary --------------------------------------------------------------

ORGS = ["rust-lang", "tokio-rs", "denoland", "tailwindlabs", "sveltejs", "vitejs",
        "astral-sh", "BurntSushi", "sharkdp", "junegunn", "neovim", "helix-editor",
        "zed-industries", "ghostty-org", "charmbracelet", "casey", "ogham", "dandavison",
        "microsoft", "google", "facebook", "apple", "cloudflare", "tailscale",
        "sqlite", "duckdb", "postgres", "redis", "nushell", "starship", "fish-shell"]
REPOS = ["ripgrep", "fd", "bat", "delta", "just", "zoxide", "eza", "fzf", "atuin",
         "serde", "clap", "tokio", "axum", "hyper", "reqwest", "rayon", "anyhow",
         "thiserror", "tracing", "crossterm", "ratatui", "notify", "walkdir",
         "vite", "esbuild", "uv", "ruff", "bun", "deno", "typescript", "zig",
         "lazygit", "gitui", "tig", "difftastic", "hyperfine", "tokei", "miniserve"]
TAGLINES = ["A fast, friendly alternative", "Blazingly fast and correct", "Zero-copy parsing",
            "Batteries included", "The missing piece", "Small, sharp, documented",
            "For humans first", "Cross-platform, no runtime", "Written in Rust"]
ISSUES = ["Panic on empty input", "Support NO_COLOR", "Windows paths with spaces",
          "Add --json output", "Wrong exit code on SIGPIPE", "Docs: clarify --follow",
          "Feature request: config file", "Slow startup with large history",
          "Unicode width miscalculated", "Broken on musl", "Respect XDG_CONFIG_HOME",
          "Colour output when piped", "Memory leak in watcher", "Flaky test on CI",
          "RFC: plugin system", "Deprecate --legacy flag", "Improve error messages"]
LANGS = ["python", "rust", "javascript", "typescript", "bash", "go", "sql", "css",
         "html", "regex", "git", "docker", "linux", "macos", "c", "swift"]
HOWS = ["How do I read a file line by line", "Why does this closure capture by move",
        "What is the difference between {} and {}", "How to parse a date without a library",
        "Best way to deduplicate a list while keeping order", "Why is my query slow after VACUUM",
        "How to escape single quotes", "How to make a container query fall back",
        "Is it safe to unwrap here", "How do I exit vim", "How to undo the last commit",
        "Why does content-visibility break scrollIntoView", "How to set a cookie with SameSite",
        "How to sort by multiple keys", "What does the -- argument mean"]
WIKI = ["Bloom filter", "Merkle tree", "Ship of Theseus", "Rosetta Stone", "Zettelkasten",
        "Voynich manuscript", "Antikythera mechanism", "Library of Alexandria",
        "Dewey Decimal Classification", "Commonplace book", "Palimpsest", "Incunable",
        "Kerning", "Widows and orphans (typesetting)", "Golden ratio", "Fibonacci sequence",
        "Bicameral mind", "Dunbar's number", "Hofstadter's law", "Parkinson's law",
        "Sourdough", "Maillard reaction", "Umami", "Kintsugi", "Wabi-sabi", "Ikigai",
        "Baader–Meinhof phenomenon", "Zeigarnik effect", "Pareto principle", "Long tail",
        "Tab (interface)", "Session (computer science)", "Write-ahead logging", "CRDT",
        "Hash table", "B-tree", "Skip list", "Bloom's taxonomy", "Peak–end rule",
        "Ostrich effect", "Endowment effect", "Sunk cost", "Loss aversion", "Cathedral",
        "Flying buttress", "Bauhaus", "Brutalist architecture", "Swiss Style", "Helvetica",
        "Garamond", "Caslon", "Baskerville", "Didone", "Blackletter", "Letterpress printing"]
HEADLINES = ["What the new {} rules mean for {}", "{} is quietly changing {}",
             "The long, strange history of {}", "Why {} still matters in 2026",
             "Inside the {} that runs {}", "{}: a field guide", "How {} lost the plot",
             "The case for {}", "Opinion: {} is not the problem", "Explainer: {} and {}",
             "Review: the {} that almost gets it right", "A year without {}"]
TOPICS = ["copyright", "batteries", "housing", "rail travel", "the four-day week", "typography",
          "open source", "tab hoarding", "the Atlantic cable", "sourdough", "mesh networks",
          "the humble spreadsheet", "libraries", "public broadcasting", "browser engines",
          "coffee", "flat-pack furniture", "sleep", "the metric system", "paper maps",
          "night trains", "birdsong", "cast iron", "the fax machine", "quiet quitting"]
VIDEO = ["{} in 100 seconds", "I tried {} for 30 days", "The {} problem nobody talks about",
         "Building a {} from scratch (full course)", "{} explained with a whiteboard",
         "Why I switched to {}", "{} tier list", "Restoring a 1970s {}", "{} — full album",
         "How {} actually works", "Live: {} Q&A", "{} for beginners, honest edition"]
VTOPIC = ["Rust", "Zig", "SQLite", "ripgrep", "tmux", "Neovim", "a mechanical keyboard",
          "letterpress", "espresso", "sourdough", "a static site", "Bloom filters",
          "the Antikythera mechanism", "a wooden plane", "fountain pens", "linocut",
          "a bicycle", "Dutch bikes", "night trains", "the Helvetica film"]
PAPERS = ["Attention is not all you need: a retrospective", "Sparse indexes for append-only logs",
          "Reading at scale: tab hoarding as external memory", "Compact sketches for set membership",
          "On the durability of local-first software", "Session restore under adversarial crashes",
          "A taxonomy of unread things", "Latency numbers every designer should know",
          "Merkle-CRDTs for offline collaboration", "Typography for dense interfaces",
          "The cost of a context switch, measured", "Incremental view maintenance in the browser"]
DOCS = {
    "developer.mozilla.org": ("/en-US/docs/Web/{a}/{b}", "{b} - {a} | MDN",
        ["CSS", "API", "HTML", "JavaScript"], ["content-visibility", "container queries", ":has()",
        "light-dark()", "ViewTransition", "IntersectionObserver", "AbortController", "Popover API",
        "dialog", "details", "grid-template-areas", "text-wrap", "scroll-snap-type", "mask-image",
        "Intl.DateTimeFormat", "structuredClone", "requestIdleCallback", "CSS nesting"]),
    "docs.python.org": ("/3/library/{b}.html", "{b} — Python 3.14 documentation", [""],
        ["pathlib", "json", "dataclasses", "itertools", "functools", "argparse", "sqlite3",
         "asyncio", "typing", "datetime", "struct", "zoneinfo", "http.server", "tomllib"]),
    "doc.rust-lang.org": ("/{a}/{b}", "{b} - {a}", ["book/ch", "std", "cargo/reference", "nomicon"],
        ["04-ownership", "08-collections", "13-functional", "19-advanced-features",
         "fs/struct.File.html", "io/trait.BufRead.html", "collections/struct.BTreeMap.html",
         "manifest.html", "profiles.html", "lifetimes.html", "atomics.html"]),
    "docs.rs": ("/{a}/latest/{a}/{b}", "{b} in {a} - Rust", REPOS[:20],
        ["index.html", "struct.Builder.html", "fn.parse.html", "trait.Read.html", "enum.Error.html"]),
    "web.dev": ("/articles/{b}", "{b} | web.dev", [""],
        ["content-visibility", "view-transitions", "inp", "css-nesting", "color-scheme",
         "focus-visible", "long-animation-frames", "font-best-practices", "prefers-reduced-motion"]),
    "developer.chrome.com": ("/docs/{a}/{b}", "{b} | {a} | Chrome for Developers", ["devtools", "css-ui", "web-platform"],
        ["performance", "memory-problems", "rendering", "css-nesting", "popover-api", "scroll-driven-animations"]),
    "learn.microsoft.com": ("/en-us/{a}/{b}", "{b} - {a} | Microsoft Learn", ["windows", "dotnet", "azure", "powershell"],
        ["file-paths", "naming-a-file", "dotnet-core-introduction", "async-await", "about_Execution_Policies"]),
    "developer.apple.com": ("/documentation/{a}/{b}", "{b} | Apple Developer Documentation", ["swift", "foundation", "appkit", "swiftui"],
        ["array", "filemanager", "nswindow", "list", "table", "focusstate", "urlsession"]),
    "kubernetes.io": ("/docs/{a}/{b}/", "{b} | Kubernetes", ["concepts", "tasks", "reference"],
        ["overview", "workloads", "configure-pod-container", "kubectl", "storage", "services-networking"]),
    "docs.docker.com": ("/{a}/{b}/", "{b} | Docker Docs", ["engine", "compose", "build"],
        ["install", "reference", "networking", "multi-stage", "cache", "secrets"]),
    "docs.aws.amazon.com": ("/{a}/latest/userguide/{b}.html", "{b} - {a}", ["s3", "ec2", "lambda", "iam"],
        ["Welcome", "UsingBucket", "instance-types", "gettingstarted", "id_credentials"]),
    "cloud.google.com": ("/{a}/docs/{b}", "{b} | {a} | Google Cloud", ["run", "storage", "sql"],
        ["quickstarts", "overview", "pricing", "iam", "backups"]),
    "postgresql.org": ("/docs/17/{b}.html", "PostgreSQL: Documentation: 17: {b}", [""],
        ["sql-vacuum", "indexes-types", "queries-with", "functions-json", "wal-intro", "sql-createindex"]),
    "sqlite.org": ("/{b}.html", "{b} - SQLite", [""],
        ["wal", "lang_upsert", "json1", "fts5", "optoverview", "atomiccommit", "whentouse", "lockingv3"]),
    "git-scm.com": ("/docs/{b}", "Git - {b} Documentation", [""],
        ["git-rebase", "git-worktree", "git-bisect", "git-reflog", "git-stash", "gitattributes", "git-log"]),
    "man7.org": ("/linux/man-pages/man{a}/{b}.{a}.html", "{b}({a}) - Linux manual page", ["1", "2", "3", "7"],
        ["rename", "fsync", "inotify", "epoll", "mmap", "signal", "xattr", "flock", "open"]),
    "pkg.go.dev": ("/{b}", "{b} package - {b} - Go Packages", [""],
        ["net/http", "encoding/json", "os", "io/fs", "context", "sync", "golang.org/x/text"]),
    "react.dev": ("/reference/react/{b}", "{b} – React", [""],
        ["useEffect", "useMemo", "useTransition", "Suspense", "memo", "startTransition"]),
    "svelte.dev": ("/docs/svelte/{b}", "{b} • Docs • Svelte", [""],
        ["$state", "$derived", "snippet", "transitions", "bind", "stores"]),
    "tailwindcss.com": ("/docs/{b}", "{b} - Core concepts - Tailwind CSS", [""],
        ["container-queries", "dark-mode", "responsive-design", "hover-focus-and-other-states"]),
    "caniuse.com": ("/{b}", "{b} | Can I use... Support tables", [""],
        ["css-has", "css-container-queries", "css-nesting", "view-transitions", "css-light-dark", "mdn-css_properties_content-visibility"]),
    "www.w3.org": ("/TR/{b}/", "{b}", [""],
        ["css-contain-2", "css-view-transitions-1", "css-color-5", "wai-aria-1.3", "css-anchor-position-1"]),
    "html.spec.whatwg.org": ("/multipage/{b}.html", "HTML Standard", [""],
        ["interactive-elements", "forms", "browsers", "dom", "scripting"]),
    "rfc-editor.org": ("/rfc/rfc{b}", "RFC {b}", [""],
        ["9110", "8259", "3339", "4180", "7230", "9111", "6455", "8446"]),
    "hexdocs.pm": ("/{a}/{b}.html", "{b} — {a}", ["elixir", "phoenix", "ecto"], ["Enum", "GenServer", "Ecto.Query", "LiveView"]),
    "kotlinlang.org": ("/docs/{b}.html", "{b} | Kotlin", [""], ["coroutines-overview", "flow", "sealed-classes", "null-safety"]),
    "ziglang.org": ("/documentation/master/{b}", "Zig Language Reference", [""], ["#comptime", "#Allocators", "#Errors", "#build-system"]),
    "nodejs.org": ("/api/{b}.html", "{b} | Node.js documentation", [""], ["fs", "path", "stream", "worker_threads", "test"]),
    "obsidian.md": ("/help/{b}", "{b} - Obsidian Help", [""], ["Properties", "Canvas", "Dataview", "Sync", "Bases"]),
}
NEWS = ["bbc.com", "theguardian.com", "nytimes.com", "reuters.com", "apnews.com", "economist.com",
        "ft.com", "arstechnica.com", "theverge.com", "wired.com", "techcrunch.com", "theregister.com",
        "lwn.net", "404media.co", "theatlantic.com", "newyorker.com", "lrb.co.uk", "aeon.co",
        "quantamagazine.org", "spiegel.de", "lemonde.fr", "nzz.ch"]
BLOGS = ["medium.com", "dev.to", "hashnode.dev", "blog.cloudflare.com", "netflixtechblog.com",
         "engineering.fb.com", "stripe.com", "vercel.com", "fly.io", "tailscale.com", "sqlite.org",
         "brooker.co.za", "jvns.ca", "danluu.com", "matklad.github.io", "fasterthanli.me",
         "blog.rust-lang.org", "simonwillison.net", "kottke.org", "daringfireball.net", "gwern.net",
         "overreacted.io", "joshwcomeau.com", "css-tricks.com", "smashingmagazine.com", "alistapart.com"]
SHOPS = ["amazon.com", "ebay.com", "etsy.com", "ikea.com", "thomann.de", "keychron.com", "jetpens.com"]
PRODUCTS = ["Mechanical keyboard, 75%, hot-swap", "Desk lamp, warm white, dimmable", "A5 dotted notebook, hardcover",
            "Fountain pen, fine nib", "Standing desk frame", "USB-C dock, 8-in-1", "Bookshelf, oak veneer",
            "Kettle, gooseneck, 0.9 L", "Wool blanket, herringbone", "Monitor arm, single, gas spring",
            "Cast iron skillet, 26 cm", "Bike pannier, waterproof", "Record player, belt drive",
            "Headphones, closed back", "Reading glasses, +1.5", "Label maker", "Index cards, 500"]
TOOLS = ["figma.com", "notion.so", "trello.com", "linear.app", "miro.com", "excalidraw.com",
         "docs.google.com", "drive.google.com", "calendar.google.com", "sheets.google.com",
         "slack.com", "zoom.us", "meet.google.com", "github.com", "gitlab.com", "codeberg.org"]
BOARDS = ["Q3 planning", "Roadmap draft", "Launch checklist", "Retro notes", "Design review",
          "Onboarding doc", "Interview loop", "Budget 2026", "Team offsite", "Reading list",
          "Untitled", "Copy of Untitled", "Weekly notes", "Backlog grooming", "Incident review"]
ACADEMIC = ["arxiv.org", "scholar.google.com", "jstor.org", "nature.com", "dl.acm.org",
            "semanticscholar.org", "openreview.net"]
TRAVEL = ["booking.com", "airbnb.com", "maps.google.com", "openstreetmap.org", "seat61.com",
          "en.wikivoyage.org", "bahn.de", "komoot.com"]
PLACES = ["Lisbon", "Porto", "Ljubljana", "Trieste", "Bergen", "Tromsø", "Edinburgh", "Bath",
          "Ghent", "Utrecht", "Leipzig", "Dresden", "Kraków", "Tallinn", "Riga", "Vilnius",
          "Bologna", "Turin", "Lyon", "Nantes", "Girona", "Valencia", "Kyoto", "Kanazawa"]
FOOD = ["seriouseats.com", "bonappetit.com", "bbcgoodfood.com", "smittenkitchen.com", "chefkoch.de"]
DISHES = ["Sourdough focaccia", "Weeknight dal", "Cacio e pepe", "Shakshuka", "Miso-glazed aubergine",
          "One-pan chicken thighs", "Overnight oats", "Roasted cauliflower soup", "Pad kra pao",
          "Lemon olive oil cake", "No-knead bread", "Ratatouille", "Bibimbap", "Kimchi fried rice",
          "Congee", "Tarte tatin", "Okonomiyaki", "Frittata", "Bean stew", "Gochujang noodles"]
MEDIA = ["youtube.com", "vimeo.com", "spotify.com", "bandcamp.com", "discogs.com", "imdb.com",
         "letterboxd.com", "goodreads.com", "archive.org", "openlibrary.org", "xkcd.com", "overcast.fm"]
SOCIAL = ["news.ycombinator.com", "lobste.rs", "reddit.com", "mastodon.social", "bsky.app",
          "x.com", "linkedin.com", "producthunt.com"]
QA = ["stackoverflow.com", "superuser.com", "unix.stackexchange.com", "askubuntu.com",
      "apple.stackexchange.com", "serverfault.com", "softwareengineering.stackexchange.com"]
MISC = ["yr.no", "wikihow.com", "1password.com", "zotero.org", "logseq.com", "hetzner.com",
        "railway.app", "localhost:3000", "127.0.0.1:8080"]


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


class World:
    def __init__(self, rng):
        self.rng = rng
        self.urls = set()

    def unique(self, make):
        for _ in range(12):
            url, title = make()
            if url not in self.urls:
                self.urls.add(url)
                return url, title
        return None     # this kind's vocabulary is thin; try another

    KINDS = ["code", "qa", "docs", "wiki", "news", "video", "paper", "shop", "tool",
             "travel", "food", "media", "social", "blog", "misc"]
    WEIGHTS = [14, 9, 16, 8, 10, 6, 4, 4, 7, 3, 3, 5, 6, 4, 1]

    def page(self):
        for _ in range(40):
            kind = self.rng.choices(self.KINDS, self.WEIGHTS)[0]
            got = self.unique(getattr(self, kind))
            if got:
                return got
        raise RuntimeError("vocabulary exhausted; add more words")

    def code(self):
        r = self.rng
        host = r.choices(["github.com", "gitlab.com", "codeberg.org"], [10, 1, 1])[0]
        org, repo = r.choice(ORGS), r.choice(REPOS)
        n = r.randint(1, 4000)
        shape = r.choices(["repo", "issue", "pr", "blob", "releases"], [4, 5, 3, 3, 1])[0]
        if shape == "repo":
            return f"https://{host}/{org}/{repo}", f"{host.split('.')[0].title()} - {org}/{repo}: {r.choice(TAGLINES)}"
        if shape == "issue":
            return f"https://{host}/{org}/{repo}/issues/{n}", f"{r.choice(ISSUES)} · Issue #{n} · {org}/{repo}"
        if shape == "pr":
            return f"https://{host}/{org}/{repo}/pull/{n}", f"{r.choice(ISSUES)} by {r.choice(ORGS).lower()} · Pull Request #{n} · {org}/{repo}"
        if shape == "blob":
            f = r.choice(["src/main.rs", "README.md", "Cargo.toml", "src/lib.rs", "CHANGELOG.md", "docs/design.md"])
            return f"https://{host}/{org}/{repo}/blob/main/{f}", f"{repo}/{f} at main · {org}/{repo}"
        return f"https://{host}/{org}/{repo}/releases", f"Releases · {org}/{repo}"

    def qa(self):
        r = self.rng
        host = r.choices(QA, [10, 2, 3, 2, 2, 1, 1])[0]
        q = r.choice(HOWS).format(r.choice(LANGS), r.choice(LANGS)) + "?"
        n = r.randint(100000, 79000000)
        site = {"stackoverflow.com": "Stack Overflow", "superuser.com": "Super User"}.get(
            host, host.split(".")[0].replace("softwareengineering", "Software Engineering").title() + " Stack Exchange")
        return f"https://{host}/questions/{n}/{slug(q)[:60]}", f"{r.choice(LANGS)} - {q} - {site}"

    def docs(self):
        r = self.rng
        host = r.choice(list(DOCS))
        path, title, aa, bb = DOCS[host]
        a, b = r.choice(aa), r.choice(bb)
        return f"https://{host}{path.format(a=a, b=b)}", title.format(a=a, b=b)

    def wiki(self):
        r = self.rng
        t = r.choice(WIKI)
        host = r.choices(["en.wikipedia.org", "de.wikipedia.org", "en.wiktionary.org"], [12, 2, 1])[0]
        suffix = {"en.wikipedia.org": "Wikipedia", "de.wikipedia.org": "Wikipedia", "en.wiktionary.org": "Wiktionary, the free dictionary"}[host]
        return f"https://{host}/wiki/{t.replace(' ', '_')}", f"{t} - {suffix}"

    def news(self):
        r = self.rng
        host = r.choice(NEWS)
        h = r.choice(HEADLINES).format(r.choice(TOPICS), r.choice(TOPICS))
        h = h[0].upper() + h[1:]
        y, m, d = 2026, r.randint(1, 9), r.randint(1, 28)
        section = r.choice(["technology", "world", "culture", "science", "business", "opinion"])
        return f"https://{host}/{section}/{y}/{m:02d}/{d:02d}/{slug(h)}", f"{h} | {host.split('.')[0].title()}"

    def video(self):
        r = self.rng
        vid = "".join(r.choices("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_", k=11))
        t = r.choice(VIDEO).format(r.choice(VTOPIC))
        return f"https://www.youtube.com/watch?v={vid}", f"{t} - YouTube"

    def paper(self):
        r = self.rng
        host = r.choices(ACADEMIC, [8, 2, 1, 1, 1, 1, 1])[0]
        t = r.choice(PAPERS)
        if host == "arxiv.org":
            n = f"{r.randint(2301, 2609)}.{r.randint(0, 29999):05d}"
            return f"https://arxiv.org/abs/{n}", f"[{n}] {t}"
        return f"https://{host}/{r.choice(['doi', 'article', 'paper', 'abstract'])}/10.{r.randint(1000, 9999)}/{slug(t)[:30]}", f"{t} | {host.split('.')[-2].title()}"

    def shop(self):
        r = self.rng
        host = r.choice(SHOPS)
        p = r.choice(PRODUCTS)
        code = "".join(r.choices("ABCDEFGHJKLMNPQRSTUVWXYZ0123456789", k=10))
        return f"https://www.{host}/dp/{code}" if host == "amazon.com" else f"https://www.{host}/p/{slug(p)}-{r.randint(1000, 99999)}", f"{p} : {host.split('.')[0].title()}"

    def tool(self):
        r = self.rng
        host = r.choice(TOOLS[:13])
        b = r.choice(BOARDS)
        tok = "".join(r.choices("abcdefghijklmnopqrstuvwxyz0123456789", k=r.choice([12, 20, 32])))
        return f"https://{host}/{r.choice(['d', 'b', 'document', 'board', 'file', 'issue'])}/{tok}", f"{b} – {host.split('.')[0].title()}"

    def travel(self):
        r = self.rng
        host = r.choice(TRAVEL)
        p = r.choice(PLACES)
        if host == "seat61.com":
            return f"https://www.seat61.com/{p}.htm", f"Trains to {p} | The Man in Seat 61"
        if host in ("maps.google.com", "openstreetmap.org"):
            return f"https://www.{host}/search/{slug(p)}/@{r.uniform(40, 60):.4f},{r.uniform(-9, 25):.4f},13z", f"{p} - {host.split('.')[0].title()}"
        return f"https://www.{host}/{r.choice(['city', 'hotel', 'rooms', 'stays'])}/{slug(p)}-{r.randint(100, 9999)}", f"{r.choice(['Stays in', 'Weekend in', 'Getting to', 'Hikes near'])} {p} · {host.split('.')[0].title()}"

    def food(self):
        r = self.rng
        host = r.choice(FOOD)
        d = r.choice(DISHES)
        return f"https://www.{host}/recipes/{slug(d)}-{r.randint(1000, 99999)}", f"{d} Recipe | {host.split('.')[0].title()}"

    def media(self):
        r = self.rng
        host = r.choice(MEDIA[2:])
        t = r.choice(VTOPIC + WIKI[:10] + PAPERS[:4])
        if host == "xkcd.com":
            n = r.randint(300, 3100)
            return f"https://xkcd.com/{n}/", f"xkcd: {r.choice(['Tabs', 'Backlog', 'Dependency', 'Standards', 'Nerd Sniping', 'Bookmarks', 'Estimation', 'Undo'])}"
        if host == "archive.org":
            return f"https://archive.org/details/{slug(t)}_{r.randint(1900, 1990)}", f"{t} : Free Download, Borrow, and Streaming : Internet Archive"
        return f"https://www.{host}/{r.choice(['title', 'album', 'artist', 'book', 'film', 'show'])}/{slug(t)}-{r.randint(10, 9999)}", f"{t} — {host.split('.')[0].title()}"

    def social(self):
        r = self.rng
        host = r.choices(SOCIAL, [10, 3, 6, 2, 3, 2, 1, 1])[0]
        t = r.choice(HEADLINES).format(r.choice(TOPICS + VTOPIC), r.choice(TOPICS))
        t = t[0].upper() + t[1:]
        if host == "news.ycombinator.com":
            return f"https://news.ycombinator.com/item?id={r.randint(38000000, 45999999)}", f"{t} | Hacker News"
        if host == "reddit.com":
            sub = r.choice(["rust", "programming", "typography", "sourdough", "bikecommuting", "selfhosted", "neovim", "commandline"])
            return f"https://www.reddit.com/r/{sub}/comments/{''.join(r.choices('abcdefghijklmnopqrstuvwxyz0123456789', k=7))}/{slug(t)[:50]}/", f"{t} : r/{sub}"
        return f"https://{host}/{r.choice(['s', 'post', 'in', 'p'])}/{''.join(r.choices('abcdefghijklmnopqrstuvwxyz0123456789', k=9))}", f"{t} — {host.split('.')[-2].title()}"

    def blog(self):
        r = self.rng
        host = r.choice(BLOGS)
        t = r.choice(HEADLINES).format(r.choice(TOPICS + VTOPIC), r.choice(TOPICS))
        t = t[0].upper() + t[1:]
        return f"https://{host}/{r.choice(['blog', 'posts', 'writing', '2026', 'articles'])}/{slug(t)}", f"{t}"

    def misc(self):
        r = self.rng
        host = r.choice(MISC)
        if host.startswith(("localhost", "127.")):
            return f"http://{host}/{r.choice(['', 'docs', 'admin', 'preview/42'])}", r.choice(["Vite App", "Dev server", "Storybook", "knowmoretabs"])
        return f"https://www.{host}/{r.choice(['help', 'about', 'pricing', 'docs', 'faq'])}/{r.randint(1, 60)}", f"{r.choice(['Help', 'Pricing', 'FAQ', 'Getting started'])} · {host.split('.')[0].title()}"


GROUP_NAMES = [("Rust", "orange"), ("Reading", "blue"), ("Trip", "green"), ("Work", "grey"),
               ("Kitchen", "yellow"), ("Later", "purple"), ("Keyboard", "pink"), ("Papers", "cyan"),
               ("", "red")]        # Chrome allows a group with no name; only its colour shows

# Pages real archives have and invented vocabularies do not. Every one is still
# invented, but the shapes are the ones that break layouts: no title, a title
# and a URL far longer than a row, non-Latin and right-to-left scripts, and
# URLs with no host at all. The domain is computed the way the backend does it.
EDGE_PAGES = [
    ("https://example.com/notes/2026/untitled-draft", ""),
    ("https://docs.example.org/reference/" + "very-long-path-segment-" * 40 + "index.html?" + "&".join(f"param{i}=value{i}" for i in range(60)),
     "A title that runs on far longer than any row could show, " * 6 + "and then keeps going for good measure so that ellipsis and wrapping are both exercised"),
    ("https://ja.wikipedia.org/wiki/日本語の記事", "日本語の記事 - Wikipedia"),
    ("https://de.wikipedia.org/wiki/Straßenbahn_Zürich", "Straßenbahn Zürich – Wikipedia"),
    ("https://ar.wikipedia.org/wiki/اللغة_العربية", "اللغة العربية - ويكيبيديا، الموسوعة الحرة"),
    ("https://he.wikipedia.org/wiki/עברית", "עברית – ויקיפדיה"),
    ("https://example.net/emoji", "🦀 Rust for Rustaceans 🦀 · 中文 · Ελληνικά"),
    ("data:text/html;base64,PGh0bWw+PGJvZHk+PGgxPkhlbGxvPC9oMT48L2JvZHk+PC9odG1sPg==", "Hello"),
    ("chrome://settings/", "Settings"),
    ("chrome://version/", "About Version"),
]


# Tags (slice 7a): a flat vocabulary the owner made, forty facets, most of
# them about the owner's work and a few about the rest of their life. Facets
# travel in themes, which is what gives the tag bar's co-occurrence counts a
# shape: pick Agent and Harness, Skills and MCP rise with it. Most pages carry
# none; the tagged ones mostly carry one to four, with a long tail.
TAG_THEMES = {
    "agents": ["Agent", "Harness", "MCP", "Skills", "Orchestration", "Guardrails", "Tool Calling",
               "Context", "Memory", "Worktrees", "Code Review", "Evals"],
    "models": ["Model", "Provider", "Inference", "Training", "Evals", "Local", "Voice", "Research", "Hosting"],
    "eng":    ["Backend", "Frontend", "DB", "Cloud", "DevOps", "Security", "Code Review", "Rust", "CLI",
               "Testing", "Hosting", "Worktrees"],
    "design": ["Design", "Inspiration", "Typography", "Frontend", "Keyboards"],
    "life":   ["Reading", "Travel", "Recipes", "Shopping", "Music", "Video", "Keyboards", "Later", "Inspiration"],
}
TAG_KINDS = {   # which themes a kind of page is tagged from, by its host
    "code": ["eng", "eng", "agents", "models"], "qa": ["eng"], "docs": ["eng", "agents", "models"],
    "paper": ["models", "agents"], "blog": ["eng", "design", "agents", "models"], "tool": ["design", "eng"],
    "video": ["life", "agents", "design"], "social": ["agents", "models", "life"], "wiki": ["life", "models"],
    "news": ["life"], "shop": ["life"], "travel": ["life"], "food": ["life"], "media": ["life"], "misc": ["life", "eng"],
}
TAG_UNUSED = ["Prompt"]         # made once, never kept on a page: a 0-count tag


def kind_of(domain):
    """A rough reverse of World's kinds, from the host alone."""
    for kind, hosts in (("code", ["github.com", "gitlab.com", "codeberg.org"]), ("qa", QA), ("paper", ACADEMIC),
                        ("blog", BLOGS), ("tool", TOOLS), ("news", NEWS), ("shop", SHOPS), ("travel", TRAVEL),
                        ("food", FOOD), ("media", MEDIA), ("social", SOCIAL)):
        if domain in hosts:
            return kind
    if domain in ("youtube.com", "vimeo.com"):
        return "video"
    if domain.endswith("wikipedia.org"):
        return "wiki"
    return "docs" if domain in {d for d in DOCS} else "misc"


def add_tags(lib, seed):
    """Page tags and the vocabulary, in the serve/export shape. Its own RNG, so
    the rest of the fixture does not move when this does."""
    r = random.Random(seed * 7 + 1)
    vocab = sorted({t for ts in TAG_THEMES.values() for t in ts}) + TAG_UNUSED
    used = set()
    for i, p in enumerate(lib["pages"]):
        k = r.choices([0, 1, 2, 3, 4, 5, 6, 8, 11], [44, 14, 14, 12, 8, 4, 2, 1, 1])[0]
        themes = TAG_KINDS[kind_of(p["domain"])]
        tags = set()
        main = r.choice(themes)
        while len(tags) < k:
            pool = TAG_THEMES[main if r.random() < 0.8 else r.choice(list(TAG_THEMES))]
            tags.add(pool[min(int(r.expovariate(0.35)), len(pool) - 1)])   # the head of a theme is used most
        p["tags"] = sorted(tags, key=str.lower)
        used |= tags
    start = datetime(2026, 3, 14, tzinfo=timezone.utc)
    lib["vocabulary"] = [{"name": n, "created_at": (start + timedelta(days=j * 4, hours=j % 9)).strftime("%Y-%m-%dT%H:%M:%SZ")}
                         for j, n in enumerate(t for t in vocab if t in used or t in TAG_UNUSED)]
    lib["vocabulary"].sort(key=lambda v: v["name"].lower())
    return lib


def add_suggested(lib, seed):
    """Suggested tags (7c): always present, like `tags`, and on a few pages a
    name the owner has neither added nor removed, with the sources that made
    it. Page 0 has one, since the contract test reads its shape from page 0.
    Its own RNG, so the rest of the fixture does not move."""
    r = random.Random(seed * 13 + 5)
    names = [v["name"] for v in lib["vocabulary"]]
    for i, p in enumerate(lib["pages"]):
        p["suggested"] = []
        if i and r.random() > 0.05:
            continue
        free = [n for n in names if n not in p["tags"]]
        for n in r.sample(free, min(len(free), r.randint(1, 2))):
            p["suggested"].append({"name": n, "sources": r.sample(["claude-opus-5", "gpt-6-luna"], r.randint(1, 2))})
    return lib


STAMP = "%Y-%m-%dT%H:%M:%SZ"
ELSEWHERE = ["https://news.ycombinator.com/item?id={n}", "https://t.co/{h}", "https://www.reddit.com/r/programming/comments/{h}/",
             "https://lobste.rs/s/{h}", "https://mastodon.social/@someone/{n}", "https://duckduckgo.com/"]


def add_history(lib, seed, recorded=3):
    """History signals, in the serve shape, for pages seen in the last `recorded`
    snapshots: what an archive looks like once `save` started reading History.
    Its own RNG, so the rest of the fixture does not move. Page 0 is the
    contract test's exemplar and every exported page must have its fields, so
    it gets none."""
    r = random.Random(seed * 11 + 3)
    snaps = lib["snapshots"]
    last = {}
    for k, s in enumerate(snaps):
        for t in s["tabs"]:
            last[t[0]] = k
    pages = lib["pages"]
    titled = [p for p in pages if p["title"] and p["url"].startswith("https://")]
    for i, p in enumerate(pages):
        k = last.get(i)
        if i == 0 or k is None or k < len(snaps) - recorded or r.random() < 0.08:
            continue
        seen = datetime.strptime(snaps[k]["captured_at"], STAMP).replace(tzinfo=timezone.utc)
        visits = 1 + int(r.expovariate(0.12)) if r.random() < 0.9 else r.randint(200, 1500)
        lastv = seen - timedelta(minutes=r.randint(1, 60 * 30))
        h = {"visits": visits, "typed": 0 if r.random() < 0.86 else r.randint(1, max(1, min(visits, 30))),
             "first_visit": (lastv - timedelta(days=r.uniform(0, 88)) if visits > 1 else lastv).strftime(STAMP),
             "last_visit": lastv.strftime(STAMP)}
        if r.random() < 0.6:
            h["foreground_seconds"] = min(700000, int(r.lognormvariate(4.5, 2.4)))
        words = [w for w in re.findall(r"[a-z0-9]+", p["title"].lower()) if len(w) > 2]
        roll = r.random()
        if words and roll < 0.22:                       # from a results page: the search and the referrer are one
            term = " ".join(r.sample(words, min(len(words), r.randint(2, 4))))
            h["search"] = {"term": term, "hops": 1}
            h["referrer"] = {"url": "https://www.google.com/search?" + urlencode({"q": term})}
        elif roll < 0.55 and titled:                    # from another page in the library
            q = r.choice(titled)
            if q is not p:
                h["referrer"] = {"url": q["url"], "title": q["title"]}
        elif roll < 0.85:                               # from somewhere the library never saw
            h["referrer"] = {"url": r.choice(ELSEWHERE).format(n=r.randint(10**6, 10**8), h=f"{r.getrandbits(40):010x}")}
        if words and "search" not in h and r.random() < 0.12:
            h["search"] = {"term": " ".join(r.sample(words, min(len(words), r.randint(1, 5)))), "hops": r.choice([0, 2, 2, 3])}
        p["history"] = h
    return lib


def domain_of(url):
    """What the backend's `public_domain` would say: host, lowercased, no www."""
    host = (urlsplit(url).hostname or "").lower()
    return host[4:] if host.startswith("www.") else host


def build(seed, snapshot_count=41, head_count=64, edges=True, forgotten_count=6, tags=True):
    r = random.Random(seed)
    world = World(r)
    pages = []          # {url,title,domain}
    meta = []           # per page: stickiness, home window, first_seen
    tab_ids = {}

    def new_page(kind, fixed=None):
        url, title = fixed or world.page()
        if fixed is None and r.random() < 0.012:
            title = ""      # Chrome sometimes writes no title
        pages.append({"url": url, "title": title, "domain": domain_of(url)})
        stick = {"head": 1.0, "sticky": 0.92, "medium": 0.6, "ephemeral": 0.12}[kind]
        meta.append({"stick": stick, "win": None})
        return len(pages) - 1

    head = [new_page("head") for _ in range(head_count)]

    # capture times: irregular, mostly mornings, some late nights
    t = datetime(2026, 3, 14, 9, 12, 2, tzinfo=timezone.utc)
    times = []
    for _ in range(snapshot_count):
        times.append(t)
        t += timedelta(days=r.choice([1, 1, 2, 2, 3, 4, 5, 7, 9]), hours=r.randint(-3, 9), minutes=r.randint(0, 59), seconds=r.randint(0, 59))

    snapshots = []
    open_set = set(head)
    dropped = []
    windows = 4
    win_of = {}
    for p in head:
        win_of[p] = r.randint(1, windows)
    # Tab groups live on a window and persist from one snapshot to the next, the
    # way Chrome's do: members stay until closed, newcomers to the window may
    # join, a group is occasionally dissolved or renamed, and a page sometimes
    # moves from one group to another. So the same page can be in "Papers" in
    # March, "Later" in June, and in no group today.
    live = {}           # window → {"name", "colour", "members": set}

    for k, when in enumerate(times):
        # windows drift slowly
        if r.random() < 0.25:
            windows = max(2, min(9, windows + r.choice([-1, 1])))
        # survivors from last time
        survivors = {p for p in open_set if r.random() < meta[p]["stick"]} | set(head)
        dropped.extend(open_set - survivors)
        # a few re-opened
        reopened = set()
        if dropped and r.random() < 0.8:
            for _ in range(r.randint(1, 6)):
                reopened.add(r.choice(dropped))
        # new arrivals; bigger bursts on some days
        burst = r.choice([22, 30, 36, 42, 48, 55, 62, 88])
        arrivals = []
        for _ in range(burst):
            kind = r.choices(["ephemeral", "medium", "sticky"], [58, 28, 14])[0]
            arrivals.append(new_page(kind))
        if edges and k == max(0, snapshot_count - 3):
            arrivals += [new_page("sticky", e) for e in EDGE_PAGES]
        open_set = survivors | reopened | set(arrivals)
        # assign windows to newcomers; a burst tends to land in one or two windows
        burst_win = r.randint(1, windows)
        for p in arrivals:
            win_of[p] = burst_win if r.random() < 0.7 else r.randint(1, windows)
        for p in reopened:
            win_of.setdefault(p, r.randint(1, windows))
        for p in open_set:
            if win_of[p] > windows:
                win_of[p] = r.randint(1, windows)
            if p not in tab_ids:
                tab_ids[p] = 100 + len(tab_ids) * r.choice([1, 2, 3])
        # groups: persist, recruit, dissolve, and let the odd page move
        for w in list(live):
            if w > windows or r.random() < 0.08:
                del live[w]
        for w in range(1, windows + 1):
            here = [p for p in open_set if win_of[p] == w]
            if w not in live:
                if r.random() < 0.18 and len(here) >= 4:
                    name, colour = r.choice(GROUP_NAMES)
                    live[w] = {"name": name, "colour": colour, "members": {p for p in here if r.random() < 0.35}}
                continue
            g = live[w]
            g["members"] = {p for p in g["members"] if p in open_set and win_of[p] == w}
            g["members"] |= {p for p in arrivals + list(reopened) if win_of[p] == w and r.random() < 0.4}
            if r.random() < 0.05:
                g["name"], g["colour"] = r.choice(GROUP_NAMES)
        if len(live) >= 2 and r.random() < 0.5:        # a page changes group
            a, b = r.sample(list(live), 2)
            movers = [p for p in live[a]["members"] if p not in head]
            if movers:
                p = r.choice(movers)
                live[a]["members"].discard(p); win_of[p] = b; live[b]["members"].add(p)
        groups, group_of = [], {}
        for w in sorted(live):
            g = live[w]
            if not g["members"]:
                continue
            gid = len(groups)
            # Collapsed is a property of the moment the snapshot was taken, not
            # of the group, so it comes from the position rather than from `r`.
            groups.append({"id": gid, "title": g["name"], "colour": g["colour"],
                           "collapsed": (k + gid) % 5 == 0})
            for p in g["members"]:
                group_of[p] = gid
        # lay out: per window, grouped tabs sit together (Chrome keeps a group
        # contiguous), ordered by first appearance within each run
        tabs = []
        for w in range(1, windows + 1):
            members = sorted((p for p in open_set if win_of[p] == w), key=lambda p: (p in group_of, tab_ids[p]))
            pos = 0
            for p in members:
                pinned = 1 if (p in head and w == 1 and pos < 4) else 0
                tabs.append([p, w, pos, tab_ids[p], pinned, group_of.get(p)])
                pos += 1
                if r.random() < 0.03:  # the same page open twice in one window
                    tabs.append([p, w, pos, tab_ids[p] + 5000 + pos, 0, group_of.get(p)])
                    pos += 1
        # The parser's counters, emitted for every snapshot. A real archive is
        # mostly clean with the occasional torn tail or command id this build
        # does not know, so two snapshots of a long run carry non-zero counts.
        torn = snapshot_count >= 8 and k == snapshot_count - 5
        strange = snapshot_count >= 8 and k == snapshot_count // 3
        snapshots.append({
            "id": when.strftime("%Y-%m-%d-%H%M%SZ"),
            "captured_at": when.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "browser": "chrome",
            "profile": "Default",
            "windows": windows,
            "tabs_total": len(tabs),
            "stats": {"dropped_tabs": 2 if torn else 0,
                      "unknown_commands": 3 if strange else 0,
                      "malformed_commands": 0,
                      "truncated_bytes": 1462 if torn else 0,
                      "marker_ok": not torn,
                      "degraded": torn or strange},
            "groups": groups,
            "tabs": tabs,
        })

    forgotten = set(r.sample(range(head_count, len(pages)), min(forgotten_count, max(0, len(pages) - head_count))))
    for i in forgotten:
        pages[i]["forgotten"] = True
    lib = {
        "schema_version": 1,
        "generated_at": times[-1].strftime("%Y-%m-%dT%H:%M:%SZ"),
        "stats": {"pages": len(pages), "snapshots": len(snapshots), "domains": len({p["domain"] for p in pages}),
                  "sightings": sum(len(s["tabs"]) for s in snapshots), "forgotten": len(forgotten)},
        "snapshots": snapshots,
        "pages": pages,
    }
    # a library from before 7a has no tags or suggestions, and one from before 7b no history
    return add_history(add_suggested(add_tags(lib, seed), seed), seed) if tags else lib


def to_export(lib):
    """The export shape: forgotten pages and their tab rows are omitted, the
    count survives in stats.forgotten, and page indices are renumbered."""
    keep = [i for i, p in enumerate(lib["pages"]) if not p.get("forgotten")]
    renum = {old: new for new, old in enumerate(keep)}
    out = json.loads(json.dumps(lib))
    out["pages"] = [{k: v for k, v in out["pages"][i].items() if k != "forgotten"} for i in keep]
    for p in out["pages"]:                              # without --with-history
        for k in ("search", "referrer"):
            p.get("history", {}).pop(k, None)
    for s in out["snapshots"]:
        s["tabs"] = [[renum[t[0]], *t[1:]] for t in s["tabs"] if t[0] in renum]
        s["tabs_total"] = len(s["tabs"])
    out["stats"].update(pages=len(out["pages"]), sightings=sum(len(s["tabs"]) for s in out["snapshots"]),
                        domains=len({p["domain"] for p in out["pages"]}))
    return out


def cases(seed):
    """Small documents for the edges a real archive has and the big fixture
    cannot show. library.json is the exemplar the Rust contract test checks the
    real export against, so every field used here is emitted there too."""
    stamp = "2026-09-21T16:01:52Z"
    clean = {"dropped_tabs": 0, "unknown_commands": 0, "malformed_commands": 0,
             "truncated_bytes": 0, "marker_ok": True, "degraded": False}
    empty = {"schema_version": 1, "generated_at": stamp,
             "stats": {"pages": 0, "snapshots": 0, "domains": 0, "sightings": 0, "forgotten": 0},
             "snapshots": [], "pages": []}
    no_pages = json.loads(json.dumps(empty))
    no_pages["stats"]["snapshots"] = 1
    no_pages["snapshots"] = [{"id": "2026-09-20-101500Z", "captured_at": "2026-09-20T10:15:00Z", "browser": "chrome",
                              "profile": "Default", "windows": 1, "tabs_total": 0, "stats": dict(clean),
                              "groups": [], "tabs": []}]
    one = build(seed + 1, snapshot_count=1, head_count=12, tags=False)   # before 7a: no tags, no vocabulary
    degraded = build(seed + 2, snapshot_count=5, head_count=12, forgotten_count=0)
    degraded["snapshots"][2]["stats"] = {"dropped_tabs": 3, "unknown_commands": 2, "malformed_commands": 0,
                                         "truncated_bytes": 1024, "marker_ok": False, "degraded": True}
    degraded["snapshots"][2]["tabs"] = degraded["snapshots"][2]["tabs"][:-3]
    degraded["snapshots"][2]["tabs_total"] -= 3
    degraded["snapshots"][4]["stats"] = {"dropped_tabs": 0, "unknown_commands": 0, "malformed_commands": 1,
                                         "truncated_bytes": 0, "marker_ok": True, "degraded": True}
    # Degraded with every named counter at zero: what a navigation fallback or
    # a group without metadata produces. Renders as the generic "parse degraded".
    degraded["snapshots"][1]["stats"] = dict(clean, degraded=True)
    exported = to_export(build(seed + 3, snapshot_count=8, head_count=12, forgotten_count=4))
    return {"empty": empty, "no-pages": no_pages, "one-snapshot": one, "degraded": degraded, "export-forgotten": exported}


def embed(html_path, blob):
    html = html_path.read_text(encoding="utf-8")
    start, end = "<!-- library-data:start -->", "<!-- library-data:end -->"
    a, b = html.index(start), html.index(end)
    safe = blob.replace("</", "<\\/")      # keeps </script> out of the JSON; still valid JSON
    html = html[: a + len(start)] + '\n<script id="library-data" type="application/json">' + safe + "</script>\n" + html[b:]
    html_path.write_text(html, encoding="utf-8")


def dump(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--seed", type=int, default=2026)
    ap.add_argument("--snapshots", type=int, default=41)
    ap.add_argument("--no-embed", action="store_true", help="only write library.json")
    args = ap.parse_args()
    lib = build(args.seed, args.snapshots)
    out = HERE / "library.json"
    out.write_text(dump(lib), encoding="utf-8")
    counts = {}
    for s in lib["snapshots"]:
        for i in {t[0] for t in s["tabs"]}:
            counts[i] = counts.get(i, 0) + 1
    once = sum(1 for c in counts.values() if c == 1)
    every = sum(1 for c in counts.values() if c == len(lib["snapshots"]))
    grouped = sum(1 for s in lib["snapshots"] for t in s["tabs"] if t[5] is not None)
    print(f"{out.name}: {out.stat().st_size:,} bytes · {lib['stats']['pages']} pages · "
          f"{lib['stats']['snapshots']} snapshots · {lib['stats']['domains']} domains · "
          f"{lib['stats']['sightings']} sightings · seen once {once} · in every snapshot {every} · "
          f"tabs/snapshot {min(s['tabs_total'] for s in lib['snapshots'])}–{max(s['tabs_total'] for s in lib['snapshots'])} · "
          f"groups {sum(len(s['groups']) for s in lib['snapshots'])} · grouped sightings {grouped}")
    cdir = HERE / "cases"
    cdir.mkdir(exist_ok=True)
    for name, doc in cases(args.seed).items():
        (cdir / f"{name}.json").write_text(dump(doc), encoding="utf-8")
    print(f"cases/: {', '.join(sorted(p.name for p in cdir.glob('*.json')))}")
    if not args.no_embed:
        html = HERE.parent / "index.html"
        if html.exists():
            embed(html, out.read_text(encoding="utf-8"))
            print(f"embedded into {html.name}")
        else:
            print("index.html not found; skipped embedding", file=sys.stderr)


if __name__ == "__main__":
    main()
