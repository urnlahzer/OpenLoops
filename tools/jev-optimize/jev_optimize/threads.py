"""Synthetic email threads for the closure walk and their derived rows."""

from __future__ import annotations

import hashlib
import json
import os
import random
import re
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from typing import Any

import httpx

from .synthetic import (
    SYNTHETIC_COMPANIES,
    SYNTHETIC_PEOPLE,
    SYNTHETIC_SERVICES,
    CorpusThresholds,
    check_corpus,
    scrub_row,
)

ROLE_OUTCOMES = {
    "waiting_party": (
        "wp_received",
        "wp_no_longer_needed",
        "wp_asks_again",
        "wp_changes_terms_owed",
        "wp_changes_terms_ends",
        "wp_changes_due",
        "unrelated",
    ),
    "user": (
        "user_delivers",
        "user_promises",
        "user_declines",
        "user_hands_off",
        "unrelated",
    ),
    "third_party": ("third_party_done", "third_party_other", "unrelated"),
}
OUTCOMES = tuple(sorted({outcome for outcomes in ROLE_OUTCOMES.values() for outcome in outcomes}))
TERMINAL = {
    "wp_received",
    "wp_no_longer_needed",
    "wp_changes_terms_ends",
}
HARD_CASES = (
    "none",
    "rsvp_before_event",
    "pay_by_date",
    "prepare_for_meeting",
    "correction_amends",
    "correction_ends",
    "thanks_will_look",
    "partial_delivery",
    "chaser_after_delivery",
    "unrelated_later_message",
)
BRANCH_IDS = {
    "waiting_party": (
        "walk.wp_received",
        "walk.wp_no_longer_needed",
        "walk.wp_asks_again",
        "walk.wp_changes_terms",
        "walk.wp_still_owed",
        "walk.wp_changes_due",
    ),
    "user": (
        "walk.user_delivers",
        "walk.user_promises",
        "walk.user_declines",
        "walk.user_hands_off",
    ),
    "third_party": ("walk.third_party_done",),
}


@dataclass(frozen=True)
class MessagePlan:
    author_role: str
    outcome: str
    paragraphs: int = 1
    responding: int = 0
    delegate: str | None = None


@dataclass(frozen=True)
class ThreadPlan:
    thread_id: str
    hard_case: str
    has_event: bool
    ends_with_event: bool | None
    later: list[MessagePlan] = field(default_factory=list)


@dataclass(frozen=True)
class Thread:
    plan: ThreadPlan
    user_name: str
    waiting_party: str
    ask: str
    due: str
    event: str | None
    later: list[list[str]]
    user_display_name: str
    user_addresses: list[str]
    waiting_party_address: str
    delegates: list[dict[str, str] | None]
    participants: list[list[dict[str, str]] | None]


def branch_labels(role: str, outcome: str) -> dict[str, bool]:
    labels = {question_id: False for question_id in BRANCH_IDS[role]}
    target = {
        "wp_received": ("walk.wp_received",),
        "wp_no_longer_needed": ("walk.wp_no_longer_needed",),
        "wp_asks_again": ("walk.wp_asks_again",),
        "wp_changes_terms_owed": ("walk.wp_changes_terms", "walk.wp_still_owed"),
        "wp_changes_terms_ends": ("walk.wp_changes_terms",),
        "wp_changes_due": ("walk.wp_changes_due", "walk.wp_still_owed"),
        "user_delivers": ("walk.user_delivers",),
        "user_promises": ("walk.user_promises",),
        "user_declines": ("walk.user_declines",),
        "user_hands_off": ("walk.user_hands_off",),
        "third_party_done": ("walk.third_party_done",),
    }.get(outcome, ())
    for question_id in target:
        labels[question_id] = True
    if role == "waiting_party" and outcome == "wp_asks_again":
        labels["walk.wp_still_owed"] = True
    return labels


