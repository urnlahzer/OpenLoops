# OAuth transaction and token-state threat model

**Status:** P0-WI-05 decision contract; runtime and G-ID evidence unimplemented.

The data flow is: user action → companion creates PKCE/state → system browser →
Entra authorization → ephemeral loopback callback → code exchange → in-memory
tokens → future OS-protected cache → refresh or disconnect. No step exists in
the current runtime; all values and accounts used by future tests must be
synthetic and disposable.

| Threat ID | Threat | Required control | Verification / fail-closed result |
|---|---|---|---|
| TM-OAUTH-001 | A malicious fork reuses a public client ID | PKCE is not represented as binary authentication; shared registration requires signed provenance and ADR-012 governance | G-ID/G-RELEASE review; shared registration remains disabled |
| TM-OAUTH-002 | Authorization code interception | Fresh PKCE S256 verifier, bound to one transaction and used once | Reject exchange without exact verifier; identity remains disabled |
| TM-OAUTH-003 | Callback injection, state mismatch, replay, duplication, expiry, mixed result, or malformed/duplicate parameters | Fresh high-entropy state, exact match, one pending transaction, query/GET-bound result shapes, bounded unknown-parameter ignore without exposure, and one-use terminal state | Synthetic state/parameter/replay matrix; reject without replacing a valid incumbent transaction |
| TM-OAUTH-004 | Port race, wrong method/path/Host/authority, LAN exposure, lingering listener, or unapproved `form_post` | Ephemeral loopback-only binding, exact pending path/authority, query/GET only, G-ID IPv4/IPv6 tests, immediate shutdown | Synthetic socket/redirect matrix; reject POST and cancel/restart with fresh state where appropriate |
| TM-OAUTH-005 | Wrong issuer, authority, audience, or cloud | Exact G-ID-validated authority/audience and token-claim contract | Reject token/session; no cross-cloud fallback |
| TM-OAUTH-006 | Account or tenant substitution | One account per OS user and explicit opaque account binding for state and operations | Cross-account tests reject; no mutation |
| TM-OAUTH-007 | Overbroad or confused consent | ADR-003 exact incremental scope manifest and token `scp` verification | Scope mismatch disables dependent feature; never broaden automatically |
| TM-OAUTH-008 | Token or secret leakage | Memory or future current-user protected store only; source sanitization; protocol URLs carry only transient opaque state/code; no application-controlled persistence, CLI, logs, diagnostics, fixtures, telemetry, copied URLs, or plaintext fallback | Privacy canaries find zero application-controlled occurrences after terminal callback handling |
| TM-OAUTH-009 | Concurrent cache corruption or stale transaction replacement | Single pending transaction, second same-process request rejected without incumbent mutation, concurrent-process fail-close, atomic protected cache contract deferred to ADR-005 | Concurrency matrix rejects the second transaction/process and preserves the incumbent |
| TM-OAUTH-010 | Revocation or Conditional Access triggers a weaker flow | Stop access and require explicit reconnect/admin resolution | No WAM/device-code/embedded-flow downgrade |
| TM-OAUTH-011 | Disconnect is presented as global logout or revocation, or protected-cache cleanup fails | Inventory-driven local cleanup, fail-closed identity/reconnect recovery, and exact residual local/session/consent disclosure | Copy and cleanup tests; incomplete cleanup keeps identity disabled and blocks release |

Residual risk remains from same-user malware, administrators, compromised
dependencies, unavoidable browser/OS-controlled protocol history or state,
paging, crash capture, backups, and
public-client-ID reuse. G-ID and G-PRIV block identity functionality;
G-SEC-AUDIT blocks broad release. Any failed or unrun gate leaves every
identity-dependent capability disabled and routes scope change to the product
owner.
