"""End-to-end generation through a fake OpenRouter that answers like a real model.

The fake writes plausible rows: pool names, ordinary capitalized phrases such
as "Best Regards" and "Monday Morning", an occasional stray name or outside
address (which must be rejected), and one truncated reply. The generator has
to reach the requested row count from that, offline.
"""

from __future__ import annotations

import json
from pathlib import Path

import httpx
import pytest

from jev_optimize.data import load_jsonl
from jev_optimize.questions import SETS
from jev_optimize.synthetic import (
    SYNTHETIC_COMPANIES,
    SYNTHETIC_PEOPLE,
    SYNTHETIC_SERVICES,
    CorpusThresholds,
    _validate_generated_row,
    check_corpus,
    generate_llm,
)

_OPENERS = (
    "Hi {person},",
    "Dear {person},",
    "Quick note for the {company} matter.",
    "Following up on Monday Morning's call.",
    "Re: the vendor contract",
)
_CLOSERS = (
    "Best Regards, {person}",
    "Thanks, {person}",
    "Kind Regards",
    "Sent from my phone",
    "Follow Up needed by Friday.",
)


def _texts(plan: dict, counter: int) -> tuple[str, str]:
    person = SYNTHETIC_PEOPLE[counter % len(SYNTHETIC_PEOPLE)]
    company = SYNTHETIC_COMPANIES[counter % len(SYNTHETIC_COMPANIES)]
    opener = _OPENERS[counter % len(_OPENERS)].format(person=person, company=company)
    closer = _CLOSERS[counter % len(_CLOSERS)].format(person=person)
    scenario = str(plan.get("scenario", plan.get("scenario_seed", "scenario")))
    body = f"{opener} {scenario} case {counter} for {company}. {closer}"
    if counter % 23 == 0:
        body += " Please contact Jane Doe about it."  # rejected: outside the pool
    if counter % 29 == 0:
        body += " Reach me at someone@outside.example."  # rejected: outside address
    return body, f"{scenario[:30]} {counter}"


def _row(set_name: str, question_id: str | None, plan: dict, counter: int) -> dict:
    body, short = _texts(plan, counter)
    if set_name == "closure":
        return {
            "obligation": {
                "title": f"Send the {short} brief",
                "evidence_text": f"Please send the {short} brief by Thursday.",
            },
            "later": {"paragraph_text": body, "from_user": True, "days_later": 3},
            "label": {},
        }
    if set_name == "triage":
        return {
            "subject": f"Re: {short}",
            "paragraph_text": body,
            "from_user": plan["from_user"],
            "label": plan["label"],
        }
    assert question_id is not None
    value = plan["label"][question_id]
    person = SYNTHETIC_PEOPLE[counter % len(SYNTHETIC_PEOPLE)]
    if question_id == "rules.recap":
        fields = {
            "sender": (
                SYNTHETIC_SERVICES[counter % len(SYNTHETIC_SERVICES)]
                if value else f"{person} <person{counter}@example.invalid>"
            ),
            "subject": f"Meeting recap: {short}" if value else f"Question about notes {short}",
            "first_paragraph": (
                f"Summary for {short}. Decisions: approved. Action items: prepare the brief."
                if value else f"{person} asks about the meeting notes for {short}."
            ),
        }
        return fields | {"label": plan["label"]}
    if question_id == "rules.scoped_event":
        return {
            "request_text": (
                f"Please bring the agenda for {short} to the meeting."
                if value else f"Please archive the meeting notes for {short}."
            ),
            "label": plan["label"],
        }
    if question_id == "rules.event_match":
        return {
            "phrase": f"the planning offsite {short}" if value else f"the sales review {short}",
            "event_name": (
                f"q3 planning offsite {short}" if value else f"q3 planning review {short}"
            ),
            "label": plan["label"],
        }
    if question_id == "rules.duplicate_action":
        return {
            "action_a": f"Prepare the vendor brief for {short}.",
            "action_b": (
                f"Draft a briefing document about the vendor matter {short}."
                if value else f"Approve the vendor brief for {short}."
            ),
            "label": plan["label"],
        }
    if question_id == "rules.thread_merge":
        return {
            "subject_a": f"Vendor review {short}",
            "subject_b": f"Re: vendor review {short}" if value else f"Vendor invoice {short}",
            "first_paragraph_a": f"Please review the vendor terms for {short}.",
            "first_paragraph_b": (
                f"Following up on those terms for {short}."
                if value else f"Please review the vendor invoice for {short}."
            ),
            "label": plan["label"],
        }
    phrases = {
        "event_tied": f"before the board meeting for {short}",
        "soft": f"whenever you get a chance on {short}",
        "unknown": f"after maybe around the {short} point",
    }
    return {"phrase": phrases[value], "label": plan["label"]}


