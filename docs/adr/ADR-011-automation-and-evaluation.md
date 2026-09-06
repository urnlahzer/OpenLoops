# ADR-011: Automation and evaluation

- **Status:** Accepted
- **Work item:** P0-WI-14
- **Owner decisions:** OWN-03
- **Blocking gates:** G-AUTO, G-AUTO-FULL
- **Decision date:** 2026-07-21

## Context

OWN-03 already selects hybrid as the full-MVP new-install default reminder
mode; development, test mode, and limited preview stay confirmation-first
until G-AUTO passes, and fully automatic mode remains unavailable until
G-AUTO-FULL passes. ADR-008 records the deterministic local policy authority
and explicitly defers "OL-REM-017 execution, eligibility strata, evaluation,
feature flags, G-AUTO/G-AUTO-FULL evidence, and rollback to confirmation-
first" to this ADR; it consumes OL-REM-010 and OL-REM-017 only to preserve
their disabled mode and gate boundary and neither implements nor completes
either requirement. ADR-009 records the reminder-adapter operation protocol
and defers "mode semantics, eligibility, evaluation, flags, and rollback" to
this ADR the same way. This ADR closes that deferral: it records the three
closed mode definitions and their exact eligibility strata, the feature-flag
topology, the sealed evaluation-corpus governance, the calibration-
invalidation rule set, rollback behavior, and the evaluation-evidence privacy
rule without running an evaluation, creating a corpus, flipping a flag, or
enabling a mode.

The executable contract is `contracts/automation/evaluation-boundary.json`.
P0-WI-14 performs no evaluation, creates no corpus, flips no flag, enables no
mode, and completes no acceptance criterion or scenario.

## Decision

### Mode catalog and eligibility strata

The three OL-REM-017 modes are a closed catalog and remain exactly as
follows:

- `confirmation_first`: every reminder creation and every evidence-driven
  service update to a Microsoft artifact requires an explicit user action;
  read-only reconciliation and local review-state updates still run
  automatically.
- `hybrid`: automatically creates only live, narrow-category cases allowed by
  G-AUTO — explicit promise, explicit directly addressed request, or explicit
  deterministic attribution — with a valid resolved deadline, high calibrated
  confidence, deterministic identity, and no quote/delegation/coreference
  ambiguity. It automatically updates only OpenLoops-owned artifact fields
  when later evidence deterministically changes them, no user
  override/conflict exists, and the update class passed G-AUTO. Every other
  mutation is proposed for confirmation.
- `automatic`: after explicit opt-in/risk disclosure and G-AUTO-FULL, it
  automatically creates/updates every live established loop in the user's
  enabled non-ambiguous categories whose exact category/confidence/
  deadline/update stratum independently passed G-AUTO-FULL. Candidate,
  ambiguous, identity-ambiguous, delegation-ambiguous, quote-ambiguous,
  undated, historical/backfill, and stale-write cases always require review
  in every mode; no mode reading ever admits them to automatic mutation.

A mode change is prospective only. No mode silently creates or rewrites a
historical batch, closes a loop, deletes an artifact, or communicates with
another person. Inferred closure and external communication are never
automatic in any mode, at any confidence level, in any category.

### Feature-flag topology

`hybrid_enabled` and `automatic_enabled` are independent flags that default
to `false`. Neither flag flips by code, configuration drift, a mode-change
setting, or any other implicit path. `hybrid_enabled` may flip to `true` only
after G-AUTO passes and an accountable owner records an explicit release
decision citing that gate evidence; `automatic_enabled` may flip to `true`
only after G-AUTO-FULL passes plus the same owner release decision plus the
user's explicit opt-in/risk acknowledgement. A user selecting a mode in
settings changes only the user's preference; it never flips a flag on its
own, and the effective mode is always the narrower of the user's preference
and the flags currently permitted by passed-gate evidence.

### Sealed evaluation-corpus governance

