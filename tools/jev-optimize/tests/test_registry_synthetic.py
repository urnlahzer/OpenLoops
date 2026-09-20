import json

from jev_optimize.data import load_jsonl
from jev_optimize.questions import SETS
from jev_optimize.registry import merge_registry
from jev_optimize.synthetic import generate_synthetic


def test_synthetic_generation_is_deterministic_and_sane(tmp_path):
    one = tmp_path / "one"
    two = tmp_path / "two"
    generate_synthetic(one, seed=42, count=300)
    generate_synthetic(two, seed=42, count=300)
    for set_name, question_ids in SETS.items():
        assert (one / f"{set_name}.jsonl").read_bytes() == (
            two / f"{set_name}.jsonl"
        ).read_bytes()
        rows = load_jsonl(one / f"{set_name}.jsonl")
        assert len(rows) == 300
        assert all(
            row.source == "synthetic" and set(row.label) == set(question_ids)
            for row in rows
        )
        if set_name != "rules":
            assert any(any(value is True for value in row.label.values()) for row in rows)
            assert any(not any(value is True for value in row.label.values()) for row in rows)


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
