# ADR-009: Reminder adapters

- **Status:** Accepted
- **Work item:** P0-WI-12
- **Owner decisions:** OWN-03, OWN-04, OWN-05, OWN-07
- **Blocking gates:** G-ID, G-MAIL, G-TODO, G-CAL, G-ADDIN, G-STATE,
  G-PRIV, G-AUTO, G-AUTO-FULL, G-SEC-AUDIT, G-RELEASE
- **Decision date:** 2026-07-21

## Context

OpenLoops projects a local loop into an optional Microsoft reminder artifact,
but the projection is not lifecycle authority. OWN-03 through OWN-05 and OWN-07
already choose confirmation-first behavior, To Do-first preview, calendar parity
for complete MVP, and privacy-preserving local state. This ADR records those
approved decisions without enabling an adapter or assuming an unrun Microsoft,
state, privacy, automation, add-in, or release gate.

The executable contract is `contracts/reminder/adapter-boundary.json`. It defines
closed logical shapes for later implementation; P0-WI-12 accepts none of those
records for persistence and makes no Graph or Office request.

## Decision

### To Do boundary

To Do product use requires the disabled `Tasks.ReadWrite` feature permission,
explicit user enablement, and G-ID/G-TODO/G-PRIV. `Tasks.Read` remains a
validation-only diagnostic. Ordinary bounded, complete list and task enumeration
is the launch reconciliation baseline; delta is not a dependency. The user must
select an eligible owned list. OpenLoops never guesses a built-in or Flagged
Email list, claims shared-list or personal-account support, substitutes Calendar
after denial, or uses application permissions.

### Durable operation before remote request

Every remote mutation begins with a pending encrypted operation-ledger record
committed under ADR-005 before a request. Its keyed commitment binds the account,
adapter, collection, loop, source generation, operation kind and exact field
mask, expected local and remote versions, transient intended-value digests,
policy and authorization generations, and explicit-recreate generation.

A duplicate external invocation of the same key and intent returns the prior
state with no new request whenever no bounded retry is currently eligible; the
same key with different intent is a collision. This replay path is distinct
from an eligible retry: replay never issues a request even while a retry
remains eligible, and an eligible retry never reuses the replay path to skip
its own durability step. Stale versions, account or authority change, rollback
suspicion, or unavailable secure state cause zero requests. Pending, started,
ambiguous, reconciling, and user-assisted operations block a dependent
synchronization cursor. Only endpoint-independent proof that nothing was sent
permits a serialized, durably recorded eligible retry that commits the next
attempt before its one bounded request, with at most three attempts total.

After restart, a detected-loop payload is reconstructed transiently from current
evidence and policy and must match the committed operation key. Manual draft text
is never persisted and cannot resume automatically; the user re-enters and
reconfirms it. A local terminal loop transition commits independently of any
remote result and is never rolled back by reminder failure.

### Remote correlation and ambiguous writes

The local operation key is not remote idempotency. Marker support is unproven
until an adapter gate proves an endpoint field, write, query, paging, visibility,
removal, and duplicate contract. A marker is recoverably derived as base32url of
SHA-256 over a domain plus the encrypted opaque operation ID. Only its HMAC is
persisted; the raw marker is transmitted solely in the proven opaque field and
never in title, body, due, URL, or link text.

An ambiguous create performs bounded complete enumeration of the exact account,
adapter, and collection. Complete zero, complete one, complete many, and
incomplete results are distinct. One match links it; many or incomplete results
require user assistance. Missing, removed, or unsupported markers never trigger
automatic retry. Explicit recreate requires confirmation, a duplicate warning,
a new recreate generation, and a new operation identity.

### Field ownership, direct edits, and conflicts

For detected email or invitation loops, title and body begin as service
projections; the operative deadline supplies due. A direct remote edit makes the
edited field user-owned. A due edit creates the approved
`reminder_due_override` and requires an explicit keep-override or update-reminder
choice before a later service change. For a user-authored Microsoft artifact,
title, body, and due are always user-authoritative.

