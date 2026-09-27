from types import SimpleNamespace

import dspy
import pytest
from dspy.experimental import Noul

from jev_optimize.metrics import ACCEPT_FLOOR, ESCALATE_FLOOR, threshold_sweep
from jev_optimize.optimize import (
    _reanchor_thresholds,
    gated_question_update,
    optimize_set,
)
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


def test_every_question_has_one_native_decision_output():
    for spec in SPECS.values():
        assert list(spec.signature.output_fields) == ["decision"]
        if spec.kind == "noul":
            assert spec.signature.output_fields["decision"].annotation is Noul


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
    labels = [True] * 10 + [False] * 10
    probabilities = [0.9] * 10 + [0.1] * 10
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


def test_result_gate_runs_reanchor_on_the_calibration_population_when_given(monkeypatch):
    # Validation alone has too few positives (10) for the chooser; the
    # calibration sweep (train + validation) has enough and is used instead.
    thin = {
        "accuracy": 0.9,
        "sweep": threshold_sweep([True] * 10 + [False] * 30, [0.9] * 10 + [0.1] * 30),
    }
    calibration = {
        "accuracy": 0.9,
        "sweep": threshold_sweep([True] * 60 + [False] * 60, [0.9] * 60 + [0.1] * 60),
        "program": object(),
        "trainset": [object()],
        "valset": [object()],
    }
    seen = []

    def fake_reanchor(question_id, predictor, trainset, valset):
        seen.append((question_id, predictor, trainset, valset))
        return {"accept": 0.8, "escalate": 0.25, "insufficient_data": False}

    monkeypatch.setattr("jev_optimize.optimize._reanchor_thresholds", fake_reanchor)
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
    assert with_calibration["accept"] == 0.8
    assert with_calibration["escalate"] == 0.25
    assert len(seen) == 1


def test_reanchor_uses_mirrored_heavy_penalties_and_reads_wrapped_predict(monkeypatch):
    metrics = []
    thresholds = iter((0.8, 0.25))

    class FakeReAnchor:
        def __init__(self, metric):
            metrics.append(metric)

        def compile(self, program, *, trainset, valset):
            assert program.predict is predictor
            assert trainset and valset
            return SimpleNamespace(
                predict=SimpleNamespace(fields={"decision": {"threshold": next(thresholds)}})
            )

    monkeypatch.setattr("jev_optimize.optimize.ReAnchor", FakeReAnchor)
    predictor = object()
    examples = [
        dspy.Example(label={"closure.fulfilled": value}) for value in [True] * 20 + [False] * 20
    ]
    result = _reanchor_thresholds(
        "closure.fulfilled", predictor, examples[:20], examples[20:]
    )
    true_prediction = SimpleNamespace(decision=SimpleNamespace(value=True))
    false_prediction = SimpleNamespace(decision=SimpleNamespace(value=False))
    false_example = dspy.Example(label={"closure.fulfilled": False})
    true_example = dspy.Example(label={"closure.fulfilled": True})
    assert metrics[0](false_example, true_prediction) == -9.0
    assert metrics[0](true_example, false_prediction) == 0.0
    assert metrics[1](false_example, true_prediction) == 0.0
    assert metrics[1](true_example, false_prediction) == -9.0
    assert result == {"accept": 0.8, "escalate": 0.25, "insufficient_data": False}


def test_reanchor_defaults_to_dspy_threshold_when_calibration_restores_empty_fields(
    monkeypatch,
):
    class EmptyFieldsReAnchor:
        def __init__(self, _metric):
            pass

        def compile(self, _program, *, trainset, valset):
            assert trainset and valset
            return SimpleNamespace(predict=SimpleNamespace(fields={}))

    monkeypatch.setattr("jev_optimize.optimize.ReAnchor", EmptyFieldsReAnchor)
    examples = [
        dspy.Example(label={"closure.fulfilled": value})
        for value in [True] * 20 + [False] * 20
    ]

    result = _reanchor_thresholds(
        "closure.fulfilled", object(), examples[:20], examples[20:]
    )

    expected_accept = max(0.5, ACCEPT_FLOOR)
    expected_escalate = max(0.5, ESCALATE_FLOOR)
    if expected_escalate >= expected_accept:
        expected_escalate = max(ESCALATE_FLOOR, round(expected_accept - 0.1, 2))
    assert result == {
        "accept": expected_accept,
        "escalate": expected_escalate,
        "insufficient_data": False,
    }


