# ADR-008: Policy and state model

- **Status:** Accepted
- **Work item:** P0-WI-11
- **Owner decision:** OWN-03
- **Consumed requirements:** OL-REM-010, OL-REM-017 (disabled mode and gate boundary only)
- **Blocking gates:** G-AUTO, G-AUTO-FULL
- **Decision date:** 2026-07-20

## Context

OpenLoops needs one deterministic local authority for loop facets, deadlines,
typed user commands, and lifecycle transitions. Model output is an untrusted
hypothesis, reminder artifacts are projections, and absence or loss of evidence
is not proof. OWN-03 already fixes the product direction: development and
preview are confirmation-first; hybrid remains unavailable until G-AUTO and
fully automatic remains unavailable until G-AUTO-FULL. This ADR records that
decision without reopening it or implementing either automatic mode.

The executable decision contract is
`contracts/domain/policy-state-boundary.json`. P0-WI-11 implements no policy
runtime, persistent logical schema, reminder adapter, model call, Graph or
Office operation, automation calibration, permission, or product claim.

P0-WI-11 implements no policy runtime.

## Decision

### Independent facets and closed records

`Loop`, `deadline_evidence`, and `transition` are closed logical records. Their
fields map exactly once to ADR-PRIV-001. Unknown, duplicate, missing,
shape-invalid, open-enum, generic-map, unbounded, or readable-content values
reject before encryption and again after authenticated decryption or migration.
ADR-005 still owns encoding, encryption, atomic commit, and rollback recovery;
this ADR closes only the logical catalogs and policy.

The exact loop facets are origin, provenance set, obligation, review flags,
deadline, closure review, reminder, analysis, source, resolution, and confidence.
They remain independent. Primary display precedence is terminal resolution,
possible closure, needs review, overdue, approaching deadline, needs deadline,
candidate, then active. Secondary facets remain visible when a higher label wins.

The following legality rules are mandatory:

1. `candidate|open` has `resolution=none`; `terminal` has exactly one non-none
   resolution. A model or system hypothesis never creates terminal state.
2. `possible|kept_open|evidence_requested` requires an open obligation.
   Candidate and terminal loops project `closure_review_state=none`.
3. `approaching|overdue` requires an open obligation and a resolved operational
   boundary. Candidate and terminal loops do not newly age; terminal loops retain
   deadline evidence and value without continuing an active aging classification.
4. `undated_confirmed` is an explicit durable user choice with no operational
   boundary. It suppresses the current prompt until causally later deadline
   evidence or an explicit user edit.
5. Quarantine requires an encrypted re-fetchable locator and retry/dismiss path
   before cursor advancement. Changed, partial, or unavailable evidence freezes
   evidence-dependent automation and never proves failure or closure.
6. Reminder completion, deletion, absence, conflict, or ambiguity changes only
   the reminder facet. Candidate remains candidate, open remains open, and
   terminal remains terminal unless an explicit lifecycle command applies.
7. Later evidence may add `needs_review` and a validated relation to a terminal
   loop, but it never silently reopens or replaces the resolution.

### Establishment and promotion

Validated explicit outgoing promises, direct incoming requests/questions, and
deterministic explicit attributions can establish open loops under the exact
product-spec evidence, confidence, and ambiguity predicates. Acknowledgement
adds provenance without overwriting requested provenance; promised is added only
when future-act evidence supports it. Soft, low-confidence, or core-ambiguous
results remain candidates. Model confidence alone never promotes a candidate.
History remains review-only and grants no artifact authority. Invitation input
remains unavailable until G-MAIL and G-CAL validate it.

### Deadline authority and aging

Requested, promised, inferred, user-supplied, operative, and reminder deadlines
remain distinct. Precision is one of instant, date, business day, week,
event-relative, soft, or unspecified and never upgrades through aging.

For one atomic action, an explicit user-promised deadline is operative over a
same-message requested deadline while both remain visible. A causally later,
confidently associated user extension or requester directive becomes operative;
a mere date mention does not. Independent actions split. Ambiguous attachment
preserves alternatives and requires review. An unresolved event-relative value
remains unresolved until one event is selected. Soft urgency is scheduled
without an aging boundary and never becomes approaching or overdue.

Date, business-day, and week boundaries use confirmed timezone/EOD/week policy
without changing evidence precision. A timezone, EOD, week, or date-only policy
change previews affected loops and requires confirmation before rewriting an
existing confirmed boundary. `defer_reminder` suppresses only the current prompt
version; it does not mean no deadline, change evidence, or authorize an artifact.

### Multiple hypotheses and keep-open

Several current closure, decline, delegation, or moot hypotheses may coexist.
No new persistent hypothesis text or key is added. Identity is derived from the
target loop, hypothesis kind, ordered validated source references including
their versions, and policy version, using only approved relation and transition
fields.

