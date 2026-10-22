import json

import httpx
import pytest

from jev_optimize.data import DatasetRow, split_name, split_rows, write_jsonl
from jev_optimize.questions import SETS, SPECS
from jev_optimize.synthetic import (
    SYNTHETIC_PEOPLE,
    check_corpus,
    generate_llm,
    generate_stub,
)
from jev_optimize.threads import (
    OUTCOMES,
    ThreadPlan,
    derive_rows,
    generate_threads_llm,
    plan_threads,
    stub_threads,
    thread_record,
)
from jev_optimize.walk_metric import (
    date_state,
    evaluate_walk,
    expected_status,
    walk_state,
    walk_status,
)

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
    "walk.handoff_to",
)
EXPIRY = ("expiry.event", "expiry.ends_with_event")


@pytest.fixture(autouse=True)
def _no_dspy_cache(monkeypatch):
    # Fake clients record the requests they receive; a cached answer from an
    # earlier run would skip them.
    monkeypatch.setattr("dspy.cache.get", lambda _request: None)
    monkeypatch.setattr("dspy.cache.put", lambda _request, _response: None)


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
    for qid in WALK[:-2] + ("expiry.ends_with_event",):
        assert SPECS[qid].kind == "noul"
    assert SPECS["walk.new_date"].kind == "choice"
    assert SPECS["walk.handoff_to"].kind == "choice"
    assert SPECS["expiry.event"].kind == "choice"
    assert "later.text" in SPECS["walk.responds"].allowed_fields
    assert "later.paragraph_text" not in SPECS["walk.responds"].allowed_fields
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
    assert SPECS["walk.handoff_to"].options == {
        "none": "The text names no listed person."
    }
    assert {
        question_id: SPECS[question_id].signature.instructions
        for question_id in WALK[:-2]
    } == {
        "walk.responds": "`later.text` responds to the request in `ask`.",
        "walk.wp_received": (
            "In `later.text`, `waiting_party` says they received what `ask` requests."
        ),
        "walk.wp_no_longer_needed": (
            "In `later.text`, `waiting_party` says they no longer need what `ask` requests."
        ),
        "walk.wp_asks_again": (
            "In `later.text`, `waiting_party` asks again for what `ask` requests "
            "or asks for an update on it."
        ),
        "walk.wp_changes_terms": (
            "In `later.text`, `waiting_party` changes what `ask` requests."
        ),
        "walk.wp_still_owed": (
            "After `later.text`, `user.name` still owes `waiting_party` an action for `ask`."
        ),
        "walk.wp_changes_due": (
            "In `later.text`, `waiting_party` changes when `ask` must be done."
        ),
        "walk.user_delivers": (
            "In `later.text`, `user.name` gives `waiting_party` what `ask` requests."
        ),
        "walk.user_promises": (
            "In `later.text`, `user.name` says they will do what `ask` requests "
            "at a later time."
        ),
        "walk.user_declines": (
            "In `later.text`, `user.name` declines to do what `ask` requests."
        ),
        "walk.user_hands_off": (
            "In `later.text`, `user.name` says another person will do what `ask` requests."
        ),
        "walk.third_party_done": (
            "In `later.text`, the writer says they did what `ask` requests or will do it."
        ),
    }
    assert SPECS["walk.new_date"].signature.instructions == (
        "Which date does `later.text` set as the new time for `ask`?"
    )
    assert SPECS["walk.handoff_to"].signature.instructions == (
        "Which person does `later.text` name as the one who will now do what `ask` requests?"
    )


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
    full = [
        row
        for row in walk
        if row["unit"] == "paragraph" and "walk.user_delivers" in row["label"]
    ]
    assert len(gate_only) == 1 and gate_only[0]["label"]["walk.responds"] is False
    assert len(full) == 1 and full[0]["label"]["walk.user_delivers"] is True
    assert full[0]["label"]["walk.user_declines"] is False
    assert full[0]["later"]["author_role"] == "user"
    assert all(row["unit"] in {"paragraph", "message"} for row in walk)
    message_row = next(row for row in walk if row["unit"] == "message")
    assert message_row["later"]["text"] == "\n\n".join(
        thread_text for thread_text in (
            row["later"]["text"] for row in walk if row["unit"] == "paragraph"
        )
    )
    assert message_row["label"] == full[0]["label"]


