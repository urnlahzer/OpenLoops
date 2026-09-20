import json

import pytest

from jev_optimize.data import load_jsonl
from jev_optimize.questions import SETS
from jev_optimize.registry import merge_registry
from jev_optimize.synthetic import generate_synthetic


def test_synthetic_generation_is_deterministic_and_sane(tmp_path):
    one = tmp_path / "one"
    two = tmp_path / "two"
    generate_synthetic(one, seed=42, count=600)
    generate_synthetic(two, seed=42, count=600)
    for set_name, question_ids in SETS.items():
        assert (one / f"{set_name}.jsonl").read_bytes() == (
            two / f"{set_name}.jsonl"
        ).read_bytes()
        rows = load_jsonl(one / f"{set_name}.jsonl")
        assert len(rows) >= 600
        assert all(
            row.source == "synthetic" and set(row.label) == set(question_ids)
            for row in rows
        )
        positive_sets = []
        for question_id in question_ids:
            values = [row.label[question_id] for row in rows]
            if all(isinstance(value, bool) for value in values):
                rate = sum(values) / len(values)
                assert 0.4 <= rate <= 0.6
                positive_sets.append(
                    {row.id for row, value in zip(rows, values, strict=True) if value}
                )
        assert len({frozenset(values) for values in positive_sets}) == len(positive_sets)


def test_registry_merge_writes_loader_compatible_shape(tmp_path):
    path = tmp_path / "registry.json"
    questions = {
        question_id: {
            "type": "choice" if question_id == "rules.deadline_kind" else "noul",
            "instructions": "A positive literal statement.",
            "accept": 0.7,
            "escalate": 0.3,
            **(
                {
                    "options": {
                        "event_tied": "Event.",
                        "soft": "Soft.",
                        "unknown": "Unknown.",
                    }
                }
                if question_id == "rules.deadline_kind"
                else {}
            ),
        }
        for ids in SETS.values()
        for question_id in ids
    }
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "model": "typesafe/jev-1.13",
                "tuned_at": None,
                "questions": questions,
            }
        ),
        encoding="utf-8",
    )
    merge_registry(
        {"closure": {"questions": {"closure.fulfilled": {"accept": 0.8, "escalate": 0.2}}}},
        path,
        tuned_at="2026-09-19T00:00:00+00:00",
    )
    result = json.loads(path.read_text(encoding="utf-8"))
    assert result["questions"]["closure.fulfilled"]["accept"] == 0.8
    assert result["metrics"]["closure"]["model"] == "typesafe/jev-1.13"


@pytest.mark.parametrize("flag", ["insufficient_data", "kept_baseline"])
def test_registry_merge_refuses_guarded_results_without_override(tmp_path, flag):
    path = tmp_path / "registry.json"
    path.write_text(
        json.dumps(
            {
                "model": "typesafe/jev-1.13",
                "questions": {
                    "closure.fulfilled": {
                        "type": "noul",
                        "instructions": "The later text shows completion.",
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    results = {"closure": {"questions": {"closure.fulfilled": {flag: True}}}}
    before = path.read_bytes()
    with pytest.raises(ValueError, match="--allow-baseline"):
        merge_registry(results, path)
    assert path.read_bytes() == before
