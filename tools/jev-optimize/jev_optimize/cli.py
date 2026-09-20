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
from .registry import DEFAULT_REGISTRY, merge_registry
from .synthetic import DEFAULT_ROOT, check_corpus, generate_llm, generate_stub

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
    mode = generate.add_mutually_exclusive_group(required=True)
    mode.add_argument("--llm", action="store_true")
    mode.add_argument("--stub", action="store_true")
    generate.add_argument("--set", choices=(*SETS, "all"), default="all")
    generate.add_argument("--rows", type=int)
    generate.add_argument("--output", type=Path, default=DEFAULT_ROOT)
    generate.add_argument("--seed", type=int, default=20260919)
    generate.add_argument(
        "--parallel", type=int, default=8, help="concurrent model calls for --llm"
    )
    generate.add_argument(
        "--smoke",
        action="store_true",
        help="cheap live trial: 128 rows into runs/smoke with relaxed checks",
    )
    check = commands.add_parser("check-corpus")
    check.add_argument("file", type=Path)
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
    write.add_argument("--allow-baseline", action="store_true")
    return parser


def main() -> None:
    args = _parser().parse_args()
    if args.command == "generate-synthetic":
        rows = args.rows or (60 if args.stub else 600)
        if args.stub:
            paths = generate_stub(args.output, selected_set=args.set, rows=rows, seed=args.seed)
        else:
            output = args.output
            if args.smoke:
                # Keep smoke runs large enough for useful per-question samples.
                rows = args.rows or 128
                if output == DEFAULT_ROOT:
                    output = Path("runs") / "smoke"
            paths = generate_llm(
                output,
                selected_set=args.set,
                rows=rows,
                seed=args.seed,
                parallel=args.parallel,
                smoke=args.smoke,
            )
        for path in paths:
            print(path)
    elif args.command == "check-corpus":
        try:
            print(json.dumps(check_corpus(args.file), indent=2, sort_keys=True))
        except ValueError as error:
            raise SystemExit(f"check-corpus: {error}") from None
    elif args.command == "fetch-enron":
        print(fetch_enron())
    elif args.command == "probe":
        print(f"zdr_member={str(DecisionsClient().probe()).lower()}")
    elif args.command == "evaluate":
        rows = load_jsonl(args.data)
        adapter = JevPrograms(_client(args.replay))
        registry = json.loads(DEFAULT_REGISTRY.read_text(encoding="utf-8"))
        result = {
            question_id: evaluate_question(
                adapter.program(question_id),
                question_id,
                split_rows(rows, question_id=question_id)["test"],
                registry_thresholds={
                    name: registry["questions"][question_id][name]
                    for name in ("accept", "escalate")
                },
            )
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
        try:
            if args.registry:
                merge_registry(merged, args.registry, allow_baseline=args.allow_baseline)
            else:
                merge_registry(merged, allow_baseline=args.allow_baseline)
        except ValueError as error:
            raise SystemExit(f"write-registry: {error}") from None


if __name__ == "__main__":
    main()
