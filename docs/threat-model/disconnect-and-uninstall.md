# Disconnect and uninstall threat model

**Status:** ADR-002/ADR-005/ADR-010/ADR-012/ADR-PRIV-001 decision-contract-only
model; runtime, cleanup mechanics, and all named gates remain inactive.

## Boundary

This model covers the future disconnect and uninstall paths across identity
(`contracts/identity/authentication-boundary.json`, ADR-002), protected local
state (`contracts/persistence/protected-state-boundary.json`, ADR-005), the
add-in bridge (`contracts/addin/bridge-boundary.json`, ADR-010), the
installer/uninstaller (`contracts/distribution/registration-boundary.json`,
ADR-012), and the privacy retention/deletion boundary
(`contracts/privacy/persistence-boundary.json`, ADR-PRIV-001). No cleanup
routine, listener, certificate, database, or installer exists. Disconnect is
never described as global logout, token/session revocation, browser-cookie
removal, or tenant consent revocation; it is local, account-bound cleanup
only. Uninstall repeats disconnect cleanup where possible and additionally
removes local bridge trust, certificates, and protocol registrations. Neither
operation performs a Microsoft Graph mutation or bulk-deletes a Microsoft
reminder artifact (product-spec 8.10). Identity, the add-in, durable state,
and release all remain disabled behind G-ID, G-ADDIN, G-STATE, G-PRIV, and
G-RELEASE.

## Threats and required controls

