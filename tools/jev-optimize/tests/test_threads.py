import json

import httpx
import pytest

from jev_optimize.data import DatasetRow, split_rows, write_jsonl
from jev_optimize.questions import SETS, SPECS
from jev_optimize.synthetic import check_corpus, generate_llm, generate_stub
from jev_optimize.threads import (
    OUTCOMES,
    ThreadPlan,
    derive_rows,
    generate_threads_llm,
    plan_threads,
    stub_threads,
)
from jev_optimize.walk_metric import expected_status, walk_status

WALK = (
    "walk.responds",
    "walk.wp_received",
    "walk.wp_no_longer_needed",
    "walk.wp_asks_again",
    "walk.wp_changes_terms",
    "walk.wp_still_owed",
    "walk.wp_changes_due",
    "walk.user_delivers",
    "walk.user_promises",
    "walk.user_declines",
    "walk.user_hands_off",
    "walk.third_party_done",
    "walk.new_date",
)
EXPIRY = ("expiry.event", "expiry.ends_with_event")


def _row(row_id: str, group: str) -> DatasetRow:
    return DatasetRow.model_validate(
        {
            "id": row_id,
            "set": "walk",
            "source": "synthetic-stub",
            "label": {"walk.responds": True},
            "group": group,
        }
    )


def test_rows_sharing_a_group_land_in_one_split():
    rows = [_row(f"walk-{i}", f"thread-{i // 4}") for i in range(400)]
    splits = split_rows(rows, question_id="walk.responds")
    where = {}
    for name, members in splits.items():
        for row in members:
            where.setdefault(row.group, set()).add(name)
    assert all(len(names) == 1 for names in where.values())


def test_walk_and_expiry_sets_are_registered():
    assert SETS["walk"] == WALK
    assert SETS["expiry"] == EXPIRY
    for qid in WALK[:-1] + ("expiry.ends_with_event",):
        assert SPECS[qid].kind == "noul"
    assert SPECS["walk.new_date"].kind == "choice"
    assert SPECS["expiry.event"].kind == "choice"
    assert set(SPECS["walk.responds"].signature.input_fields) == {
        "user",
        "waiting_party",
        "ask",
        "due",
        "later",
    }
    assert set(SPECS["expiry.ends_with_event"].signature.input_fields) == {
        "ask",
        "waiting_party",
        "event",
    }


def test_plans_cover_every_outcome_and_hard_case():
    plans = plan_threads(120, seed=7)
    seen = {message.outcome for plan in plans for message in plan.later}
    assert seen >= set(OUTCOMES)
    kinds = {plan.hard_case for plan in plans}
    assert {
        "rsvp_before_event",
        "pay_by_date",
        "prepare_for_meeting",
        "correction_amends",
        "correction_ends",
        "chaser_after_delivery",
        "unrelated_later_message",
    } <= kinds


def test_walk_stops_after_a_terminal_outcome():
    plan = ThreadPlan(
        thread_id="t-1",
        hard_case="none",
        has_event=False,
        ends_with_event=None,
        later=[
            _m("waiting_party", "wp_received"),
            _m("waiting_party", "wp_asks_again"),
        ],
    )
    rows = derive_rows(stub_threads([plan]))
    walk = [row for row in rows if row["set"] == "walk"]
    assert {row["group"] for row in walk} == {"t-1"}
    assert not any(row["label"].get("walk.wp_asks_again") for row in walk)


def test_chaser_after_delivery_remains_in_the_derived_walk():
    plan = ThreadPlan(
        thread_id="t-chaser",
        hard_case="chaser_after_delivery",
        has_event=False,
        ends_with_event=None,
        later=[_m("user", "user_delivers"), _m("waiting_party", "wp_asks_again")],
    )
    rows = [row for row in derive_rows(stub_threads([plan])) if row["set"] == "walk"]
    assert any(row["label"].get("walk.user_delivers") for row in rows)
    assert any(row["label"].get("walk.wp_asks_again") for row in rows)