def test_message_units_exist_only_for_multparagraph_messages_and_join_exactly():
    plans = [
        ThreadPlan(
            "one-paragraph",
            "none",
            False,
            None,
            [_m("third_party", "third_party_done")],
        ),
        ThreadPlan(
            "many-paragraphs",
            "none",
            False,
            None,
            [_m("user", "user_declines", paragraphs=3, responding=1)],
        ),
    ]
    rows = [row for row in derive_rows(stub_threads(plans)) if row["set"] == "walk"]
    message_rows = [row for row in rows if row["unit"] == "message"]
    assert [row["group"] for row in message_rows] == ["many-paragraphs"]
    paragraph_rows = [row for row in rows if row["group"] == "many-paragraphs"]
    expected_text = "\n\n".join(
        row["later"]["text"] for row in paragraph_rows if row["unit"] == "paragraph"
    )
    assert message_rows[0]["later"]["text"] == expected_text
    assert message_rows[0]["label"]["walk.responds"] is True
    assert message_rows[0]["label"]["walk.user_declines"] is True
    assert message_rows[0]["label"] == next(
        row["label"]
        for row in paragraph_rows
        if row["unit"] == "paragraph" and row["label"]["walk.responds"]
    )


def test_later_change_separator_matches_the_app_bytes():
    plan = ThreadPlan(
        "amendment-bytes",
        "none",
        False,
        None,
        [
            _m("waiting_party", "wp_changes_terms_owed", paragraphs=2, responding=1),
            _m("waiting_party", "wp_asks_again"),
        ],
    )
    rows = [row for row in derive_rows(stub_threads([plan])) if row["set"] == "walk"]
    first_message_text = next(
        row["later"]["text"]
        for row in rows
        if row["group"] == "amendment-bytes"
        and row["unit"] == "paragraph"
        and row["label"].get("walk.wp_changes_terms")
    )
    second_ask = next(
        row["ask"]
        for row in rows
        if row["group"] == "amendment-bytes"
        and row["ask"] != "Could you send the vendor summary 0 before the planning session?"
    )
    assert second_ask.endswith(f"\n\nLater change: {first_message_text}")


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


def _m(role, outcome, paragraphs=1, responding=0, delegate=None):
    from jev_optimize.threads import MessagePlan

    return MessagePlan(
        author_role=role,
        outcome=outcome,
        paragraphs=paragraphs,
        responding=responding,
        delegate=delegate,
    )


def test_delegate_plans_are_balanced_and_records_use_synthetic_addresses():
    plans = plan_threads(240, seed=17)
    handoff_messages = [
        message
        for plan in plans
        for message in plan.later
        if message.outcome == "user_hands_off"
    ]
    counts = {
        mark: sum(message.delegate == mark for message in handoff_messages)
        for mark in ("internal", "external", "absent")
    }
    assert all(count > 0 for count in counts.values())
    assert max(counts.values()) - min(counts.values()) <= 1

    for thread in stub_threads(plans):
        record = thread_record(thread)
        user_domain = record["user_addresses"][0].rsplit("@", 1)[-1]
        assert user_domain.endswith(".example.invalid")
        assert record["waiting_party_address"].endswith("@copperlantern.example.invalid")
        for index, message in enumerate(thread.plan.later):
            if message.outcome != "user_hands_off":
                assert thread.delegates[index] is None
                continue
            later = record["later"][index]
            truth = later["delegate"]
            assert truth["name"] in SYNTHETIC_PEOPLE
            assert truth["name"] not in {thread.user_display_name, thread.waiting_party}
            assert truth["mark"] in {"internal", "external", "none"}
            assert truth["name"] in thread.later[index][message.responding]
            for participant in later["participants"]:
                assert set(participant) == {"name", "address"}
                assert participant["name"] in SYNTHETIC_PEOPLE
                assert participant["address"].endswith(".example.invalid")
            delegate_participants = [
                person
                for person in later["participants"]
                if person["name"] == truth["name"]
            ]
            if truth["mark"] == "none":
                assert not delegate_participants
            else:
                assert len(delegate_participants) == 1
                delegate_domain = delegate_participants[0]["address"].rsplit("@", 1)[-1]
                assert (delegate_domain == user_domain) == (truth["mark"] == "internal")


