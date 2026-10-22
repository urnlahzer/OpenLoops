"""Bootstrap DSPy signatures and their Jev registry metadata."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import dspy
from dspy.experimental import Choice, Noul


@dataclass(frozen=True)
class QuestionSpec:
    set_name: str
    signature: type[dspy.Signature]
    intent: str
    primary_fields: tuple[str, ...]
    allowed_fields: tuple[str, ...]
    kind: str = "noul"
    options: dict[str, str] | None = None


class ClosureFulfilled(dspy.Signature):
    """The later text shows the obligation has been carried out."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the obligation is fulfilled.")


class ClosureWithdrawn(dspy.Signature):
    """The later text cancels or withdraws the obligation."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the obligation is withdrawn.")


class ClosureDeadlineChanged(dspy.Signature):
    """The later text sets a different deadline for the obligation."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the deadline changed.")


class ClosureModified(dspy.Signature):
    """The later text changes what the obligation requires."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the obligation was modified.")


_CLOSURE_OUTCOME_OPTIONS = {
    "fulfilled": "The later paragraph says the obligation was carried out.",
    "withdrawn": "The later paragraph cancels or withdraws the obligation.",
    "deadline_changed": "The later paragraph sets a different deadline for the obligation.",
    "modified": "The later paragraph changes what the obligation requires.",
    "none": "The later paragraph expresses none of the listed outcomes for the obligation.",
}
ClosureOutcomeDecision = Choice[tuple(_CLOSURE_OUTCOME_OPTIONS.items())]


class ClosureOutcome(dspy.Signature):
    """Which single outcome, if any, does the later paragraph express for the obligation?"""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: ClosureOutcomeDecision = dspy.OutputField(
        desc="The single expressed obligation outcome, or none."
    )


class TriageBase(dspy.Signature):
    subject: str = dspy.InputField()
    paragraph_text: str = dspy.InputField()
    from_user: bool = dspy.InputField()


class TriageAsksRecipient(TriageBase):
    """The paragraph asks its recipient to do something."""

    decision: Noul = dspy.OutputField(desc="Probability of a request to the recipient.")


class TriageCommitsSender(TriageBase):
    """The paragraph commits its sender to doing something."""

    decision: Noul = dspy.OutputField(desc="Probability of a commitment by the sender.")


class TriageAsksQuestion(TriageBase):
    """The paragraph asks a genuine question that seeks an answer."""

    decision: Noul = dspy.OutputField(desc="Probability of a genuine question.")


class TriageNamesTime(TriageBase):
    """The paragraph names a time, date, deadline, or event-relative time."""

    decision: Noul = dspy.OutputField(desc="Probability that a time is named.")


class TriageBoilerplate(TriageBase):
    """The paragraph is a signature, legal footer, unsubscribe notice, or disclaimer."""

    decision: Noul = dspy.OutputField(desc="Probability that the paragraph is boilerplate.")


class TriageAutomatedNotification(TriageBase):
    """The paragraph is an automatically generated notification."""

    decision: Noul = dspy.OutputField(desc="Probability of an automated notification.")


_EXTRACT_CLAIM_TYPE_OPTIONS = {
    "request": "The paragraph asks its recipient to do something.",
    "promise": "The paragraph commits its sender to doing something.",
    "question": "The paragraph asks a genuine question that seeks an answer.",
    "attribution": "The paragraph attributes an obligation or commitment to another party.",
    "delegation": "The paragraph delegates an obligation from one party to another.",
    "none": "The paragraph expresses none of the listed claim types.",
}
ExtractClaimTypeDecision = Choice[tuple(_EXTRACT_CLAIM_TYPE_OPTIONS.items())]


class ExtractClaimType(TriageBase):
    """Which single claim type, if any, does the paragraph express?"""

    decision: ExtractClaimTypeDecision = dspy.OutputField(
        desc="The single expressed claim type, or none."
    )


# `extract.waiting_party` and `extract.temporal` (P5) are dynamic-option
# choice questions: the app issues a fresh option set per request (the
# conversation's own participant handles, or a paragraph's own normalized
# date candidates), not a fixed vocabulary this harness can enumerate.
# Their `state` shape (documented in full in
# `docs/plans/2026-09-19-jev-decision-model.md`'s P5 section, and mirrored
# by `crates/openloops-desktop/src/review_scan.rs`'s `extraction_state`) is
# `{subject, paragraph_text, from_user, to_user, cc_user, user:
# {display_name, given_name}, participants: [{handle, text}...]}`. The
# `options` below are bootstrap placeholders (`none` only) so the registry
# entry is well-formed; tuning their *instructions* wording can proceed on
# this bootstrap text, but this harness has no labeled corpus for either
# question yet -- see the design doc's P5 section for what a corpus would
# need.
_EXTRACT_WAITING_PARTY_OPTIONS = {
    "none": "No participant is waiting on the signed-in user for this paragraph."
}
ExtractWaitingPartyDecision = Choice[tuple(_EXTRACT_WAITING_PARTY_OPTIONS.items())]


class ExtractWaitingParty(TriageBase):
    """Which participant is waiting on the signed-in user for this paragraph?"""

    participants: list[dict[str, str]] = dspy.InputField()
    decision: ExtractWaitingPartyDecision = dspy.OutputField(
        desc="The waiting participant's issued handle, or none."
    )


_EXTRACT_TEMPORAL_OPTIONS = {
    "none": "No temporal candidate applies to this paragraph."
}
ExtractTemporalDecision = Choice[tuple(_EXTRACT_TEMPORAL_OPTIONS.items())]


class ExtractTemporal(TriageBase):
    """Which named time, date, or deadline candidate, if any, does this paragraph express?"""

    decision: ExtractTemporalDecision = dspy.OutputField(
        desc="The chosen normalized temporal candidate's issued handle, or none."
    )


def _rule_signature(name: str, instructions: str, fields: tuple[str, ...]) -> type[dspy.Signature]:
    annotations = {field: str for field in fields} | {"decision": Noul}
    namespace = {"__annotations__": annotations, "__doc__": instructions}
    namespace.update({field: dspy.InputField() for field in fields})
    namespace["decision"] = dspy.OutputField(desc=f"Probability that {instructions.lower()}")
    return type(name, (dspy.Signature,), namespace)


RulesRecap = _rule_signature(
    "RulesRecap",
    "This email is an automatically generated meeting summary, recap, or transcript.",
    ("sender", "subject", "first_paragraph"),
)
RulesScopedEvent = _rule_signature(
    "RulesScopedEvent",
    "This request is about attending, preparing for, or bringing something to a meeting or event.",
    ("request_text",),
)
RulesEventMatch = _rule_signature(
    "RulesEventMatch", "The phrase refers to the named event.", ("phrase", "event_name")
)
RulesDuplicateAction = _rule_signature(
    "RulesDuplicateAction", "These two sentences ask for the same thing.", ("action_a", "action_b")
)
RulesThreadMerge = _rule_signature(
    "RulesThreadMerge",
    "These two messages belong to the same conversation topic.",
    ("subject_a", "subject_b", "first_paragraph_a", "first_paragraph_b"),
)


_RULES_DEADLINE_KIND_OPTIONS = {
    "event_tied": "The deadline is tied to a named event.",
    "soft": "The timing is flexible or aspirational.",
    "unknown": "The phrase has no supported deadline classification.",
}
RulesDeadlineKindDecision = Choice[tuple(_RULES_DEADLINE_KIND_OPTIONS.items())]


class RulesDeadlineKind(dspy.Signature):
    """Classify how the phrase expresses a deadline."""

    phrase: str = dspy.InputField()
    decision: RulesDeadlineKindDecision = dspy.OutputField(
        desc="event_tied: tied to an event; soft: flexible timing; unknown: neither classification."
    )


class WalkBase(dspy.Signature):
    user: dict[str, str] = dspy.InputField()
    waiting_party: str = dspy.InputField()
    ask: str = dspy.InputField()
    due: str = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()


def _walk_signature(name: str, instructions: str) -> type[dspy.Signature]:
    return type(
        name,
        (WalkBase,),
        {
            "__doc__": instructions,
            "__annotations__": {"decision": Noul},
            "decision": dspy.OutputField(
                desc="Probability that the statement is true."
            ),
        },
    )


_WALK_STATEMENTS = {
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

_WALK_INTENTS = {
    "walk.responds": "`later.text` is about the request.",
    "walk.wp_received": "In `later.text`, the waiting party confirms receipt.",
    "walk.wp_no_longer_needed": "In `later.text`, the waiting party withdraws the request.",
    "walk.wp_asks_again": "In `later.text`, the waiting party chases the request.",
    "walk.wp_changes_terms": (
        "In `later.text`, the waiting party changes what the request requires."
    ),
    "walk.wp_still_owed": "After `later.text`, the user still owes an action for the request.",
    "walk.wp_changes_due": "In `later.text`, the waiting party changes the due time.",
    "walk.user_delivers": "In `later.text`, the user delivers what was requested.",
    "walk.user_promises": "In `later.text`, the user promises to do it later.",
    "walk.user_declines": "In `later.text`, the user declines the request.",
    "walk.user_hands_off": "In `later.text`, the user hands the request to another person.",
    "walk.third_party_done": "In `later.text`, another participant did or will do the request.",
}

_WALK_FIELDS = (
    "user.name",
    "waiting_party",
    "ask",
    "due",
    "later.author_role",
    "later.text",
)


_WALK_NEW_DATE_OPTIONS = {"none": "The text sets no new date for the request."}
WalkNewDateDecision = Choice[tuple(_WALK_NEW_DATE_OPTIONS.items())]


class WalkNewDate(dspy.Signature):
    """Which date does `later.text` set as the new time for `ask`?"""

    ask: str = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    decision: WalkNewDateDecision = dspy.OutputField(desc="The issued date handle, or none.")


_WALK_HANDOFF_OPTIONS = {"none": "The text names no listed person."}
WalkHandoffToDecision = Choice[tuple(_WALK_HANDOFF_OPTIONS.items())]


class WalkHandoffTo(WalkBase):
    """Which person does `later.text` name as the one who will now do what `ask` requests?"""

    decision: WalkHandoffToDecision = dspy.OutputField(
        desc="The issued participant handle, or none."
    )


_EXPIRY_EVENT_OPTIONS = {"none": "The request serves no listed event."}
ExpiryEventDecision = Choice[tuple(_EXPIRY_EVENT_OPTIONS.items())]


class ExpiryEvent(dspy.Signature):
    """Which event does `ask` help `waiting_party` prepare for or take part in?"""

    ask: str = dspy.InputField()
    waiting_party: str = dspy.InputField()
    due: str = dspy.InputField()
    decision: ExpiryEventDecision = dspy.OutputField(desc="The issued event handle, or none.")


class ExpiryEndsWithEvent(dspy.Signature):
    """`ask` has no purpose after `event` ends."""

    ask: str = dspy.InputField()
    waiting_party: str = dspy.InputField()
    event: str = dspy.InputField()
    decision: Noul = dspy.OutputField(desc="Probability that the statement is true.")


def _walk_specs() -> dict[str, QuestionSpec]:
    return {
        **{
            qid: QuestionSpec(
                "walk",
                _walk_signature(qid.replace(".", "_").title(), text),
                _WALK_INTENTS[qid],
                ("later.text",),
                _WALK_FIELDS,
            )
            for qid, text in _WALK_STATEMENTS.items()
        },
        "walk.new_date": QuestionSpec(
            "walk",
            WalkNewDate,
            "The new date set in later.text for the request.",
            ("later.text",),
            ("ask", "later.text"),
            "choice",
            _WALK_NEW_DATE_OPTIONS,
        ),
        "walk.handoff_to": QuestionSpec(
            "walk",
            WalkHandoffTo,
            "The person named in later.text who will now do the request.",
            ("later.text",),
            _WALK_FIELDS,
            "choice",
            _WALK_HANDOFF_OPTIONS,
        ),
        "expiry.event": QuestionSpec(
            "expiry",
            ExpiryEvent,
            "The event the request serves.",
            ("ask",),
            ("ask", "waiting_party", "due"),
            "choice",
            {"none": "The request serves no listed event."},
        ),
        "expiry.ends_with_event": QuestionSpec(
            "expiry",
            ExpiryEndsWithEvent,
            "The request loses its purpose when the event ends.",
            ("ask", "event"),
            ("ask", "waiting_party", "event"),
        ),
    }


_CLOSURE_FIELDS = (
    "obligation.title",
    "obligation.evidence_text",
    "later.paragraph_text",
    "later.from_user",
    "later.days_later",
)
_TRIAGE_FIELDS = ("subject", "paragraph_text", "from_user")


SPECS: dict[str, QuestionSpec] = {
    "closure.fulfilled": QuestionSpec(
        "closure", ClosureFulfilled,
        "The later paragraph shows the obligation was carried out.",
        ("later.paragraph_text",), _CLOSURE_FIELDS,
    ),
    "closure.withdrawn": QuestionSpec(
        "closure", ClosureWithdrawn,
        "The later paragraph cancels or withdraws the obligation.",
        ("later.paragraph_text",), _CLOSURE_FIELDS,
    ),
    "closure.deadline_changed": QuestionSpec(
        "closure", ClosureDeadlineChanged,
        "The later paragraph sets a different deadline for the obligation.",
        ("later.paragraph_text",), _CLOSURE_FIELDS,
    ),
    "closure.modified": QuestionSpec(
        "closure", ClosureModified,
        "The later paragraph changes what the obligation requires.",
        ("later.paragraph_text",), _CLOSURE_FIELDS,
    ),
    "closure.outcome": QuestionSpec(
        "closure",
        ClosureOutcome,
        "Which single outcome, if any, does the later paragraph express for the obligation",
        ("later.paragraph_text",),
        _CLOSURE_FIELDS,
        "choice",
        _CLOSURE_OUTCOME_OPTIONS,
    ),
    "triage.asks_recipient": QuestionSpec(
        "triage", TriageAsksRecipient,
        "The paragraph asks its recipient to do something.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "triage.commits_sender": QuestionSpec(
        "triage", TriageCommitsSender,
        "The paragraph commits its sender to doing something.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "triage.asks_question": QuestionSpec(
        "triage", TriageAsksQuestion,
        "The paragraph asks a genuine question that seeks an answer.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "triage.names_time": QuestionSpec(
        "triage", TriageNamesTime,
        "The paragraph names a time, date, deadline, or event-relative time.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "triage.boilerplate": QuestionSpec(
        "triage", TriageBoilerplate,
        "The paragraph is a signature, legal footer, unsubscribe notice, or disclaimer.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "triage.automated_notification": QuestionSpec(
        "triage", TriageAutomatedNotification,
        "The paragraph is an automatically generated notification.",
        ("paragraph_text",), _TRIAGE_FIELDS,
    ),
    "extract.claim_type": QuestionSpec(
        "extract",
        ExtractClaimType,
        "Which single claim type, if any, does the paragraph express",
        ("paragraph_text",),
        _TRIAGE_FIELDS,
        "choice",
        _EXTRACT_CLAIM_TYPE_OPTIONS,
    ),
    "extract.waiting_party": QuestionSpec(
        "extract",
        ExtractWaitingParty,
        "Which participant is waiting on the signed-in user for this paragraph",
        ("paragraph_text", "participants"),
        (*_TRIAGE_FIELDS, "participants"),
        "choice",
        _EXTRACT_WAITING_PARTY_OPTIONS,
    ),
    "extract.temporal": QuestionSpec(
        "extract",
        ExtractTemporal,
        "Which named time, date, or deadline candidate, if any, does this paragraph express",
        ("paragraph_text",),
        _TRIAGE_FIELDS,
        "choice",
        _EXTRACT_TEMPORAL_OPTIONS,
    ),
    "rules.recap": QuestionSpec(
        "rules", RulesRecap,
        "The email is an automatically generated meeting summary, recap, or transcript.",
        ("first_paragraph",), ("sender", "subject", "first_paragraph"),
    ),
    "rules.scoped_event": QuestionSpec(
        "rules", RulesScopedEvent,
        "The request concerns attending, preparing for, or bringing something to an event.",
        ("request_text",), ("request_text",),
    ),
    "rules.event_match": QuestionSpec(
        "rules", RulesEventMatch,
        "The phrase refers to the named event.",
        ("phrase", "event_name"), ("phrase", "event_name"),
    ),
    "rules.duplicate_action": QuestionSpec(
        "rules", RulesDuplicateAction,
        "The two action sentences ask for the same thing.",
        ("action_a", "action_b"), ("action_a", "action_b"),
    ),
    "rules.thread_merge": QuestionSpec(
        "rules", RulesThreadMerge,
        "The two messages belong to the same conversation topic.",
        ("first_paragraph_a", "first_paragraph_b"),
        ("subject_a", "subject_b", "first_paragraph_a", "first_paragraph_b"),
    ),
    "rules.deadline_kind": QuestionSpec(
        "rules",
        RulesDeadlineKind,
        "The phrase is classified by whether its timing is event-tied, soft, or unknown.",
        ("phrase",),
        ("phrase",),
        "choice",
        _RULES_DEADLINE_KIND_OPTIONS,
    ),
    **_walk_specs(),
}


SETS = {
    name: tuple(key for key, spec in SPECS.items() if spec.set_name == name)
    for name in ("closure", "triage", "rules", "extract", "walk", "expiry")
}
