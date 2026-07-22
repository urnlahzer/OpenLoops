# Graph mail content and synchronization threat model

**Status:** ADR-003/ADR-004/ADR-006/ADR-007 decision-contract-only model;
runtime, Graph transport, and all named gates remain inactive.

## Boundary

This model covers the future path from delegated `Mail.Read` authority
(`contracts/identity/permission-boundary.json`, ADR-003) through bounded
per-folder delta polling and reconciliation
(`contracts/synchronization/mail-sync-boundary.json`, ADR-004), the immutable
evidence identity and Unicode/HTML canonicalization boundary
(`contracts/evidence/identity-boundary.json`, ADR-006), and the bounded
transient model projection a validated message may contribute
(`contracts/model/provider-boundary.json`, ADR-007). No adapter, scheduler,
checkpoint, Graph request, or model transmission exists. `Mail.Read` is
mailbox-wide delegated authority; OpenLoops policy nevertheless processes only
Inbox and Sent Items by default and additional owned folders after explicit
per-folder opt-in, and the consent UI must disclose that mismatch before
enablement (ADR-003, OL-SYNC-005). Mail detection, evidence, and all
dependent capabilities remain disabled behind G-MAIL, G-PRIV, and (for model
projection) G-MODEL. External mail content is always untrusted input and
cannot authorize a tool, scope change, or mutation (product-spec section 4).

## Threats and required controls

