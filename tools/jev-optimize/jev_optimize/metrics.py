"""Calibration and selective-classification metrics."""

from __future__ import annotations

from typing import Any

import numpy as np


def accuracy(labels: list[bool], probabilities: list[float], threshold: float = 0.5) -> float:
    if not labels:
        return 0.0
    predicted = np.asarray(probabilities) >= threshold
    return float(np.mean(predicted == np.asarray(labels, dtype=bool)))


def brier_score(labels: list[bool], probabilities: list[float]) -> float:
    if not labels:
        return 0.0
    return float(np.mean((np.asarray(probabilities) - np.asarray(labels, dtype=float)) ** 2))


def expected_calibration_error(
    labels: list[bool], probabilities: list[float], bins: int = 10
) -> float:
    if not labels:
        return 0.0
    truth = np.asarray(labels, dtype=float)
    probs = np.asarray(probabilities, dtype=float)
    total = len(labels)
    ece = 0.0
    for lower in np.linspace(0.0, 1.0, bins, endpoint=False):
        upper = lower + 1.0 / bins
        mask = (probs >= lower) & (probs <= upper if upper >= 1.0 else probs < upper)
        if mask.any():
            ece += float(mask.sum() / total * abs(probs[mask].mean() - truth[mask].mean()))
    return ece


def threshold_sweep(labels: list[bool], probabilities: list[float]) -> list[dict[str, Any]]:
    truth = np.asarray(labels, dtype=bool)
    probs = np.asarray(probabilities, dtype=float)
    rows: list[dict[str, Any]] = []
    for step in range(21):
        threshold = step * 0.05
        selected = probs >= threshold
        count = int(selected.sum())
        wrong = int(np.logical_not(truth[selected]).sum()) if count else 0
        rejected = probs <= threshold
        rejected_count = int(rejected.sum())
        rejected_wrong = int(truth[rejected].sum()) if rejected_count else 0
        rows.append(
            {
                "threshold": round(threshold, 2),
                "coverage": count / len(labels) if labels else 0.0,
                "selective_risk": wrong / count if count else 0.0,
                "selected": count,
                "below_coverage": rejected_count / len(labels) if labels else 0.0,
                "below_error": rejected_wrong / rejected_count if rejected_count else 0.0,
            }
        )
    return rows


def choose_thresholds(
    sweep: list[dict[str, Any]], max_selective_risk: float = 0.05
) -> tuple[float, float]:
    acceptable = [
        row["threshold"]
        for row in sweep
        if row.get("selected", 0) > 0 and row["selective_risk"] <= max_selective_risk
    ]
    accept = max(0.05, min(acceptable, default=1.0))
    mostly_wrong = [
        row["threshold"]
        for row in sweep
        if row.get("below_coverage", 0) > 0
        and row["threshold"] < accept
        and row.get("below_error", 0.0) > 0.5
    ]
    escalate = max(mostly_wrong, default=0.0)
    if escalate >= accept:
        escalate = max(0.0, round(accept - 0.05, 2))
    return float(accept), float(escalate)