def test_older_thread_plans_load_missing_delegate_as_none():
    from jev_optimize.walk_metric import _plan_from_record

    plan = _plan_from_record(
        {
            "plan": {
                "thread_id": "old-thread",
                "hard_case": "none",
                "has_event": False,
                "ends_with_event": None,
                "later": [
                    {
                        "author_role": "user",
                        "outcome": "user_hands_off",
                        "paragraphs": 1,
                        "responding": 0,
                    }
                ],
            }
        }
    )
    assert plan.later[0].delegate is None


def test_walk_state_and_date_state_serialize_to_exact_wire_bytes():
    state = walk_state(
        "Priya",
        "Tomas Lindqvist",
        "Send the summary.",
        "Friday",
        "user",
        "Attached is the summary.",
    )
    assert json.dumps(state, separators=(",", ":")) == (
        '{"user":{"name":"Priya"},"waiting_party":"Tomas Lindqvist",'
        '"ask":"Send the summary.","due":"Friday",'
        '"later":{"author_role":"user","text":"Attached is the summary."}}'
    )
    assert json.dumps(date_state("Send the summary.", "Attached."), separators=(",", ":")) == (
        '{"ask":"Send the summary.","later":{"text":"Attached."}}'
    )


def test_optimize_inputs_exclude_group_and_unit():
    from jev_optimize.optimize import _inputs

    row = DatasetRow.model_validate(
        {
            "id": "walk-input",
            "set": "walk",
            "source": "synthetic-stub",
            "label": {"walk.responds": True},
            "group": "thread-input",
            "unit": "message",
            "user": {"name": "Priya"},
            "waiting_party": "Tomas Lindqvist",
            "ask": "Send the summary.",
            "due": "Friday",
            "later": {"author_role": "user", "text": "Sent."},
        }
    )
    inputs = _inputs(row)
    assert "group" not in inputs
    assert "unit" not in inputs
    assert inputs["later"] == {"author_role": "user", "text": "Sent."}


def test_dynamic_handoff_question_stays_at_its_baseline(tmp_path):
    from jev_optimize.optimize import optimize_set

    path = tmp_path / "walk.jsonl"
    write_jsonl(
        path,
        [
            {
                "id": "walk-dynamic",
                "set": "walk",
                "source": "synthetic-llm",
                "label": {"walk.responds": True},
                "group": "walk-dynamic-thread",
                "unit": "paragraph",
                "user": {"name": "Priya"},
                "waiting_party": "Tomas Lindqvist",
                "ask": "Send the summary.",
                "due": "Friday",
                "later": {"author_role": "user", "text": "Sent."},
            }
        ],
    )
    result = optimize_set(
        "walk", object(), path, only=("walk.handoff_to",)
    )
    assert result["questions"]["walk.handoff_to"] == {
        "kept_baseline": True,
        "baseline_reason": "dynamic options are issued per request",
    }


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
    assert report["unit_counts"] == {
        unit: sum(row["unit"] == unit for row in rows)
        for unit in ("paragraph", "message")
    }


def test_generate_llm_refuses_thread_derived_sets(tmp_path):
    with pytest.raises(ValueError, match="derived from threads"):
        generate_llm(tmp_path, selected_set="walk", rows=600)


def test_generate_stub_refuses_thread_derived_sets(tmp_path):
    with pytest.raises(ValueError, match="derived from threads"):
        generate_stub(tmp_path, selected_set="expiry", rows=60)


