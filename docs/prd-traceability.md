# OpenLoops PRD traceability and disposition matrix

**Status:** Product-owner decisions accepted 2026-07-19; capability and security gates remain release-blocking.
**Source:** OpenLoops MVP PRD draft supplied with the design request.
**Companion documents:** [Product specification](product-spec.md) and [implementation plan](implementation-plan.md)

## Legend

- **Specified:** represented by normative product-spec requirements.
- **Gated:** remains required for the claimed MVP feature, but depends on a named capability/safety gate.
- **Owner-approved adaptation:** changes or narrows a PRD interpretation under an accepted OWN decision; named gates still apply.
- **Deferred/rejected:** intentionally outside MVP; any mismatch with the PRD requires owner approval.
- Test IDs are planned stable test suites/scenarios; implementation creates executable cases with the same prefix.

## Phase 0 implementation checks

These checks grade architecture/governance work without claiming that a product
acceptance criterion or external capability gate has passed.

| Check ID | Exact assertion | Evidence | Status after P0-WI-01 | Blocks |
|---|---|---|---|---|
| P0-ADR-OWN-001 | OWN-00 through OWN-10 occur exactly once in the stable owner registry, remain accepted, and route only to known ADRs and gates. ADR-001 faithfully encodes the approved runtime boundary. | `research/microsoft-graph/product-decisions.md`; `docs/adr/ADR-001-runtime-and-self-hosting-boundary.md`; `tools/check-governance.ps1` | Executable | Any implementation choice that could reopen an OWN disposition |
| P0-CAP-001 | Every listed outward capability is disabled and unadvertised; each names known owner decisions, blocking gates, permission contracts, product-owner failure routing, and a safe fallback. No gate is represented as passed. | `contracts/governance/capabilities.json`; `tools/check-governance.ps1` | Executable | Capability implementation or claim |
| P0-THREAT-001 | Tokens, Graph content, model transmission, add-in bridge, local state, Microsoft artifacts, diagnostics/crash/CI, update, and disconnect boundaries route to planned ADRs, gates, and fail-closed behavior. | `docs/threat-model/README.md`; `tools/check-governance.ps1` | Executable routing check; detailed models remain planned | Dependent adapter or persistence work |
| P0-TRACE-001 | Owner, ADR, and gate inventories are exact; unknown, duplicate, dangling, prematurely accepted, or acceptance-claiming entries fail. | `contracts/governance/capabilities.json`; `tools/check-governance.ps1`; `tools/test-governance.ps1` | Executable with synthetic negative cases | Next Phase 0 work item |
| P0-FRESH-CHECKER-001 | A fresh-context checker grades the four assertions above and reports no unresolved high-severity boundary or traceability defect. | Work-item review only; no transcript or model output is stored in the repository | Required before closing P0-WI-01 | Next Phase 0 work item |

P0-WI-01 completes no PRD acceptance criterion and passes no Graph, add-in,
privacy, state, model, automation, security-audit, or release gate. In particular,
AC-23 and AC-24 remain blocked on their existing OWN and named-gate rows.

### P0-WI-02 privacy-boundary checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-PRIV-INVENTORY-001 | The persistent record and field privacy inventory is exact, closed, classified, purpose-limited, and independently allowlisted; it explicitly approves no runtime values. | `contracts/privacy/persistence-boundary.json`; `tools/check-privacy-boundary.ps1` | Executable manifest check | ADR-005 logical schemas and durable-state implementation |
| P0-PRIV-SOURCE-001 | Exactly three typed source variants exist and manual origin has one user-authoritative Microsoft artifact with title/body/due ownership. | Manifest; ADR-PRIV-001 | Executable | Evidence and manual-origin adapters |
| P0-PRIV-CLOSED-001 | Unknown/duplicate manifest keys, fields, records, source variants, and generic extension containers fail closed. Runtime enum/value/version schemas approve no values until ADR-005 and dependent domain ADRs define exact catalogs and bounds. | Manifest; deterministic negative suite | Executable manifest contract; runtime schema and enforcement remain gated | ADR-005, G-STATE |
| P0-PRIV-NOTEXT-001 | Human-readable Microsoft content, generated text, prompts/model output, diagnostics, logs, transcripts, and prohibited artifacts cannot enter the allowlist. | Manifest forbidden classes; deterministic canary mutations | Executable policy contract | G-PRIV |
| P0-PRIV-ID-001 | Raw Microsoft, tenant, account, workspace, and object identifiers are prohibited. The manifest permits only named locator/digest slots; ADR-005 must define their exact variants, algorithms, lengths, and bounds before any runtime value is approved. | Manifest semantic categories and fields | Executable manifest check; runtime value schema remains gated | ADR-005, G-STATE, G-PRIV |
| P0-PRIV-MANUAL-001 | Manual-origin field ownership, deletion-as-unavailable behavior, and email/calendar origin exclusions are exact cross-record invariants. | Manifest invariants; ADR-PRIV-001 | Executable policy contract | Reminder and manual-origin implementation |
| P0-PRIV-RETENTION-001 | Observation, loop-lineage, operation, settings, and aggregate-counter retention boundaries are exact and deletion makes no physical-erasure claim. | Manifest retention policies; ADR-PRIV-001 | Executable policy contract | G-STATE, G-PRIV |
| P0-PRIV-OPERATIONS-001 | Delete/reset/disconnect/export operations are local-only, make zero Graph mutations, never bulk-delete Microsoft artifacts, and product-state/metric export remains disabled. | Manifest operation policies and invariants | Executable policy contract | State operations and release |
| P0-PRIV-DIAG-001 | The diagnostic-event allowlist is empty; future diagnostics require a separate exact content-free schema and named gates. | Manifest; ADR-PRIV-001 | Executable policy contract | Diagnostics and release |
| P0-PRIV-TRACE-001 | ADR, OWN, requirement, AC, gate, manifest, and governance claims remain synchronized: zero capabilities, gates, or ACs are advanced. | Manifest; governance registry; both checkers | Executable | Next Phase 0 work item |
| P0-PRIV-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity privacy or traceability defect. | Work-item review only; no transcript or model output is stored | Required before closing P0-WI-02 | Next Phase 0 work item |

P0-WI-02 accepts the closed-world design boundary only. It completes no product
acceptance criterion and passes no capability, Graph, state, privacy,
security-audit, or release gate. Runtime persistence and its privacy-canary scan
remain unavailable until ADR-005 and the named gates have independent evidence.

### P0-WI-03 validation-matrix checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-SUPPORT-INVENTORY-001 | Windows, Outlook, account, cloud, provider, add-in, freshness, source, and invariant inventories are exact and closed. | `contracts/support/support-matrix.json`; `tools/check-support-matrix.ps1` | Executable manifest check | Skeleton and contract-harness matrix |
| P0-SUPPORT-WINDOWS-001 | Only serviced, release-tested Windows 11 x64 24H2/25H2 are approved validation targets; all other OS/runtime rows remain unavailable. | Support manifest and documentation | Executable | Platform build/release claims |
| P0-SUPPORT-OUTLOOK-001 | Classic Microsoft 365 Outlook, new Outlook, and same-Windows Outlook web are the only Outlook targets; activation differences and native fallback are explicit. | Support manifest; official-source snapshot | Executable policy check | G-ADDIN prototype |
| P0-SUPPORT-ADDIN-001 | The proposed add-in floor is desktop Mailbox 1.13 `ReadItem`; broader permissions, shared folders, token/sync authority, and event-based loop creation are prohibited. | Support manifest | Executable policy check; prototype remains gated | G-ADDIN |
| P0-SUPPORT-ACCOUNT-001 | One commercial-global work/school primary Exchange Online mailbox is the core target; personal requires its full matrix and all other account rows remain unavailable. | Support manifest; OWN-01/02 | Executable policy check | G-ID and feature contracts |
| P0-SUPPORT-CLOUD-001 | Only commercial-global authority/Graph origins are validation targets; government and China clouds are never selected opportunistically. | Support manifest | Executable policy check | Identity and Graph transport |
| P0-SUPPORT-MODEL-001 | Disabled is the only provider default; Ollama local/cloud and approved HTTPS remain unadvertised behind exact locality/origin/auth/privacy contracts and G-MODEL/G-PRIV. | Support manifest; OWN-08 | Executable policy check | ADR-007 and provider contracts |
| P0-SUPPORT-FRESHNESS-001 | Every unstable platform/service claim has a release-time official-source recheck; drift disables the row and routes scope changes to the owner. | Support manifest source and freshness inventories | Executable policy check | Release support claims |
| P0-SUPPORT-CLAIMS-001 | No row, capability, gate, acceptance criterion, provider transmission, permission request, or release claim is enabled or advertised. | Support and governance manifests; both checkers | Executable | Next Phase 0 work item |
| P0-SUPPORT-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity support, scope, permission, privacy, or traceability defect. | Work-item review only; no transcript or model output is stored | Required before closing P0-WI-03 | Next Phase 0 work item |

P0-WI-03 records the product-owner-approved `P0-MATRIX-001` validation targets.
It completes no acceptance criterion and passes no identity, Graph, add-in,
state, model, privacy, security-audit, or release gate. “Validation target” is
not interchangeable with “supported,” “enabled,” or “advertised.”

### P0-WI-04 build-skeleton checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-SKELETON-TOOLCHAIN-001 | Rust, Node, npm, TypeScript, and Office/Node type packages are exact, locked, current for the dated official-source snapshot, and fail closed on version drift. | `contracts/build-skeleton/skeleton.json`; toolchain files; dependency locks; checker | Executable | Phase 0 source build |
| P0-SKELETON-DIRECTION-001 | The exact workspace exists; domain/contracts have no adapter dependency, application depends only inward, and Graph/inference/persistence/desktop composition cannot reverse the dependency direction. | Cargo manifests and lock; checker and negative suite | Executable | Concrete adapters |
| P0-SKELETON-CONTRACT-001 | One closed, content-free JSON Schema deterministically generates untracked Rust and TypeScript skeleton types that cannot carry a capability or passed gate. | Schema; Rust build script; TypeScript generator; synthetic tests | Executable | IPC/model contract work |
| P0-SKELETON-PRIVACY-001 | The skeleton has no OAuth, Graph transport, model provider, persistence, add-in manifest, network origin, accepted secret, content fixture, log, source map, or tracked/generated diagnostic artifact. | Build manifest; source; ignore rules; privacy/public-repository gates | Executable | Any outward adapter |
| P0-SKELETON-BUILD-001 | The canonical script verifies pinned prerequisites, restores only locked public packages, formats/lints/builds/tests both languages, and accepts no credential or configuration argument. | `tools/source-build.ps1`; synthetic smoke output | Requires exact local prerequisites | Next Phase 0 work item |
| P0-SKELETON-PACKAGE-001 | Every crate is non-publishable; npm is private with an explicit empty file allowlist; release profiles omit debug symbols/source maps; dry-run package and repository scans contain no prohibited artifact. | Cargo/package manifests; package dry run; public-repository checker | Executable | Distribution work |
| P0-SKELETON-CLAIMS-001 | The skeleton completes zero acceptance criteria and advances or advertises zero capability or named gate. | Build, governance, privacy, and support manifests | Executable | Any product claim |
| P0-SKELETON-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity build, privacy, dependency, package, or traceability defect. | Work-item review only; no transcript or model output is stored | Required before closing P0-WI-04 | Next Phase 0 work item |

P0-WI-04 is inert build infrastructure. A passing synthetic smoke test is not
G-RELEASE and does not make the desktop companion or Outlook add-in usable.

