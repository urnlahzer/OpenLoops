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
    "walk.handoff_to": {"accept": 0.7, "escalate": 0.3},
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
    units: list[dict[str, Any]],
    thresholds: dict[str, dict[str, float]],
) -> tuple[str, list[tuple[str, str | None]]]:
    gate_bands = [
        _band("walk.responds", float(unit.get("walk.responds", 0.0)), thresholds)
        for unit in units
    ]
    responding = [
        unit
        for unit, band in zip(units, gate_bands, strict=True)
        if band == "accept"
    ]
    if not responding:
        return ("escalated", []) if "gray" in gate_bands else ("no_response", [])

    terminal: list[tuple[str, str | None]] = []
    other: dict[str, tuple[str, str | None]] = {}
    for unit in responding:
        effects = _paragraph_effects(role, unit, thresholds)
        if effects is None:
            return "escalated", []
        for effect in effects:
            if effect[0].startswith(("closed:", "suggested:")):
                terminal.append(effect)
            else:
                other.setdefault(effect[0], effect)
    if terminal:
        if any(effect[0] != terminal[0][0] for effect in terminal):
            return "escalated", []
        return "apply", [terminal[0]]
    ranks = {
        "open:asked_again": 1,
        "open:amended": 2,
        "due:waiting_party": 3,
        "due:user": 3,
    }
    effects = sorted(other.values(), key=lambda effect: ranks.get(effect[0], 99))
    return ("apply", effects) if effects else ("no_response", [])


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


def walk_state(
    user_name: str, waiting_party: str, ask: str, due: str, role: str, text: str
) -> dict[str, Any]:
    """Build the ordered production state for one paragraph or whole-message unit."""
    return {
        "user": {"name": user_name},
        "waiting_party": waiting_party,
        "ask": ask,
        "due": due,
        "later": {"author_role": role, "text": text},
    }


def date_state(ask: str, text: str) -> dict[str, Any]:
    """Build the ordered production state for a date follow-up."""
    return {"ask": ask, "later": {"text": text}}


def expected_status(plan: ThreadPlan) -> str:
    """Compute a synthetic thread's expected final status from its fixed path."""
    from .threads import TERMINAL

    steps: list[tuple[str, list[dict[str, Any]]]] = []
    for message in plan.later:
        units = []
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
            units.append(probabilities)
        if message.paragraphs >= 2:
            message_responds = message.outcome != "unrelated"
            message_values = {
                question_id: False for question_id in BRANCH_IDS[message.author_role]
            }
            if message_responds:
                message_values.update(labels)
            units.append(
                {
                    "walk.responds": 0.99 if message_responds else 0.01,
                    **{
                        question_id: 0.99 if value else 0.01
                        for question_id, value in message_values.items()
                    },
                }
            )
        steps.append((message.author_role, units))
        if message.outcome in TERMINAL:
            break
    return walk_status(steps, _FALLBACK_THRESHOLDS)


def _plan_from_record(value: dict[str, Any]) -> ThreadPlan:
    raw = value["plan"]
    later = []
    for message in raw.get("later", []):
        later.append(
            MessagePlan(
                author_role=str(message["author_role"]),
                outcome=str(message["outcome"]),
                paragraphs=int(message.get("paragraphs", 1)),
                responding=int(message.get("responding", 0)),
                delegate=message.get("delegate"),
            )
        )
    return ThreadPlan(
        thread_id=str(raw["thread_id"]),
        hard_case=str(raw["hard_case"]),
        has_event=bool(raw["has_event"]),
        ends_with_event=raw.get("ends_with_event"),
        later=later,
    )


def _later_record(value: Any) -> dict[str, Any]:
    if isinstance(value, list):
        return {"paragraphs": value}
    return value if isinstance(value, dict) else {}


def _handoff_options(
    later: dict[str, Any], user_addresses: set[str], waiting_party_address: str
) -> tuple[dict[str, str], dict[str, dict[str, str]]]:
    options: dict[str, str] = {}
    participants_by_handle: dict[str, dict[str, str]] = {}
    participants = later.get("participants", [])
    if not isinstance(participants, list):
        participants = []
    for participant in participants:
        if not isinstance(participant, dict):
            continue
        address = str(participant.get("address", ""))
        normalized_address = address.casefold()
        if normalized_address in user_addresses or normalized_address == (
            waiting_party_address.casefold()
        ):
            continue
        name = str(participant.get("name", "")).strip()
        if not name:
            name = address.partition("@")[0]
        handle = f"p{len(participants_by_handle)}"
        options[handle] = name
        participants_by_handle[handle] = {"name": name, "address": address}
    options["none"] = "The text names no listed person."
    return options, participants_by_handle


