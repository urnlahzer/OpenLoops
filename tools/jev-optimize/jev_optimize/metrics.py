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
    positive_count = int(truth.sum())
    negative_count = len(labels) - positive_count
    for step in range(21):
        threshold = step * 0.05
        selected = probs >= threshold
        count = int(selected.sum())
        wrong = int(np.logical_not(truth[selected]).sum()) if count else 0
        rejected = probs < threshold
        rejected_count = int(rejected.sum())
        rejected_wrong = int(truth[rejected].sum()) if rejected_count else 0
        selected_positives = int(truth[selected].sum()) if count else 0
        rows.append(
            {
                "threshold": round(threshold, 2),
                "coverage": count / len(labels) if labels else 0.0,
                "selective_risk": wrong / count if count else 0.0,
                "selected": count,
                "below_coverage": rejected_count / len(labels) if labels else 0.0,
                "below_error": rejected_wrong / rejected_count if rejected_count else 0.0,
                "positive_coverage": (
                    selected_positives / positive_count if positive_count else 0.0
                ),
                "positive_below_fraction": (
                    rejected_wrong / positive_count if positive_count else 0.0
                ),
                "positives": positive_count,
                "negatives": negative_count,
            }
        )
    return rows


# A noul below 0.5 leans false; accepting it as true would be wrong whatever
# the sweep says. Escalation covers the band between the floors.
ACCEPT_FLOOR = 0.5
ESCALATE_FLOOR = 0.2


def choose_thresholds(
    sweep: list[dict[str, Any]], max_selective_risk: float = 0.05
) -> dict[str, float | bool]:
    """Choose guarded accept/escalate thresholds from a validation sweep.

    At least 20 positive and 20 negative examples are required; otherwise the registry
    defaults (0.70, 0.30) are returned with ``insufficient_data``. The accept threshold is
    the smallest threshold of at least ``ACCEPT_FLOOR`` whose selective risk is within the
    limit and whose positive recall is at least 0.5: a probability below 0.5 means the
    model leans false, so it can never count as acceptance however clean a validation
    sweep looks. Escalate is the largest lower threshold of at least ``ESCALATE_FLOOR``
    with at most 0.1 of all positives below it. Fallbacks preserve ``escalate < accept``.
    """

    positives = int(sweep[0].get("positives", 0)) if sweep else 0
    negatives = int(sweep[0].get("negatives", 0)) if sweep else 0
    if positives < 20 or negatives < 20:
        return {"accept": 0.7, "escalate": 0.3, "insufficient_data": True}
    acceptable = [
        row["threshold"]
        for row in sweep
        if row["threshold"] >= ACCEPT_FLOOR
        and row.get("selected", 0) > 0
        and row["selective_risk"] <= max_selective_risk
        and row.get("positive_coverage", 0.0) >= 0.5
    ]
    accept = float(min(acceptable, default=0.7))
    low_positive_tail = [
        row["threshold"]
        for row in sweep
        if ESCALATE_FLOOR <= row["threshold"] < accept
        and row.get("positive_below_fraction", 0.0) <= 0.1
    ]
    escalate = float(max(low_positive_tail, default=max(ESCALATE_FLOOR, accept - 0.4)))
    if escalate >= accept:
        escalate = max(ESCALATE_FLOOR, round(accept - 0.1, 2))
    return {"accept": accept, "escalate": escalate, "insufficient_data": False}
