"""LLM-authored synthetic corpora plus a small deterministic test stub."""

from __future__ import annotations

import hashlib
import json
import os
import random
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx

from .data import DatasetRow, load_jsonl, write_jsonl
from .questions import SETS

DEFAULT_ROOT = Path(__file__).parents[1] / "data" / "synthetic"
DOMAINS = ("legal", "sales", "operations", "academic", "personal administration")
OBLIGATION_KINDS = ("deliverable", "review", "coordination")
RELATIONSHIPS = ("peer-to-peer", "requester-to-provider")
SYNTHETIC_PEOPLE = (
    "Priya Venkataraman", "Tomas Lindqvist", "Amara Okafor", "Mateo Villanueva",
    "Linnea Bergstrom", "Devon Fairchild", "Soraya Nwosu", "Kenji Takamura",
)
SYNTHETIC_COMPANIES = (
    "Halcyon Ridge LLC", "Juniper Vale Ltd", "Copper Lantern Inc", "Blue Heron Labs",
)
HARD_NEGATIVES = (
    "thanks-only acknowledgement", "quoted history without a current update",
    "question about status without an answer", "unrelated update",
)
_EMAIL_RE = re.compile(r"\b[^\s@]+@[^\s@]+\b")
_URL_RE = re.compile(r"(?:https?://|www\.)\S+", re.IGNORECASE)
_CAPITALIZED_BIGRAM_RE = re.compile(r"\b([A-Z][a-z]+(?:\s+[A-Z][A-Za-z]+)+)\b")
_ROW_FIELDS = {
    "closure": {"obligation", "later", "label"},
    "triage": {"subject", "paragraph_text", "from_user", "label"},
    "rules": {
        "sender", "subject", "first_paragraph", "request_text", "phrase", "event_name",
        "action_a", "action_b", "subject_a", "subject_b", "first_paragraph_a",
        "first_paragraph_b", "label",
    },
}


@dataclass(frozen=True)
class CorpusThresholds:
    min_rows: int = 600
    min_paragraphs: int = 300
    min_obligations: int = 40
    max_from_user_gap: float = 0.1
    max_days_mean_gap: float = 2.0
    positive_rate_min: float = 0.4
    positive_rate_max: float = 0.6
    # Four mutually exclusive closure labels cannot each have a 40% positive rate.
    closure_positive_rate_min: float = 0.15
    closure_positive_rate_max: float = 0.30


def scenario_seeds(question_id: str) -> list[dict[str, str]]:
    """Return the fixed 30-scenario matrix used to diversify one question."""

    if question_id not in {item for values in SETS.values() for item in values}:
        raise ValueError(f"unknown question: {question_id}")
    return [
        {"question": question_id, "domain": domain, "obligation_kind": kind,
         "relationship": relationship}
        for domain in DOMAINS
        for kind in OBLIGATION_KINDS
        for relationship in RELATIONSHIPS
    ]


def _normalize(value: str) -> str:
    return " ".join(value.casefold().split())


def _text_values(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, dict):
        # Sorted by key so the id does not depend on insertion order, which a
        # JSON round trip with sorted keys would otherwise change.
        return [text for _, item in sorted(value.items()) for text in _text_values(item)]
    if isinstance(value, list):
        return [text for item in value for text in _text_values(item)]
    return []


def _stable_id(set_name: str, row: dict[str, Any]) -> str:
    material = "\n".join(
        _normalize(text)
        for key, value in sorted(row.items())
        if key not in {"id", "label", "set", "source"}
        for text in _text_values(value)
    )
    return f"{set_name}-{hashlib.sha256(material.encode()).hexdigest()[:20]}"


def _dedupe_text(set_name: str, row: dict[str, Any]) -> str:
    if set_name == "closure":
        return _normalize(str(row["later"]["paragraph_text"]))
    if set_name == "triage":
        return _normalize(str(row["paragraph_text"]))
    return "\n".join(
        _normalize(text)
        for key, value in sorted(row.items())
        if key not in {"id", "label", "set", "source"}
        for text in _text_values(value)
    )


def _allowed_name_phrases() -> set[str]:
    allowed: set[str] = set()
    for name in (*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES):
        parts = name.split()
        for start in range(len(parts) - 1):
            for end in range(start + 2, len(parts) + 1):
                allowed.add(" ".join(parts[start:end]))
    return allowed


