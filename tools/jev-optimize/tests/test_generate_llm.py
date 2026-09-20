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
    CorpusThresholds,
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
    scenario = str(plan.get("scenario", "scenario"))
    body = f"{opener} {scenario} case {counter} for {company}. {closer}"
    if counter % 23 == 0:
        body += " Please contact Jane Doe about it."  # rejected: outside the pool
    if counter % 29 == 0:
        body += " Reach me at someone@outside.example."  # rejected: outside address
    return body, f"{scenario[:30]} {counter}"


def _row(set_name: str, plan: dict, counter: int) -> dict:
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
        return {"subject": f"Re: {short}", "paragraph_text": body, "from_user": True, "label": {}}
    return {
        "sender": f"{SYNTHETIC_PEOPLE[counter % 8].split()[0].lower()}@example.invalid",
        "subject": f"Re: {short}",
        "first_paragraph": body,
        "request_text": f"Please prepare the {short} item.",
        "phrase": f"before the {short} review",
        "event_name": f"{short} review",
        "action_a": f"Prepare the {short} item.",
        "action_b": f"Complete the {short} action.",
        "subject_a": f"{short} topic",
        "subject_b": f"{short} follow-up",
        "first_paragraph_a": f"A discussion for {short}.",
        "first_paragraph_b": f"Another discussion for {short}.",
        "label": {},
    }


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
        rows = []
        for plan in plans:
            self.counter += 1
            rows.append(_row(set_name, plan, self.counter))
        text = json.dumps({"rows": rows})
        if self.calls == 2:
            text = text[: len(text) // 2]  # one truncated reply, must be retried
        return httpx.Response(200, json={"choices": [{"message": {"content": text}}]})


def _set_from_fields(content: str) -> str:
    if "obligation {title,evidence_text}" in content:
        return "closure"
    if "subject; paragraph_text; from_user" in content:
        return "triage"
    return "rules"


@pytest.mark.parametrize("set_name", list(SETS))
def test_generate_llm_reaches_the_row_count_offline(tmp_path: Path, monkeypatch, set_name):
    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "synthetic/model")
    fake = FakeOpenRouter()
    # 128 rows: the plan encodes labels in the bits of index // 2, and the
    # triage set has six of them, so smaller runs cannot balance every question.
    with httpx.Client(transport=httpx.MockTransport(fake.handler)) as client:
        paths = generate_llm(
            tmp_path, selected_set=set_name, rows=128, client=client, parallel=3, smoke=True
        )
    assert paths == [tmp_path / f"{set_name}.jsonl"]
    rows = load_jsonl(paths[0])
    assert len(rows) == 128
    assert all(row.source == "synthetic-llm" for row in rows)
    assert len({row.id for row in rows}) == 128
    report = check_corpus(
        paths[0], CorpusThresholds(min_rows=128, min_paragraphs=64, min_obligations=4)
    )
    assert report["errors"] == []
    # 11 batches cover 128 rows; the rejected rows and the truncated reply
    # cost a few more.
    assert fake.calls <= 24


def test_generate_llm_refuses_small_runs_unless_smoke(tmp_path: Path, monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "synthetic-test-placeholder")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "synthetic/model")
    with pytest.raises(ValueError):
        generate_llm(tmp_path, selected_set="closure", rows=48)