def plan_threads(count: int, seed: int = 20261021) -> list[ThreadPlan]:
    """Fixed plans: every hard case and every role outcome appear in rotation."""
    rng = random.Random(seed)
    plans = []
    role_cycle = [
        (role, outcome)
        for role, outcomes in ROLE_OUTCOMES.items()
        for outcome in outcomes
    ]
    for index in range(count):
        hard_case = HARD_CASES[index % len(HARD_CASES)]
        later = _later_for(hard_case, index, role_cycle, rng)
        has_event = hard_case in {"rsvp_before_event", "prepare_for_meeting"} or index % 7 == 0
        ends = None
        if has_event:
            ends = hard_case in {"rsvp_before_event", "prepare_for_meeting"} or index % 2 == 0
        plans.append(ThreadPlan(f"thread-{index:05d}", hard_case, has_event, ends, later))
    handoff_count = sum(
        message.outcome == "user_hands_off"
        for plan in plans
        for message in plan.later
    )
    delegate_marks: list[str] = []
    while len(delegate_marks) < handoff_count:
        cycle = ["internal", "external", "absent"]
        rng.shuffle(cycle)
        delegate_marks.extend(cycle)
    marks = iter(delegate_marks[:handoff_count])
    return [
        replace(
            plan,
            later=[
                replace(message, delegate=next(marks))
                if message.outcome == "user_hands_off"
                else message
                for message in plan.later
            ],
        )
        for plan in plans
    ]


def _later_for(
    hard_case: str,
    index: int,
    role_cycle: list[tuple[str, str]],
    rng: random.Random,
) -> list[MessagePlan]:
    fixed = {
        "correction_amends": [MessagePlan("waiting_party", "wp_changes_terms_owed")],
        "correction_ends": [MessagePlan("waiting_party", "wp_changes_terms_ends")],
        "thanks_will_look": [MessagePlan("user", "user_promises")],
        "partial_delivery": [MessagePlan("user", "user_promises", 2, 1)],
        "chaser_after_delivery": [
            MessagePlan("user", "user_delivers"),
            MessagePlan("waiting_party", "wp_asks_again"),
        ],
        "unrelated_later_message": [MessagePlan("third_party", "unrelated", 2, 0)],
    }
    if hard_case in fixed:
        return fixed[hard_case]
    count = 1 + index % 3
    later = []
    for step in range(count):
        role, outcome = role_cycle[(index + step) % len(role_cycle)]
        paragraphs = 1 + rng.randrange(3)
        later.append(MessagePlan(role, outcome, paragraphs, rng.randrange(paragraphs)))
    return later


def stub_threads(plans: list[ThreadPlan]) -> list[Thread]:
    """Deterministic template text; keyword markers keep the offline fake predictable."""
    threads = []
    for number, plan in enumerate(plans):
        user_display_name = SYNTHETIC_PEOPLE[number % len(SYNTHETIC_PEOPLE)]
        user = user_display_name.split()[0]
        waiting = SYNTHETIC_PEOPLE[(number + 3) % len(SYNTHETIC_PEOPLE)]
        user_address = _person_address(user_display_name, "halcyonridge.example.invalid")
        waiting_address = _person_address(waiting, "copperlantern.example.invalid")
        ask, due, event = _stub_ask(plan, number)
        later = []
        delegates: list[dict[str, str] | None] = []
        participants: list[list[dict[str, str]] | None] = []
        for message_index, message in enumerate(plan.later):
            paragraphs = [
                f"Unrelated note {number}-{index}: the parking garage closes early this week."
                for index in range(message.paragraphs)
            ]
            delegate = _delegate_person(
                message,
                user_display_name,
                waiting,
                number,
                message_index,
            )
            delegates.append(delegate)
            if message.outcome == "user_hands_off":
                message_participants = [
                    {"name": user_display_name, "address": user_address},
                    {"name": waiting, "address": waiting_address},
                ]
                if message.delegate != "absent" and delegate is not None:
                    message_participants.append(delegate)
                participants.append(message_participants)
            else:
                participants.append(None)
            paragraphs[message.responding] = _stub_paragraph(
                message,
                number,
                delegate["name"] if delegate is not None else None,
            )
            later.append(paragraphs)
        threads.append(
            Thread(
                plan,
                user,
                waiting,
                ask,
                due,
                event,
                later,
                user_display_name,
                [user_address],
                waiting_address,
                delegates,
                participants,
            )
        )
    return threads


