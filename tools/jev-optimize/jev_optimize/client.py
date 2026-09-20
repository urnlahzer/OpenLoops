"""Strict, content-safe client helpers for OpenRouter Decisions."""

from __future__ import annotations

import hashlib
import json
import os
import time
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import httpx

DECISIONS_URL = "https://openrouter.ai/api/alpha/decisions"
_ENVELOPE_KEYS = {"model", "answers", "usage", "id", "object", "created", "provider"}


class DecisionResponseError(ValueError):
    """The endpoint returned a response outside the Jev contract."""


def stable_request_hash(state: Mapping[str, Any], questions: Mapping[str, Any]) -> str:
    payload = json.dumps(
        [state, questions], ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def request_bytes(
    model: str,
    state: Mapping[str, Any],
    questions: Mapping[str, Any],
    *,
    zdr_member: bool,
) -> bytes:
    body: dict[str, Any] = {"model": model, "state": state, "questions": questions}
    if zdr_member:
        body["provider"] = {"zdr": True}
    return json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()


def model_matches(reported: str, selected: str) -> bool:
    """The exact selected label, or that label with a dated or variant suffix.

    OpenRouter reports the build actually served, e.g. ``typesafe/jev-1.13``
    as ``typesafe/jev-1.13-20260917``; a different model or family never
    matches.
    """
    if reported == selected:
        return True
    for separator in (":", "-"):
        prefix = f"{selected}{separator}"
        if reported.startswith(prefix) and len(reported) > len(prefix):
            return all(ch.isalnum() or ch in ".-_" for ch in reported[len(prefix) :])
    return False


def _probability(value: Any) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise DecisionResponseError("invalid probability")
    result = float(value)
    if not 0.0 <= result <= 1.0:
        raise DecisionResponseError("invalid probability")
    return result


def parse_response(raw: bytes, model: str, questions: Mapping[str, Any]) -> dict[str, Any]:
    def no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in pairs:
            if key in result:
                raise DecisionResponseError("duplicate JSON member")
            result[key] = item
        return result

    try:
        value = json.loads(raw, object_pairs_hook=no_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise DecisionResponseError("invalid JSON") from error
    if not isinstance(value, dict) or set(value) - _ENVELOPE_KEYS:
        raise DecisionResponseError("invalid response envelope")
    reported = value.get("model")
    if not isinstance(reported, str) or not model_matches(reported, model):
        # The reported label is provider metadata, never mail content.
        raise DecisionResponseError(f"model mismatch: reported {reported!r}")
    supplied = value.get("answers")
    if not isinstance(supplied, dict) or set(supplied) != set(questions):
        raise DecisionResponseError("answer ids mismatch")
    # `usage` is provider accounting metadata: OpenRouter adds members such as
    # `cost` beside TypeSafe's token counts. Only `input_tokens` is read.
    usage = value.get("usage")
    if usage is not None and (
        not isinstance(usage, dict)
        or isinstance(usage.get("input_tokens"), bool)
        or not isinstance(usage.get("input_tokens"), int)
        or usage["input_tokens"] < 0
    ):
        raise DecisionResponseError("invalid usage")
    parsed: dict[str, Any] = {}
    for question_id, question in questions.items():
        answer = supplied[question_id]
        if not isinstance(answer, dict) or answer.get("type") != question.get("type"):
            raise DecisionResponseError("answer type mismatch")
        kind = question["type"]
        if kind == "noul":
            if set(answer) != {"type", "noul"}:
                raise DecisionResponseError("invalid noul answer")
            parsed[question_id] = {"type": "noul", "noul": _probability(answer["noul"])}
        elif kind == "choice":
            if set(answer) != {"type", "choice", "probabilities", "confidence"}:
                raise DecisionResponseError("invalid choice answer")
            choices = set(question.get("criteria", {}))
            probabilities = answer.get("probabilities")
            if (
                answer.get("choice") not in choices
                or not isinstance(probabilities, dict)
                or set(probabilities) - choices
            ):
                raise DecisionResponseError("invalid choice")
            parsed[question_id] = {
                "type": "choice",
                "choice": answer["choice"],
                "probabilities": {key: _probability(item) for key, item in probabilities.items()},
                "confidence": _probability(answer.get("confidence")),
            }
        else:
            raise DecisionResponseError("unsupported question type")
    return parsed


class DecisionsClient:
    """OpenRouter Jev client which never emits request content to logs."""

    def __init__(
        self,
        base_url: str = DECISIONS_URL,
        model: str = "typesafe/jev-1.13",
        api_key_env: str = "OPENROUTER_API_KEY",
        *,
        transport: httpx.BaseTransport | None = None,
        max_retries: int = 3,
    ) -> None:
        self.base_url = base_url
        self.model = model
        self.api_key_env = api_key_env
        self.zdr_member = True
        self.max_retries = max_retries
        self._http = httpx.Client(transport=transport, timeout=20.0, follow_redirects=False)

    def _post(self, content: bytes) -> httpx.Response:
        key = os.environ.get(self.api_key_env)
        if not key:
            raise RuntimeError(f"{self.api_key_env} is not set")
        headers = {"Authorization": f"Bearer {key}", "Content-Type": "application/json"}
        for attempt in range(self.max_retries + 1):
            response = self._http.post(self.base_url, headers=headers, content=content)
            if response.status_code not in {429, 529} or attempt == self.max_retries:
                return response
            time.sleep(0.25 * (2**attempt))
        raise AssertionError("unreachable")

    def probe(self) -> bool:
        """Run a content-free capability probe and remember provider-member support.

        The state is one synthetic sentence and the question set is one noul, so
        the server's reason for a rejection may be shown: nothing in it is mail.
        """
        state = {"text": "Please send the signed form by Friday."}
        questions = {
            "asks_recipient": {
                "type": "noul",
                "instructions": "The text asks the reader to do something.",
            }
        }
        response = self._post(request_bytes(self.model, state, questions, zdr_member=True))
        if response.status_code in {400, 422}:
            self.zdr_member = False
            response = self._post(
                request_bytes(self.model, state, questions, zdr_member=False)
            )
        if response.status_code >= 400:
            excerpt = response.text[:500]
            raise RuntimeError(
                f"probe rejected with HTTP {response.status_code}: {excerpt}"
            )
        parse_response(response.content, self.model, questions)
        return self.zdr_member

    def decide(
        self, state: Mapping[str, Any], questions: Mapping[str, Any]
    ) -> dict[str, Any]:
        response = self._post(
            request_bytes(self.model, state, questions, zdr_member=self.zdr_member)
        )
        response.raise_for_status()
        return parse_response(response.content, self.model, questions)


class ReplayClient:
    """Offline Decisions client backed by stable request hashes."""

    def __init__(self, records: Mapping[str, Any] | str | Path) -> None:
        if isinstance(records, (str, Path)):
            records = json.loads(Path(records).read_text(encoding="utf-8"))
        self.records = dict(records)

    def decide(
        self, state: Mapping[str, Any], questions: Mapping[str, Any]
    ) -> dict[str, Any]:
        key = stable_request_hash(state, questions)
        try:
            raw = self.records[key]
        except KeyError as error:
            raise KeyError(f"no replay for request hash {key}") from error
        return raw["answers"] if isinstance(raw, dict) and "answers" in raw else raw


class RecordingClient:
    """Record wrapped-client answers for explicit reproducible runs."""

    def __init__(self, client: Any, path: str | Path) -> None:
        self.client = client
        self.path = Path(path)
        if self.path.parent.name != "runs":
            raise ValueError("recordings must be stored in the git-ignored runs folder")
        self.records: dict[str, Any] = {}

    def decide(
        self, state: Mapping[str, Any], questions: Mapping[str, Any]
    ) -> dict[str, Any]:
        answer = self.client.decide(state, questions)
        self.records[stable_request_hash(state, questions)] = {"answers": answer}
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps(self.records, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        return answer
