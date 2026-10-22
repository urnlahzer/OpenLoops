import hashlib
import json
import re
import shutil
import threading
from pathlib import Path

import dspy
import httpx
import pytest
from dspy.clients import configure_cache
from dspy.utils.dummies import DummyLM

import jev_optimize.optimize as optimize_module
from jev_optimize.adapter import program
from jev_optimize.client import DecisionResponseError, DecisionsClient
from jev_optimize.data import DatasetRow, load_jsonl, split_name, write_jsonl
from jev_optimize.metrics import ACCEPT_FLOOR, ESCALATE_FLOOR
from jev_optimize.optimize import evaluate_question, optimize_set
from jev_optimize.questions import SETS, SPECS
from jev_optimize.registry import DEFAULT_REGISTRY, merge_registry
from jev_optimize.walk_metric import evaluate_walk

TOOL_ROOT = Path(__file__).parents[1]
MODEL = "typesafe/jev-1.13"
REPORTED_MODEL = "typesafe/jev-1.13-20260917"


def _stable_jitter(question_id, text):
    digest = hashlib.sha256(f"{question_id}\0{text}".encode()).digest()
    unit = int.from_bytes(digest[:8], "big") / ((1 << 64) - 1)
    return 0.3 * unit - 0.15


def _noul_probability(question_id, text):
    lower = text.casefold()
    if question_id == "walk.responds":
        matched = "unrelated note" not in lower and "mk-unrelated" not in lower
        probability = 0.9 if matched else 0.1
        return min(0.99, max(0.01, probability + _stable_jitter(question_id, text)))
    walk_markers = {
        "walk.wp_received": "mk-received.",
        "walk.wp_no_longer_needed": "mk-withdrawn.",
        "walk.wp_asks_again": "mk-chase.",
        "walk.wp_changes_terms": "mk-amend-",
        "walk.wp_still_owed": "mk-still-owed.",
        "walk.wp_changes_due": "mk-newdate.",
        "walk.user_delivers": "mk-delivered.",
        "walk.user_promises": "mk-promise.",
        "walk.user_declines": "mk-decline.",
        "walk.user_hands_off": "mk-handoff.",
        "walk.third_party_done": "mk-thirddone.",
    }
    if question_id in walk_markers:
        matched = walk_markers[question_id] in lower
        probability = 0.9 if matched else 0.1
        return min(0.99, max(0.01, probability + _stable_jitter(question_id, text)))
    signals = {
        "triage.asks_recipient": ("please", "could you", "would you", "can you"),
        "triage.commits_sender": ("i will", "i'll", "we will", "we'll"),
        "triage.asks_question": ("?",),
        "triage.names_time": (
            "today",
            "tomorrow",
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "deadline",
        ),
        "triage.boilerplate": ("unsubscribe", "confidential", "disclaimer"),
        "triage.automated_notification": ("automated", "do not reply", "notification"),
    }
    fallback = ("done", "sent", "completed")
    matched = any(token in lower for token in signals.get(question_id, fallback))
    return min(0.99, max(0.01, (0.62 if matched else 0.38) + _stable_jitter(question_id, text)))


def _choice_answer(question_id, text, criteria):
    labels = list(criteria)
    weights = []
    for label in labels:
        digest = hashlib.sha256(f"{question_id}\0{label}\0{text}".encode()).digest()
        weight = 0.2 + int.from_bytes(digest[:4], "big") / (1 << 32)
        if label.replace("_", " ") in text.casefold():
            weight += 1.0
        weights.append(weight)
    total = sum(weights)
    probabilities = {label: weight / total for label, weight in zip(labels, weights, strict=True)}
    probabilities[labels[-1]] = 1.0 - sum(probabilities[label] for label in labels[:-1])
    choice = max(labels, key=probabilities.get)
    return {
        "type": "choice",
        "choice": choice,
        "probabilities": probabilities,
        "confidence": probabilities[choice],
    }