### P0-WI-05 Rust-authentication contract checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-AUTH-INVENTORY-001 | The closed authentication manifest has the exact work item, ADR, owner, requirement, source, boundary, and separate-decision inventory; unknown, missing, duplicate, or drifted entries fail. | `contracts/identity/authentication-boundary.json`; checker and negative suite | Executable | Identity implementation |
| P0-AUTH-FLOW-001 | Delegated public-client system-browser authorization code with PKCE S256 is the sole MVP direction; embedded browser, WAM dependency, device-code fallback, ROPC, implicit, client credentials, application permissions, tenant-wide access, and secrets are prohibited. | ADR-002; authentication manifest; OAuth threat model | Executable decision contract | G-ID harness |
| P0-AUTH-REDIRECT-001 | The callback is same-machine, ephemeral, query/GET-only, exact-path/state, single-use, and immediately closed; LAN/remote/wildcard/fragment/POST callbacks fail. Malformed, duplicate, or mixed result parameters fail; bounded unknown parameters are ignored without exposure. Host/path and IPv4/IPv6 choices remain unresolved behind G-ID. | Authentication manifest; OAuth threat model | Executable decision contract | G-ID redirect matrix |
| P0-AUTH-ACCOUNT-001 | Exactly one commercial-global work/school primary-mailbox target is permitted for validation; personal, guest, shared, sovereign, multiple-account, and cross-account inheritance paths remain disabled or unsupported. | Authentication and support manifests | Executable decision contract | G-ID account matrix |
| P0-AUTH-TOKEN-001 | Token persistence is unimplemented. The protocol may transiently carry only opaque random state and code in the system-browser request/callback URLs; those and all tokens, verifiers, cookies, headers, and real identifiers cannot enter application-controlled persistence, CLI, fixtures, logs, diagnostics, telemetry, copied URLs, or plaintext files. Secure-store absence is session-only or fail closed. | Authentication/privacy/build manifests; negative suite | Executable boundary | ADR-005, G-ID, G-PRIV |
| P0-AUTH-CONCURRENCY-001 | Missing, mismatched, duplicate, replayed, expired, concurrent, cancelled, port-raced, or account-switched transactions fail closed and cannot silently replace a pending transaction. | Authentication manifest; synthetic mutations | Executable decision contract | G-ID concurrency tests |
| P0-AUTH-DISCONNECT-001 | Disconnect clears application-controlled identity state where supported and never claims global logout, token/session revocation, browser-cookie removal, or tenant-consent revocation. | ADR-002; manifest; threat model | Executable decision contract | ADR-005 and disconnect tests |
| P0-AUTH-DEPENDENCIES-001 | Reviewed direct crate versions/features/licenses are exact but unactivated; activation requires a complete Cargo lock and renewed license/security review. Git/wildcard dependencies and ad hoc OAuth/crypto are prohibited. | Authentication manifest; Cargo workspace/lock; checker | Executable selection boundary | Identity implementation and G-SEC-AUDIT |
| P0-AUTH-CLAIMS-001 | At P0-WI-05 closure, ADR-002 acceptance requested no permission, contacted no service, completed no AC, and advanced or advertised no capability, support row, or gate. G-ID remained unrun; ADR-003 and ADR-005 were then planned. | Authentication, governance, support, privacy, and build manifests | Executable historical closure claim | Next Phase 0 work item |
| P0-AUTH-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity identity, secret, permission, dependency, privacy, or traceability defect. | Work-item review only; no transcript or model output is stored | Required before closing P0-WI-05 | Next Phase 0 work item |

P0-WI-05 accepts an identity decision contract, not an identity implementation.
It passes no G-ID experiment and makes no Microsoft 365 capability usable.

### P0-WI-06 incremental-authorization checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-AUTHZ-INVENTORY-001 | The closed permission contract has the exact work item, ADR, OWN-02, requirement, permission-row, composition, source, and separate-decision inventories; unknown, missing, duplicate, or drifted entries fail. | `contracts/identity/permission-boundary.json`; checker and negative suite | Executable | Permission implementation |
| P0-AUTHZ-FEATURE-SCOPE-001 | Every disabled feature and validation row maps to its exact delegated/Office/unresolved/no-permission value, activation, gates, denial/revocation fallback, and zero advertised state; aggregate capability inventories are never consent bundles. | Permission manifest; ADR-003 | Executable decision contract | Graph/add-in contract harnesses |
| P0-AUTHZ-INCREMENTAL-001 | Initial combined consent is prohibited; one explicit feature action requests only its exact delta. Extra grants never activate features, while denial, revocation, admin, Conditional Access, and account mismatch fail only dependent paths without broader permission, weaker auth, or adapter substitution. | Permission manifest; negative suite | Executable decision contract | G-ID and feature gates |
| P0-AUTHZ-FOLDERS-001 | `Mail.Read` is disclosed as mailbox-wide authorization while processing remains Inbox/Sent by default plus explicitly opted-in owned folders, bounded review-only backfill, no shared/automatic expansion, and honest removal/coverage behavior. | Permission manifest; ADR-003 | Executable processing policy | G-MAIL |
| P0-AUTHZ-OFFICE-MANIFEST-001 | Graph and Office authority remain separate; Mailbox 1.13/ReadItem are disabled validation targets, not a production manifest/support claim, and no add-in manifest, origin, token, synchronization, or deployment authority exists. | Permission/support/build manifests | Executable decision contract | G-ADDIN |
| P0-AUTHZ-CROSS-CONTRACT-001 | ADR-003, governance permission summaries, ADR-002 separation, privacy scope-code slots, support targets, and inert build claims reconcile without enabling a row or introducing runtime/network artifacts. | All Phase 0 manifests and deterministic checker | Executable | Next Phase 0 work item |
| P0-AUTHZ-CLAIMS-001 | ADR-003 requests no permission, contacts no service, completes no AC, and advances or advertises no capability, support row, or gate. | Permission/governance/support/build manifests | Executable | Next Phase 0 work item |
| P0-AUTHZ-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity scope, folder, Office-authority, privacy, fallback, or traceability defect. | Work-item review only; no transcript or model output is stored | Required before closing P0-WI-06 | Next Phase 0 work item |

P0-WI-06 accepts a disabled permission decision contract. It requests no OAuth,
Graph, or Office permission and passes no identity, resource, add-in, privacy,
security, or release gate.

### P0-WI-07 synchronization checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-SYNC-INVENTORY-001 | The closed synchronization manifest has exact P0-WI-07, ADR-004, OWN-05/09, requirement, source, collection, and deferred-decision inventories; all collection templates remain disabled and unadvertised. | Synchronization manifest; checker and negative suite | Executable | Synchronization runtime |
| P0-SYNC-CHECKPOINT-001 | Every future selected folder/list/calendar collection has one encrypted account/collection/query-bound checkpoint with only the approved fields and scope-removal lifecycle; no cursor is configured, shared, plaintext, logged, exported, reconstructed, or reused. | Synchronization and privacy manifests | Executable decision contract | ADR-005, G-STATE, G-PRIV |
| P0-SYNC-DELTA-001 | Mail change discovery is bounded single-flight per-folder delta with opaque continuations, at-least-once application, no ordering assumption, tombstone handling, and visible bounded rebaseline only for a G-MAIL-validated invalid-cursor signature; exact fields and budgets remain gated. | Manifest; ADR-004; Microsoft delta sources | Executable decision contract | G-MAIL, ADR-006 |
| P0-SYNC-ELIGIBILITY-001 | Incoming unread-to-read, post-baseline first-observed-already-read, initial/backfill already-read, and first saved-sent-copy paths are exact and idempotent; unread waits, history is review-only, drafts/compose are excluded, and read/delivery claims are prohibited. | Manifest; ADR-004 | Executable decision contract | Mail ingestion |
| P0-SYNC-TRANSACTION-001 | Page outcomes and cursor replacement commit atomically; every item is durably safe or recoverably quarantined before advancement, whole-page failure blocks advancement, last-complete updates only at deltaLink, crash replay is idempotent, and ambiguous writes are never blindly retried. | Manifest; ADR-004; negative suite | Executable decision contract | ADR-005/009/011 |
| P0-SYNC-RESILIENCE-001 | Valid Retry-After is honored; fallback is bounded jittered backoff; items are isolated; quarantine stores only approved encrypted/content-free state; failed-without-recoverability and raw error/URL/header/body handling are prohibited. | Manifest; privacy contract; Microsoft throttling source | Executable decision contract | G-MAIL, G-PRIV |
| P0-SYNC-COVERAGE-001 | Reconciliation is selected-scope bounded; tombstone/move/rename/deletion/removal behavior and known losses are explicit; freshness is visible and no complete, instant, inactive-background, read-attention, or delivery claim is allowed. | Manifest; ADR-003/004 | Executable disclosure contract | G-MAIL |
| P0-SYNC-REPLAY-001 | Replay is explicit, visible, selected-scope bounded, reuses the same keys, never broadens permission/folders, and routes historical discoveries to review with zero automatic reminder or terminal mutation. | Manifest; negative suite | Executable decision contract | Replay implementation |
| P0-SYNC-SCHEDULING-001 | Poll configuration remains 60–900 seconds/default 60 with per-collection single-flight, adaptive finite budgets, visible inactive/stale states, and internal-only latency targets until measured G-MAIL evidence. | Manifest; ADR-004 | Executable policy contract | Scheduler and G-MAIL |
| P0-SYNC-CROSS-CONTRACT-001 | ADR-004 reconciles exactly with governance, ADR-003 permissions/folders, account binding, ADR-PRIV-001 fields/lifecycle, support/build inactivity, deferred ADR ownership, threat routing, and current Microsoft source boundaries. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-SYNC-CLAIMS-001 | ADR-004 makes zero Graph/network calls, configures no account/collection/cursor, persists no runtime state, activates no dependency/runtime, and advances no gate, AC, capability, support row, or advertised claim. | Synchronization/governance/privacy/support/build manifests | Executable | Any runtime or product claim |
| P0-SYNC-FRESH-CHECKER-001 | Fresh-context and adversarial judges report no unresolved high-severity cursor, transaction, retry, coverage, privacy, ownership, deferred-ADR, or traceability defect. | Work-item review only; no transcript or model output stored | Required before closing P0-WI-07 | Next Phase 0 work item |

P0-WI-07 accepts a disabled synchronization decision contract only. It makes no
Graph call, creates no checkpoint, and passes no identity, mail, reminder,
calendar, state, privacy, security, support, or release gate.

