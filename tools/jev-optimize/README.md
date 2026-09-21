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
40 distinct obligation texts. The threshold tuner requires at least 20 positives
and 20 negatives in its sweep;
smaller samples retain the registry defaults.

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
`json.dumps(obj, separators=(",", ":"), ensure_ascii=False)`. The registry is
written with sorted keys, so PowerShell's `ConvertTo-Json -Depth 100 -Compress`
preserves the same member order. On the hand-written bootstrap file the hashes
do differ: PowerShell preserves the decimal scale (`0.70`) while Python emits
`0.7`. After `write-registry` normalizes the file through Python, the two
recipes serialize parsed numeric values identically.
