# Jev question optimization

This uv project tunes OpenLoops' typed Jev questions with DSPy GEPA. It is an
owner-run development harness, not application runtime code and not a CI job.
The committed bootstrap corpus contains only invented people and
`example.invalid` addresses. Downloaded Enron data and recordings stay in the
git-ignored `data/enron/` and `runs/` directories.

## Setup

Install Python 3.11 or newer and uv, then run:

```powershell
cd tools/jev-optimize
uv sync --locked
uv run pytest -q
uv run ruff check .
```

The live commands read `OPENROUTER_API_KEY` only at request time; the harness
does not print, log, or store it. `OPENROUTER_REFLECTION_MODEL` selects the chat
model used by GEPA for prompt reflection, and `OPENROUTER_SYNTH_MODEL` selects
the model that authors synthetic rows. The Decisions endpoint receives
`provider: {"zdr": true}` unless a content-free probe shows that the alpha
endpoint rejects that member. The reflection endpoint always requests ZDR.

`JevLM` sends the production wire shape: flat state fields, the registry question
ID as the question key, and the DSPy signature docstring in that question's
`instructions`. `tests/test_offline_end_to_end.py` runs the real GEPA and
ReAnchor path against a fake Decisions endpoint and is the required offline gate
before any paid optimization run.

## Commands

```powershell
$env:OPENROUTER_API_KEY = "<injected-secret>"
$env:OPENROUTER_SYNTH_MODEL = "<openrouter-model-id>"
uv run jev-optimize generate-synthetic --llm --set closure --rows 600
uv run jev-optimize check-corpus data/synthetic/closure.jsonl
uv run jev-optimize probe
uv run jev-optimize fetch-enron
uv run jev-optimize evaluate --set closure --data data/synthetic/closure.jsonl --replay runs/closure.json
uv run jev-optimize optimize --set closure --budget light
uv run jev-optimize write-registry runs/closure-result.json
```

Use `--set triage`, `--set rules`, or `--set all` for the other corpora. The
stdlib-only `generate-synthetic --stub` mode writes tiny deterministic fixtures
for tests; tests always direct those files to a temporary directory. It is not a
replacement for the committed LLM-authored corpus.

## Derived questions: `closure.outcome` and `extract.claim_type`

