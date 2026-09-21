"""LLM-authored synthetic corpora plus a small deterministic test stub."""

from __future__ import annotations

import hashlib
import json
import os
import random
import re
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx

from .data import DatasetRow, load_jsonl, write_jsonl
from .questions import SETS, SPECS

DEFAULT_ROOT = Path(__file__).parents[1] / "data" / "synthetic"
DOMAINS = ("legal", "sales", "operations", "academic", "personal administration")
OBLIGATION_KINDS = ("deliverable", "review", "coordination")
RELATIONSHIPS = ("peer-to-peer", "requester-to-provider")
SYNTHETIC_PEOPLE = (
    "Priya Venkataraman", "Tomas Lindqvist", "Amara Okafor", "Mateo Villanueva",
    "Linnea Bergstrom", "Devon Fairchild", "Soraya Nwosu", "Kenji Takamura",
)
SYNTHETIC_COMPANIES = (
    "Halcyon Ridge LLC", "Juniper Vale Ltd", "Copper Lantern Inc", "Blue Heron Labs",
)
SYNTHETIC_SERVICES = (
    "notes@meetingnotes.example.invalid",
    "recap@calltranscripts.example.invalid",
    "noreply@notetaker.example.invalid",
)
HARD_NEGATIVES = (
    "thanks-only acknowledgement", "quoted history without a current update",
    "question about status without an answer", "unrelated update",
)
_EMAIL_RE = re.compile(r"\b[^\s@]+@[^\s@]+\b")
_URL_RE = re.compile(r"(?:https?://|www\.)\S+", re.IGNORECASE)
_CAPITALIZED_BIGRAM_RE = re.compile(r"\b([A-Z][a-z]+(?:\s+[A-Z][A-Za-z]+)+)\b")
# Capitalized words that start ordinary phrases in business mail. A run of
# capitalized words is treated as a name only when none of its words is here,
# so "Best Regards", "Monday Morning" and "Project Update" are not names while
# "Unlisted Person" still is.
_COMMON_CAPITALIZED_TEXT = (
        "Monday Tuesday Wednesday Thursday Friday Saturday Sunday January February March "
        "April May June July August September October November December Hi Hello Dear "
        "Thanks Thank Best Kind Warm Regards Cheers Sincerely Yours Please Re Fw Fwd Subject "
        "Sent From To Cc Date Attached Attachment Draft Final Revised Updated Project Client "
        "Team Board Committee Council Department Office Group Meeting Call Review Quarter Q1 "
        "Q2 Q3 Q4 Week Month Year Morning Afternoon Evening Today Tomorrow Yesterday Invoice "
        "Contract Agreement Brief Memo Report Proposal Plan Budget Summary Notes Minutes "
        "Agenda Update Status Reminder Deadline Request Action Item Items Next Steps Follow "
        "Up The This That These Those A An And Or But If When Where While After Before Since "
        "Until Legal Sales Operations Academic Personal Admin Administration Finance "
        "Marketing Support Engineering Research Program Programme Course Semester Term Class "
        "Lab Study Paper Thesis Order Purchase Vendor Supplier Customer Account Case Matter "
        "File Filing Court Hearing Trial Zoom Teams Meet Calendar Outlook Slack Email Phone "
        "Video Conference Room Suite Floor North South East West New Old Main Central Grand "
        "Upper Lower Inner Outer I We You They He She It My Our Your Their His Her Its"
)
_COMMON_CAPITALIZED_WORDS = frozenset(_COMMON_CAPITALIZED_TEXT.split())
# A named gathering for rules.event_match: the event name must contain one of
# these nouns, so a task such as "review the draft agenda" is rejected.
_EVENT_NOUN_RE = re.compile(
    r"\b(meeting|call|offsite|off-site|onsite|conference|hearing|workshop|kickoff|kick-off|"
    r"review session|review|sync|standup|stand-up|summit|deposition|trial|retreat|demo|"
    r"town hall|all-hands|webinar|session|briefing|presentation|interview|training|"
    r"orientation|ceremony|dinner|lunch|reception|event)\b",
    re.IGNORECASE,
)
# A deadline phrase for rules.deadline_kind is a fragment, not a sentence: no
# greeting with a comma, no sentence-final punctuation followed by more text.
_SENTENCE_SHAPE_RE = re.compile(r"^\s*[A-Z][a-z]+,|[.!?]\s+\S")
# A scoped_event request is the request itself, not a whole email.
_EMAIL_SHAPE_RE = re.compile(r"^\s*subject\s*:|^\s*(hi|hello|dear)\b", re.IGNORECASE)
_QUESTION_FIELDS = {
    **{question_id: ("subject", "paragraph_text") for question_id in SETS["triage"]},
    "rules.recap": ("sender", "subject", "first_paragraph"),
    "rules.scoped_event": ("request_text",),
    "rules.event_match": ("phrase", "event_name"),
    "rules.duplicate_action": ("action_a", "action_b"),
    "rules.thread_merge": (
        "subject_a", "subject_b", "first_paragraph_a", "first_paragraph_b",
    ),
    "rules.deadline_kind": ("phrase",),
}
_MAIN_TEXT_FIELD = {
    **{question_id: "paragraph_text" for question_id in SETS["triage"]},
    "rules.recap": "first_paragraph",
    "rules.scoped_event": "request_text",
    "rules.event_match": "phrase",
    "rules.duplicate_action": "action_a",
    "rules.thread_merge": "first_paragraph_a",
    "rules.deadline_kind": "phrase",
}
_HARD_NEGATIVE_GUIDANCE = {
    "triage.asks_recipient": "a quoted request, suggestion, or statement that asks nobody to act",
    "triage.commits_sender": "a hope, plan under discussion, or another person's commitment",
    "triage.asks_question": "a rhetorical or quoted question that seeks no answer",
    "triage.names_time": (
        "a number or sequence that is not a time, date, deadline, or event-relative time"
    ),
    "triage.boilerplate": (
        "ordinary prose mentioning a signature, disclaimer, or unsubscribe request"
    ),
    "triage.automated_notification": "a human-written note discussing an alert or notification",
    "rules.recap": (
        "a person writing about a meeting, including ordinary mail mentioning recap or notes"
    ),
    "rules.scoped_event": (
        "work unrelated to attendance, including meeting notes, minutes, or a recording"
    ),
    "rules.event_match": "a different event with overlapping words",
    "rules.duplicate_action": "different actions on the same topic that share most of their words",
    "rules.thread_merge": "different matters with deceptively similar subjects",
}