### P0-WI-08 protected-state checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-STATE-INVENTORY-001 | The closed P0-WI-08/ADR-005 manifest has exact owner, requirement, gate, source, dependency, secret, threat-route, and deferred-decision inventories; only ADR-005 advances from planned to accepted. | Protected-state manifest; ADR registry; checker and mutation suite | Executable decision contract | Persistence runtime |
| P0-STATE-SCHEMA-001 | ADR-PRIV-001 remains the exact record/field allowlist; every future logical field must have one exact required/null/type/catalog/version/variant/bound contract and must validate before encryption and after authenticated decrypt/migration. Unknown, duplicate, open, unbounded, silently dropped, or generic values reject; dependent domain catalogs remain unavailable. | Protected-state and privacy manifests; deterministic mutations | Executable envelope/schema boundary; runtime domain schemas remain gated | ADR-006/008/009, G-STATE |
| P0-STATE-ENVELOPE-001 | AES-256-GCM uses a 32-byte key, fresh random 12-byte Windows-CSPRNG nonce, full 16-byte tag, canonical bounded encoding, exact account/record/type/schema/ciphertext/key/algorithm AAD, conservative per-key reservation, collision rejection, early rotation, and a hard stop before the 2^32 random-IV limit. RNG, authentication, version, or bound failure releases no plaintext and fails closed. | Manifest; ADR-005; NIST/RFC/Microsoft sources; mutation suite | Executable decision contract | G-STATE, G-PRIV, G-SEC-AUDIT |
| P0-STATE-KEYS-001 | AEAD, HMAC, rollback-anchor, OAuth-token, provider, pairing-root, and session secrets have distinct exact owners, purposes, locations, references, rotation/deletion, and failure behavior. DPAPI is current-user, noninteractive, non-machine-scope, strictly validated, and never falls back to plaintext; session-only mode is ephemeral and non-mutating. | Manifest; ADR-005; DPAPI sources; auth/privacy contracts | Executable decision contract | G-ID, G-STATE, G-PRIV |
| P0-STATE-BINDING-001 | Opaque account binding, canonical AAD, per-account databases/key namespaces, DPAPI/ACL checks, and key/database/anchor comparisons reject ciphertext swaps, cross-account use, cross-user access, simple machine transfer, and unknown state. Roaming/full-profile/VM restore limits are disclosed and enter reconciliation rather than being claimed impossible. | Manifest; protected-state threat model; mutation suite | Executable decision contract | G-STATE |
| P0-STATE-TRANSACTION-001 | One FULL-synchronous SQLite transaction preserves ADR-004 page/checkpoint/operation/quarantine ordering; only complete envelopes reach SQL/WAL/SHM/journal/temp/staging, pending operation identity precedes an external request, ambiguous writes reconcile before retry, and crashes cannot create early cursor advancement or dangling quarantine references. | Manifest; ADR-004; privacy manifest; mutation suite | Executable decision contract | Persistence and ingestion runtime |
| P0-STATE-MIGRATION-001 | Closed compatibility rules and an exclusive copy-authenticate-old/validate/migrate/validate-new/fresh-encrypt/verify/atomic-replace state machine survive crash/disk-full without mixed writable generations. Rotation creates separate fresh keys/nonces, retains old decrypt-only keys until verified, and older binaries refuse newer schema/ciphertext writes or downgrade conversion. | Manifest; ADR-005; mutation suite | Executable decision contract | G-STATE, G-RELEASE |
| P0-STATE-ROLLBACK-001 | A separate protected generation anchor detects ordinary DB rollback/divergence; every startup and any DB/key/anchor/clock/tamper/restore ambiguity freezes cursor advancement and outward mutation until bounded mail/reminder reconciliation and review. Coherent profile/VM rollback is not claimed cryptographically detectable or impossible. | Manifest; ADR-005; protected-state threat model | Executable decision contract | G-STATE, G-PRIV |
| P0-STATE-LIFECYCLE-001 | Privacy retention and quarantine transitions remain exact; delete/reset/scope removal/disconnect/uninstall are reference-aware and local-only with zero Graph mutation. Disconnect removes account-bound local state/secrets and preserves the Microsoft-artifact choice; export/backup/recovery-key paths remain disabled and no physical-erasure or global-revocation claim is made. | Protected-state and privacy manifests; auth disconnect contract; mutation suite | Executable decision contract | G-STATE, G-PRIV, G-RELEASE |
| P0-STATE-PRIVACY-001 | Encryption never legitimizes prohibited content. Decrypted values cannot reach SQL, WAL, SHM, journal, temp, staging, backup, log, browser, crash, diagnostic, package, installer, CI, or repository artifacts; the diagnostic allowlist is empty and canary failure never prints the value. | Manifest; privacy/public-safety checkers; package allowlist | Executable policy contract | G-PRIV, G-SEC-AUDIT |
| P0-STATE-CROSS-CONTRACT-001 | ADR-005 reconciles exactly with governance, ADR-PRIV-001, ADR-002/003/004, support/build inactivity, threat routing, current official sources, and the still-deferred ownership of ADR-006 through ADR-013. No permission, origin, support, runtime, or hosted-state fallback is introduced. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-STATE-CLAIMS-001 | P0-WI-08 creates no DB/key/token/cache/secret/backup/migration/diagnostic/network operation, activates no dependency, completes no AC, and passes/enables/advertises no gate, capability, or support row. The persistence adapter and Microsoft-hosted state remain disabled. | Manifest; source/build/governance/support checks | Executable historical closure claim | Next Phase 0 work item |
| P0-STATE-FRESH-CHECKER-001 | Fresh-context security, privacy, and adversarial judges report no unresolved high-severity schema, cryptography, key, rollback, migration, lifecycle, privacy, claim, or traceability defect. | Work-item review only; no transcript or model output stored | Passed at P0-WI-08 closure | Next Phase 0 work item |

P0-WI-08 accepts ADR-005's disabled persistence-and-cryptography decision
contract only. It approves no runtime value, creates no protected state, and
passes no state, privacy, security-audit, release, Graph, or product gate.

P0-WI-08 closed on 2026-07-20 after all twelve executable protected-state
assertions, 161 synthetic adversarial mutations, the complete Phase 0 regression
set, the credential-free source build, package allowlist, prospective public-repo
scan, and fresh security/privacy/adversarial review passed. No review transcript,
model output, generated diagnostic, or real identifier/content is retained as
closure evidence.

### P0-WI-09 / P0-WI-09R evidence identity and anchoring checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-EVID-INVENTORY-001 | The closed P0-WI-09/ADR-006 contract and product-owner-approved P0-WI-09R repair have exact owner, requirement, scenario, gate, source, source-variant, conflict, repair-scope, and separate-decision inventories; only ADR-006 advances from planned to accepted. | Evidence manifest; ADR registry; checker and mutation suite | Executable decision contract | Evidence runtime |
| P0-EVID-SCHEMA-001 | `email_evidence_ref` maps every ADR-PRIV-001 field exactly once with closed required/null/type/catalog/version/bound/order rules. Unknown, duplicate, missing, invalid, oversized, cross-bound, empty, or non-scalar-range values reject before encryption and after authenticated decryption. | Evidence/privacy manifests; deterministic mutations | Executable logical schema | G-STATE, G-PRIV |
| P0-EVID-IDENTITY-001 | Primary identity is a case-sensitive Graph immutable ID bound to opaque account and an exact purpose-separated ADR-005 mailbox HMAC over closed validated claim bytes. Raw claims never persist. Folder, conversation, subject, Internet Message-ID, web link, and content digest are non-identity; default REST ID is encrypted candidate-only fallback and cross-account use rejects before any call. | Evidence/protected-state manifests; ADR-002/005/006; threat model | Executable decision contract | G-ID, G-MAIL, G-STATE |
| P0-EVID-GRAPH-001 | Exact v1.0 paths, immutable/body preferences, ordered selected fields, prohibited fields, and opaque continuation behavior are closed. Every accepted message enumerates the exact metadata-only attachment endpoint/query to terminal completion regardless of `hasAttachments`, preserving inline-only names without fetching bytes. `translateExchangeIds` and its unapproved directory permission remain prohibited. | Evidence/permission/sync manifests; current official Microsoft message/attachment sources | Executable request contract; calls remain disabled | G-MAIL |
| P0-EVID-ANCHOR-001 | Evidence uses a versioned exact Graph projection and inert WHATWG-fragment HTML-to-text profile with an exact depth-first entry/exit event algorithm, table-cell separators, quote routing, whitespace finalization, and public sibling/nested/table/inline-attachment/Unicode/paging vectors. Closed numeric maps, Unicode-scalar ranges, and full purpose-separated HMAC-SHA-256 anchors remain fixed. A digest match is never identity. | Evidence/protected-state manifests; ADR-006; deterministic canonicalization mutations | Executable decision contract | Mail canonicalizer; G-STATE/G-PRIV |
| P0-EVID-RESOLUTION-001 | Binding rejection precedes calls; primary resolution requires all three anchors; changed content freezes automation. Only a terminal enumeration of the complete authorized bounded universe can classify zero/one/many candidates; incomplete/capped searches never offer a singleton. Automatic reanchoring and stored content fallback are prohibited, and a confirmed replacement is one protected transaction. | Evidence manifest; threat model; mutation suite | Executable state-machine contract | Resolver runtime |
| P0-EVID-LIFECYCLE-001 | Same-primary-mailbox move behavior, including the ordinary Archive folder, remains G-MAIL-tested; the separate in-place/online archive mailbox is unsupported. Copies are always distinct; export/import do not inherit identity; draft/compose creates no evidence; only first validated saved-sent observation may create evidence; delete/retention/permission loss never proves delivery, closure, or failure. | Evidence manifest; ADR-004/006; Microsoft source conflict inventory | Executable decision contract | G-MAIL |
| P0-EVID-OFFICE-001 | Graph and Office authority remain separate. EWS IDs and REST conversion are transient; only a bound Graph response can supply a persistent immutable locator. Graph-to-Office display remains gated and must use asynchronous callback-confirmed display. `webLink` is transient, single-parse exact-origin top-level navigation rejecting credentials, ports, controls, IP/trailing-dot/punycode drift; review links contain only a non-secret local handle. | Evidence/permission manifests; official Office/Graph sources | Executable decision contract | G-ADDIN, G-MAIL |
| P0-EVID-UNAVAILABLE-001 | Changed, ambiguous, stale, missing, and navigation-unavailable projections expose only safe codes/non-content facts, freeze dependent automation, preserve abstract lineage, and allow explicit recovery actions without reconstructing task/person/source text or inferring closure/failure. | Evidence manifest; product spec §8.5; AS-10 | Executable behavior contract; AS-10 unpassed | Resolver/UI runtime |
| P0-EVID-PRIVACY-001 | Raw IDs are permitted only inside exact AEAD locator fields; HMACs remain keyed/full/encrypted; all selected content, IDs, links, offsets, and anchor inputs are bounded-memory only. Diagnostic allowlist and prohibited canary count are zero; residual OS/Office exposure is disclosed. | Evidence/privacy/protected-state manifests; threat model | Executable privacy contract | G-PRIV, G-SEC-AUDIT |
| P0-EVID-CROSS-CONTRACT-001 | ADR-006 reconciles exactly with OWN-05/07, ADR-003/004/005/PRIV-001, governance/threat routing, support/build inactivity, current Microsoft conflicts, and still-deferred ADR-009/010 ownership without adding scope, runtime, support, or alternate source variants. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-EVID-CLAIMS-001 | P0-WI-09/P0-WI-09R make zero Graph/Office/product-network calls, create no record, and activate no dependency, account, folder, AC, scenario, gate, capability, or support row. The exact product/package-script execution surface and aggregate fingerprint cover runtime source, generator/config/test/schema inputs, Cargo manifests/lock, and the full npm lock graph. Exact imports, root package fields, npm scripts, and dependency allowlists disable implicit Prettier/EditorConfig discovery and reject `node:https`, `undici`, `got`, arbitrary dependencies/scripts, dynamic loaders, process launchers, and all reachable drift before secondary signatures. | Evidence/governance/support/build manifests; exact workspace fingerprint and synthetic bypass mutations | Executable | Any runtime or product claim |
| P0-EVID-FRESH-CHECKER-001 | Fresh-context privacy/security and adversarial judges report no unresolved material identity, scope, anchor, Unicode, attachment, runtime-claim, copy/move/send, navigation, unavailable, privacy, or traceability defect. | Work-item review only; no transcript or model output stored | Passed at P0-WI-09R closure | Next Phase 0 work item |

P0-WI-09 accepts ADR-006's disabled evidence identity/anchoring decision
contract only. P0-WI-09R is the product-owner-approved repair continuation for
the exact runtime inventory, deterministic DOM events, and inline-safe attachment
enumeration defects; it adds no capability or permission. Neither item performs
a Graph/Office request or creates a source record.
G-MAIL, G-CAL, G-ADDIN, G-STATE, and G-PRIV remain unrun; AS-10, AS-14,
AS-16, AS-19, and every product acceptance criterion remain unpassed.

