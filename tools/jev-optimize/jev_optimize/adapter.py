"""Thin DSPy system-one LM for OpenRouter Decisions."""

from __future__ import annotations

from typing import Any

import dspy

from .questions import SPECS


class JevLM:
    """Expose the Decisions transport through DSPy's decision-request protocol."""

    supports_decision_requests = True
    cache = True

    def __init__(self, client: Any, question_id: str) -> None:
        self.client = client
        self.question_id = question_id

    def __deepcopy__(self, memo: dict[int, Any]) -> JevLM:
        """Copy LM metadata while deliberately sharing the transport client."""

        copied = type(self)(self.client, self.question_id)
        memo[id(self)] = copied
        return copied

    def __call__(
        self, state: dict[str, Any], questions: dict[str, Any]
    ) -> dict[str, Any]:
        if set(questions) != {"decision"}:
            raise ValueError("JevLM requires exactly one 'decision' question")
        flat_state = state["inputs"]
        production_questions = {
            self.question_id: {
                **questions["decision"],
                "instructions": state["instructions"],
            }
        }
        request = {
            "provider": "jev",
            "state": flat_state,
            "questions": production_questions,
        }
        response = dspy.cache.get(request) if self.cache else None
        if response is None:
            response = self.client.decide(flat_state, production_questions)
            if self.cache:
                dspy.cache.put(request, response)
        return {"decision": response[self.question_id]}


def program(question_id: str, client: Any) -> dspy.Predict:
    """Build one native decision predictor and bind it to the shared transport."""

    predictor = dspy.Predict(SPECS[question_id].signature)
    predictor.set_lm(JevLM(client, question_id))
    return predictor