def _handoff_mark(
    participant: dict[str, str] | None, user_domains: set[str]
) -> str:
    if participant is None:
        return "none"
    # Same list as the desktop app's closure walk.
    public_mail_domains = {
        "aol.com",
        "fastmail.com",
        "gmail.com",
        "gmx.com",
        "gmx.net",
        "googlemail.com",
        "hey.com",
        "hotmail.com",
        "icloud.com",
        "live.com",
        "mac.com",
        "mail.com",
        "me.com",
        "msn.com",
        "outlook.com",
        "passmail.net",
        "pm.me",
        "proton.me",
        "protonmail.com",
        "tutanota.com",
        "yahoo.com",
        "yandex.com",
        "ymail.com",
        "zoho.com",
    }
    address_domain = participant["address"].rsplit("@", 1)[-1].casefold()
    return (
        "internal"
        if address_domain in user_domains and address_domain not in public_mail_domains
        else "external"
    )


def _registry_thresholds(registry: dict[str, Any] | None = None) -> dict[str, dict[str, float]]:
    from .registry import DEFAULT_REGISTRY

    thresholds = {key: dict(value) for key, value in _FALLBACK_THRESHOLDS.items()}
    if registry is None:
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


def is_held_out_thread(thread_id: str) -> bool:
    """A thread is held out only when it is test data for every question.

    Large question populations split 78/7/15 and small ones 60/20/20, so a
    thread in buckets 80-84 is test data for a small question but validation
    data for `walk.responds`. Only buckets 85-99 are test under both.
    """
    return (
        split_name(thread_id, small=True) == "test"
        and split_name(thread_id, small=False) == "test"
    )


