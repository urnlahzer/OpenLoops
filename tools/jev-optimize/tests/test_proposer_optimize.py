from types import SimpleNamespace

import pytest

from jev_optimize.metrics import threshold_sweep
from jev_optimize.optimize import gated_question_update
from jev_optimize.proposer import JevProposer, validate_statement
from jev_optimize.questions import SPECS


def test_proposer_validator_accepts_literal_statement():
    statement = "later.paragraph_text states that the obligation is complete."
    assert validate_statement(
        statement,
        allowed_fields=("later.paragraph_text",),
        primary_fields=("later.paragraph_text",),
    ) == []


def test_every_question_declares_intent_and_field_contract():
    assert all(spec.intent for spec in SPECS.values())
    assert all(spec.primary_fields for spec in SPECS.values())
    assert all(set(spec.primary_fields) <= set(spec.allowed_fields) for spec in SPECS.values())


@pytest.mark.parametrize(
    ("statement", "expected"),
    [
        ("word " * 46, "maximum"),
        ("The state is clear.\n- Check the next item.", "newline or list"),
        ("The state contains 10 entries.", "digits-only"),
        ("Northstar is complete.", "capitalized example"),
        ("Output the answer.", "banned phrase"),
        ("You inspect later.paragraph_text.", "second-person"),
        ("The text is not absent and not unclear.", "stacks negation"),
        ("One is true. Two is true. Three is true.", "more than two"),
        ("Is later.paragraph_text complete?", "not declarative"),
    ],
)
def test_proposer_validator_rejects_each_violation(statement, expected):
    violations = validate_statement(
        statement,
        current_statement="The current statement is literal.",
        example_texts=("The Northstar example appears here.",),
    )
    assert any(expected in violation for violation in violations)


def test_proposer_retries_once_then_falls_back():
    responses = iter(("Output a probability.", "You are an assistant."))

    def predictor(**_kwargs):
        return SimpleNamespace(proposed_statement=next(responses))

    proposer = JevProposer(
        None,
        ("later.paragraph_text",),
        intent="The later paragraph shows the obligation was carried out.",
        primary_fields=("later.paragraph_text",),
        predictor=predictor,
    )
    current = "later.paragraph_text states that the obligation is complete."
    result = proposer(
        {"predict": current},
        {"predict": [{"Feedback": "label=true; predicted_probability=0.100."}]},
        ["predict"],
    )
    assert result == {"predict": current}
    assert proposer.rejection_count == 2


def test_proposer_passes_fixed_intent_and_field_contract():
    calls = []

    def predictor(**kwargs):
        calls.append(kwargs)
        return SimpleNamespace(
            proposed_statement="later.paragraph_text shows the obligation was carried out."
        )

    proposer = JevProposer(
        None,
        ("obligation.evidence_text", "later.paragraph_text"),
        intent="The later paragraph shows the obligation was carried out.",
        primary_fields=("later.paragraph_text",),
        predictor=predictor,
    )
    result = proposer(
        {"predict": "later.paragraph_text shows completion."}, {}, ["predict"]
    )
    assert result["predict"].startswith("later.paragraph_text")
    assert calls[0]["intent"] == proposer.intent
    assert calls[0]["primary_fields"] == "later.paragraph_text"


@pytest.mark.parametrize(
    ("statement", "expected"),
    [
        ("obligation.evidence_text shows completion.", "required primary"),
        ("later.paragraph_text and sender show completion.", "disallowed fields"),
    ],
)
def test_proposer_validator_enforces_field_contract(statement, expected):
    violations = validate_statement(
        statement,
        allowed_fields=("obligation.evidence_text", "later.paragraph_text"),
        primary_fields=("later.paragraph_text",),
    )
    assert any(expected in violation for violation in violations)


def _metrics(accuracy):
    labels = [True] * 20 + [False] * 20
    probabilities = [0.9] * 20 + [0.1] * 20
    return {"accuracy": accuracy, "sweep": threshold_sweep(labels, probabilities)}


def test_result_gate_keeps_baseline_when_test_accuracy_regresses():
    update = gated_question_update(
        "closure.fulfilled",
        baseline_validation=_metrics(0.8),
        tuned_validation=_metrics(0.9),
        baseline_test={"accuracy": 0.85},
        tuned_test={"accuracy": 0.84},
        tuned_instructions="later.paragraph_text states that the obligation is done.",
    )
    assert update["kept_baseline"] is True
    assert update["instructions"] == "The later text shows the obligation has been carried out."
    assert "test accuracy" in update["baseline_reason"]


def test_result_gate_keeps_non_regressing_tuned_statement():
    tuned = "later.paragraph_text states that the obligation is complete."
    update = gated_question_update(
        "closure.fulfilled",
        baseline_validation=_metrics(0.8),
        tuned_validation=_metrics(0.8),
        baseline_test={"accuracy": 0.85},
        tuned_test={"accuracy": 0.86},
        tuned_instructions=tuned,
    )
    assert update["instructions"] == tuned
    assert "kept_baseline" not in update


def test_result_gate_takes_thresholds_from_the_calibration_sweep_when_given():
    # Validation alone has too few positives (10) for the chooser; the
    # calibration sweep (train + validation) has enough and is used instead.
    thin = {
        "accuracy": 0.9,
        "sweep": threshold_sweep([True] * 10 + [False] * 30, [0.9] * 10 + [0.1] * 30),
    }
    calibration = {
        "accuracy": 0.9,
        "sweep": threshold_sweep([True] * 60 + [False] * 60, [0.9] * 60 + [0.1] * 60),
    }
    without = gated_question_update(
        "closure.fulfilled",
        baseline_validation=thin,
        tuned_validation=thin,
        baseline_test={"accuracy": 0.85},
        tuned_test={"accuracy": 0.85},
        tuned_instructions="later.paragraph_text states that the obligation is complete.",
    )
    assert without["insufficient_data"] is True
    with_calibration = gated_question_update(
        "closure.fulfilled",
        baseline_validation=thin,
        tuned_validation=thin,
        baseline_test={"accuracy": 0.85},
        tuned_test={"accuracy": 0.85},
        tuned_instructions="later.paragraph_text states that the obligation is complete.",
        calibration=calibration,
    )
    assert with_calibration["insufficient_data"] is False
    assert with_calibration["accept"] >= 0.5 > with_calibration["escalate"]
