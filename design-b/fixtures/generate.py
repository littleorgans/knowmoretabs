#!/usr/bin/env python3
"""Synthetic library fixture for the knowmoretabs frontend prototype.

slice: library
why:   The UI has to be designed against a realistic shape — a head of pages
       present in every snapshot, a long tail seen once, ~150 domains — and
       committed fixtures must never contain anyone's real browsing. Every
       URL and title here is invented. Deterministic: same seed, same file.

Writes fixtures/library.json and re-embeds the blob into ../index.html
between the two marker comments, so the prototype opens from file://.

    python3 generate.py            # ~2000 pages, ~41 snapshots
    python3 generate.py --seed 7   # a different but equally plausible world
"""
import argparse, json, random, re, sys
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
               ("Kitchen", "yellow"), ("Later", "purple"), ("Keyboard", "pink"), ("Papers", "cyan")]


def build(seed, snapshot_count=41, head_count=64):
    r = random.Random(seed)
    world = World(r)
    pages = []          # {url,title,domain}
    meta = []           # per page: stickiness, home window, first_seen
    tab_ids = {}

    def new_page(kind):
        url, title = world.page()
        if r.random() < 0.012:
            title = ""      # Chrome sometimes writes no title
        host = re.sub(r"^https?://(www\.)?", "", url).split("/")[0]
        pages.append({"url": url, "title": title, "domain": host})
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
        # groups on a couple of windows
        groups = []
        group_of = {}
        for w in range(1, windows + 1):
            if r.random() < 0.3:
                name, colour = r.choice(GROUP_NAMES)
                gid = len(groups)
                groups.append({"id": gid, "title": name, "colour": colour})
                for p in open_set:
                    if win_of[p] == w and r.random() < 0.35:
                        group_of[p] = gid
        # lay out: per window, ordered by first appearance (older tabs sit left)
        tabs = []
        for w in range(1, windows + 1):
            members = sorted((p for p in open_set if win_of[p] == w), key=lambda p: tab_ids[p])
            pos = 0
            for p in members:
                pinned = 1 if (p in head and w == 1 and pos < 4) else 0
                tabs.append([p, w, pos, tab_ids[p], pinned, group_of.get(p)])
                pos += 1
                if r.random() < 0.03:  # the same page open twice in one window
                    tabs.append([p, w, pos, tab_ids[p] + 5000 + pos, 0, group_of.get(p)])
                    pos += 1
        snapshots.append({
            "id": when.strftime("%Y-%m-%d-%H%M%SZ"),
            "captured_at": when.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "browser": "chrome",
            "profile": "Default",
            "windows": windows,
            "tabs_total": len(tabs),
            "groups": groups,
            "tabs": tabs,
        })

    forgotten = set(r.sample(range(head_count, len(pages)), 6))
    for i in forgotten:
        pages[i]["forgotten"] = True
    domains = {p["domain"] for p in pages}
    return {
        "schema_version": 1,
        "generated_at": times[-1].strftime("%Y-%m-%dT%H:%M:%SZ"),
        "stats": {"pages": len(pages), "snapshots": len(snapshots), "domains": len(domains),
                  "sightings": sum(len(s["tabs"]) for s in snapshots), "forgotten": len(forgotten)},
        "snapshots": snapshots,
        "pages": pages,
    }


def embed(html_path, blob):
    html = html_path.read_text(encoding="utf-8")
    start, end = "<!-- library-data:start -->", "<!-- library-data:end -->"
    a, b = html.index(start), html.index(end)
    safe = blob.replace("</", "<\\/")      # keeps </script> out of the JSON; still valid JSON
    html = html[: a + len(start)] + '\n<script id="library-data" type="application/json">' + safe + "</script>\n" + html[b:]
    html_path.write_text(html, encoding="utf-8")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--seed", type=int, default=2026)
    ap.add_argument("--snapshots", type=int, default=41)
    ap.add_argument("--no-embed", action="store_true", help="only write library.json")
    args = ap.parse_args()
    lib = build(args.seed, args.snapshots)
    out = HERE / "library.json"
    out.write_text(json.dumps(lib, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    counts = {}
    for s in lib["snapshots"]:
        for i in {t[0] for t in s["tabs"]}:
            counts[i] = counts.get(i, 0) + 1
    once = sum(1 for c in counts.values() if c == 1)
    every = sum(1 for c in counts.values() if c == len(lib["snapshots"]))
    print(f"{out.name}: {out.stat().st_size:,} bytes · {lib['stats']['pages']} pages · "
          f"{lib['stats']['snapshots']} snapshots · {lib['stats']['domains']} domains · "
          f"{lib['stats']['sightings']} sightings · seen once {once} · in every snapshot {every} · "
          f"tabs/snapshot {min(s['tabs_total'] for s in lib['snapshots'])}–{max(s['tabs_total'] for s in lib['snapshots'])}")
    if not args.no_embed:
        html = HERE.parent / "index.html"
        if html.exists():
            embed(html, out.read_text(encoding="utf-8"))
            print(f"embedded into {html.name}")
        else:
            print("index.html not found; skipped embedding", file=sys.stderr)


if __name__ == "__main__":
    main()