class FakeOpenRouter:
    def __init__(self) -> None:
        self.calls = 0
        self.counter = 0

    def handler(self, request: httpx.Request) -> httpx.Response:
        self.calls += 1
        payload = json.loads(request.content)
        assert payload["provider"] == {"zdr": True}
        assert payload["max_tokens"] >= 8192
        content = payload["messages"][0]["content"]
        plans = json.loads(content.split("Plans:\n", 1)[1])
        set_name = _set_from_fields(content)
        question_id = _question_from_prompt(content)
        rows = []
        for plan in plans:
            self.counter += 1
            rows.append(_row(set_name, question_id, plan, self.counter))
        text = json.dumps({"rows": rows})
        if self.calls == 2:
            text = text[: len(text) // 2]  # one truncated reply, must be retried
        return httpx.Response(200, json={"choices": [{"message": {"content": text}}]})


def _set_from_fields(content: str) -> str:
    if "obligation {title,evidence_text}" in content:
        return "closure"
    if "This batch is only for question triage." in content:
        return "triage"
    return "rules"


def _question_from_prompt(content: str) -> str | None:
    marker = "This batch is only for question "
    if marker not in content:
        return None
    return content.split(marker, 1)[1].split(". Definition:", 1)[0]


@pytest.mark.parametrize("set_name", list(SETS))
def test_generate_llm_reaches_the_row_count_offline(tmp_path: Path, monkeypatch, set_name):
    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "synthetic/model")
    fake = FakeOpenRouter()
    requested = 600 if set_name in {"triage", "rules"} else 128
    with httpx.Client(transport=httpx.MockTransport(fake.handler)) as client:
        paths = generate_llm(
            tmp_path, selected_set=set_name, rows=requested, client=client, parallel=3, smoke=True
        )
    assert paths == [tmp_path / f"{set_name}.jsonl"]
    rows = load_jsonl(paths[0])
    assert len(rows) == requested
    assert all(row.source == "synthetic-llm" for row in rows)
    assert len({row.id for row in rows}) == requested
    if set_name != "closure":
        assert all(len(row.label) == 1 for row in rows)
        for question_id in SETS[set_name]:
            question_rows = [row for row in rows if question_id in row.label]
            assert len(question_rows) == 100
            values = [row.label[question_id] for row in question_rows]
            if question_id == "rules.deadline_kind":
                assert sorted(values.count(option) for option in set(values)) == [33, 33, 34]
            else:
                assert values.count(True) == values.count(False) == 50
    report = check_corpus(
        paths[0], CorpusThresholds(
            min_rows=requested, min_paragraphs=requested // 2, min_obligations=4
        )
    )
    assert report["errors"] == []
    assert fake.calls <= 70


def test_duplicate_action_validator_rejects_true_near_duplicates():
    plan = {
        "focus_question": "rules.duplicate_action",
        "label": {"rules.duplicate_action": True},
    }
    row = {
        "action_a": "Prepare the vendor brief today.",
        "action_b": "Prepare the vendor brief today!",
        "label": {},
    }
    accepted, reason = _validate_generated_row("rules", row, plan)
    assert accepted is None
    assert "near-identical" in reason


@pytest.mark.parametrize(
    ("value", "sender", "accepted"),
    [
        (True, SYNTHETIC_SERVICES[0], True),
        (True, "Priya Venkataraman <priya@example.invalid>", False),
        (False, "Priya Venkataraman <priya@example.invalid>", True),
        (False, SYNTHETIC_SERVICES[0], False),
    ],
)
def test_recap_validator_enforces_sender_pools(value, sender, accepted):
    plan = {"focus_question": "rules.recap", "label": {"rules.recap": value}}
    row = {
        "sender": sender,
        "subject": "Meeting notes",
        "first_paragraph": "Summary text.",
        "label": {},
    }
    result, _reason = _validate_generated_row("rules", row, plan)
    assert (result is not None) is accepted


def test_generate_llm_refuses_small_runs_unless_smoke(tmp_path: Path, monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "synthetic/model")
    with pytest.raises(ValueError):
        generate_llm(tmp_path, selected_set="closure", rows=48)