def test_responding_paragraph_carries_role_labels_and_others_only_the_gate():
    plan = ThreadPlan(
        thread_id="t-2",
        hard_case="none",
        has_event=False,
        ends_with_event=None,
        later=[_m("user", "user_delivers", paragraphs=2, responding=1)],
    )
    walk = [row for row in derive_rows(stub_threads([plan])) if row["set"] == "walk"]
    gate_only = [row for row in walk if set(row["label"]) == {"walk.responds"}]
    full = [row for row in walk if "walk.user_delivers" in row["label"]]
    assert len(gate_only) == 1 and gate_only[0]["label"]["walk.responds"] is False
    assert len(full) == 1 and full[0]["label"]["walk.user_delivers"] is True
    assert full[0]["label"]["walk.user_declines"] is False
    assert full[0]["later"]["author_role"] == "user"


def test_expiry_rows_only_for_threads_with_an_event():
    with_event = ThreadPlan(
        thread_id="t-3",
        hard_case="rsvp_before_event",
        has_event=True,
        ends_with_event=True,
        later=[],
    )
    without = ThreadPlan(
        thread_id="t-4",
        hard_case="pay_by_date",
        has_event=False,
        ends_with_event=None,
        later=[],
    )
    rows = derive_rows(stub_threads([with_event, without]))
    expiry = [row for row in rows if row["set"] == "expiry"]
    assert [row["group"] for row in expiry] == ["t-3"]
    assert expiry[0]["label"] == {"expiry.ends_with_event": True}


def _m(role, outcome, paragraphs=1, responding=0):
    from jev_optimize.threads import MessagePlan

    return MessagePlan(
        author_role=role,
        outcome=outcome,
        paragraphs=paragraphs,
        responding=responding,
    )


def test_check_corpus_accepts_a_stub_walk_corpus(tmp_path):
    rows = [
        row
        for row in derive_rows(stub_threads(plan_threads(800)))
        if row["set"] == "walk"
    ]
    path = tmp_path / "walk.jsonl"
    write_jsonl(path, rows)
    report = check_corpus(path)
    assert report["set"] == "walk"
    assert report["questions"]["walk.responds"]["rows"] >= 40


def test_generate_llm_refuses_thread_derived_sets(tmp_path):
    with pytest.raises(ValueError, match="derived from threads"):
        generate_llm(tmp_path, selected_set="walk", rows=600)


def test_generate_stub_refuses_thread_derived_sets(tmp_path):
    with pytest.raises(ValueError, match="derived from threads"):
        generate_stub(tmp_path, selected_set="expiry", rows=60)


class FakeOpenRouter:
    def __init__(self):
        self.calls = 0

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.calls += 1
        body = json.loads(request.content)
        assert body["provider"] == {"zdr": True}
        plans = json.loads(body["messages"][-1]["content"].split("Plans:\n", 1)[1])
        if self.calls == 2:
            return httpx.Response(
                200, json={"choices": [{"message": {"content": "{not json"}}]}
            )
        threads = []
        for index, plan in enumerate(plans):
            later = [
                {
                    "paragraphs": [
                        f"Paragraph {paragraph} about the vendor summary for {plan['thread_id']}."
                        for paragraph in range(message["paragraphs"])
                    ]
                }
                for message in plan["later"]
            ]
            ask = f"Could you send the vendor summary for {plan['thread_id']} by Friday?"
            if self.calls == 3 and index == 0:
                ask += " See https://example.com/x"
            if self.calls == 3 and index == 1 and later:
                later[0]["paragraphs"].append("An extra paragraph.")
            threads.append(
                {
                    "thread_id": plan["thread_id"],
                    "ask": ask,
                    "due": "Friday",
                    "event": plan.get("event_hint"),
                    "later": later,
                }
            )
        return httpx.Response(
            200,
            json={
                "choices": [
                    {"message": {"content": json.dumps({"threads": threads})}}
                ]
            },
        )


def test_llm_threads_reject_bad_rows_and_retry_offline(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "fake/model")
    fake = FakeOpenRouter()
    client = httpx.Client(transport=httpx.MockTransport(fake))
    out = generate_threads_llm(
        tmp_path,
        threads=24,
        seed=3,
        client=client,
        parallel=1,
        smoke=True,
        batch_size=8,
    )
    lines = (tmp_path / "threads.jsonl").read_text(encoding="utf-8").splitlines()
    assert len(lines) == 24
    assert fake.calls >= 4
    assert "https://" not in (tmp_path / "walk.jsonl").read_text(encoding="utf-8")
    assert out["rejected"] >= 2


