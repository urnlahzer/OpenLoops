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
uv run jev-optimize generate-synthetic
uv run pytest -q
uv run ruff check .
```

The live commands read `OPENROUTER_API_KEY` only at request time; the harness
does not print, log, or store it. `OPENROUTER_REFLECTION_MODEL` selects the chat
model used by GEPA for prompt reflection. The Decisions endpoint receives
`provider: {"zdr": true}` unless a content-free probe shows that the alpha
endpoint rejects that member. The reflection endpoint always requests ZDR.

## Commands

```powershell
uv run jev-optimize probe
uv run jev-optimize fetch-enron
uv run jev-optimize evaluate --set closure --data data/synthetic/closure.jsonl --replay runs/closure.json
uv run jev-optimize optimize --set closure --budget light
uv run jev-optimize write-registry runs/closure-result.json
```

Run `optimize` separately for `closure`, `triage`, and `rules`; pass all result
files to `write-registry` to merge them in one reviewed edit. Replay files are
stable-hash keyed and allow evaluation without a network call.

Enron rows are explicitly marked `source: "enron-unlabeled"`. Their simple
rule-produced silver labels are for smoke, calibration, and coverage sweeps
only, never accuracy claims. Real Enron labels require owner review. An owner
export is similarly opt-in and outside this repository; use
`feedback_includes_text=False` for it so GEPA feedback remains content-free.

The registry hash printed by `write-registry` is SHA-256 over
`json.dumps(obj, separators=(",", ":"), ensure_ascii=False)`. The registry is
written with sorted keys, so PowerShell's `ConvertTo-Json -Depth 100 -Compress`
preserves the same member order. On the hand-written bootstrap file the hashes
do differ: PowerShell preserves the decimal scale (`0.70`) while Python emits
`0.7`. After `write-registry` normalizes the file through Python, the two
recipes serialize parsed numeric values identically.