def _person_address(name: str, domain: str) -> str:
    mailbox = name.casefold().replace(" ", ".")
    return f"{mailbox}@{domain}"


def _delegate_person(
    message: MessagePlan,
    user_name: str,
    waiting_party: str,
    thread_number: int,
    message_index: int,
) -> dict[str, str] | None:
    if message.outcome != "user_hands_off":
        return None
    if message.delegate not in {"internal", "external", "absent"}:
        raise ValueError("hand-off plans must have a delegate mark")
    candidates = [
        name
        for offset in range(len(SYNTHETIC_PEOPLE))
        if (name := SYNTHETIC_PEOPLE[
            (thread_number + message_index + 5 + offset) % len(SYNTHETIC_PEOPLE)
        ]).casefold()
        not in {user_name.casefold(), waiting_party.casefold()}
    ]
    name = candidates[0]
    domain = (
        "halcyonridge.example.invalid"
        if message.delegate == "internal"
        else "copperlantern.example.invalid"
    )
    return {"name": name, "address": _person_address(name, domain)}


def _stub_ask(plan: ThreadPlan, number: int) -> tuple[str, str, str | None]:
    if plan.hard_case == "rsvp_before_event":
        return (
            f"Please RSVP by Friday for the spring gala {number}.",
            "Friday",
            f"the spring gala {number} on 2026-10-30",
        )
    if plan.hard_case == "prepare_for_meeting":
        return (
            f"Please prepare the slides for board meeting {number} on 9/12.",
            "9/12",
            f"board meeting {number} on 2026-09-12",
        )
    if plan.hard_case == "pay_by_date":
        return f"Please pay invoice {number} by 9/12.", "9/12", None
    event = f"planning session {number} on 2026-11-05" if plan.has_event else None
    if plan.ends_with_event:
        return (
            f"Could you prepare the agenda notes {number} for the planning session?",
            "not stated",
            event,
        )
    return (
        f"Could you send the vendor summary {number} before the planning session?",
        "not stated",
        event,
    )


def _stub_paragraph(
    message: MessagePlan, number: int, delegate_name: str | None = None
) -> str:
    if message.outcome == "user_hands_off":
        if not delegate_name:
            raise ValueError("hand-off stub text requires its planned delegate name")
        return f"{delegate_name} will send summary {number}. mk-handoff."
    return {
        "wp_received": f"Got the summary {number}, thanks. mk-received.",
        "wp_no_longer_needed": f"Never mind summary {number}, we no longer need it. mk-withdrawn.",
        "wp_asks_again": f"Any update on summary {number}? mk-chase. mk-still-owed.",
        "wp_changes_terms_owed": (
            f"Small change to summary {number}: add the Q3 totals. "
            "mk-amend-owed. mk-still-owed."
        ),
        "wp_changes_terms_ends": (
            f"Forget summary {number}, we will use the dashboard. mk-amend-ends."
        ),
        "wp_changes_due": (
            f"Can you get summary {number} to me by Wednesday instead? "
            "mk-newdate. mk-still-owed."
        ),
        "user_delivers": f"Attached is summary {number}. mk-delivered.",
        "user_promises": f"I will send summary {number} next week. mk-promise.",
        "user_declines": f"I can't take on summary {number}, sorry. mk-decline.",
        "third_party_done": f"I sent summary {number} this morning. mk-thirddone.",
        "third_party_other": (
            f"I am reviewing summary {number} and will share an update tomorrow. mk-reviewing."
        ),
        "unrelated": f"Separate topic {number}: the offsite menu is final. mk-unrelated.",
    }[message.outcome]


