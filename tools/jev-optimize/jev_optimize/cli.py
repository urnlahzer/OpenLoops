"""Command-line entry point for the Jev optimization harness."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from .adapter import JevPrograms
from .client import DecisionsClient, ReplayClient
from .data import fetch_enron, load_jsonl, split_rows
from .optimize import evaluate_question, optimize_set
from .questions import SETS
from .registry import merge_registry
from .synthetic import generate_synthetic

ROOT = Path(__file__).parents[1]


def _client(replay: str | None) -> Any:
    if replay:
        return ReplayClient(replay)
    client = DecisionsClient()
    client.probe()
    return client


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="jev-optimize")
    commands = parser.add_subparsers(dest="command", required=True)
    generate = commands.add_parser("generate-synthetic")
    generate.add_argument("--seed", type=int, default=20260919)
    generate.add_argument("--count", type=int, default=360)
    commands.add_parser("fetch-enron")
    commands.add_parser("probe")
    evaluate = commands.add_parser("evaluate")
    evaluate.add_argument("--set", required=True, choices=SETS)
    evaluate.add_argument("--data", required=True)
    evaluate.add_argument("--replay")
    optimize = commands.add_parser("optimize")
    optimize.add_argument("--set", required=True, choices=SETS)
    optimize.add_argument("--data")
    optimize.add_argument("--budget", choices=("light", "medium"), default="light")
    optimize.add_argument(
        "--replay", nargs="?", const=str(ROOT / "runs" / "latest.json")
    )
    write = commands.add_parser("write-registry")
    write.add_argument("results", nargs="+")
    write.add_argument("--registry")
    return parser


def main() -> None:
    args = _parser().parse_args()
    if args.command == "generate-synthetic":
        for path in generate_synthetic(seed=args.seed, count=args.count):
            print(path)
    elif args.command == "fetch-enron":
        print(fetch_enron())
    elif args.command == "probe":
        print(f"zdr_member={str(DecisionsClient().probe()).lower()}")
    elif args.command == "evaluate":
        rows = split_rows(load_jsonl(args.data))["test"]
        adapter = JevPrograms(_client(args.replay))
        result = {
            question_id: evaluate_question(adapter.program(question_id), question_id, rows)
            for question_id in SETS[args.set]
        }
        print(json.dumps(result, indent=2, sort_keys=True))
    elif args.command == "optimize":
        data = args.data or ROOT / "data" / "synthetic" / f"{args.set}.jsonl"
        result = optimize_set(args.set, _client(args.replay), data, budget=args.budget)
        output = ROOT / "runs" / f"{args.set}-result.json"
        output.parent.mkdir(exist_ok=True)
        output.write_text(
            json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(output)
    elif args.command == "write-registry":
        merged = {}
        for path in args.results:
            result = json.loads(Path(path).read_text(encoding="utf-8"))
            set_name = next(
                name
                for name, ids in SETS.items()
                if set(result["questions"]) <= set(ids)
            )
            merged[set_name] = result
        if args.registry:
            merge_registry(merged, args.registry)
        else:
            merge_registry(merged)


if __name__ == "__main__":
    main()