@dataclass(frozen=True)
class CorpusThresholds:
    min_rows: int = 600
    min_paragraphs: int = 300
    min_obligations: int = 40
    max_from_user_gap: float = 0.1
    max_days_mean_gap: float = 2.0
    positive_rate_min: float = 0.4
    positive_rate_max: float = 0.6
    # Four mutually exclusive closure labels cannot each have a 40% positive rate.
    closure_positive_rate_min: float = 0.15
    closure_positive_rate_max: float = 0.30


def scenario_seeds(question_id: str) -> list[dict[str, str]]:
    """Return the fixed 30-scenario matrix used to diversify one question."""

    if question_id not in {item for values in SETS.values() for item in values}:
        raise ValueError(f"unknown question: {question_id}")
    return [
        {"question": question_id, "domain": domain, "obligation_kind": kind,
         "relationship": relationship}
        for domain in DOMAINS
        for kind in OBLIGATION_KINDS
        for relationship in RELATIONSHIPS
    ]


def _normalize(value: str) -> str:
    return " ".join(value.casefold().split())


def _text_values(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, dict):
        # Sorted by key so the id does not depend on insertion order, which a
        # JSON round trip with sorted keys would otherwise change.
        return [text for _, item in sorted(value.items()) for text in _text_values(item)]
    if isinstance(value, list):
        return [text for item in value for text in _text_values(item)]
    return []


def _stable_id(set_name: str, row: dict[str, Any]) -> str:
    material = "\n".join(
        _normalize(text)
        for key, value in sorted(row.items())
        if key not in {"id", "label", "set", "source"}
        for text in _text_values(value)
    )
    return f"{set_name}-{hashlib.sha256(material.encode()).hexdigest()[:20]}"


def _dedupe_text(set_name: str, row: dict[str, Any]) -> str:
    if set_name == "closure":
        return _normalize(str(row["later"]["paragraph_text"]))
    if set_name == "triage":
        return _normalize(str(row["paragraph_text"]))
    return "\n".join(
        _normalize(text)
        for key, value in sorted(row.items())
        if key not in {"id", "label", "set", "source"}
        for text in _text_values(value)
    )


def _allowed_name_phrases() -> set[str]:
    allowed: set[str] = set()
    for name in (*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES):
        parts = name.split()
        for start in range(len(parts) - 1):
            for end in range(start + 2, len(parts) + 1):
                allowed.add(" ".join(parts[start:end]))
    return allowed


def scrub_row(row: dict[str, Any]) -> list[str]:
    """Return privacy/content violations without echoing the suspect values."""

    violations: list[str] = []
    text = "\n".join(_text_values(row))
    email_domains = {
        address.rstrip(".,;:)>").rsplit("@", 1)[-1].casefold()
        for address in _EMAIL_RE.findall(text)
    }
    if any(
        domain != "example.invalid" and not domain.endswith(".example.invalid")
        for domain in email_domains
    ):
        violations.append("non-example.invalid email address")
    if _URL_RE.search(text):
        violations.append("URL")
    allowed_names = _allowed_name_phrases()
    for match in _CAPITALIZED_BIGRAM_RE.finditer(text):
        phrase = match.group(1)
        if phrase in allowed_names:
            continue
        words = phrase.split()
        if any(word in _COMMON_CAPITALIZED_WORDS for word in words):
            continue
        # A name-shaped run that is not a pool name and contains no ordinary
        # capitalized word.
        violations.append("capitalized name outside the synthetic pool")
        break
    return violations


