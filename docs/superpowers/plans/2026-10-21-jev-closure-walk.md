# Jev closure walk implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat Jev closure batch with a per-loop walk over later messages, plus a due-versus-expires check, as specified in `docs/superpowers/specs/2026-10-21-jev-closure-walk-design.md`.

**Architecture:** Three parts, each on its own branch. Part A adds a `walk` and an `expiry` question set to the Python harness, with a thread-based synthetic corpus. Part B adds bootstrap registry entries and re-pins the registry hash. Part C adds a pure decision module `closure_walk.rs` to the desktop crate, replaces the body of `scan_decision_closures` with a round-based walk, and adds the new card outcomes.

**Tech stack:** Rust 2024 (desktop crate `openloops-desktop`, feature `native-ui`), Slint adapter code in Rust only (no `.slint` edits), Python 3.11 with uv and dspy 3.4.0 (`tools/jev-optimize`), PowerShell 7 repo checkers.

---

## Ground rules for every task

- Branches: Part A `feat/jev-walk-harness` from `main`. Part B `chore/contract-walk-registry` from `main`. Part C `feat/jev-closure-walk` from Part B's tip. Never commit to `main`.
- The repository is public. Use invented names only (the harness name pool) and `example.invalid` addresses. No real mail text, names, paths or keys.
- No paid call. No command in this plan contacts OpenRouter. Tests use `httpx.MockTransport` or `FixedDecisionClient`.
- `cargo` is not on PATH. In PowerShell run `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"` first.
- Desktop tests need `--features native-ui -- --test-threads=1`.
- Never use a bare `python` command; use `uv run` inside `tools/jev-optimize`.
- Run the gate, read the result, then commit, as separate commands. Before a commit: `git status --short`, `git add <files>`, `pwsh ./tools/check-public-repo.ps1 -Mode Staged`.
- Checker baseline on 2026-10-21 (before this work): these eight checkers already fail and must not be "fixed" here: addin-bridge, automation-evaluation, distribution-registration, evidence-identity, policy-state, protected-state, reminder-adapter, self-email. `check-model-boundary.ps1` passes and must still pass after Part B.
- If a signature in this plan does not match the code, follow the code and keep the behaviour this plan describes.

## Adjustments to the spec

Owner review, 2026-10-21: adjustments 3 to 6 stand. The spec's amendment section replaces adjustments 1 and 2 (gate per paragraph and per message; `walk.handoff_to` restored). Follow-up commits on each branch implement the amendment.

The design was approved on 2026-10-21. Code mapping showed six places where the spec needs a smaller or safer form. The plan implements these:

1. **Gate per paragraph.** The spec's gate was a choice over paragraph indexes plus a noul. Registry instructions are fixed text and cannot name a paragraph index, so each paragraph gets its own request with `walk.responds` and the branch nouls. All paragraphs of one message still go out in the same round, so the number of round trips is unchanged. `walk.responds_paragraph` is not added.
2. **No `walk.handoff_to`.** The Delegated suggestion shows the paragraph that names the new person. A name field would need a new dynamic-option question with no corpus. Accept still moves the card to Watching.
3. **Dynamic-option choices are not tuned yet.** `walk.new_date` and `expiry.event` build their options per request, like `extract.temporal`. The harness cannot yet carry per-row options, so both keep bootstrap wording. The thread corpus still records their true answers for a later harness change.
4. **A promise with no date is an acknowledgement.** When `walk.user_promises` accepts and the paragraph has no date candidate, nothing changes. When the waiting party changes the due date and no date candidate exists, the loop escalates.
5. **Amendments go into `ask`.** The state has no separate `amendments` field. After an amendment, `ask` is the original paragraph followed by `"\n\nLater change: "` and the amending paragraph.
6. **The event guard stays for the toggle-off path.** `deadline_is_event_time` still governs `close_passed_events`, which also runs with the decision model off. With the decision model on, loops that guard keeps open reach the walk, and the walk's expiry step decides them.

## File map

| File | Part | Responsibility |
|---|---|---|
| `tools/jev-optimize/jev_optimize/data.py` | A | Group-aware splits (thread id) |
| `tools/jev-optimize/jev_optimize/questions.py` | A | Walk and expiry signatures and specs; `SETS` names |
| `tools/jev-optimize/jev_optimize/threads.py` (new) | A | Thread model, plans, stub generator, LLM generator, row derivation |
| `tools/jev-optimize/jev_optimize/synthetic.py` | A | `check_corpus` rules for walk and expiry; refuse walk/expiry in `generate_llm` |
| `tools/jev-optimize/jev_optimize/walk_metric.py` (new) | A | Thread-level status and `evaluate-walk` |
| `tools/jev-optimize/jev_optimize/optimize.py` | A | `only` filter for `optimize_set` |
| `tools/jev-optimize/jev_optimize/cli.py` | A | `generate-threads`, `optimize --only` |
| `tools/jev-optimize/tests/test_threads.py` (new) | A | Thread derivation, splits, LLM generator offline |
| `tools/jev-optimize/tests/test_offline_end_to_end.py` | A | Walk subset end to end |
| `tools/jev-optimize/data/synthetic/{threads,walk,expiry}.jsonl` (new) | A | Stub corpora (`synthetic-stub`) |
| `tools/jev-optimize/README.md` | A | Walk section |
| `contracts/model/decision-questions.json` | B | 15 new entries |
| `crates/openloops-inference/src/decision.rs` | B | `REQUIRED_IDS` |
| `tools/check-model-boundary.ps1` | B | Hash literal only |
| `crates/openloops-desktop/src/claim_view.rs` | C | New enum variants; `EventPassed::expired` |
| `crates/openloops-desktop/src/review_model.rs` | C | Accept handling; expired label |
| `crates/openloops-desktop/src/slint_review.rs` | C | Labels, pills, evidence card labels |
| `crates/openloops-desktop/src/closure_walk.rs` (new) | C | Pure walk logic: ids, states, questions, verdicts, expiry |
| `crates/openloops-desktop/src/review_scan.rs` | C | Walk orchestration replaces the decision closure internals |
| `crates/openloops-desktop/src/training_export.rs` | C | `walk.jsonl` export |
| `docs/inbox-review.md`, `docs/openrouter.md` | C | Behaviour and transmitted-field prose |

---

# Part A: harness (`feat/jev-walk-harness`)

All commands run from `tools/jev-optimize`.

### Task A1: group-aware splits

**Files:** Modify `jev_optimize/data.py` (`split_rows`, about line 106). Test: `tests/test_threads.py` (new).

- [ ] **Step 1: Write the failing test**

```python
# tests/test_threads.py
from jev_optimize.data import DatasetRow, split_rows


def _row(row_id: str, group: str) -> DatasetRow:
    return DatasetRow.model_validate(
        {"id": row_id, "set": "walk", "source": "synthetic-stub",
         "label": {"walk.responds": True}, "group": group}
    )


def test_rows_sharing_a_group_land_in_one_split():
    rows = [_row(f"walk-{i}", f"thread-{i // 4}") for i in range(400)]
    splits = split_rows(rows, question_id="walk.responds")
    where = {}
    for name, members in splits.items():
        for row in members:
            where.setdefault(row.group, set()).add(name)
    assert all(len(names) == 1 for names in where.values())
```

- [ ] **Step 2:** Run `uv run pytest tests/test_threads.py -q`. Expected: FAIL (rows of one group land in several splits).
- [ ] **Step 3: Implement.** In `split_rows`, compute the split key per row as `getattr(row, "group", None) or row.id` and pass that key to `split_name`. Leave every other behaviour unchanged.
- [ ] **Step 4:** Run `uv run pytest -q`. Expected: all pass.
- [ ] **Step 5:** Commit `data.py` and `tests/test_threads.py`: `feat(jev-optimize): split rows by group so one thread stays in one split`.

### Task A2: walk and expiry question specs

**Files:** Modify `jev_optimize/questions.py`. Test: `tests/test_threads.py`.

Production state shapes (Part C sends exactly these):

```text
walk.*  (one request per later paragraph)
{"user": {"name": str}, "waiting_party": str, "ask": str, "due": str,
 "later": {"author_role": "waiting_party" | "user" | "third_party", "paragraph_text": str}}

walk.new_date
{"ask": str, "later": {"paragraph_text": str}}

expiry.event
{"ask": str, "waiting_party": str, "due": str}

expiry.ends_with_event
{"ask": str, "waiting_party": str, "event": str}
```

- [ ] **Step 1: Write the failing test**

```python
from jev_optimize.questions import SETS, SPECS

WALK = (
    "walk.responds", "walk.wp_received", "walk.wp_no_longer_needed", "walk.wp_asks_again",
    "walk.wp_changes_terms", "walk.wp_still_owed", "walk.wp_changes_due",
    "walk.user_delivers", "walk.user_promises", "walk.user_declines", "walk.user_hands_off",
    "walk.third_party_done", "walk.new_date",
)
EXPIRY = ("expiry.event", "expiry.ends_with_event")


def test_walk_and_expiry_sets_are_registered():
    assert SETS["walk"] == WALK
    assert SETS["expiry"] == EXPIRY
    for qid in WALK[:-1] + ("expiry.ends_with_event",):
        assert SPECS[qid].kind == "noul"
    assert SPECS["walk.new_date"].kind == "choice"
    assert SPECS["expiry.event"].kind == "choice"
    assert set(SPECS["walk.responds"].signature.input_fields) == {
        "user", "waiting_party", "ask", "due", "later"}
    assert set(SPECS["expiry.ends_with_event"].signature.input_fields) == {
        "ask", "waiting_party", "event"}
```

- [ ] **Step 2:** Run it. Expected: FAIL (`KeyError: 'walk'`).
- [ ] **Step 3: Implement.** Add after the rules signatures:

