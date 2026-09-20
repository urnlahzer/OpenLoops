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
) -> str:
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
