"""Deterministic, construction-labeled synthetic business-email corpora."""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any

NAMES = ("Avery Quill", "Briar Voss", "Cato Mere", "Della Rune", "Ember Pike")
PROJECTS = ("Lumen", "Mosaic", "Northstar", "Orbit", "Pine")


def _write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as stream:
        for row in rows:
            stream.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def _closure_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    cases = (
        ("I sent the revised brief this morning.", "closure.fulfilled"),
        ("Please disregard that request; it is no longer needed.", "closure.withdrawn"),
        ("The new deadline is Friday at 3:00 PM.", "closure.deadline_changed"),
        ("Please send a one-page summary instead of the full report.", "closure.modified"),
        ("Thanks for the update.", None),
        ("> I sent the revised brief this morning.\nThanks.", None),
        ("The deadline has moved, and I will confirm the date later.", "closure.deadline_changed"),
        ("The deadline remains Friday; no change is needed.", None),
    )
    rows = []
    ids = (
        "closure.fulfilled",
        "closure.withdrawn",
        "closure.deadline_changed",
        "closure.modified",
    )
    for index in range(count):
        text, positive = cases[index % len(cases)]
        project = rng.choice(PROJECTS)
        rows.append(
            {
                "id": f"closure-{index:04d}",
                "set": "closure",
                "obligation": {
                    "title": f"Send the {project} brief",
                    "evidence_text": f"Please send the {project} brief by Thursday.",
                },
                "later": {
                    "paragraph_text": text,
                    "from_user": bool(index % 2),
                    "days_later": 1 + index % 14,
                },
                "label": {question_id: question_id == positive for question_id in ids},
                "source": "synthetic",
            }
        )
    return rows


def _triage_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    cases: tuple[tuple[str, tuple[str, ...]], ...] = (
        (
            "Could you send the signed form by Tuesday?",
            ("asks_recipient", "asks_question", "names_time"),
        ),
        ("I will deliver the draft tomorrow.", ("commits_sender", "names_time")),
        ("Wouldn't it be nice if every launch were easy?", ()),
        ("Avery, please ask Briar to review the plan.", ()),
        ("Regards,\nCato Mere\ncato.mere@example.invalid", ("boilerplate",)),
        ("To unsubscribe from these notices, update your preferences.", ("boilerplate",)),
        ("This is an automated notification. Do not reply.", ("automated_notification",)),
        ("Thanks for the update.", ()),
        ("> Could you send the form?\nAcknowledged.", ()),
        ("Please bring the sample to the launch review.", ("asks_recipient",)),
    )
    keys = (
        "asks_recipient",
        "commits_sender",
        "asks_question",
        "names_time",
        "boilerplate",
        "automated_notification",
    )
    rows = []
    for index in range(count):
        text, positives = cases[index % len(cases)]
        project = rng.choice(PROJECTS)
        rows.append(
            {
                "id": f"triage-{index:04d}",
                "set": "triage",
                "subject": f"{project} coordination",
                "paragraph_text": text,
                "from_user": bool(index % 2),
                "label": {f"triage.{key}": key in positives for key in keys},
                "source": "synthetic",
            }
        )
    return rows


def _rules_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    variants = (
        {
            "sender": "recorder@example.invalid",
            "subject": "Automated meeting recap",
            "first_paragraph": "Summary and transcript from the Lumen review.",
            "request_text": "Bring the prototype to the launch review.",
            "phrase": "before the launch",
            "event_name": "Launch review",
            "action_a": "Send the revised brief.",
            "action_b": "Please deliver the updated brief.",
            "subject_a": "Lumen launch",
            "subject_b": "Re: Lumen launch",
            "first_paragraph_a": "Review the Lumen plan.",
            "first_paragraph_b": "Following up on the Lumen plan.",
            "deadline_phrase": "before the launch",
            "bools": (True, True, True, True, True),
            "kind": "event_tied",
        },
        {
            "sender": "della.rune@example.invalid",
            "subject": "Thanks",
            "first_paragraph": "Thanks for the update.",
            "request_text": "Send the ledger when convenient.",
            "phrase": "when convenient",
            "event_name": "Quarterly review",
            "action_a": "Send the ledger.",
            "action_b": "Book the venue.",
            "subject_a": "Ledger",
            "subject_b": "Venue",
            "first_paragraph_a": "Please send the ledger.",
            "first_paragraph_b": "Please book the venue.",
            "deadline_phrase": "when convenient",
            "bools": (False, False, False, False, False),
            "kind": "soft",
        },
        {
            "sender": "ember.pike@example.invalid",
            "subject": "Project note",
            "first_paragraph": "An ordinary project update follows.",
            "request_text": "Check the totals.",
            "phrase": "soon",
            "event_name": "Planning session",
            "action_a": "Check the totals.",
            "action_b": "Confirm the totals.",
            "subject_a": "Totals",
            "subject_b": "Re: Totals",
            "first_paragraph_a": "Check the totals.",
            "first_paragraph_b": "Confirm the totals.",
            "deadline_phrase": "after a while",
            "bools": (False, False, False, True, True),
            "kind": "unknown",
        },
    )
    rows = []
    bool_ids = (
        "rules.recap",
        "rules.scoped_event",
        "rules.event_match",
        "rules.duplicate_action",
        "rules.thread_merge",
    )
    for index in range(count):
        case = dict(variants[index % len(variants)])
        bools = case.pop("bools")
        kind = case.pop("kind")
        case["subject"] = f"{case['subject']} {rng.choice(PROJECTS)}"
        rows.append(
            {
                "id": f"rules-{index:04d}",
                "set": "rules",
                **case,
                "phrase": case["deadline_phrase"] if index % 2 else case["phrase"],
                "label": {**dict(zip(bool_ids, bools, strict=True)), "rules.deadline_kind": kind},
                "source": "synthetic",
            }
        )
    return rows


def generate_synthetic(
    output: str | Path | None = None, seed: int = 20260919, count: int = 360
) -> list[Path]:
    if count < 300:
        raise ValueError("each bootstrap set requires at least 300 rows")
    root = Path(output or Path(__file__).parents[1] / "data" / "synthetic")
    rng = random.Random(seed)
    sets = {
        "closure": _closure_rows(rng, count),
        "triage": _triage_rows(rng, count),
        "rules": _rules_rows(rng, count),
    }
    paths = []
    for name, rows in sets.items():
        path = root / f"{name}.jsonl"
        _write_jsonl(path, rows)
        paths.append(path)
    return paths


if __name__ == "__main__":
    generate_synthetic()
