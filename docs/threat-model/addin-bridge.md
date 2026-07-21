# Add-in bridge

**Status:** ADR-010 decision contract accepted; runtime and all named gates
remain inactive.

The primary safety property is that no same-user local process, page, or
prior pairing can obtain companion authority without an explicit, bound,
current pairing, and that a bound session can only ever exercise the least
authority the add-in surface requires. No mechanism this ADR pins is a
remotely reachable service; it is a same-user loopback or native-pipe
boundary only.

| Threat | Required control | Fail-closed result |
|---|---|---|
| Pairing bootstrap race between two first-pair attempts | At most one bound pairing per port; explicit user-verifiable action | The losing attempt is rejected, not silently bound |
| Replayed pairing nonce | Single-use nonce with explicit expiry | Reuse or expiry rejects the pairing |
| Brute-forced pairing verification | Rate- and attempt-bounded verification | Verification fails closed rather than degrading into an open window |
| Malicious local page issues loopback requests | No unpaired command exists; every command requires a bound, live session | Request rejected before any companion action |
| DNS-rebinding a hostname to 127.0.0.1 | Exact Host/Origin allowlist plus loopback-only binding rejects rebound origins | Request rejected; no LAN/wildcard bind exists to exploit |
| CSRF from the add-in WebView or another page | Explicit CSRF defense independent of cookies | Request rejected |
| Certificate/trust abuse | No machine-wide trust or general-purpose root; current-user, non-exportable, name-limited, lifecycle-managed certificate only if selected | Untested or broadened trust blocks the add-in rather than persisting |
| Secret leaks via URL, roaming settings, or localStorage | Bootstrap and session values bound only in-memory or in the pairing flow; prohibited storage locations enumerated | Value never reaches a prohibited location; content boundary rejects it |
| Browser persistence of mailbox content | Content boundary prohibits localStorage/IndexedDB/service-worker/URL/console/analytics/crash-report | No content reaches browser storage |
| Unpaired command | Every command requires a bound, live, least-authority session | Command rejected |
| Add-in authority escalation | Sessions are command-scoped to review/evidence/settings/status only; never generic or synchronization authority | Escalation attempt rejected by scope check |
| Stale session after account switch or reconnect | Session rotates on reconnect and revokes on account change | Stale session rejected, not silently reused |
| Uninstall/disconnect cleanup failure | Certificate and pairing cleanup required on disconnect/uninstall; failure blocks the add-in | No orphaned trust or pairing persists undetected |
| Review link becomes a credential | Random non-secret handle only; authenticated per-user account-bound resolution; ADR-009's prohibited list unchanged | Link grants no authority by itself |
| Same-user malware | Explicitly out of scope; DPAPI/session boundary defends against a different local account or a malicious page, not code already running as the authenticated user | Documented residual limitation, not a claimed defense |

No listener, pipe, certificate, pairing, manifest, permission, or client
support is enabled by this threat model. A failed G-ADDIN blocks the
Outlook-add-in MVP and routes to OWN-01; native UI is never a silent
substitute.