def derive_rows(threads: list[Thread], source: str = "synthetic-stub") -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for thread in threads:
        ask = thread.ask
        for message, paragraphs in zip(thread.plan.later, thread.later, strict=True):
            for ordinal, text in enumerate(paragraphs):
                responds = ordinal == message.responding and message.outcome != "unrelated"
                label: dict[str, bool] = {"walk.responds": responds}
                if responds:
                    label.update(branch_labels(message.author_role, message.outcome))
                rows.append(
                    _walk_row(
                        thread, ask, message.author_role, text, "paragraph", label, source
                    )
                )
            if len(paragraphs) >= 2:
                message_text = "\n\n".join(paragraphs)
                label = {"walk.responds": message.outcome != "unrelated"}
                if label["walk.responds"]:
                    label.update(branch_labels(message.author_role, message.outcome))
                rows.append(
                    _walk_row(
                        thread,
                        ask,
                        message.author_role,
                        message_text,
                        "message",
                        label,
                        source,
                    )
                )
            if message.outcome == "wp_changes_terms_owed":
                ask = f"{ask}\n\nLater change: {paragraphs[message.responding]}"
            if message.outcome in TERMINAL:
                break
        if thread.event is not None and thread.plan.ends_with_event is not None:
            rows.append(
                _with_id(
                    {
                        "set": "expiry",
                        "group": thread.plan.thread_id,
                        "source": source,
                        "ask": thread.ask,
                        "waiting_party": thread.waiting_party,
                        "due": thread.due,
                        "event": thread.event,
                        "label": {
                            "expiry.ends_with_event": thread.plan.ends_with_event
                        },
                    }
                )
            )
    return rows


def thread_record(thread: Thread, source: str = "synthetic-stub") -> dict[str, Any]:
    """Return one JSON-safe thread record containing its fixed plan and text."""
    later_records = []
    for index, (message, paragraphs) in enumerate(
        zip(thread.plan.later, thread.later, strict=True)
    ):
        later_record: dict[str, Any] = {"paragraphs": paragraphs}
        if message.outcome == "user_hands_off":
            later_record["participants"] = thread.participants[index] or []
            delegate = thread.delegates[index]
            later_record["delegate"] = {
                "name": delegate["name"] if delegate else "",
                "mark": "none" if message.delegate == "absent" else message.delegate,
            }
        later_records.append(later_record)
    return {
        "thread_id": thread.plan.thread_id,
        "source": source,
        "plan": asdict(thread.plan),
        "user_name": thread.user_name,
        "user_display_name": thread.user_display_name,
        "user_addresses": thread.user_addresses,
        "waiting_party": thread.waiting_party,
        "waiting_party_address": thread.waiting_party_address,
        "ask": thread.ask,
        "due": thread.due,
        "event": thread.event,
        "later": later_records,
    }


def _plan_payload(plan: ThreadPlan, thread: Thread) -> dict[str, Any]:
    later = []
    for index, message in enumerate(plan.later):
        # The model sees what to write, not where it goes: code places the
        # response at the planned paragraph.
        responds = message.outcome != "unrelated"
        delegate = thread.delegates[index]
        later.append(
            {
                "author_role": message.author_role,
                "outcome": message.outcome,
                "response_required": responds,
                "filler_count": message.paragraphs - int(responds),
                "delegate_name": delegate["name"] if delegate else None,
            }
        )
    return {
        "thread_id": plan.thread_id,
        "hard_case": plan.hard_case,
        "has_event": plan.has_event,
        "ends_with_event": plan.ends_with_event,
        "user_name": thread.user_name,
        "waiting_party": thread.waiting_party,
        "event_hint": thread.event,
        "later": later,
    }


