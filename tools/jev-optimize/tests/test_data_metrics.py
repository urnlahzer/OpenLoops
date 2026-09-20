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
    sweep = threshold_sweep([True, True, False, False], [0.95, 0.85, 0.65, 0.10])
    accept, escalate = choose_thresholds(sweep, max_selective_risk=0.05)
    assert accept == 0.7
    assert 0.0 <= escalate < accept
