"""DSPy GEPA orchestration for Jev question sets."""

from __future__ import annotations

import os
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import dspy
from dspy.experimental import ReAnchor

from .adapter import program as make_program
from .data import DatasetRow, load_jsonl, split_rows
from .metrics import (
    ACCEPT_FLOOR,
    ESCALATE_FLOOR,
    accuracy,
    brier_score,
    choose_thresholds,
    expected_calibration_error,
    threshold_sweep,
)
from .proposer import JevProposer
from .questions import SETS, SPECS

# One wrong automatic decision costs this many right ones: 9 targets ~10% error
# in the accepted and rejected bands (owner decision 2026-09-27; 19 gave ~5%
# and sent most rows of the weak rules questions to the chat model).
WRONG_DECISION_PENALTY = 9.0


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
        if question_id not in example.label:
            raise ValueError(f"example does not carry label for {question_id}")
        label = example.label[question_id]
        decision = prediction.decision
        if isinstance(label, bool):
            probability = float(decision.probability)
            correct = decision.value == label
            feedback = f"label={str(label).lower()}; predicted_probability={probability:.3f}."
        else:
            correct = decision.value == label
            probability = float(decision.probabilities.get(label, 0.0))
            feedback = f"label={label}; predicted_label_probability={probability:.3f}."
        if feedback_includes_text and hasattr(example, "paragraph_text"):
            feedback += f" Synthetic/public context: {example.paragraph_text}"
        return dspy.Prediction(score=float(correct), feedback=feedback)

    return metric


def _examples(rows: list[DatasetRow], question_id: str) -> list[dspy.Example]:
    examples = []
    for row in rows:
        if question_id not in row.label:
            continue
        values = _inputs(row) | {"label": row.label, "source": row.source}
        examples.append(dspy.Example(**values).with_inputs(*_inputs(row)))
    return examples


def evaluate_question(
    program: Any,
    question_id: str,
    rows: list[DatasetRow],
    *,
    registry_thresholds: dict[str, float] | None = None,
) -> dict[str, Any]:
    labels: list[bool] = []
    probabilities: list[float] = []
    multiclass_brier: list[float] = []
    selected = [row for row in rows if question_id in row.label]
    # The decisions are independent, so they are requested concurrently;
    # results come back in row order.
    with ThreadPoolExecutor(max_workers=8) as pool:
        predictions = list(pool.map(lambda row: program(**_inputs(row)), selected))
    for row, prediction in zip(selected, predictions, strict=True):
        label = row.label[question_id]
        decision = prediction.decision
        if isinstance(label, bool):
            labels.append(label)
            probabilities.append(float(decision.probability))
        else:
            correct = decision.value == label
            labels.append(correct)
            probabilities.append(float(decision.confidence))
            multiclass_brier.append(
                sum(
                    (
                        float(decision.probabilities.get(option, 0.0))
                        - float(option == label)
                    )
                    ** 2
                    for option in (SPECS[question_id].options or {})
                )
            )
    is_choice = SPECS[question_id].kind == "choice"
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
    if registry_thresholds:
        result["confusion"] = {}
        for name in ("accept", "escalate"):
            threshold = float(registry_thresholds[name])
            predictions = [probability >= threshold for probability in probabilities]
            result["confusion"][name] = {
                "threshold": threshold,
                "tp": sum(
                    actual and predicted
                    for actual, predicted in zip(labels, predictions, strict=True)
                ),
                "fp": sum(
                    not actual and predicted
                    for actual, predicted in zip(labels, predictions, strict=True)
                ),
                "tn": sum(
                    not actual and not predicted
                    for actual, predicted in zip(labels, predictions, strict=True)
                ),
                "fn": sum(
                    actual and not predicted
                    for actual, predicted in zip(labels, predictions, strict=True)
                ),
            }
    return result


class _ReAnchorProgram(dspy.Module):
    """Give ReAnchor a named predictor while the public factory stays a bare Predict."""

    def __init__(self, predictor: dspy.Predict) -> None:
        super().__init__()
        self.predict = predictor

    def forward(self, **kwargs: Any) -> dspy.Prediction:
        return self.predict(**kwargs)


def _reanchor_thresholds(
    question_id: str,
    predictor: dspy.Predict,
    trainset: list[dspy.Example],
    valset: list[dspy.Example],
) -> dict[str, float | bool]:
    labels = [example.label[question_id] for example in [*trainset, *valset]]
    positives = sum(label is True for label in labels)
    negatives = sum(label is False for label in labels)
    if positives < 20 or negatives < 20:
        return {"accept": 0.7, "escalate": 0.3, "insufficient_data": True}

    def metric(*, false_positive_penalty: bool):
        def score(example: dspy.Example, prediction: dspy.Prediction) -> float:
            actual = bool(example.label[question_id])
            predicted = bool(prediction.decision.value)
            if predicted == actual:
                return 1.0
            penalized = (
                predicted and not actual
                if false_positive_penalty
                else actual and not predicted
            )
            return -WRONG_DECISION_PENALTY if penalized else 0.0

        return score

    wrapped = _ReAnchorProgram(predictor)
    accept_program = ReAnchor(metric(false_positive_penalty=True)).compile(
        wrapped, trainset=trainset, valset=valset
    )
    escalate_program = ReAnchor(metric(false_positive_penalty=False)).compile(
        wrapped, trainset=trainset, valset=valset
    )
    fitted_accept = float(
        accept_program.predict.fields.get("decision", {}).get("threshold", 0.5)
    )
    fitted_escalate = float(
        escalate_program.predict.fields.get("decision", {}).get("threshold", 0.5)
    )
    accept = max(fitted_accept, ACCEPT_FLOOR)
    escalate = max(fitted_escalate, ESCALATE_FLOOR)
    if escalate >= accept:
        escalate = max(ESCALATE_FLOOR, round(accept - 0.1, 2))
    return {"accept": accept, "escalate": escalate, "insufficient_data": False}


