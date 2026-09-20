"""DSPy adapter that executes one typed Jev request per signature call."""

from __future__ import annotations

from typing import Any

import dspy

from .questions import SPECS, jev_question


class JevLM(dspy.BaseLM):
    """Marker LM; typed execution is handled by :class:`JevAdapter`."""

    def __init__(self) -> None:
        super().__init__(model="typesafe/jev", model_type="chat", cache=False)

    def forward(self, *_args: Any, **_kwargs: Any) -> Any:
        raise RuntimeError("JevLM requires JevAdapter")


class JevAdapter(dspy.Adapter):
    """Translate a DSPy signature directly to Jev state and typed questions."""

    def __init__(self, client: Any, question_id: str) -> None:
        super().__init__()
        self.client = client
        self.question_id = question_id

    def __deepcopy__(self, memo: dict[int, Any]) -> JevAdapter:
        """Copy adapter metadata while deliberately sharing the transport client."""

        copied = type(self)(self.client, self.question_id)
        memo[id(self)] = copied
        return copied

    def __call__(
        self,
        lm: Any,
        lm_kwargs: dict[str, Any],
        signature: type[dspy.Signature],
        demos: list[dict[str, Any]],
        inputs: dict[str, Any],
    ) -> list[dict[str, Any]]:
        del lm, lm_kwargs, demos
        question = jev_question(self.question_id)
        question["instructions"] = signature.instructions
        answer = self.client.decide(inputs, {self.question_id: question})[self.question_id]
        if answer["type"] == "noul":
            return [{"probability": float(answer["noul"])}]
        return [
            {
                "choice": answer["choice"],
                "probabilities": answer["probabilities"],
                "confidence": answer["confidence"],
            }
        ]


class JevPredict(dspy.Module):
    """A GEPA-optimizable predictor backed by Jev rather than a chat model."""

    def __init__(self, question_id: str, client: Any) -> None:
        super().__init__()
        self.predict = dspy.Predict(SPECS[question_id].signature)
        self.adapter = JevAdapter(client, question_id)
        self.lm = JevLM()

    def forward(self, **kwargs: Any) -> dspy.Prediction:
        inputs = {name: kwargs[name] for name in self.predict.signature.input_fields}
        with dspy.context(adapter=self.adapter, lm=self.lm):
            return self.predict(**inputs)


class JevPrograms:
    """Factory for per-question GEPA programs sharing one Decisions client."""

    def __init__(self, client: Any) -> None:
        self.client = client

    def program(self, question_id: str) -> JevPredict:
        return JevPredict(question_id, self.client)
