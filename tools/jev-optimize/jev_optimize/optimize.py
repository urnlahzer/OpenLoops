"""DSPy GEPA orchestration for Jev question sets."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import dspy

from .adapter import JevPrograms
from .data import DatasetRow, load_jsonl, split_rows
from .metrics import (
    accuracy,
    brier_score,
    choose_thresholds,
    expected_calibration_error,
    threshold_sweep,
)
from .questions import SETS, SPECS


def _inputs(row: DatasetRow) -> dict[str, Any]:
    return row.model_dump(exclude={"id", "set", "label", "source"})


def metric_for(question_id: str, *, feedback_includes_text: bool = False):
    """Return GEPA's score-plus-feedback metric; owner exports keep feedback text-free."""

    def metric(example: Any, prediction: Any, trace: Any = None) -> dspy.Prediction:
        del trace
        label = example.label[question_id]
        if isinstance(label, bool):
            probability = float(prediction.probability)
            correct = (probability >= 0.5) == label
            feedback = f"label={str(label).lower()}; predicted_probability={probability:.3f}."
        else:
            correct = prediction.choice == label
            probability = float(prediction.probabilities.get(label, 0.0))
            feedback = f"label={label}; predicted_label_probability={probability:.3f}."
        if feedback_includes_text and hasattr(example, "paragraph_text"):
            feedback += f" Synthetic/public context: {example.paragraph_text}"
        return dspy.Prediction(score=float(correct), feedback=feedback)

    return metric


def _examples(rows: list[DatasetRow]) -> list[dspy.Example]:
    examples = []
    for row in rows:
        values = _inputs(row) | {"label": row.label, "source": row.source}
        examples.append(dspy.Example(**values).with_inputs(*_inputs(row)))
    return examples


def evaluate_question(program: Any, question_id: str, rows: list[DatasetRow]) -> dict[str, Any]:
    labels: list[bool] = []
    probabilities: list[float] = []
    multiclass_brier: list[float] = []
    for row in rows:
        prediction = program(**_inputs(row))
        label = row.label[question_id]
        if isinstance(label, bool):
            labels.append(label)
            probabilities.append(float(prediction.probability))
        else:
            correct = prediction.choice == label
            labels.append(correct)
            probabilities.append(float(prediction.confidence))
            multiclass_brier.append(
                sum(
                    (
                        float(prediction.probabilities.get(option, 0.0))
                        - float(option == label)
                    )
                    ** 2
                    for option in (SPECS[question_id].options or {})
                )
            )
    is_choice = question_id == "rules.deadline_kind"
    sweep = threshold_sweep(labels, probabilities)
    if is_choice:
        for row in sweep:
            below = [
                correct
                for correct, confidence in zip(labels, probabilities, strict=True)
                if confidence <= row["threshold"]
            ]
            row["below_error"] = (
                sum(not correct for correct in below) / len(below) if below else 0.0
            )
    result = {
        "accuracy": sum(labels) / len(labels)
        if is_choice and labels
        else accuracy(labels, probabilities),
        "brier": brier_score(labels, probabilities),
        "ece_10": expected_calibration_error(labels, probabilities),
        "sweep": sweep,
    }
    if multiclass_brier:
        result["brier"] = sum(multiclass_brier) / len(multiclass_brier)
    return result


def optimize_set(
    set_name: str,
    client: Any,
    data_path: str | Path,
    *,
    budget: str = "light",
    feedback_includes_text: bool = False,
) -> dict[str, Any]:
    rows = load_jsonl(data_path)
    parts = split_rows(rows)
    reflection_model = os.environ.get("OPENROUTER_REFLECTION_MODEL")
    if not reflection_model:
        raise RuntimeError("OPENROUTER_REFLECTION_MODEL is not set")
    reflection_lm = dspy.LM(
        f"openrouter/{reflection_model}",
        api_base="https://openrouter.ai/api/v1",
        extra_body={"provider": {"zdr": True}},
    )
    adapter = JevPrograms(client)
    output: dict[str, Any] = {
        "model": getattr(client, "model", "typesafe/jev-1.13"),
        "dataset_sizes": {name: len(values) for name, values in parts.items()},
        "questions": {},
        "test_metrics": {},
    }
    for question_id in SETS[set_name]:
        program = adapter.program(question_id)
        optimizer = dspy.GEPA(
            metric=metric_for(question_id, feedback_includes_text=feedback_includes_text),
            auto=budget,
            reflection_lm=reflection_lm,
        )
        optimized = optimizer.compile(
            program,
            trainset=_examples(parts["train"]),
            valset=_examples(parts["validation"]),
        )
        output["test_metrics"][question_id] = evaluate_question(
            optimized, question_id, parts["test"]
        )
        validation = evaluate_question(optimized, question_id, parts["validation"])
        update = {"instructions": optimized.predict.signature.instructions}
        if "sweep" in validation:
            update["accept"], update["escalate"] = choose_thresholds(validation["sweep"])
        if SPECS[question_id].options:
            update["options"] = SPECS[question_id].options
        output["questions"][question_id] = update
    return output
