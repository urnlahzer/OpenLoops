# ADR-005: Persistence and cryptography

- **Status:** Accepted
- **Date:** 2026-07-20
- **Work item:** P0-WI-08
- **Owner decisions:** OWN-06, OWN-07
- **Blocking gates:** G-STATE, G-PRIV, G-SEC-AUDIT

## Context

OWN-06 already selects minimized encrypted local state as the MVP baseline, and
OWN-07 already approves only the closed derived-field and source-reference
inventory in ADR-PRIV-001. This ADR does not reopen either choice. It translates
them into an implementable cryptographic, transaction, recovery, and lifecycle
contract while every durable-state capability remains disabled.

The operating-system boundary is useful but limited. Windows DPAPI normally
binds protected material to the same user and machine, yet Microsoft documents
roaming-profile exceptions. Same-user code, administrators, endpoint tooling,
paging, snapshots, and coherent profile or VM restores remain residual risks.
Encrypted state is therefore minimized sensitive state, not proof that the
device cannot expose or roll it back.

## Decision

`contracts/persistence/protected-state-boundary.json` is the exact P0-WI-08
decision contract. The deterministic checker enforces exactly twelve executable
P0-STATE assertions; a thirteenth fresh-checker assertion closes the work item.

Future state uses one current-user-only SQLite database per opaque local account
binding. Only ADR-PRIV-001-approved logical records may enter it. Every record is
validated before serialization, independently encrypted with AES-256-GCM using a
fresh random 96-bit Windows-CSPRNG nonce and a full 128-bit tag, and validated
again after authenticated decryption or migration. The AAD is one exact 78-byte
fixed-width encoding of the domain, numeric algorithm code, key, opaque account
binding, record type, record ID, logical schema version, and ciphertext version.
Authentication failure releases no
plaintext. Nonce collisions, RNG failure, truncated tags, unknown versions, and
ad-hoc cryptography fail closed. Before any nonce is generated, each mutation
calculates and atomically reserves the exact number of encryption attempts per
active AES key for its complete canonically ordered record set. Every record
consumes one distinct reservation. Keys rotate at 2^30 attempts and can never
reach NIST's 2^32 random-IV limit. A durable reservation is intentionally burned
if nonce generation or a later step fails.

Approved derived digests use HMAC-SHA-256 with one exact length-framed input and
a closed unique purpose-tag catalog. The catalog deliberately leaves each domain
component layout unavailable until its owning ADR closes it. ADR-006 now closes
the first seven layouts: query/source/observation versions, the nested
`email_evidence_ref.message_locator` mailbox binding, and content/prefix/suffix
anchors. The mailbox binding uses the distinct per-account content-digest key,
its own purpose tag, and exactly one common input envelope; its raw ID-token
claim inputs never persist. Rollback commitments
use a separate HMAC-SHA-256 key and the exact fixed anchor framing in the
contract; they include every current logical envelope and ciphertext row in a
canonical order. AEAD, content-digest HMAC, token, provider, pairing, session,
and rollback-anchor material are never interchangeable. The database stores only
opaque protected-store references and nonsecret key IDs. Random state/HMAC keys,
OAuth token state, and provider credentials use bounded versioned DPAPI
current-user blobs. `CRYPTPROTECT_LOCAL_MACHINE`, prompt UI, non-null readable
descriptions, colocated optional entropy, Credential Manager enterprise roaming,
plaintext files, command-line secrets, and `.env` product fallbacks are
prohibited. Pairing issuance and rotation remain ADR-010's responsibility;
session secrets remain memory-only.

DPAPI blobs live only beneath the Windows known-folder-resolved per-user local
application-data root and an opaque random account directory. The file catalog,
sizes, protected owner-only DACL, fixed local-volume requirement, reparse-point,
hard-link, alternate-stream, and path-identity checks are closed. Atomic writes
use a flushed same-directory random `CREATE_NEW` temporary file followed by the
specified `MoveFileExW` or `ReplaceFileW` operation and a handle-based reopen and
validation. Temporary files are never promoted at startup. Disconnect and
uninstall enumerate only the exact allowlist and never recurse through an
untrusted directory.

SQLite uses WAL with `synchronous=FULL`, verified pragmas, one account database,
and no extension loading or trace callbacks. SQL, indexes, WAL, SHM, rollback
journals, temporary files, migration staging, and future reviewed snapshots may
contain only the minimum clear envelope and ciphertext—never decrypted logical
values. A raw copy of a live database is not a supported backup. Product-state,
metric, plaintext-backup, and recovery-key export remain disabled.