The release-judge corpus is wholly synthetic, is versioned, and is sealed
outside the repository, outside the implementation-LLM's context, and
outside the maker workflow, matching implementation-plan section 8.2. At
least 30% of held examples are difficult/ambiguous and at least 20% contain
quoted history; the corpus is split by conversation family, never by message
row, and no family straddles a split. Per-stratum positive and negative
sample sizes are declared before evaluation and are large enough to support
the claimed lower-bound confidence procedure; a point estimate alone never
substitutes for a declared lower bound. Corpus content, version, and hashes
are immutable once sealed; a maximum tuning-attempt budget is pinned before
evaluation begins and re-tuning past that budget requires a fresh corpus
version, never a silent extra round against the same sealed data.
Contamination checks confirm that no sealed-corpus example, label, or
identifier reaches a public development fixture, the repository, or any
maker/implementation-LLM context. "Zero observed unintended mutations in at
least 10,000 events" is reported strictly as a finite-sample observation, not
a guarantee, matching product-spec section 10 and implementation-plan
section 8.2.

### Calibration invalidation

Provider, model digest/label, schema, prompt, policy, review threshold, or
category drift invalidates the corresponding calibration. An invalidated
calibration reverts any already-flipped flag to `confirmation_first` pending
re-evaluation; it never continues to operate on stale calibration, and no
drifted stratum may be represented as still passing. Only an explicit,
review-only replay is permitted after drift; no automatic terminal, Graph,
Office, or reminder mutation follows a drift-triggered reversion.

### Rollback to confirmation-first

The user can fall back to `confirmation_first` instantly, unilaterally, and
without qualification, regardless of the current flag state, gate status, or
any pending evaluation. Rollback is prospective and lossless: it never
mutates, recreates, or deletes a historical batch, an existing reminder
artifact, or a prior loop transition, and it never requires additional
confirmation, delay, or owner approval to take effect.

### Evaluation-evidence privacy boundary

No corpus content, label, prediction, prompt, transcript, or model output
enters the repository, a package, or a diagnostic artifact. Only sanitized
aggregate counts and stable non-content identifiers may appear in reported
results. Reported metrics are limited to the implementation-plan section 8.2
list — macro and per-stratum recall/precision/F2, atomic split/merge errors,
evidence span F1, attribution, deadline normalization, dedup, association,
review routing, calibration, latency/cost, unintended mutations, and privacy
canaries — and none of them implies persistent user profiling.

## Consequences

P0-WI-14 accepts only this disabled decision contract. ADR-011 alone advances
from planned to accepted; ADR-012 and ADR-013 remain planned. No mode is
enabled, no evaluation is run, no corpus is created, and no flag is flipped.
G-AUTO and G-AUTO-FULL remain unrun, and every acceptance criterion or
scenario that depends on either gate remains unpassed. ADR-008 and ADR-009's
deferred wording is satisfied without either ADR being reopened; ADR-011
alone owns OL-REM-017 execution, eligibility strata, evaluation, feature
flags, G-AUTO/G-AUTO-FULL evidence, and rollback to confirmation-first, and
ADR-008 and ADR-009 continue to consume OL-REM-010/OL-REM-017 only as a
disabled boundary they do not complete.

## Verification

The deterministic checker pins the complete manifest and accepted ADR, checks
the exact requirement/AC/scenario/gate/source/input-authority inventories,
validates the closed mode, eligibility-stratum, flag, corpus-governance,
calibration-invalidation, rollback, and privacy catalogs, and reconciles
ADR-007's drift rules, ADR-008's deferral wording, ADR-009's hybrid strata,
the governance registry, the support matrix, and build-skeleton inactivity.
Synthetic mutations must reject a default-enabled flag, a flag flip without
gate evidence, any widened eligibility stratum (a medium-confidence auto-
create, an ambiguous-category auto-eligibility, an undated auto-create), a
historical-batch auto-mutation, a retroactive mode change, an in-repository
corpus, a removed corpus hash/version, a removed tuning budget, a per-stratum
threshold below the 95% lower bound, a weakened zero-mutation observation, a
drift that preserves calibration, a rollback that mutates history, persisted
evaluation content/labels/predictions, a gate-passed or capability-enabled
claim, fresh-checker preapproval, and additive documentation contradiction.
Fresh security, evaluation-integrity, governance, and adversarial judges are
required for closure; their prompts, transcripts, and output are not
repository evidence.
