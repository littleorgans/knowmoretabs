"""`tagger <step>`: each step reads the previous step's files under the data directory."""

import argparse
import os

from .paths import resolve


def main() -> None:
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    parser = argparse.ArgumentParser(prog="tagger")
    parser.add_argument("--data", help="data directory (default: $KMT_TAGGER_DATA)")
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
    steps.add_parser("cost", help="load time, RSS, throughput and single page latency per model")
    steps.add_parser("report", help="write out/results.md")
    compare = steps.add_parser("compare", help="compare metric outputs with another data directory")
    compare.add_argument("other")
    args = parser.parse_args()
    paths = resolve(args.data)
    if args.step == "dataset":
        from . import dataset

        dataset.run(paths)
    elif args.step == "embed":
        from . import embed

        embed.run(paths, args.model, args.chunked)
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
    elif args.step == "cost":
        from . import cost

        cost.run(paths)
    elif args.step == "compare":
        from . import compare

        compare.run(paths, args.other)
    elif args.step == "report":
        from . import report

        report.run(paths)