```python
class WalkBase(dspy.Signature):
    user: dict[str, str] = dspy.InputField()
    waiting_party: str = dspy.InputField()
    ask: str = dspy.InputField()
    due: str = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()


def _walk_signature(name: str, instructions: str) -> type[dspy.Signature]:
    return type(name, (WalkBase,), {
        "__doc__": instructions,
        "__annotations__": {"decision": Noul},
        "decision": dspy.OutputField(desc="Probability that the statement is true."),
    })


_WALK_STATEMENTS = {
    "walk.responds": "`later.paragraph_text` responds to the request in `ask`.",
    "walk.wp_received": "In `later.paragraph_text`, `waiting_party` says they received what `ask` requests.",
    "walk.wp_no_longer_needed": "In `later.paragraph_text`, `waiting_party` says they no longer need what `ask` requests.",
    "walk.wp_asks_again": "In `later.paragraph_text`, `waiting_party` asks again for what `ask` requests or asks for an update on it.",
    "walk.wp_changes_terms": "In `later.paragraph_text`, `waiting_party` changes what `ask` requests.",
    "walk.wp_still_owed": "After `later.paragraph_text`, `user.name` still owes `waiting_party` an action for `ask`.",
    "walk.wp_changes_due": "In `later.paragraph_text`, `waiting_party` changes when `ask` must be done.",
    "walk.user_delivers": "In `later.paragraph_text`, `user.name` gives `waiting_party` what `ask` requests.",
    "walk.user_promises": "In `later.paragraph_text`, `user.name` says they will do what `ask` requests at a later time.",
    "walk.user_declines": "In `later.paragraph_text`, `user.name` declines to do what `ask` requests.",
    "walk.user_hands_off": "In `later.paragraph_text`, `user.name` says another person will do what `ask` requests.",
    "walk.third_party_done": "In `later.paragraph_text`, the writer says they did what `ask` requests or will do it.",
}

_WALK_INTENTS = {
    "walk.responds": "The later paragraph is about the request.",
    "walk.wp_received": "The waiting party confirms receipt.",
    "walk.wp_no_longer_needed": "The waiting party withdraws the request.",
    "walk.wp_asks_again": "The waiting party chases the request.",
    "walk.wp_changes_terms": "The waiting party changes what the request requires.",
    "walk.wp_still_owed": "The user still owes an action after the change.",
    "walk.wp_changes_due": "The waiting party changes the due time.",
    "walk.user_delivers": "The user delivers what was requested.",
    "walk.user_promises": "The user promises to do it later.",
    "walk.user_declines": "The user declines the request.",
    "walk.user_hands_off": "The user hands the request to another person.",
    "walk.third_party_done": "Another participant did or will do the request.",
}

_WALK_FIELDS = ("user.name", "waiting_party", "ask", "due", "later.author_role",
                "later.paragraph_text")


class WalkNewDate(dspy.Signature):
    """Which date does `later.paragraph_text` set as the new time for `ask`?"""

    ask: str = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: Choice[(("none", "The paragraph sets no new date for the request."),)] = (
        dspy.OutputField(desc="The issued date handle, or none."))


class ExpiryEvent(dspy.Signature):
    """Which event does `ask` help `waiting_party` prepare for or take part in?"""

    ask: str = dspy.InputField()
    waiting_party: str = dspy.InputField()
    due: str = dspy.InputField()
    decision: Choice[(("none", "The request serves no listed event."),)] = (
        dspy.OutputField(desc="The issued event handle, or none."))


class ExpiryEndsWithEvent(dspy.Signature):
    """`ask` has no purpose after `event` ends."""

    ask: str = dspy.InputField()
    waiting_party: str = dspy.InputField()
    event: str = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the statement is true.")
```

Add to `SPECS` (keep insertion order as listed in Step 1):

```python
**{
    qid: QuestionSpec("walk", _walk_signature(qid.replace(".", "_").title(), text),
                      _WALK_INTENTS[qid], ("later.paragraph_text",), _WALK_FIELDS)
    for qid, text in _WALK_STATEMENTS.items()
},
"walk.new_date": QuestionSpec(
    "walk", WalkNewDate, "The new date the paragraph sets for the request.",
    ("later.paragraph_text",), ("ask", "later.paragraph_text"), "choice",
    {"none": "The paragraph sets no new date for the request."},
),
"expiry.event": QuestionSpec(
    "expiry", ExpiryEvent, "The event the request serves.", ("ask",),
    ("ask", "waiting_party", "due"), "choice",
    {"none": "The request serves no listed event."},
),
"expiry.ends_with_event": QuestionSpec(
    "expiry", ExpiryEndsWithEvent, "The request loses its purpose when the event ends.",
    ("ask", "event"), ("ask", "waiting_party", "event"),
),
```

Change the `SETS` name tuple to `("closure", "triage", "rules", "extract", "walk", "expiry")`.

If `SPECS` is a plain dict literal, build it as `SPECS = {...existing..., **_walk_specs(), ...}`, where `_walk_specs()` returns the dict comprehension above. If `JevProposer` rejects a dotted field such as `later.author_role`, add it the way the closure dotted fields are handled. Do not loosen the proposer rules.

- [ ] **Step 4:** Run `uv run pytest -q`. Expected: the new test passes. If `test_wire_shape.py` or `test_registry_synthetic.py` now fail only because `data/synthetic/walk.jsonl` or `expiry.jsonl` do not exist, Task A4 fixes that. Note the failures and continue.
- [ ] **Step 5:** Commit `questions.py` and the test: `feat(jev-optimize): walk and expiry question specs`.

### Task A3: thread model, stub generator and row derivation

**Files:** Create `jev_optimize/threads.py`. Test: `tests/test_threads.py`.

A thread is one ask plus 1 to 3 later messages. Each later message has an author role, 1 to 3 paragraphs, and at most one responding paragraph with an outcome. The plan fixes labels before any text exists; text comes from templates (stub) or from the LLM (Task A5).

- [ ] **Step 1: Write the failing tests**

```python
from jev_optimize.threads import (OUTCOMES, ThreadPlan, derive_rows, plan_threads,
                                  stub_threads)


def test_plans_cover_every_outcome_and_hard_case():
    plans = plan_threads(120, seed=7)
    seen = {m.outcome for p in plans for m in p.later}
    assert seen >= set(OUTCOMES)
    kinds = {p.hard_case for p in plans}
    assert {"rsvp_before_event", "pay_by_date", "prepare_for_meeting",
            "correction_amends", "correction_ends", "chaser_after_delivery",
            "unrelated_later_message"} <= kinds


def test_walk_stops_after_a_terminal_outcome():
    plan = ThreadPlan(
        thread_id="t-1", hard_case="none", has_event=False, ends_with_event=None,
        later=[_m("waiting_party", "wp_received"), _m("waiting_party", "wp_asks_again")],
    )
    rows = derive_rows(stub_threads([plan]))
    walk = [r for r in rows if r["set"] == "walk"]
    assert {r["group"] for r in walk} == {"t-1"}
    assert not any(r["label"].get("walk.wp_asks_again") for r in walk)


def test_responding_paragraph_carries_role_labels_and_others_only_the_gate():
    plan = ThreadPlan(thread_id="t-2", hard_case="none", has_event=False,
                      ends_with_event=None,
                      later=[_m("user", "user_delivers", paragraphs=2, responding=1)])
    walk = [r for r in derive_rows(stub_threads([plan])) if r["set"] == "walk"]
    gate_only = [r for r in walk if set(r["label"]) == {"walk.responds"}]
    full = [r for r in walk if "walk.user_delivers" in r["label"]]
    assert len(gate_only) == 1 and gate_only[0]["label"]["walk.responds"] is False
    assert len(full) == 1 and full[0]["label"]["walk.user_delivers"] is True
    assert full[0]["label"]["walk.user_declines"] is False
    assert full[0]["later"]["author_role"] == "user"


def test_expiry_rows_only_for_threads_with_an_event():
    with_event = ThreadPlan(thread_id="t-3", hard_case="rsvp_before_event", has_event=True,
                            ends_with_event=True, later=[])
    without = ThreadPlan(thread_id="t-4", hard_case="pay_by_date", has_event=False,
                         ends_with_event=None, later=[])
    rows = derive_rows(stub_threads([with_event, without]))
    expiry = [r for r in rows if r["set"] == "expiry"]
    assert [r["group"] for r in expiry] == ["t-3"]
    assert expiry[0]["label"] == {"expiry.ends_with_event": True}


def _m(role, outcome, paragraphs=1, responding=0):
    from jev_optimize.threads import MessagePlan
    return MessagePlan(author_role=role, outcome=outcome, paragraphs=paragraphs,
                       responding=responding)
```

- [ ] **Step 2:** Run them. Expected: FAIL (`ModuleNotFoundError: jev_optimize.threads`).
- [ ] **Step 3: Implement `threads.py`.**

```python
"""Synthetic email threads for the closure walk and their derived rows."""

from __future__ import annotations

import hashlib
import random
from dataclasses import dataclass, field
from typing import Any

from .synthetic import SYNTHETIC_PEOPLE

ROLE_OUTCOMES = {
    "waiting_party": ("wp_received", "wp_no_longer_needed", "wp_asks_again",
                      "wp_changes_terms_owed", "wp_changes_terms_ends", "wp_changes_due",
                      "unrelated"),
    "user": ("user_delivers", "user_promises", "user_declines", "user_hands_off",
             "unrelated"),
    "third_party": ("third_party_done", "unrelated"),
}
OUTCOMES = tuple(sorted({o for v in ROLE_OUTCOMES.values() for o in v}))
TERMINAL = {"wp_received", "wp_no_longer_needed", "wp_changes_terms_ends",
            "user_delivers", "user_declines", "user_hands_off", "third_party_done"}
HARD_CASES = ("none", "rsvp_before_event", "pay_by_date", "prepare_for_meeting",
              "correction_amends", "correction_ends", "thanks_will_look",
              "partial_delivery", "chaser_after_delivery", "unrelated_later_message")
BRANCH_IDS = {
    "waiting_party": ("walk.wp_received", "walk.wp_no_longer_needed", "walk.wp_asks_again",
                      "walk.wp_changes_terms", "walk.wp_still_owed", "walk.wp_changes_due"),
    "user": ("walk.user_delivers", "walk.user_promises", "walk.user_declines",
             "walk.user_hands_off"),
    "third_party": ("walk.third_party_done",),
}


@dataclass(frozen=True)
class MessagePlan:
    author_role: str
    outcome: str
    paragraphs: int = 1
    responding: int = 0


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
    later: list[list[str]]  # paragraphs per later message


def branch_labels(role: str, outcome: str) -> dict[str, bool]:
    labels = {qid: False for qid in BRANCH_IDS[role]}
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
    for qid in target:
        labels[qid] = True
    if role == "waiting_party" and outcome in {"wp_asks_again"}:
        labels["walk.wp_still_owed"] = True
    return labels


def plan_threads(count: int, seed: int = 20261021) -> list[ThreadPlan]:
    """Fixed plans: every hard case and every role outcome appear in rotation."""
    rng = random.Random(seed)
    plans = []
    role_cycle = [(role, outcome) for role, outs in ROLE_OUTCOMES.items() for outcome in outs]
    for index in range(count):
        hard_case = HARD_CASES[index % len(HARD_CASES)]
        later = _later_for(hard_case, index, role_cycle, rng)
        has_event = hard_case in {"rsvp_before_event", "prepare_for_meeting"} or index % 7 == 0
        ends = None
        if has_event:
            ends = hard_case in {"rsvp_before_event", "prepare_for_meeting"} or index % 2 == 0
        plans.append(ThreadPlan(f"thread-{index:05d}", hard_case, has_event, ends, later))
    return plans


def _later_for(hard_case, index, role_cycle, rng) -> list[MessagePlan]:
    fixed = {
        "correction_amends": [MessagePlan("waiting_party", "wp_changes_terms_owed")],
        "correction_ends": [MessagePlan("waiting_party", "wp_changes_terms_ends")],
        "thanks_will_look": [MessagePlan("user", "user_promises")],
        "partial_delivery": [MessagePlan("user", "user_promises", 2, 1)],
        "chaser_after_delivery": [MessagePlan("user", "user_delivers"),
                                  MessagePlan("waiting_party", "wp_asks_again")],
        "unrelated_later_message": [MessagePlan("third_party", "unrelated", 2, 0)],
    }
    if hard_case in fixed:
        return fixed[hard_case]
    count = 1 + index % 3
    out = []
    for step in range(count):
        role, outcome = role_cycle[(index + step) % len(role_cycle)]
        paragraphs = 1 + rng.randrange(3)
        out.append(MessagePlan(role, outcome, paragraphs, rng.randrange(paragraphs)))
    return out


def stub_threads(plans: list[ThreadPlan]) -> list[Thread]:
    """Deterministic template text; keyword markers keep the offline fake predictable."""
    threads = []
    for n, plan in enumerate(plans):
        user = SYNTHETIC_PEOPLE[n % len(SYNTHETIC_PEOPLE)].split()[0]
        waiting = SYNTHETIC_PEOPLE[(n + 3) % len(SYNTHETIC_PEOPLE)]
        ask, due, event = _stub_ask(plan, n)
        later = []
        for m in plan.later:
            paragraphs = [f"Unrelated note {n}-{i}: the parking garage closes early this week."
                          for i in range(m.paragraphs)]
            paragraphs[m.responding] = _stub_paragraph(m, n)
            later.append(paragraphs)
        threads.append(Thread(plan, user, waiting, ask, due, event, later))
    return threads


def _stub_ask(plan: ThreadPlan, n: int) -> tuple[str, str, str | None]:
    if plan.hard_case == "rsvp_before_event":
        return (f"Please RSVP by Friday for the spring gala {n}.", "Friday",
                f"the spring gala {n} on 2026-10-30")
    if plan.hard_case == "prepare_for_meeting":
        return (f"Please prepare the slides for board meeting {n} on 9/12.", "9/12",
                f"board meeting {n} on 2026-09-12")
    if plan.hard_case == "pay_by_date":
        return (f"Please pay invoice {n} by 9/12.", "9/12", None)
    event = f"planning session {n} on 2026-11-05" if plan.has_event else None
    return (f"Could you send the vendor summary {n} before the planning session?",
            "not stated", event)


def _stub_paragraph(m: MessagePlan, n: int) -> str:
    return {
        "wp_received": f"Got the summary {n}, thanks. RECEIVED.",
        "wp_no_longer_needed": f"Never mind summary {n}, we no longer need it. WITHDRAWN.",
        "wp_asks_again": f"Any update on summary {n}? CHASE.",
        "wp_changes_terms_owed": f"Small change to summary {n}: add the Q3 totals. AMEND-OWED.",
        "wp_changes_terms_ends": f"Forget summary {n}, we will use the dashboard. AMEND-ENDS.",
        "wp_changes_due": f"Can you get summary {n} to me by Wednesday instead? NEWDATE.",
        "user_delivers": f"Attached is summary {n}. DELIVERED.",
        "user_promises": f"I will send summary {n} next week. PROMISE.",
        "user_declines": f"I can't take on summary {n}, sorry. DECLINE.",
        "user_hands_off": f"Looping in a colleague who will send summary {n}. HANDOFF.",
        "third_party_done": f"I sent summary {n} this morning. THIRDDONE.",
        "unrelated": f"Separate topic {n}: the offsite menu is final. UNRELATED.",
    }[m.outcome]


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
                rows.append(_walk_row(thread, ask, message.author_role, text, label, source))
            if message.outcome == "wp_changes_terms_owed":
                ask = f"{ask}\n\nLater change: {paragraphs[message.responding]}"
            if message.outcome in TERMINAL:
                break
        if thread.event is not None and thread.plan.ends_with_event is not None:
            rows.append(_with_id({
                "set": "expiry", "group": thread.plan.thread_id, "source": source,
                "ask": thread.ask, "waiting_party": thread.waiting_party,
                "event": thread.event,
                "label": {"expiry.ends_with_event": thread.plan.ends_with_event},
            }))
    return rows


def _walk_row(thread, ask, role, text, label, source) -> dict[str, Any]:
    return _with_id({
        "set": "walk", "group": thread.plan.thread_id, "source": source,
        "user": {"name": thread.user_name}, "waiting_party": thread.waiting_party,
        "ask": ask, "due": thread.due,
        "later": {"author_role": role, "paragraph_text": text}, "label": label,
    })


def _with_id(row: dict[str, Any]) -> dict[str, Any]:
    payload = {k: v for k, v in row.items() if k not in {"label", "set", "source", "id"}}
    digest = hashlib.sha256(repr(sorted(payload.items())).encode("utf-8")).hexdigest()[:20]
    return {**row, "id": f"{row['set']}-{digest}"}
```

