from pathlib import Path

import pytest

from jev_optimize.adapter import program
from jev_optimize.client import stable_request_hash
from jev_optimize.data import load_jsonl
from jev_optimize.questions import SPECS

ROOT = Path(__file__).parents[1]


class CaptureClient:
    def __init__(self, question_id):
        self.question_id = question_id
        self.calls = []

    def decide(self, state, questions):
        self.calls.append((state, questions))
        question = questions[self.question_id]
        if question["type"] == "noul":
            answer = {"type": "noul", "noul": 0.75}
        else:
            labels = list(question["criteria"])
            answer = {
                "type": "choice",
                "choice": labels[0],
                "probabilities": {
                    label: float(index == 0) for index, label in enumerate(labels)
                },
                "confidence": 1.0,
            }
        return {self.question_id: answer}


def _row_inputs(question_id):
    spec = SPECS[question_id]
    data_set = "triage" if spec.set_name == "extract" else spec.set_name
    rows = load_jsonl(ROOT / "data" / "synthetic" / f"{data_set}.jsonl")
    row = next(
        (candidate for candidate in rows if question_id in candidate.label),
        rows[0],
    )
    values = row.model_dump(exclude={"id", "set", "label", "source"})
    # The bootstrap waiting-party question has no labeled corpus yet.
    values.setdefault("participants", [])
    return {name: values[name] for name in spec.signature.input_fields}


@pytest.mark.parametrize("question_id", SPECS)
def test_native_predict_sends_the_production_wire_shape(question_id, monkeypatch):
    monkeypatch.setattr("dspy.cache.get", lambda _request: None)
    monkeypatch.setattr("dspy.cache.put", lambda _request, _response: None)
    spec = SPECS[question_id]
    inputs = _row_inputs(question_id)
    expected_question = {
        "type": spec.kind,
        "instructions": spec.signature.instructions,
        **({"criteria": spec.options} if spec.options else {}),
    }
    expected = (inputs, {question_id: expected_question})
    client = CaptureClient(question_id)

    program(question_id, client)(**inputs)

    captured = client.calls[0]
    assert captured == expected
    assert stable_request_hash(*captured) == stable_request_hash(*expected)


def test_gepa_instruction_update_moves_to_production_question(monkeypatch):
    monkeypatch.setattr("dspy.cache.get", lambda _request: None)
    monkeypatch.setattr("dspy.cache.put", lambda _request, _response: None)
    question_id = "triage.asks_recipient"
    client = CaptureClient(question_id)
    predictor = program(question_id, client)
    predictor.signature = predictor.signature.with_instructions("New wording.")

    predictor(**_row_inputs(question_id))

    assert client.calls[0][1][question_id]["instructions"] == "New wording."