class FakeDecisionsEndpoint:
    def __init__(self, mode="normal"):
        self.mode = mode
        self.calls = 0
        self.question_ids = set()
        self.seen_ids = set()
        self.saw_envelope_state = False
        self._lock = threading.Lock()

    def __call__(self, request):
        assert request.headers.get("Authorization") == "Bearer test-key-not-real"
        body = json.loads(request.content)
        assert set(body) == {"model", "state", "questions", "provider"}
        assert body["model"] == MODEL
        assert body["provider"] == {"zdr": True}
        assert "inputs" not in body["state"]
        assert "instructions" not in body["state"]
        assert len(body["questions"]) == 1
        question_id, question = next(iter(body["questions"].items()))
        assert re.fullmatch(r"[a-z_]+\.[a-z_]+", question_id)
        assert set(body["state"]) == set(SPECS[question_id].signature.input_fields)
        expected_question_keys = {"type", "instructions"}
        if SPECS[question_id].options:
            expected_question_keys.add("criteria")
        assert set(question) == expected_question_keys
        assert question["type"] == SPECS[question_id].kind
        with self._lock:
            self.calls += 1
            call_number = self.calls
            self.question_ids.add(question_id)
            self.seen_ids.add(question_id)
            self.saw_envelope_state |= "inputs" in body["state"]
        if self.mode == "http_400":
            return httpx.Response(400, json={"error": "synthetic rejection"})
        if self.mode == "retry" and call_number == 1:
            return httpx.Response(429, json={"error": "synthetic rate limit"})
        text = json.dumps(body["state"], ensure_ascii=False, sort_keys=True)
        if question["type"] == "noul":
            probability = _noul_probability(question_id, text)
            if self.mode == "malformed" and call_number == 1:
                probability = 1.5
            answer = {"type": "noul", "noul": probability}
        else:
            answer = _choice_answer(question_id, text, question["criteria"])
            assert sum(answer["probabilities"].values()) == pytest.approx(1.0)
        return httpx.Response(
            200,
            json={
                "model": REPORTED_MODEL,
                "usage": {"input_tokens": max(1, len(request.content) // 4), "cost": 0.0001},
                "answers": {question_id: answer},
            },
        )


def _reduced_triage_rows():
    rows = load_jsonl(TOOL_ROOT / "data" / "synthetic" / "triage.jsonl")
    selected = []
    for question_id in SETS["triage"]:
        prefix = []
        for row in (candidate for candidate in rows if question_id in candidate.label):
            prefix.append(row)
            train_validation = [
                item for item in prefix if split_name(item.id, small=True) != "test"
            ]
            positives = sum(item.label[question_id] is True for item in train_validation)
            negatives = sum(item.label[question_id] is False for item in train_validation)
            split_counts = {
                name: sum(split_name(item.id, small=True) == name for item in prefix)
                for name in ("train", "validation", "test")
            }
            if positives >= 20 and negatives >= 20 and all(split_counts.values()):
                break
        assert positives >= 20 and negatives >= 20
        assert all(split_counts.values())
        selected.extend(prefix)
    return selected


def _one_row():
    return DatasetRow(
        id="offline-evaluation-row",
        set="triage",
        subject="Synthetic request",
        paragraph_text="Could you please review this synthetic note?",
        from_user=False,
        label={"triage.asks_recipient": True},
        source="synthetic-stub",
    )


def _client(endpoint):
    return DecisionsClient(
        model=MODEL,
        transport=httpx.MockTransport(endpoint),
        max_retries=2,
    )


@pytest.fixture(autouse=True)
def memory_only_dspy_cache():
    had_cache = "cache" in dspy.__dict__
    previous_cache = dspy.__dict__.get("cache")
    configure_cache(
        enable_disk_cache=False,
        enable_memory_cache=True,
        disk_cache_dir=None,
    )
    yield
    with dspy._cache_lock:
        if had_cache:
            dspy.cache = previous_cache
        else:
            dspy.__dict__.pop("cache", None)


def test_real_optimize_path_is_offline_and_production_shaped(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    data_path = tmp_path / "triage-reduced.jsonl"
    write_jsonl(data_path, (row.model_dump() for row in _reduced_triage_rows()))
    endpoint = FakeDecisionsEndpoint()
    reflection_lm = DummyLM(
        {
            "paragraph_text": {
                "reasoning": "The statement stays literal and names the primary field.",
                "proposed_statement": "paragraph_text states the relevant triage condition.",
            }
        }
    )
    real_reanchor = optimize_module.ReAnchor
    reanchor_calls = []

    class RecordingReAnchor(real_reanchor):
        def compile(self, student, *, trainset, valset=None):
            reanchor_calls.append((student, trainset, valset))
            return super().compile(student, trainset=trainset, valset=valset)

    monkeypatch.setattr(optimize_module, "ReAnchor", RecordingReAnchor)
    result = optimize_set(
        "triage",
        _client(endpoint),
        data_path,
        budget="light",
        reflection_lm=reflection_lm,
    )

    assert set(result["questions"]) == set(SETS["triage"])
    if "extract.claim_type" in SETS["triage"]:
        assert "extract.claim_type" in result["questions"]
    for question_id in SETS["triage"]:
        update = result["questions"][question_id]
        assert isinstance(update["accept"], float)
        assert isinstance(update["escalate"], float)
        assert ESCALATE_FLOOR <= update["escalate"] < update["accept"]
        assert update["accept"] >= ACCEPT_FLOOR
        assert update["insufficient_data"] is False
        assert {"accuracy", "brier", "ece_10"} <= set(
            result["test_metrics"][question_id]
        )
    noul_count = sum(SPECS[question_id].kind == "noul" for question_id in SETS["triage"])
    assert len(reanchor_calls) == 2 * noul_count
    assert endpoint.question_ids == set(SETS["triage"])
    assert endpoint.saw_envelope_state is False

    registry_path = tmp_path / "decision-questions.json"
    shutil.copyfile(DEFAULT_REGISTRY, registry_path)
    before = json.loads(registry_path.read_text(encoding="utf-8"))
    digest = merge_registry(
        {"triage": result},
        path=registry_path,
        allow_baseline=True,
    )
    after = json.loads(registry_path.read_text(encoding="utf-8"))
    assert re.fullmatch(r"[0-9a-f]{64}", digest)
    assert set(after) == set(before)
    assert {key: set(value) for key, value in after["questions"].items()} == {
        key: set(value) for key, value in before["questions"].items()
    }


def _reflection_lm():
    return DummyLM(
        {
            "later.text": {
                "reasoning": "The statement stays literal and names the primary field.",
                "proposed_statement": "later.text states the relevant walk condition.",
            }
        }
    )


def test_walk_subset_optimizes_offline(tmp_path, monkeypatch):
    from jev_optimize.threads import derive_rows, plan_threads, stub_threads

    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    rows = [
        row
        for row in derive_rows(stub_threads(plan_threads(400)))
        if row["set"] == "walk"
    ]
    data = tmp_path / "walk.jsonl"
    write_jsonl(data, rows)
    endpoint = FakeDecisionsEndpoint()
    result = optimize_set(
        "walk",
        _client(endpoint),
        data,
        budget="light",
        reflection_lm=_reflection_lm(),
        only=("walk.responds", "walk.user_delivers"),
    )
    assert set(result["questions"]) == {"walk.responds", "walk.user_delivers"}
    for question_id in result["questions"]:
        question = result["questions"][question_id]
        assert ESCALATE_FLOOR <= question["escalate"] < question["accept"]
    assert endpoint.seen_ids >= {"walk.responds", "walk.user_delivers"}


def test_evaluate_walk_reports_offline_metrics(tmp_path, monkeypatch):
    from jev_optimize.threads import plan_threads, stub_threads, thread_record

    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    path = tmp_path / "threads.jsonl"
    write_jsonl(path, map(thread_record, stub_threads(plan_threads(40))))
    endpoint = FakeDecisionsEndpoint()
    report = evaluate_walk(path, _client(endpoint))
    assert {"accuracy", "escalated_share", "confusion"} <= set(report)


def test_evaluate_question_rejects_malformed_reply(monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    endpoint = FakeDecisionsEndpoint("malformed")
    with pytest.raises(DecisionResponseError):
        evaluate_question(
            program("triage.asks_recipient", _client(endpoint)),
            "triage.asks_recipient",
            [_one_row()],
        )


def test_evaluate_question_propagates_rejected_row(monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    endpoint = FakeDecisionsEndpoint("http_400")
    with pytest.raises(httpx.HTTPStatusError):
        evaluate_question(
            program("triage.asks_recipient", _client(endpoint)),
            "triage.asks_recipient",
            [_one_row()],
        )


def test_evaluate_question_retries_429_then_succeeds(monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setattr("jev_optimize.client.time.sleep", lambda _seconds: None)
    endpoint = FakeDecisionsEndpoint("retry")
    result = evaluate_question(
        program("triage.asks_recipient", _client(endpoint)),
        "triage.asks_recipient",
        [_one_row()],
    )
    assert endpoint.calls == 2
    assert {"accuracy", "brier", "ece_10"} <= set(result)