Keep `_stub_paragraph` markers in upper case. The offline fake in Task A6 keys on them.

- [ ] **Step 4:** Run `uv run pytest tests/test_threads.py -q`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(jev-optimize): synthetic thread plans, stub threads and walk/expiry row derivation`.

### Task A4: corpus checks and the stub corpora

**Files:** Modify `jev_optimize/synthetic.py` (`check_corpus`, `generate_llm`), `jev_optimize/cli.py`. Create `data/synthetic/threads.jsonl`, `walk.jsonl`, `expiry.jsonl`. Test: `tests/test_threads.py`, `tests/test_generate_llm.py`.

- [ ] **Step 1: Write the failing tests**

```python
import pytest

from jev_optimize.synthetic import check_corpus, generate_llm
from jev_optimize.threads import derive_rows, plan_threads, stub_threads
from jev_optimize.data import write_jsonl


def test_check_corpus_accepts_a_stub_walk_corpus(tmp_path):
    rows = [r for r in derive_rows(stub_threads(plan_threads(400))) if r["set"] == "walk"]
    path = tmp_path / "walk.jsonl"
    write_jsonl(path, rows)
    report = check_corpus(path)
    assert report["set"] == "walk"
    assert report["questions"]["walk.responds"]["rows"] >= 40


def test_generate_llm_refuses_thread_derived_sets(tmp_path):
    with pytest.raises(ValueError, match="derived from threads"):
        generate_llm(tmp_path, selected_set="walk", rows=600)
```

In `tests/test_generate_llm.py`, exclude `walk` and `expiry` from the set parametrisation the same way `extract` is excluded.

- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `generate_llm` and `generate_stub`: for `selected_set in {"walk", "expiry"}`, raise `ValueError("walk and expiry rows are derived from threads; use generate-threads")`. With `selected_set == "all"`, skip those two sets.
  - `check_corpus`: when the set is `walk` or `expiry`, apply thread rules instead of the paragraph rules:
    - every row has `group`;
    - stable id recomputed with `threads._with_id` matches;
    - `scrub_row` passes;
    - for each question in `SETS[set]` except `walk.new_date` and `expiry.event`: report `rows`, `positives` and `negatives`, and require at least 20 of each (error text `"{qid}: needs 20 positives and 20 negatives"`);
    - distinct `later.paragraph_text` at least 300 for walk; distinct `ask` at least 40 for both.
    - Return the same report keys as today (`set`, `rows`, `distinct_paragraph_texts`, `distinct_obligation_texts`, `questions`, `errors`).
  - `cli.py`: add `generate-threads` with `--stub | --llm`, `--threads` (default 400 stub, 600 llm), `--seed`, `--smoke`, `--output` (default `DEFAULT_ROOT`). Stub mode writes `threads.jsonl` (one line per thread: plan plus texts), `walk.jsonl` and `expiry.jsonl`, then runs `check_corpus` on both derived files. LLM mode calls Task A5's `generate_threads_llm`.
- [ ] **Step 4:** Generate and check the committed stub corpora:

```powershell
uv run jev-optimize generate-threads --stub --threads 400
uv run jev-optimize check-corpus data/synthetic/walk.jsonl
uv run jev-optimize check-corpus data/synthetic/expiry.jsonl
```

Expected: both reports print with `"errors": []`. If a question has fewer than 20 positives, raise `--threads`, and the plan rotation if needed, until each passes. Write the final count in the README (Task A7).
- [ ] **Step 5:** Run `uv run pytest -q`. Expected: all pass, including `test_wire_shape.py` for the new ids. The wire test loads `data/synthetic/{set}.jsonl`, so the stub corpora must exist first.
- [ ] **Step 6:** Commit `synthetic.py`, `cli.py`, the tests, and the three data files: `feat(jev-optimize): thread corpus checks and committed stub walk/expiry corpora`.

### Task A5: LLM thread generator, tested offline

**Files:** Modify `jev_optimize/threads.py`. Test: `tests/test_threads.py`.

- [ ] **Step 1: Write the failing test.** A fake OpenRouter answers through `httpx.MockTransport`:
  - batch 1 has realistic texts that match the plans;
  - batch 2 is malformed JSON;
  - batch 3 has one thread with a URL, which `scrub_row` rejects, and one thread whose paragraph count does not match its plan.

```python
import json

import httpx

from jev_optimize.threads import generate_threads_llm, plan_threads


class FakeOpenRouter:
    def __init__(self):
        self.calls = 0

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.calls += 1
        body = json.loads(request.content)
        assert body["provider"] == {"zdr": True}
        plans = json.loads(body["messages"][-1]["content"].split("Plans:\n", 1)[1])
        if self.calls == 2:
            return httpx.Response(200, json={"choices": [{"message": {"content": "{not json"}}]})
        threads = []
        for i, plan in enumerate(plans):
            later = [{"paragraphs": [f"Paragraph {j} about the vendor summary for {plan['thread_id']}."
                                     for j in range(m["paragraphs"])]} for m in plan["later"]]
            ask = f"Could you send the vendor summary for {plan['thread_id']} by Friday?"
            if self.calls == 3 and i == 0:
                ask += " See https://example.com/x"
            if self.calls == 3 and i == 1 and later:
                later[0]["paragraphs"].append("An extra paragraph.")
            threads.append({"thread_id": plan["thread_id"], "ask": ask, "due": "Friday",
                            "event": plan.get("event_hint"), "later": later})
        return httpx.Response(200, json={"choices": [{"message": {"content": json.dumps({"threads": threads})}}]})