def _thread_schema() -> dict[str, Any]:
    later = {
        "type": "object",
        "properties": {
            "response": {"type": ["string", "null"]},
            "fillers": {"type": "array", "items": {"type": "string"}},
            "delegate_name": {"type": ["string", "null"]},
        },
        "required": ["response", "fillers", "delegate_name"],
        "additionalProperties": False,
    }
    item = {
        "type": "object",
        "properties": {
            "thread_id": {"type": "string"},
            "ask": {"type": "string"},
            "due": {"type": "string"},
            "event": {"type": ["string", "null"]},
            "later": {"type": "array", "items": later},
        },
        "required": ["thread_id", "ask", "due", "event", "later"],
        "additionalProperties": False,
    }
    return {
        "type": "json_schema",
        "json_schema": {
            "name": "synthetic_threads",
            "strict": True,
            "schema": {
                "type": "object",
                "properties": {"threads": {"type": "array", "items": item}},
                "required": ["threads"],
                "additionalProperties": False,
            },
        },
    }


_PLAN_DEFINITIONS = (
    "Definitions. The ask is a request from the waiting party to the user. "
    "event: when has_event is true, write a named event with a date in the event field and "
    "mention it in the ask; otherwise event is null. ends_with_event true: the ask only helps "
    "the waiting party prepare for or take part in that event (prepare, attend, RSVP, bring), "
    "so doing it after the event ends has no purpose. ends_with_event false: the ask stays "
    "useful after the event (send a report, pay an invoice), even if it mentions the event. "
    "Outcomes for response: wp_received = the waiting party says they received "
    "what the ask requests; wp_no_longer_needed = the waiting party withdraws the ask; "
    "wp_asks_again = the waiting party chases or asks for an update; wp_changes_terms_owed = "
    "the waiting party changes what is required and the user still owes it (\"the fee is 350, "
    "not 359\"); wp_changes_terms_ends = the waiting party changes the ask so the user no longer "
    "owes anything (\"forget the fee\"); wp_changes_due = the waiting party moves the due date "
    "and states the new date; user_delivers = the user gives what was asked; user_promises = "
    "the user says they will do it later; user_declines = the user declines; user_hands_off = "
    "the user says another named person will do it; third_party_done = another participant "
    "says they did it or will do it; third_party_other = another participant mentions the ask "
    "without doing it or promising it; unrelated = the message has no response to the ask. "
    "Hard cases: thanks_will_look = a brief acknowledgement that promises a later look without "
    "a date; partial_delivery = the user sends part and promises the rest in the same response "
    "paragraph; "
    "chaser_after_delivery = the waiting party chases after the user claimed delivery. "
    "For every user_hands_off message, its planned delegate_name is the full name of the "
    "person who will do the request. Use that name in the response paragraph. Copy the "
    "planned delegate_name into the message result; use null for other outcomes."
)


def _thread_prompt(plans: list[dict[str, Any]]) -> str:
    return (
        "Write realistic but fully invented email threads from these fixed plans. Return strict "
        "JSON with one member named threads, in the same order as the plans. Each result must "
        "contain thread_id, ask, due, event, and later. Each later message contains response, "
        "fillers, and delegate_name. The ask is the request paragraph as the waiting "
        "party wrote it, without a speaker label such as 'Name:'. For a responding outcome, "
        "response is one paragraph that carries the planned outcome by itself, including any date "
        "or name it needs. For partial_delivery, that one response paragraph both sends the part "
        "and promises the rest. Each later message plan gives response_required and "
        "filler_count. When response_required is true, response is that one paragraph and is "
        "never null. When response_required is false, response is null. fillers always has "
        "exactly filler_count paragraphs, and a filler never carries the outcome. "
        "Each filler must be about a different topic and must never mention the ask, its subject, "
        "its documents, the event, or the request. Preserve the listed author role, hard case, and "
        "event semantics. "
        + _PLAN_DEFINITIONS
        + " Use only these invented names: "
        f"{', '.join((*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES, *SYNTHETIC_SERVICES))}. "
        "Any address must use example.invalid. Do not include URLs. "
        "Plans:\n"
        + json.dumps(plans, ensure_ascii=False, sort_keys=True)
    )


