from jev_optimize.data import DatasetRow, split_name, split_rows
from jev_optimize.metrics import (
    accuracy,
    brier_score,
    choose_thresholds,
    expected_calibration_error,
    threshold_sweep,
)


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