def _question_assignment(set_name: str, index: int, rows: int) -> tuple[str, int]:
    """Return a question and its zero-based local row number."""

    question_ids = tuple(
        question_id for question_id in SETS[set_name] if question_id != "closure.outcome"
    )
    quotient, remainder = divmod(rows, len(question_ids))
    offset = 0
    for ordinal, question_id in enumerate(question_ids):
        count = quotient + (ordinal < remainder)
        if index < offset + count:
            return question_id, index - offset
        offset += count
    raise IndexError(index)


def _control_plan(set_name: str, index: int, rows: int = 600) -> dict[str, Any]:
    question_ids = tuple(
        question_id for question_id in SETS[set_name] if question_id != "closure.outcome"
    )
    if set_name == "closure":
        target = (*question_ids, None)[index % (len(question_ids) + 1)]
        labels = {question_id: question_id == target for question_id in question_ids}
        return {
            "label": labels,
            "from_user": bool((index // (len(question_ids) + 1)) % 2),
            "days_later": 1 + (index // (2 * (len(question_ids) + 1))) % 14,
            "hard_negative": (
                HARD_NEGATIVES[index % len(HARD_NEGATIVES)] if target is None else None
            ),
            "focus_question": target or question_ids[index % len(question_ids)],
        }
    question_id, local_index = _question_assignment(set_name, index, rows)
    value: bool | str
    if question_id == "rules.deadline_kind":
        value = ("event_tied", "soft", "unknown")[local_index % 3]
    else:
        value = bool(local_index % 2)
    return {
        "row_index": index,
        "label": {question_id: value},
        # Within each boolean label, this alternates after the label itself,
        # producing equal from_user rates for positives and negatives.
        **({"from_user": bool((local_index // 2) % 2)} if set_name == "triage" else {}),
        "focus_question": question_id,
        "hard_negative": value is False,
    }


def _person_sender(index: int) -> str:
    person = SYNTHETIC_PEOPLE[index % len(SYNTHETIC_PEOPLE)]
    mailbox = person.casefold().replace(" ", ".")
    return f"{person} <{mailbox}@example.invalid>"


def _stub_row(set_name: str, index: int, rows: int = 600) -> dict[str, Any]:
    plan = _control_plan(set_name, index, rows)
    person = SYNTHETIC_PEOPLE[index % len(SYNTHETIC_PEOPLE)]
    company = SYNTHETIC_COMPANIES[(index // len(SYNTHETIC_PEOPLE)) % len(SYNTHETIC_COMPANIES)]
    marker = f"case {index:03d}"
    if set_name == "closure":
        target = next((q for q, value in plan["label"].items() if value), None)
        outcomes = {
            "closure.fulfilled": "reports that the requested item was delivered",
            "closure.withdrawn": "withdraws the earlier request",
            "closure.deadline_changed": "moves the deadline to next Thursday",
            "closure.modified": "changes the requested item to a short summary",
        }
        paragraph = (
            f"{person} {outcomes[target]} for {company}; reference {marker}."
            if target
            else f"{person} asks whether there is an update for {company}; reference {marker}."
        )
        row = {
            "set": set_name,
            "obligation": {"title": f"Synthetic obligation {marker}",
                           "evidence_text": f"Please prepare item {index:03d} for {company}."},
            "later": {"paragraph_text": paragraph, "from_user": plan["from_user"],
                      "days_later": plan["days_later"]},
            "label": plan["label"],
        }
    elif set_name == "triage":
        question_id, value = next(iter(plan["label"].items()))
        row = {
            "set": set_name, "subject": f"Synthetic coordination {marker}",
            "paragraph_text": (
                f"{person} writes to {company} about {marker}; "
                f"the {question_id.rsplit('.', 1)[1]} example is {str(value).lower()}."
            ),
            "from_user": plan["from_user"], "label": plan["label"],
        }
    else:
        question_id, value = next(iter(plan["label"].items()))
        fields: dict[str, str]
        if question_id == "rules.recap":
            fields = {
                "sender": (
                    SYNTHETIC_SERVICES[index % len(SYNTHETIC_SERVICES)]
                    if value else _person_sender(index)
                ),
                "subject": (
                    f"Meeting recap: review {marker}"
                    if value else f"Notes question for {marker}"
                ),
                "first_paragraph": (
                    f"Summary for {company}: decisions are recorded. "
                    f"Action items: prepare {marker}."
                    if value
                    else f"{person} asks whether the meeting notes include {marker} for {company}."
                ),
            }
        elif question_id == "rules.scoped_event":
            fields = {"request_text": (
                f"Please bring the agenda for {marker} to the review meeting."
                if value else f"Please archive the meeting recording for {marker}."
            )}
        elif question_id == "rules.event_match":
            fields = {
                "phrase": (
                    f"the planning offsite for {marker}"
                    if value else f"the sales review for {marker}"
                ),
                "event_name": (
                    f"q3 planning offsite for {marker}"
                    if value else f"q3 planning review for {marker}"
                ),
            }
        elif question_id == "rules.duplicate_action":
            fields = {
                "action_a": f"Prepare the vendor brief for {marker}.",
                "action_b": (
                    f"Draft the briefing document for the vendor matter {marker}."
                    if value else f"Approve the vendor brief for {marker}."
                ),
            }
        elif question_id == "rules.thread_merge":
            fields = {
                "subject_a": f"Vendor review {marker}",
                "subject_b": (
                    f"Re: vendor review {marker}"
                    if value else f"Vendor review invoice {marker}"
                ),
                "first_paragraph_a": f"Please review the vendor terms for {marker}.",
                "first_paragraph_b": (
                    f"Following up on those vendor terms for {marker}."
                    if value else f"Please review the vendor invoice for {marker}."
                ),
            }
        else:
            phrases = {
                "event_tied": f"before the board meeting for {marker}",
                "soft": f"whenever you get a chance on {marker}",
                "unknown": f"after maybe around the {marker} point",
            }
            fields = {"phrase": phrases[str(value)]}
        row = {"set": set_name, **fields, "label": plan["label"]}
    row["source"] = "synthetic-stub"
    row["id"] = _stable_id(set_name, row)
    return row


def generate_stub(
    output: str | Path, *, selected_set: str = "all", rows: int = 60, seed: int = 20260919
) -> list[Path]:
    """Write small deterministic fixtures. The seed controls only row ordering."""

    if rows < 10:
        raise ValueError("stub generation requires at least 10 rows")
    names = tuple(name for name in SETS if name != "extract") if selected_set == "all" else (
        selected_set,
    )
    if "extract" in names:
        raise ValueError("extract rows are derived from the triage corpus")
    root = Path(output)
    paths: list[Path] = []
    for offset, set_name in enumerate(names):
        generated = [_stub_row(set_name, index, rows) for index in range(rows)]
        random.Random(seed + offset).shuffle(generated)
        path = root / f"{set_name}.jsonl"
        write_jsonl(path, generated)
        paths.append(path)
    return paths


def _prompt(
    set_name: str, plans: list[dict[str, Any]], question_id: str | None = None
) -> str:
    if set_name == "closure":
        return (
            "Create realistic but fully invented evaluation rows. Return strict JSON as an object "
            "with one member named rows whose value is an array in exactly the requested order. "
            "Each row has these fields: obligation {title,evidence_text}; later "
            "{paragraph_text,from_user,days_later}; label. Copy every supplied label and assigned "
            "from_user/days_later value exactly. Write each text as one coherent situation. "
            "Closure rows may express exactly the one true closure label, or none; they must not "
            "imply another closure outcome. When hard_negative is set, implement that negative "
            "pattern. Use only these invented people and organizations: "
            f"{', '.join((*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES))}. Never name any other person, "
            "company, product, place or document title; write project, document and product names "
            "in lowercase. Any email must end in example.invalid. Do not emit URLs. Plans:\n"
            + json.dumps(plans, ensure_ascii=False, sort_keys=True)
        )
    if question_id is None:
        question_id = str(plans[0]["focus_question"])
    output_fields = [*_QUESTION_FIELDS[question_id], "label"]
    if set_name == "triage":
        output_fields.insert(-1, "from_user")
    fields = "; ".join(output_fields)
    definition = SPECS[question_id].intent
    true_pattern = {
        "rules.recap": (
            "Use a sender exactly from the synthetic service list, a subject such as "
            "'Meeting recap: ...', 'Your call summary', or 'Transcript: ...', and a first "
            "paragraph structured as a summary with decisions or action items."
        ),
        "rules.scoped_event": (
            "Make the request about attending, preparing for, or bringing something to an "
            "event. request_text is the request itself: one or two sentences, no subject line, "
            "greeting or signature."
        ),
        "rules.event_match": (
            "event_name is the name of a gathering (a meeting, call, offsite, hearing, "
            "conference, workshop, kickoff, review session), for example 'Q3 planning offsite' "
            "or 'Copper Lantern contract review call', never a task. Make the phrase refer to "
            "that gathering by paraphrase, abbreviation, or shorthand."
        ),
        "rules.duplicate_action": (
            "Express the same action in substantially different wording; never copy either "
            "sentence."
        ),
        "rules.thread_merge": (
            "Show one thread despite a reply prefix, typo, or reasonable retitle."
        ),
    }.get(question_id, f"Make the text clearly satisfy this definition: {definition}")
    if question_id == "rules.deadline_kind":
        label_guidance = (
            "The label is a choice: event_tied uses an event-relative deadline such as 'before "
            "the board meeting'; soft uses flexible timing such as 'whenever you get a chance'; "
            "unknown is garbled or ambiguous. Every phrase must be a deadline expression that a "
            "simple deadline parser cannot parse. phrase is only the deadline expression as it "
            "would appear inside a sentence, at most eight words (for example 'before the "
            "board meeting', 'whenever you get a chance'): no greeting, name, or full sentence."
        )
    else:
        label_guidance = (
            f"For label true: {true_pattern} For label false: make it genuinely false. "
            f"False hard negatives must use this pattern: {_HARD_NEGATIVE_GUIDANCE[question_id]}."
        )
    people_and_companies = ", ".join((*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES))
    service_guidance = (
        f" Synthetic service senders (only for true recap rows): {', '.join(SYNTHETIC_SERVICES)}."
        if question_id == "rules.recap" else ""
    )
    return (
        "Create realistic but fully invented evaluation rows. Return strict JSON as an object "
        "with one member named rows whose value is an array in exactly the requested order. "
        f"This batch is only for question {question_id}. Definition: {definition} "
        f"Write exactly these fields and no others: {fields}. Copy the supplied label and any "
        "from_user value exactly; validation stamps them from the plan. "
        + label_guidance + service_guidance + " "
        "Write each row as one coherent situation. Use only these invented people and "
        "organizations: "
        f"{people_and_companies}. Never name any other person, "
        "company, product, place or document title; write project, document and product names "
        "in lowercase (for example: the licensing brief, the vendor contract). Human senders must "
        "include a pool person's full name and may use only an example.invalid address. Do not "
        "emit "
        "URLs. Vary language, artifacts, relationships, and contexts. Plans:\n"
        + json.dumps(plans, ensure_ascii=False, sort_keys=True)
    )


def _response_format(set_name: str, question_id: str | None = None) -> dict[str, Any]:
    label_properties = {
        question_id: (
            {"type": "string", "enum": ["event_tied", "soft", "unknown"]}
            if question_id == "rules.deadline_kind"
            else {"type": "boolean"}
        )
        for question_id in SETS[set_name]
    }
    label = {
        "type": "object", "properties": label_properties,
        "required": list(label_properties), "additionalProperties": False,
    }
    if set_name == "closure":
        properties: dict[str, Any] = {
            "obligation": {
                "type": "object",
                "properties": {"title": {"type": "string"},
                               "evidence_text": {"type": "string"}},
                "required": ["title", "evidence_text"], "additionalProperties": False,
            },
            "later": {
                "type": "object",
                "properties": {"paragraph_text": {"type": "string"},
                               "from_user": {"type": "boolean"},
                               "days_later": {"type": "integer"}},
                "required": ["paragraph_text", "from_user", "days_later"],
                "additionalProperties": False,
            },
            "label": label,
        }
    elif question_id is None:
        raise ValueError("question_id is required for per-question synthetic rows")
    else:
        properties = {
            field: {"type": "string"}
            for field in _QUESTION_FIELDS[question_id]
        }
        if set_name == "triage":
            properties["from_user"] = {"type": "boolean"}
        properties["label"] = {
            "type": "object",
            "properties": {question_id: label_properties[question_id]},
            "required": [question_id],
            "additionalProperties": False,
        }
    row_schema = {
        "type": "object", "properties": properties,
        "required": list(properties), "additionalProperties": False,
    }
    return {
        "type": "json_schema",
        "json_schema": {
            "name": f"synthetic_{set_name}_{(question_id or 'all').replace('.', '_')}_rows",
            "strict": True,
            "schema": {
                "type": "object", "properties": {
                    "rows": {"type": "array", "items": row_schema}
                },
                "required": ["rows"], "additionalProperties": False,
            },
        },
    }


def _chat_rows(
    client: httpx.Client, model: str, api_key: str, set_name: str, plans: list[dict[str, Any]],
    question_id: str | None = None,
) -> list[dict[str, Any]]:
    response = client.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        json={"model": model,
              "messages": [{"role": "user", "content": _prompt(set_name, plans, question_id)}],
              "response_format": _response_format(set_name, question_id),
              "provider": {"zdr": True},
              "temperature": 0.9,
              # Twelve rows of JSON run to several thousand tokens; a default
              # output cap truncates the reply mid-string.
              "max_tokens": 16384},
    )
    response.raise_for_status()
    choice = response.json()["choices"][0]
    content = choice["message"]["content"]
    try:
        payload = json.loads(content)
    except json.JSONDecodeError:
        # A truncated or malformed reply is a failed batch; the caller
        # re-queues its plans rather than aborting the whole set.
        return []
    if not isinstance(payload, dict) or not isinstance(payload.get("rows"), list):
        return []
    return payload["rows"]


def _nonempty_strings(mapping: Any, keys: tuple[str, ...]) -> bool:
    return isinstance(mapping, dict) and all(
        isinstance(mapping.get(key), str) and mapping[key].strip() for key in keys
    )


def _action_tokens(value: str) -> set[str]:
    return set(re.findall(r"[a-z0-9]+", value.casefold()))


def _duplicate_action_too_similar(action_a: str, action_b: str) -> bool:
    if _normalize(action_a) == _normalize(action_b):
        return True
    left, right = _action_tokens(action_a), _action_tokens(action_b)
    union = left | right
    return bool(union) and len(left & right) / len(union) >= 0.9


def _is_pool_person_sender(sender: str) -> bool:
    normalized = sender.casefold()
    return any(person.casefold() in normalized for person in SYNTHETIC_PEOPLE)


def _validate_generated_row(
    set_name: str, row: Any, plan: dict[str, Any]
) -> tuple[dict[str, Any] | None, str]:
    """Return the accepted row and ``"ok"``, or ``None`` and the rejection reason.

    The model writes only the texts. The label, ``from_user`` and
    ``days_later`` are stamped from the plan, so a row is never rejected for
    failing to echo a value the plan already fixed.
    """

    if not isinstance(row, dict):
        return None, "not an object"
    if set_name == "closure":
        if not _nonempty_strings(row.get("obligation"), ("title", "evidence_text")):
            return None, "obligation texts missing"
        if not _nonempty_strings(row.get("later"), ("paragraph_text",)):
            return None, "later paragraph missing"
        built: dict[str, Any] = {
            "obligation": {
                "title": row["obligation"]["title"],
                "evidence_text": row["obligation"]["evidence_text"],
            },
            "later": {
                "paragraph_text": row["later"]["paragraph_text"],
                "from_user": plan["from_user"],
                "days_later": plan["days_later"],
            },
        }
    elif set_name == "triage":
        question_id = str(plan["focus_question"])
        fields = _QUESTION_FIELDS[question_id]
        if set(row) != {*fields, "from_user", "label"} or not _nonempty_strings(
            row, fields
        ):
            return None, "triage texts missing"
        built = {
            "subject": row["subject"],
            "paragraph_text": row["paragraph_text"],
            "from_user": plan["from_user"],
        }
    else:
        question_id = str(plan["focus_question"])
        text_keys = _QUESTION_FIELDS[question_id]
        if set(row) != {*text_keys, "label"} or not _nonempty_strings(row, text_keys):
            return None, "rules texts missing"
        built = {key: row[key] for key in text_keys}
        value = plan["label"][question_id]
        if (
            question_id == "rules.duplicate_action"
            and value is True
            and _duplicate_action_too_similar(built["action_a"], built["action_b"])
        ):
            return None, "duplicate actions use identical or near-identical text"
        if question_id == "rules.recap":
            sender = built["sender"]
            if value is True and sender.casefold() not in {
                service.casefold() for service in SYNTHETIC_SERVICES
            }:
                return None, "positive recap sender is not a synthetic service"
            if value is False and not _is_pool_person_sender(sender):
                return None, "negative recap sender is not a pool person"
        if question_id == "rules.event_match" and not _EVENT_NOUN_RE.search(
            built["event_name"]
        ):
            return None, "event_name is not a named gathering"
        if question_id == "rules.deadline_kind" and (
            len(built["phrase"].split()) > 12 or _SENTENCE_SHAPE_RE.search(built["phrase"])
        ):
            return None, "deadline phrase is a sentence, not a phrase"
        if question_id == "rules.scoped_event" and _EMAIL_SHAPE_RE.search(
            built["request_text"]
        ):
            return None, "request_text is a whole email, not a request"
    built["label"] = dict(plan["label"])
    built["set"] = set_name
    built["source"] = "synthetic-llm"
    scrub = scrub_row(built)
    if scrub:
        return None, "scrub: " + scrub[0]
    built["id"] = _stable_id(set_name, built)
    try:
        DatasetRow.model_validate(built)
    except ValueError:
        return None, "row failed validation"
    return built, "ok"


def generate_llm(
    output: str | Path = DEFAULT_ROOT, *, selected_set: str = "all", rows: int = 600,
    seed: int = 20260919, client: httpx.Client | None = None, parallel: int = 8,
    smoke: bool = False,
) -> list[Path]:
    """Generate corpora through OpenRouter; callers own the live-network decision.

    Batches of 12 plans go to the model ``parallel`` at a time (the calls are
    independent); acceptance and de-duplication stay serial so the result
    does not depend on completion order. ``smoke`` allows a small row count
    and relaxed corpus checks for a cheap live trial; its output must never
    replace the committed corpus.
    """

    if rows < 600 and not smoke:
        raise ValueError("LLM generation requires at least 600 rows per set")
    if smoke and rows < 128:
        raise ValueError("a smoke run needs at least 128 rows to balance every label")
    parallel = max(1, parallel)
    api_key = os.environ.get("OPENROUTER_API_KEY")
    model = os.environ.get("OPENROUTER_SYNTH_MODEL")
    if not api_key or not model:
        raise RuntimeError("OPENROUTER_API_KEY and OPENROUTER_SYNTH_MODEL must be set")
    names = tuple(name for name in SETS if name != "extract") if selected_set == "all" else (
        selected_set,
    )
    if "extract" in names:
        raise ValueError("extract rows are derived from the triage corpus")
    root = Path(output)
    owned_client = client is None
    http = client or httpx.Client(timeout=120.0)
    paths: list[Path] = []
    try:
        for set_name in names:
            accepted: dict[int, dict[str, Any]] = {}
            seen_ids: set[str] = set()
            seen_texts: set[str] = set()
            pending = [(index, _control_plan(set_name, index, rows)) for index in range(rows)]
            for index, plan in pending:
                seeds = scenario_seeds(plan["focus_question"])
                plan["scenario" if set_name == "closure" else "scenario_seed"] = seeds[
                    index % len(seeds)
                ]
            attempts = 0
            rejections: dict[str, int] = {}
            while len(accepted) < rows and attempts < rows * 8:
                batches: list[list[tuple[int, dict[str, Any]]]] = []
                round_items: list[tuple[int, dict[str, Any]]] = []
                while pending and len(batches) < parallel:
                    focus = pending[0][1]["focus_question"]
                    take = 0
                    while (
                        take < min(12, len(pending))
                        and (set_name == "closure" or pending[take][1]["focus_question"] == focus)
                    ):
                        take += 1
                    batch, pending = pending[:take], pending[take:]
                    batches.append(batch)
                    round_items.extend(batch)

                def request_batch(batch: list[tuple[int, dict[str, Any]]], name: str = set_name):
                    question_id = None if name == "closure" else batch[0][1]["focus_question"]
                    return _chat_rows(
                        http, model, api_key, name, [plan for _i, plan in batch], question_id
                    )

                with ThreadPoolExecutor(max_workers=parallel) as pool:
                    results = list(pool.map(request_batch, batches))
                completed: set[int] = set()
                for batch, candidates in zip(batches, results, strict=True):
                    if len(candidates) < len(batch):
                        rejections["short batch"] = (
                            rejections.get("short batch", 0) + len(batch) - len(candidates)
                        )
                    for candidate, (index, plan) in zip(candidates, batch, strict=False):
                        validated, reason = _validate_generated_row(set_name, candidate, plan)
                        if validated is None:
                            rejections[reason] = rejections.get(reason, 0) + 1
                            continue
                        normalized_text = _dedupe_text(set_name, validated)
                        if validated["id"] in seen_ids or normalized_text in seen_texts:
                            rejections["duplicate"] = rejections.get("duplicate", 0) + 1
                            continue
                        accepted[index] = validated
                        seen_ids.add(validated["id"])
                        seen_texts.add(normalized_text)
                        completed.add(index)
                failed = [item for item in round_items if item[0] not in completed]
                pending.extend(failed)
                attempts += len(round_items)
            if len(accepted) < rows:
                summary = ", ".join(
                    f"{reason}: {count}" for reason, count in sorted(rejections.items())
                )
                raise RuntimeError(
                    "synthetic generation produced only "
                    f"{len(accepted)} valid distinct {set_name} rows; rejections: {summary}"
                )
            generated = [accepted[index] for index in range(rows)]
            random.Random(seed).shuffle(generated)
            path = root / f"{set_name}.jsonl"
            temporary = path.with_suffix(".jsonl.tmp")
            write_jsonl(temporary, generated)
            thresholds = (
                CorpusThresholds(min_rows=rows, min_paragraphs=rows // 2, min_obligations=4)
                if smoke
                else CorpusThresholds()
            )
            try:
                check_corpus(temporary, thresholds)
                temporary.replace(path)
            except Exception:
                temporary.unlink(missing_ok=True)
                raise
            summary = ", ".join(
                f"{reason}: {count}" for reason, count in sorted(rejections.items())
            ) or "none"
            print(
                f"{set_name}: accepted {rows} rows from {attempts} plans; "
                f"rejections: {summary}"
            )
            paths.append(path)
    finally:
        if owned_client:
            http.close()
    return paths


def _paragraph_text(row: DatasetRow) -> str:
    if row.set == "closure":
        return str(row.later["paragraph_text"])
    if row.set == "triage":
        return str(row.paragraph_text)
    question_id = next(iter(row.label))
    return str(getattr(row, _MAIN_TEXT_FIELD[question_id]))


def check_corpus(
    path: str | Path, thresholds: CorpusThresholds | None = None
) -> dict[str, Any]:
    """Validate corpus diversity, balance, provenance, and closure exclusivity."""

    thresholds = thresholds or CorpusThresholds()
    rows = load_jsonl(path)
    if not rows:
        raise ValueError("corpus is empty")
    set_names = {row.set for row in rows}
    if len(set_names) != 1:
        raise ValueError("corpus must contain exactly one set")
    set_name = set_names.pop()
    errors: list[str] = []
    if len(rows) < thresholds.min_rows:
        errors.append(f"rows {len(rows)} < {thresholds.min_rows}")
    if set_name != "closure":
        expected_label_count = 2 if set_name == "triage" else 1
        if any(len(row.label) != expected_label_count for row in rows):
            errors.append("non-closure rows must carry exactly one question label")
        malformed = 0
        for row in rows:
            own_labels = [question_id for question_id in SETS[set_name] if question_id in row.label]
            if len(row.label) != expected_label_count or len(own_labels) != 1:
                continue
            question_id = own_labels[0]
            if question_id not in _QUESTION_FIELDS:
                malformed += 1
                continue
            expected = {"id", "set", "label", "source", *_QUESTION_FIELDS[question_id]}
            if set_name == "triage":
                expected.add("from_user")
            if set(row.model_dump()) != expected:
                malformed += 1
        if malformed:
            errors.append(f"{malformed} rows do not match their question field set")
    if any(row.id != _stable_id(row.set, row.model_dump()) for row in rows):
        errors.append("one or more row ids are not stable text hashes")
    scrubbed = sum(bool(scrub_row(row.model_dump())) for row in rows)
    if scrubbed:
        errors.append(f"privacy scrub rejected {scrubbed} rows")
    paragraphs = {_normalize(_paragraph_text(row)) for row in rows}
    if len(paragraphs) < thresholds.min_paragraphs:
        errors.append(f"distinct paragraph texts {len(paragraphs)} < {thresholds.min_paragraphs}")
    obligations: set[str] = set()
    if set_name == "closure":
        obligations = {_normalize(str(row.obligation["evidence_text"])) for row in rows}
        if len(obligations) < thresholds.min_obligations:
            errors.append(
                f"distinct obligation texts {len(obligations)} < {thresholds.min_obligations}"
            )
        if any(sum(value is True for value in row.label.values()) > 1 for row in rows):
            errors.append("a closure row is positive for more than one closure question")
    question_stats: dict[str, Any] = {}
    for question_id in SETS[set_name]:
        question_rows = [row for row in rows if question_id in row.label]
        minimum_question_rows = min(
            80, thresholds.min_rows // len(SETS[set_name])
        )
        if len(question_rows) < minimum_question_rows:
            errors.append(
                f"{question_id} rows {len(question_rows)} < {minimum_question_rows}"
            )
        values = [row.label[question_id] for row in question_rows]
        if not all(isinstance(value, bool) for value in values):
            options = tuple((SPECS[question_id].options or {}).keys())
            counts = {
                option: values.count(option)
                for option in options
            }
            if set(values) != set(counts):
                errors.append(f"{question_id} has an unknown or missing choice label")
            if question_id != "closure.outcome" and (
                not counts or max(counts.values()) - min(counts.values()) > 1
            ):
                errors.append(f"{question_id} choice labels are not evenly balanced")
            question_stats[question_id] = {
                "rows": len(question_rows),
                "label_counts": counts,
            }
            continue
        positives = [
            row for row, value in zip(question_rows, values, strict=True) if value
        ]
        negatives = [
            row for row, value in zip(question_rows, values, strict=True) if not value
        ]
        rate = len(positives) / len(question_rows) if question_rows else 0.0
        low, high = (
            (thresholds.closure_positive_rate_min, thresholds.closure_positive_rate_max)
            if set_name == "closure"
            else (thresholds.positive_rate_min, thresholds.positive_rate_max)
        )
        if not low <= rate <= high:
            errors.append(f"{question_id} positive rate {rate:.3f} outside [{low}, {high}]")
        stats: dict[str, Any] = {"rows": len(question_rows), "positive_rate": rate}
        if positives and negatives and set_name == "triage":
            pos_rate = sum(bool(row.from_user) for row in positives) / len(positives)
            neg_rate = sum(bool(row.from_user) for row in negatives) / len(negatives)
            stats["from_user_gap"] = abs(pos_rate - neg_rate)
        elif positives and negatives and set_name == "closure":
            pos_rate = sum(bool(row.later["from_user"]) for row in positives) / len(positives)
            neg_rate = sum(bool(row.later["from_user"]) for row in negatives) / len(negatives)
            stats["from_user_gap"] = abs(pos_rate - neg_rate)
            pos_days = sum(float(row.later["days_later"]) for row in positives) / len(positives)
            neg_days = sum(float(row.later["days_later"]) for row in negatives) / len(negatives)
            stats["days_later_mean_gap"] = abs(pos_days - neg_days)
            if stats["days_later_mean_gap"] > thresholds.max_days_mean_gap:
                errors.append(
                    f"{question_id} days_later mean gap "
                    f"{stats['days_later_mean_gap']:.3f} > {thresholds.max_days_mean_gap}"
                )
        if stats.get("from_user_gap", 0.0) > thresholds.max_from_user_gap:
            errors.append(
                f"{question_id} from_user gap {stats['from_user_gap']:.3f} > "
                f"{thresholds.max_from_user_gap}"
            )
        if question_id == "rules.duplicate_action" and any(
            _normalize(str(row.action_a)) == _normalize(str(row.action_b))
            for row in positives
        ):
            errors.append("rules.duplicate_action has an identical true action pair")
        question_stats[question_id] = stats
    report = {
        "set": set_name, "rows": len(rows), "distinct_paragraph_texts": len(paragraphs),
        "distinct_obligation_texts": len(obligations) if set_name == "closure" else None,
        "questions": question_stats, "errors": errors,
    }
    if errors:
        raise ValueError("; ".join(errors))
    return report
