import json

import httpx
import pytest

from jev_optimize.adapter import JevPrograms
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
        return httpx.Response(200, json={"model": MODEL, "answers": {}})

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


def test_one_signature_becomes_one_exact_typed_question():
    class CaptureClient:
        def __init__(self):
            self.call = None

        def decide(self, state, questions):
            self.call = (state, questions)
            return {"triage.asks_recipient": {"type": "noul", "noul": 0.75}}

    client = CaptureClient()
    prediction = JevPrograms(client).program("triage.asks_recipient")(
        subject="Mosaic update", paragraph_text="Please review the draft.", from_user=False
    )
    assert prediction.probability == 0.75
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