def test_llm_threads_reject_bad_rows_and_retry_offline(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENROUTER_API_KEY", "test-key-not-real")
    monkeypatch.setenv("OPENROUTER_SYNTH_MODEL", "fake/model")
    fake = FakeOpenRouter()
    client = httpx.Client(transport=httpx.MockTransport(fake))
    out = generate_threads_llm(tmp_path, threads=24, seed=3, client=client, parallel=1,
                               smoke=True, batch_size=8)
    lines = (tmp_path / "threads.jsonl").read_text(encoding="utf-8").splitlines()
    assert len(lines) == 24
    assert fake.calls >= 4  # the malformed batch and the rejected threads were re-queued
    assert "https://" not in (tmp_path / "walk.jsonl").read_text(encoding="utf-8")
    assert out["rejected"] >= 2
```

- [ ] **Step 2:** Run. Expected: FAIL (`ImportError`).
- [ ] **Step 3: Implement `generate_threads_llm(output, *, threads, seed, client=None, parallel=8, smoke=False, batch_size=8) -> dict`.** Follow `synthetic.generate_llm`'s pattern:
  - `OPENROUTER_API_KEY` and `OPENROUTER_SYNTH_MODEL` are required.
  - POST to `https://openrouter.ai/api/v1/chat/completions` with `provider: {"zdr": true}`, `temperature 0.9` and a strict `json_schema` response format `{threads: [{thread_id, ask, due, event, later: [{paragraphs: [str]}]}]}`.
  - The prompt names the invented-name pool, requires `example.invalid` addresses and no URLs, describes each plan's role, outcome, paragraph count, responding index and hard case, and ends with `"Plans:\n" + json.dumps(plans)`.
  - Validation per thread:
    - the `thread_id` matches a plan;
    - the paragraph counts equal the plan's;
    - `scrub_row` passes on every text;
    - `due` is non-empty;
    - `event` is non-empty only when the plan has an event.
  - Rejected and malformed batches are re-queued, capped at `threads * 8` attempts.
  - Labels always come from the plan, never from the model.
  - Write `threads.jsonl`, then `derive_rows(..., source="synthetic-llm")` into `walk.jsonl` and `expiry.jsonl`, then `check_corpus` both. In smoke mode, relax the minimums the way `generate_llm` does.
  - Return `{"threads": n, "rejected": k, "files": [...]}`.
- [ ] **Step 4:** Run `uv run pytest -q`. Expected: all pass.
- [ ] **Step 5:** Commit: `feat(jev-optimize): LLM thread generator with offline fake-endpoint test`.

### Task A6: optimize a subset, end to end offline

**Files:** Modify `jev_optimize/optimize.py` (`optimize_set`), `jev_optimize/cli.py`, `tests/test_offline_end_to_end.py`.

- [ ] **Step 1: Write the failing test.** Add to `tests/test_offline_end_to_end.py`:

```python
def test_walk_subset_optimizes_offline(tmp_path, monkeypatch):
    from jev_optimize.data import write_jsonl
    from jev_optimize.threads import derive_rows, plan_threads, stub_threads

    monkeypatch.chdir(tmp_path)
    rows = [r for r in derive_rows(stub_threads(plan_threads(400))) if r["set"] == "walk"]
    data = tmp_path / "walk.jsonl"
    write_jsonl(data, rows)
    endpoint = FakeDecisionsEndpoint()
    result = optimize_set("walk", _client(endpoint), data, budget="light",
                          reflection_lm=_reflection_lm(), only=("walk.responds", "walk.user_delivers"))
    assert set(result["questions"]) == {"walk.responds", "walk.user_delivers"}
    for qid in result["questions"]:
        q = result["questions"][qid]
        assert ESCALATE_FLOOR <= q["escalate"] < q["accept"]
    assert endpoint.seen_ids >= {"walk.responds", "walk.user_delivers"}
```

Extend `FakeDecisionsEndpoint`'s noul heuristic so walk questions answer from the stub markers: `walk.responds` is high unless the paragraph contains `UNRELATED` or `Unrelated note`. Each branch id is high only when its marker is present: `DELIVERED` for `user_delivers`, `RECEIVED` for `wp_received`, and so on, matching `_stub_paragraph`. Keep the existing hash jitter. If the endpoint has no `seen_ids` attribute, add one that records question ids. Reuse whatever reflection-model fixture the triage test uses as `_reflection_lm()`, and make its proposed statement mention `later.paragraph_text`.

- [ ] **Step 2:** Run `uv run pytest tests/test_offline_end_to_end.py -q`. Expected: FAIL (`unexpected keyword argument 'only'`).
- [ ] **Step 3: Implement.** `optimize_set(..., only: tuple[str, ...] | None = None)` iterates `[q for q in SETS[set_name] if only is None or q in only]`. Skip `walk.new_date` and `expiry.event` with `{"kept_baseline": True, "baseline_reason": "dynamic options are issued per request"}` when they are selected. Add `--only` (comma-separated) to the `optimize` subcommand. Its data default for `walk` and `expiry` is `data/synthetic/<set>.jsonl`.
- [ ] **Step 4:** Run `uv run pytest -q` and `uv run ruff check .`. Expected: all pass, ruff clean.
- [ ] **Step 5:** Commit: `feat(jev-optimize): optimize --only and an offline walk end-to-end test`.

### Task A6b: thread-level walk metric

The spec makes "share of held-out threads that end in the correct status" the go/no-go number. Per-question metrics do not give it, because errors compound along a path.

**Files:** Create `jev_optimize/walk_metric.py`. Modify `cli.py`. Test: `tests/test_threads.py`.

- [ ] **Step 1: Write the failing test**

```python
from jev_optimize.walk_metric import expected_status, walk_status


def _answers(role, gate, **branch):
    from jev_optimize.threads import BRANCH_IDS
    probs = {"walk.responds": gate}
    probs.update({qid: branch.get(qid.split(".")[1], 0.05) for qid in BRANCH_IDS[role]})
    return probs


THRESHOLDS = {qid: {"accept": 0.7, "escalate": 0.3} for qid in (
    "walk.responds", "walk.wp_received", "walk.wp_no_longer_needed", "walk.wp_asks_again",
    "walk.wp_changes_terms", "walk.wp_still_owed", "walk.wp_changes_due",
    "walk.user_delivers", "walk.user_promises", "walk.user_declines", "walk.user_hands_off",
    "walk.third_party_done")}


def test_walk_status_matches_the_rust_verdict_rules():
    steps = [("user", [_answers("user", 0.95, user_delivers=0.9)]),
             ("waiting_party", [_answers("waiting_party", 0.95, wp_received=0.95)])]
    assert walk_status(steps, THRESHOLDS) == "closed:completed"
    gray = [("user", [_answers("user", 0.5)])]
    assert walk_status(gray, THRESHOLDS) == "escalated"
    none = [("third_party", [_answers("third_party", 0.05)])]
    assert walk_status(none, THRESHOLDS) == "open"


def test_expected_status_reads_the_plan():
    from jev_optimize.threads import MessagePlan, ThreadPlan
    plan = ThreadPlan("t", "none", False, None,
                      [MessagePlan("user", "user_delivers"), MessagePlan("waiting_party", "wp_received")])
    assert expected_status(plan) == "closed:completed"
```

- [ ] **Step 2:** Run. Expected: FAIL (`ModuleNotFoundError`).
- [ ] **Step 3: Implement** `walk_metric.py`:
  - `walk_status(steps, thresholds) -> str` ports `step_verdict` and the apply rules from Task C2 and C3 exactly (gate bands, gray escalation, terminal precedence, conflicts, still-owed split).
  - It returns one of:
    - `"open"`;
    - `"escalated"`;
    - `"closed:completed"` or `"closed:withdrawn"` (automatic);
    - `"suggested:closure"`, `"suggested:declined"` or `"suggested:delegated"` (the last suggestion still pending);
    - `"open:amended"`, `"open:rescheduled"` or `"open:asked_again"` (the last non-terminal change, when nothing closed).
  - `expected_status(plan) -> str` computes the same label from the plan's outcomes, with the same walk rules.
  - Add CLI `evaluate-walk --threads data/synthetic/threads.jsonl [--replay PATH]`:
    - uses the test-split threads (group split);
    - asks Jev per paragraph through `DecisionsClient` (paid) or a `ReplayClient`;
    - uses the registry thresholds;
    - prints the share of threads whose `walk_status` equals `expected_status`, the share escalated, and a confusion table.
  - Add an offline test that runs `evaluate-walk`'s function against `FakeDecisionsEndpoint` on 40 stub threads and checks that the report has these keys.
- [ ] **Step 4:** Run `uv run pytest -q`. Expected: pass.
- [ ] **Step 5:** Commit: `feat(jev-optimize): thread-level walk accuracy (evaluate-walk)`.

### Task A7: README

**Files:** Modify `tools/jev-optimize/README.md`.

- [ ] **Step 1:** Add a "Walk and expiry sets" section:
  - what a thread is;
  - `generate-threads --stub` and `--llm --smoke`;
  - `check-corpus` thread rules;
  - group splits;
  - `optimize --set walk --only ...`;
  - which questions are not tuned yet (`walk.new_date`, `expiry.event`) and why;
  - the stub corpus size from Task A4.
- [ ] **Step 2:** Run `uv run pytest -q`. Expected: pass.
- [ ] **Step 3:** Stage, run `pwsh ./tools/check-public-repo.ps1 -Mode Staged` from the repo root, then commit: `docs(jev-optimize): walk and expiry sets`.

---

# Part B: registry (`chore/contract-walk-registry`, from `main`)

### Task B1: registry entries and required ids

**Files:** Modify `contracts/model/decision-questions.json` and `crates/openloops-inference/src/decision.rs` (`REQUIRED_IDS`, about line 481).

- [ ] **Step 1: Write the failing test.** In `decision.rs` tests, add:

```rust
#[test]
fn registry_contains_the_closure_walk_ids() {
    let registry = registry::Registry::get();
    for id in [
        "walk.responds", "walk.wp_received", "walk.wp_no_longer_needed", "walk.wp_asks_again",
        "walk.wp_changes_terms", "walk.wp_still_owed", "walk.wp_changes_due",
        "walk.user_delivers", "walk.user_promises", "walk.user_declines",
        "walk.user_hands_off", "walk.third_party_done", "walk.new_date",
        "expiry.event", "expiry.ends_with_event",
    ] {
        assert!(registry.question(id).is_some(), "{id}");
        assert!(registry::REQUIRED_IDS.contains(&id), "{id}");
    }
}
```

- [ ] **Step 2:** Run:

```powershell
cargo test -p openloops-inference --features ollama-cloud,openrouter --locked registry_contains_the_closure_walk_ids
```

Expected: FAIL.
- [ ] **Step 3: Implement.** Append the 15 ids to `REQUIRED_IDS` in the order of the test. Add these entries to `questions` in the JSON (keys sorted, two-space indent, matching the file's style):

| id | type | accept | escalate | instructions |
|---|---|---|---|---|
| `walk.responds` | noul | 0.7 | 0.3 | `` `later.paragraph_text` responds to the request in `ask`. `` |
| `walk.wp_received` | noul | 0.9 | 0.5 | `` In `later.paragraph_text`, `waiting_party` says they received what `ask` requests. `` |
| `walk.wp_no_longer_needed` | noul | 0.9 | 0.5 | `` In `later.paragraph_text`, `waiting_party` says they no longer need what `ask` requests. `` |
| `walk.wp_asks_again` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `waiting_party` asks again for what `ask` requests or asks for an update on it. `` |
| `walk.wp_changes_terms` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `waiting_party` changes what `ask` requests. `` |
| `walk.wp_still_owed` | noul | 0.9 | 0.5 | `` After `later.paragraph_text`, `user.name` still owes `waiting_party` an action for `ask`. `` |
| `walk.wp_changes_due` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `waiting_party` changes when `ask` must be done. `` |
| `walk.user_delivers` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `user.name` gives `waiting_party` what `ask` requests. `` |
| `walk.user_promises` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `user.name` says they will do what `ask` requests at a later time. `` |
| `walk.user_declines` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `user.name` declines to do what `ask` requests. `` |
| `walk.user_hands_off` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, `user.name` says another person will do what `ask` requests. `` |
| `walk.third_party_done` | noul | 0.7 | 0.3 | `` In `later.paragraph_text`, the writer says they did what `ask` requests or will do it. `` |
| `walk.new_date` | choice | 0.7 | 0.3 | `` Which date does `later.paragraph_text` set as the new time for `ask`? `` with options `{"none": "The paragraph sets no new date for the request."}` |
| `expiry.event` | choice | 0.7 | 0.3 | `` Which event does `ask` help `waiting_party` prepare for or take part in? `` with options `{"none": "The request serves no listed event."}` |
| `expiry.ends_with_event` | noul | 0.9 | 0.5 | `` `ask` has no purpose after `event` ends. `` |

The wording must equal the Part A docstrings character for character. Leave every existing entry, `metrics` and `tuned_at` untouched.
- [ ] **Step 4:** Run the full inference tests:

```powershell
cargo test -p openloops-inference --features ollama-cloud,openrouter --locked
```

Expected: all pass.
- [ ] **Step 5:** Do not commit yet. Task B2 re-pins the hash in the same commit.

### Task B2: re-pin the registry hash

**Files:** Modify `tools/check-model-boundary.ps1` (the `$decisionHash -ne '...'` literal only).

- [ ] **Step 1:** Run `pwsh ./tools/check-model-boundary.ps1`. Expected: `P0-MODEL-DECISIONS-001` fails because the hash changed.
- [ ] **Step 2:** Compute the new value with the PowerShell recipe (it differs from the Python digest because of `tuned_at`):

```powershell
$m = Get-Content contracts/model/decision-questions.json -Raw | ConvertFrom-Json
$c = $m | ConvertTo-Json -Depth 100 -Compress
$sha = [System.Security.Cryptography.SHA256]::Create()
($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($c)) | ForEach-Object { $_.ToString('x2') }) -join ''
```

- [ ] **Step 3:** Replace only the 64-hex literal in the `$decisionHash -ne '...'` line with that value. Change nothing else in the checker.
- [ ] **Step 4:** Run `pwsh ./tools/check-model-boundary.ps1` and `pwsh ./tools/test-model-boundary.ps1`. Expected: both pass.
- [ ] **Step 5:** Run the gate:

```powershell
cargo fmt --all -- --check
cargo clippy -p openloops-inference --features ollama-cloud,openrouter --all-targets --locked -- -D warnings
```

Expected: clean.
- [ ] **Step 6:** Stage the three files, run `pwsh ./tools/check-public-repo.ps1 -Mode Staged`, then commit: `chore(contract): bootstrap closure-walk and expiry questions in the decision registry (CONTRACT EDIT FOR OWNER REVIEW)`.

---

# Part C: desktop walk (`feat/jev-closure-walk`, from Part B's tip)

Gate commands for Part C (run all before every commit):

```powershell
cargo fmt --all -- --check
cargo clippy -p openloops-desktop --features native-ui,live-connection --all-targets --locked -- -D warnings
cargo test -p openloops-desktop --features native-ui --locked -- --test-threads=1
```

### Task C1: card types and their display

**Files:** Modify `claim_view.rs`, `review_model.rs`, `slint_review.rs`, and every `EventPassed { .. }` literal.

- [ ] **Step 1: Write the failing tests** in `slint_review.rs` tests, reusing `model_with_suggestion`, `suggestion`, `card_at` and `resolve_selected_suggestion`:

```rust
#[test]
fn accepting_a_delegation_moves_the_card_to_watching() {
    let (mut app, key) = model_with_suggestion(suggestion(SuggestedUpdateKind::Delegated, None));
    resolve_selected_suggestion(&mut app, Some(key), true);
    assert!(matches!(app.review.decisions.get(&key).decision, Decision::Watching));
}

#[test]
fn accepting_a_decline_closes_the_card_as_declined() {
    let (mut app, key) = model_with_suggestion(suggestion(SuggestedUpdateKind::Declined, None));
    resolve_selected_suggestion(&mut app, Some(key), true);
    let item = &app.review.scan.as_ref().unwrap().analysis.items[2];
    assert_eq!(item.resolution_kind, Some(ResolutionKind::Declined));
    assert!(matches!(app.review.decisions.get(&key).decision, Decision::Done));
}

#[test]
fn mention_origins_label_their_evidence_cards_and_pills() {
    let (mut app, _) = model_with_suggestion(suggestion(SuggestedUpdateKind::Closure, None));
    let item = &mut app.review.scan.as_mut().unwrap().analysis.items[2];
    for origin in [MentionOrigin::Restated, MentionOrigin::Amended, MentionOrigin::Rescheduled] {
        item.mentions.push(Mention { message: "m0".into(), block: 0, quote: "q".into(),
                                     timestamp: 1, origin });
    }
    let labels: Vec<String> = evidence_cards(item, &[]).into_iter().map(|c| c.label).collect();
    assert!(labels.contains(&"Asked again".to_string()));
    assert!(labels.contains(&"Changed by the requester".to_string()));
    assert!(labels.contains(&"Earlier deadline".to_string()));
    let pills: Vec<String> = pills_for(item, &card_at(&app, 2)).into_iter().map(|p| p.text).collect();
    assert!(pills.contains(&"Asked 2x".to_string()));
    assert!(pills.contains(&"Changed".to_string()));
    assert!(pills.contains(&"Due date changed".to_string()));
}
```

Add one test in `review_model.rs` beside `event_passed_closure_hides_sorts_labels_and_can_be_overridden`: an `EventPassed { expired: true, name: "the gala".into(), .. }` gives a status label that starts with `"Expired with the gala"`. Adjust field access (`app.review.decisions`, `scan`) to the real names.
- [ ] **Step 2:** Run the desktop tests. Expected: compile FAIL (unknown variants and field).
- [ ] **Step 3: Implement.**
  - `claim_view.rs`:
    - add `Declined` and `Delegated` to `SuggestedUpdateKind`;
    - add `Amended` and `Rescheduled` to `MentionOrigin`;
    - add `pub expired: bool` to `EventPassed`, and set `expired: false` at every existing construction (`close_from_index`, `close_from_stated_time`, fixtures).
  - `review_model.rs`:
    - add `SuggestionOutcome::MarkWatching`;
    - in `resolve_suggested_update`'s accept match, before the wildcard:
      - `(SuggestedUpdateKind::Declined, _)` sets `item.resolution = Some(Anchor { message: source_message, block: source_block, quote: evidence_text.clone(), context: evidence_text })` and `item.resolution_kind = Some(ResolutionKind::Declined)`, and returns `MarkHandled`;
      - `(SuggestedUpdateKind::Delegated, _)` returns `MarkWatching`;
    - `event_passed_status_label`: when `event.expired`, return `format!("Expired with {} (ended {date})", event.name)`.
  - `slint_review.rs`:
    - `suggested_kind_label`: `Declined => "You may have declined this"`, `Delegated => "You may have handed this off"`;
    - `suggested_view`: `Declined => ("declined", true, "Reject")`, `Delegated => ("delegated", true, "Reject")`;
    - `resolve_selected_suggestion`: `MarkWatching` sets the status "Moved to Watching." and calls `decide_selected(model, selected, DECISION_WATCHING)`;
    - `evidence_cards`: the mention label is `mention_label(mention.origin)`:

```rust
const fn mention_label(origin: MentionOrigin) -> &'static str {
    match origin {
        MentionOrigin::Restated | MentionOrigin::OwnerLink | MentionOrigin::Duplicate => "Asked again",
        MentionOrigin::Amended => "Changed by the requester",
        MentionOrigin::Rescheduled => "Earlier deadline",
    }
}
```

    - `pills_for`: "Asked {n}x" counts only mentions whose origin is `Restated`, `OwnerLink` or `Duplicate` (n = count + 1, shown when count > 0). Add a neutral "Changed" pill when any mention is `Amended`, with hint "The requester changed what this asks for.", and a neutral "Due date changed" pill when any is `Rescheduled`, with hint "The requester moved the due date."
  - `training_export.rs` `closure_label_map`: the new kinds map to all-false labels. Check that its `==` comparisons still compile.
- [ ] **Step 4:** Run the gate. Expected: all pass, including the existing suggested-update, event-passed and mention tests.
- [ ] **Step 5:** Commit: `feat(review): declined and delegated suggestions, amended and rescheduled mentions, expired label`.

### Task C2: pure walk logic

**Files:** Create `crates/openloops-desktop/src/closure_walk.rs`. Modify `review_scan.rs`, adding near the top after the imports:

```rust
#[path = "closure_walk.rs"]
mod closure_walk;
```

`review_scan.rs` is itself loaded as `review_model::scanning` through `#[path]`, so `closure_walk` is a child module and reaches private `scanning` items through `super::`.

- [ ] **Step 1: Write the failing tests** in `closure_walk.rs` (`#[cfg(test)] mod tests`). Use registry thresholds, never literals:

```rust
use super::*;

fn accept(id: &str) -> f64 { Registry::get().question(id).unwrap().accept }
fn gray(id: &str) -> f64 {
    let q = Registry::get().question(id).unwrap();
    q.accept.midpoint(q.escalate)
}
fn no(id: &str) -> f64 { Registry::get().question(id).unwrap().escalate / 2.0 }

fn paragraph(ordinal: usize, role: AuthorRole, yes: &[&'static str], grays: &[&'static str]) -> ParagraphAnswers {
    let mut probabilities = BTreeMap::new();
    for id in std::iter::once(&GATE).chain(role.branch_ids()) {
        let p = if yes.contains(id) { accept(id) } else if grays.contains(id) { gray(id) } else { no(id) };
        probabilities.insert(*id, p);
    }
    ParagraphAnswers { ordinal, probabilities }
}

#[test]
fn no_responding_paragraph_is_no_response() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]), StepVerdict::NoResponse);
}

#[test]
fn a_gray_gate_with_no_accepted_gate_escalates() {
    let p = paragraph(0, AuthorRole::User, &[], &[GATE]);
    assert_eq!(step_verdict(AuthorRole::User, &[p]), StepVerdict::Escalate);
}

#[test]
fn waiting_party_receipt_closes_as_completed() {
    let p = paragraph(1, AuthorRole::WaitingParty, &[GATE, WP_RECEIVED], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]),
               StepVerdict::Apply(vec![Effect::Close { kind: ResolutionKind::Completed, ordinal: 1 }]));
}

#[test]
fn waiting_party_withdrawal_closes_as_withdrawn() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_NO_LONGER_NEEDED], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]),
               StepVerdict::Apply(vec![Effect::Close { kind: ResolutionKind::Withdrawn, ordinal: 0 }]));
}

#[test]
fn a_correction_that_leaves_the_action_owed_amends() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_CHANGES_TERMS, WP_STILL_OWED], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]),
               StepVerdict::Apply(vec![Effect::Amended { ordinal: 0 }]));
}

#[test]
fn a_correction_that_ends_the_action_withdraws() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_CHANGES_TERMS], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]),
               StepVerdict::Apply(vec![Effect::Close { kind: ResolutionKind::Withdrawn, ordinal: 0 }]));
}

#[test]
fn a_gray_still_owed_after_a_change_escalates() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_CHANGES_TERMS], &[WP_STILL_OWED]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[p]), StepVerdict::Escalate);
}

#[test]
fn chaser_and_new_date_combine() {
    let p = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_ASKS_AGAIN, WP_CHANGES_DUE], &[]);
    let StepVerdict::Apply(effects) = step_verdict(AuthorRole::WaitingParty, &[p]) else { panic!() };
    assert!(effects.contains(&Effect::AskedAgain { ordinal: 0 }));
    assert!(effects.iter().any(|e| matches!(e, Effect::NewDate { source: DateSource::WaitingParty, .. })));
}

#[test]
fn terminal_beats_non_terminal_and_conflicting_terminals_escalate() {
    let both = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_RECEIVED, WP_ASKS_AGAIN], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[both]),
               StepVerdict::Apply(vec![Effect::Close { kind: ResolutionKind::Completed, ordinal: 0 }]));
    let a = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_RECEIVED], &[]);
    let b = paragraph(1, AuthorRole::WaitingParty, &[GATE, WP_NO_LONGER_NEEDED], &[]);
    assert_eq!(step_verdict(AuthorRole::WaitingParty, &[a, b]), StepVerdict::Escalate);
}

#[test]
fn user_branches_become_suggestions() {
    for (id, kind) in [(USER_DELIVERS, SuggestedUpdateKind::Closure),
                       (USER_DECLINES, SuggestedUpdateKind::Declined),
                       (USER_HANDS_OFF, SuggestedUpdateKind::Delegated)] {
        let p = paragraph(0, AuthorRole::User, &[GATE, id], &[]);
        let StepVerdict::Apply(effects) = step_verdict(AuthorRole::User, &[p]) else { panic!() };
        assert!(matches!(effects.as_slice(), [Effect::Suggest { kind: k, ordinal: 0, .. }] if *k == kind));
    }
    let p = paragraph(0, AuthorRole::User, &[GATE, USER_PROMISES], &[]);
    let StepVerdict::Apply(effects) = step_verdict(AuthorRole::User, &[p]) else { panic!() };
    assert!(matches!(effects.as_slice(), [Effect::NewDate { source: DateSource::User, .. }]));
}

#[test]
fn third_party_done_suggests_handled_and_gray_branch_escalates() {
    let p = paragraph(0, AuthorRole::ThirdParty, &[GATE, THIRD_PARTY_DONE], &[]);
    let StepVerdict::Apply(effects) = step_verdict(AuthorRole::ThirdParty, &[p]) else { panic!() };
    assert!(matches!(effects.as_slice(), [Effect::Suggest { kind: SuggestedUpdateKind::Closure, .. }]));
    let p = paragraph(0, AuthorRole::User, &[GATE], &[USER_DELIVERS]);
    assert_eq!(step_verdict(AuthorRole::User, &[p]), StepVerdict::Escalate);
}

#[test]
fn walk_state_has_the_harness_shape() {
    let state = walk_state(&WalkContext { user_name: "Alex", waiting_party: "Priya Venkataraman",
        ask: "Send the summary.", due: "Friday" }, AuthorRole::User, "Attached.");
    assert_eq!(state.to_string(),
        r#"{"user":{"name":"Alex"},"waiting_party":"Priya Venkataraman","ask":"Send the summary.","due":"Friday","later":{"author_role":"user","paragraph_text":"Attached."}}"#);
}
```

- [ ] **Step 2:** Run the desktop tests. Expected: compile FAIL (module items missing).
- [ ] **Step 3: Implement** `closure_walk.rs`:

```rust
//! Closure walk: follows one open loop forward through later messages.
//! This module holds the pure parts (registry ids, request states,
//! question sets and the per-message verdict); `review_scan` sends the
//! requests and applies the verdicts to cards.

use std::collections::BTreeMap;

use openloops_inference::decision::{Answer, Answers, Questions, registry::Registry};
use openloops_inference::provider::ProviderError;

use crate::claim_view::{ResolutionKind, SuggestedUpdateKind};

pub(super) const GATE: &str = "walk.responds";
pub(super) const WP_RECEIVED: &str = "walk.wp_received";
pub(super) const WP_NO_LONGER_NEEDED: &str = "walk.wp_no_longer_needed";
pub(super) const WP_ASKS_AGAIN: &str = "walk.wp_asks_again";
pub(super) const WP_CHANGES_TERMS: &str = "walk.wp_changes_terms";
pub(super) const WP_STILL_OWED: &str = "walk.wp_still_owed";
pub(super) const WP_CHANGES_DUE: &str = "walk.wp_changes_due";
pub(super) const USER_DELIVERS: &str = "walk.user_delivers";
pub(super) const USER_PROMISES: &str = "walk.user_promises";
pub(super) const USER_DECLINES: &str = "walk.user_declines";
pub(super) const USER_HANDS_OFF: &str = "walk.user_hands_off";
pub(super) const THIRD_PARTY_DONE: &str = "walk.third_party_done";
pub(super) const NEW_DATE: &str = "walk.new_date";
pub(super) const EXPIRY_EVENT: &str = "expiry.event";
pub(super) const EXPIRY_ENDS: &str = "expiry.ends_with_event";
pub(super) const NONE_OPTION: &str = "none";

const WAITING_PARTY_IDS: [&str; 6] = [WP_RECEIVED, WP_NO_LONGER_NEEDED, WP_ASKS_AGAIN,
    WP_CHANGES_TERMS, WP_STILL_OWED, WP_CHANGES_DUE];
const USER_IDS: [&str; 4] = [USER_DELIVERS, USER_PROMISES, USER_DECLINES, USER_HANDS_OFF];
const THIRD_PARTY_IDS: [&str; 1] = [THIRD_PARTY_DONE];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorRole { WaitingParty, User, ThirdParty }

impl AuthorRole {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::WaitingParty => "waiting_party",
            Self::User => "user",
            Self::ThirdParty => "third_party",
        }
    }

    pub(super) const fn branch_ids(self) -> &'static [&'static str] {
        match self {
            Self::WaitingParty => &WAITING_PARTY_IDS,
            Self::User => &USER_IDS,
            Self::ThirdParty => &THIRD_PARTY_IDS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Band { Accept, Gray, Reject }

pub(super) fn band(id: &str, probability: f64) -> Band {
    let question = Registry::get().question(id).expect("closure walk registry question");
    if probability >= question.accept {
        Band::Accept
    } else if probability >= question.escalate {
        Band::Gray
    } else {
        Band::Reject
    }
}

pub(super) struct WalkContext<'a> {
    pub user_name: &'a str,
    pub waiting_party: &'a str,
    pub ask: &'a str,
    pub due: &'a str,
}

pub(super) fn walk_state(context: &WalkContext<'_>, role: AuthorRole, paragraph: &str) -> serde_json::Value {
    serde_json::json!({
        "user": {"name": context.user_name},
        "waiting_party": context.waiting_party,
        "ask": context.ask,
        "due": context.due,
        "later": {"author_role": role.wire(), "paragraph_text": paragraph},
    })
}

pub(super) fn walk_questions(role: AuthorRole) -> Result<Questions, ProviderError> {
    let mut ids = vec![GATE];
    ids.extend_from_slice(role.branch_ids());
    Questions::from_registry(&ids)
}

#[derive(Clone, Debug)]
pub(super) struct ParagraphAnswers {
    pub ordinal: usize,
    pub probabilities: BTreeMap<&'static str, f64>,
}

pub(super) fn paragraph_answers(ordinal: usize, role: AuthorRole, answers: &Answers) -> Result<ParagraphAnswers, ProviderError> {
    let mut probabilities = BTreeMap::new();
    for id in std::iter::once(&GATE).chain(role.branch_ids()) {
        let Some(Answer::Noul { probability }) = answers.get(id) else {
            return Err(ProviderError::InvalidResponse);
        };
        probabilities.insert(*id, *probability);
    }
    Ok(ParagraphAnswers { ordinal, probabilities })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DateSource { WaitingParty, User }

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Effect {
    /// Automatic: the waiting party confirmed or withdrew.
    Close { kind: ResolutionKind, ordinal: usize },
    /// Review: the user's own reply or a third party's message.
    Suggest { kind: SuggestedUpdateKind, ordinal: usize, probability: f64 },
    AskedAgain { ordinal: usize },
    Amended { ordinal: usize },
    NewDate { source: DateSource, ordinal: usize, probability: f64 },
}

impl Effect {
    const fn is_terminal(&self) -> bool {
        matches!(self, Self::Close { .. } | Self::Suggest { .. })
    }

    fn same_outcome(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Close { kind: a, .. }, Self::Close { kind: b, .. }) => a == b,
            (Self::Suggest { kind: a, .. }, Self::Suggest { kind: b, .. }) => a == b,
            _ => false,
        }
    }

    const fn rank(&self) -> u8 {
        match self {
            Self::Close { .. } | Self::Suggest { .. } => 0,
            Self::AskedAgain { .. } => 1,
            Self::Amended { .. } => 2,
            Self::NewDate { .. } => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum StepVerdict { NoResponse, Escalate, Apply(Vec<Effect>) }

/// One later message's verdict from its paragraphs' answers.
pub(super) fn step_verdict(role: AuthorRole, paragraphs: &[ParagraphAnswers]) -> StepVerdict {
    let gate = |p: &ParagraphAnswers| band(GATE, p.probabilities[GATE]);
    let responding: Vec<&ParagraphAnswers> = paragraphs.iter().filter(|p| gate(p) == Band::Accept).collect();
    if responding.is_empty() {
        return if paragraphs.iter().any(|p| gate(p) == Band::Gray) {
            StepVerdict::Escalate
        } else {
            StepVerdict::NoResponse
        };
    }
    let mut terminal = Vec::new();
    let mut other: Vec<Effect> = Vec::new();
    for paragraph in responding {
        let Some(effects) = paragraph_effects(role, paragraph) else {
            return StepVerdict::Escalate;
        };
        for effect in effects {
            if effect.is_terminal() {
                terminal.push(effect);
            } else if !other.iter().any(|kept| kept.rank() == effect.rank()) {
                other.push(effect);
            }
        }
    }
    if let Some(first) = terminal.first() {
        if terminal.iter().any(|effect| !effect.same_outcome(first)) {
            return StepVerdict::Escalate;
        }
        return StepVerdict::Apply(vec![first.clone()]);
    }
    if other.is_empty() {
        StepVerdict::NoResponse
    } else {
        other.sort_by_key(Effect::rank);
        StepVerdict::Apply(other)
    }
}

/// `None` when an answer that matters sits in the gray band.
fn paragraph_effects(role: AuthorRole, paragraph: &ParagraphAnswers) -> Option<Vec<Effect>> {
    let ordinal = paragraph.ordinal;
    let p = |id: &str| paragraph.probabilities[id];
    let yes = |id: &str| band(id, p(id)) == Band::Accept;
    let checked: &[&str] = match role {
        AuthorRole::WaitingParty => &[WP_RECEIVED, WP_NO_LONGER_NEEDED, WP_ASKS_AGAIN, WP_CHANGES_TERMS, WP_CHANGES_DUE],
        AuthorRole::User => &USER_IDS,
        AuthorRole::ThirdParty => &THIRD_PARTY_IDS,
    };
    if checked.iter().any(|id| band(id, p(id)) == Band::Gray) {
        return None;
    }
    let mut effects = Vec::new();
    match role {
        AuthorRole::WaitingParty => {
            if yes(WP_RECEIVED) {
                effects.push(Effect::Close { kind: ResolutionKind::Completed, ordinal });
            }
            if yes(WP_NO_LONGER_NEEDED) {
                effects.push(Effect::Close { kind: ResolutionKind::Withdrawn, ordinal });
            }
            if yes(WP_CHANGES_TERMS) {
                match band(WP_STILL_OWED, p(WP_STILL_OWED)) {
                    Band::Accept => effects.push(Effect::Amended { ordinal }),
                    Band::Reject => effects.push(Effect::Close { kind: ResolutionKind::Withdrawn, ordinal }),
                    Band::Gray => return None,
                }
            }
            if yes(WP_ASKS_AGAIN) {
                effects.push(Effect::AskedAgain { ordinal });
            }
            if yes(WP_CHANGES_DUE) {
                effects.push(Effect::NewDate { source: DateSource::WaitingParty, ordinal, probability: p(WP_CHANGES_DUE) });
            }
        }
        AuthorRole::User => {
            for (id, kind) in [(USER_DELIVERS, SuggestedUpdateKind::Closure),
                               (USER_DECLINES, SuggestedUpdateKind::Declined),
                               (USER_HANDS_OFF, SuggestedUpdateKind::Delegated)] {
                if yes(id) {
                    effects.push(Effect::Suggest { kind, ordinal, probability: p(id) });
                }
            }
            if yes(USER_PROMISES) {
                effects.push(Effect::NewDate { source: DateSource::User, ordinal, probability: p(USER_PROMISES) });
            }
        }
        AuthorRole::ThirdParty => {
            if yes(THIRD_PARTY_DONE) {
                effects.push(Effect::Suggest { kind: SuggestedUpdateKind::Closure, ordinal, probability: p(THIRD_PARTY_DONE) });
            }
        }
    }
    Some(effects)
}

/// Follow-up for a date change: `{ask, later: {paragraph_text}}`.
pub(super) fn date_state(ask: &str, paragraph: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "later": {"paragraph_text": paragraph}})
}

/// `candidates` are `(handle, normalized value)` pairs.
pub(super) fn date_questions(candidates: &[(String, String)]) -> Result<Questions, ProviderError> {
    let registered = Registry::get().question(NEW_DATE).ok_or(ProviderError::InvalidQuestion)?;
    let none = registered.options.iter().find(|(key, _)| key == NONE_OPTION)
        .map_or("The paragraph sets no new date for the request.", |(_, text)| text.as_str());
    let mut options: Vec<(&str, &str)> = candidates.iter().map(|(h, v)| (h.as_str(), v.as_str())).collect();
    options.push((NONE_OPTION, none));
    Questions::new().choice(NEW_DATE, &registered.instructions, &options)
}

/// The chosen normalized date, or `None` (escalate) for `none`, gray or low confidence.
pub(super) fn date_pick(answers: &Answers, candidates: &[(String, String)]) -> Option<String> {
    let Some(Answer::Choice { choice, confidence, .. }) = answers.get(NEW_DATE) else {
        return None;
    };
    if choice == NONE_OPTION || band(NEW_DATE, *confidence) != Band::Accept {
        return None;
    }
    candidates.iter().find(|(handle, _)| handle == choice).map(|(_, value)| value.clone())
}

/// One event an ask may serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EventCandidate {
    pub handle: String,
    pub name: String,
    pub description: String,
    pub end: i64,
    pub message_handle: String,
    pub from_subject: bool,
}

pub(super) fn expiry_state(ask: &str, waiting_party: &str, due: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "waiting_party": waiting_party, "due": due})
}

/// The event choice, with candidates in order or reversed; `none` is always last.
pub(super) fn expiry_event_questions(candidates: &[EventCandidate], reversed: bool) -> Result<Questions, ProviderError> {
    let registered = Registry::get().question(EXPIRY_EVENT).ok_or(ProviderError::InvalidQuestion)?;
    let none = registered.options.iter().find(|(key, _)| key == NONE_OPTION)
        .map_or("The request serves no listed event.", |(_, text)| text.as_str());
    let mut options: Vec<(&str, &str)> = candidates.iter()
        .map(|c| (c.handle.as_str(), c.description.as_str())).collect();
    if reversed {
        options.reverse();
    }
    options.push((NONE_OPTION, none));
    Questions::new().choice(EXPIRY_EVENT, &registered.instructions, &options)
}

/// Both orders must pick the same non-`none` event at accept confidence.
pub(super) fn expiry_pick<'a>(forward: &Answers, reversed: &Answers, candidates: &'a [EventCandidate]) -> Option<&'a EventCandidate> {
    let pick = |answers: &Answers| match answers.get(EXPIRY_EVENT) {
        Some(Answer::Choice { choice, confidence, .. })
            if choice != NONE_OPTION && band(EXPIRY_EVENT, *confidence) == Band::Accept => Some(choice.clone()),
        _ => None,
    };
    let (a, b) = (pick(forward)?, pick(reversed)?);
    (a == b).then(|| candidates.iter().find(|c| c.handle == a)).flatten()
}

pub(super) fn ends_state(ask: &str, waiting_party: &str, event: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "waiting_party": waiting_party, "event": event})
}

pub(super) fn ends_questions() -> Result<Questions, ProviderError> {
    Questions::from_registry(&[EXPIRY_ENDS])
}

/// Expired: the ask ends with the event and the event is over.
pub(super) fn expired(ends: &Answers, event_end: i64, now: i64) -> bool {
    matches!(ends.get(EXPIRY_ENDS), Some(Answer::Noul { probability }) if band(EXPIRY_ENDS, *probability) == Band::Accept)
        && event_end < now
}
```

  Check `ProviderError`'s import path against `review_scan.rs` (`openloops_inference::provider::ProviderError`). If `Questions::new()` is not `pub`, use the builder entry the code uses for dynamic choices (see `extraction_questions`).
- [ ] **Step 4:** Add the expiry tests to the same test module, then run the gate:

```rust
const MODEL: &str = "typesafe/jev-1.13";

fn candidates() -> Vec<EventCandidate> {
    ["e0", "e1"].iter().map(|handle| EventCandidate {
        handle: (*handle).to_string(), name: format!("event {handle}"),
        description: format!("event {handle}, ending Oct 30, 2026 23:59"),
        end: 1_800_000_000, message_handle: "m0".into(), from_subject: false,
    }).collect()
}

fn choice_answers(questions: &Questions, choice: &str, confidence: f64) -> Answers {
    let mut probabilities = serde_json::Map::new();
    for key in ["e0", "e1", NONE_OPTION] {
        probabilities.insert(key.into(), serde_json::Value::from(if key == choice { confidence } else { 0.0 }));
    }
    let body = serde_json::json!({
        "model": MODEL,
        "answers": {EXPIRY_EVENT: {"type": "choice", "choice": choice,
                                   "probabilities": probabilities, "confidence": confidence}},
        "usage": {"input_tokens": 1}
    });
    openloops_inference::decision::parse_answers(&serde_json::to_vec(&body).unwrap(), MODEL, questions).unwrap()
}

fn ends_answers(probability: f64) -> Answers {
    let body = serde_json::json!({
        "model": MODEL,
        "answers": {EXPIRY_ENDS: {"type": "noul", "noul": probability}},
        "usage": {"input_tokens": 1}
    });
    openloops_inference::decision::parse_answers(&serde_json::to_vec(&body).unwrap(), MODEL, &ends_questions().unwrap()).unwrap()
}

#[test]
fn expiry_needs_both_orders_to_agree() {
    let events = candidates();
    let forward_q = expiry_event_questions(&events, false).unwrap();
    let reversed_q = expiry_event_questions(&events, true).unwrap();
    let yes = accept(EXPIRY_EVENT);
    let picked = expiry_pick(&choice_answers(&forward_q, "e0", yes), &choice_answers(&reversed_q, "e0", yes), &events);
    assert_eq!(picked.map(|c| c.handle.as_str()), Some("e0"));
    assert!(expiry_pick(&choice_answers(&forward_q, "e0", yes), &choice_answers(&reversed_q, "e1", yes), &events).is_none());
    assert!(expiry_pick(&choice_answers(&forward_q, NONE_OPTION, yes), &choice_answers(&reversed_q, NONE_OPTION, yes), &events).is_none());
    assert!(expiry_pick(&choice_answers(&forward_q, "e0", gray(EXPIRY_EVENT)), &choice_answers(&reversed_q, "e0", yes), &events).is_none());
}

#[test]
fn due_and_expires_table() {
    let end = 1_800_000_000;
    assert!(expired(&ends_answers(accept(EXPIRY_ENDS)), end, end + 1));
    assert!(!expired(&ends_answers(accept(EXPIRY_ENDS)), end, end));
    assert!(!expired(&ends_answers(no(EXPIRY_ENDS)), end, end + 1));
    assert!(!expired(&ends_answers(gray(EXPIRY_ENDS)), end, end + 1));
}

#[test]
fn reversed_event_questions_keep_none_last() {
    let events = candidates();
    let body = openloops_inference::decision::request_body(MODEL, &serde_json::json!({}),
        &expiry_event_questions(&events, true).unwrap(), false).unwrap();
    let text = String::from_utf8(body).unwrap();
    let (e1, e0, none) = (text.find("\"e1\"").unwrap(), text.find("\"e0\"").unwrap(), text.find("\"none\"").unwrap());
    assert!(e1 < e0 && e0 < none);
}
```

  If `parse_answers` or `request_body` is not reachable from the desktop crate, call them the way the `FixedDecisionClient` impl in `review_scan.rs` tests does.
- [ ] **Step 5:** Commit: `feat(review): pure closure-walk decisions (states, questions, verdicts, expiry)`.

### Task C3: walk orchestration

**Files:** Modify `review_scan.rs`: `scan_decision_closures` (about line 6253), `scan_closures_selected` (about 6318) and its call in `scan` (about 3339). Remove `DecisionOutcome`, `DecisionPair`, `DecisionParagraphJob`, `DecisionProbabilities`, `DecisionChoice`, `DecisionJobResult`, `decision_pairs`, `decision_paragraph_jobs`, `closure_questions`, `closure_state`, `decide_paragraph`, `normalized_deadline`, `accepted_update`, `choice_update`, `pair_has_escalation`, `recombine_decisions` and `escalated_reach`, with their tests. Keep `offered_loops`, `loops_by_conversation`, `closure_candidates`, `closure_jobs`, `closure_request`, `merge_closure_outcomes`, `apply_updates` and `outranks`.

Behaviour (spec, "How the walk runs"):

1. `scan_closures_selected` gains `now: i64`. `scan` passes `chrono::Utc::now().timestamp()`. The test helper `run_decision_closures` passes a fixed `now`.
2. For each offered loop, build its ordered later messages:
   - same conversation: every message with `timestamp > source.input.timestamp`, any sender;
   - other conversations reached through `loops_by_conversation`: only messages returned by `closure_candidates` (the user's later messages to the waiting party);
   - with a `ClosureScope`, keep only messages `scope.admits(offered, later)` accepts;
   - sort by `(timestamp, handle)`.
3. Author role for a later message:
   - `later.input.from_user` gives `User`;
   - else, when the sender's `waiting_party_address` equals `waiting_party_address(&item.waiting_party)`, `WaitingParty`;
   - else `ThirdParty`.
   - The sender label is `later.input.message.sender.as_ref().map(CanonicalBlock::as_string)`. With no waiting-party address, every non-user author is `ThirdParty`.
4. `WalkContext`:
   - `user_name` = first non-empty of `own_given_name`, `own_display_name`, else `"the user"`;
   - `waiting_party` = `participant_display_name(&item.waiting_party)` or the label itself;
   - `ask` = `item.evidence.context`, plus amendments (adjustment 5);
   - `due` = `item.deadline.as_ref().map_or("not stated", |d| d.quote.as_str())`.
5. **Expiry setup** (before the walk, two rounds):
   - Candidates:
     - events from `build_event_index_with_rules(messages, None)` in the loop's conversation with `end > source.input.timestamp`;
     - dates from `prose_event_time_candidates(&item.evidence.context, source.input.timestamp, offset)` with `end > source.input.timestamp`, named `format!("the event on {}", local date "%b %d, %Y")`;
     - deduplicated by `end`, at most 8, handles `e0..`;
     - each `description` is `format!("{name}, ending {local date-time %b %d, %Y %H:%M}")`.
   - Round E1: two requests per loop with candidates (forward and reversed).
   - Round E2: one `expiry.ends_with_event` request per loop with a picked event, `event` = the description.
6. **Walk rounds.** While any loop is walking:
   - one round over the next later message of each walking loop;
   - one job per paragraph (first `MAX_CLOSURE_PARAGRAPHS` body blocks), with `walk_state` and `walk_questions(role)`;
   - every job error except `Cancelled` becomes a failed slot, as today, so a rate limit never stops the pass.
   - Per loop:
     - any failed paragraph: the loop is skipped (counted, stops walking, no escalation);
     - otherwise `step_verdict`:
       - `NoResponse`: next message;
       - `Escalate`: record `(later.account, later.conversation) -> offered index` and stop walking;
       - `Apply(effects)`: apply in order (below). Effects needing a date queue a follow-up.
   - Follow-up round:
     - for each queued date effect, build candidates with `extraction_temporal_candidates(&later.input, ordinal)` as `(handle, value)`;
     - no candidates: a `User` source does nothing (adjustment 4) and a `WaitingParty` source escalates;
     - otherwise send `date_questions` with `date_state`; `date_pick` gives a value, or escalate.
7. **Applying effects** (`ordinal` is the later message's body block):
   - `Close { kind }`: set `item.resolution = Some(Anchor { message: later.input.handle, block: ordinal, quote: text, context: text })`, `item.resolution_kind = Some(kind)`, `item.cross_thread = later.conversation != source.conversation`, and `item.suggested_update = None`. Stop walking.
   - `Suggest { kind, probability }`: set `item.suggested_update = Some(SuggestedUpdate { kind, evidence_text: text, source_message: later.input.handle, source_block: ordinal, temporal_value: None, confidence_micros: micros(probability) })`. A later suggestion replaces an earlier one. Count it in `result.suggested_updates` when the slot was empty. Keep walking.
   - `AskedAgain`: push `Mention { origin: Restated, message, block, quote: text, timestamp: later.input.timestamp }`. A pending `Closure` suggestion whose source is a user message is cleared (chaser after delivery).
   - `Amended`: push `Mention { origin: Amended, .. }` and append `"\n\nLater change: " + text` to the loop's `ask`.
   - `NewDate { source: WaitingParty }` with a picked value:
     - push the old deadline (if any) as `Mention { origin: Rescheduled, message: old.message, block: old.block, quote: old.quote, timestamp: later.input.timestamp }`;
     - set `item.deadline = Some(Anchor { message: later.input.handle, block: ordinal, quote: value, context: text })`, `deadline_kind_hint = None`, `unverified_deadline = false`;
     - update the context's `due`.
   - `NewDate { source: User }` with a picked value: a `DeadlineChange` suggestion with `temporal_value: Some(value)`.
8. **Time step**, after the walk, for loops not closed: when E2 ran and `expired(ends, event.end, now)`, set `item.event_passed = Some(EventPassed { name, end, message_handle, from_subject, expired: true })` and add 1 to `result.event_closures`.
9. **Escalation:** unless cancelled, run `closure_jobs(reach, messages)` and the existing chat call exactly as today (`chat_pass.restart()`, `run_jobs`, `closure_request`, `merge_closure_outcomes`).
10. **Progress:**
    - add each round's job count to `progress.total` before the round;
    - keep `progress.closure_phase = true`;
    - set `conversation_total` to the running total.
11. **Coverage note** (replaces the old decision line):
    - `"Closure walk: {closed} closed by the waiting party, {withdrawn} withdrawn, {expired} expired, {asked} asked again, {changed} changed, {rescheduled} new due dates, {suggested} suggested updates, {escalated} loops escalated to the chat model, {skipped} skipped (rate limit / errors)."`
    - keep the "Escalated update checks were capped" line.

- [ ] **Step 1: Port and write the failing tests** in the `review_scan.rs` test module.
  - `FixedDecisionClient`'s `decide` uses `DecisionOutcome::ALL` and the `closure.outcome` option list. Remove those closure branches together with `DecisionOutcome`. Keep the triage branch working.
  - Extend `FixedDecisionClient` with a constructor `walk(answer: impl Fn(usize, &serde_json::Value, &str) -> Option<f64>)`. It returns a probability per question id (`None` means "use `no`"), plus a `choice` hook for `walk.new_date` and `expiry.event`.
  - Use `registry_accept` and `registry_gray` for probabilities.
  - Tests:
    - `walk_closes_a_loop_when_the_waiting_party_confirms_receipt`: the waiting-party reply in the same thread gives `resolution_kind == Some(Completed)` and a resolution anchor on that message.
    - `walk_suggests_handled_when_the_user_delivers_and_keeps_walking`: the user reply suggests `Closure`, then a waiting-party chaser clears it and adds a `Restated` mention.
    - `walk_carries_an_amendment_into_later_states`: the second message's recorded state `ask` contains `"Later change: "`.
    - `walk_applies_a_waiting_party_date_change_and_keeps_the_old_deadline_as_a_mention`.
    - `walk_turns_a_user_promise_with_a_date_into_a_deadline_suggestion_and_ignores_one_without`.
    - `walk_escalates_a_gray_gate_to_the_chat_closure_call_for_that_conversation` (port of `gray_band_escalates_only_that_conversation_and_loop_to_chat`).
    - `walk_counts_a_rate_limited_loop_as_skipped_and_does_not_escalate_it` (port of `decision_rate_limit_is_counted_once…`).
    - `walk_cancel_before_dispatch_sends_nothing` (port).
    - `walk_requests_only_the_first_eight_body_paragraphs` (port).
    - `walk_cross_thread_reads_only_the_users_later_messages_to_the_waiting_party` (with `cross_thread_fixture`).
    - `walk_expires_a_prepare_for_meeting_ask_after_the_meeting`: ask "Please prepare the slides for the board meeting on 9/12.", deadline "9/12", now after 9/12, both event choices agree, ends accepted: `event_passed.expired == true`.
    - `walk_keeps_an_rsvp_overdue_until_the_event_ends`: ask "Please RSVP by Friday for the gala on 10/30.", deadline Friday, now between Friday and 10/30. No expiry, so the card stays open. With now after 10/30: expired.
    - `walk_keeps_pay_by_date_overdue_when_ends_is_rejected`.
    - `walk_ignores_disagreeing_event_choices`.
    - `toggle_off_runs_the_unchanged_chat_pass` (keep as is).
    - `walk_state_bytes_match_the_harness_shape`: assert the exact state string of the first request.
- [ ] **Step 2:** Run the desktop tests. Expected: FAIL.
- [ ] **Step 3:** Implement the behaviour above in `review_scan.rs`, using the `closure_walk` functions. Keep the job-runner pattern (`ParallelPass::new(decision_client.max_parallel())`, `run_jobs`, `JobOutcome`). Remove the old decision-closure code and its tests listed above.
- [ ] **Step 4:** Run the gate. Expected: all pass. `grep -n "closure.fulfilled\|closure.outcome" crates/openloops-desktop/src/review_scan.rs` returns nothing.
- [ ] **Step 5:** Commit: `feat(review): closure walk replaces the flat decision-model closure batch`.

### Task C4: training export `walk.jsonl`

**Files:** Modify `training_export.rs`.

- [ ] **Step 1: Write the failing test.** An accepted `Closure` suggestion exports a `walk.jsonl` row:
  - `set: "walk"`;
  - `label: {"walk.user_delivers": true, "walk.user_promises": false, "walk.user_declines": false, "walk.user_hands_off": false}`;
  - `label_source: "gold"`;
  - `later.author_role: "user"`;
  - state keys exactly `user`, `waiting_party`, `ask`, `due`, `later`, plus `id`, `group`, `label`, `label_source`, `set`, `source`.

  A rejected suggestion gives gold all-false. Mirror `a_rejected_suggested_update_yields_gold_all_false`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `walk_rows` uses the same candidates as `closure_rows` (`later_candidates`, user-sent paragraphs, the same size caps).
  - The state matches `closure_walk::walk_state`, with `user.name` from the message's own display or given name, the waiting-party display name, `ask = item.evidence.context` and `due` as in Task C3.
  - Label map:

    | Suggestion kind | Label set to true |
    |---|---|
    | `Closure` | `walk.user_delivers` |
    | `DeadlineChange` | `walk.user_promises` |
    | `Declined` | `walk.user_declines` |
    | `Delegated` | `walk.user_hands_off` |

  - The gold and silver rules match `closure_label`.
  - `group` = the loop's evidence `stable_id`.
  - Write `walk.jsonl` beside `closure.jsonl`. Update the module doc comment and `docs/inbox-review.md`'s file list.
- [ ] **Step 4:** Run the gate. Expected: pass.
- [ ] **Step 5:** Commit: `feat(review): export walk-shaped gold rows for the user branch`.

### Task C5: docs

**Files:** Modify `docs/inbox-review.md` (the closure-pass paragraph under "Coverage and interpretation") and `docs/openrouter.md` (closure prose near lines 105-114).

- [ ] **Step 1: Describe:**
  - the walk order;
  - what closes automatically (waiting-party receipt or withdrawal, expiry) and what is a suggestion (your reply, a decline, a hand-off, a third party);
  - the "Asked again", "Changed" and "Due date changed" pills;
  - "Expired with …";
  - escalation to the chat model;
  - the coverage line.

  For transmission, list the walk state fields: user first name, waiting party name, the ask paragraph, the due text, the author role and one later paragraph; the date candidates; and the expiry state.
- [ ] **Step 2:** Run `pwsh ./tools/check-governance.ps1` and `pwsh ./tools/check-model-boundary.ps1`. Expected: pass.
- [ ] **Step 3:** Commit: `docs: closure walk behaviour and transmitted fields`.

### Task C6: final verification

- [ ] **Step 1:** Run the Part C gate, plus:

```powershell
cargo test -p openloops-inference --features ollama-cloud,openrouter --locked
pwsh ./tools/check-model-boundary.ps1
pwsh ./tools/test-model-boundary.ps1
```

Run every `tools/check-*.ps1` and compare with the baseline list. Only the eight baseline failures may fail.
- [ ] **Step 2:** Run `git log --oneline main..HEAD` and `git diff --stat main...HEAD`. Confirm no file outside this plan's file map changed, apart from test fixtures in the same files.
- [ ] **Step 3:** Run `pwsh ./tools/check-public-repo.ps1 -Mode History` before any push.

## What the owner runs after merge readiness (paid, not part of this plan)

From the spec's cost gates, in order, each with a smoke run first:

1. `uv run jev-optimize generate-threads --llm --smoke` and inspect the output, then the full run.
2. `uv run jev-optimize optimize --set walk --only walk.responds --budget light` and inspect it, then all walk and expiry questions.
3. `uv run jev-optimize evaluate-walk --threads data/synthetic/threads.jsonl`: the thread-level share that decides go or no-go.
4. Build Part C, export training data, then compare `evaluate --set walk --data <export>/walk.jsonl` with `evaluate --set closure --data <export>/closure.jsonl` on the same gold rows. Part C merges only if the walk agrees with more of the owner's decisions.
