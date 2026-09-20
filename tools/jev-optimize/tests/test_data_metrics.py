from types import SimpleNamespace

from jev_optimize.data import DatasetRow, split_name, split_rows
from jev_optimize.metrics import (
    accuracy,
    brier_score,
    choose_thresholds,
    expected_calibration_error,
    threshold_sweep,
)
from jev_optimize.optimize import evaluate_question


def test_split_is_deterministic_and_complete():
    rows = [
        DatasetRow(id=f"row-{index}", set="triage", label={"q": True}, source="synthetic")
        for index in range(1000)
    ]
    first = split_rows(rows, seed=7)
    second = split_rows(reversed(rows), seed=7)
    assert {name: sorted(row.id for row in values) for name, values in first.items()} == {
        name: sorted(row.id for row in values) for name, values in second.items()
    }
    assert split_name("stable", 7) == split_name("stable", 7)
    assert sum(map(len, first.values())) == 1000


def test_split_filters_rows_for_one_question():
    rows = [
        DatasetRow(id="one", set="triage", label={"q.one": True}, source="synthetic"),
        DatasetRow(id="two", set="triage", label={"q.two": False}, source="synthetic"),
    ]
    parts = split_rows(rows, question_id="q.one")
    assert [row.id for values in parts.values() for row in values] == ["one"]


def test_metrics_on_toy_set():
    labels = [True, True, False, False]
    probabilities = [0.9, 0.8, 0.2, 0.1]
    assert accuracy(labels, probabilities) == 1.0
    assert abs(brier_score(labels, probabilities) - 0.025) < 1e-12
    assert abs(expected_calibration_error(labels, probabilities) - 0.15) < 1e-12
    sweep = threshold_sweep(labels, probabilities)
    assert len(sweep) == 21
    assert sweep[10]["coverage"] == 0.5
    assert sweep[10]["selective_risk"] == 0.0


def test_threshold_chooser_obeys_risk_and_orders_thresholds():
    labels = [True] * 20 + [False] * 20
    probabilities = [0.9] * 20 + [0.1] * 20
    result = choose_thresholds(threshold_sweep(labels, probabilities))
    assert result == {"accept": 0.15, "escalate": 0.1, "insufficient_data": False}


def test_threshold_chooser_uses_defaults_for_insufficient_support():
    result = choose_thresholds(threshold_sweep([True] * 19 + [False] * 20, [0.5] * 39))
    assert result == {"accept": 0.7, "escalate": 0.3, "insufficient_data": True}


def test_threshold_chooser_enforces_positive_recall_floor():
    labels = [True] * 20 + [False] * 20
    probabilities = [0.9] * 9 + [0.4] * 11 + [0.45] * 20
    result = choose_thresholds(threshold_sweep(labels, probabilities))
    assert result["accept"] == 0.7
    assert result["escalate"] == 0.4
    assert result["insufficient_data"] is False


def test_evaluate_reports_confusion_at_registry_thresholds():
    probabilities = iter((0.9, 0.6, 0.4, 0.1))

    def program(**_inputs):
        return SimpleNamespace(probability=next(probabilities))

    rows = [
        DatasetRow(
            id=f"row-{index}",
            set="triage",
            subject="Synthetic subject",
            paragraph_text=f"Synthetic paragraph {index}.",
            from_user=bool(index % 2),
            label={"triage.asks_recipient": label},
            source="synthetic-stub",
        )
        for index, label in enumerate((True, True, False, False))
    ]
    result = evaluate_question(
        program,
        "triage.asks_recipient",
        rows,
        registry_thresholds={"accept": 0.7, "escalate": 0.3},
    )
    assert result["confusion"] == {
        "accept": {"threshold": 0.7, "tp": 1, "fp": 0, "tn": 2, "fn": 1},
        "escalate": {"threshold": 0.3, "tp": 2, "fp": 1, "tn": 1, "fn": 0},
    }


def test_evaluate_skips_rows_without_the_question_label():
    rows = [
        DatasetRow(
            id="other", set="triage", subject="Other", paragraph_text="Other text.",
            from_user=False, label={"triage.names_time": True}, source="synthetic-stub",
        ),
        DatasetRow(
            id="target", set="triage", subject="Target", paragraph_text="Target text.",
            from_user=True, label={"triage.asks_recipient": True}, source="synthetic-stub",
        ),
    ]
    calls = []

    def program(**inputs):
        calls.append(inputs)
        return SimpleNamespace(probability=0.9)

    result = evaluate_question(program, "triage.asks_recipient", rows)
    assert result["accuracy"] == 1.0
    assert len(calls) == 1
