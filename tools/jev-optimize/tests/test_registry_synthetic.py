import json

import httpx
import pytest

from jev_optimize.data import load_jsonl
from jev_optimize.questions import SETS
from jev_optimize.registry import merge_registry
from jev_optimize.synthetic import (
    CorpusThresholds,
    _chat_rows,
    check_corpus,
    generate_stub,
    scenario_seeds,
    scrub_row,
)


@pytest.fixture
def relaxed_thresholds():
    return CorpusThresholds(min_rows=60, min_paragraphs=30, min_obligations=20)


def test_synthetic_generation_is_deterministic_and_sane(tmp_path, relaxed_thresholds):
    one = tmp_path / "one"
    two = tmp_path / "two"
    generate_stub(one, seed=42, rows=60)
    generate_stub(two, seed=42, rows=60)
    for set_name, question_ids in SETS.items():
        assert (one / f"{set_name}.jsonl").read_bytes() == (
            two / f"{set_name}.jsonl"
        ).read_bytes()
        rows = load_jsonl(one / f"{set_name}.jsonl")
        assert len(rows) == 60
        assert all(
            row.source == "synthetic-stub" and set(row.label) == set(question_ids)
            for row in rows
        )
        positive_sets = []
        for question_id in question_ids:
            values = [row.label[question_id] for row in rows]
            if all(isinstance(value, bool) for value in values):
                rate = sum(values) / len(values)
                if set_name == "closure":
                    assert 0.15 <= rate <= 0.3
                else:
                    assert 0.4 <= rate <= 0.6
                positive_sets.append(
                    {row.id for row, value in zip(rows, values, strict=True) if value}
                )
        assert len({frozenset(values) for values in positive_sets}) == len(positive_sets)
        report = check_corpus(one / f"{set_name}.jsonl", relaxed_thresholds)
        assert report["errors"] == []


def test_every_question_has_at_least_thirty_scenario_seeds():
    for question_ids in SETS.values():
        for question_id in question_ids:
            assert len(scenario_seeds(question_id)) >= 30


def test_scrub_rejects_external_addresses_urls_and_unknown_names():
    assert scrub_row({"text": "contact@" + "outside.invalid"}) == [
        "non-example.invalid email address"
    ]
    assert scrub_row({"text": "See https://example.invalid/path"}) == ["URL"]
    assert scrub_row({"text": "Unlisted" + " Person sent the note."}) == [
        "capitalized name outside the synthetic pool"
    ]
    assert scrub_row({"text": "Priya Venkataraman used priya@example.invalid."}) == []


def test_llm_request_uses_zdr_and_strict_json_without_live_network():
    seen = {}

    def handler(request):
        seen.update(json.loads(request.content))
        return httpx.Response(
            200,
            json={"choices": [{"message": {"content": '{"rows": []}'}}]},
        )

    with httpx.Client(transport=httpx.MockTransport(handler)) as client:
        assert _chat_rows(
            client, "synthetic/model", "synthetic-test-placeholder", "triage", []
        ) == []
    assert seen["provider"] == {"zdr": True}
    assert seen["response_format"]["type"] == "json_schema"
    assert seen["response_format"]["json_schema"]["strict"] is True


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