Before any update, the adapter refetches the artifact and compares the exact
owned-field digests and version. Changed or stale state becomes a visible
conflict; no PATCH occurs until adapter-specific conditional writes are proven.
Any future patch contains only the exact service-owned field mask and preserves
all user-owned sibling fields.

ADR-PRIV-001 approves no persisted reminder-time value, digest, ownership, or
override field. Reminder time is therefore recomputed transiently from the
approved operative due value and encrypted lead rule; only its transient digest
participates in the operation-key HMAC. An independent remote reminder-time edit
is shown as unmanaged/review and is not overwritten automatically pending an
approved privacy revision.

To Do completion for a detected loop yields `completed_needs_evidence`; it never
closes the loop. Manual-source completion enters an explicit confirmation flow.
Deletion makes the reminder missing or the manual source unavailable and proves
neither closure nor failure. OpenLoops never automatically deletes an artifact.

### Manual artifacts

A manual loop is durable only after an explicit user action and a proven single
Microsoft artifact. In one atomic transaction, OpenLoops creates exactly one
user-authored source reference, loop/source relation, and reminder link bound to
the same verified account, adapter, collection, and artifact locator. Cancellation
or definitive failure leaves no durable manual loop or readable draft. Ambiguity
is reconciled only through a proven marker or explicit user selection and never
by automatic recreation.

### Calendar and invitations

The least delegated Calendar permission and endpoint contract remain unresolved
behind G-CAL; this ADR requests no guessed scope. A reminder event, if later
proven, is application-owned, self-only, and has zero attendees, no online
meeting, no response request, and no invitation/update/cancellation mail side
effect. An invitation event is evidence only: OpenLoops never responds, changes
attendees, updates, deletes, or cancels it. Calendar has no completion operation.
To Do-first preview makes no complete-MVP Calendar or invitation claim.

### Review-link and privacy boundary

A review link contains one random, non-secret opaque loop handle and grants no
authority. ADR-010 and G-ADDIN own activation, authenticated per-user and
account-bound resolution, and client behavior. Tokens, secrets, Microsoft or
account identifiers, mailbox content, host/port authority, and raw evidence
links are prohibited.

The only closed logical record names are `reminder_link`, `operation_ledger`,
and `user_authored_artifact_ref`, with the exact ADR-PRIV-001 fields in the
manifest. Unknown fields, open enums, generic maps, unbounded values, readable
title/body/due/draft/reminder text, raw identifiers/URLs/markers, request or
response bodies, free text, and prompt/model material reject. Diagnostics are
fixed non-content codes and bounded counts only.

## Consequences

P0-WI-12 accepts only this disabled decision contract. ADR-009 alone advances
from planned to accepted. Adapter runtime, logical or persistent records,
permissions, dependencies, calls, mutations, support claims, capabilities,
acceptance criteria, scenarios, and gates remain empty, inactive, or unrun.
ADR-010 owns review-link activation, ADR-011 owns automation modes/evaluation,
ADR-013 owns self-email, and adapter gates own the exact Microsoft behavior.

## Verification

The deterministic checker pins the complete manifest and accepted ADR, checks
exact inventories and the privacy-record mapping, validates the closed catalogs,
operation ordering, correlation outcomes, ownership matrix, reminder-time trap,
manual atomicity, Calendar and review-link prohibitions, and reconciles all prior
contracts. Synthetic mutations must reject raw or unrecoverable markers,
identity-by-readable-field, incomplete-as-zero, blind retry/recreate, draft
persistence, invented reminder-time state, user-owned overwrite, stale PATCH,
Calendar attendees or invitation mutation, authoritative review links, generic
operation kinds, incomplete operation keys, runtime activation, claims, and
additive documentation contradictions. Fresh privacy, protocol, and adversarial
judges are required for closure; their prompts, transcripts, and output are not
repository evidence.