| Threat ID | Threat | Required control | Fail-closed result |
|---|---|---|---|
| TM-DISCONNECT-001 | Disconnect is presented as global logout, token/session revocation, browser-cookie removal, or tenant consent revocation | ADR-002 prohibits describing disconnect as any of these; the UI must direct the user to revoke browser sessions and Microsoft consent separately | No such claim is ever rendered; the UI discloses that revocation is a separate user/admin action |
| TM-DISCONNECT-002 | An application-owned DPAPI blob (`state-root.dpapi`, `pairing-root.dpapi`, `provider-credential.dpapi`, or a token-cache blob) is orphaned after a failed disconnect/uninstall cleanup | Cleanup enumerates only the exact closed ADR-005 allowlist and never recurses through an untrusted directory; a failed protected-cache deletion keeps identity and affected capabilities disabled until owner-approved recovery completes (ADR-002, ADR-005) | Incomplete DPAPI cleanup blocks reconnect and the affected capability; the failure is disclosed, never silently treated as success |
| TM-DISCONNECT-003 | A bridge certificate, trust artifact, or pairing record is orphaned after a failed disconnect/uninstall cleanup | ADR-010 requires certificate and pairing cleanup on disconnect/uninstall; failure of any trust-cleanup step blocks the add-in rather than leaving orphaned trust behind | Orphaned bridge trust blocks the add-in capability until cleanup is confirmed complete |
| TM-DISCONNECT-004 | A crash or interruption mid-cleanup deletes some allowlisted DPAPI blobs or bridge-trust artifacts but not others, leaving a partially cleaned, inconsistent state | Cleanup treats the exact enumerated ADR-005/ADR-010 allowlist as one all-or-nothing outcome; partial completion is reported and treated as failure, never as success | Any incomplete deletion across the enumerated set keeps the affected capability disabled and reports incomplete cleanup rather than declaring disconnect/uninstall complete |
| TM-DISCONNECT-005 | Reconnect after a failed cleanup silently succeeds, reusing orphaned DPAPI or bridge-trust material instead of requiring resolved cleanup first | ADR-002 keeps identity and reconnect disabled until owner-approved recovery completes after a failed protected-cache deletion; ADR-010 keeps the add-in disabled while bridge-trust cleanup is incomplete | Reconnect is blocked, not silently allowed, until cleanup or recovery is confirmed complete; no orphaned material is silently reused |
| TM-DISCONNECT-006 | Uninstall or disconnect deletes another OS user's state, or a path/reparse/hard-link substitution causes cross-account deletion | ADR-005 fixes current-user DPAPI, a protected owner-only DACL, a known-folder-resolved root, fixed-volume/reparse-point/hard-link/path-identity checks, and an opaque per-account directory; cleanup enumerates only the exact allowlist under that account's directory | A cross-user or cross-account deletion attempt is rejected; only the current opaque account's exact allowlisted files are removed |
| TM-DISCONNECT-007 | Disconnect or uninstall bulk-deletes existing Microsoft reminder artifacts (To Do tasks, Calendar events) automatically | Product-spec 8.10 and ADR-005 require the user to choose remain/detach/individual-review; disconnect and uninstall perform zero Graph mutation and never bulk-delete a Microsoft artifact | Any bulk Graph-mutation or automatic-delete attempt during disconnect/uninstall is prohibited by contract and blocked before any request |
| TM-DISCONNECT-008 | Disconnect or uninstall claims physical erasure, backup erasure, or removal from OS paging, snapshots, or endpoint-security tooling | ADR-005, ADR-PRIV-001, and ADR-012 require explicit residual-risk disclosure of OS paging, backups, snapshots, endpoint security, and same-user/administrator access; no cryptographic-impossibility or physical-erasure claim is permitted | Any physical-erasure or complete-removal claim is rejected by review; the disclosed residual-risk text is required instead |
| TM-DISCONNECT-009 | Cleanup leaves a stale but still-decryptable or partially consistent SQLite database or rollback anchor after disconnect | Disconnect deletes account-bound local state and protected secrets only after jobs stop and only via the same exclusive commit ordering as ordinary mutation (ADR-005) | A failed or partial deletion disables the affected capability rather than leaving a partially cleaned, still-usable local database |
| TM-DISCONNECT-010 | Uninstall or disconnect proceeds while a job is in-flight or a write is ambiguous, leaving an unresolved external mutation with no local record to reconcile it | Product-spec 8.10 requires disconnect to stop jobs and resolve or mark in-flight writes before removing local state; ADR-004 ordering keeps ambiguous writes reconciled before any cursor or state change | An in-flight or ambiguous write blocks local-state deletion until it is resolved or explicitly marked; it is never silently discarded |
| TM-DISCONNECT-011 | Uninstall completes only a partial removal (e.g. bridge trust or a protocol registration left behind) while reporting success | ADR-012 requires uninstall to repeat disconnect cleanup where possible and remove every local bridge trust, certificate, and protocol registration ADR-010 created, leaving no orphaned trust behind | An uninstall that cannot complete cleanup discloses the incomplete state; it never reports success while trust or a registration remains |
| TM-DISCONNECT-012 | Disconnect or uninstall runs while a schema or key-rotation migration is in progress, corrupting state or allowing an old binary to write a newer schema | ADR-005 requires an older binary to refuse writing a newer schema or ciphertext version and requires disconnect/uninstall during an in-progress migration to follow the same copying/ready-to-swap/installed-pending-cleanup recovery phases rather than an ad hoc deletion | Disconnect/uninstall during an in-progress migration enters non-mutating recovery instead of partially deleting inconsistent state |

## Residual risk and gates

DPAPI, database, and bridge-trust cleanup cannot promise physical erasure
from SSD history, OS paging, backups, snapshots, crash residue, endpoint
security tooling, same-user access, or administrator access; these remain
disclosed residual risks, not eliminated ones. G-ID must validate disconnect
and secure-store-failure recovery with synthetic accounts. G-STATE must
validate cross-user, rollback, and cleanup-failure recovery. G-ADDIN must
validate certificate/pairing cleanup and reconnect-after-failure behavior.
G-PRIV must find zero content or credential canaries in any cleanup path.
G-RELEASE must validate the per-user unelevated installer and complete
uninstall boundary on a fresh machine. No gate, capability, or product claim
advances by this threat model; a failed or unrun gate keeps identity, the
add-in, durable state, and release blocked and routes to the product owner,
and cleanup failure specifically blocks a supported release.
