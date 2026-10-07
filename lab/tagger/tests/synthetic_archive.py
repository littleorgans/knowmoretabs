"""A synthetic knowmoretabs archive for the app and its road test. Every page, host, text, image and tag is
invented here from fixed word banks; nothing comes from a real archive. Hosts use the reserved .example TLD.

`uv run --locked python tests/synthetic_archive.py <archive root> <data dir>` writes the archive (the shape
`tagger.archive.Archive` and the Rust binary read) and `<data dir>/zeroshot/descriptions.json`.
"""

import hashlib
import json
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SEED = 20261008
AT = "2026-10-01T00:00:00Z"

# topic: (owner tag, hue, words, title templates)
TOPICS = {
    "coffee": (
        "Coffee",
        30,
        ["espresso", "grinder", "burr", "light roast", "pour over", "crema", "brew ratio", "kettle", "beans"],
        ["Dialling in {w} at home", "A field guide to {w}", "Why your {w} tastes sour", "{w}: a buying guide"],
    ),
    "rust": (
        "Rust",
        15,
        ["borrow checker", "lifetimes", "async runtime", "traits", "cargo", "unsafe code", "pinning", "macros"],
        ["Understanding {w} in Rust", "{w} without tears", "A tour of {w}", "Notes on {w} and the compiler"],
    ),
    "trains": (
        "Trains",
        210,
        ["night train", "sleeper cabin", "rail pass", "timetable", "border crossing", "couchette", "dining car"],
        ["Taking the {w} across the mountains", "A {w} route log", "How to book a {w}", "Ten hours on a {w}"],
    ),
    "typography": (
        "Typography",
        265,
        ["variable fonts", "kerning", "optical sizes", "serif faces", "line spacing", "type specimens", "ligatures"],
        ["{w}, explained", "Setting text with {w}", "A short history of {w}", "Choosing {w} for the screen"],
    ),
    "keyboards": (
        "Keyboards",
        300,
        ["tactile switches", "keycaps", "split layouts", "firmware", "stabilizers", "spring weight", "hot swap"],
        ["Building a board with {w}", "{w} compared on a force rig", "Lubing {w}", "My year with {w}"],
    ),
    "recipes": (
        "Recipes",
        95,
        ["sourdough", "braised beans", "fresh pasta", "miso soup", "flatbread", "roast vegetables", "dumplings"],
        ["Weeknight {w}", "The only {w} recipe you need", "{w} for a crowd", "Getting {w} right"],
    ),
    "photography": (
        "Photography",
        180,
        ["film cameras", "prime lenses", "darkroom", "street photos", "exposure", "colour grading", "medium format"],
        ["Shooting {w} on a budget", "{w}: what I learned", "A beginner's guide to {w}", "Fixing {w} in post"],
    ),
    "japan": (
        "Japan",
        350,
        ["Kyoto temples", "onsen towns", "ramen shops", "Shinkansen", "ryokan stays", "Tokyo neighbourhoods"],
        ["Two weeks of {w}", "Notes on {w}", "Visiting {w} in winter", "A map of {w}"],
    ),
}
# tag: definition (the vocabulary, and the zero shot query's description)
DEFINITIONS = {
    "Coffee": "home coffee brewing, espresso and gear",
    "Rust": "the Rust programming language",
    "Trains": "rail travel and night trains",
    "Typography": "type design, fonts and setting text",
    "Keyboards": "mechanical keyboards, switches and builds",
    "Recipes": "recipes and cooking techniques",
    "Photography": "cameras, lenses, film and photo editing",
    "Japan": "travel and culture in Japan",
    "Read later": "long reads worth coming back to",
    "Woodworking": "woodworking, joinery and hand tools",
}
PAGES_PER_TOPIC = {"japan": 8}  # Japan stays under the 10 positives a head needs
DEFAULT_PAGES = 15
LOCAL = "http://localhost:4000/dev"


def sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=1) + "\n")


def jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(r) + "\n" for r in rows))


def pages(rng: random.Random) -> list[dict]:
    out = []
    for topic, (tag, hue, words, titles) in TOPICS.items():
        for i in range(PAGES_PER_TOPIC.get(topic, DEFAULT_PAGES)):
            word = words[i % len(words)]
            others = rng.sample(words, 3)
            url = f"https://{topic}{i % 4}.example/{topic}/{i}-{word.replace(' ', '-').lower()}"
            body = " ".join(
                f"This piece is about {w}. It covers {rng.choice(others)} and {rng.choice(words)} in some detail."
                for w in [word, *others]
            )
            tags = []
            if rng.random() < 0.75:  # a quarter of each topic is left for the model to find
                tags.append(tag)
            if rng.random() < 0.15:
                tags.append("Read later")
            out.append(
                {
                    "url": url,
                    "title": titles[i % len(titles)].format(w=word),
                    "topic": topic,
                    "word": word,
                    "hue": hue,
                    "body": body,
                    "tags": tags,
                    "text_ok": i % 7 != 3,
                    "image_ok": i % 5 != 2,
                    "metadata": i % 3 != 1,
                }
            )
    return out