P0-WI-09R closed on 2026-07-20 after the exact source build, all Phase 0
baseline checks, 694 synthetic/adversarial rejection cases, package allowlist,
prospective 78-blob public-repository scan, and three independent cold-start
closure judgments passed. Closure records only counts and stable IDs; no prompt,
transcript, model output, mailbox content, real identifier, or generated
diagnostic artifact is retained.

### P0-WI-10 model-provider boundary checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-MODEL-INVENTORY-001 | P0-WI-10/ADR-007 has exact OWN-08, requirement, acceptance-scenario, gate, profile, source, runtime, and claim inventories; only ADR-007 advances from planned to accepted. | Model manifest; governance and ADR registries; checker and mutations | Executable decision contract | Provider runtime |
| P0-MODEL-PROFILES-001 | Disabled is the sole default. Built-in `ollama_local` and `ollama_cloud` plus optional `approved_https` remain unadvertised and disabled behind exact authorities, paths, credentials, transport rules, fallbacks, and named gates; approved HTTPS cannot substitute for the hosted Ollama path. | Model/support manifests; current official Ollama sources | Executable profile contract | G-MODEL, G-PRIV |
| P0-MODEL-REQUEST-001 | One changed-message projection and at most four deterministic relevance-selected context projections use closed component/count/scalar/byte bounds. Whole mailbox/thread, attachment bytes, linked content, URLs, credentials, unrelated recipients, Graph locators, tools, functions, images, and retrieval are prohibited. | Model manifest; ADR-007; bounded synthetic mutations | Executable request contract | Model adapter |
| P0-MODEL-RESPONSE-001 | A bounded non-streaming response passes strict UTF-8, single-JSON-value, duplicate-member, unknown-field, schema/catalog/bound, supplied-handle, Unicode range, transient evidence, participant, deterministic-time, consistency, and semantic checks in exact order. Provider schema enforcement is defense in depth only; invalid output causes a typed safe result and zero mutation. | Model manifest; ADR-007; malformed/schema-valid adversarial fixtures | Executable response contract | G-MODEL |
| P0-MODEL-NETWORK-001 | Local transport is exact direct IPv4 loopback. External transport is exact consented public HTTPS with valid TLS, address-policy resolution and connected-peer revalidation. Redirects, implicit/ambient proxies, cross-origin authorization, unsafe addresses, custom trust, content retries, and oversized/late responses fail closed. | Model manifest; threat model; transport mutation suite | Executable network contract; calls remain disabled | G-MODEL, G-SEC-AUDIT |
| P0-MODEL-CONSENT-001 | Provider, exact authority, model, transmitted categories/count, provider privacy responsibility, local/external boundary, request/deadline/closure sensitivities, and per-claim review thresholds are disclosed. Safe defaults are pinned by `model-sensitivity-v1`; changes offer bounded review-only replay, never start replay automatically, and authorize no terminal or external mutation. Provider/origin/path/model/field/schema/policy/capability drift invalidates consent; credential replacement requires content-free revalidation, deletion disables use, neither triggers replay, and keys remain write-only. | Model/privacy/protected-state manifests; ADR-007 | Executable consent and settings-disclosure contract | G-MODEL, G-PRIV |
| P0-MODEL-AUTHORITY-001 | The model is an untrusted transient hypothesis producer, never Graph/Office/lifecycle/closure/scope/prompt/endpoint/tool authority. Schema-valid output is not semantically safe; automation remains unavailable until ADR-011 and the applicable independent G-AUTO gate pass, and drift permits review-only replay. | Model manifest; product spec; semantic adversarial policy | Executable authority contract | G-MODEL, G-AUTO/G-AUTO-FULL |
| P0-MODEL-PRIVACY-001 | Prompt, request, response, output, rationale, transcript, embedding, endpoint detail, key/header/body, source identifier, and provider SDK text never persist or enter diagnostics. Only fixed codes and bounded non-content counters are allowed; application-controlled canary count is zero outside the exact enabled test request. | Model/privacy/protected-state manifests; canary mutations | Executable privacy contract | G-PRIV, G-SEC-AUDIT |
| P0-MODEL-SOURCES-001 | The dated source inventory pins current official Ollama local/cloud bases, direct API-key authentication, local cloud-disable boundary, model listing, API versioning caveat, and the current Cloud structured-output limitation. Application validation never relies on a provider-side schema guarantee. | Model/support manifests; official Ollama documentation | Dated executable research contract | Source freshness review |
| P0-MODEL-CROSS-CONTRACT-001 | ADR-007 reconciles exactly with OWN-08, ADR-PRIV-001/005, governance/threat routing, support profiles, privacy fields, protected credential lifecycle, and build inactivity without adding Graph scope, runtime dependency, support row, acceptance claim, or alternate provider. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-MODEL-CLAIMS-001 | P0-WI-10 makes zero provider/Graph/Office calls, accepts no credential, creates no record, and enables or advertises no dependency, profile, origin, permission, capability, support row, acceptance criterion/scenario, or gate. AS-12, AS-16, and AS-22 remain unpassed. | Model/governance/support/build manifests; public-repository and package gates | Executable | Any provider or product claim |
| P0-MODEL-FRESH-CHECKER-001 | Fresh-context privacy/security, transport, and adversarial judges report no unresolved material scope, SSRF/proxy/redirect, credential, consent, schema/evidence, semantic-authority, drift, privacy, source, claim, or traceability defect. | Work-item review only; no transcript or model output stored | Passed at P0-WI-10 closure | Next Phase 0 work item |

P0-WI-10 accepts only the disabled ADR-007 decision contract. Provider
transport, credentials, content transmission, model-dependent analysis,
automation calibration, support claims, acceptance scenarios, and every named
gate remain inactive or unrun.

P0-WI-10 closed on 2026-07-20 after the exact source build, all Phase 0
deterministic checkers, 790 synthetic/adversarial rejection cases, the
metadata-only package allowlist, a prospective 84-blob public-repository scan,
and three cold-start closure judgments passed. The judgments are ephemeral
work-item evidence; no reviewer prompt, transcript, or model output is tracked
or packaged.

### P0-WI-11 policy and state model checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-POLICY-INVENTORY-001 | P0-WI-11/ADR-008 has exact OWN-03, owned/consumed requirement (including deferred OL-REM-010/OL-REM-017), AC, scenario, gate, authoritative product-spec/implementation-plan source, input-authority, separate-decision, runtime, and claim inventories; only ADR-008 advances from planned to accepted. | Policy manifest; product spec; implementation plan; governance and ADR registries; checker and mutations | Executable decision contract | Policy runtime |
| P0-POLICY-SCHEMA-001 | Loop, deadline-evidence, and transition records map every ADR-PRIV-001 field exactly once with closed required/nullable/type/catalog/order/count/version rules. Unknown, generic, open, readable, or unbounded shapes reject before encryption and after authenticated decode. | Policy/privacy/persistence manifests | Executable logical-schema contract; runtime disabled | G-STATE, G-PRIV |
| P0-POLICY-FACETS-001 | Every loop facet and secondary chip uses a closed catalog; provenance/review sets are canonical; primary display precedence is terminal, possible closure, review, overdue, approaching, unresolved deadline, candidate, active. | Policy manifest; ADR-008 | Executable catalog check | Domain runtime |
| P0-POLICY-LEGALITY-001 | Full-state validation enforces resolution/terminal equivalence, open-only closure review and aging, durable undated choice, quarantine recoverability, evidence-loss freeze, reminder/lifecycle separation, terminal later-evidence behavior, and user-correction precedence. The 420 core tuples have exactly 41 legal projections. | Policy manifest; exhaustive deterministic checker | Executable property check | Domain runtime |
| P0-POLICY-PROMOTION-001 | Exact evidence/confidence/ambiguity predicates establish open versus candidate loops; provenance accumulates; history is review-only; model confidence alone never promotes; invalid hypotheses cause no transition. | Policy manifest; product-spec promotion table | Executable decision table | Detection runtime |
| P0-POLICY-DEADLINE-001 | Requested/promised/inferred/user-supplied/operative/reminder deadlines remain separate; precedence, ambiguity, precision, policy-change confirmation, defer/no-deadline, event-relative, soft, and aging rules are exact and never fabricate precision. | Policy manifest; operative-deadline and aging tables | Executable decision table | Deadline runtime |
| P0-POLICY-COMMANDS-001 | Candidate, closure, decline, moot, transfer, keep-open, evidence-request/remap/select, outside-completion, dismissal, and reopen commands have closed from-state/input/result/external-effect rules. | Policy manifest; command matrix | Executable command contract | Review runtime |
| P0-POLICY-CONFIRMATION-001 | Every non-none terminal resolution and reopen is user-command-only. Inferred closure remains open/possible; shared/assisted/retained/unclear stays open; candidate terminal commands never silently promote first. | Policy manifest; ADR-008; model boundary | Executable invariant | Any lifecycle claim |
| P0-POLICY-IDEMPOTENCY-001 | Automated replay keys source/version/policy/target. User handling checks key before version: identical replay is a no-op, collision rejects, unseen stale version conflicts, and one valid command appends one transition/version increment atomically. | Policy manifest; transition contract | Executable algorithm contract | Domain runtime |
| P0-POLICY-STALE-REOPEN-001 | Keep-open suppresses only one loop/kind/versioned-source/policy identity; later evidence remains reviewable. Reopen retains history and old command keys cannot re-terminal a new generation. | Policy manifest; ADR-008 | Executable state contract | Review runtime |
| P0-POLICY-CORRECTION-001 | Current correction and optional future rule require separate confirmations; future rules are typed/prospective, replay is bounded review-only, duplicate merge is atomic/non-lossy, and no text, training, profiling, or historical mutation is implied. | Policy/privacy manifests; corrections flow | Executable correction contract | Settings/review runtime |
| P0-POLICY-PRIVACY-001 | Only approved loop/deadline/transition fields exist; readable source/generated text, raw Microsoft IDs/URLs, free-text reasons, model material, generic/open/unbounded values, and content-bearing diagnostics are prohibited. | Policy/privacy/persistence manifests; canary mutations | Executable privacy contract | G-PRIV |
| P0-POLICY-DEFERRED-001 | ADR-009 exclusively owns reminder adapters/operations/ownership/conflicts; ADR-011 owns mode execution/evaluation/feature flags. ADR-008 cannot activate Graph/Office or hybrid/automatic behavior. | Policy/protected-state/governance manifests | Executable ownership boundary | ADR-009, ADR-011 |
| P0-POLICY-CROSS-CONTRACT-001 | ADR-008 reconciles with ADR-004/005/006/007/PRIV-001, governance, support, build, privacy, persistence, evidence, and model boundaries without widening fields, scope, permissions, runtime, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-POLICY-CLAIMS-001 | P0-WI-11 accepts no logical record, creates no persistent state, makes no model/Graph/Office call, requests no permission, and enables/advertises/completes/passes no dependency, capability, support row, AC, scenario, or gate. | Policy/governance/support/build manifests; package/public gates | Executable | Any product or runtime claim |
| P0-POLICY-FRESH-CHECKER-001 | Fresh-context state, privacy, scope, and adversarial judges report no unresolved material schema, facet, legality, promotion, deadline, command, confirmation, idempotency, stale/reopen, correction, privacy, deferred-ownership, claim, or traceability defect. | Work-item review only; no transcript or model output stored | Passed at P0-WI-11 closure | Next Phase 0 work item |

P0-WI-11 accepts only the disabled ADR-008 deterministic decision contract.
Policy execution, persistent records, Graph/Office/model calls, reminder adapters,
hybrid/automatic modes, acceptance results, support claims, and all named gates
remain inactive or unrun.

P0-WI-11 closed on 2026-07-21 after all 11 Phase 0 deterministic checkers,
860 synthetic/adversarial rejection cases, the metadata-only package allowlist,
a prospective 89-blob public-repository scan, and three final fresh-context
closure judgments passed. The unchanged product source remains covered by the
P0-WI-10 exact source-build pass; current add-in tests, formatting, and lint also
passed. The judgments are ephemeral work-item evidence; no reviewer prompt,
transcript, or model output is tracked or packaged.

