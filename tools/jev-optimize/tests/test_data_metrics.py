from types import SimpleNamespace

from jev_optimize.data import DatasetRow, load_jsonl, split_name, split_rows, write_jsonl
from jev_optimize.metrics import (
    accuracy,
    brier_score,
    choose_thresholds,
    expected_calibration_error,
    threshold_sweep,
)
from jev_optimize.optimize import evaluate_question
from jev_optimize.synthetic import CorpusThresholds, _stable_id, check_corpus


def test_split_is_deterministic_and_complete():
    rows = [
        DatasetRow(id=f"row-{index}", set="triage", label={"q": True}, source="synthetic")
        for index in range(1000)
    ]
    first = split_rows(rows, seed=7)
    second = split_rows(reversed(rows), seed=7)
    assert {name: sorted(row.id for row in values) for name, values in first.items()} == {
        name: sorted(row.id for row in values) for name, values in second.items()
    }
    assert split_name("stable", 7) == split_name("stable", 7)
    assert sum(map(len, first.values())) == 1000


def test_split_filters_rows_for_one_question():
    rows = [
        DatasetRow(id="one", set="triage", label={"q.one": True}, source="synthetic"),
        DatasetRow(id="two", set="triage", label={"q.two": False}, source="synthetic"),
    ]
    parts = split_rows(rows, question_id="q.one")
    assert [row.id for values in parts.values() for row in values] == ["one"]


def test_metrics_on_toy_set():
    labels = [True, True, False, False]
    probabilities = [0.9, 0.8, 0.2, 0.1]
    assert accuracy(labels, probabilities) == 1.0
    assert abs(brier_score(labels, probabilities) - 0.025) < 1e-12
    assert abs(expected_calibration_error(labels, probabilities) - 0.15) < 1e-12
    sweep = threshold_sweep(labels, probabilities)
    assert len(sweep) == 21
    assert sweep[10]["coverage"] == 0.5
    assert sweep[10]["selective_risk"] == 0.0


def test_threshold_chooser_obeys_risk_and_never_accepts_below_half():
    # Perfectly separated data would let a naive chooser accept at 0.15; a
    # noul below 0.5 leans false, so accept is floored at 0.5 and the
    # escalation band sits just below it.
    labels = [True] * 20 + [False] * 20
    probabilities = [0.9] * 20 + [0.1] * 20
    result = choose_thresholds(threshold_sweep(labels, probabilities))
    assert result == {"accept": 0.5, "escalate": 0.45, "insufficient_data": False}
    assert result["accept"] >= 0.5 > result["escalate"] >= 0.2


def test_threshold_chooser_uses_defaults_for_insufficient_support():
    result = choose_thresholds(threshold_sweep([True] * 19 + [False] * 20, [0.5] * 39))
    assert result == {"accept": 0.7, "escalate": 0.3, "insufficient_data": True}


def test_threshold_chooser_enforces_positive_recall_floor():
    labels = [True] * 20 + [False] * 20
    probabilities = [0.9] * 9 + [0.4] * 11 + [0.45] * 20
    result = choose_thresholds(threshold_sweep(labels, probabilities))
    assert result["accept"] == 0.7
    assert result["escalate"] == 0.4
    assert result["insufficient_data"] is False


def test_evaluate_reports_confusion_at_registry_thresholds():
    probabilities = iter((0.9, 0.6, 0.4, 0.1))

    def program(**_inputs):
        return SimpleNamespace(probability=next(probabilities))

    rows = [
        DatasetRow(
            id=f"row-{index}",
            set="triage",
            subject="Synthetic subject",
            paragraph_text=f"Synthetic paragraph {index}.",
            from_user=bool(index % 2),
            label={"triage.asks_recipient": label},
            source="synthetic-stub",
        )
        for index, label in enumerate((True, True, False, False))
    ]
    result = evaluate_question(
        program,
        "triage.asks_recipient",
        rows,
        registry_thresholds={"accept": 0.7, "escalate": 0.3},
    )
    assert result["confusion"] == {
        "accept": {"threshold": 0.7, "tp": 1, "fp": 0, "tn": 2, "fn": 1},
        "escalate": {"threshold": 0.3, "tp": 2, "fp": 1, "tn": 1, "fn": 0},
    }


def test_evaluate_skips_rows_without_the_question_label():
    rows = [
        DatasetRow(
            id="other", set="triage", subject="Other", paragraph_text="Other text.",
            from_user=False, label={"triage.names_time": True}, source="synthetic-stub",
        ),
        DatasetRow(
            id="target", set="triage", subject="Target", paragraph_text="Target text.",
            from_user=True, label={"triage.asks_recipient": True}, source="synthetic-stub",
        ),
    ]
    calls = []

    def program(**inputs):
        calls.append(inputs)
        return SimpleNamespace(probability=0.9)

    result = evaluate_question(program, "triage.asks_recipient", rows)
    assert result["accuracy"] == 1.0
    assert len(calls) == 1