def scrub_row(row: dict[str, Any]) -> list[str]:
    """Return privacy/content violations without echoing the suspect values."""

    violations: list[str] = []
    text = "\n".join(_text_values(row))
    if any(
        not address.rstrip(".,;:)").casefold().endswith("@example.invalid")
        for address in _EMAIL_RE.findall(text)
    ):
        violations.append("non-example.invalid email address")
    if _URL_RE.search(text):
        violations.append("URL")
    allowed_names = _allowed_name_phrases()
    if any(
        match.group(1) not in allowed_names
        for match in _CAPITALIZED_BIGRAM_RE.finditer(text)
    ):
        violations.append("capitalized name outside the synthetic pool")
    return violations


def _control_plan(set_name: str, index: int) -> dict[str, Any]:
    question_ids = SETS[set_name]
    if set_name == "closure":
        target = (*question_ids, None)[index % (len(question_ids) + 1)]
        labels = {question_id: question_id == target for question_id in question_ids}
        return {
            "label": labels,
            "from_user": bool((index // (len(question_ids) + 1)) % 2),
            "days_later": 1 + (index // (2 * (len(question_ids) + 1))) % 14,
            "hard_negative": (
                HARD_NEGATIVES[index % len(HARD_NEGATIVES)] if target is None else None
            ),
            "focus_question": target or question_ids[index % len(question_ids)],
        }
    bool_ids = tuple(q for q in question_ids if q != "rules.deadline_kind")
    labels: dict[str, bool | str] = {
        question_id: bool((index // 2 >> offset) & 1)
        for offset, question_id in enumerate(bool_ids)
    }
    if set_name == "rules":
        labels["rules.deadline_kind"] = ("event_tied", "soft", "unknown")[index % 3]
    return {
        "label": labels,
        "from_user": bool(index % 2),
        "focus_question": question_ids[index % len(question_ids)],
    }


def _stub_row(set_name: str, index: int) -> dict[str, Any]:
    plan = _control_plan(set_name, index)
    person = SYNTHETIC_PEOPLE[index % len(SYNTHETIC_PEOPLE)]
    company = SYNTHETIC_COMPANIES[(index // len(SYNTHETIC_PEOPLE)) % len(SYNTHETIC_COMPANIES)]
    marker = f"case {index:03d}"
    if set_name == "closure":
        target = next((q for q, value in plan["label"].items() if value), None)
        outcomes = {
            "closure.fulfilled": "reports that the requested item was delivered",
            "closure.withdrawn": "withdraws the earlier request",
            "closure.deadline_changed": "moves the deadline to next Thursday",
            "closure.modified": "changes the requested item to a short summary",
        }
        paragraph = (
            f"{person} {outcomes[target]} for {company}; reference {marker}."
            if target
            else f"{person} asks whether there is an update for {company}; reference {marker}."
        )
        row = {
            "set": set_name,
            "obligation": {"title": f"Synthetic obligation {marker}",
                           "evidence_text": f"Please prepare item {index:03d} for {company}."},
            "later": {"paragraph_text": paragraph, "from_user": plan["from_user"],
                      "days_later": plan["days_later"]},
            "label": plan["label"],
        }
    elif set_name == "triage":
        active = [q.rsplit(".", 1)[1] for q, value in plan["label"].items() if value]
        row = {
            "set": set_name, "subject": f"Synthetic coordination {marker}",
            "paragraph_text": (
                f"{person} writes to {company} about {marker}; signals: {', '.join(active)}."
            ),
            "from_user": plan["from_user"], "label": plan["label"],
        }
    else:
        positive = [q.rsplit(".", 1)[1] for q, value in plan["label"].items() if value is True]
        row = {
            "set": set_name, "sender": "notices@example.invalid",
            "subject": f"Synthetic rule case {index:03d}",
            "first_paragraph": f"Rule fixture {marker} for {company}: {', '.join(positive)}.",
            "request_text": f"Prepare synthetic item {index:03d} for the review meeting.",
            "phrase": f"before review event {index:03d}",
            "event_name": f"review event {index:03d}",
            "action_a": f"Prepare synthetic item {index:03d}.",
            "action_b": f"Complete synthetic action {index:03d}.",
            "subject_a": f"Synthetic topic {index:03d}",
            "subject_b": f"Synthetic follow-up {index:03d}",
            "first_paragraph_a": f"A discussion for {marker}.",
            "first_paragraph_b": f"Another discussion for {marker}.",
            "label": plan["label"],
        }
    row["source"] = "synthetic-stub"
    row["id"] = _stable_id(set_name, row)
    return row


def generate_stub(
    output: str | Path, *, selected_set: str = "all", rows: int = 60, seed: int = 20260919
) -> list[Path]:
    """Write small deterministic fixtures. The seed controls only row ordering."""

    if rows < 10:
        raise ValueError("stub generation requires at least 10 rows")
    names = tuple(SETS) if selected_set == "all" else (selected_set,)
    root = Path(output)
    paths: list[Path] = []
    for offset, set_name in enumerate(names):
        generated = [_stub_row(set_name, index) for index in range(rows)]
        random.Random(seed + offset).shuffle(generated)
        path = root / f"{set_name}.jsonl"
        write_jsonl(path, generated)
        paths.append(path)
    return paths


def _prompt(set_name: str, plans: list[dict[str, Any]]) -> str:
    fields = {
        "closure": "obligation {title,evidence_text}; later {paragraph_text,from_user,days_later}",
        "triage": "subject; paragraph_text; from_user",
        "rules": (
            "sender; subject; first_paragraph; request_text; phrase; event_name; action_a; "
            "action_b; subject_a; subject_b; first_paragraph_a; first_paragraph_b"
        ),
    }[set_name]
    return (
        "Create realistic but fully invented evaluation rows. Return strict JSON as an object "
        "with one member named rows whose value is an array in exactly the requested order. "
        f"Each row has these fields: {fields}; label. Copy every supplied label and assigned "
        "from_user/days_later value exactly. Write each text as one coherent situation, never by "
        "concatenating independent label sentences. Closure rows may express exactly the one true "
        "closure label, or none; they must not imply another closure outcome. When "
        "hard_negative is "
        "set, implement that negative pattern. Use only these invented people and organizations: "
        f"{', '.join((*SYNTHETIC_PEOPLE, *SYNTHETIC_COMPANIES))}. Any email must end in "
        "example.invalid. Do not emit URLs. Vary language, artifacts, relationships, and contexts. "
        "Plans:\n" + json.dumps(plans, ensure_ascii=False, sort_keys=True)
    )


def _response_format(set_name: str) -> dict[str, Any]:
    label_properties = {
        question_id: (
            {"type": "string", "enum": ["event_tied", "soft", "unknown"]}
            if question_id == "rules.deadline_kind"
            else {"type": "boolean"}
        )
        for question_id in SETS[set_name]
    }
    label = {
        "type": "object", "properties": label_properties,
        "required": list(label_properties), "additionalProperties": False,
    }
    if set_name == "closure":
        properties: dict[str, Any] = {
            "obligation": {
                "type": "object",
                "properties": {"title": {"type": "string"},
                               "evidence_text": {"type": "string"}},
                "required": ["title", "evidence_text"], "additionalProperties": False,
            },
            "later": {
                "type": "object",
                "properties": {"paragraph_text": {"type": "string"},
                               "from_user": {"type": "boolean"},
                               "days_later": {"type": "integer"}},
                "required": ["paragraph_text", "from_user", "days_later"],
                "additionalProperties": False,
            },
            "label": label,
        }
    else:
        properties = {
            field: ({"type": "boolean"} if field == "from_user" else {"type": "string"})
            for field in _ROW_FIELDS[set_name] - {"label"}
        }
        properties["label"] = label
    row_schema = {
        "type": "object", "properties": properties,
        "required": list(properties), "additionalProperties": False,
    }
    return {
        "type": "json_schema",
        "json_schema": {
            "name": f"synthetic_{set_name}_rows", "strict": True,
            "schema": {
                "type": "object", "properties": {
                    "rows": {"type": "array", "items": row_schema}
                },
                "required": ["rows"], "additionalProperties": False,
            },
        },
    }


def _chat_rows(
    client: httpx.Client, model: str, api_key: str, set_name: str, plans: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    response = client.post(
        "https://openrouter.ai/api/v1/chat/completions",
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        json={"model": model,
              "messages": [{"role": "user", "content": _prompt(set_name, plans)}],
              "response_format": _response_format(set_name), "provider": {"zdr": True},
              "temperature": 0.9},
    )
    response.raise_for_status()
    content = response.json()["choices"][0]["message"]["content"]
    payload = json.loads(content)
    if not isinstance(payload, dict) or not isinstance(payload.get("rows"), list):
        raise ValueError("synthetic model response must be an object containing a rows array")
    return payload["rows"]


def _validate_generated_row(
    set_name: str, row: Any, plan: dict[str, Any]
) -> dict[str, Any] | None:
    if (
        not isinstance(row, dict)
        or set(row) != _ROW_FIELDS[set_name]
        or row.get("label") != plan["label"]
    ):
        return None
    if set_name == "closure" and (
        not isinstance(row.get("obligation"), dict)
        or set(row["obligation"]) != {"title", "evidence_text"}
        or not isinstance(row.get("later"), dict)
        or set(row["later"]) != {"paragraph_text", "from_user", "days_later"}
    ):
        return None
    if set_name in {"closure", "triage"}:
        location = row.get("later", row)
        if not isinstance(location, dict) or location.get("from_user") is not plan["from_user"]:
            return None
    if set_name == "closure":
        if row.get("later", {}).get("days_later") != plan["days_later"]:
            return None
        if sum(value is True for value in row["label"].values()) > 1:
            return None
    row = dict(row)
    row["set"] = set_name
    row["source"] = "synthetic-llm"
    if scrub_row(row):
        return None
    row["id"] = _stable_id(set_name, row)
    try:
        DatasetRow.model_validate(row)
    except ValueError:
        return None
    return row


def generate_llm(
    output: str | Path = DEFAULT_ROOT, *, selected_set: str = "all", rows: int = 600,
    seed: int = 20260919, client: httpx.Client | None = None,
) -> list[Path]:
    """Generate corpora through OpenRouter; callers own the live-network decision."""

    if rows < 600:
        raise ValueError("LLM generation requires at least 600 rows per set")
    api_key = os.environ.get("OPENROUTER_API_KEY")
    model = os.environ.get("OPENROUTER_SYNTH_MODEL")
    if not api_key or not model:
        raise RuntimeError("OPENROUTER_API_KEY and OPENROUTER_SYNTH_MODEL must be set")
    names = tuple(SETS) if selected_set == "all" else (selected_set,)
    root = Path(output)
    owned_client = client is None
    http = client or httpx.Client(timeout=120.0)
    paths: list[Path] = []
    try:
        for set_name in names:
            accepted: dict[int, dict[str, Any]] = {}
            seen_ids: set[str] = set()
            seen_texts: set[str] = set()
            pending = [(index, _control_plan(set_name, index)) for index in range(rows)]
            for index, plan in pending:
                seeds = scenario_seeds(plan["focus_question"])
                plan["scenario"] = seeds[(index // len(SETS[set_name])) % len(seeds)]
            attempts = 0
            while len(accepted) < rows and attempts < rows * 5:
                batch = pending[:12]
                plans = [plan for _index, plan in batch]
                candidates = _chat_rows(http, model, api_key, set_name, plans)
                completed: set[int] = set()
                for candidate, (index, plan) in zip(candidates, batch, strict=False):
                    validated = _validate_generated_row(set_name, candidate, plan)
                    normalized_text = (
                        _dedupe_text(set_name, validated) if validated is not None else ""
                    )
                    if (
                        validated is not None
                        and validated["id"] not in seen_ids
                        and normalized_text not in seen_texts
                    ):
                        accepted[index] = validated
                        seen_ids.add(validated["id"])
                        seen_texts.add(normalized_text)
                        completed.add(index)
                failed = [item for item in batch if item[0] not in completed]
                pending = pending[len(batch):] + failed
                attempts += len(batch)
            if len(accepted) < rows:
                raise RuntimeError(
                    "synthetic generation produced only "
                    f"{len(accepted)} valid distinct {set_name} rows"
                )
            generated = [accepted[index] for index in range(rows)]
            random.Random(seed).shuffle(generated)
            path = root / f"{set_name}.jsonl"
            temporary = path.with_suffix(".jsonl.tmp")
            write_jsonl(temporary, generated)
            try:
                check_corpus(temporary)
                temporary.replace(path)
            except Exception:
                temporary.unlink(missing_ok=True)
                raise
            paths.append(path)
    finally:
        if owned_client:
            http.close()
    return paths


def _paragraph_text(row: DatasetRow) -> str:
    if row.set == "closure":
        return str(row.later["paragraph_text"])
    if row.set == "triage":
        return str(row.paragraph_text)
    values = row.model_dump(exclude={"id", "set", "label", "source"})
    return " ".join(_text_values(values))


def check_corpus(
    path: str | Path, thresholds: CorpusThresholds | None = None
) -> dict[str, Any]:
    """Validate corpus diversity, balance, provenance, and closure exclusivity."""

    thresholds = thresholds or CorpusThresholds()
    rows = load_jsonl(path)
    if not rows:
        raise ValueError("corpus is empty")
    set_names = {row.set for row in rows}
    if len(set_names) != 1:
        raise ValueError("corpus must contain exactly one set")
    set_name = set_names.pop()
    errors: list[str] = []
    if len(rows) < thresholds.min_rows:
        errors.append(f"rows {len(rows)} < {thresholds.min_rows}")
    if any(row.id != _stable_id(row.set, row.model_dump()) for row in rows):
        errors.append("one or more row ids are not stable text hashes")
    scrubbed = sum(bool(scrub_row(row.model_dump())) for row in rows)
    if scrubbed:
        errors.append(f"privacy scrub rejected {scrubbed} rows")
    paragraphs = {_normalize(_paragraph_text(row)) for row in rows}
    if len(paragraphs) != len(rows):
        errors.append("normalized paragraph text is not unique")
    if len(paragraphs) < thresholds.min_paragraphs:
        errors.append(f"distinct paragraph texts {len(paragraphs)} < {thresholds.min_paragraphs}")
    obligations: set[str] = set()
    if set_name == "closure":
        obligations = {_normalize(str(row.obligation["evidence_text"])) for row in rows}
        if len(obligations) < thresholds.min_obligations:
            errors.append(
                f"distinct obligation texts {len(obligations)} < {thresholds.min_obligations}"
            )
        if any(sum(value is True for value in row.label.values()) > 1 for row in rows):
            errors.append("a closure row is positive for more than one closure question")
    question_stats: dict[str, Any] = {}
    for question_id in SETS[set_name]:
        values = [row.label[question_id] for row in rows]
        if not all(isinstance(value, bool) for value in values):
            continue
        positives = [row for row, value in zip(rows, values, strict=True) if value]
        negatives = [row for row, value in zip(rows, values, strict=True) if not value]
        rate = len(positives) / len(rows)
        low, high = (
            (thresholds.closure_positive_rate_min, thresholds.closure_positive_rate_max)
            if set_name == "closure"
            else (thresholds.positive_rate_min, thresholds.positive_rate_max)
        )
        if not low <= rate <= high:
            errors.append(f"{question_id} positive rate {rate:.3f} outside [{low}, {high}]")
        stats: dict[str, Any] = {"positive_rate": rate}
        if positives and negatives and set_name == "triage":
            pos_rate = sum(bool(row.from_user) for row in positives) / len(positives)
            neg_rate = sum(bool(row.from_user) for row in negatives) / len(negatives)
            stats["from_user_gap"] = abs(pos_rate - neg_rate)
        elif positives and negatives and set_name == "closure":
            pos_rate = sum(bool(row.later["from_user"]) for row in positives) / len(positives)
            neg_rate = sum(bool(row.later["from_user"]) for row in negatives) / len(negatives)
            stats["from_user_gap"] = abs(pos_rate - neg_rate)
            pos_days = sum(float(row.later["days_later"]) for row in positives) / len(positives)
            neg_days = sum(float(row.later["days_later"]) for row in negatives) / len(negatives)
            stats["days_later_mean_gap"] = abs(pos_days - neg_days)
            if stats["days_later_mean_gap"] > thresholds.max_days_mean_gap:
                errors.append(
                    f"{question_id} days_later mean gap "
                    f"{stats['days_later_mean_gap']:.3f} > {thresholds.max_days_mean_gap}"
                )
        if stats.get("from_user_gap", 0.0) > thresholds.max_from_user_gap:
            errors.append(
                f"{question_id} from_user gap {stats['from_user_gap']:.3f} > "
                f"{thresholds.max_from_user_gap}"
            )
        question_stats[question_id] = stats
    report = {
        "set": set_name, "rows": len(rows), "distinct_paragraph_texts": len(paragraphs),
        "distinct_obligation_texts": len(obligations) if set_name == "closure" else None,
        "questions": question_stats, "errors": errors,
    }
    if errors:
        raise ValueError("; ".join(errors))
    return report
