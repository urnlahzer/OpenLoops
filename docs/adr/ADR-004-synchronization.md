# ADR-004: Bounded synchronization and recoverable cursor advancement

- **Status:** Accepted
- **Date:** 2026-07-20
- **Work item:** P0-WI-07
- **Owner decisions:** OWN-05, OWN-09
- **Requirements:** OL-GOV-001, OL-AUTH-004, OL-SYNC-001–013, OL-NFR-001, OL-NFR-003–005, OL-NFR-007, OL-NFR-012
- **Blocking gates:** G-ID, G-MAIL, G-TODO, G-CAL, G-STATE, G-PRIV

## Context

Microsoft Graph message delta is folder-scoped state reconciliation, not a
complete event log. It returns opaque continuation URLs and can coalesce or omit
intermediate states. Polling cannot prove instant delivery, careful reading,
sent delivery, or observation of an item that enters and leaves scope between
cycles. OWN-09 selects polling plus reconciliation and visible freshness.
OWN-05 keeps invitation behavior behind both G-MAIL and G-CAL.

## Decision

`contracts/synchronization/mail-sync-boundary.json` is the closed Phase 0
synchronization contract. It activates no runtime, scheduler, transport,
checkpoint, dependency, permission, or network origin.

Mail work is one non-overlapping, single-flight delta/reconciliation job per
bound account and selected folder. Inbox and Sent Items are defaults; additional
owned folders require explicit opt-in under ADR-003. Each selected folder and
future reminder collection has its own encrypted, account/collection/query-bound
checkpoint. Full `nextLink` and `deltaLink` values are opaque encrypted locators:
they are never reconstructed, edited, reused across a binding, logged, exported,
or persisted in plaintext. Exact fields, immutable-ID headers, cursor-error
signatures, and operational budgets remain G-MAIL/ADR-006 contracts.

Every returned page is an atomic unit. Its continuation and keyed observation
identities are staged; every item must reach a durable safe result or durable
recoverable quarantine; only then may the checkpoint be replaced in the same
transaction. `last_complete_at` advances only when a committed `deltaLink`
closes the round. A crash before commit replays; idempotency prevents duplicate
transitions or mutations. Whole-page parse/schema/transport failure blocks
advancement. Ambiguous external writes remain under ADR-009/011 and are never
blindly retried.

Initial/backfill scans analyze already-read inbound messages and saved sent
copies into one noninterruptive history-review batch with zero automatic
reminders. An initially unread item waits for a later read observation. After
the committed baseline, unread-to-read and first-observed-already-read inbound
items may be analyzed once. A saved sent copy may be analyzed once, but drafts
and compose activity never activate loops. `isRead` is a trigger, not evidence
of attention; the product says “sent copy observed,” never “delivered.”

Cycles and retries are finite and prioritize incremental work. Valid
`Retry-After` is never shortened; absent or malformed values use bounded
exponential backoff with positive jitter. Exact page/item/time/retry limits,
reconciliation cadence, jitter distribution, circuit thresholds, and supported
load remain G-MAIL experiments. Exhausted items cannot be discarded as
`failed_bounded`: before cursor advance they receive an ADR-PRIV-001-approved
encrypted re-fetch locator and content-free failure/retry state, or the cursor
stays blocked.

The product owner approved the narrow privacy clarification on 2026-07-20. An
unresolved quarantine pins only approved encrypted observation and content-free
health fields. Retry success, dismissal, or recovery atomically deletes the
related health record before the observation returns to ordinary retention; the
observation is deleted immediately if that window elapsed. Disconnect atomically
deletes both local records regardless of age with zero Graph mutation, and no
quarantine reference may dangle. It never permits readable mail, raw IDs, URLs, payloads,
headers, errors, prompts, or model output.

Reconciliation remains within selected folders, the visible 1–365-day history
window, and finite cycle bounds. Tombstones mean a source availability change,
not closure or global deletion. Folder identity is never inferred from a display
name. A missing/deleted selected folder stops that collection, deletes its local
checkpoint, discloses coverage loss, and requires explicit remapping. Replay is
bounded and visible, reuses the same keys, broadens no scope, and sends newly
detected historical results only to review with zero automatic reminders.

Known unrecoverable cases include arrive/read/leave between observations,
unsaved or immediately moved/deleted sent copies, unvalidated alternate sent
folders or send-as/on-behalf-of behavior, and time outside the selected history
window. No complete, immediate, background-while-inactive, or delivery claim is
allowed.

## Consequences

- ADR-005 still owns encryption and atomic durable-state implementation.
- ADR-006 still owns exact fields, immutable identity, moves/copies, and links.
- ADR-009/011 still own reminder mutation and ambiguous-write reconciliation.
- G-MAIL must validate paging, duplicates, ordering, tombstones, coalesced
  states, cursor errors, folder moves/deletion/rename, throttling, load, and
  known-loss disclosures using only synthetic disposable-tenant content.
- No gate, capability, support row, or acceptance criterion advances here.

## Verification

P0-WI-07 is graded by exactly these twelve checks:
P0-SYNC-INVENTORY-001, P0-SYNC-CHECKPOINT-001, P0-SYNC-DELTA-001,
P0-SYNC-ELIGIBILITY-001, P0-SYNC-TRANSACTION-001,
P0-SYNC-RESILIENCE-001, P0-SYNC-COVERAGE-001, P0-SYNC-REPLAY-001,
P0-SYNC-SCHEDULING-001, P0-SYNC-CROSS-CONTRACT-001,
P0-SYNC-CLAIMS-001, and P0-SYNC-FRESH-CHECKER-001.