The loop-level closure facet is a deterministic projection: `possible` if any
unsuppressed current hypothesis exists; otherwise `evidence_requested` if a
request is pending; otherwise `kept_open` if a current hypothesis was explicitly
suppressed; otherwise `none`. Keep-open suppresses only the exact identity. A
causally later source version, different loop, kind, source tuple, or policy
version remains reviewable.

### Typed commands and confirmation

Commands form a closed catalog. Fulfillment requires an open loop, validated
closure evidence, and explicit user confirmation. Decline requires explicit
decline evidence or manual decline confirmation and remains distinct from
fulfillment, dismissal, and mootness. Full responsibility transfer alone yields
`delegated`; shared, assisted, retained, and unclear responsibility stays open.
Withdrawal, supersession, or no-longer-relevant confirmation yields `moot`.
Completed outside email is explicit and fabricates no evidence. Typed dismissal
uses a reason code and never silently means fulfillment.

Candidate loops may be explicitly declined, transferred, made moot, completed
outside email, or dismissed when that command's evidence/manual precondition is
met; the command does not silently promote the candidate first. `closed` from a
closure hypothesis requires an established open obligation. False or invalid
detection is dismissed instead.

Reopening is user-only. It appends a reversal transition, restores `open` with
`resolution=none` and closure review none, adds `needs_review`, retains prior
history/evidence, and re-evaluates later evidence. A later message alone never
reopens. Remap version-checks both loops and atomically detaches/attaches the
relation; duplicate correction transfers only validated non-conflicting history,
terminalizes the loser as dismissed duplicate, and leaves conflicts reviewable.

### Idempotency, staleness, and correction

Automated replay is keyed by validated source identity/version, policy version,
and target loop. User-command handling first looks up the opaque command key:
same key and canonical payload returns the original result without mutation;
same key and different payload is a collision. An unseen key then checks the
expected loop version; mismatch is a visible refresh conflict. Only after all
preconditions pass does one atomic transaction append one transition and
increment the loop version. Old keys cannot re-terminal a reopened generation.

A correction to the current loop and an optional durable future rule are
separate explicit confirmations. Future rules are typed settings, prospective
by default, never hidden model training or profiling. Optional bounded replay is
review-only and never mutates historical reminders automatically. Free-text
correction and rationale persistence are prohibited.

### Reminder and automation boundary

ADR-008 authorizes deterministic local facet transitions and proposals only.
A terminal local transition commits independently of any optional reminder
operation; remote failure cannot roll it back. Candidate or terminal state may
revoke an unissued local proposal, but never deletes or reconciles an extant
artifact automatically.

ADR-009 exclusively owns reminder adapters, Graph calls and permissions,
operation ledgers, remote correlation, ambiguous writes, direct edits,
completion/deletion reconciliation, ownership, conflict, and calendar behavior.
ADR-011 exclusively owns OL-REM-017 execution, eligibility strata, evaluation,
feature flags, G-AUTO/G-AUTO-FULL evidence, and rollback to confirmation-first.
ADR-008 consumes OL-REM-010 and OL-REM-017 only to preserve their disabled mode
and gate boundary; it neither implements nor completes either requirement.
Neither accepted OWN-03 nor this ADR enables hybrid or automatic behavior.

### Privacy and failure behavior

Only the ADR-PRIV-001 fields for loop, deadline evidence, and transition may be
represented. Human-readable task/person/client/source/deadline text, free-text
reasons, rationales, raw Microsoft identifiers or URLs, prompts/model material,
generic maps, extension bags, and unbounded values are prohibited. Diagnostics
are fixed non-content codes and bounded counters only; they contain no facet
history, source identifier, temporal value, or command payload.

Invalid state, stale input, missing evidence, unavailable secure persistence,
rollback suspicion, or any deferred-adapter condition fails closed with zero
Graph, Office, reminder, lifecycle, or provider mutation. Existing state is
preserved for visible recovery where its authenticated record is valid.

## Consequences

P0-WI-11 closes the deterministic policy decision without shipping it. OWN-03
remains accepted, ADR-008 alone advances from planned to accepted, and G-AUTO
and G-AUTO-FULL remain unrun. Every named AC and scenario remains unpassed. No
capability, dependency, permission, support row, persistent record, network
origin, account, provider, reminder, or product claim is enabled or advertised.

## Verification

The checker must pin the complete manifest and assert exact requirement, AC,
scenario, gate, record, facet, legality, establishment, deadline, hypothesis,
command, confirmation, idempotency, stale/reopen, correction, privacy, deferred
ownership, cross-contract, runtime, and claim inventories. Synthetic mutations
must reject unknown fields/enums, illegal Cartesian facet combinations,
inferred terminal transitions, reminder-driven closure, stale/colliding commands,
overbroad keep-open suppression, deadline precision upgrades, terminal aging,
automatic replay/rules, readable fields, Graph/Office authority, completed ACs
or scenarios, passed gates, advertised modes, and cross-ADR ownership drift.
A fresh state/privacy/adversarial panel is required before closure; its prompts,
transcripts, and model output are not repository evidence.