_KEY_TERM_STOP_WORDS = frozenset(
    {
        "about", "after", "again", "before", "could", "email", "every", "fully", "great", "hello",
        "morning", "other", "please", "quick", "should", "still", "thank", "thanks", "their",
        "there", "these", "those", "today", "tomorrow", "update", "which", "while", "would",
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january",
        "february", "march", "april", "may", "june", "july", "august", "september", "october",
        "november", "december",
    }
)
_POOL_NAME_WORDS = frozenset(
    word
    for name in (*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES, *SYNTHETIC_SERVICES)
    for word in re.findall(r"[^\W\d_]+", name.casefold())
)


def _key_terms(text: str) -> set[str]:
    return {
        word
        for word in re.findall(r"[^\W\d_]+", text.casefold())
        if len(word) >= 5 and word not in _KEY_TERM_STOP_WORDS and word not in _POOL_NAME_WORDS
    }


def _request_thread_batch(
    client: httpx.Client,
    model: str,
    api_key: str,
    plans: list[dict[str, Any]],
) -> list[Any]:
    try:
        response = client.post(
            "https://openrouter.ai/api/v1/chat/completions",
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            json={
                "model": model,
                "messages": [{"role": "user", "content": _thread_prompt(plans)}],
                "response_format": _thread_schema(),
                "provider": {"zdr": True},
                "temperature": 0.9,
                "max_tokens": 16384,
            },
        )
    except httpx.HTTPError:
        return []
    if response.status_code >= 400:
        # A refusal such as a data-policy 404 or an auth error repeats on every
        # retry, so it stops the run instead of counting as rejected threads.
        if response.status_code not in _RETRYABLE_STATUS:
            raise ThreadRequestError(
                f"thread request refused: HTTP {response.status_code} "
                f"{_error_message(response)}".rstrip()
            )
        return []
    try:
        content = response.json()["choices"][0]["message"]["content"]
        payload = json.loads(content)
    except (KeyError, IndexError, TypeError, ValueError):
        return []
    if not isinstance(payload, dict) or not isinstance(payload.get("threads"), list):
        return []
    return payload["threads"]


class ThreadRequestError(RuntimeError):
    """The endpoint refused a thread request in a way a retry would repeat."""


_RETRYABLE_STATUS = frozenset({408, 429, 500, 502, 503, 504})


def _error_message(response: httpx.Response) -> str:
    """The endpoint's own error text (no request content), cut to 300 characters."""
    try:
        body = response.json()
    except ValueError:
        return ""
    error = body.get("error") if isinstance(body, dict) else None
    message = error.get("message") if isinstance(error, dict) else None
    return str(message)[:300] if message else ""


