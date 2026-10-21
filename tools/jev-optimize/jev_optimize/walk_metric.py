"""Thread-level status metrics for the closure walk harness."""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from .data import split_name
from .threads import BRANCH_IDS, MessagePlan, ThreadPlan, branch_labels

_AUTO_CLOSED = {
    "wp_received": "closed:completed",
    "wp_no_longer_needed": "closed:withdrawn",
    "wp_changes_terms_ends": "closed:withdrawn",
}
_ROLE_IDS = {
    "waiting_party": (
        "walk.wp_received",
        "walk.wp_no_longer_needed",
        "walk.wp_asks_again",
        "walk.wp_changes_terms",
        "walk.wp_changes_due",
    ),
    "user": BRANCH_IDS["user"],
    "third_party": BRANCH_IDS["third_party"],
}
_FALLBACK_THRESHOLDS = {
    "walk.responds": {"accept": 0.7, "escalate": 0.3},
    "walk.wp_received": {"accept": 0.9, "escalate": 0.5},
    "walk.wp_no_longer_needed": {"accept": 0.9, "escalate": 0.5},
    "walk.wp_asks_again": {"accept": 0.7, "escalate": 0.3},
    "walk.wp_changes_terms": {"accept": 0.7, "escalate": 0.3},
    "walk.wp_still_owed": {"accept": 0.9, "escalate": 0.5},
    "walk.wp_changes_due": {"accept": 0.7, "escalate": 0.3},
    "walk.user_delivers": {"accept": 0.7, "escalate": 0.3},
    "walk.user_promises": {"accept": 0.7, "escalate": 0.3},
    "walk.user_declines": {"accept": 0.7, "escalate": 0.3},
    "walk.user_hands_off": {"accept": 0.7, "escalate": 0.3},
    "walk.third_party_done": {"accept": 0.7, "escalate": 0.3},
    "walk.new_date": {"accept": 0.7, "escalate": 0.3},
}


def _threshold(thresholds: dict[str, dict[str, float]], question_id: str) -> dict[str, float]:
    return thresholds.get(question_id, _FALLBACK_THRESHOLDS[question_id])


def _band(
    question_id: str, probability: float, thresholds: dict[str, dict[str, float]]
) -> str:
    values = _threshold(thresholds, question_id)
    if probability >= values["accept"]:
        return "accept"
    if probability >= values["escalate"]:
        return "gray"
    return "reject"


def _new_date_status(answer: Any, thresholds: dict[str, dict[str, float]]) -> str:
    if answer is None:
        return "missing"
    if isinstance(answer, str):
        return "none" if not answer.strip() or answer.strip().casefold() == "none" else "selected"
    if isinstance(answer, int | float):
        band = _band("walk.new_date", float(answer), thresholds)
        return "selected" if band == "accept" else "gray" if band == "gray" else "none"
    if isinstance(answer, dict):
        choice = answer.get("choice")
        confidence = answer.get("confidence")
        probabilities = answer.get("probabilities")
        if confidence is None and isinstance(probabilities, dict) and choice in probabilities:
            confidence = probabilities[choice]
        if confidence is not None:
            band = _band("walk.new_date", float(confidence), thresholds)
            if band == "gray":
                return "gray"
            if band == "reject":
                return "none"
        if isinstance(choice, str):
            return "none" if choice.casefold() == "none" else "selected"
    return "missing"


def _paragraph_effects(
    role: str,
    paragraph: dict[str, Any],
    thresholds: dict[str, dict[str, float]],
) -> list[tuple[str, str | None]] | None:
    probabilities = paragraph
    if role not in _ROLE_IDS:
        return None
    checked = _ROLE_IDS[role]
    if any(
        _band(question_id, float(probabilities.get(question_id, 0.0)), thresholds) == "gray"
        for question_id in checked
    ):
        return None

    def yes(question_id: str) -> bool:
        return (
            _band(question_id, float(probabilities.get(question_id, 0.0)), thresholds)
            == "accept"
        )

    effects: list[tuple[str, str | None]] = []
    if role == "waiting_party":
        if yes("walk.wp_received"):
            effects.append(("closed:completed", None))
        if yes("walk.wp_no_longer_needed"):
            effects.append(("closed:withdrawn", None))
        if yes("walk.wp_changes_terms"):
            still_owed = _band(
                "walk.wp_still_owed",
                float(probabilities.get("walk.wp_still_owed", 0.0)),
                thresholds,
            )
            if still_owed == "gray":
                return None
            effects.append(
                ("open:amended", None)
                if still_owed == "accept"
                else ("closed:withdrawn", None)
            )
        if yes("walk.wp_asks_again"):
            effects.append(("open:asked_again", None))
        if yes("walk.wp_changes_due"):
            effects.append(("due:waiting_party", probabilities.get("walk.new_date")))
    elif role == "user":
        if yes("walk.user_delivers"):
            effects.append(("suggested:closure", "user"))
        if yes("walk.user_declines"):
            effects.append(("suggested:declined", "user"))
        if yes("walk.user_hands_off"):
            effects.append(("suggested:delegated", "user"))
        if yes("walk.user_promises"):
            effects.append(("due:user", probabilities.get("walk.new_date")))
    elif yes("walk.third_party_done"):
        effects.append(("suggested:closure", "third_party"))
    return effects


