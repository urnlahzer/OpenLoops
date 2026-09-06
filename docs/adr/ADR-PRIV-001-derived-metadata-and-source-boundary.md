# ADR-PRIV-001: Derived metadata and source boundary

- **Status:** Accepted
- **Date:** 2026-07-19
- **Work item:** P0-WI-02
- **Owner decisions:** OWN-06, OWN-07
- **Blocking gates:** G-STATE, G-PRIV, G-SEC-AUDIT

## Context

OpenLoops must preserve enough state to reconcile loop evidence, user-authored
Microsoft artifacts, retries, and user settings without creating a shadow
mailbox. OWN-06 approves local encrypted storage only after security validation.
OWN-07 requires human-readable source text to remain in Microsoft 365 and be
retrieved on demand. These are approved product decisions, not questions this
ADR reopens.

All allowed state is private or sensitive. Keyed digests, locators, confidence
buckets, timing, counters, and relationships can disclose behavior even when
they cannot reconstruct source text. Encryption does not make an otherwise
unapproved field permissible.

## Decision

`contracts/privacy/persistence-boundary.json` is the authoritative, closed-world
privacy allowlist for persistent application state. Its record types, fields,
semantic categories, sources, lifecycle transitions, retention policies,
operations, and cross-record invariants are exact. Every field inherits a complete classification, purpose, necessity,
threat, disclosure, protection, retention, deletion, reset, export,
reconstruction, and source description from its field group. A new record,
field, semantic category, source variant, diagnostic event, or generic extension bag
requires an explicit reviewed revision of this ADR and manifest.

This Phase 0 manifest is not a runtime serialization schema and approves no
runtime values. Before durable state can exist, ADR-005 and each dependent
domain ADR must bind every allowlisted field to an exact logical type, required
and null shape, enum/version catalog, locator variant, algorithm and length,
container limit, and numeric or text bound. The resulting typed logical records
must be validated immediately before application-level authenticated encryption
and again immediately after decryption or migration, before any consumer sees
them. Those validators must reject unknown or duplicate keys, record/schema
versions, enum values, type mismatches, silent field dropping, and unbounded
strings, bytes, maps, blobs, or metadata bags. Validating ciphertext shape alone
is insufficient. Until ADR-005 and G-STATE pass, durable state remains disabled;
this ADR must not be used as a substitute runtime validator.

The only source-reference variants are:

1. `email_evidence`, a typed component/relation locator with Unicode ranges and
   keyed anchors, gated by G-MAIL and G-PRIV;
2. `calendar_invitation`, an invitation-email/event/response relation, gated by
   G-CAL, G-MAIL, and G-PRIV; and
3. `user_authored_microsoft_artifact`, the manual-origin exception for a
   user-created To Do task or calendar event.

For manual origin, exactly one user-authored artifact is authoritative. Its
title, body, and due fields are always `user_authoritative`; model or system
output cannot overwrite them. OpenLoops may store locators, keyed change
digests, versions, ownership codes, and state, but never copies the readable
title, body, or due phrase. Artifact deletion makes the source unavailable and
the loop open for review; it never proves closure. Email-origin loops have email
evidence and no authoritative artifact. Calendar-origin loops remain disabled
until all three named gates pass.

The manifest fixes reachable, state-dependent retention transitions:
candidate/open loop lineage uses `active_loop_lineage` and moves to
`terminal_loop_lineage` on terminal resolution; pending/ambiguous operation
ledger entries use `unresolved_operation` and move to `completed_operation_90`
only after reconciliation proves completion. Observations use the configured
1–365-day history window plus exactly 15 days, with only required active
references pinned; candidate/open lineage lasts until terminal resolution,
explicit deletion, or disconnect; terminal lineage defaults to 30 days and is
configurable from 0 through 365; completed operations last exactly 90 days;
pending or ambiguous operations last until reconciled or explicitly abandoned;
and counters are rolling 30-day aggregates that can be reset independently.
The product owner clarified on 2026-07-20 that an unresolved recoverable
quarantine is a required active reference: only its approved encrypted
message-observation fields and content-free health fields remain pinned while
retry, dismissal, or recovery is unresolved. Retry success, dismissal, or
recovery completion atomically deletes the related health record before the
observation returns to ordinary retention; the observation is deleted
immediately when that window has already elapsed. Account disconnect atomically
deletes both local records regardless of age and performs zero Graph mutations.
No `quarantine_ref` may dangle. This exception
never permits readable mailbox content, raw identifiers, URLs, response data,
or error text.

Delete, reset, and disconnect affect only application-controlled local state.
They perform no Graph mutation and never delete or bulk-modify Microsoft
artifacts. SQLite/page/WAL compaction and key retirement are best effort, so the
product makes no physical-erasure guarantee for storage media, backups,
snapshots, or crash residue. Product-state and metric export are disabled.
Diagnostics have an empty allowlist; any future diagnostic export needs an
exact content-free schema plus ADR-012, G-PRIV, G-RELEASE, and G-SEC-AUDIT.

OAuth tokens, refresh material, state-encryption keys, HMAC keys, provider keys,
pairing secrets, and session secrets are not database records. They may exist
only in memory or an operating-system protected secret store as governed by
ADR-005 and ADR-010. Plaintext fallback is prohibited.

The allowlist prohibits mailbox subject/body/preview/quote text; generated
summaries or explanations; task title/body and source deadline phrases;
participant names, addresses, display names, attachment content or names, and
link text or URLs; raw Microsoft, tenant, account, workspace, or object IDs;
raw Graph cursors, URLs, requests, or responses; credentials and authorization
material; prompts, model requests, responses, outputs, rationales, embeddings,
or vectors; test samples, labels, predictions, and free-text corrections; and
human-readable exceptions, logs, transcripts, browser caches, diagnostic
bundles, source maps, screenshots, or HAR files.

## Consequences

The manifest and deterministic checker make privacy-boundary expansion visible
and fail closed. ADR-005 now fixes the envelope, cryptographic, secret,
transaction, recovery, and schema-validation mechanics, but dependent domain
ADRs still owe exact value catalogs and the runtime still owes executable
pre-encryption/post-decryption tests. The cost is deliberate schema work for every new state need and limited
post-hoc debugging. Residual risks remain from same-user or administrator
access, dependencies, endpoint compromise, paging, backups, snapshots, crash
residue, and correlation through ciphertext size, timing, locators, digests, or
state transitions. ADR-005 and the security audit must address those risks;
this ADR does not claim that G-STATE, G-PRIV, or G-SEC-AUDIT passed.

No capability is enabled or advertised and no product acceptance criterion is
completed by this decision.