Two questions have no corpus of their own; their labels are derived on load
(`data.py`'s `load_jsonl`) from an existing set's rows, so `--set extract`
generation is refused (`extract` rows come from `triage`) and `--set closure`
already carries `closure.outcome`.

- `closure.outcome` (choice: `fulfilled` / `withdrawn` / `deadline_changed` /
  `modified` / `none`) is derived from a closure row's four noul labels: the
  one true noul, or `none` when all four are false. A row positive for more
  than one noul is a data-quality issue and is left without a derived
  `closure.outcome` label (`check-corpus`'s own exclusivity check catches it
  separately). `optimize --set closure` and `check-corpus` on a closure file
  tune and report `closure.outcome` alongside the four nouls without any
  separate `closure.outcome` corpus file.
- `extract.claim_type` (choice: `request` / `promise` / `question` /
  `attribution` / `delegation` / `none`) is derived from a triage row's own
  labels: `question` when `triage.asks_question` is true, else `request` when
  `triage.asks_recipient` is true, else `promise` when
  `triage.commits_sender` is true, else `none`. `attribution` and
  `delegation` have no triage signal to derive from and never appear in the
  derived corpus; they are registered so the desktop app's compare probe and
  a future owner-labeled corpus can use them. `optimize --set extract` and
  `evaluate --set extract` default their `--data` to the `triage` corpus
  (`triage.jsonl`); point `--data` at a `triage`-shaped file explicitly to use
  another one. `check-corpus` run on a `triage` file reports both the
  triage question's own balance and `extract.claim_type`'s label
  distribution.

Run `optimize` separately for `closure`, `triage`, and `rules`; pass all result
files to `write-registry` to merge them in one reviewed edit. Replay files are
stable-hash keyed and allow evaluation without a network call.

The committed synthetic corpus must contain at least 600 rows per set and pass
`check-corpus` before a PR. Closure rows carry all four closure labels; those
labels are mutually exclusive (or all-negative), so each question is 15–30%
positive. Triage and rules use a per-question layout: each row carries exactly
one label and only the input fields used by that question. At 600 rows, each of
their six questions receives 100 examples. Binary questions are exactly half
positive and half negative; `rules.deadline_kind` is divided as evenly as
possible among `event_tied`, `soft`, and `unknown` (34/33/33 at 100 rows).
For every applicable binary label, `from_user` differs by at most 0.1 between
positive and negative rows and the `days_later` means differ by at most 2 days.
Each corpus has at least 300 distinct paragraph texts, and closure has at least
40 distinct obligation texts. Calibration requires at least 20 positives and
20 negatives; smaller samples retain the registry defaults. For Noul questions,
DSPy's ReAnchor runs twice on train plus validation after GEPA: one metric heavily
penalizes false positives to fit the accept threshold, and the mirrored metric
heavily penalizes false negatives to fit the escalation threshold. Choice
questions continue to use the confidence-based selective-classification sweep.

LLM generation uses a fixed matrix of at least 30 scenario seeds per question,
sends a separate prompt and strict schema for each question batch, assigns
labels and nuisance fields before each request, requests
`provider.zdr=true`, deduplicates normalized text, and gives rows stable text
hash IDs. The post-generation scrub rejects URLs, addresses outside
`example.invalid`, and capitalized names outside the documented invented pool.
Rejected or duplicate rows are regenerated. Generation and tests never make a
live request unless the owner explicitly chooses `--llm`.

GEPA uses a Jev-specific proposer. Every question supplies a fixed semantic
intent plus allowed and primary fields. Proposed instructions are one or two
literal,
present-tense declarative sentences of at most 45 words, may name state fields,
and contain no role framing, output directions, examples, lists, stacked
negation, or proper nouns copied from examples. They must mention a primary
field, may mention no field outside the allowed list, and must preserve the
intent while changing only wording or precision. Invalid proposals are retried
once and then discarded in favor of the current statement.

`evaluate` includes TP/FP/TN/FN counts at both the checked-in accept and
escalate thresholds for each question.

Optimization results retain baseline and tuned validation/test measurements.
`write-registry` prints a per-question comparison and refuses results marked
`insufficient_data` or `kept_baseline`. After reviewing why a baseline was kept,
an owner may explicitly override that guard with `--allow-baseline`.

Enron rows are explicitly marked `source: "enron-unlabeled"`. Their simple
rule-produced silver labels are for smoke, calibration, and coverage sweeps
only, never accuracy claims. Real Enron labels require owner review. An owner
export is similarly opt-in and outside this repository; use
`feedback_includes_text=False` for it so GEPA feedback remains content-free.

The desktop app's own **Export training data** control (Sources screen,
OpenRouter card) writes this owner export directly in the harness's JSONL
schema: `triage.jsonl` and `closure.jsonl`, one JSON object per line,
`sort_keys`-equivalent (sorted member order), `source: "owner-export"`.
Closure rows also carry `label_source: "gold"` where the label reflects the
owner's own Accept/Reject on a suggested update, or `"silver"` where it is
the chat model's own guess (pending, or no signal at all) -- treat gold rows
as ground truth and silver rows the same as any other silver source. Point
`evaluate --data`/`optimize --data` (or `check-corpus`) at either file
directly; run `--set triage`/`--set closure` to match. Both files are
already outside this repository by construction (the exporter refuses to
write inside it), so no extra step is needed to keep them out of `git`.

The registry hash printed by `write-registry` is SHA-256 over
`json.dumps(obj, separators=(",", ":"), ensure_ascii=False)`. It is not the
value to pin in `tools/check-model-boundary.ps1`. That checker hashes
`ConvertTo-Json -Depth 100 -Compress` of the parsed file, and PowerShell parses
`tuned_at` into a DateTime and re-emits it in the machine's local offset, so the
two recipes differ in that one member (and the checker's value depends on the
timezone of the machine that computes it). Pin the checker's own value:

```powershell
$c = Get-Content -Raw contracts/model/decision-questions.json | ConvertFrom-Json
$json = $c | ConvertTo-Json -Depth 100 -Compress
$sha = [System.Security.Cryptography.SHA256]::Create()
($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($json)) | ForEach-Object { $_.ToString('x2') }) -join ''
```