def _step_verdict(
    role: str,
    paragraphs: list[dict[str, Any]],
    thresholds: dict[str, dict[str, float]],
) -> tuple[str, list[tuple[str, str | None]]]:
    gate_bands = [
        _band("walk.responds", float(paragraph.get("walk.responds", 0.0)), thresholds)
        for paragraph in paragraphs
    ]
    responding = [
        paragraph
        for paragraph, band in zip(paragraphs, gate_bands, strict=True)
        if band == "accept"
    ]
    if not responding:
        return ("escalated", []) if "gray" in gate_bands else ("no_response", [])

    terminal: list[tuple[str, str | None]] = []
    other: list[tuple[str, str | None]] = []
    ranks = {"open:asked_again": 1, "open:amended": 2, "due:waiting_party": 3, "due:user": 3}
    for paragraph in responding:
        effects = _paragraph_effects(role, paragraph, thresholds)
        if effects is None:
            return "escalated", []
        for effect in effects:
            if effect[0].startswith(("closed:", "suggested:")):
                terminal.append(effect)
            elif not any(ranks.get(kept[0]) == ranks.get(effect[0]) for kept in other):
                other.append(effect)
    if terminal:
        if any(effect[0] != terminal[0][0] for effect in terminal):
            return "escalated", []
        return "apply", [terminal[0]]
    other.sort(key=lambda effect: ranks.get(effect[0], 99))
    return ("apply", other) if other else ("no_response", [])


def walk_status(
    steps: list[tuple[str, list[dict[str, Any]]]],
    thresholds: dict[str, dict[str, float]],
) -> str:
    """Apply the walk's gate, branch, terminal, and follow-up rules to probabilities."""
    current = "open"
    suggestion: str | None = None
    suggestion_source: str | None = None
    for role, paragraphs in steps:
        verdict, effects = _step_verdict(role, paragraphs, thresholds)
        if verdict == "escalated":
            return "escalated"
        if verdict == "no_response":
            continue
        for effect, source in effects:
            if effect.startswith("closed:"):
                return effect
            if effect.startswith("suggested:"):
                suggestion, suggestion_source = effect, source
                continue
            if effect == "open:asked_again":
                current = effect
                if suggestion == "suggested:closure" and suggestion_source == "user":
                    suggestion = None
                    suggestion_source = None
                continue
            if effect == "open:amended":
                current = effect
                continue
            if effect.startswith("due:"):
                # Mirrors the desktop walk: a picked date from the waiting party
                # applies; one from the user is a deadline suggestion. With no
                # date candidate ("missing") a user promise changes nothing and a
                # waiting-party change escalates; a gray or "none" pick over real
                # candidates escalates either way.
                date_status = _new_date_status(source, thresholds)
                source_role = effect.split(":", 1)[1]
                if date_status in {"gray", "none"}:
                    return "escalated"
                if date_status == "selected":
                    if source_role == "waiting_party":
                        current = "open:rescheduled"
                    else:
                        suggestion, suggestion_source = "suggested:deadline_change", "user"
                elif source_role == "waiting_party":
                    return "escalated"
    return suggestion or current


def expected_status(plan: ThreadPlan) -> str:
    """Compute a synthetic thread's expected final status from its fixed path."""
    from .threads import TERMINAL

    steps: list[tuple[str, list[dict[str, Any]]]] = []
    for message in plan.later:
        paragraphs = []
        labels = branch_labels(message.author_role, message.outcome)
        for ordinal in range(message.paragraphs):
            responds = ordinal == message.responding and message.outcome != "unrelated"
            values = {question_id: False for question_id in BRANCH_IDS[message.author_role]}
            if responds:
                values.update(labels)
            probabilities: dict[str, Any] = {
                "walk.responds": 0.99 if responds else 0.01,
                **{
                    question_id: 0.99 if value else 0.01
                    for question_id, value in values.items()
                },
            }
            if message.outcome in {"wp_changes_due", "user_promises"} and responds:
                probabilities["walk.new_date"] = "synthetic date"
            paragraphs.append(probabilities)
        steps.append((message.author_role, paragraphs))
        if message.outcome in TERMINAL:
            break
    return walk_status(steps, _FALLBACK_THRESHOLDS)


def _plan_from_record(value: dict[str, Any]) -> ThreadPlan:
    raw = value["plan"]
    return ThreadPlan(
        thread_id=str(raw["thread_id"]),
        hard_case=str(raw["hard_case"]),
        has_event=bool(raw["has_event"]),
        ends_with_event=raw.get("ends_with_event"),
        later=[MessagePlan(**message) for message in raw.get("later", [])],
    )