def _validated_thread(
    candidate: Any,
    plan: ThreadPlan,
    assigned: Thread,
) -> tuple[Thread | None, str]:
    if not isinstance(candidate, dict) or candidate.get("thread_id") != plan.thread_id:
        return None, "thread id mismatch"
    if not isinstance(candidate.get("ask"), str) or not candidate["ask"].strip():
        return None, "ask is empty"
    if not isinstance(candidate.get("due"), str) or not candidate["due"].strip():
        return None, "due is empty"
    event = candidate.get("event")
    if plan.has_event:
        if not isinstance(event, str) or not event.strip():
            return None, "event is empty"
    elif event not in (None, ""):
        return None, "unexpected event"

    raw_later = candidate.get("later")
    if not isinstance(raw_later, list) or len(raw_later) != len(plan.later):
        return None, "later message count mismatch"
    blocked_terms = _key_terms(candidate["ask"])
    if isinstance(event, str):
        blocked_terms.update(_key_terms(event))
    later: list[list[str]] = []
    for index, (planned_message, generated_message) in enumerate(
        zip(plan.later, raw_later, strict=True)
    ):
        if not isinstance(generated_message, dict):
            return None, "later message is malformed"
        assigned_delegate = assigned.delegates[index]
        expected_delegate_name = (
            assigned_delegate["name"] if assigned_delegate is not None else None
        )
        if generated_message.get("delegate_name") != expected_delegate_name:
            return None, "delegate name mismatch"
        if "response" not in generated_message:
            return None, "response is missing or malformed"
        response = generated_message.get("response")
        responds = planned_message.outcome != "unrelated"
        if responds and (not isinstance(response, str) or not response.strip()):
            return None, "response is missing or malformed"
        if not responds and response is not None:
            return None, "unexpected response"
        fillers = generated_message.get("fillers")
        expected_fillers = planned_message.paragraphs - int(responds)
        if (
            not isinstance(fillers, list)
            or len(fillers) != expected_fillers
            or not all(isinstance(text, str) and text.strip() for text in fillers)
        ):
            return None, "filler count or text mismatch"
        if any(_key_terms(filler) & blocked_terms for filler in fillers):
            return None, "filler shares ask or event key term"
        if responds:
            paragraph_iter = iter(fillers)
            paragraphs = [
                response if ordinal == planned_message.responding else next(paragraph_iter)
                for ordinal in range(planned_message.paragraphs)
            ]
        else:
            paragraphs = list(fillers)
        if planned_message.outcome == "user_hands_off" and (
            not expected_delegate_name
            or expected_delegate_name.casefold()
            not in paragraphs[planned_message.responding].casefold()
        ):
            return None, "hand-off paragraph omits the planned delegate"
        later.append(paragraphs)

    texts = {
        "ask": candidate["ask"],
        "due": candidate["due"],
        "event": event or "",
        "later": later,
        "user_name": assigned.user_name,
        "waiting_party": assigned.waiting_party,
    }
    if scrub_row(texts):
        return None, "privacy scrub failed"
    return (
        Thread(
            plan=plan,
            user_name=assigned.user_name,
            waiting_party=assigned.waiting_party,
            ask=candidate["ask"],
            due=candidate["due"],
            event=event or None,
            later=later,
            user_display_name=assigned.user_display_name,
            user_addresses=assigned.user_addresses,
            waiting_party_address=assigned.waiting_party_address,
            delegates=assigned.delegates,
            participants=assigned.participants,
        ),
        "ok",
    )