Page outcomes, checkpoints, operation ledger entries, and quarantine health
commit in one database. No rollback generation, commitment, key, or anchor
reference is stored in SQLite; those values exist only in `state-root.dpapi`.
ADR-004 ordering remains binding:
every item has a durable safe result or complete recoverable quarantine before
cursor replacement; pending operation identity precedes an external request;
ambiguous writes reconcile before retry; no quarantine reference dangles.

A distinct key and the versioned generation/root commitment inside
`state-root.dpapi` detect ordinary database-only rollback and key divergence.
Every mutating SQLite transaction commits first, the complete logical database
root is recomputed, and only then is the protected anchor atomically replaced
and both values reopened and verified. Nothing publishes a result, cursor, or
outward mutation before that sequence completes. A crash may conservatively
trigger recovery but cannot authorize a blind mutation. Missing, mismatched,
corrupt, cross-account, clock-reversed, or unknown state freezes cursor
advancement and every outward mutation. Ordinary absence of closed SQLite
sidecars is valid; an unexpected sidecar or interrupted transaction is not.
Because
a coherent profile or VM restore may roll back both database and anchor, every
process start reconciles mail checkpoints and reminder operations before outward
mutation. Suspected restore creates fresh active write keys only after bounded
reconciliation and explicit review of ambiguity. The product never claims that
rollback is cryptographically impossible.

Schema migration and key rotation are exclusive, journaled, copy-validate-
encrypt-verify-swap operations. The initial migration phase records the old
commitment and target versions only; the proposed commitment is added only after
the complete staging database exists and its logical root verifies. Staging
explicitly starts in WAL mode. Both active and staging databases must run
`PRAGMA wal_checkpoint(TRUNCATE)` with zero busy and uncheckpointed frames, switch to `journal_mode=DELETE`,
close all handles, and have no sidecars before the swap. The staging main file is
flushed and its complete logical root verified. `ReplaceFileW` creates only the
allowlisted internal `state.rollback.db` recovery generation; it is not a product
backup or export. The active database is reopened and verified before the anchor
advances, and the rollback generation is deleted before WAL mode and work resume.
The protected migration state remains `installed_pending_cleanup` after the new
anchor is installed and is cleared only after rollback deletion, WAL reopening,
and PRAGMA verification. Startup handles each copying, ready-to-swap, and
installed-pending-cleanup file combination explicitly; an orphan migration file
or unrecognized combination remains in non-mutating recovery. An
older binary refuses to write a newer schema or ciphertext version. Old keys
remain decrypt-only until every record and the explicit recovery decision are
verified, then retire best effort.

Retention, quarantine cleanup, scope removal, disconnect, and uninstall follow
ADR-PRIV-001 exactly. They are local-only and perform zero Graph mutations.
Disconnect deletes account-bound local state and protected secrets after jobs
stop while leaving Microsoft artifacts to the existing remain/detach/individual-
review choice. Cleanup failure keeps identity and affected capabilities disabled.
Neither disconnect nor uninstall claims global logout, consent revocation,
physical erasure, backup erasure, or Microsoft-artifact deletion.

Secure-store absence permits only ephemeral, non-mutating status or synthetic
test behavior. Any path requiring durable tokens, checkpoints, idempotency,
quarantine recovery, provider credentials, or Microsoft mutation fails closed
without creating a cache artifact. Allowing session-only mutations would require
a new product-owner privacy and safety decision.

The exact Rust dependencies are selected but not activated. Their compiled
feature graph, native SQLite, Windows unsafe boundary, CSPRNG behavior, recent
AES-GCM release, and the in-progress NIST GCM revision require renewed review
before implementation and G-SEC-AUDIT.

## Consequences

This contract favors recoverable false-positive recovery over silent state loss,
duplicate mutation, plaintext fallback, or rollback-blind writes. It creates no
database, key, token cache, backup, migration, diagnostic, or network contact.
Dependent domain catalogs remain owned by ADR-006, ADR-008, ADR-009, and their
named gates; this ADR does not approve a generic runtime record.

P0-WI-08 accepts ADR-005's persistence-and-cryptography decision contract only.
No gate, capability, support row, or acceptance criterion advances. The
persistence adapter remains unavailable; G-STATE, G-PRIV, and G-SEC-AUDIT remain
unrun.