def _closure_row(row_id: str, **outcomes: bool) -> dict:
    label = {
        f"closure.{outcome}": outcomes.get(outcome, False)
        for outcome in ("fulfilled", "withdrawn", "deadline_changed", "modified")
    }
    return {
        "id": row_id,
        "set": "closure",
        "obligation": {"title": "Synthetic obligation", "evidence_text": "Please send it."},
        "later": {"paragraph_text": "Synthetic follow-up.", "from_user": True, "days_later": 1},
        "label": label,
        "source": "synthetic-stub",
    }


def test_closure_outcome_is_derived_from_the_single_true_noul(tmp_path):
    path = tmp_path / "closure.jsonl"
    write_jsonl(
        path,
        [
            _closure_row("fulfilled-row", fulfilled=True),
            _closure_row("withdrawn-row", withdrawn=True),
            _closure_row("deadline-row", deadline_changed=True),
            _closure_row("modified-row", modified=True),
            _closure_row("none-row"),
        ],
    )
    rows = {row.id: row for row in load_jsonl(path)}
    assert rows["fulfilled-row"].label["closure.outcome"] == "fulfilled"
    assert rows["withdrawn-row"].label["closure.outcome"] == "withdrawn"
    assert rows["deadline-row"].label["closure.outcome"] == "deadline_changed"
    assert rows["modified-row"].label["closure.outcome"] == "modified"
    assert rows["none-row"].label["closure.outcome"] == "none"


def test_closure_outcome_is_not_derived_when_more_than_one_noul_is_true(tmp_path):
    path = tmp_path / "closure.jsonl"
    write_jsonl(path, [_closure_row("ambiguous-row", fulfilled=True, withdrawn=True)])
    rows = load_jsonl(path)
    assert "closure.outcome" not in rows[0].label


def _triage_row(row_id: str, **labels: bool) -> dict:
    label = {f"triage.{name}": value for name, value in labels.items()}
    return {
        "id": row_id,
        "set": "triage",
        "subject": "Synthetic subject",
        "paragraph_text": "Synthetic paragraph.",
        "from_user": False,
        "label": label,
        "source": "synthetic-stub",
    }


def test_extract_claim_type_is_derived_from_triage_labels(tmp_path):
    path = tmp_path / "triage.jsonl"
    write_jsonl(
        path,
        [
            _triage_row("question-row", asks_question=True, asks_recipient=True),
            _triage_row("request-row", asks_recipient=True),
            _triage_row("promise-row", commits_sender=True),
            _triage_row("none-row", names_time=True),
        ],
    )
    rows = {row.id: row for row in load_jsonl(path)}
    assert rows["question-row"].label["extract.claim_type"] == "question"
    assert rows["request-row"].label["extract.claim_type"] == "request"
    assert rows["promise-row"].label["extract.claim_type"] == "promise"
    assert rows["none-row"].label["extract.claim_type"] == "none"


def test_extract_claim_type_is_not_derived_outside_the_triage_set(tmp_path):
    path = tmp_path / "rules.jsonl"
    write_jsonl(
        path,
        [
            {
                "id": "rules-row",
                "set": "rules",
                "phrase": "before the board meeting",
                "label": {"rules.deadline_kind": "event_tied"},
                "source": "synthetic-stub",
            }
        ],
    )
    rows = load_jsonl(path)
    assert "extract.claim_type" not in rows[0].label


def test_check_corpus_reports_the_closure_outcome_label_distribution(tmp_path):
    rows = (
        [_closure_row(f"fulfilled-{index}", fulfilled=True) for index in range(20)]
        + [_closure_row(f"withdrawn-{index}", withdrawn=True) for index in range(20)]
        + [_closure_row(f"deadline-{index}", deadline_changed=True) for index in range(20)]
        + [_closure_row(f"modified-{index}", modified=True) for index in range(20)]
        + [_closure_row(f"none-{index}") for index in range(20)]
    )
    # Distinct obligation and paragraph text per row so the diversity checks
    # in check_corpus do not themselves fail, and a stable id matching what
    # check_corpus recomputes from the row's own content.
    for index, row in enumerate(rows):
        row["obligation"]["evidence_text"] = f"Please send item {index}."
        row["later"]["paragraph_text"] = f"Synthetic follow-up {index}."
        row["id"] = _stable_id("closure", row)
    path = tmp_path / "closure.jsonl"
    write_jsonl(path, rows)
    report = check_corpus(
        path,
        CorpusThresholds(min_rows=100, min_paragraphs=50, min_obligations=50),
    )
    counts = report["questions"]["closure.outcome"]["label_counts"]
    assert counts == {
        "fulfilled": 20,
        "withdrawn": 20,
        "deadline_changed": 20,
        "modified": 20,
        "none": 20,
    }


def test_split_rows_filters_on_a_derived_question_id(tmp_path):
    path = tmp_path / "triage.jsonl"
    write_jsonl(
        path,
        [
            _triage_row("question-row", asks_question=True, asks_recipient=True),
            _triage_row("none-row", names_time=True),
        ],
    )
    rows = load_jsonl(path)
    parts = split_rows(rows, question_id="extract.claim_type")
    selected_ids = {row.id for values in parts.values() for row in values}
    assert selected_ids == {"question-row", "none-row"}
