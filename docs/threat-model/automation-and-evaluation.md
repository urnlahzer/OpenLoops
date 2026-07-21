# Automation and evaluation

**Status:** ADR-011 decision contract accepted; runtime and all named gates
remain inactive.

The primary safety property is that no candidate, ambiguous, undated,
historical, medium/low-confidence, identity-ambiguous, delegation-ambiguous,
or quote-ambiguous case can ever receive an automatic mutation under any mode
reading, that a flag can never flip without independent gate evidence plus an
owner release decision, and that a user can always fall back to
confirmation-first instantly and unconditionally. No mechanism this ADR pins
is a guarantee derived from a finite-sample observation; "zero in 10,000"
remains an observation, never a proof.

| Threat | Required control | Fail-closed result |
|---|---|---|
| Eligibility creep (a medium/low-confidence, ambiguous, or undated case admitted to automatic mutation) | Closed mode/stratum catalogs; hybrid and automatic both keep candidate/ambiguous/identity-ambiguous/delegation-ambiguous/quote-ambiguous/undated/historical/stale-write cases review-only in every mode reading | Widened stratum rejects; the case remains visible without automatic mutation |
| Calibration rot after provider/model/schema/prompt/policy/threshold/category drift | Calibration-invalidation catalog; any already-flipped flag reverts to confirmation_first pending re-evaluation | Drift is detected and the flag reverts; stale calibration is never trusted silently |
| Sealed judge-corpus contamination or leakage into maker/implementation-LLM context | Corpus lives outside the repository, the implementation-LLM's context, and the maker workflow; contamination checks confirm no example, label, or identifier crosses that boundary | Contaminated or leaked material blocks the evaluation result from being trusted |
| Flag flip without independent gate evidence | `hybrid_enabled`/`automatic_enabled` default false; each flip requires the named gate passing plus an accountable owner release decision citing that evidence | A flip attempted by code, configuration drift, or a mode-change UI alone fails closed |
| Retroactive batch mutation on mode change or replay | Mode changes and rollback are prospective only; no mode or replay creates, rewrites, closes, or deletes a historical batch/loop/artifact | Historical state is never mutated by a mode change, settings change, or replay |
| Rollback to confirmation-first failing or being conditional | Rollback is instant, unilateral, lossless, and unconditional regardless of flag/gate state | A blocked, delayed, or partial rollback is a defect, not an accepted behavior |
| Judge-corpus content, labels, or predictions leaking into the repository or diagnostics | Evaluation-evidence privacy boundary permits only sanitized aggregate counts and stable non-content identifiers | Corpus content, transcript, or model output in a tracked artifact fails the privacy boundary |
| Semantic manipulation that passes schema validation but is unsafe | ADR-007's deterministic positive constraints and independently held semantic-adversarial suite remain mandatory; schema-valid output is never treated as semantically safe | A schema-valid but semantically unsafe result is rejected before any mutation |
| Sample sizes too small for the claimed 95% lower-bound precision | Per-stratum sample sizes and a lower-bound confidence procedure are declared before evaluation; a point estimate never substitutes for the bound | An underpowered stratum cannot be represented as passing |
| Inferred closure or external communication becoming automatic in any mode | OL-REM-017 never authorizes inferred closure or communicating with another person automatically, in any mode, at any confidence | The action remains user-confirmed regardless of mode or confidence |

No evaluation, corpus, flag flip, or mode is enabled by this threat model. A
failed or untested stratum remains confirmation-only and the product cannot
advertise hybrid or fully automatic mode; failure routes to the product
owner.