| Threat ID | Threat | Required control | Fail-closed result |
|---|---|---|---|
| TM-MAILCONTENT-001 | Over-fetch beyond configured folders or fields, using the mailbox-wide `Mail.Read` grant to read outside the configured processing scope | Folder/field minimization is an application policy control, not an OAuth restriction; requests are limited to Inbox/Sent Items plus explicit per-folder opt-in and the exact ADR-006/G-MAIL selected-field set (ADR-003, ADR-004) | A request outside the configured folder or field set is never issued; folder configuration drift blocks that folder's processing rather than silently widening scope |
| TM-MAILCONTENT-002 | Consent disclosure mismatch: the user is not told that `Mail.Read` grants mailbox-wide reading even though only configured folders are processed | OL-SYNC-005/ADR-003 require the consent UI to disclose the mailbox-wide grant versus the actual configured processing scope before enablement | Enablement is blocked until the disclosure is shown and an explicit folder opt-in is recorded; no implicit or bundled consent activates processing |
| TM-MAILCONTENT-003 | Mail content (subject, body, preview, quote text, participant text, attachment names, link text/URLs) persists past job completion | ADR-PRIV-001 prohibits persisting any of these fields; only typed locators, keyed digests, and structured temporal/state values may persist (ADR-005/ADR-006); transient canonical projections are cleared after the job | A content field reaching a persisted record is rejected by the pre-encryption validator; G-PRIV canary scans must find zero occurrences in application-controlled durable, diagnostic, or log artifacts |
| TM-MAILCONTENT-004 | Coalesced polling states are misrepresented as a stronger claim than observed, e.g. "read carefully" or "delivered" when only an `isRead` transition or a saved-sent copy was observed | `isRead` is a trigger, not evidence of attention; the product says "sent copy observed," never "delivered" (ADR-004, OL-SYNC-003/004) | No delivery, attention, or completeness claim is ever rendered; only the exact observed-transition language is permitted |
| TM-MAILCONTENT-005 | Cursor expiry or an invalid `deltaLink` causes silent, undisclosed data loss instead of a bounded, visible rescan | A cursor-error signature triggers bounded resynchronization within the configured 1–365-day history window and reuses the same keys; OL-SYNC-002/008/012 (ADR-004) | An invalid cursor without a completed bounded rescan freezes cursor advancement and discloses the coverage loss; it never silently resumes as if nothing was missed |
| TM-MAILCONTENT-006 | A quarantined item retains readable mail text, a raw Graph ID, a raw URL, a header, or error text instead of the approved encrypted/content-free fields | ADR-PRIV-001 and ADR-004 fix quarantine to exactly one approved encrypted observation field plus content-free health fields; no readable mail, raw ID, URL, header, response, or error text is permitted | Any additional field is rejected before encryption; G-PRIV canary scans must find zero occurrences |
| TM-MAILCONTENT-007 | A quarantine reference dangles or leaks its locator after retry, dismissal, recovery, or disconnect | Retry success, dismissal, or recovery atomically deletes the related health record before the observation returns to ordinary retention; disconnect atomically deletes both records regardless of age with zero Graph mutation; no `quarantine_ref` may dangle (ADR-PRIV-001, ADR-004) | A dangling or unresolved quarantine reference blocks cursor advancement; incomplete cleanup keeps the affected checkpoint disabled rather than silently dropping the reference |
| TM-MAILCONTENT-008 | Hostile HTML, a link, or an embedded instruction in mail content attempts prompt injection: selecting data, issuing a network call, altering policy/scope, or authorizing a mutation | Mail content is always untrusted input (product-spec section 4); ADR-006 canonicalizes it into an inert WHATWG-fragment projection with no remote-resource fetch, no `href` read, and no execution; ADR-007 confines any model use to a bounded, schema-validated, semantic-adversarial-checked projection with no tool surface | An instruction embedded in mail content is treated only as inert data; it can never select data, call a network origin, change policy/scope, or cause a mutation |
| TM-MAILCONTENT-009 | Throttling (429/5xx) drives starvation: a shortened backoff, an ignored `Retry-After`, or one account/folder blocking another | Valid `Retry-After` is never shortened; absent or malformed values use bounded exponential backoff with positive jitter; each account/folder is one non-overlapping, single-flight job (ADR-004) | A throttled folder or account backs off independently without blocking other accounts or folders, and `Retry-After` is never shortened or ignored |
| TM-MAILCONTENT-010 | A fetch or checkpoint binds to the wrong account, e.g. after an account switch or multi-account confusion | Each selected folder has its own account/collection/query-bound encrypted checkpoint; ADR-006's mailbox-binding HMAC compares the opaque account before any request (ADR-004, ADR-006) | A cross-account checkpoint or locator mismatch is rejected before any Graph call; zero network or mutation follows |
| TM-MAILCONTENT-011 | A raw Graph `nextLink`/`deltaLink`, cursor, or URL leaks into logs, diagnostics, fixtures, or exported state | `nextLink`/`deltaLink` values are opaque encrypted locators, never reconstructed, edited, reused across a binding, logged, exported, or persisted in plaintext (ADR-004); the diagnostic-event allowlist stays empty (ADR-PRIV-001) | A raw cursor or URL found in a log, diagnostic, fixture, or package artifact blocks G-PRIV and release without printing the value |
| TM-MAILCONTENT-012 | A folder rename or deletion silently widens or relocates configured processing scope, or folder identity is inferred from a display name | Folder identity is never inferred from a display name; a missing or deleted selected folder stops that collection, deletes its local checkpoint, discloses the coverage loss, and requires explicit remapping (ADR-004) | A rename or deletion never auto-expands or silently remaps scope; continued processing requires an explicit user remapping action |
| TM-MAILCONTENT-013 | Mail-derived state (observation, quarantine health, locator) is retained beyond its configured retention window | ADR-PRIV-001 fixes exact retention transitions: the configured history window plus 15 days for observations, atomic quarantine-health deletion on resolution, and disconnect deleting both regardless of age | An observation or quarantine record outliving its retention window is a defect, not accepted residual state, and is purged |
| TM-MAILCONTENT-014 | The product overclaims complete, real-time, or background-while-inactive event coverage | ADR-004 permits no complete, immediate, background-while-inactive, or delivery claim; the freshness UI must link to the coverage statement disclosing known unrecoverable cases (arrive/read/leave between polls, unsaved or immediately moved/deleted sent copies, unvalidated alternate sent folders, time outside the selected history window) (OL-SYNC-009/011) | No completeness or real-time claim is ever rendered; a coverage-loss disclosure accompanies every known gap |
| TM-MAILCONTENT-015 | A model projection built from mail content exceeds the bounded transient shape, e.g. whole-mailbox or whole-thread-by-default input, attachment bytes, or an unrelated recipient | ADR-007 bounds one changed-message projection plus at most four context-message projections to the closed content categories (subject, body block, quote block, sender, To/Cc participant positions, attachment name, link label); whole-mailbox/whole-thread-by-default input, attachment bytes, URLs/`href` values, and unrelated recipients are prohibited | An oversized or out-of-category projection is rejected before any model transmission; this flow can mark content eligible for bounded projection but cannot itself activate transmission, which remains gated separately by G-MODEL/G-PRIV |

## Residual risk and gates

Service and client behavior can differ by mailbox, cached mode, retention
policy, and Graph version; throttling, partial outages, and coalesced
intermediate states are not eliminated, only bounded and disclosed. G-MAIL
must validate exact selected fields, folder-policy versus token-scope
behavior, Inbox/Sent delta, coalesced arrive→read/move sequences, first
observation, saved-sent-disabled/moved/deleted/send-as cases, paging/replay/
cursor-expiry behavior, and bounded resync/known-loss disclosure using only
synthetic disposable-tenant content. G-PRIV places synthetic canaries in
every content, locator, and cursor position and requires zero occurrences in
application-controlled durable, diagnostic, log, package, and repository
artifacts. G-MODEL separately governs whether any validated projection may
ever be transmitted to a provider. No gate, capability, permission, or
product claim advances by this threat model; mail detection and evidence
capabilities remain disabled and route to the product owner on any failed or
unrun gate.
