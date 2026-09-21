"""JSONL datasets, deterministic splits, and public Enron ingestion."""

from __future__ import annotations

import email
import hashlib
import json
import re
import tarfile
from collections.abc import Iterable
from pathlib import Path
from typing import Any

import httpx
from pydantic import BaseModel, ConfigDict

ENRON_URL = "https://www.cs.cmu.edu/~enron/enron_mail_20150507.tar.gz"


class DatasetRow(BaseModel):
    """Flexible set-specific row with required provenance and labels."""

    model_config = ConfigDict(extra="allow")
    id: str
    set: str
    label: dict[str, bool | str]
    source: str


def load_jsonl(path: str | Path) -> list[DatasetRow]:
    rows: list[DatasetRow] = []
    with Path(path).open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            if line.strip():
                try:
                    value = json.loads(line)
                    _derive_labels(value)
                    rows.append(DatasetRow.model_validate(value))
                except ValueError as error:
                    raise ValueError(f"invalid row {line_number}") from error
    return rows


_CLOSURE_OUTCOMES = (
    "fulfilled",
    "withdrawn",
    "deadline_changed",
    "modified",
)


def _derive_labels(value: dict[str, Any]) -> None:
    """Add choice labels implied by existing exported/synthetic labels."""

    label = value.get("label")
    if not isinstance(label, dict):
        return
    if value.get("set") == "closure" and all(
        f"closure.{outcome}" in label for outcome in _CLOSURE_OUTCOMES
    ):
        selected = [
            outcome for outcome in _CLOSURE_OUTCOMES if label[f"closure.{outcome}"] is True
        ]
        if len(selected) <= 1:
            label["closure.outcome"] = selected[0] if selected else "none"
    if value.get("set") == "triage":
        if label.get("triage.asks_question") is True:
            claim_type = "question"
        elif label.get("triage.asks_recipient") is True:
            claim_type = "request"
        elif label.get("triage.commits_sender") is True:
            claim_type = "promise"
        else:
            claim_type = "none"
        label["extract.claim_type"] = claim_type


def write_jsonl(path: str | Path, rows: Iterable[dict[str, Any]]) -> None:
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w", encoding="utf-8", newline="\n") as stream:
        for row in rows:
            stream.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def split_name(row_id: str, seed: int = 0, *, small: bool = False) -> str:
    """Assign a row to train, validation or test by a stable hash of its id.

    Large populations split 78/7/15: GEPA scores every candidate on the whole
    validation set, so a small validation set leaves the budget for exploring
    candidates while the held-out test set keeps its size. Populations under
    ``SMALL_POPULATION`` rows (one question of a per-question corpus) split
    60/20/20: the threshold chooser needs at least 20 examples of each class
    in the validation sweep, which 400 rows per question meets at 20%.
    """
    bucket = int.from_bytes(
        hashlib.sha256(f"{seed}:{row_id}".encode()).digest()[:8], "big"
    ) % 100
    train_end, validation_end = (60, 80) if small else (78, 85)
    return "train" if bucket < train_end else "validation" if bucket < validation_end else "test"


SMALL_POPULATION = 1000


def split_rows(
    rows: Iterable[DatasetRow], seed: int = 0, question_id: str | None = None
) -> dict[str, list[DatasetRow]]:
    population = [
        row for row in rows if question_id is None or question_id in row.label
    ]
    small = len(population) < SMALL_POPULATION
    result: dict[str, list[DatasetRow]] = {"train": [], "validation": [], "test": []}
    for row in population:
        result[split_name(row.id, seed, small=small)].append(row)
    return result


def _paragraphs(text: str) -> Iterable[str]:
    for paragraph in re.split(r"\r?\n\s*\r?\n", text):
        cleaned = " ".join(paragraph.split())
        if 20 <= len(cleaned) <= 4000 and not cleaned.startswith(">"):
            yield cleaned


def _silver_labels(text: str) -> dict[str, bool]:
    lower = text.lower()
    return {
        "triage.asks_recipient": bool(
            re.search(r"\b(please|could you|would you)\b", lower)
        ),
        "triage.commits_sender": bool(
            re.search(r"\b(i will|i'll|we will|we'll)\b", lower)
        ),
        "triage.asks_question": "?" in text,
        "triage.names_time": bool(
            re.search(
                r"\b(today|tomorrow|monday|tuesday|wednesday|thursday|friday|"
                r"\d{1,2}:\d{2})\b",
                lower,
            )
        ),
        "triage.boilerplate": bool(
            re.search(r"\b(unsubscribe|confidentiality notice)\b", lower)
        ),
        "triage.automated_notification": bool(
            re.search(r"\b(automated|do not reply)\b", lower)
        ),
    }


def fetch_enron(destination: str | Path | None = None) -> Path:
    """Download public mail and emit honest smoke-only silver rows."""
    root = Path(destination or Path(__file__).parents[1] / "data" / "enron")
    root.mkdir(parents=True, exist_ok=True)
    archive = root / "enron_mail.tar.gz"
    rows_path = root / "paragraphs.jsonl"
    if not archive.exists():
        with httpx.stream("GET", ENRON_URL, follow_redirects=False, timeout=120.0) as response:
            response.raise_for_status()
            with archive.open("wb") as stream:
                for chunk in response.iter_bytes():
                    stream.write(chunk)
    rows: list[dict[str, Any]] = []
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle:
            if not member.isfile() or len(rows) >= 20_000:
                continue
            source = bundle.extractfile(member)
            if source is None:
                continue
            message = email.message_from_bytes(source.read())
            payload = message.get_payload(decode=True)
            if not isinstance(payload, bytes):
                continue
            body = payload.decode(message.get_content_charset() or "utf-8", errors="replace")
            for ordinal, paragraph in enumerate(_paragraphs(body)):
                digest = hashlib.sha256(f"{member.name}:{ordinal}".encode()).hexdigest()[:20]
                rows.append(
                    {
                        "id": f"enron-{digest}",
                        "set": "triage",
                        "subject": str(message.get("subject", ""))[:500],
                        "paragraph_text": paragraph,
                        "from_user": False,
                        "label": _silver_labels(paragraph),
                        "source": "enron-unlabeled",
                    }
                )
    write_jsonl(rows_path, rows)
    return rows_path
