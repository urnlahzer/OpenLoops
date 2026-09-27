import json
import warnings
from copy import deepcopy

import httpx
import pytest

from jev_optimize.adapter import JevLM, program
from jev_optimize.client import (
    DecisionResponseError,
    DecisionsClient,
    ReplayClient,
    parse_response,
    request_bytes,
    stable_request_hash,
)

MODEL = "typesafe/jev-1.13"
QUESTION = {"triage.asks_recipient": {"type": "noul", "instructions": "Act?"}}


def test_request_shape_is_byte_exact(monkeypatch):
    seen = []

    def handler(request):
        seen.append(request.content)
        return httpx.Response(
            200,
            json={
                "model": MODEL,
                "answers": {"triage.asks_recipient": {"type": "noul", "noul": 0.8}},
            },
        )

    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    client = DecisionsClient(transport=httpx.MockTransport(handler))
    client.decide({"text": "Synthetic."}, QUESTION)
    assert seen == [
        b'{"model":"typesafe/jev-1.13","state":{"text":"Synthetic."},'
        b'"questions":{"triage.asks_recipient":{"type":"noul","instructions":"Act?"}},'
        b'"provider":{"zdr":true}}'
    ]


@pytest.mark.parametrize(
    "body",
    [
        {"model": MODEL, "answers": {}},
        {"model": MODEL, "answers": {"triage.asks_recipient": {"type": "choice"}}},
        {"model": MODEL, "answers": {"triage.asks_recipient": {"type": "noul", "noul": 1.1}}},
        {
            "model": MODEL,
            "answers": {
                "triage.asks_recipient": {"type": "noul", "noul": 0.5, "text": "x"}
            },
        },
    ],
)
def test_strict_parser_rejects_invalid_answers(body):
    with pytest.raises(DecisionResponseError):
        parse_response(json.dumps(body).encode(), MODEL, QUESTION)


def test_strict_parser_rejects_duplicate_members():
    raw = b'{"model":"typesafe/jev-1.13","model":"typesafe/jev-1.13","answers":{}}'
    with pytest.raises(DecisionResponseError):
        parse_response(raw, MODEL, {})


def test_probe_drops_rejected_provider_member(monkeypatch):
    calls = []

    def handler(request):
        calls.append(request.content)
        if len(calls) == 1:
            return httpx.Response(422)
        return httpx.Response(
            200,
            json={
                "model": MODEL,
                "answers": {"asks_recipient": {"type": "noul", "noul": 0.9}},
            },
        )

    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    client = DecisionsClient(transport=httpx.MockTransport(handler))
    assert client.probe() is False
    assert b'"provider"' in calls[0]
    assert b'"provider"' not in calls[1]


def test_replay_uses_stable_state_and_question_hash():
    state = {"text": "Synthetic."}
    answer = {"triage.asks_recipient": {"type": "noul", "noul": 0.25}}
    client = ReplayClient({stable_request_hash(state, QUESTION): answer})
    assert client.decide(state, QUESTION) == answer
    assert request_bytes(MODEL, state, QUESTION, zdr_member=False).endswith(b"}")


def test_one_signature_becomes_one_exact_typed_question(monkeypatch):
    class CaptureClient:
        def __init__(self):
            self.call = None

        def decide(self, state, questions):
            self.call = (state, questions)
            return {"triage.asks_recipient": {"type": "noul", "noul": 0.75}}

    client = CaptureClient()
    monkeypatch.setattr("dspy.cache.get", lambda _request: None)
    monkeypatch.setattr("dspy.cache.put", lambda _request, _response: None)
    prediction = program("triage.asks_recipient", client)(
        subject="Mosaic update", paragraph_text="Please review the draft.", from_user=False
    )
    assert prediction.decision.value is True
    assert prediction.decision.probability == 0.75
    assert client.call == (
        {
            "subject": "Mosaic update",
            "paragraph_text": "Please review the draft.",
            "from_user": False,
        },
        {
            "triage.asks_recipient": {
                "type": "noul",
                "instructions": "The paragraph asks its recipient to do something.",
            }
        },
    )


def test_choice_signature_decodes_one_native_decision(monkeypatch):
    class ChoiceClient:
        def __init__(self):
            self.questions = None

        def decide(self, state, questions):
            self.questions = questions
            return {
                "closure.outcome": {
                    "type": "choice",
                    "choice": "fulfilled",
                    "probabilities": {
                        "fulfilled": 0.8,
                        "withdrawn": 0.0,
                        "deadline_changed": 0.0,
                        "modified": 0.0,
                        "none": 0.2,
                    },
                    "confidence": 0.8,
                }
            }

    monkeypatch.setattr("dspy.cache.get", lambda _request: None)
    monkeypatch.setattr("dspy.cache.put", lambda _request, _response: None)
    client = ChoiceClient()
    prediction = program("closure.outcome", client)(
        obligation={"title": "Synthetic", "evidence_text": "Please send it."},
        later={"paragraph_text": "Sent.", "from_user": True, "days_later": 1},
    )
    assert prediction.decision.value == "fulfilled"
    assert prediction.decision.probabilities["fulfilled"] == 0.8
    assert prediction.decision.confidence == 0.8
    assert client.questions["closure.outcome"]["type"] == "choice"
    assert set(client.questions["closure.outcome"]["criteria"]) == {
        "fulfilled",
        "withdrawn",
        "deadline_changed",
        "modified",
        "none",
    }


def test_jev_lm_deepcopy_shares_client_without_warning():
    client = object()
    lm = JevLM(client, "triage.asks_recipient")
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        copied = deepcopy(lm)
    assert copied is not lm
    assert copied.client is client
    assert copied.question_id == "triage.asks_recipient"
    assert not [warning for warning in caught if "Failed to deep copy" in str(warning.message)]


def test_jev_lm_caches_identical_decision_requests(monkeypatch):
    calls = 0

    class CountingClient:
        def decide(self, state, questions):
            nonlocal calls
            calls += 1
            return {"triage.asks_recipient": {"type": "noul", "noul": 0.6}}

    cached = {}
    cache_requests = []

    def cache_get(request):
        cache_requests.append(request)
        return cached.get(repr(request))

    monkeypatch.setattr("dspy.cache.get", cache_get)
    monkeypatch.setattr(
        "dspy.cache.put",
        lambda request, response: cached.__setitem__(repr(request), response),
    )
    lm = JevLM(CountingClient(), "triage.asks_recipient")
    state = {
        "instructions": "Synthetic decision.",
        "input_fields": "1. `text` (str):",
        "inputs": {"text": "Cache-specific synthetic state."},
    }
    questions = {"decision": {"type": "noul", "instructions": "Output description."}}
    assert lm(state, questions) == lm(deepcopy(state), deepcopy(questions))
    assert calls == 1
    assert cache_requests[0] == {
        "provider": "jev",
        "state": {"text": "Cache-specific synthetic state."},
        "questions": {
            "triage.asks_recipient": {
                "type": "noul",
                "instructions": "Synthetic decision.",
            }
        },
    }


def test_jev_lm_rejects_multiple_decision_questions():
    lm = JevLM(object(), "triage.asks_recipient")
    with pytest.raises(ValueError, match="exactly one 'decision'"):
        lm(
            {"instructions": "Synthetic.", "inputs": {"text": "Synthetic."}},
            {
                "decision": {"type": "noul", "instructions": "Synthetic."},
                "other": {"type": "noul", "instructions": "Other."},
            },
        )