def generate_threads_llm(
    output: str | Path,
    *,
    threads: int,
    seed: int,
    client: httpx.Client | None = None,
    parallel: int = 8,
    smoke: bool = False,
    batch_size: int = 8,
) -> dict[str, Any]:
    """Generate plan-labeled synthetic threads; tests inject an offline transport."""
    if threads < 600 and not smoke:
        raise ValueError("LLM thread generation requires at least 600 threads")
    if smoke and threads < 10:
        raise ValueError("a smoke run needs at least 10 threads")
    if batch_size < 1:
        raise ValueError("batch_size must be positive")
    api_key = os.environ.get("OPENROUTER_API_KEY")
    model = os.environ.get("OPENROUTER_SYNTH_MODEL")
    if not api_key or not model:
        raise RuntimeError("OPENROUTER_API_KEY and OPENROUTER_SYNTH_MODEL must be set")

    plans = plan_threads(threads, seed=seed)
    assigned_threads = stub_threads(plans)
    assigned_by_id = {thread.plan.thread_id: thread for thread in assigned_threads}
    prompt_by_id = {
        plan.thread_id: _plan_payload(plan, assigned_by_id[plan.thread_id]) for plan in plans
    }
    pending = list(plans)
    accepted: dict[str, Thread] = {}
    rejected = 0
    rejected_reasons: dict[str, int] = {}

    def reject(reason: str) -> None:
        nonlocal rejected
        rejected += 1
        rejected_reasons[reason] = rejected_reasons.get(reason, 0) + 1

    attempts = 0
    parallel = max(1, parallel)
    owned_client = client is None
    http = client or httpx.Client(timeout=120.0)
    try:
        while pending and attempts < threads * 8:
            batches = [
                pending[index : index + batch_size]
                for index in range(0, len(pending), batch_size)
            ]
            batches = batches[:parallel]
            round_plans = [plan for batch in batches for plan in batch]
            pending = pending[len(round_plans) :]
            with ThreadPoolExecutor(max_workers=parallel) as pool:
                results = list(
                    pool.map(
                        lambda batch: _request_thread_batch(
                            http,
                            model,
                            api_key,
                            [prompt_by_id[plan.thread_id] for plan in batch],
                        ),
                        batches,
                    )
                )
            completed: set[str] = set()
            for batch, candidates in zip(batches, results, strict=True):
                candidate_by_id = {
                    candidate.get("thread_id"): candidate
                    for candidate in candidates
                    if isinstance(candidate, dict) and isinstance(candidate.get("thread_id"), str)
                }
                for plan in batch:
                    candidate = candidate_by_id.get(plan.thread_id)
                    if candidate is None:
                        reject("missing thread result")
                        continue
                    thread, reason = _validated_thread(
                        candidate, plan, assigned_by_id[plan.thread_id]
                    )
                    if thread is None:
                        reject(reason)
                        continue
                    accepted[plan.thread_id] = thread
                    completed.add(plan.thread_id)
            pending.extend(plan for plan in round_plans if plan.thread_id not in completed)
            attempts += len(round_plans)
    finally:
        if owned_client:
            http.close()

    if len(accepted) != threads:
        raise RuntimeError(
            f"synthetic thread generation produced {len(accepted)} of {threads} threads "
            f"after {attempts} attempts; rejected {rejected} rows; "
            f"reasons {json.dumps(rejected_reasons, sort_keys=True)}"
        )

    generated = [accepted[plan.thread_id] for plan in plans]
    root = Path(output)
    root.mkdir(parents=True, exist_ok=True)
    from .data import write_jsonl

    write_jsonl(
        root / "threads.jsonl",
        (thread_record(thread, "synthetic-llm") for thread in generated),
    )
    rows = derive_rows(generated, source="synthetic-llm")
    files = [root / "threads.jsonl"]
    thresholds = (
        CorpusThresholds(
            min_rows=threads,
            min_paragraphs=threads // 2,
            min_obligations=4,
            min_thread_class_rows=0,
        )
        if smoke
        else CorpusThresholds()
    )
    for set_name in ("walk", "expiry"):
        path = root / f"{set_name}.jsonl"
        write_jsonl(path, (row for row in rows if row["set"] == set_name))
        check_corpus(path, thresholds)
        files.append(path)
    return {
        "threads": threads,
        "rejected": rejected,
        "rejected_reasons": rejected_reasons,
        "files": [str(path) for path in files],
    }


def _walk_row(thread, ask, role, text, unit, label, source) -> dict[str, Any]:
    return _with_id(
        {
            "set": "walk",
            "group": thread.plan.thread_id,
            "source": source,
            "user": {"name": thread.user_name},
            "waiting_party": thread.waiting_party,
            "ask": ask,
            "due": thread.due,
            "later": {"author_role": role, "text": text},
            "unit": unit,
            "label": label,
        }
    )


def _with_id(row: dict[str, Any]) -> dict[str, Any]:
    payload = {
        key: value
        for key, value in row.items()
        if key not in {"label", "set", "source", "id"}
    }
    digest = hashlib.sha256(repr(sorted(payload.items())).encode("utf-8")).hexdigest()[:20]
    return {**row, "id": f"{row['set']}-{digest}"}
