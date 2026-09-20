"""Deterministic, balanced, construction-labeled synthetic business-email corpora."""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any

PROJECTS = ("Lumen", "Mosaic", "Northstar", "Orbit", "Pine")

CLOSURE_POSITIVES = {
    "closure.fulfilled": (
        "I sent the revised brief this morning.", "The requested brief was delivered today.",
        "I completed the review and shared it.", "The signed form has been submitted.",
        "We finished the draft and sent it.", "I uploaded the completed worksheet.",
        "The final memo reached the recipient.", "I handled the requested filing yesterday.",
        "The package is complete and delivered.", "I have carried out the requested update.",
        'History says "the brief was pending." I sent it today.',
        "We completed and delivered the requested item.",
    ),
    "closure.withdrawn": (
        "Please disregard the request.", "The obligation is withdrawn.",
        "That task is no longer needed.", "I cancel the earlier request.",
        "Please drop the requested work.", "The assignment is now cancelled.",
        "Never mind about that task.", "I withdraw the original request.",
        "The requested action is no longer required.", "Please stop work on the earlier request.",
        'The prior note says "please send it." I withdraw that request.',
        "The prior assignment is cancelled.",
    ),
    "closure.deadline_changed": (
        "The new deadline is Friday afternoon.", "The due date moves to next Monday.",
        "Delivery is now due tomorrow morning.", "The deadline has changed to Wednesday.",
        "Completion is due before the next review.", "The revised due date is Tuesday.",
        "The task is now due at the end of the week.", "Submission moves to Thursday morning.",
        "The delivery date changes to the next business day.",
        "The deadline is postponed until the planning session.",
        'History lists "Thursday." The new deadline is Friday.',
        "The new delivery window ends Friday.",
    ),
    "closure.modified": (
        "Please send a short summary instead of the full report.",
        "The request now includes the supporting worksheet.",
        "Please deliver the document to the review team instead.",
        "The required format changes to a one-page memo.",
        "Add the approval record to the requested package.",
        "The assignment now covers the revised totals.",
        "Please replace the draft with the signed version.",
        "The request adds a separate list of open items.",
        "Use the updated template for the deliverable.",
        "Please prepare two summaries rather than one report.",
        'The original says "full report." Please send a short summary instead.',
        "Include the source notes with the final document.",
    ),
}

TRIAGE_POSITIVES = {
    "triage.asks_recipient": (
        "Please review the draft.", "Could the recipient send the signed form?",
        "Kindly confirm the totals.", "Please bring the sample to the review.",
        "Could the recipient approve the schedule?", "Please upload the worksheet.",
        "Kindly return the completed checklist.", "Please verify the delivery address.",
        "Could the recipient revise the summary?", "Please share the final notes.",
        'History says "draft pending." Please review it.', "Please file the amended report.",
    ),
    "triage.commits_sender": (
        "I will deliver the draft.", "We will review the totals.",
        "I plan to send the signed form.", "The sender commits to preparing the summary.",
        "I will upload the worksheet.", "We will complete the checklist.",
        "I intend to confirm the schedule.", "The sender will bring the sample.",
        "I will revise the report.", "We promise to share the final notes.",
        'History says "filing is open." I will file the amendment.',
        "The sender commits to booking the room.",
    ),
    "triage.asks_question": (
        "What is the current status?", "Which version is final?", "When does the review begin?",
        "Where is the signed form?", "Who approved the revised total?",
        "How should the item be filed?", "Is the draft ready?", "Did the package arrive?",
        "Why did the schedule change?", "Can the total be confirmed?",
        'History says "status unknown." Has the review finished?',
        "Does the summary include the notes?",
    ),
    "triage.names_time": (
        "The review starts Tuesday.", "Delivery is due tomorrow.",
        "The session begins this afternoon.", "The filing deadline is Friday.",
        "Work starts before the quarterly review.", "The draft is due next week.",
        "The meeting begins in the morning.", "Submission closes at noon.",
        "The update is scheduled for Wednesday.", "Delivery follows the planning session.",
        'History says "schedule pending." The review starts Tuesday.',
        "The deadline falls on the next business day.",
    ),
    "triage.boilerplate": (
        "Regards, the coordination team.", "This message contains a confidentiality notice.",
        "To unsubscribe, update notification preferences.",
        "This footer is intended for the named recipient.",
        "Manage subscriptions through the preference page.",
        "Standard legal notice applies to this message.", "Sincerely, the planning office.",
        "This communication may contain protected material.",
        "Subscription settings control these notices.", "The usual email disclaimer follows.",
        'The quoted footer says "confidential." Standard legal notice follows.',
        "Distribution of this message may be restricted.",
    ),
    "triage.automated_notification": (
        "This is an automated notification.", "A system generated this delivery notice.",
        "This status alert was sent automatically.",
        "An automated service created this reminder.", "This is a system-generated update.",
        "The scheduling service sent this notification.",
        "This automatic alert confirms receipt.", "A workflow service generated this message.",
        "This notification was created without manual action.",
        "The system automatically issued this status note.",
        'History says "manual update." This notification is automatically generated.',
        "An automated process sent this update.",
    ),
}

TRIAGE_HARD_NEGATIVES = (
    '> The earlier message said, "Please review the draft." The current note only acknowledges it.',
    "The sender reports that another person promised to deliver the draft.",
    "Would it be nice if every review were easy?",
    "The schedule is useful, although this note names no time.",
    "The message discusses a footer but is ordinary correspondence.",
    "A person describes an automated service in an ordinary note.",
)

CLOSURE_HARD_NEGATIVES = {
    "closure.fulfilled": 'Quoted history: "I sent the revised brief this morning."',
    "closure.withdrawn": "The request remains active.",
    "closure.deadline_changed": "The original Thursday deadline remains in effect.",
    "closure.modified": "The requested format and recipient remain unchanged.",
}