class FakeOpenRouter:
    def __init__(
        self,
        *,
        malformed_call=2,
        corrupt_call=3,
        omit_handoff_once=False,
        repeat_key_once=False,
    ):
        self.calls = 0
        self.malformed_call = malformed_call
        self.corrupt_call = corrupt_call
        self.omit_handoff_once = omit_handoff_once
        self.repeat_key_once = repeat_key_once
        self.omitted_handoffs = set()
        self.omitted_names = {}
        self.omitted_paragraphs = {}
        self.repeated_key_threads = set()

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.calls += 1
        body = json.loads(request.content)
        assert body["provider"] == {"zdr": True}
        plans = json.loads(body["messages"][-1]["content"].split("Plans:\n", 1)[1])
        if self.calls == self.malformed_call:
            return httpx.Response(
                200, json={"choices": [{"message": {"content": "{not json"}}]}
            )
        threads = []
        for index, plan in enumerate(plans):
            later = []
            for message in plan["later"]:
                filler_topics = (
                    "The break room has a new coffee machine.",
                    "A garden path is being repaved this week.",
                    "The parking garage now closes at six.",
                )
                filler_count = message["filler_count"]
                fillers = [
                    filler_topics[paragraph % len(filler_topics)]
                    for paragraph in range(filler_count)
                ]
                response = (
                    None
                    if message["outcome"] == "unrelated"
                    else f"I will send the vendor summary for {plan['thread_id']} next week."
                )
                delegate_name = message.get("delegate_name")
                if message["outcome"] == "user_hands_off":
                    omit_name = (
                        self.omit_handoff_once
                        and plan["thread_id"] not in self.omitted_handoffs
                    )
                    if omit_name:
                        self.omitted_handoffs.add(plan["thread_id"])
                        self.omitted_names[plan["thread_id"]] = delegate_name
                        self.omitted_paragraphs[plan["thread_id"]] = response
                        response = f"I will send the vendor summary for {plan['thread_id']}."
                    else:
                        response = f"{delegate_name} will send the vendor summary."
                if self.repeat_key_once and not self.repeated_key_threads and fillers:
                    self.repeated_key_threads.add(plan["thread_id"])
                    fillers[0] = "The vendor kitchen has a new coffee machine."
                later.append(
                    {"response": response, "fillers": fillers, "delegate_name": delegate_name}
                )
            ask = f"Could you send the vendor summary for {plan['thread_id']} by Friday?"
            if self.calls == self.corrupt_call and index == 0:
                ask += " See https://example.com/x"
            if self.calls == self.corrupt_call and index == 1 and later:
                later[0]["fillers"].append("An extra paragraph.")
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
    assert sum(out["rejected_reasons"].values()) == out["rejected"]


