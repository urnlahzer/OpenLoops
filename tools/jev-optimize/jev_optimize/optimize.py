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
from .proposer import JevProposer
from .questions import SETS, SPECS

CLOSURE_FIELDS = (
    "obligation.title",
    "obligation.evidence_text",
    "later.paragraph_text",
    "later.from_user",
    "later.days_later",
)


def _inputs(row: DatasetRow) -> dict[str, Any]:
    return row.model_dump(exclude={"id", "set", "label", "source"})


def metric_for(question_id: str, *, feedback_includes_text: bool = False):
    """Return GEPA's score-plus-feedback metric; owner exports keep feedback text-free."""

    def metric(
        example: Any,
        prediction: Any,
        trace: Any = None,
        pred_name: Any = None,
        pred_trace: Any = None,
    ) -> dspy.Prediction:
        # GEPA calls the metric with five positional arguments.
        del trace, pred_name, pred_trace
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


def gated_question_update(
    question_id: str,
    *,
    baseline_validation: dict[str, Any],
    tuned_validation: dict[str, Any],
    baseline_test: dict[str, Any],
    tuned_test: dict[str, Any],
    tuned_instructions: str,
) -> dict[str, Any]:
    """Keep tuned wording only when validation and held-out accuracy do not regress."""

    baseline_validation_score = float(baseline_validation["accuracy"])
    tuned_validation_score = float(tuned_validation["accuracy"])
    baseline_test_accuracy = float(baseline_test["accuracy"])
    tuned_test_accuracy = float(tuned_test["accuracy"])
    keep_tuned = (
        tuned_validation_score >= baseline_validation_score
        and tuned_test_accuracy >= baseline_test_accuracy
    )
    selected_validation = tuned_validation if keep_tuned else baseline_validation
    thresholds = choose_thresholds(selected_validation["sweep"])
    update: dict[str, Any] = {
        "instructions": (
            tuned_instructions
            if keep_tuned
            else (SPECS[question_id].signature.instructions or "").strip()
        ),
        "accept": thresholds["accept"],
        "escalate": thresholds["escalate"],
        "insufficient_data": thresholds["insufficient_data"],
        "baseline_validation_score": baseline_validation_score,
        "tuned_validation_score": tuned_validation_score,
        "baseline_test_metrics": baseline_test,
        "tuned_test_metrics": tuned_test,
    }
    if not keep_tuned:
        reasons = []
        if tuned_validation_score < baseline_validation_score:
            reasons.append("tuned validation score regressed")
        if tuned_test_accuracy < baseline_test_accuracy:
            reasons.append("tuned test accuracy regressed")
        update["kept_baseline"] = True
        update["baseline_reason"] = "; ".join(reasons)
    if SPECS[question_id].options:
        update["options"] = SPECS[question_id].options
    return update


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
        "proposer_rejection_count": 0,
    }
    for question_id in SETS[set_name]:
        program = adapter.program(question_id)
        baseline_validation = evaluate_question(program, question_id, parts["validation"])
        baseline_test = evaluate_question(program, question_id, parts["test"])
        fields = (
            CLOSURE_FIELDS
            if set_name == "closure"
            else tuple(SPECS[question_id].signature.input_fields)
        )
        proposer = JevProposer(reflection_lm, fields)
        # GEPA checkpoints its state under log_dir and resumes from it, so an
        # interrupted run keeps the candidates it already scored; delete the
        # directory to start a question over.
        log_dir = Path("runs") / "gepa" / set_name / question_id.replace(".", "-")
        log_dir.mkdir(parents=True, exist_ok=True)
        optimizer = dspy.GEPA(
            metric=metric_for(question_id, feedback_includes_text=feedback_includes_text),
            auto=budget,
            reflection_lm=reflection_lm,
            instruction_proposer=proposer,
            log_dir=str(log_dir),
            track_stats=True,
        )
        optimized = optimizer.compile(
            program,
            trainset=_examples(parts["train"]),
            valset=_examples(parts["validation"]),
        )
        tuned_test = evaluate_question(optimized, question_id, parts["test"])
        tuned_validation = evaluate_question(optimized, question_id, parts["validation"])
        update = gated_question_update(
            question_id,
            baseline_validation=baseline_validation,
            tuned_validation=tuned_validation,
            baseline_test=baseline_test,
            tuned_test=tuned_test,
            tuned_instructions=optimized.predict.signature.instructions,
        )
        update["proposer_rejection_count"] = proposer.rejection_count
        output["proposer_rejection_count"] += proposer.rejection_count
        output["test_metrics"][question_id] = (
            baseline_test if update.get("kept_baseline") else tuned_test
        )
        output["questions"][question_id] = update
    return output