RULE_PHRASES = {
    "event_tied": (
        "before the launch", "after the review", "by the planning session", "during the audit",
        "ahead of the workshop", "following the briefing", "prior to the meeting",
        "when the conference begins", "once the review ends", "at the kickoff",
        "before the hearing", "after the quarterly session",
    ),
    "soft": (
        "when convenient", "when practical", "at the next opportunity", "sometime soon",
        "when time permits", "as circumstances allow", "at a suitable time", "when feasible",
        "as soon as practical", "at your convenience", "when the schedule allows", "in due course",
    ),
    "unknown": (
        "with the attachment", "using the template", "for the summary", "to the review team",
        "with supporting notes", "in the shared folder", "for the final version", "with approval",
        "in the requested format", "for the project record", "with the source list",
        "to the recipient",
    ),
}


def _write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as stream:
        for row in rows:
            stream.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def _rotating_labels(ids: tuple[str, ...], index: int, width: int) -> set[str]:
    primary = index % len(ids)
    cycle = index // len(ids)
    offsets = [0]
    for step in range(1, width):
        offsets.append(1 + (cycle + step - 1) % (len(ids) - 1))
    return {ids[(primary + offset) % len(ids)] for offset in offsets}


def _closure_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    ids = tuple(CLOSURE_POSITIVES)
    rows = []
    for index in range(count):
        positives = _rotating_labels(ids, index, 2)
        variant = (index // len(ids)) % 12
        paragraph = " ".join(CLOSURE_POSITIVES[key][variant] for key in ids if key in positives)
        negative_id = ids[(index + 2) % len(ids)]
        if negative_id not in positives:
            paragraph += " " + CLOSURE_HARD_NEGATIVES[negative_id]
        project = rng.choice(PROJECTS)
        rows.append(
            {
                "id": f"closure-{index:04d}", "set": "closure",
                "obligation": {
                    "title": f"Send the {project} brief",
                    "evidence_text": f"Please send the {project} brief by Thursday.",
                },
                "later": {
                    "paragraph_text": paragraph, "from_user": bool(index % 2),
                    "days_later": 1 + index % 14,
                },
                "label": {question_id: question_id in positives for question_id in ids},
                "source": "synthetic",
            }
        )
    return rows


def _triage_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    ids = tuple(TRIAGE_POSITIVES)
    rows = []
    for index in range(count):
        positives = _rotating_labels(ids, index, 3)
        variant = (index // len(ids)) % 12
        paragraph = " ".join(TRIAGE_POSITIVES[key][variant] for key in ids if key in positives)
        negative_index = (index + 3) % len(ids)
        if ids[negative_index] not in positives and index % 4 == 0:
            paragraph += " " + TRIAGE_HARD_NEGATIVES[negative_index]
        rows.append(
            {
                "id": f"triage-{index:04d}", "set": "triage",
                "subject": f"{rng.choice(PROJECTS)} coordination",
                "paragraph_text": paragraph, "from_user": bool(index % 2),
                "label": {question_id: question_id in positives for question_id in ids},
                "source": "synthetic",
            }
        )
    return rows


def _balanced(index: int, offset: int) -> bool:
    return (index + offset) % 10 < 5


def _rules_rows(rng: random.Random, count: int) -> list[dict[str, Any]]:
    del rng
    bool_ids = (
        "rules.recap", "rules.scoped_event", "rules.event_match",
        "rules.duplicate_action", "rules.thread_merge",
    )
    rows = []
    kinds = tuple(RULE_PHRASES)
    for index in range(count):
        labels = {
            question_id: _balanced(index, offset * 2)
            for offset, question_id in enumerate(bool_ids)
        }
        variant = (index // 5) % 12
        kind = kinds[index % len(kinds)]
        recap, scoped, event_match, duplicate, thread_merge = (
            labels[question_id] for question_id in bool_ids
        )
        event_name = ("Planning session", "Quarterly review", "Launch meeting")[index % 3]
        base_phrase = RULE_PHRASES[kind][variant]
        phrase = (
            f"{base_phrase} for the {event_name}"
            if event_match
            else f"{base_phrase} for an unrelated event"
        )
        rows.append(
            {
                "id": f"rules-{index:04d}", "set": "rules",
                "sender": "recorder@example.invalid" if recap else "updates@example.invalid",
                "subject": f"{'Automated recap' if recap else 'Project note'} variant {variant}",
                "first_paragraph": (
                    f"Generated meeting summary and transcript format {variant}." if recap
                    else f"An ordinary correspondence update in format {variant}."
                ),
                "request_text": (
                    f"Bring item {variant} to the {event_name}." if scoped
                    else f"File item {variant} in the shared folder."
                ),
                "phrase": phrase,
                "event_name": event_name,
                "action_a": f"Send revised summary {variant}.",
                "action_b": f"Deliver revised summary {variant}." if duplicate
                else f"Book review room {variant}.",
                "subject_a": f"Review topic {variant}",
                "subject_b": f"Re: Review topic {variant}" if thread_merge
                else f"Separate topic {variant}",
                "first_paragraph_a": f"Review the plan version {variant}.",
                "first_paragraph_b": f"Following up on plan version {variant}." if thread_merge
                else f"Discuss a separate budget version {variant}.",
                "label": {**labels, "rules.deadline_kind": kind}, "source": "synthetic",
            }
        )
    return rows


def generate_synthetic(
    output: str | Path | None = None, seed: int = 20260919, count: int = 600
) -> list[Path]:
    if count < 600:
        raise ValueError("each bootstrap set requires at least 600 rows")
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