def test_llm_handoff_paragraph_without_delegate_is_rejected_and_regenerated(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "fake/model")
    fake = FakeOpenRouter(
        malformed_call=None,
        corrupt_call=None,
        omit_handoff_once=True,
    )
    client = httpx.Client(transport=httpx.MockTransport(fake))
    result = generate_threads_llm(
        tmp_path,
        threads=24,
        seed=3,
        client=client,
        parallel=1,
        smoke=True,
        batch_size=24,
    )
    assert fake.omitted_handoffs
    assert all(
        name not in fake.omitted_paragraphs[thread_id]
        for thread_id, name in fake.omitted_names.items()
    )
    assert fake.calls >= 2
    assert result["rejected"] >= 1
    records = [
        json.loads(line)
        for line in (tmp_path / "threads.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    regenerated = [
        record
        for record in records
        if record["thread_id"] in fake.omitted_handoffs
    ]
    assert regenerated
    for record in regenerated:
        assert fake.omitted_names[record["thread_id"]] in json.dumps(record["later"])
        plan = record["plan"]["later"]
        for index, message in enumerate(plan):
            if message["outcome"] == "user_hands_off":
                later_message = record["later"][index]
                assert later_message["delegate"]["name"] in later_message["paragraphs"][
                    message["responding"]
                ]


def test_llm_thread_assembly_places_response_at_the_planned_index():
    from jev_optimize.threads import ThreadPlan, _validated_thread, stub_threads

    plan = ThreadPlan(
        "assembly-thread",
        "none",
        False,
        None,
        [
            _m("user", "user_delivers", paragraphs=2, responding=0),
            _m("user", "user_promises", paragraphs=3, responding=2),
            _m("third_party", "unrelated", paragraphs=2, responding=1),
        ],
    )
    assigned = stub_threads([plan])[0]
    fillers = [
        "The break room has a new coffee machine.",
        "A garden path is being repaved this week.",
        "The parking garage now closes at six.",
    ]
    candidate = {
        "thread_id": plan.thread_id,
        "ask": "Could you send the vendor summary before Friday?",
        "due": "Friday",
        "event": None,
        "later": [
            {
                "response": "Attached is the vendor summary.",
                "fillers": fillers[:1],
                "delegate_name": None,
            },
            {
                "response": "I will send the vendor summary next week.",
                "fillers": fillers[:2],
                "delegate_name": None,
            },
            {"response": None, "fillers": fillers[:2], "delegate_name": None},
        ],
    }

    thread, reason = _validated_thread(candidate, plan, assigned)

    assert reason == "ok"
    assert thread is not None
    assert thread.later == [
        ["Attached is the vendor summary.", fillers[0]],
        [fillers[0], fillers[1], "I will send the vendor summary next week."],
        fillers[:2],
    ]


def test_llm_thread_schema_uses_strict_response_and_filler_fields():
    from jev_optimize.threads import _thread_schema

    schema = _thread_schema()["json_schema"]
    later = schema["schema"]["properties"]["threads"]["items"]["properties"]["later"]
    later_item = later["items"]
    assert schema["strict"] is True
    assert later_item["additionalProperties"] is False
    assert set(later_item["properties"]) == {"response", "fillers", "delegate_name"}
    assert set(later_item["required"]) == {"response", "fillers", "delegate_name"}


def test_llm_threads_reject_missing_response_and_wrong_filler_count():
    from jev_optimize.threads import ThreadPlan, _validated_thread, stub_threads

    plan = ThreadPlan(
        "validation-thread",
        "none",
        False,
        None,
        [_m("user", "user_delivers", paragraphs=2, responding=1)],
    )
    assigned = stub_threads([plan])[0]
    candidate = {
        "thread_id": plan.thread_id,
        "ask": "Please send the vendor summary by Friday.",
        "due": "Friday",
        "event": None,
        "later": [
            {
                "response": None,
                "fillers": ["The break room has a new coffee machine."],
                "delegate_name": None,
            }
        ],
    }
    thread, reason = _validated_thread(candidate, plan, assigned)
    assert thread is None
    assert reason == "response is missing or malformed"

    candidate["later"][0]["response"] = "Attached is the vendor summary."
    candidate["later"][0]["fillers"].append("A garden path is being repaved this week.")
    thread, reason = _validated_thread(candidate, plan, assigned)
    assert thread is None
    assert reason == "filler count or text mismatch"


def test_repeated_filler_key_term_is_rejected_then_regenerated(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "fake/model")
    fake = FakeOpenRouter(
        malformed_call=None,
        corrupt_call=None,
        repeat_key_once=True,
    )
    client = httpx.Client(transport=httpx.MockTransport(fake))
    result = generate_threads_llm(
        tmp_path,
        threads=24,
        seed=3,
        client=client,
        parallel=1,
        smoke=True,
        batch_size=24,
    )

    reason = "filler shares ask or event key term"
    assert fake.repeated_key_threads
    assert fake.calls >= 2
    assert result["rejected_reasons"][reason] >= 1
    records = [
        json.loads(line)
        for line in (tmp_path / "threads.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    regenerated = [record for record in records if record["thread_id"] in fake.repeated_key_threads]
    assert regenerated
    for record in regenerated:
        for planned, later in zip(record["plan"]["later"], record["later"], strict=True):
            paragraphs = later["paragraphs"]
            if planned["outcome"] == "unrelated":
                fillers = paragraphs
            else:
                fillers = [
                    paragraph
                    for index, paragraph in enumerate(paragraphs)
                    if index != planned["responding"]
                ]
            assert all("vendor" not in filler.casefold() for filler in fillers)


def test_filler_key_term_filter_ignores_stop_words_and_pool_names():
    from jev_optimize.threads import ThreadPlan, _validated_thread, stub_threads

    plan = ThreadPlan(
        "allowed-filler-thread",
        "none",
        False,
        None,
        [_m("user", "user_delivers", paragraphs=2)],
    )
    assigned = stub_threads([plan])[0]
    candidate = {
        "thread_id": plan.thread_id,
        "ask": "Could you send notes about Tomas Lindqvist before Friday?",
        "due": "Friday",
        "event": None,
        "later": [
            {
                "response": "Attached are the notes.",
                "fillers": ["Tomas Lindqvist waits before lunch."],
                "delegate_name": None,
            }
        ],
    }

    thread, reason = _validated_thread(candidate, plan, assigned)

    assert reason == "ok"
    assert thread is not None


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
        "walk.handoff_to",
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


def test_whole_message_unit_can_respond_when_no_paragraph_does():
    paragraphs = [
        _answers("waiting_party", 0.05),
        _answers("waiting_party", 0.1),
        _answers("waiting_party", 0.95, wp_received=0.95),
    ]
    assert walk_status([("waiting_party", paragraphs)], THRESHOLDS) == "closed:completed"


def test_disagreeing_paragraph_and_message_terminals_escalate():
    paragraphs = [
        _answers("waiting_party", 0.95, wp_received=0.95),
        _answers("waiting_party", 0.05),
        _answers("waiting_party", 0.95, wp_no_longer_needed=0.95),
    ]
    assert walk_status([("waiting_party", paragraphs)], THRESHOLDS) == "escalated"


def test_gray_message_gate_without_a_responding_unit_escalates():
    paragraphs = [
        _answers("user", 0.05),
        _answers("user", 0.05),
        _answers("user", 0.5),
    ]
    assert walk_status([("user", paragraphs)], THRESHOLDS) == "escalated"


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


def _test_thread_id(prefix):
    index = 0
    while True:
        thread_id = f"{prefix}-{index}"
        if split_name(thread_id, small=True) == "test":
            return thread_id
        index += 1


def test_evaluate_walk_reports_handoff_mark_and_person_accuracy(tmp_path):
    from jev_optimize.threads import MessagePlan

    class HandoffClient:
        def decide(self, state, questions):
            question_id, question = next(iter(questions.items()))
            text = state["later"]["text"].casefold()
            if question["type"] == "noul":
                marker = {
                    "walk.responds": "mk-handoff.",
                    "walk.user_hands_off": "mk-handoff.",
                }.get(question_id)
                probability = 0.99 if marker and marker in text else 0.01
                return {question_id: {"type": "noul", "noul": probability}}
            choice = next(
                (
                    handle
                    for handle, name in question["criteria"].items()
                    if handle != "none" and name.casefold() in text
                ),
                "none",
            )
            return {
                question_id: {
                    "type": "choice",
                    "choice": choice,
                    "probabilities": {choice: 1.0},
                    "confidence": 1.0,
                }
            }

    marks = ("internal", "external", "absent")
    plans = [
        ThreadPlan(
            _test_thread_id(f"delegate-{mark}"),
            "none",
            False,
            None,
            [MessagePlan("user", "user_hands_off", delegate=mark)],
        )
        for mark in marks
    ]
    path = tmp_path / "threads.jsonl"
    write_jsonl(path, map(thread_record, stub_threads(plans)))
    report = evaluate_walk(path, HandoffClient(), thresholds=THRESHOLDS)
    assert report["handoff"] == {
        "total": 3,
        "mark_correct": 3,
        "person_correct": 3,
        "mark_accuracy": 1.0,
        "person_accuracy": 1.0,
    }


def test_whole_message_date_follow_up_uses_joined_message_text(tmp_path):
    from jev_optimize.threads import MessagePlan

    class WholeMessageDateClient:
        def __init__(self):
            self.date_states = []

        def decide(self, state, questions):
            question_id, question = next(iter(questions.items()))
            if question_id == "walk.new_date":
                self.date_states.append(state)
                return {
                    question_id: {
                        "type": "choice",
                        "choice": "none",
                        "probabilities": {"none": 1.0},
                        "confidence": 1.0,
                    }
                }
            text = state["later"]["text"]
            if question["type"] == "noul":
                positive = "\n\n" in text and question_id in {
                    "walk.responds",
                    "walk.user_promises",
                }
                probability = 0.99 if positive else 0.01
                return {question_id: {"type": "noul", "noul": probability}}
            raise AssertionError("handoff choice should not be requested")

    plan = ThreadPlan(
        _test_thread_id("whole-date"),
        "none",
        False,
        None,
        [MessagePlan("user", "user_promises", paragraphs=2, responding=1)],
    )
    thread = stub_threads([plan])[0]
    path = tmp_path / "threads.jsonl"
    write_jsonl(path, [thread_record(thread)])
    client = WholeMessageDateClient()
    evaluate_walk(path, client, thresholds=THRESHOLDS)
    expected = "\n\n".join(thread.later[0])
    assert client.date_states == [date_state(thread.ask, expected)]


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


def test_scrub_does_not_join_names_across_fields():
    from jev_optimize.synthetic import scrub_row

    row = {"user": {"name": "Priya"}, "waiting_party": "Mateo Villanueva"}
    assert scrub_row(row) == []
    assert scrub_row({"later": {"text": "Please ask Quentin Marlowe."}}) == [
        "capitalized name outside the synthetic pool"
    ]


def test_generate_threads_smoke_is_small_and_never_writes_the_committed_corpus(monkeypatch):
    import sys
    from pathlib import Path

    import jev_optimize.threads as threads_module
    from jev_optimize import cli

    calls = []

    def fake_generate(output, *, threads, seed, smoke):
        calls.append((Path(output), threads, smoke))
        return {"threads": threads}

    monkeypatch.setattr(threads_module, "generate_threads_llm", fake_generate)
    monkeypatch.setattr(sys, "argv", ["jev-optimize", "generate-threads", "--llm", "--smoke"])
    cli.main()
    monkeypatch.setattr(sys, "argv", ["jev-optimize", "generate-threads", "--llm"])
    cli.main()

    assert calls[0] == (Path("runs") / "smoke-threads", 24, True)
    assert calls[1] == (cli.DEFAULT_ROOT, 600, False)


def test_a_refused_thread_request_stops_the_run_and_names_the_status(tmp_path, monkeypatch):
    from jev_optimize.threads import ThreadRequestError

    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "fake/model")
    calls = []

    def refuse(request):
        calls.append(request)
        return httpx.Response(
            404, json={"error": {"message": "No endpoints found matching your data policy"}}
        )

    client = httpx.Client(transport=httpx.MockTransport(refuse))
    with pytest.raises(ThreadRequestError, match="HTTP 404.*data policy"):
        generate_threads_llm(
            tmp_path, threads=24, seed=3, client=client, parallel=1, smoke=True, batch_size=8
        )
    assert len(calls) == 1
    assert not (tmp_path / "threads.jsonl").exists()


def test_plan_payload_states_what_to_write_not_where_it_goes():
    from jev_optimize.threads import _plan_payload

    plan = ThreadPlan(
        "payload-thread",
        "none",
        False,
        None,
        [
            _m("waiting_party", "wp_received", paragraphs=3, responding=1),
            _m("user", "unrelated", 2),
        ],
    )
    thread = stub_threads([plan])[0]
    later = _plan_payload(plan, thread)["later"]
    assert later[0] == {
        "author_role": "waiting_party",
        "outcome": "wp_received",
        "response_required": True,
        "filler_count": 2,
        "delegate_name": None,
    }
    assert later[1]["response_required"] is False
    assert later[1]["filler_count"] == 2
    assert all("paragraphs" not in item and "responding" not in item for item in later)


def test_pay_by_date_plans_mention_an_event_that_does_not_end_the_ask():
    plans = plan_threads(600)
    pay = [plan for plan in plans if plan.hard_case == "pay_by_date"]
    assert pay and all(plan.has_event and plan.ends_with_event is False for plan in pay)
    labels = [plan.ends_with_event for plan in plans if plan.has_event]
    assert labels.count(False) >= 80
    assert labels.count(True) >= 80