### P0-WI-12 reminder adapter checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-REMINDER-INVENTORY-001 | P0-WI-12/ADR-009 has exact OWN-03/04/05/07, owned/consumed requirement, AC, scenario/dependency, gate, source, input-authority, separate-decision, runtime, and claim inventories; only ADR-009 advances from planned to accepted. | Reminder manifest; specification/plan; governance and ADR registries | Executable decision contract | Adapter runtime |
| P0-REMINDER-SCHEMA-001 | Reminder link, operation ledger, and user-authored artifact reference map every ADR-PRIV-001 field exactly once with closed field, required, nullable, bound, and readable-content rejection rules. | Reminder/privacy manifests | Executable logical-schema contract; runtime disabled | G-STATE, G-PRIV |
| P0-REMINDER-CATALOGS-001 | Adapter, artifact, operation, state, reconciliation, failure, field-mask, ownership, version, source, and Calendar catalogs are exact and contain no generic send/delete/bulk/invitation-response authority. | Reminder manifest | Executable catalog check | Adapter runtime |
| P0-REMINDER-TODO-BASELINE-001 | Product To Do uses only disabled delegated Tasks.ReadWrite after explicit enablement and gates; Tasks.Read is validation-only; launch reconciliation is ordinary bounded complete enumeration with explicit owned-list selection and no delta, guessed-list, shared, personal, app-only, or Calendar-fallback claim. | Reminder/permission manifests; G-TODO research | Executable decision contract | G-TODO |
| P0-REMINDER-OPERATION-PROTOCOL-001 | A pending encrypted ledger entry precedes every request and commits exact account, adapter, collection, loop/source generations, operation/field mask, versions, transient digests, policy/auth, and recreate generation; replay, collision, stale/authority/rollback/secure-store, retry, restart, cursor, and terminal-local rules are closed. | Reminder/persistence/policy manifests | Executable protocol contract | G-STATE, adapter gates |
| P0-REMINDER-AMBIGUOUS-WRITE-001 | The local key is not remote idempotency; marker support is gate-proven, derivation is recoverable and non-secret, only its HMAC persists, and complete zero/one/many/incomplete/missing outcomes never collapse into blind retry or automatic recreate. | Reminder manifest; synthetic ambiguity mutations | Executable reconciliation contract | G-TODO, G-CAL |
| P0-REMINDER-OWNERSHIP-001 | Detected title/body/due begin as service projections; direct edit transfers ownership and due creates the approved override; manual-artifact fields are always user-authoritative; service updates preserve every user-owned sibling. | Reminder/privacy manifests; product spec §8.4 | Executable ownership matrix | Adapter runtime |
| P0-REMINDER-DIRECT-EDIT-001 | Completion and deletion change only reminder/source state, never close or prove failure, and automatic delete is prohibited. Reminder time is transiently recomputed with no invented persistent field/digest/override; independent remote edits are unmanaged pending privacy revision. | Reminder/policy/privacy manifests | Executable direct-edit contract | G-PRIV, adapter gates |
| P0-REMINDER-CONFLICT-001 | Every update refetches and compares owned-field digests and versions; stale or changed state produces a visible conflict and zero PATCH until conditional writes are proven. | Reminder manifest; synthetic TOCTOU mutations | Executable conflict contract | Adapter gates |
| P0-REMINDER-MANUAL-SOURCE-001 | Manual creation keeps the readable draft memory-only and atomically creates source, loop relation, and reminder link only after one proven artifact; cancellation/failure/ambiguity never creates or automatically recreates a durable manual loop. | Reminder/privacy/evidence manifests | Executable manual-source contract | G-STATE, G-PRIV, adapter gates |
| P0-REMINDER-CALENDAR-SAFETY-001 | Calendar scope remains unresolved; a future reminder event is self-only with zero attendees/response/online meeting/mail side effect; invitations are evidence-only and never mutated; Calendar has no completion concept or preview support claim. | Reminder/permission/evidence manifests; G-CAL research | Executable disabled contract | G-CAL, G-MAIL |
| P0-REMINDER-REVIEW-LINK-001 | Review links contain one non-secret opaque loop handle with no authority; tokens, secrets, Microsoft/account IDs, content, URLs, or evidence links are prohibited and activation remains ADR-010/G-ADDIN-owned. | Reminder/evidence manifests | Executable disabled contract | ADR-010, G-ADDIN |
| P0-REMINDER-PRIVACY-001 | Only the three approved record types and exact fields exist; readable content, raw identifiers/markers/requests/responses, free text, generic/open/unbounded values, and prompt/model material are prohibited; diagnostics are fixed non-content codes and counts. | Reminder/privacy/persistence manifests; canary mutations | Executable privacy contract | G-PRIV, G-SEC-AUDIT |
| P0-REMINDER-DEFERRED-001 | ADR-003/004/005/006/008/PRIV-001 remain input authorities; ADR-010/011/013 and G-TODO/G-CAL own their separate activation, mode, self-email, endpoint, scope, marker, and conditional-write decisions. | All Phase 0 contracts | Executable ownership boundary | Dependent ADRs and gates |
| P0-REMINDER-CROSS-CONTRACT-001 | ADR-009 reconciles with permission, synchronization, persistence, evidence, policy, governance, support, and build boundaries without widening fields, scope, runtime, permissions, dependencies, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-REMINDER-CLAIMS-001 | P0-WI-12 accepts no logical record, creates no persistent state, makes no Graph/Office call, requests no permission, and enables/advertises/completes/passes no dependency, capability, support row, AC, scenario, gate, collection behavior, marker, or conditional write. | Reminder/governance/support/build manifests; package/public gates | Executable | Any product or runtime claim |
| P0-REMINDER-FRESH-CHECKER-001 | Fresh-context privacy, protocol, scope, and adversarial judges must report no unresolved material schema, operation, ambiguity, ownership, direct-edit, conflict, manual-source, Calendar, review-link, privacy, deferred-ownership, claim, or traceability defect. | Work-item review only; no prompt, transcript, or model output stored | Passed at P0-WI-12 closure | Next Phase 0 work item |

P0-WI-12 accepts only the disabled ADR-009 reminder-adapter decision contract.
Adapter runtime, logical/persistent records, Graph/Office calls, permissions,
automation, support claims, acceptance results, and all named gates remain
inactive, empty, disabled, unavailable, or unrun.

P0-WI-12 closed on 2026-07-21 after the bounded retry-contract repair, all 17
deterministic reminder-adapter checks, 73 synthetic/adversarial rejection
cases, the complete Phase 0 deterministic checker regression, the
metadata-only package allowlist, a prospective staged public-repository scan,
and three fresh-context closure judgments passed. The unchanged product source
remains covered by the P0-WI-10 exact source-build pass; the current add-in
tests, formatting, and lint also passed. The judgments are ephemeral work-item
evidence; no reviewer prompt, transcript, or model output is tracked or
packaged.

### P0-WI-13 add-in bridge checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-BRIDGE-INVENTORY-001 | P0-WI-13/ADR-010 has exact OWN-01, owned/consumed requirement (OL-UX-006/007 owned, OL-REM-016 consumed from ADR-009), AC, gate, source, input-authority, separate-decision, runtime, and claim inventories; only ADR-010 advances from planned to accepted. | Bridge manifest; specification/plan; governance and ADR registries | Executable decision contract | Bridge runtime |
| P0-BRIDGE-TRANSPORT-001 | Named pipes are preferred for companion-internal/native-UI traffic; a same-user loopback web bridge is the sole Office add-in candidate, proven only by the G-ADDIN spike; exact transport, host/port, certificate mechanics, discovery, and per-client behavior stay `unresolved_pending_G-ADDIN`, and no unproven reachability/authority assumption is pinned. | Bridge manifest; implementation-plan §5; product-spec G-ADDIN gate text | Executable disabled contract | G-ADDIN |
| P0-BRIDGE-BOOTSTRAP-001 | First pairing binds packaged companion, add-in origin/client, OS user, opaque account reference, single-use nonce, expiry, and an explicit user-verifiable action; race, replay, and brute-force defenses are closed; no bootstrap value may sit in a URL, Office roaming settings, localStorage, or repository configuration. | Bridge manifest; synthetic bootstrap mutations | Executable protocol contract | G-ADDIN |
| P0-BRIDGE-SESSION-001 | Sessions are least-authority, command-scoped, memory-only, rotated on reconnect, and revoked on account change; a stale or prior-account session is rejected; the same-user-malware limitation is explicit. | Bridge manifest; synthetic session mutations | Executable session contract | G-ADDIN |
| P0-BRIDGE-CERTIFICATE-001 | No mechanism may install machine-wide trust or a general-purpose trusted root with a retained signing key; any selected certificate is current-user-only, name-limited, non-exportable-key, lifecycle-managed, and completely removed on disconnect/uninstall. | Bridge manifest; product-spec G-ADDIN gate text | Executable certificate contract | G-ADDIN |
| P0-BRIDGE-REVIEW-LINK-001 | Review-link activation resolves one random non-secret opaque loop handle through an authenticated per-user account-bound session; the ADR-009 prohibited list is unchanged and unsupported clients omit the link or show a fixed safe fallback. | Bridge/reminder manifests | Executable disabled contract | ADR-009, G-ADDIN |
| P0-BRIDGE-CONTENT-001 | No mailbox content, identifier, token, or authority-bearing URL may reach browser localStorage, IndexedDB, service-worker cache, a URL, console output, analytics, or a crash report on any add-in surface. | Bridge manifest; canary mutations | Executable content contract | G-PRIV |
| P0-BRIDGE-FALLBACK-001 | Native companion status/review/recovery is the fallback whenever the bridge or add-in is unavailable; a failed G-ADDIN blocks the Outlook-add-in MVP and routes to OWN-01 rather than silently substituting native UI. | Bridge manifest; support-matrix fallback rule | Executable fallback contract | G-ADDIN |
| P0-BRIDGE-PRIVACY-001 | No new persisted database record is introduced; any future pairing root stays in ADR-005's reserved pairing-root.dpapi blob and session secrets remain memory-only; a future persisted bridge record requires a separate ADR-PRIV-001 revision. | Bridge/persistence/privacy manifests | Executable privacy contract | G-PRIV, G-STATE |
| P0-BRIDGE-CROSS-CONTRACT-001 | ADR-010 reconciles with ADR-005's pairing-root ownership, ADR-006's Office/link boundary, ADR-009's review-link deferral, the governance registry (ADR-010 and ADR-011 accepted; ADR-012/013 still planned), the support-matrix client floor, and build-skeleton inactivity without widening fields, scope, permissions, runtime, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-BRIDGE-CLAIMS-001 | P0-WI-13 accepts no logical record, creates no persistent state, makes no network/Graph/Office call, requests no permission, and enables/advertises/completes/passes no dependency, capability, support row, AC, scenario, gate, transport, or certificate mechanic. | Bridge/governance/support/build manifests; package/public gates | Executable | Any product or runtime claim |
| P0-BRIDGE-FRESH-CHECKER-001 | Fresh-context security, privacy, governance, and adversarial judges must report no unresolved material transport, bootstrap, session, certificate, review-link, content, fallback, privacy, or traceability defect before closure. | Work-item review only; no prompt, transcript, or model output stored | Passed at P0-WI-13 closure | Next Phase 0 work item |

P0-WI-13 accepts only the disabled ADR-010 add-in-bridge decision contract.
Bridge runtime, a deployed manifest, an active listener or pipe, an issued
certificate, a created pairing, requested permissions, support claims,
acceptance results, and all named gates remain inactive, empty, disabled,
unavailable, or unrun.

