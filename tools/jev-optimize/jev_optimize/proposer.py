"""Jev-specific, constrained instruction proposals for DSPy GEPA."""

from __future__ import annotations

import re
from collections.abc import Iterable, Mapping
from typing import Any

import dspy

MAX_WORDS = 45
BANNED_PHRASES = (
    "output",
    "probability in",
    "you are",
    "assistant",
    "rules",
    "for example",
    "e.g.",
    "such as",
    "act as",
    "expert",
    "system prompt",
    "return only",
    "respond with",
    "input format",
)
_WORD_RE = re.compile(r"\b[\w.-]+\b")
_CAPITALIZED_RE = re.compile(r"\b[A-Z][A-Za-z'-]*\b")
_BULLET_RE = re.compile(r"(?:^|\s)(?:[-*\u2022]|\d+[.)])\s")
_FIELD_TOKEN_RE = re.compile(
    r"\b[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+\b"
)
_KNOWN_SIMPLE_FIELDS = {
    "action_a", "action_b", "event_name", "first_paragraph", "first_paragraph_a",
    "first_paragraph_b", "from_user", "paragraph_text", "phrase", "request_text",
    "sender", "subject", "subject_a", "subject_b",
}
_NEGATION_RE = re.compile(
    r"\b(?:not|no|never|neither|nor|without)\b(?:\W+\w+){0,3}\W+"
    r"\b(?:not|no|never|neither|nor|without)\b",
    re.IGNORECASE,
)
_COMMON_CAPITALIZED = {
    "A",
    "An",
    "Current",
    "False",
    "Feedback",
    "Fields",
    "Input",
    "Later",
    "The",
    "This",
    "True",
}


class JevInstructionProposal(dspy.Signature):
    """Write one Jev decision statement that returns the probability the statement is true.

    The statement has at most two plain declarative sentences and at most 45 words. It is
    present tense and literal. It must preserve the supplied intent, changing only wording or
    precision, and explicitly name at least one supplied primary field. It may refer only to the
    supplied allowed state fields. It contains
    no role framing, second-person language, output instructions, lists, examples, copied names
    of people, companies, projects, or documents, and no stacked negation.
    """

    current_statement: str = dspy.InputField(desc="The current literal decision statement.")
    intent: str = dspy.InputField(
        desc="The fixed meaning that the replacement must preserve exactly."
    )
    feedback: str = dspy.InputField(desc="Feedback lines from passing and failing examples.")
    state_fields: str = dspy.InputField(
        desc="Comma-separated allowed state field names; no other field may be named."
    )
    primary_fields: str = dspy.InputField(
        desc="Comma-separated fields; the statement must explicitly name at least one."
    )
    violations: str = dspy.InputField(
        desc="Validation violations from the previous attempt, or 'none'."
    )
    proposed_statement: str = dspy.OutputField(
        desc="Only the replacement decision statement, with no quotation marks or commentary."
    )


def _strings(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, Mapping):
        for item in value.values():
            yield from _strings(item)
    elif isinstance(value, Iterable) and not isinstance(value, (bytes, bytearray)):
        for item in value:
            yield from _strings(item)
    elif hasattr(value, "__dict__"):
        yield from _strings(vars(value))


def _feedback_lines(value: Any) -> list[str]:
    lines: list[str] = []
    if isinstance(value, Mapping):
        for key, item in value.items():
            if "feedback" in str(key).lower():
                lines.extend(part.strip() for text in _strings(item) for part in text.splitlines())
            else:
                lines.extend(_feedback_lines(item))
    elif isinstance(value, Iterable) and not isinstance(value, (str, bytes, bytearray)):
        for item in value:
            lines.extend(_feedback_lines(item))
    elif hasattr(value, "__dict__"):
        lines.extend(_feedback_lines(vars(value)))
    return [line for line in lines if line]


