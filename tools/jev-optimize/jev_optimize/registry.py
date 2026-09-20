"""Merge reviewed optimization results into the pinned question registry."""

from __future__ import annotations

import hashlib
import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .questions import SETS

REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_REGISTRY = REPO_ROOT / "contracts" / "model" / "decision-questions.json"


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()


def registry_hash(value: Any) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def merge_registry(
    results: dict[str, Any],
    path: str | Path = DEFAULT_REGISTRY,
    *,
    tuned_at: str | None = None,
    allow_baseline: bool = False,
) -> str:
    print(
        "question | validation baseline->tuned | test accuracy baseline->tuned | "
        "accept/escalate"
    )
    blocked: list[str] = []
    for result in results.values():
        for question_id, update in result.get("questions", {}).items():
            baseline_test = update.get("baseline_test_metrics", {})
            tuned_test = update.get("tuned_test_metrics", {})
            baseline_validation = update.get("baseline_validation_score", "-")
            tuned_validation = update.get("tuned_validation_score", "-")
            print(
                f"{question_id} | "
                f"{baseline_validation}->{tuned_validation} | "
                f"{baseline_test.get('accuracy', '-')}->{tuned_test.get('accuracy', '-')} | "
                f"{update.get('accept', '-')}/{update.get('escalate', '-')}"
            )
            reasons = []
            if update.get("insufficient_data"):
                reasons.append("insufficient_data")
            if update.get("kept_baseline"):
                reasons.append("kept_baseline")
            if reasons:
                blocked.append(f"{question_id} ({', '.join(reasons)})")
    if blocked and not allow_baseline:
        raise ValueError(
            "refusing to write guarded question results: "
            + ", ".join(blocked)
            + "; pass --allow-baseline after review"
        )

    destination = Path(path)
    registry = json.loads(destination.read_text(encoding="utf-8"))
    metrics = registry.setdefault("metrics", {})
    for set_name, result in results.items():
        if set_name not in SETS:
            raise ValueError(f"unknown set: {set_name}")
        for question_id, update in result.get("questions", {}).items():
            if question_id not in SETS[set_name]:
                raise ValueError(f"question {question_id} is outside set {set_name}")
            entry = registry["questions"][question_id]
            for key in ("instructions", "options", "accept", "escalate"):
                if key in update:
                    entry[key] = update[key]
        metrics[set_name] = {
            "model": result.get("model", registry["model"]),
            "dataset_sizes": result.get("dataset_sizes", {}),
            "test": result.get("test_metrics", {}),
        }
    registry["tuned_at"] = tuned_at or datetime.now(UTC).isoformat()
    stable = json.loads(json.dumps(registry, ensure_ascii=False, sort_keys=True))
    destination.write_text(
        json.dumps(stable, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8"
    )
    digest = registry_hash(stable)
    print(digest)
    return digest