def test_optimize_set_calibrates_baseline_predictor_when_tuned_wording_is_rejected(
    monkeypatch, tmp_path
):
    question_id = "closure.fulfilled"
    baseline = SimpleNamespace(signature=SimpleNamespace(instructions="baseline"))
    optimized = SimpleNamespace(signature=SimpleNamespace(instructions="tuned"))
    parts = {"train": [object()], "validation": [object()], "test": [object()]}
    evaluation_calls = []
    evaluation_results = iter(
        (
            {"accuracy": 0.9},
            {"accuracy": 0.9},
            {"accuracy": 0.8},
            {"accuracy": 0.9},
            {
                "accuracy": 0.9,
                "sweep": threshold_sweep(
                    [True] * 20 + [False] * 20,
                    [0.9] * 20 + [0.1] * 20,
                ),
            },
        )
    )

    def fake_evaluate(program, _question_id, _rows):
        evaluation_calls.append(program)
        return next(evaluation_results)

    class FakeGEPA:
        def __init__(self, **_kwargs):
            pass

        def compile(self, program, *, trainset, valset):
            assert program is baseline
            assert trainset and valset
            return optimized

    class FakeProposer:
        def __init__(self, *_args, **_kwargs):
            self.rejection_count = 0

    calibrated_programs = []

    def fake_reanchor(_question_id, predictor, _trainset, _valset):
        calibrated_programs.append(predictor)
        return {"accept": 0.7, "escalate": 0.3, "insufficient_data": False}

    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_REFLECTION_MODEL", "synthetic-model")
    monkeypatch.setattr("jev_optimize.optimize.SETS", {"test-set": (question_id,)})
    monkeypatch.setattr("jev_optimize.optimize.load_jsonl", lambda _path: [object()])
    monkeypatch.setattr("jev_optimize.optimize.split_rows", lambda _rows, **_kwargs: parts)
    monkeypatch.setattr("jev_optimize.optimize.make_program", lambda *_args: baseline)
    monkeypatch.setattr("jev_optimize.optimize.evaluate_question", fake_evaluate)
    monkeypatch.setattr("jev_optimize.optimize._examples", lambda rows, _question_id: rows)
    monkeypatch.setattr("jev_optimize.optimize.JevProposer", FakeProposer)
    monkeypatch.setattr("jev_optimize.optimize.dspy.LM", lambda *_args, **_kwargs: object())
    monkeypatch.setattr("jev_optimize.optimize.dspy.GEPA", FakeGEPA)
    monkeypatch.setattr("jev_optimize.optimize._reanchor_thresholds", fake_reanchor)

    result = optimize_set("test-set", SimpleNamespace(model="test-model"), "unused.jsonl")

    assert evaluation_calls[-1] is baseline
    assert calibrated_programs == [baseline]
    assert result["questions"][question_id]["kept_baseline"] is True


def test_choice_gate_keeps_using_confidence_sweep():
    metrics = {
        "accuracy": 0.9,
        "sweep": threshold_sweep([True] * 20 + [False] * 20, [0.9] * 20 + [0.1] * 20),
    }
    update = gated_question_update(
        "closure.outcome",
        baseline_validation=metrics,
        tuned_validation=metrics,
        baseline_test={"accuracy": 0.9},
        tuned_test={"accuracy": 0.9},
        tuned_instructions="later.paragraph_text states one obligation outcome.",
        calibration=metrics,
    )
    assert update["accept"] == 0.5
    assert update["escalate"] == 0.45