def evaluate_walk(
    threads_path: str | Path,
    client: Any,
    *,
    thresholds: dict[str, dict[str, float]] | None = None,
    registry: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Evaluate held-out thread statuses through the DecisionsClient interface.

    With ``registry``, the predictors use its instructions and thresholds (the
    wording that ships); without it, the bootstrap docstrings.
    """
    from .adapter import program
    from .questions import SPECS

    records = [
        json.loads(line)
        for line in Path(threads_path).read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not records:
        raise ValueError("thread corpus is empty")
    held_out = [record for record in records if is_held_out_thread(str(record["thread_id"]))]
    active_thresholds = thresholds or _registry_thresholds(registry)
    registry_instructions = {
        question_id: str(item["instructions"])
        for question_id, item in (registry or {}).get("questions", {}).items()
        if isinstance(item, dict) and item.get("instructions")
    }
    predictors: dict[str, Any] = {}
    outcomes: list[tuple[str, str]] = []
    handoff_total = 0
    handoff_mark_correct = 0
    handoff_person_correct = 0
    for record in held_out:
        plan = _plan_from_record(record)
        ask = str(record["ask"])
        due = str(record["due"])
        steps: list[tuple[str, list[dict[str, Any]]]] = []
        user_addresses = {
            str(address).casefold()
            for address in record.get("user_addresses", [])
            if isinstance(address, str)
        }
        user_domains = {
            address.rsplit("@", 1)[-1]
            for address in user_addresses
            if "@" in address
        }
        waiting_party_address = str(record.get("waiting_party_address", ""))
        for message, raw_later in zip(plan.later, record["later"], strict=True):
            role = message.author_role
            later = _later_record(raw_later)
            paragraphs = later.get("paragraphs", [])
            if not isinstance(paragraphs, list):
                paragraphs = []
            unit_texts = [str(text) for text in paragraphs]
            if len(unit_texts) >= 2:
                unit_texts.append("\n\n".join(unit_texts))
            unit_answers = []
            for text in unit_texts:
                state = walk_state(
                    str(record.get("user_name", "the user")),
                    str(record["waiting_party"]),
                    ask,
                    due,
                    role,
                    text,
                )
                answers: dict[str, Any] = {}
                question_ids = ("walk.responds", *BRANCH_IDS[role])
                for question_id in question_ids:
                    predictor = predictors.setdefault(
                        question_id,
                        program(question_id, client, registry_instructions.get(question_id)),
                    )
                    prediction = predictor(**state)
                    answers[question_id] = float(prediction.decision.probability)
                gate_accepts = (
                    _band("walk.responds", answers["walk.responds"], active_thresholds)
                    == "accept"
                )
                needs_date = (
                    gate_accepts
                    and role == "waiting_party"
                    and _band(
                        "walk.wp_changes_due",
                        answers["walk.wp_changes_due"],
                        active_thresholds,
                    )
                    == "accept"
                ) or (
                    gate_accepts
                    and role == "user"
                    and _band(
                        "walk.user_promises",
                        answers["walk.user_promises"],
                        active_thresholds,
                    )
                    == "accept"
                )
                if needs_date:
                    date_predictor = predictors.setdefault(
                        "walk.new_date",
                        program(
                            "walk.new_date", client, registry_instructions.get("walk.new_date")
                        ),
                    )
                    date_prediction = date_predictor(**date_state(ask, text)).decision
                    answers["walk.new_date"] = {
                        "choice": str(date_prediction.value),
                        "confidence": float(date_prediction.confidence),
                    }
                handoff_accepts = (
                    gate_accepts
                    and role == "user"
                    and _band(
                        "walk.user_hands_off",
                        answers["walk.user_hands_off"],
                        active_thresholds,
                    )
                    == "accept"
                )
                if handoff_accepts:
                    options, participants_by_handle = _handoff_options(
                        later, user_addresses, waiting_party_address
                    )
                    question = {
                        "walk.handoff_to": {
                            "type": "choice",
                            "instructions": registry_instructions.get(
                                "walk.handoff_to",
                                SPECS["walk.handoff_to"].signature.instructions,
                            ),
                            "criteria": options,
                        }
                    }
                    response = client.decide(state, question)["walk.handoff_to"]
                    choice = str(response.get("choice", "none"))
                    confidence = float(response.get("confidence", 0.0))
                    if _band("walk.handoff_to", confidence, active_thresholds) != "accept":
                        choice = "none"
                    selected = participants_by_handle.get(choice)
                    truth = later.get("delegate")
                    if isinstance(truth, dict) and isinstance(truth.get("mark"), str):
                        expected_mark = str(truth["mark"])
                        expected_name = str(truth.get("name", ""))
                        selected_mark = _handoff_mark(selected, user_domains)
                        handoff_total += 1
                        handoff_mark_correct += selected_mark == expected_mark
                        person_matches = (
                            selected is None
                            if expected_mark == "none"
                            else bool(expected_name)
                            and selected is not None
                            and selected["name"].casefold() == expected_name.casefold()
                        )
                        handoff_person_correct += person_matches
                unit_answers.append(answers)
            steps.append((role, unit_answers))
            status = walk_status(steps, active_thresholds)
            if status.startswith(("closed:", "escalated")):
                break
            if role == "waiting_party":
                for text, answers in zip(unit_texts, unit_answers, strict=True):
                    if (
                        _band(
                            "walk.responds",
                            float(answers.get("walk.responds", 0.0)),
                            active_thresholds,
                        )
                        == "accept"
                        and _band(
                            "walk.wp_changes_terms",
                            float(answers.get("walk.wp_changes_terms", 0.0)),
                            active_thresholds,
                        )
                        == "accept"
                        and _band(
                            "walk.wp_still_owed",
                            float(answers.get("walk.wp_still_owed", 0.0)),
                            active_thresholds,
                        )
                        == "accept"
                    ):
                        ask = f"{ask}\n\nLater change: {text}"
                        break
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
        "instructions": "registry" if registry_instructions else "bootstrap",
        "threads": total,
        "correct": correct,
        "accuracy": correct / total if total else 0.0,
        "escalated": escalated,
        "escalated_share": escalated / total if total else 0.0,
        "confusion": confusion,
        "handoff": {
            "total": handoff_total,
            "mark_correct": handoff_mark_correct,
            "person_correct": handoff_person_correct,
            "mark_accuracy": (
                handoff_mark_correct / handoff_total if handoff_total else 0.0
            ),
            "person_accuracy": (
                handoff_person_correct / handoff_total if handoff_total else 0.0
            ),
        },
    }