def _registry_thresholds() -> dict[str, dict[str, float]]:
    from .registry import DEFAULT_REGISTRY

    thresholds = {key: dict(value) for key, value in _FALLBACK_THRESHOLDS.items()}
    try:
        registry = json.loads(DEFAULT_REGISTRY.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return thresholds
    for question_id in thresholds:
        item = registry.get("questions", {}).get(question_id)
        if isinstance(item, dict) and "accept" in item and "escalate" in item:
            thresholds[question_id] = {
                "accept": float(item["accept"]),
                "escalate": float(item["escalate"]),
            }
    return thresholds


def evaluate_walk(
    threads_path: str | Path,
    client: Any,
    *,
    thresholds: dict[str, dict[str, float]] | None = None,
) -> dict[str, Any]:
    """Evaluate held-out thread statuses through the DecisionsClient interface."""
    from .adapter import program

    records = [
        json.loads(line)
        for line in Path(threads_path).read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not records:
        raise ValueError("thread corpus is empty")
    held_out = [
        record
        for record in records
        if split_name(str(record["thread_id"]), small=True) == "test"
    ]
    active_thresholds = thresholds or _registry_thresholds()
    predictors: dict[str, Any] = {}
    outcomes: list[tuple[str, str]] = []
    for record in held_out:
        plan = _plan_from_record(record)
        ask = str(record["ask"])
        due = str(record["due"])
        steps: list[tuple[str, list[dict[str, Any]]]] = []
        for message, paragraphs in zip(plan.later, record["later"], strict=True):
            role = message.author_role
            paragraph_answers = []
            for text in paragraphs:
                state = {
                    "user": {"name": str(record["user_name"])},
                    "waiting_party": str(record["waiting_party"]),
                    "ask": ask,
                    "due": due,
                    "later": {"author_role": role, "paragraph_text": str(text)},
                }
                answers: dict[str, Any] = {}
                question_ids = ("walk.responds", *BRANCH_IDS[role])
                for question_id in question_ids:
                    predictor = predictors.setdefault(
                        question_id, program(question_id, client)
                    )
                    prediction = predictor(**state)
                    answers[question_id] = float(prediction.decision.probability)
                needs_date = (
                    role == "waiting_party"
                    and _band(
                        "walk.wp_changes_due",
                        answers["walk.wp_changes_due"],
                        active_thresholds,
                    )
                    == "accept"
                ) or (
                    role == "user"
                    and _band(
                        "walk.user_promises",
                        answers["walk.user_promises"],
                        active_thresholds,
                    )
                    == "accept"
                )
                if needs_date:
                    date_predictor = predictors.setdefault(
                        "walk.new_date", program("walk.new_date", client)
                    )
                    date_prediction = date_predictor(
                        ask=ask,
                        later={"paragraph_text": str(text)},
                    ).decision
                    answers["walk.new_date"] = {
                        "choice": str(date_prediction.value),
                        "confidence": float(date_prediction.confidence),
                    }
                paragraph_answers.append(answers)
            steps.append((role, paragraph_answers))
            status = walk_status(steps, active_thresholds)
            if status.startswith(("closed:", "escalated")):
                break
            if role == "waiting_party":
                changed = any(
                    answers.get("walk.responds", 0) >= active_thresholds["walk.responds"]["accept"]
                    and answers.get("walk.wp_changes_terms", 0)
                    >= active_thresholds["walk.wp_changes_terms"]["accept"]
                    and answers.get("walk.wp_still_owed", 0)
                    >= active_thresholds["walk.wp_still_owed"]["accept"]
                    for answers in paragraph_answers
                )
                if changed:
                    responding_text = next(
                        str(text)
                        for text, answers in zip(paragraphs, paragraph_answers, strict=True)
                        if answers.get("walk.responds", 0)
                        >= active_thresholds["walk.responds"]["accept"]
                        and answers.get("walk.wp_changes_terms", 0)
                        >= active_thresholds["walk.wp_changes_terms"]["accept"]
                        and answers.get("walk.wp_still_owed", 0)
                        >= active_thresholds["walk.wp_still_owed"]["accept"]
                    )
                    ask = f"{ask}\n\nLater change: {responding_text}"
        actual = walk_status(steps, active_thresholds)
        outcomes.append((expected_status(plan), actual))

    correct = sum(expected == actual for expected, actual in outcomes)
    escalated = sum(actual == "escalated" for _expected, actual in outcomes)
    confusion_counts: dict[str, Counter[str]] = defaultdict(Counter)
    for expected, actual in outcomes:
        confusion_counts[expected][actual] += 1
    confusion = {
        expected: dict(sorted(actuals.items()))
        for expected, actuals in sorted(confusion_counts.items())
    }
    total = len(outcomes)
    return {
        "threads": total,
        "correct": correct,
        "accuracy": correct / total if total else 0.0,
        "escalated": escalated,
        "escalated_share": escalated / total if total else 0.0,
        "confusion": confusion,
    }
