# Protected local-state threat model

**Status:** P0-WI-08 decision contract; runtime and state-gate evidence
unimplemented.

This model is authoritative together with ADR-005,
`contracts/persistence/protected-state-boundary.json`, ADR-PRIV-001, and its
closed field allowlist. All future test values are synthetic.

| Threat ID | Threat | Required control | Fail-closed result |
|---|---|---|---|
| TM-STATE-001 | An unapproved or malformed field is hidden inside ciphertext | Exact typed validation before encryption and after authenticated decryption/migration; no open maps, enums, strings, blobs, or silent drops | Reject the record; persist nothing |
| TM-STATE-002 | Ciphertext is swapped across account, record, type, schema, key, or algorithm | Canonical AAD binds the exact opaque account and envelope identity | Authentication fails; release no plaintext and enter recovery |
| TM-STATE-003 | AES-GCM nonce repeats, a multi-record transaction undercounts attempts, or its invocation bound is exceeded | Calculate and durably reserve the exact per-key attempt count for the complete ordered record set before RNG; one fresh Windows-CSPRNG 96-bit nonce and one distinct reservation per record; retained-generation collision rejection, rotation at 2^30, hard stop before 2^32; failed attempts burn reservations | Stop encryption and all dependent mutation |
| TM-STATE-004 | Secret material enters the database, command line, config, diagnostics, or repository | Purpose-separated bounded DPAPI current-user blobs in the closed protected file catalog or memory only; database references only; no plaintext fallback | Disable the dependent capability without creating a cache |
| TM-STATE-005 | Cross-user, copied database, machine transfer, link, or path substitution exposes state | Current-user DPAPI, protected owner-only DACL, known-folder root, fixed-volume and handle-based reparse/hard-link/stream/path checks, opaque account binding, AEAD, and key/database checks | Decrypt/use fails; no automatic reset or mutation |
| TM-STATE-006 | Database-only, profile, VM, or clock rollback replays a mutation | Separate rollback HMAC key; exact full logical-database commitment in the protected anchor; every transaction updates and verifies the anchor before publication; every startup reconciles before mutation | Freeze cursor advancement and outward mutation; reconcile and review ambiguity |
| TM-STATE-007 | Crash or disk-full leaves a partially migrated or mixed writable store | Exclusive copying/ready-to-swap/installed-pending-cleanup phases; proposed commitment only after staging verification; explicit staging WAL; zero-frame checkpoints, closed handles and absent sidecars; allowlisted `ReplaceFileW` rollback generation; phase-specific restart combinations and delayed phase clearing | Complete or abort the one phase-specific verified generation path, otherwise remain in non-mutating recovery |
| TM-STATE-008 | Older code downgrades or silently drops newer state | Closed version catalogs and old-binary write refusal | Require a compatible binary; perform zero writes |
| TM-STATE-009 | Page, checkpoint, ledger, or quarantine records become partially durable or publish before their rollback anchor | One database transaction and ADR-004 ordering; pending operation precedes request; no dangling quarantine reference; recompute, atomically replace, reopen, and verify the protected anchor before any result, cursor, or mutation is published | Roll back/replay idempotently or reconcile; never blindly retry |
| TM-STATE-010 | SQLite sidecars, temp, backup, staging, crash, or package artifacts leak content | Decrypted values never reach SQL; WAL/FULL; no trace/extensions/raw live copy; exhaustive canary scans and package allowlist | Block G-PRIV and release without printing the canary |
| TM-STATE-011 | Delete, disconnect, or uninstall mutates Microsoft data or overclaims erasure/revocation | Local-only reference-aware cleanup, zero Graph mutation, explicit residual disclosure | Keep identity/reconnect disabled when cleanup is incomplete |
| TM-STATE-012 | Same-user code, administrator, paging, snapshot, roaming profile, or endpoint tooling bypasses the local boundary | Minimize state, disclose residual risk, independent security audit, no claim of cryptographic impossibility | G-SEC-AUDIT and broad release remain blocked |

DPAPI is selected without machine scope, prompt UI, a description, or optional
entropy. Its output is itself untrusted until the versioned key bundle is
strictly validated. Application AEAD authenticates every state record before
parsing. Credential Manager is not the baseline because its generic entries are
individually replaced and enterprise persistence may roam; introducing it later
requires a reviewed contract revision rather than an implicit fallback.

The diagnostic allowlist, product backup/export, and recovery-key export remain
empty or disabled. Physical erasure from SSD history, paging, old journals,
backups, snapshots, crash residue, endpoint security, same-user access, or
administrator access is not promised. G-STATE, G-PRIV, and G-SEC-AUDIT remain
unrun, so durable state and every dependent capability remain disabled.
