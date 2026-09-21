"""Bootstrap DSPy signatures and their Jev registry metadata."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import dspy


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
    probability: float = dspy.OutputField(desc="Probability that the obligation is fulfilled.")


class ClosureWithdrawn(dspy.Signature):
    """The later text cancels or withdraws the obligation."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    probability: float = dspy.OutputField(desc="Probability that the obligation is withdrawn.")


class ClosureDeadlineChanged(dspy.Signature):
    """The later text sets a different deadline for the obligation."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    probability: float = dspy.OutputField(desc="Probability that the deadline changed.")


class ClosureModified(dspy.Signature):
    """The later text changes what the obligation requires."""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    probability: float = dspy.OutputField(desc="Probability that the obligation was modified.")


class ClosureOutcome(dspy.Signature):
    """Which single outcome, if any, does the later paragraph express for the obligation?"""

    obligation: dict[str, str] = dspy.InputField()
    later: dict[str, Any] = dspy.InputField()
    choice: str = dspy.OutputField(desc="The single expressed obligation outcome, or none.")
    probabilities: dict[str, float] = dspy.OutputField(
        desc="Probability for each issued obligation-outcome option."
    )
    confidence: float = dspy.OutputField(desc="Confidence in the selected obligation outcome.")


class TriageBase(dspy.Signature):
    subject: str = dspy.InputField()
    paragraph_text: str = dspy.InputField()
    from_user: bool = dspy.InputField()


class TriageAsksRecipient(TriageBase):
    """The paragraph asks its recipient to do something."""

    probability: float = dspy.OutputField(desc="Probability of a request to the recipient.")


class TriageCommitsSender(TriageBase):
    """The paragraph commits its sender to doing something."""

    probability: float = dspy.OutputField(desc="Probability of a commitment by the sender.")


class TriageAsksQuestion(TriageBase):
    """The paragraph asks a genuine question that seeks an answer."""

    probability: float = dspy.OutputField(desc="Probability of a genuine question.")


class TriageNamesTime(TriageBase):
    """The paragraph names a time, date, deadline, or event-relative time."""

    probability: float = dspy.OutputField(desc="Probability that a time is named.")


class TriageBoilerplate(TriageBase):
    """The paragraph is a signature, legal footer, unsubscribe notice, or disclaimer."""

    probability: float = dspy.OutputField(desc="Probability that the paragraph is boilerplate.")


class TriageAutomatedNotification(TriageBase):
    """The paragraph is an automatically generated notification."""

    probability: float = dspy.OutputField(desc="Probability of an automated notification.")


class ExtractClaimType(TriageBase):
    """Which single claim type, if any, does the paragraph express?"""

    choice: str = dspy.OutputField(desc="The single expressed claim type, or none.")
    probabilities: dict[str, float] = dspy.OutputField(
        desc="Probability for each issued claim-type option."
    )
    confidence: float = dspy.OutputField(desc="Confidence in the selected claim type.")


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
class ExtractWaitingParty(TriageBase):
    """Which participant is waiting on the signed-in user for this paragraph?"""

    participants: list[dict[str, str]] = dspy.InputField()
    choice: str = dspy.OutputField(desc="The waiting participant's issued handle, or none.")
    probabilities: dict[str, float] = dspy.OutputField(
        desc="Probability for each issued participant-handle option, plus none."
    )
    confidence: float = dspy.OutputField(desc="Confidence in the selected waiting party.")


class ExtractTemporal(TriageBase):
    """Which named time, date, or deadline candidate, if any, does this paragraph express?"""

    choice: str = dspy.OutputField(
        desc="The chosen normalized temporal candidate's issued handle, or none."
    )
    probabilities: dict[str, float] = dspy.OutputField(
        desc="Probability for each issued temporal-candidate option, plus none."
    )
    confidence: float = dspy.OutputField(desc="Confidence in the selected temporal candidate.")


def _rule_signature(name: str, instructions: str, fields: tuple[str, ...]) -> type[dspy.Signature]:
    annotations = {field: str for field in fields} | {"probability": float}
    namespace = {"__annotations__": annotations, "__doc__": instructions}
    namespace.update({field: dspy.InputField() for field in fields})
    namespace["probability"] = dspy.OutputField(desc=f"Probability that {instructions.lower()}")
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


class RulesDeadlineKind(dspy.Signature):
    """Classify how the phrase expresses a deadline."""

    phrase: str = dspy.InputField()
    choice: str = dspy.OutputField(
        desc="event_tied: tied to an event; soft: flexible timing; unknown: neither classification."
    )
    probabilities: dict[str, float] = dspy.OutputField(
        desc="Probability for each issued deadline-kind option."
    )
    confidence: float = dspy.OutputField(desc="Confidence in the selected deadline kind.")


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
        {
            "fulfilled": "The later paragraph says the obligation was carried out.",
            "withdrawn": "The later paragraph cancels or withdraws the obligation.",
            "deadline_changed": "The later paragraph sets a different deadline for the obligation.",
            "modified": "The later paragraph changes what the obligation requires.",
            "none": "The later paragraph expresses none of the listed outcomes for the obligation.",
        },
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
        {
            "request": "The paragraph asks its recipient to do something.",
            "promise": "The paragraph commits its sender to doing something.",
            "question": "The paragraph asks a genuine question that seeks an answer.",
            "attribution": "The paragraph attributes an obligation or commitment to another party.",
            "delegation": "The paragraph delegates an obligation from one party to another.",
            "none": "The paragraph expresses none of the listed claim types.",
        },
    ),
    "extract.waiting_party": QuestionSpec(
        "extract",
        ExtractWaitingParty,
        "Which participant is waiting on the signed-in user for this paragraph",
        ("paragraph_text", "participants"),
        (*_TRIAGE_FIELDS, "participants"),
        "choice",
        {"none": "No participant is waiting on the signed-in user for this paragraph."},
    ),
    "extract.temporal": QuestionSpec(
        "extract",
        ExtractTemporal,
        "Which named time, date, or deadline candidate, if any, does this paragraph express",
        ("paragraph_text",),
        _TRIAGE_FIELDS,
        "choice",
        {"none": "No temporal candidate applies to this paragraph."},
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
        {
            "event_tied": "The deadline is tied to a named event.",
            "soft": "The timing is flexible or aspirational.",
            "unknown": "The phrase has no supported deadline classification.",
        },
    ),
}


SETS = {
    name: tuple(key for key, spec in SPECS.items() if spec.set_name == name)
    for name in ("closure", "triage", "rules", "extract")
}


def jev_question(question_id: str) -> dict[str, Any]:
    spec = SPECS[question_id]
    question: dict[str, Any] = {
        "type": spec.kind,
        "instructions": (spec.signature.__doc__ or "").strip(),
    }
    if spec.options:
        question["criteria"] = spec.options
    return question