def _answers(role, gate, **branch):
    from jev_optimize.threads import BRANCH_IDS

    probabilities = {"walk.responds": gate}
    probabilities.update(
        {
            question_id: branch.get(question_id.split(".")[1], 0.05)
            for question_id in BRANCH_IDS[role]
        }
    )
    return probabilities


THRESHOLDS = {
    question_id: {"accept": 0.7, "escalate": 0.3}
    for question_id in (
        "walk.responds",
        "walk.wp_received",
        "walk.wp_no_longer_needed",
        "walk.wp_asks_again",
        "walk.wp_changes_terms",
        "walk.wp_still_owed",
        "walk.wp_changes_due",
        "walk.user_delivers",
        "walk.user_promises",
        "walk.user_declines",
        "walk.user_hands_off",
        "walk.third_party_done",
    )
}


def test_walk_status_matches_the_rust_verdict_rules():
    steps = [
        ("user", [_answers("user", 0.95, user_delivers=0.9)]),
        ("waiting_party", [_answers("waiting_party", 0.95, wp_received=0.95)]),
    ]
    assert walk_status(steps, THRESHOLDS) == "closed:completed"
    gray = [("user", [_answers("user", 0.5)])]
    assert walk_status(gray, THRESHOLDS) == "escalated"
    none = [("third_party", [_answers("third_party", 0.05)])]
    assert walk_status(none, THRESHOLDS) == "open"


def test_expected_status_reads_the_plan():
    from jev_optimize.threads import MessagePlan, ThreadPlan

    plan = ThreadPlan(
        "t",
        "none",
        False,
        None,
        [MessagePlan("user", "user_delivers"), MessagePlan("waiting_party", "wp_received")],
    )
    assert expected_status(plan) == "closed:completed"


def test_paid_commands_refuse_stub_rows(tmp_path):
    from jev_optimize.cli import refuse_stub_rows

    stub = tmp_path / "walk.jsonl"
    stub.write_text(json.dumps({"source": "synthetic-stub"}) + "\n", encoding="utf-8")
    with pytest.raises(SystemExit, match="synthetic-stub"):
        refuse_stub_rows(stub, None)
    refuse_stub_rows(stub, "runs/latest.json")
    llm = tmp_path / "llm.jsonl"
    llm.write_text(json.dumps({"source": "synthetic-llm"}) + "\n", encoding="utf-8")
    refuse_stub_rows(llm, None)


def test_walk_status_date_rules_match_the_desktop_walk():
    thresholds = {
        "walk.responds": {"accept": 0.7, "escalate": 0.3},
        "walk.user_delivers": {"accept": 0.7, "escalate": 0.3},
        "walk.user_promises": {"accept": 0.7, "escalate": 0.3},
        "walk.user_declines": {"accept": 0.7, "escalate": 0.3},
        "walk.user_hands_off": {"accept": 0.7, "escalate": 0.3},
        "walk.wp_received": {"accept": 0.9, "escalate": 0.5},
        "walk.wp_no_longer_needed": {"accept": 0.9, "escalate": 0.5},
        "walk.wp_asks_again": {"accept": 0.7, "escalate": 0.3},
        "walk.wp_changes_terms": {"accept": 0.7, "escalate": 0.3},
        "walk.wp_still_owed": {"accept": 0.9, "escalate": 0.5},
        "walk.wp_changes_due": {"accept": 0.7, "escalate": 0.3},
        "walk.new_date": {"accept": 0.7, "escalate": 0.3},
    }

    def promise(date):
        paragraph = {"walk.responds": 0.95, "walk.user_promises": 0.9}
        if date is not None:
            paragraph["walk.new_date"] = date
        return [("user", [paragraph])]

    def wp_due(date):
        paragraph = {"walk.responds": 0.95, "walk.wp_changes_due": 0.9}
        if date is not None:
            paragraph["walk.new_date"] = date
        return [("waiting_party", [paragraph])]

    picked = {"choice": "t0", "confidence": 0.9}
    none = {"choice": "none", "confidence": 0.9}
    assert walk_status(promise(picked), thresholds) == "suggested:deadline_change"
    assert walk_status(promise(None), thresholds) == "open"
    assert walk_status(promise(none), thresholds) == "escalated"
    assert walk_status(wp_due(picked), thresholds) == "open:rescheduled"
    assert walk_status(wp_due(None), thresholds) == "escalated"
    assert walk_status(wp_due(none), thresholds) == "escalated"