P0-WI-13 closed on 2026-07-21 after all 12 deterministic bridge checks, 72
synthetic/adversarial rejection cases, the complete Phase 0 deterministic
checker regression, the metadata-only package allowlist, a prospective staged
public-repository scan, and four fresh-context closure judgments passed. The
unchanged product source remains covered by the P0-WI-10 exact source-build
pass; the current add-in tests, formatting, and lint also passed. The
judgments are ephemeral work-item evidence; no reviewer prompt, transcript, or
model output is tracked or packaged.

### P0-WI-14 automation and evaluation checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-AUTOMATION-INVENTORY-001 | P0-WI-14/ADR-011 has exact OWN-03, owned requirement (OL-REM-010/017, exclusive of any other manifest's ownership claim), AC/scenario, gate, source, input-authority, separate-decision, runtime, and claim inventories; only ADR-011 advances from planned to accepted. | Automation manifest; specification/plan; governance and ADR registries | Executable decision contract | Automation runtime |
| P0-AUTOMATION-MODES-001 | The three OL-REM-017 modes are a closed catalog: confirmation-first requires an explicit user action for every creation/service update; hybrid auto-creates only the three narrow explicit categories with a resolved deadline, high confidence, deterministic identity, and no quote/delegation/coreference ambiguity, and auto-updates only OpenLoops-owned fields; automatic requires opt-in plus G-AUTO-FULL and still keeps candidate/ambiguous/identity/delegation/quote/undated/historical/stale-write cases review-only. | Automation manifest; product-spec §6.8 | Executable mode contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-STRATA-001 | No mode reading ever admits a candidate, ambiguous, undated, historical, medium/low-confidence, identity-ambiguous, delegation-ambiguous, or quote-ambiguous case to automatic mutation; inferred closure and external communication are never automatic in any mode. | Automation manifest; synthetic stratum-widening mutations | Executable eligibility contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-FLAGS-001 | `hybrid_enabled` and `automatic_enabled` default false and flip only after their named gate passes plus an accountable owner release decision; no code default, configuration drift, mode-change UI, or settings preference alone flips a flag. | Automation manifest; synthetic flag mutations | Executable flag-topology contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-CORPUS-001 | The release-judge corpus is wholly synthetic, sealed outside the repository/implementation-LLM context/maker workflow, split by conversation family, at least 30% difficult/ambiguous and at least 20% quoted-history, with declared per-stratum sample sizes, a lower-bound confidence procedure, immutable hashes/version, a pinned maximum tuning-attempt budget, and contamination checks; "zero in 10,000" is framed as an observation, never a guarantee. | Automation manifest; implementation-plan §8.2 | Executable corpus-governance contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-CALIBRATION-001 | Provider, model digest/label, schema, prompt, policy, review-threshold, or category drift invalidates calibration and reverts any already-flipped flag to confirmation-first pending re-evaluation; only an explicit review-only replay follows drift. | Automation manifest; ADR-007 drift rules | Executable calibration contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-ROLLBACK-001 | Rollback to confirmation-first is instant, unilateral, lossless, and unconditional regardless of flag/gate state, and never mutates, recreates, or deletes a historical batch, artifact, or loop transition. | Automation manifest; synthetic rollback mutations | Executable rollback contract | G-AUTO, G-AUTO-FULL |
| P0-AUTOMATION-PRIVACY-001 | No corpus content, label, prediction, prompt, transcript, or model output may enter the repository, a package, or a diagnostic artifact; only sanitized aggregate counts and stable non-content identifiers are approved; reported metrics match implementation-plan §8.2 and imply no persistent user profiling. | Automation manifest; canary mutations | Executable privacy contract | G-PRIV |
| P0-AUTOMATION-CROSS-CONTRACT-001 | ADR-011 reconciles with ADR-007's drift rules, ADR-008's deferral wording, ADR-009's hybrid strata, the governance registry (ADR-011 accepted; ADR-012/013 still planned), the support matrix, and build-skeleton inactivity without widening fields, scope, runtime, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-AUTOMATION-CLAIMS-001 | P0-WI-14 enables no mode, flips no flag, runs no evaluation, creates no corpus, and enables/advertises/completes/passes no capability, support row, AC, scenario, gate, or calibration result. | Automation/governance/support/build manifests; package/public gates | Executable | Any product or runtime claim |
| P0-AUTOMATION-FRESH-CHECKER-001 | Fresh-context safety, evaluation-integrity, governance, and adversarial judges must report no unresolved material mode, eligibility-stratum, flag, corpus-governance, calibration, rollback, privacy, or traceability defect before closure. | Work-item review only; no prompt, transcript, or model output stored | Passed at P0-WI-14 closure | Next Phase 0 work item |

P0-WI-14 accepts only the disabled ADR-011 automation-and-evaluation decision
contract. No mode is enabled, no evaluation is run, no corpus is created, no
flag is flipped, and no acceptance criterion, scenario, or named gate
advances.

P0-WI-14 closed on 2026-07-21 after all 11 deterministic automation checks, 48
synthetic/adversarial rejection cases, the complete Phase 0 deterministic
checker regression, the metadata-only package allowlist, a prospective staged
public-repository scan, and four fresh-context closure judgments passed. The
unchanged product source remains covered by the P0-WI-10 exact source-build
pass; the current add-in tests, formatting, and lint also passed. The
judgments are ephemeral work-item evidence; no reviewer prompt, transcript, or
model output is tracked or packaged.

### P0-WI-15 distribution and registration checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-DIST-INVENTORY-001 | P0-WI-15/ADR-012 has exact OWN-00/01/02, owned requirement (OL-NFR-010/011/012, exclusive of any other manifest's ownership claim), source, input-authority, separate-decision, runtime, and claim inventories; only ADR-012 advances from planned to accepted. | Distribution manifest; specification/plan; governance and ADR registries | Executable decision contract | Distribution runtime |
| P0-DIST-REGISTRATION-001 | BYO public-client registration is the only enabled Phase 0/source-build path with placeholder-only tracked configuration and no secret on a command line; the PKCE-does-not-authenticate-the-binary limitation is preserved regardless of registration mode. | Distribution manifest; ADR-001; research connection-options.md | Executable registration contract | G-ID |
| P0-DIST-SHARED-GOVERNANCE-001 | Shared project registration stays disabled and cannot be enabled by sign-in success, development convenience, or a passing test; its required governance-control catalog matches the research source's complete before-shared-production-registration list exactly. | Distribution manifest; research connection-options.md | Executable governance contract | G-ID, G-RELEASE |
| P0-DIST-UPDATE-TRUST-001 | Update metadata is signed independently of package signing; a trusted-key inventory supports rotation/revocation; version policy is monotonic anti-downgrade; channel binding, expiry, exact package hash/size, atomic protected staging/replacement with recovery, source-revision-verifiable provenance, and no default elevation are all exact. | Distribution manifest; ADR-005 migration/rollback rules; R-21/R-22 | Executable update-trust contract | G-RELEASE, G-SEC-AUDIT |
| P0-DIST-ARTIFACTS-001 | Every released artifact class has an explicit allowlist; SBOM and provenance are required; source maps and generated diagnostics are prohibited; npm/Cargo publishability remain disabled in Phase 0; canary/public-repository gates run at required release points and never print a suspected value. | Distribution manifest; AGENTS.md; build-skeleton manifest | Executable artifact-boundary contract | G-RELEASE |
| P0-DIST-INSTALLER-001 | The installer is per-user and unelevated; uninstall is complete, removing every ADR-010 bridge-trust/certificate/protocol registration; residual-risk disclosure follows ADR-005/ADR-PRIV-001 without a physical-erasure claim; zero Graph mutation and no bulk artifact deletion. | Distribution manifest; ADR-001/005/010/PRIV-001 | Executable installer contract | G-RELEASE |
| P0-DIST-DIAGNOSTICS-001 | The diagnostic-event allowlist stays empty; any future export requires an exact content-free schema plus ADR-012, G-PRIV, G-RELEASE, and G-SEC-AUDIT, and none of the four may be represented as satisfied by a partial subset. | Distribution/privacy manifests; ADR-PRIV-001 | Executable diagnostics contract | G-PRIV, G-RELEASE, G-SEC-AUDIT |
| P0-DIST-PRIVACY-001 | No tracked client ID, tenant ID, secret, or non-placeholder registration value exists in any fixture or example; no signing key, package identity, or update-transport mechanic is guessed ahead of G-RELEASE. | Distribution manifest; canary mutations | Executable privacy contract | G-PRIV |
| P0-DIST-CROSS-CONTRACT-001 | ADR-012 reconciles with ADR-001's registration-boundary wording, ADR-005's migration/rollback rules, ADR-010's certificate-cleanup rule, ADR-PRIV-001's diagnostics allowlist, the governance registry (ADR-012 accepted; ADR-013 still planned), the support matrix, and build-skeleton package rules without widening scope, runtime, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-DIST-CLAIMS-001 | P0-WI-15 signs no package, publishes no update, registers no shared Entra application, builds no installer, and enables/advertises/completes/passes no capability, support row, AC, scenario, or gate. | Distribution/governance/support/build manifests; package/public gates | Executable | Any product or runtime claim |
| P0-DIST-FRESH-CHECKER-001 | Fresh-context security, registration-governance, governance, and adversarial judges must report no unresolved material registration, update-trust, artifact, installer, diagnostics, privacy, or traceability defect before closure. | Work-item review only; no prompt, transcript, or model output stored | Passed at P0-WI-15 closure | Next Phase 0 work item |

P0-WI-15 accepts only the disabled ADR-012 distribution-and-registration
decision contract. Nothing is signed, packaged, published, registered, or
installed; no update mechanism exists; G-ID and G-RELEASE remain unrun, and
every acceptance criterion or scenario that depends on either gate remains
unpassed.

P0-WI-15 closed on 2026-07-21 after all 11 deterministic distribution checks,
56 synthetic/adversarial rejection cases, the complete Phase 0 deterministic
checker regression, the metadata-only package allowlist, a prospective staged
public-repository scan, and four fresh-context closure judgments passed. The
unchanged product source remains covered by the P0-WI-10 exact source-build
pass; the current add-in tests, formatting, and lint also passed. The
judgments are ephemeral work-item evidence; no reviewer prompt, transcript, or
model output is tracked or packaged.

### P0-WI-16 self-email checks

