# Reminder adapter boundary

**Status:** ADR-009 decision contract accepted; runtime and all named gates remain inactive.

The primary safety property is that a local intent cannot become an unbounded,
duplicated, privacy-leaking, or lifecycle-authoritative Microsoft write. A
pending encrypted operation must exist before the request, bind every authority
and value version, and prevent cursor advancement while its outcome is unknown.

| Threat | Required control | Fail-closed result |
|---|---|---|
| Crash or retry duplicates an artifact | Durable operation first; proven remote marker; complete exact-scope reconciliation | No retry when ambiguous, incomplete, unsupported, removed, or changed |
| Readable content or identifiers enter state | Exact ADR-PRIV-001 records; HMAC commitments; manual drafts memory-only | Reject before persistence; fixed non-content diagnostic |
| Remote direct edit is overwritten | Refetch, compare versions and owned-field digests, exact partial field mask | Visible conflict; zero PATCH |
| Reminder controls loop lifecycle | ADR-008 remains lifecycle authority | Completion/deletion changes only reminder/source state |
| Calendar action emails or affects another person | Self-only, zero attendees, no response/online meeting/mail; invitations evidence-only | Calendar remains unavailable until G-CAL proof |
| Review link becomes a credential | Random non-secret handle only; ADR-010 authenticated resolution | Link omitted or fixed safe fallback |
| Manual draft survives a crash or failed create | No draft persistence; atomic source/link/loop only after one proven artifact | Re-entry and reconfirmation; no durable manual loop |
| Rollback, account, authorization, or version drift replays intent | Bind all generations and expected versions in the operation commitment | Zero request and visible recovery |

No adapter, permission, record, call, mutation, support row, AC, scenario, or gate
is enabled by this threat model.
