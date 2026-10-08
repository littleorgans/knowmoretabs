"""`tagger <step>`: each step reads the previous step's files under the data directory."""

import argparse
import os
from pathlib import Path

from .paths import resolve, resolve_root

APP_PORT = 7879  # fixed, so a bookmarklet can reach the app; knowmoretabs `serve` keeps 7878


def main() -> None:
    os.umask(0o077)  # per page outputs are private to the owner (0600)
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    parser = argparse.ArgumentParser(prog="tagger")
    parser.add_argument("--data", help="data directory (default: KMT_TAGGER_DATA, else <root>/tagger)")
    parser.add_argument("--root", help="archive directory (default: KMT_ROOT, else ~/.knowmoretabs)")
    steps = parser.add_subparsers(dest="step", required=True)
    steps.add_parser("dataset", help="build the page records from the snapshot")
    embed = steps.add_parser("embed", help="embed inputs A and B per model, and images")
    embed.add_argument("--model", action="append", help="model key (default: all)")
    embed.add_argument("--chunked", action="store_true", help="input B with chunked mean pooling to 8,192 tokens")
    evaluate = steps.add_parser("eval", help="cross validated grid on the 80%%, or --final on the test split")
    evaluate.add_argument("--final", action="store_true", help="score the CV pick once on the held out 20%%")
    steps.add_parser("zeroshot", help="tags as queries, zero labels; then the label efficiency curve")
    steps.add_parser("suggest", help="out of fold suggestions as tag --import JSONL, then a dry run")
    steps.add_parser("cluster", help="unsupervised clusters of the best config against the owner's tags")
    steps.add_parser("gate", help="guided discovery gate: description query plus 5 to 20 answers per tag")
    steps.add_parser("retrieval", help="test R1: tags as saved searches, a ticked grid re-ranks the library")
    steps.add_parser("search-tag", help="test S2: search, pick a few tags, tag the results with only those")
    steps.add_parser("cost", help="load time, RSS, throughput and single page latency per model")
    steps.add_parser("report", help="write out/results.md")
    app = steps.add_parser("app", help="the search then tag prototype, served on 127.0.0.1")
    app.add_argument("--port", type=int, default=APP_PORT, help=f"port (default: {APP_PORT}; 0 picks a free one)")
    app.add_argument("--smoke", action="store_true", help="load, time searches, print numbers only, exit")
    compare = steps.add_parser("compare", help="compare metric outputs with another data directory")
    compare.add_argument("other")
    for step in (embed, app):
        step.add_argument("--root", default=argparse.SUPPRESS, help="archive directory (overrides KMT_ROOT)")
    args = parser.parse_args()
    root = resolve_root(args.root, os.environ.get("KMT_ROOT"), Path.home())
    paths = resolve(args.data, root)
    if args.step == "dataset":
        from . import dataset

        dataset.run(paths)
    elif args.step == "embed":
        from . import embed

        embed.run(paths, args.model, args.chunked, root)
    elif args.step == "eval":
        from . import evaluate

        evaluate.run(paths, args.final)
    elif args.step == "zeroshot":
        from . import zeroshot

        zeroshot.run(paths)
    elif args.step == "suggest":
        from . import suggest

        suggest.run(paths)
    elif args.step == "cluster":
        from . import cluster

        cluster.run(paths)
    elif args.step == "gate":
        from .guided import gate

        gate.run(paths)
    elif args.step == "retrieval":
        from .guided import retrieval

        retrieval.run(paths)
    elif args.step == "search-tag":
        from .guided import search_tag

        search_tag.run(paths)
    elif args.step == "cost":
        from . import cost

        cost.run(paths)
    elif args.step == "compare":
        from . import compare

        compare.run(paths, args.other)
    elif args.step == "app":
        os.environ["HF_HUB_OFFLINE"] = "1"  # the model is cached; the app never goes online
        from .app import server

        server.run(paths, root, args.port, args.smoke)
    elif args.step == "report":
        from . import report

        report.run(paths)