| Check ID | Exact assertion | Evidence | Status | Blocks |
|---|---|---|---|---|
| P0-SELFMAIL-INVENTORY-001 | P0-WI-16/ADR-013 has exact OWN-10, owned requirement (OL-SUM-002/003/004, exclusive of any other manifest's ownership claim), source, input-authority, separate-decision, runtime, and claim inventories; only ADR-013 advances from planned to accepted. | Self-email manifest; specification/plan; governance and ADR registries | Executable decision contract | Self-email runtime |
| P0-SELFMAIL-RECIPIENT-001 | The only legal recipient is the canonical contract-tested address of the authenticated account; its source remains unresolved pending G-SELFMAIL; arbitrary or additional recipients, a user-input-sourced recipient, Cc/Bcc, reply-to redirection, and distribution lists are prohibited. | Self-email manifest; ADR-013 | Executable recipient contract | G-SELFMAIL |
| P0-SELFMAIL-CONSENT-001 | Self-email requires separate explicit feature enablement and separate incremental `Mail.Send` consent granted only after G-SELFMAIL, matching ADR-003 exactly; disablement or consent loss stops sends before any request. | Self-email manifest; ADR-003 | Executable consent contract | G-SELFMAIL |
| P0-SELFMAIL-MARKER-001 | A verified OpenLoops-generated marker, with candidate mechanisms unresolved pending G-SELFMAIL, must survive the full round trip to the saved sent copy, verify without relying on subject text, exclude only OpenLoops-generated summaries, never exclude user-authored mail, and never carry content, an identifier, or a secret. | Self-email manifest; ADR-013 | Executable marker contract | G-SELFMAIL |
| P0-SELFMAIL-RECURSION-001 | A detected marker prevents loop creation, summary-of-summary inclusion, and re-summarization; a suppression failure fails closed to no-send rather than a best-effort filter. | Self-email manifest; ADR-013 | Executable recursion contract | G-SELFMAIL |
| P0-SELFMAIL-OPERATION-001 | The send reuses the ADR-005/ADR-009 durable-before-request operation-ledger protocol exactly; an ambiguous outcome reconciles against the ledger and the Sent Items observation before any retry; a lost response is never blindly retried; secure-store loss, rollback suspicion, account mismatch, or marker-verification failure each cause zero send requests. | Self-email manifest; ADR-004/005/009 | Executable operation contract | G-SELFMAIL |
| P0-SELFMAIL-SCHEDULE-001 | A scheduled send runs only while enabled, consented, and authenticated; missed schedules coalesce into the next eligible run with no catch-up burst. | Self-email manifest; ADR-013 | Executable schedule contract | G-SELFMAIL |
| P0-SELFMAIL-PRIVACY-001 | No summary copy, draft, or template output is stored under OL-SUM-003; content rules inherit the ADR-PRIV-001 privacy boundary, which prohibits persisting generated summaries or explanations; no marker mechanism or canonical recipient source is guessed ahead of G-SELFMAIL. | Self-email/privacy manifests; ADR-PRIV-001 | Executable privacy contract | G-PRIV |
| P0-SELFMAIL-CROSS-CONTRACT-001 | ADR-013 reconciles with ADR-003's `Mail.Send` consent wording, ADR-004's sent-copy observation rule, ADR-005/ADR-009's operation-ledger protocol, ADR-PRIV-001's generated-summary prohibition, the governance registry (ADR-013 accepted), the support matrix, and build-skeleton inactivity without widening scope, runtime, or claims. | All Phase 0 contracts; deterministic checker | Executable | Next Phase 0 work item |
| P0-SELFMAIL-CLAIMS-001 | P0-WI-16 requests no `Mail.Send` scope, sends no message, selects no marker mechanism, and enables/advertises/completes/passes no capability, support row, AC, scenario, or gate. | Self-email/governance/support/build manifests | Executable | Any product or runtime claim |
| P0-SELFMAIL-FRESH-CHECKER-001 | Fresh-context send-safety, privacy, governance, and adversarial judges must report no unresolved material recipient, consent, marker, recursion, operation, schedule, privacy, or traceability defect before closure. | Work-item review only; no prompt, transcript, or model output stored | Passed at P0-WI-16 closure | Next Phase 0 work item |

P0-WI-16 accepts only the disabled ADR-013 self-email decision contract. No
`Mail.Send` scope is requested, no message is sent, no marker mechanism is
selected, and G-SELFMAIL and G-PRIV remain unrun; every acceptance criterion
or scenario that depends on either gate remains unpassed.

P0-WI-16 closed on 2026-07-21 after all 11 deterministic self-email checks, 73
synthetic/adversarial rejection cases, the complete Phase 0 deterministic
checker regression, the metadata-only package allowlist, a prospective staged
public-repository scan, and four fresh-context closure judgments passed. The
unchanged product source remains covered by the P0-WI-10 exact source-build
pass; the current add-in tests, formatting, and lint also passed. The
judgments are ephemeral work-item evidence; no reviewer prompt, transcript, or
model output is tracked or packaged.

## Product and functional requirements

| PRD ID | Source requirement | Disposition | Specification | Planned verification | Approval |
|---|---|---|---|---|---|
| PRD-01.01 | Identify what others reasonably expect from the user from Microsoft 365 email | Specified | §§1, 5; OL-DET-* | E2E-EXPECT-* | — |
| PRD-01.02 | Determine whether later email contains typed closure evidence; unresolved remains open | Specified | OL-CLOSE-*, OL-DELEG-* | E2E-CLOSE-* | — |
| PRD-02.01 | Detect informal requests/promises/attribution/questions/deadline changes and confirmation needs | Specified | OL-DET-*, OL-DUE-*, OL-CLOSE-* | EVAL-CREATION-*, EVAL-STATE-* | — |
| PRD-03.01 | Treat mailbox as MVP evidence universe without claiming outside-email knowledge | Specified | OL-DET-007, OL-METRIC-003 | COPY-EVIDENCE-*, E2E-NO-NEGATIVE-* | — |
| PRD-04.01 | Expectations, not reminder artifacts, are source of truth | Specified | §1, OL-REM-006/011 | DOMAIN-SOURCE-* | — |
| PRD-04.02 | Empathetic, uncertainty-preserving framing | Specified | OL-UX-003 | UX-COPY-* | — |
| PRD-04.03 | High recall; expose ambiguity | Specified with thresholds | OL-DET-002, §10 | EVAL-RECALL-*, EVAL-AMBIG-* | — |
| PRD-04.04 | Every conclusion grounded in inspectable evidence | Specified | §§5.2, 6.3–6.7; OL-MODEL-003/004 | EVID-SPAN-*, E2E-NAV-* | — |
| PRD-04.05 | Minimal persistence; evidence map rather than copied evidence | Specified with material privacy decision | PD-10/11, §§5.2, 5.6 | PRIV-STATE-*, PRIV-CANARY-* | OWN-06/07 |
| PRD-04.06 | No automatic communication to another person | Specified | actor boundary; OL-REM-015; OL-SUM-002 | MUTATION-RECIPIENT-ZERO-* | — |
| PRD-04.07 | Ambient default; add-in for review/settings/evidence | Specified, add-in gated | OL-UX-*, §8.8 | ADDIN-CLIENT-*, UX-AMBIENT-* | OWN-01 |
| PRD-05.01 | Individual professional; legal/client-service context without legal dependency | Specified | §§1, 4; generic domain enums | EVAL-DOMAIN-VARIETY-* | — |
| PRD-05.02 | Private individual aid; no management/surveillance | Specified | OL-METRIC-*, OL-REWARD-002, §12 | PRIV-NO-PROFILE-* | — |
| PRD-06.01 | Open loop/candidate/request/acknowledged request/promise/attribution definitions | Specified | §§5.1, 6.3 | DOMAIN-PROVENANCE-* | — |
| PRD-06.02 | Closure evidence is typed and confirmed, not silently assumed | Specified | OL-CLOSE-001–006 | DOMAIN-CLOSURE-*, E2E-CLOSE-* | — |
| PRD-06.03 | Latest-relevant-message wins without acceptance required | Specified | OL-DUE-005 and decision table | DUE-RENEGOTIATE-* | — |
| PRD-07.01 | Detect explicit promises/direct/accepted requests/questions/soft/implied/social expectations | Specified | OL-DET-001/002 | EVAL-CATEGORY-* | — |
| PRD-07.02 | Detect aliases/@mentions/name/role/context responsibility and outgoing promises | Specified | OL-DET-001, OL-ID-* | EVAL-IDENTITY-* | — |
| PRD-07.03 | Detect relevant quoted-history obligations without duplicate | Specified | OL-DEDUP-* | EVAL-QUOTE-* | — |
| PRD-07.04 | Detect unresolved calendar invitations | Gated | OL-INV-001–006 | CAL-INVITE-*, MAIL-INVITE-* | OWN-05; G-CAL/G-MAIL |
| PRD-07.05 | A request may create a loop before acceptance; preserve provenance categories | Specified | OL-DET-001/004; §5.1 | DOMAIN-PROVENANCE-* | — |
| PRD-07.06 | All categories visible by default; configurable visibility/reminder behavior | Specified | OL-UX-005; settings §7 | UX-FILTER-*, SETTINGS-CATEGORY-* | — |
| PRD-08.01 | Multiple independently managed loops per message, each with evidence/deadline/artifact/state/closure/history | Specified | OL-DET-003/006, OL-REM-003 | EVAL-ATOMIC-*, E2E-SIBLING-* | — |
| PRD-09.01 | Recognize explicit, relative, vague, event-relative, and inferred deadlines | Specified | §§5.3, 6.6 | DUE-PARSE-*, EVAL-DUE-* | — |
| PRD-09.02 | Distinguish requested/promised/inferred/operative/reminder time | Specified | §5.3, OL-DUE-001/008 | DOMAIN-DUE-TYPES-* | — |
| PRD-09.03 | Store timezone; configurable EOD; preserve ambiguity | Specified | OL-DUE-002–004/009; §7 | DUE-TZ-DST-*, SETTINGS-TIME-* | — |
| PRD-09.04 | Prompt for missing deadline after relevant read transition | Specified with polling/backfill semantics | OL-DUE-007; OL-SYNC-010; §8.8 | E2E-NEEDS-DUE-* | OWN-09 |
| PRD-10.01 | Analyze incoming message when unread→read regardless of client | Owner-approved observable-eligibility adaptation | OL-SYNC-003/010/011 | MAIL-COALESCE-*, MAIL-FIRSTOBS-* | OWN-09 accepted; G-MAIL |
| PRD-10.02 | Reconcile missed real-time events and process promptly | Specified | OL-SYNC-006–013; OL-NFR-001/002 | SYNC-REPLAY-*, PERF-FRESH-* | — |
| PRD-11.01 | Analyze outgoing email immediately after sending | Owner-approved saved-sent-copy observation adaptation | OL-SYNC-004; flow 8.2 | MAIL-SENTCOPY-* | OWN-09 accepted; G-MAIL |
| PRD-11.02 | Compose hint is informational/nonblocking; loop active only after send | Specified | OL-UX-008; flow 8.2 | ADDIN-COMPOSE-* | G-ADDIN |
| PRD-12.01 | Analyze body/subject/participants/Cc/filenames/link labels/quotes/signatures | Specified | canonical model; OL-DEDUP-001, OL-MODEL-002 | CANON-FIELDS-* | G-MAIL |
| PRD-12.02 | Do not parse attachment/link contents; names/existence not definitive proof | Specified | OL-MODEL-002, OL-CLOSE-002 | EVAL-METADATA-NOT-PROOF-* | — |
| PRD-13.01 | Repeated quote from already received original does not duplicate | Specified | OL-DEDUP-002/007 | EVAL-QUOTE-DUP-* | — |
| PRD-13.02 | Forwarded history newly assigning user may create grounded loop | Specified | OL-DEDUP-003/004 | EVAL-FORWARD-ASSIGN-* | — |
| PRD-13.03 | Probabilistic ambiguous association routes to review | Specified | OL-DEDUP-005/006 | EVAL-ASSOC-AMBIG-* | — |
| PRD-14.01 | User-configured names/addresses/aliases/nicknames/initials/usernames/roles/teams/terms | Specified | OL-ID-001; §7 | SETTINGS-IDENTITY-* | — |
| PRD-14.02 | Contextual “you” and identity mapping reviewable/evidence-based | Specified | OL-ID-002–004 | EVAL-CONTEXT-ID-* | — |
| PRD-15.01 | Support listed PRD states and later-message transitions with references | Specified as independent facets/display mapping | §5.1, §5.4 | DOMAIN-STATE-COMBOS-* | — |
| PRD-16.01 | Detect listed closure categories including attachments/links/answers/third parties/calendar/moot | Specified/gated for calendar | OL-CLOSE-001/002, OL-INV-* | EVAL-CLOSE-*, CAL-CLOSE-* | G-CAL |
| PRD-16.02 | Present original/closure evidence, explanation, confirm/keep/remap/select evidence | Specified | OL-CLOSE-003–005; command matrix | UX-CLOSURE-CMDS-* | — |
| PRD-16.03 | One message may close one loop but not siblings | Specified | OL-CLOSE-005 | E2E-SIBLING-CLOSE-* | — |
| PRD-16.04 | Declined expectation is a typed closure outcome | Specified | OL-CLOSE-001/008; command matrix | E2E-DECLINE-*; AS-20 | — |
| PRD-17.01 | Manual completion requires evidence choice or completed outside email | Specified | OL-CLOSE-006 | UX-MANUAL-COMPLETE-* | — |
| PRD-17.02 | Classify not mine/false/no longer relevant/duplicate/delegated/moot/other | Specified | OL-CLOSE-007; §8.9 | UX-CORRECTION-* | — |
| PRD-17.03 | Corrections improve future behavior when available | Specified as explicit optional rules, no model training | PD-15; §8.9 | SETTINGS-CORRECTION-RULE-* | — |
| PRD-18.01 | Distinguish transfer/shared/assisted/accountable and authenticated-user Jordan cases | Specified | OL-DELEG-001/002; command matrix | EVAL-DELEG-* | — |
| PRD-19.01 | User may choose To Do or calendar artifact | Specified; calendar gated | OL-REM-001/002 | TODO-E2E-*, CAL-E2E-* | OWN-04; G-TODO/G-CAL |
| PRD-19.02 | Artifact fields include title/deadline/reminder/evidence/review/status | Specified where client supports | OL-REM-004/005 | REM-FIELDS-* | G-TODO/G-CAL/G-ADDIN |
| PRD-19.03 | Hybrid default; automatic/confirmation-first selectable | Specified with exact mode separation and owner-approved safety staging | PD-09, OL-REM-010/017; §7 | AUTO-ELIG-*, SETTINGS-MODE-*, AS-23 | OWN-03 accepted; G-AUTO/G-AUTO-FULL |
| PRD-19.04 | Direct due edit reminder-only; complete starts evidence flow; delete does not close; description edit reminder-only | Specified with field ownership/conflicts | OL-REM-006/007/011–013 | REM-CONFLICT-*, REM-DIRECT-* | G-TODO/G-CAL |
| PRD-20.01 | Add-in main/review/closure/evidence/group/settings/model/test/summary/manual views | Specified; bridge gated | OL-UX-001 | ADDIN-FLOW-* | G-ADDIN |
| PRD-20.02 | Group/sort by deadline/time/person/client/conversation/type/confidence/status/overdue/review | Specified including grounded client association | OL-UX-004; §5.7 | UX-GROUP-* | — |
| PRD-20.03 | Manual loop creation persists under the privacy architecture | Specified as email-grounded or user-authored Microsoft artifact | §5.8; OL-UX-009/010 | E2E-MANUAL-ORIGIN-*; AS-19 | OWN-07; G-TODO/G-CAL as applicable |
| PRD-21.01 | Loop card reconstructs all listed fields and suggested action | Specified | OL-UX-002 | UX-CARD-* | — |
| PRD-21.02 | No performance score/profile; only same-loop deadline changes | Specified | OL-REWARD-002, OL-METRIC-002 | PRIV-NO-PROFILE-* | — |
| PRD-22.01 | Summary add-in/email/both choice and listed categories | Specified; self-email gated | OL-SUM-001–004; §8.8 | SUM-ADDIN-*, SELFMAIL-* | OWN-10; G-SELFMAIL |
| PRD-22.02 | Configurable positive feedback/gamification optional | Specified | OL-REWARD-001/002 | UX-REWARD-* | — |
| PRD-23.01 | Open-source/self-hosted/no required central OpenLoops database | Owner-approved Windows local-companion interpretation | PD-01/10, §12 | ARCH-BOUNDARY-* | OWN-01/06 accepted; G-STATE/G-SEC-AUDIT |
| PRD-23.02 | Persist abstract evidence map fields, state/history/cursors/artifact refs | Specified with explicit sensitive-derived-metadata approval | §§5.1–5.6 | PERSIST-SCHEMA-*, PRIV-STATE-* | OWN-07 |
| PRD-23.03 | Avoid listed human-readable/raw/derived content in OpenLoops store | Specified as no human-readable text; encrypted derived metadata enumerated | PD-11, §5.6 | PRIV-CANARY-* | OWN-07 |
| PRD-23.04 | Ephemeral reconstruction/release/no content logging | Specified with application-control limits | §§5.2, 8.5; OL-NFR-005 | PRIV-LIFECYCLE-* | — |
| PRD-23.05 | State inside Microsoft where technically feasible/cross-device/deletion/narrow access | Owner-approved encrypted-local baseline; Microsoft-hosted adapter remains a later feasibility path | PD-10; G-STATE | STATE-HOSTED-SPIKE-* | OWN-06 accepted with security condition; G-STATE/G-PRIV/G-SEC-AUDIT |
| PRD-23.06 | Human-readable Microsoft artifacts allowed; app stores linkage only | Specified | OL-REM-004 | REM-PERSIST-BOUNDARY-* | — |
| PRD-24.01 | BYO provider/model/endpoint/local endpoint and sensitivities/thresholds | Specified | PD-12; OL-MODEL-*; §7 | MODEL-CONFIG-* | OWN-08; G-MODEL |
| PRD-24.02 | No prompt/body/model-response logging; disclose provider privacy | Specified | OL-MODEL-007/011/013; OL-NFR-005 | PRIV-PROVIDER-* | — |
| PRD-25.01 | All listed scan/reminder/category/confidence/identity/time/reminder/summary/reward/deadline/exclusion/model settings | Specified in typed table | §7 | SETTINGS-* | OWN-03/08/10 where noted |
| PRD-25.02 | All loop types visible by default | Specified | OL-UX-005; §7 | UX-FILTER-DEFAULT-* | — |
| PRD-26.01 | Optional bounded Test OpenLoops sample with listed results/corrections/calibration | Specified session-only | OL-TEST-*, §8.7 | TESTMODE-* | — |
| PRD-26.02 | Labels not required for routine onboarding | Specified | §8.6/8.7 | UX-ONBOARD-NOLABEL-* | — |
| PRD-27.01 | Primary outcome fewer forgotten commitments | Product thesis; not directly provable | §1, OL-METRIC-003 | Qualitative preview only | — |
| PRD-27.02 | Listed product indicators without performance score | Specified locally with retention limits | OL-METRIC-* | METRIC-EVENT-* | OWN-07 |
| PRD-28.01 | All listed MVP features | Specified or explicitly gated in rows above | §12; gates | Capability matrix | OWN decisions |
| PRD-29.01 | Listed explicit exclusions | Specified | §12 | CAPABILITY-NEGATIVE-* | — |
| PRD-30.01 | Future possibilities do not weaken inspectable-evidence principle | Specified architecture invariant | OL-MODEL-001/003; §11 DoD | DOMAIN-EVIDENCE-INVARIANT-* | — |
| PRD-31.01 | Representative stories | Specified as acceptance scenarios | product spec §13 | E2E-SCENARIO-* | — |
| PRD-33.01 | Resolve all listed engineering questions | Named gates/ADRs/workstreams | product spec §10; plan §§5–10 | Gate reports | OWN decisions where material |
| PRD-34.01 | Core product statement | Specified | product spec §1 | Product review | — |

## Acceptance criteria

| AC | Exact acceptance outcome | Disposition/specification | Executable test ID | Gate/approval |
|---:|---|---|---|---|
| 01 | Analyze newly read incoming message | Eligibility semantics specified by OL-SYNC-003/010 | E2E-AC-01-INCOMING | OWN-09; G-MAIL |
| 02 | Analyze newly sent outgoing message | Saved-sent-copy semantics by OL-SYNC-004 | E2E-AC-02-SENT | OWN-09; G-MAIL |
| 03 | Detect at least one request or promise | OL-DET-001 | E2E-AC-03-DETECT | G-MODEL |
| 04 | Detect multiple loops in one message | OL-DET-003/006 | E2E-AC-04-MULTI | G-MODEL |
| 05a | Ground each detected email/invitation loop in specific message/invitation evidence | §5.2, OL-DET-005, OL-MODEL-003/004 | E2E-AC-05A-DETECTED-EVIDENCE | G-MAIL/G-MODEL; G-CAL for invitation branch |
| 05b | Ground an email-created manual loop in user-selected valid message/component evidence | §§5.2/5.8, OL-UX-009 | E2E-AC-05B-MANUAL-EMAIL, AS-19 | G-MAIL |
| 05c | Ground an artifact-backed manual loop in a valid authoritative `UserAuthoredArtifactRef` | §5.8, OL-UX-009/010, OL-REM-006/011 | E2E-AC-05C-MANUAL-ARTIFACT, AS-19 | OWN-07; G-PRIV and G-TODO/G-CAL as selected |
| 06 | Reconstruct task without stored summary | §§5.2/8.5 | E2E-AC-06-RECONSTRUCT | G-PRIV |
| 07 | Identify waiting person through references | OL-DET-005 | E2E-AC-07-WAITING | G-MODEL |
| 08 | Extract or infer deadline | OL-DUE-* | E2E-AC-08-DEADLINE | G-MODEL |
| 09 | Prompt when no deadline | OL-DUE-007, §8.8 | E2E-AC-09-NODUE | — |
| 10 | Create To Do or calendar artifact | OL-REM-001–005/008/014 | E2E-AC-10-REMINDER | OWN-04; G-TODO/G-CAL |
| 11 | Show active loops in add-in | OL-UX-001/002 | E2E-AC-11-ADDIN | G-ADDIN |
| 12 | Link to supporting correspondence | OL-REM-005, OL-UX-002 | E2E-AC-12-NAV | G-MAIL/G-ADDIN |
| 13 | Later relevant message updates operative deadline | OL-DUE-005 and decision table | E2E-AC-13-RENEGOTIATE | — |
| 14 | Detect possible closure evidence | OL-CLOSE-001/002 | E2E-AC-14-CLOSE-CANDIDATE | G-MODEL |
| 15 | Require confirmation before inferred closure | OL-CLOSE-003 and state invariant | E2E-AC-15-NOAUTOCLOSE | — |
| 16 | Select alternative evidence | command matrix | E2E-AC-16-ALT-EVIDENCE | — |
| 17 | Completed outside email | OL-CLOSE-006 | E2E-AC-17-OUTSIDE | — |
| 18 | Avoid quote duplicates | OL-DEDUP-* | E2E-AC-18-QUOTE | G-MODEL |
| 19 | User-defined names/aliases | OL-ID-*; §7 | E2E-AC-19-IDENTITY | — |
| 20 | Daily summary | OL-SUM-* | E2E-AC-20-SUMMARY | G-SELFMAIL only for email |
| 21 | Optional closure celebration | OL-REWARD-* | E2E-AC-21-REWARD | — |
| 22 | User-supplied model key | OL-MODEL-* | E2E-AC-22-PROVIDER | OWN-08; G-MODEL/G-PRIV |
| 23 | Evidence map in Microsoft environment where feasible | Owner-approved encrypted-local baseline; later Microsoft-hosted feasibility adapter | E2E-AC-23-STATE | OWN-06 accepted with security condition; G-STATE/G-PRIV/G-SEC-AUDIT |
| 24 | Avoid copied bodies/generated summaries in app store | PD-11, §5.6 | E2E-AC-24-NOTEXT | OWN-07; G-PRIV |
| 25 | Optional bounded mailbox test | OL-TEST-*, §8.7 | E2E-AC-25-TESTMODE | — |

## Approval record

OWN-00 through OWN-10 were accepted by the product owner on 2026-07-19 and are reflected in the product specification, implementation plan, and this matrix. Phase 0 records them in ADRs without reopening them. Any later change requires a dated owner disposition and a synchronized revision of all three documents. Named technical, privacy, model, mutation, and security gates remain mandatory even though the product choices are settled.