def validate_statement(
    statement: str,
    *,
    current_statement: str = "",
    example_texts: Iterable[str] = (),
    allowed_fields: Iterable[str] = (),
    primary_fields: Iterable[str] = (),
) -> list[str]:
    """Return every Jev prompt constraint violated by ``statement``."""

    violations: list[str] = []
    stripped = statement.strip()
    words = _WORD_RE.findall(stripped)
    lower = stripped.casefold()
    if not stripped:
        violations.append("statement is empty")
    if len(words) > MAX_WORDS:
        violations.append(f"statement has {len(words)} words; maximum is {MAX_WORDS}")
    if "\n" in statement or "\r" in statement or _BULLET_RE.search(statement):
        violations.append("statement contains a newline or list marker")
    if len(re.findall(r"[.!?](?:[\"']?)(?=\s|$)", stripped)) > 2:
        violations.append("statement has more than two sentences")
    if "?" in stripped or "!" in stripped:
        violations.append("statement is not declarative")
    large_numbers = [token for token in words if token.isdigit() and len(token) > 1]
    if large_numbers:
        violations.append("statement contains a digits-only token larger than a single digit")
    for phrase in BANNED_PHRASES:
        if phrase in lower:
            violations.append(f"statement contains banned phrase {phrase!r}")
    if re.search(r"\b(?:you|your|yours|yourself)\b", lower):
        violations.append("statement uses second-person language")
    if _NEGATION_RE.search(stripped):
        violations.append("statement stacks negation")

    allowed = tuple(allowed_fields)
    primary = tuple(primary_fields)
    mentions_primary = any(
        re.search(rf"(?<![\w.]){re.escape(field)}(?![\w.])", stripped)
        for field in primary
    )
    if primary and not mentions_primary:
        violations.append("statement mentions none of the required primary fields")
    named_fields = set(_FIELD_TOKEN_RE.findall(stripped)) | {
        field
        for field in _KNOWN_SIMPLE_FIELDS
        if re.search(rf"(?<![\w.]){re.escape(field)}(?![\w.])", stripped)
    }
    disallowed = sorted(named_fields - set(allowed))
    if disallowed:
        violations.append("statement mentions disallowed fields: " + ", ".join(disallowed))

    current_caps = set(_CAPITALIZED_RE.findall(current_statement))
    example_caps = {
        token
        for text in example_texts
        for token in _CAPITALIZED_RE.findall(text)
        if token not in _COMMON_CAPITALIZED
    }
    proposed_caps = set(_CAPITALIZED_RE.findall(stripped))
    leaked = sorted((proposed_caps & example_caps) - current_caps - _COMMON_CAPITALIZED)
    if leaked:
        violations.append("statement copies capitalized example tokens: " + ", ".join(leaked))
    return violations


class JevProposer:
    """GEPA ``ProposalFn`` that rejects prompts unsuitable for Jev."""

    def __init__(
        self,
        reflection_lm: Any,
        fields: Iterable[str],
        *,
        intent: str = "",
        primary_fields: Iterable[str] = (),
        predictor: Any | None = None,
    ) -> None:
        self.reflection_lm = reflection_lm
        self.fields = tuple(fields)
        self.intent = intent
        self.primary_fields = tuple(primary_fields)
        self.predictor = predictor or dspy.ChainOfThought(JevInstructionProposal)
        self.rejection_count = 0

    def _propose(
        self, current: str, feedback: str, violations: list[str]
    ) -> str:
        kwargs = {
            "current_statement": current,
            "intent": self.intent,
            "feedback": feedback or "No feedback was supplied.",
            "state_fields": ", ".join(self.fields),
            "primary_fields": ", ".join(self.primary_fields),
            "violations": "; ".join(violations) if violations else "none",
        }
        if self.reflection_lm is None:
            prediction = self.predictor(**kwargs)
        else:
            with dspy.context(lm=self.reflection_lm):
                prediction = self.predictor(**kwargs)
        return str(prediction.proposed_statement).strip().strip('"')

    def __call__(
        self,
        candidate: dict[str, str],
        reflective_dataset: Any,
        components_to_update: list[str],
    ) -> dict[str, str]:
        feedback = "\n".join(_feedback_lines(reflective_dataset))
        example_texts = tuple(_strings(reflective_dataset))
        result: dict[str, str] = {}
        for component in components_to_update:
            current = candidate[component]
            proposal = self._propose(current, feedback, [])
            violations = validate_statement(
                proposal,
                current_statement=current,
                example_texts=example_texts,
                allowed_fields=self.fields,
                primary_fields=self.primary_fields,
            )
            if violations:
                self.rejection_count += 1
                proposal = self._propose(current, feedback, violations)
                violations = validate_statement(
                    proposal,
                    current_statement=current,
                    example_texts=example_texts,
                    allowed_fields=self.fields,
                    primary_fields=self.primary_fields,
                )
            if violations:
                self.rejection_count += 1
                proposal = current
            result[component] = proposal
        return result