def gated_question_update(
    question_id: str,
    *,
    baseline_validation: dict[str, Any],
    tuned_validation: dict[str, Any],
    baseline_test: dict[str, Any],
    tuned_test: dict[str, Any],
    tuned_instructions: str,
    calibration: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Keep tuned wording only when validation and held-out accuracy do not regress.

    Calibration uses train plus validation, which holds enough of each class
    where validation alone does not. Noul questions run two ReAnchor passes;
    Choice questions retain the confidence sweep. The held-out test split
    never feeds threshold choice.
    """

    baseline_validation_score = float(baseline_validation["accuracy"])
    tuned_validation_score = float(tuned_validation["accuracy"])
    baseline_test_accuracy = float(baseline_test["accuracy"])
    tuned_test_accuracy = float(tuned_test["accuracy"])
    keep_tuned = (
        tuned_validation_score >= baseline_validation_score
        and tuned_test_accuracy >= baseline_test_accuracy
    )
    selected_validation = tuned_validation if keep_tuned else baseline_validation
    calibration_data = calibration or selected_validation
    if SPECS[question_id].kind == "choice":
        thresholds = choose_thresholds(calibration_data["sweep"])
    else:
        sweep = calibration_data["sweep"]
        positives = int(sweep[0].get("positives", 0)) if sweep else 0
        negatives = int(sweep[0].get("negatives", 0)) if sweep else 0
        if positives < 20 or negatives < 20:
            thresholds = {"accept": 0.7, "escalate": 0.3, "insufficient_data": True}
        else:
            thresholds = _reanchor_thresholds(
                question_id,
                calibration_data["program"],
                calibration_data["trainset"],
                calibration_data["valset"],
            )
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
    reflection_lm: Any | None = None,
) -> dict[str, Any]:
    rows = load_jsonl(data_path)
    if reflection_lm is None:
        reflection_model = os.environ.get("OPENROUTER_REFLECTION_MODEL")
        if not reflection_model:
            raise RuntimeError("OPENROUTER_REFLECTION_MODEL is not set")
        reflection_lm = dspy.LM(
            f"openrouter/{reflection_model}",
            api_base="https://openrouter.ai/api/v1",
            extra_body={"provider": {"zdr": True}},
        )
    output: dict[str, Any] = {
        "model": getattr(client, "model", "typesafe/jev-1.13"),
        "dataset_sizes": {},
        "questions": {},
        "test_metrics": {},
        "proposer_rejection_count": 0,
    }
    for question_id in SETS[set_name]:
        parts = split_rows(rows, question_id=question_id)
        output["dataset_sizes"][question_id] = {
            name: len(values) for name, values in parts.items()
        }
        program = make_program(question_id, client)
        baseline_validation = evaluate_question(program, question_id, parts["validation"])
        baseline_test = evaluate_question(program, question_id, parts["test"])
        spec = SPECS[question_id]
        proposer = JevProposer(
            reflection_lm,
            spec.allowed_fields,
            intent=spec.intent,
            primary_fields=spec.primary_fields,
        )
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
            trainset=_examples(parts["train"], question_id),
            valset=_examples(parts["validation"], question_id),
        )
        tuned_test = evaluate_question(optimized, question_id, parts["test"])
        tuned_validation = evaluate_question(optimized, question_id, parts["validation"])
        keep_tuned = (
            tuned_validation["accuracy"] >= baseline_validation["accuracy"]
            and tuned_test["accuracy"] >= baseline_test["accuracy"]
        )
        shipping_program = optimized if keep_tuned else program
        # Thresholds are chosen on train + validation for the wording that
        # will ship, so the sweep holds enough of each class; test stays out.
        calibration = evaluate_question(
            shipping_program,
            question_id,
            [*parts["train"], *parts["validation"]],
        )
        calibration.update(
            {
                # ReAnchor must calibrate the predictor whose wording ships,
                # using the same keep_tuned gate as the accuracy numbers.
                "program": shipping_program,
                "trainset": _examples(parts["train"], question_id),
                "valset": _examples(parts["validation"], question_id),
            }
        )
        update = gated_question_update(
            question_id,
            baseline_validation=baseline_validation,
            tuned_validation=tuned_validation,
            baseline_test=baseline_test,
            tuned_test=tuned_test,
            tuned_instructions=optimized.signature.instructions,
            calibration=calibration,
        )
        update["calibration_rows"] = len(parts["train"]) + len(parts["validation"])
        update["proposer_rejection_count"] = proposer.rejection_count
        output["proposer_rejection_count"] += proposer.rejection_count
        output["test_metrics"][question_id] = (
            baseline_test if update.get("kept_baseline") else tuned_test
        )
        output["questions"][question_id] = update
    return output
