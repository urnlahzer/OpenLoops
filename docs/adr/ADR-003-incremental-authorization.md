# ADR-003: Incremental authorization and configured processing

- **Status:** Accepted
- **Date:** 2026-07-20
- **Work item:** P0-WI-06
- **Owner decisions:** OWN-02; referenced feature consequences remain governed by OWN-04, OWN-05, and OWN-10 in their named ADRs
- **Requirements:** OL-GOV-001, OL-AUTH-003, OL-SYNC-005, OL-REM-001–002, OL-SUM-002
- **Blocking gates:** G-ID, G-MAIL, G-TODO, G-CAL, G-ADDIN, G-SELFMAIL, G-PRIV

## Context

OpenLoops needs independent consent boundaries for identity, mail detection,
reminder adapters, Calendar, and optional self-email. A Graph permission defines
what Microsoft authorizes; it does not define what OpenLoops is configured to
process, prove that an API contract works, or pass a product gate. Office add-in
manifest permissions are a separate authority system and never grant Graph
access.

## Decision

`contracts/identity/permission-boundary.json` is the closed Phase 0 permission
and processing policy. Every row is disabled and unadvertised, and this ADR
requests no permission or network contact.

Base sign-in/refresh scopes remain an unresolved G-ID set. OpenLoops does not
guess `User.Read`, standard OIDC scopes, or library-added scopes. Mail detection
uses delegated `Mail.Read` only after explicit enablement and G-MAIL/G-PRIV.
`Mail.ReadBasic` remains a synthetic diagnostic boundary and cannot provide
body-based detection. `Mail.ReadWrite` is permitted only for the disposable-tenant
mutation-without-`Mail.Send` experiment named by the validation plan; it
is never a product permission or capability. To Do create/update/reconciliation uses delegated
`Tasks.ReadWrite`; `Tasks.Read` remains validation-only, and automation modes add
no permission. Calendar uses no named scope until G-CAL validates the least
delegated contract. Self-email adds `Mail.Send` only through separate explicit
consent after G-SELFMAIL and may address only the contract-tested authenticated
self recipient.

Consent is never bundled into a request-everything grant. Each explicit user
action requests only one feature delta. A returned token containing extra or old
grants never activates another feature. Missing, denied, revoked, admin-gated,
Conditional-Access-blocked, or account-mismatched authority stops the dependent
path before an API call; it never broadens the request, weakens authentication,
or silently selects another adapter.

`Mail.Read` is mailbox-wide delegated authority. OpenLoops policy nevertheless
processes only Inbox and Sent Items by default and additional owned folders after
explicit per-folder opt-in. Folder selection, history windows, filters, selected
fields, and polling bounds are minimization controls—not OAuth restrictions.
Removing a folder stops future processing and deletes the application-owned
folder checkpoint under ADR-005 and ADR-PRIV-001. It does not revoke Microsoft
consent or erase separately governed active-loop lineage. Selected-folder polling has explicit
coverage limits and never claims complete event observation.

The proposed desktop Office validation floor remains Mailbox 1.13 with
`ReadItem`; it is not the accepted production manifest and makes no support
claim. G-ADDIN must validate the exact production permission, requirement set,
clients, and bridge behavior. The add-in owns no Graph token, token cache,
synchronization authority, or durable content, and `ReadItem` is not Graph
`Mail.Read`.

All application, app-only, tenant-wide, shared, `*.All`, `/.default`, directory,
product mail-mutation, broad add-in, guessed Calendar, combined-consent, and untraced
permissions listed in the manifest are prohibited. Grant success in a disposable
tenant is test evidence only and cannot enable or advertise a capability.

## Consequences

- ADR-002 continues to own the browser-PKCE transaction contract; ADR-005 owns
  protected token/cache state.
- ADR-004/006 must define exact Graph fields, delta, and evidence identity.
- ADR-010 must define the add-in bridge; ADR-013 must define self-email safety.
- Denial disables only dependent features. Scope removal in Microsoft remains a
  distinct user/admin action and local disablement is not consent revocation.
- No gate, capability, support row, or acceptance criterion advances here.

## Verification

P0-WI-06 is graded by exactly these eight checks in
`docs/prd-traceability.md`: P0-AUTHZ-INVENTORY-001,
P0-AUTHZ-FEATURE-SCOPE-001, P0-AUTHZ-INCREMENTAL-001,
P0-AUTHZ-FOLDERS-001, P0-AUTHZ-OFFICE-MANIFEST-001,
P0-AUTHZ-CROSS-CONTRACT-001, P0-AUTHZ-CLAIMS-001, and
P0-AUTHZ-FRESH-CHECKER-001. The deterministic checker and mutation suite keep
Graph authority, configured processing, Office authority, gate status, and
product claims distinct.