def image(page: dict, path: Path) -> None:
    """A poster: the topic's hue, a few shapes and the page's subject in large type."""
    rng = random.Random(page["url"])
    im = Image.new("RGB", (480, 320), _hsv(page["hue"], 0.25, 0.95))
    draw = ImageDraw.Draw(im)
    for _ in range(4):
        x, y, r = rng.randint(0, 480), rng.randint(0, 320), rng.randint(30, 90)
        draw.ellipse((x - r, y - r, x + r, y + r), fill=_hsv(page["hue"], 0.55, 0.75))
    draw.text((24, 130), page["word"].upper(), fill=(20, 20, 20), font=ImageFont.load_default(size=40))
    path.parent.mkdir(parents=True, exist_ok=True)
    im.save(path, "JPEG", quality=80)


def _hsv(hue: int, s: float, v: float) -> tuple[int, int, int]:
    import colorsys

    return tuple(round(255 * c) for c in colorsys.hsv_to_rgb(hue / 360, s, v))


def build(root: Path, data: Path | None = None) -> list[dict]:
    """Write the archive under `root` (and descriptions under `data`); returns the invented pages."""
    rng = random.Random(SEED)
    ps = pages(rng)
    ps[0]["tags"] = []  # a known page with no owner tags
    forgotten = [ps[1]["url"]]
    tabs = [
        {
            "tab_id": i + 1,
            "window": 1,
            "position": i,
            "url": p["url"],
            "title": p["title"],
            "pinned": False,
            "active": i == 0,
            "group": None,
            "last_active": None,
            "window_id": 1,
        }
        for i, p in enumerate(ps)
    ]
    tabs.append({**tabs[0], "tab_id": len(tabs) + 1, "position": len(tabs), "url": LOCAL, "title": "Dev server"})
    snapshot_id = "2026-10-01-000000Z"
    write_json(
        root / "snapshots" / snapshot_id / "snapshot.json",
        {
            "schema_version": 1,
            "id": snapshot_id,
            "captured_at": AT,
            "source": {
                "browser": "chrome",
                "profile": "Default",
                "profile_display": "Synthetic",
                "path": "/synthetic/Session_1",
                "file": "session.snss",
                "sha256": "",
                "bytes": 0,
                "saved_at": AT,
                "session_started_at": None,
            },
            "stats": {
                "file_version": 1,
                "command_table": "synthetic",
                "commands": 0,
                "commands_by_id": {},
                "unknown_commands": 0,
                "unknown_command_ids": [],
                "malformed_commands": 0,
                "truncated_bytes": 0,
                "marker_count": 1,
                "marker_ok": True,
                "windows": 1,
                "tabs": len(tabs),
                "groups": 0,
                "dropped_tabs": 0,
                "dropped_tab_reasons": {"no_navigations": 0, "window_missing": 0, "window_closed": 0},
                "navigation_fallbacks": 0,
                "groups_without_metadata": 0,
            },
            "windows": [{"id": 1, "number": 1, "kind": "normal", "kind_id": 1, "active_tab": None, "tabs": len(tabs)}],
            "groups": [],
            "tabs": tabs,
        },
    )
    write_json(
        root / "library.json",
        {
            "schema_version": 1,
            "forgotten": forgotten,
            "tags": {p["url"]: {"add": p["tags"]} for p in ps if p["tags"]},
            "vocabulary": {t: {"created_at": AT, "definition": d} for t, d in DEFINITIONS.items()},
        },
    )
    content, images, metadata = [], [], []
    for p in ps:
        key = sha256_hex(p["url"])
        if p["text_ok"]:
            (root / "pages/content").mkdir(parents=True, exist_ok=True)
            (root / "pages/content" / f"{key}.md").write_text(
                f"---\nurl: {p['url']}\n---\n# {p['title']}\n\n{p['body']}\n"
            )
        content.append(
            {"schema_version": 1, "url": p["url"], "status": "ok" if p["text_ok"] else "failed", "attempted_at": AT}
        )
        if p["image_ok"]:
            image(p, root / "pages/images" / f"{key}.jpg")
        images.append(
            {"schema_version": 1, "url": p["url"], "status": "ok" if p["image_ok"] else "failed", "attempted_at": AT}
        )
        if p["metadata"]:
            metadata.append(
                {
                    "url": p["url"],
                    "status": "ok",
                    "title": p["title"],
                    "description": f"An invented page about {p['word']}.",
                    "og": {"site_name": f"{p['topic'].title()} Notes"},
                }
            )
    jsonl(root / "pages/content.jsonl", content)
    jsonl(root / "pages/images.jsonl", images)
    jsonl(root / "pages/metadata.jsonl", metadata)
    if data is not None:
        write_json(data / "zeroshot" / "descriptions.json", DEFINITIONS)
    return ps


if __name__ == "__main__":
    built = build(Path(sys.argv[1]), Path(sys.argv[2]))
    print(json.dumps({"pages": len(built), "tagged": sum(bool(p["tags"]) for p in built)}))
