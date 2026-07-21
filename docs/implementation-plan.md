# OpenLoops MVP implementation plan

**Status:** Approved product direction; Phase 0 ADRs and named technical/security gates block dependent features
**Companion specification:** [OpenLoops MVP product specification](product-spec.md)
**Planning principle:** validate Graph and privacy contracts before building product behavior on top of them

## 1. Delivery objective

Deliver a Windows-first, open-source, per-user OpenLoops MVP consisting of:

1. an unelevated desktop companion that owns authentication, synchronization, transient analysis, encrypted minimized state, and reminder reconciliation;
2. an Outlook add-in that provides review, evidence navigation, settings, test mode, daily summary, and optional nonblocking compose hints;
3. delegated Microsoft Graph adapters for Outlook Mail, Microsoft To Do, and—only after its gate passes—Outlook calendar;
4. a provider-neutral constrained model adapter with local-endpoint and explicitly enabled external-provider modes; and
5. a synthetic evaluation and disposable-tenant contract-test harness that proves capability, privacy, idempotency, and degraded behavior.

Development and limited preview are confirmation-first. The full MVP ships hybrid-by-default only after G-AUTO passes. No inferred closure is automatic, no email is sent to another person, and no human-readable source or generated mailbox text is intentionally persisted in OpenLoops state. The product owner approved the encrypted derived classifications, fingerprints, temporal values, source references, and transitions enumerated in the product spec; ADR-PRIV-001 must still make the approved boundary implementable and auditable.

OWN-00 through OWN-10 were accepted on 2026-07-19. A failed mandatory-feature gate returns to the product owner for an explicit scope decision; it does not silently redefine the MVP. Broad public release also requires G-SEC-AUDIT because the local connector holds credential-equivalent and sensitive derived state.

## 2. Architecture

```text
                         Microsoft 365
                ┌──────── Mail / To Do / Calendar ────────┐
                │                                          │
                │ delegated Graph, exact scopes/fields     │
                v                                          │
┌────────────────────────────────────────────────────────────────────┐
│ Per-user Windows desktop companion                                 │
│                                                                    │
│ Identity ─ Graph adapters ─ Ingestion ledger ─ Job scheduler       │
│                               │                                    │
│                               v                                    │
│ Canonicalize ─ Extract ─ Validate ─ Associate ─ Policy/state       │
│                               │                   │                │
│                    bounded model adapter        Reminder adapters │
│                               │                   │                │
│                     local or opted-in provider    └─────── Graph ──┘
│                                                                    │
│ OS secret store + encrypted opaque state + content-free health     │
└──────────────────────────────┬─────────────────────────────────────┘
                               │ authenticated local bridge (G-ADDIN)
                               v
                    ┌──────────────────────────┐
                    │ Outlook add-in           │
                    │ review/evidence/settings│
                    │ no content persistence  │
                    └──────────────────────────┘
```

### 2.1 Technology baseline

Pin exact supported versions at implementation kickoff and record them in the dependency lock and ADRs.

| Area | Baseline | Reason |
|---|---|---|
| Desktop companion | Pure Rust on a repository-pinned stable toolchain | Product-owner choice; memory safety, small native distribution, typed/concurrency boundaries, and future cross-platform core seams. Windows integration remains explicitly tested. |
| Async/HTTP/serialization | Vetted, pinned Rust crates for async scheduling, HTTPS, OAuth/PKCE primitives, JSON/schema contracts, cancellation, and bounded streaming | Exact crates/versions are selected in ADR-002/Phase 0 after maintenance, license, security, and Windows review; no ad hoc OAuth or crypto implementation. |
| Identity | Rust system-browser authorization code with PKCE, ephemeral loopback listener, and OS-protected refresh/token state | Standards-backed public-client path. WAM is deferred unless a later Rust-compatible adapter passes G-ID. |
| Add-in | TypeScript, Office.js, and a minimal component UI | Supported Outlook surface; strict typing and schema generation. Avoid offline/service-worker behavior. |
| Contracts | Language-neutral JSON Schema plus generated Rust/TypeScript types | One validation source for model, IPC, and synthetic fixtures. |
| Local state | SQLite metadata/ledger with a versioned application-level authenticated-encryption envelope; random data/HMAC keys protected by Windows DPAPI/current-user scope | Transactional checkpoints/idempotency while keeping sensitive derived fields encrypted. ADR-005 must specify nonce/AAD/rollback/migration/rotation behavior before implementation. |
| Windows integration | Vetted Rust Windows bindings for DPAPI/Credential Manager, ACLs, single-instance/process lifecycle, notifications, signing/installer hooks, and a minimal tray/settings/recovery UI | Required for sign-in, secure pairing, stale status, and add-in-unavailable recovery. It is not a second task manager. |
| Tests | Unit/property/integration tests plus a disposable-tenant contract-test executable | Keeps deterministic behavior reproducible and Graph observations sanitized. |

Do not introduce a remote or hosted OpenLoops web server, container, central database, message broker, application permission, or cloud relay into the MVP. A same-user loopback web listener is permitted only for the Office add-in bridge after G-ADDIN selects and threat-models it; it is not a remotely reachable service.

The approved target is a Windows-native Rust host. DPAPI/Credential Manager, OAuth loopback and protected tokens, code signing, installation/update verification, per-user background execution, and the add-in bridge are the highest-risk platform-specific areas. macOS/Linux remain future interface implementations and cannot use plaintext secret/state fallbacks.

### 2.2 Interface boundaries

Define these interfaces before concrete adapters:

```text
IdentityProvider       acquire/refresh/disconnect delegated sessions
SecureStore            store/delete small secrets; fail closed when unavailable
GraphTransport         authenticated request, throttling classification, sanitized errors
MailSource             initial window, folder delta, bounded message fetch, link resolution
ReminderAdapter        propose/create/reconcile/complete after policy authorization
StateStore             encrypted transaction, cursor, ledger, loop/source operations
JobScheduler           poll/reconcile/retry/quarantine scheduling
ContentCanonicalizer   safe HTML/MIME to transient blocks plus offset map
Extractor              zero-to-many evidence-grounded candidate claims
AssociationEngine      bounded retrieval/ranking for existing-loop relations
PolicyEngine           deterministic state transitions and mutation eligibility
ModelProvider          bounded structured inference; no tools
UiBridge               authenticated session-scoped commands and projections
Clock                  testable UTC/user-timezone behavior
```

Graph DTOs, provider DTOs, domain records, and UI projections must be separate types. No Graph/model response object may be written directly to state or returned directly to the add-in.

### 2.3 Planned repository layout

```text
Cargo.toml                      Rust workspace
rust-toolchain.toml             pinned stable toolchain
crates/
  openloops-domain/             value objects, state machine, policy
  openloops-application/        use cases, jobs, ports/traits
  openloops-graph/              identity/mail/To Do/calendar adapters
  openloops-inference/          canonicalization, rules, provider adapters
  openloops-persistence/        encrypted store, migrations, secure store
  openloops-desktop/            lifecycle, tray/recovery UI, scheduler, bridge
outlook-addin/                  TypeScript/Office.js review/settings/test/compose UI
contracts/                      JSON Schemas and Rust/TypeScript generation
tests/
  unit/
  property/
  integration/
  graph-contracts/              disposable-tenant, synthetic-only
  privacy/
  adversarial/
fixtures/synthetic/             invented messages/labels only
docs/adr/                       accepted architecture decisions
tools/                          guardrails, packaging, canary and artifact checks
```

The implementation may adjust project names after ADR approval, but must preserve the dependency direction: UI/adapters depend inward on application/domain contracts; the domain does not depend on Graph, Office.js, persistence, or a model SDK.

## 3. Persistent data and invariants

### 3.1 Store design

Use a per-account encrypted database under the current OS user only after ADR-005 and ADR-PRIV-001 pass. Protect random data-encryption and HMAC keys with DPAPI current-user scope. The proposed record envelope uses a versioned approved AEAD, a fresh 96-bit CSPRNG nonce per encryption (never a rollbackable counter), `key_id`, and authenticated additional data binding at least account reference, record type, record ID, schema version, and ciphertext version. Detect nonce collisions defensively and fail closed on RNG failure. Never use account IDs, message text, provider keys, counters, or timestamps as encryption keys/nonces.

Key rotation creates a new random key ID and transactionally copy/verifies records before swap; the old key remains until every record and backup/recovery decision is verified. A restored full Windows profile may legitimately decrypt an older database, so rollback suspicion enters recovery and remote reconciliation before mutations rather than claiming cryptographic prevention. Older binaries refuse write access to newer schema/ciphertext versions.

Minimum tables/collections:

| Record | Contents | Prohibited contents |
|---|---|---|
| `account_binding` | random account ref, cloud enum, enabled feature/scope codes, encrypted Rust OAuth token-state reference | address, display name, tenant ID in diagnostics |
| `sync_checkpoint` | encrypted folder/list ref, opaque delta/next URL, query fingerprint, last complete time, status code | raw request/response, content-bearing URL logs |
| `message_observation` | HMAC source/version key, abstract direction/read/first-observation flags, policy version, job outcome; encrypted re-fetch locator for quarantined work | subject, participants, body, unencrypted raw ID |
| `source_ref` | tagged union of encrypted email evidence locator/component/range/digests; invitation mail/event locator/version; or user-authored Microsoft artifact locator/source-field flags/digests | excerpt, title/body/due text, filename, URL, participant value |
| `loop` | facets, confidence bucket, encrypted temporal value/range, version labels | task/person/deadline phrase, summary, rationale |
| `transition` | old/new facets, reason code, `LoopSourceRef` IDs and/or explicit user-action ID, actor, time, policy version | copied explanation |
| `reminder_link` | encrypted artifact/list refs, adapter type, operation/reconciliation codes | title/body |
| `operation_ledger` | HMAC operation key, pending/succeeded/ambiguous state, encrypted destination ref, retry time | Graph payload or full error |
| `job_health` | allowlisted error class, attempt count, retry/stale time, quarantine generation/ref | raw URL, header, ID, response body |
| `settings` | encrypted user-provided aliases/rules/preferences; provider label/exact consented origin | provider key, mailbox-derived human-readable labels/text |

These tables intentionally contain the sensitive derived metadata enumerated in product-spec section 5.6. The implementation and privacy notice MUST describe them accurately; “no derived content” is prohibited wording.

### 3.2 Hard invariants

Automated tests and code assertions enforce:

1. No human-readable source or generated mailbox text crosses the state-store interface; only OWN-07-approved derived fields may cross it.
2. No provider/model DTO crosses the state-store or UI boundary without validation and projection.
3. Every detected email/invitation loop fact and system transition references validated evidence or an explicit user action. A manual loop references its authoritative user-authored Microsoft artifact plus explicit user actions; no ungrounded local free text is allowed.
4. An inferred closure never produces `resolution=closed` without a user-confirmation command.
5. A reminder completion/deletion never closes a loop.
6. A delta cursor advances only after all page items reach a durable terminal job outcome.
7. An external mutation has a durable pending operation key before the request.
8. An ambiguous write is reconciled before retry.
9. Invalid model output produces zero mutations.
10. Missing evidence is not replaced by stored content or treated as negative proof.
11. Secure-store failure never falls back to plaintext.
12. Every log/diagnostic event is an allowlisted code plus safe scalar measurements.
13. A quarantined item retains an encrypted re-fetchable locator and user/retry state before cursor advancement.
14. An ambiguous reminder create is never retried without a proven remote correlation marker; otherwise it requires user-assisted reconciliation.
15. A suspected database/profile rollback stops mutations until mail/reminder reconciliation completes.
16. Outlook add-in, calendar, self-email, personal-account, hybrid/automatic-reminder, and Microsoft-hosted-state capabilities remain disabled until their respective gates and OWN dispositions pass; G-AUTO enables hybrid and G-AUTO-FULL separately enables fully automatic mode.

## 4. Processing design

### 4.1 Canonical message input

Build a bounded, in-memory canonical representation:

```text
CanonicalMessage
  opaque_input_handle
  direction, sent/received time, timezone context
  participant slots (sender, to[n], cc[n]) with transient values
  subject block
  current-body blocks
  signature/disclaimer blocks
  quoted/forwarded blocks with detected boundaries
  attachment-name and link-label blocks (no content fetch)
  offset map to source/rendered components
  transient Graph locator/version metadata
```

Sanitization removes active HTML, scripts, styles, trackers, remote resources, and dangerous schemes. Preserve text needed as evidence, including hostile-looking instructions, but treat it only as data.

### 4.2 Extraction contract

The model receives opaque input handles and bounded blocks. It returns strict JSON:

```json
{
  "schema_version": "1.0",
  "analysis_status": "ok",
  "abstention_reasons": [],
  "proposed_loops": [
    {
      "proposal_id": "p-1",
      "kind": "request",
      "atomic_action": {
        "predicate": {"message": "input-0", "block": 3, "start": 4, "end": 10},
        "objects": []
      },
      "actor": {"type": "authenticated_user", "evidence": []},
      "waiting_parties": [
        {"role": "requester", "participant_ref": "sender", "evidence": []}
      ],
      "deadline_hypotheses": [],
      "confidence": {"bucket": "high", "reason_codes": []},
      "ambiguity_codes": [],
      "evidence": []
    }
  ],
  "relation_hypotheses": []
}
```

The final schema must use enums, `additionalProperties: false`, bounded collection lengths, bounded strings limited to opaque input handles/enum codes, and evidence spans for every substantive claim. Existing loops appear only as opaque candidate handles supplied by the application. The model cannot emit Graph IDs, URLs, arbitrary participants, mutation plans, final UI copy, or free-form rationale.

Validation order:

1. response size and JSON parse;
2. exact schema/version and unknown-field rejection;
3. supplied-handle membership;
4. block/range bounds and Unicode boundary validity;
5. evidence text correspondence in the transient canonical input;
6. participant-position membership;
7. deterministic date reparse and timezone resolution;
8. internal claim/relation consistency;
9. policy confidence/ambiguity routing.

Any failure returns a typed `analysis_unavailable` or `needs_review` result with zero external mutation.

Provider transport policy:

- local mode accepts only explicit loopback endpoints and never silently broadens to LAN/private addresses;
- external mode accepts only the exact consented HTTPS origin with valid TLS;
- redirects, cross-origin authorization forwarding, ambient proxy inheritance, link-local/cloud-metadata targets, and unexpected private-network resolution are blocked;
- resolve/revalidate addresses against policy, cap request/response bytes and wall time, and cancel streaming over limits;
- sanitize provider SDK exceptions at the adapter boundary before they enter application error types;
- endpoint/origin/model changes invalidate consent and automation calibration; and
- schema validity does not imply semantic safety. Deterministic positive constraints and adversarial scoring remain required for any automatic eligibility.

Built-in provider profiles:

| Profile | Default connection | Credential and consent boundary | Release proof |
|---|---|---|---|
| `ollama_local` | User-approved loopback Ollama API only; the documented local API default is `http://localhost:11434/api` | No Ollama API key is expected for the local API. Mailbox content remains transient but still crosses into the separately running local Ollama process, which the disclosure names explicitly. | Schema/timeout/size/cancellation tests, loopback-only enforcement, unavailable-model fallback, and no implicit LAN/proxy route. |
| `ollama_cloud` | Exact pinned origin `https://ollama.com/api` | User API key in the OS secret store; explicit disclosure and consent for categories sent to Ollama Cloud. Endpoint/model edits invalidate consent and automation calibration. | Live BYO-key contract test outside CI plus sanitized synthetic fixtures; redirects and cross-origin key forwarding are rejected. |
| `approved_https` | User-entered exact HTTPS origin | Separate explicit consent and OS-protected credential; no assumption that OpenAI-compatible shape proves privacy or semantics. | Optional capability, disabled until the adapter passes the same transport, schema, disclosure, and privacy tests. |

These profiles follow Ollama's official local/cloud API and authentication documentation linked in the product specification. Provider privacy and retention are the provider's responsibility under the user's chosen account/configuration; OpenLoops exposes that fact rather than implying that BYO credentials make transmission local.

### 4.3 Deterministic/model responsibility split

| Deterministic code | Model hypothesis only |
|---|---|
| Direction, participants, timestamps, MIME/HTML sanitization, quote/signature blocks, attachments/links metadata | Soft/implied request or promise semantics |
| Configured identity/address matching | Contextual identity/role mapping |
| Explicit date parsing, timezone/EOD rules, deadline arithmetic | Ambiguous/event-relative/soft deadline interpretation |
| Source idempotency and existing-loop retrieval | Atomic clause interpretation where grammar is ambiguous |
| Evidence span/schema validation | Candidate closure/delegation/renegotiation relation among supplied loops |
| State transitions and mutation policy | Confidence hypothesis and ambiguity codes |
| Graph reads/writes and retry policy | Never permitted |

### 4.4 Ingestion transaction

For every delta page:

1. Stage the page cursor and item observation keys in an encrypted transaction.
2. For each changed item, deduplicate by `HMAC(accountRef, testedMessageLocator, observedVersion)`.
3. Determine eligibility from a read transition, post-baseline first observation already read, explicit history/backfill batch, or newly observed saved sent copy. Never treat saved-copy observation as delivery proof.
4. Fetch only required content fields and process in a bounded job.
5. Persist validated evidence/loop transitions or a defined safe outcome (`no_candidate`, `needs_review`, `deferred`). A retry-exhausted item becomes `quarantined`, retaining encrypted locator/version/failure/retry state; it is not discarded as `failed_bounded`.
6. Commit page work and cursor atomically only after every item is terminal.
7. Retry transient failures independently; move exhausted items to a recoverable visible quarantine lane after the configured cap.

Cursor expiry runs a visible bounded rescan over the configured history/folder scope. Initial/backfill already-read messages enter a review-only history batch with no automatic reminders. The same observation keys and transition idempotency prevent duplicate loops/reminders. Document that polling selected folders cannot recover an item that enters, is read, and leaves monitored scope between observations.

### 4.5 Reminder operation protocol

```text
Policy authorizes proposal/create/update/complete
    ↓
Persist operation_key + Pending before Graph request
    ↓
Call adapter with correlation headers only in memory
    ↓
Success → persist encrypted destination ref + Succeeded
Timeout/unknown → Ambiguous → query a contract-tested remote marker → reconcile
No reliable marker → user-assisted reconciliation; do not retry create
Definitive failure → typed retry or visible failure lane
```

The local operation key alone does not make a Graph POST idempotent. Each adapter must prove a durable opaque remote correlation marker supported by the endpoint (for example, only a contract-tested linked resource/extension or other field). Test identical tasks, delayed visibility, movement, marker removal, and deletion. If Graph cannot support reliable correlation, an ambiguous create remains blocked for user selection and automatic/hybrid creation fails its gate.

Updates use per-field ownership and compare/reconcile conflict rules from OL-REM-011–013. Until endpoint conditional writes are contract-tested, a stale read never authorizes an overwrite. Calendar reminder events are self-only with zero attendees/meeting/response behavior and no invented completion operation.

## 5. Outlook add-in and local bridge spike

G-ADDIN is on the critical path. Prototype before full UI work and record an ADR.

Evaluate at least:

1. packaged add-in asset hosting/distribution model, Office manifest permissions/requirement sets, supported-client matrix, and lifecycle when the task pane is closed;
2. compose/send event availability and whether it can provide an informational hint without changing authoritative sent-item processing;
3. evidence deep-link behavior across supported Outlook clients;
4. loopback HTTPS feasibility from the add-in WebView, manifest/network policy, CORS, CSP, firewall, IPv4/IPv6, and port collision behavior; certificate tests must prohibit machine-wide trust and a general-purpose trusted root with retained signing key, and must prove current-user-only trust/name scope, non-exportable private-key ACL, issuance/rotation/expiry/replacement, and complete disconnect/uninstall cleanup;
5. exact companion discovery plus a first-pair bootstrap protocol bound to expected packaged companion, Office add-in origin/client, OS user, account reference, nonce, expiry, and user-verifiable action, without secrets in URL parameters, Office roaming settings, localStorage, or repository configuration;
6. least-authority command-scoped in-memory sessions, rotation/revocation on reconnect/account change, first-pair race/replay/brute-force defenses, and the explicit limitation against same-user malware;
7. exact Host/Origin validation, CSRF defense, loopback-only binding, rate/size limits, session expiry, IPv4/IPv6/port/DNS-rebinding behavior, account switching, and reconnect behavior;
8. safe opaque non-secret review-link activation from To Do/Outlook clients, with no bearer/session token in the link and graceful companion-stopped/multi-user/tamper behavior; and
9. fallback native status/review/recovery behavior when the add-in or bridge is unavailable.

Prefer native named pipes for companion-internal/native UI traffic. Use a loopback web bridge for the Office add-in only if the runnable packaged spike proves asset hosting, certificate/network, discovery, and bootstrap boundaries on every advertised client. Do not assume a named pipe is reachable from Office.js or that Graph delegated scopes authorize add-in operations. If G-ADDIN fails, the PRD-required Outlook add-in is blocked and the product owner must change OWN-01; native UI is not a silent substitute.

## 6. Workstreams and milestones

Each phase ends with executable evidence. A failed gate narrows the advertised capability or blocks dependent work; it is not papered over with an assumption.

### Phase 0 — ADRs, threat model, and Rust skeleton

**Deliverables**

- Record the already accepted OWN-00 through OWN-10 consequences in the research decision record and create the ADRs below; do not reopen product decisions implicitly through implementation choices.
- Complete ADR-PRIV-001: enumerate every persisted derived field/fingerprint/classification and every `LoopSourceRef` variant, explicitly including manual artifact identifiers, keyed field digests, source-field ownership/version flags, and manual-origin source-of-truth exception; record purpose, necessity, disclosure, retention/deletion/reset/export, backup/physical-erasure limits, and threat-model change.
- Pin supported Windows, Outlook client, Microsoft account/cloud, and model-provider matrices. `P0-MATRIX-001`, accepted 2026-07-19, pins the disabled validation targets in `contracts/support/support-matrix.json`; it is not an advertised support claim and every named gate remains mandatory.
- Create the pinned Rust workspace/toolchain, TypeScript add-in project, dependency-direction tests, Rust/TypeScript contract generation, formatting/lint/test runners, and privacy-safe build configuration.
- Commit the dependency lockfile and a placeholder-only source-build bootstrap that verifies pinned prerequisites and runs a synthetic smoke test without Graph/model credentials.
- P0-WI-04 implements these two skeleton deliverables only. Its smoke path is inert, accepts no configuration or secret, and advances no capability, acceptance criterion, or named gate.
- P0-WI-05 accepts ADR-002 and its closed authentication decision contract only.
  It records the pure-Rust system-browser PKCE, redirect, transaction, account,
  token, dependency, disconnect, and threat boundaries without implementing
  OAuth, activating dependencies, requesting a scope, contacting Microsoft,
  persisting a token, or advancing G-ID. ADR-003 feature scopes and ADR-005
  protected token state remain separate gate-bounded work items.
- P0-WI-06 accepts ADR-003 and its closed, disabled feature-to-permission and
  configured-processing contract only. It requests no permission, contacts no
  service, activates no dependency, and leaves every Graph/add-in capability and
  gate disabled or unrun.
- P0-WI-07 accepts ADR-004 and its bounded synchronization decision contract
  only. It implements no Graph transport, scheduler, durable checkpoint, query,
  dependency, or tenant interaction. Exact fields, identifiers, budgets, cursor
  errors, and service behavior remain behind ADR-005/006 and G-MAIL.
- P0-WI-08 closed on 2026-07-20 and accepts ADR-005 and its bounded persistence-and-cryptography decision
  contract only. It pins the closed envelope, keys, DPAPI scope, SQLite
  transaction, rollback/recovery, migration/rotation, deletion, and dependency
  boundaries without creating a database, key, token cache, backup, diagnostic,
  or cryptographic runtime; G-STATE, G-PRIV, and G-SEC-AUDIT remain unrun.
- P0-WI-09 accepts ADR-006 and its disabled evidence identity/anchoring contract
  only. It pins exact claim-bound immutable/fallback locators, copy/move/archive/send
  behavior, versioned Unicode/HTML canonical bytes and keyed anchors, complete-only
  fallback classification, user-confirmed reanchoring, exact-origin/asynchronous
  Office/deep-link boundaries, and unavailable behavior without a Graph/Office
  call, permission, persistent record, navigation path, support claim, or gate.
- P0-WI-09R closed on 2026-07-20 as the product-owner-approved repair
  continuation for three reproduced final-gate defects only: it closes the
  inactive runtime file/import/dependency/script inventory, disables implicit
  formatter configuration discovery, pins the exact DOM entry/exit, table-cell,
  and quote-vector byte algorithm, and requires bounded inline-safe attachment
  metadata enumeration. It enables no runtime, permission, capability, support
  row, acceptance criterion, or gate. The exact build, 694-case Phase 0 negative
  regression, prospective 78-blob public scan, and three cold-start closure
  judgments passed; no transcript or model output was retained.
- P0-WI-10 closed on 2026-07-20 and accepts ADR-007 and its disabled
  model-provider boundary contract only. It pins the built-in Ollama local/cloud
  and optional approved-HTTPS profiles, exact network and consent policy,
  bounded transient input, strict application-side output/evidence validation,
  semantic-adversarial routing, source-sanitized errors, closed sensitivity and
  review-threshold disclosures, and no-tools/no-transcript rules without
  contacting a provider, accepting a credential, enabling model analysis,
  completing AS-12/16/22, advertising support, or passing
  G-MODEL/G-PRIV/G-SEC-AUDIT/G-RELEASE. The exact source build, 790-case Phase 0
  negative regression, prospective 84-blob public scan, and three cold-start
  closure judgments passed; no reviewer prompt, transcript, or model output was
  retained.
- P0-WI-11 closed on 2026-07-21 and accepts ADR-008 and its disabled
  deterministic policy/state contract only. It closes the independent facet catalogs, full-state legality,
  establishment/promotion, deadline precision/aging, concurrent hypothesis
  projection, typed confirmation/correction commands, idempotency/stale/reopen,
  and reminder-separation rules without accepting a logical record, activating
  policy runtime, making a Graph/Office/model call, enabling a reminder or
  automation mode, completing an AC/scenario, or passing G-AUTO/G-AUTO-FULL.
  All 11 Phase 0 deterministic checkers, 860 synthetic/adversarial rejection
  cases, the metadata-only package allowlist, a prospective 89-blob public scan,
  and three final fresh-context closure judgments passed. The unchanged product
  source remains covered by the P0-WI-10 exact source-build pass; the current
  add-in tests, formatting, and lint also passed. No reviewer prompt,
  transcript, or model output was retained.
- P0-WI-12 is active and is bounded to accepting ADR-009's disabled reminder-
  adapter decision contract. It defines exact privacy-mapped record shapes,
  durable-before-request operation identity, ambiguous-write reconciliation,
  per-field ownership and conflict rules, manual-source atomicity, Calendar and
  review-link safety boundaries, and explicit deferred gate ownership. It does
  not implement an adapter, persist a record, request a permission, make a
  Graph/Office call, enable an automation mode, complete an AC/scenario, pass a
  gate, or advertise support. The 2026-07-21 checkpoint remains open because the
  retry contract must distinguish a duplicate external invocation (zero direct
  requests) from a serialized, durably recorded eligible retry (one request per
  recorded attempt after revalidation, maximum three). Until that bounded repair
  and its mutation pass, the fresh-checker exit condition is not satisfied.
- Write data-flow/threat models for tokens, Graph content, model transmission, add-in bridge, local state, artifacts, diagnostics, update, and disconnect.
- Define feature-to-scope manifest and capability flags.
- Install repository guardrails on the working clone.

**Exit gate**

- Architecture/security review translates the approved scope/privacy boundary into complete ADRs and executable gates.
- Build/test skeleton passes without secrets or generated artifacts tracked.
- Every unresolved capability has a named gate, owner role, and fallback.

### Phase 1 — Disposable-tenant Graph and add-in contract harness

**Deliverables**

- Synthetic-only runner that records sanitized pass/fail plus API/service/library versions.
- Identity matrix: pure-Rust system-browser PKCE, redirect/path/port/IPv4/IPv6, OAuth state replay/duplication/expiry, exact scopes, refresh/token rotation, consent/admin, revocation, concurrent instance, protected cache/disconnect, work/school core, and gated personal accounts. WAM is excluded unless separately reintroduced.
- Mail matrix from [validation plan](../research/microsoft-graph/validation-plan.md): `Mail.ReadBasic` boundary, `Mail.Read`, token-scope versus folder-policy enforcement, Inbox/Sent delta, fields, coalesced arrive→read→move, first-observation/backfill, rules-routed/transient-folder mail, paging/replay/cursor expiry, exact Graph immutable-ID preference and fallback, Graph-to-Office link/ID conversion, moves/copies/archive/draft-send/delete, save-sent-disabled/immediate sent-copy move/delete/send-as, and initial filters.
- To Do matrix: `Tasks.Read` enumeration, `Tasks.ReadWrite` CRUD, built-in lists, direct edits/conflicts, exact durable remote correlation candidates, identical concurrent tasks, delayed visibility/movement/deletion, lost-response ambiguous POST, and conditional-write observations; no launch dependency on disputed To Do delta.
- New calendar/invitation research and tests for least delegated permission, invite-mail/event correlation, response states, CRUD/direct edits, identifiers/links/conflicts, supported account types, and proof that self-only generated reminders send no invitations/updates/cancellations.
- G-ADDIN prototype matrix described in section 5.
- Microsoft-hosted evidence-state storage spike: identify candidates and define feasibility before testing—delegated permissions no broader than approved, required opaque data size, cross-device latency, deterministic conflict handling, retention/deletion/export, tenant/personal availability, quota, offline/recovery, and privacy. Record a go/no-go owner/ADR decision; “superior” alone is not a criterion.
- Self-email spike: prove canonical authenticated-mailbox recipient for every account type, generated-message marker and recursion suppression, `Mail.Send` consent, operation idempotency, and lost-response behavior without accepting arbitrary recipients.

**Exit gate**

- G-ID, G-MAIL, G-TODO, G-ADDIN pass for the claimed core matrix.
- G-CAL either passes or calendar/invitation scope returns to the product owner; it is not automatically omitted.
- G-STATE proves the already approved encrypted-local baseline; the Microsoft-hosted/cross-device adapter spike records future feasibility but does not replace the baseline silently.
- No raw ID, tenant/account value, token, payload, or canary is written to test results or the repository.

### Phase 2 — Secure desktop foundation and persistence

**Deliverables**

- Pure-Rust system-browser PKCE identity adapter with incremental consent and reconnect/disconnect UX; no embedded browser or device-code fallback.
- Pinned, audited Rust OAuth/HTTP primitives plus per-user OS-protected token state, interprocess locking/single-instance enforcement, atomic replacement, token rotation/revocation inventory, and explicit app-owned cache semantics; provider keys are never accepted on command lines.
- ADR-005 encryption envelope, DPAPI-protected application keys, transactional encrypted store, migration/rotation journal, rollback detection/recovery, compatibility/downgrade refusal, and key inventory/lifecycle for data, HMAC, provider, and pairing secrets.
- Typed Graph transport with exact request field sets, timeout/cancellation, `Retry-After`, bounded backoff/jitter, circuit breaker, and sanitized errors.
- Rust scheduler, encrypted operation ledger, per-item retry/quarantine lane, checkpoint transactions, account isolation, and freshness status.
- Native tray/settings/recovery surface and authenticated internal IPC.
- Allowlisted diagnostics and canary scanner.

**Exit gate**

- Secure-store-unavailable, concurrent process/cache callback, cross-user, DB-only rollback, full-profile/VM restore, clock rollback, database/key mismatch, cache-loss/divergence, key rotation during jobs, account mismatch, forced RNG failure/nonce collision, ciphertext swap, crash/disk-full at every migration/rotation step, old/new binary alternation, tamper, disconnect, and uninstall tests pass.
- G-PRIV passes for all foundation paths.
- No plaintext fallback, content-bearing exceptions, or raw Graph URLs appear.

### Phase 3 — Mail ingestion and evidence substrate

**Deliverables**

- Configured initial/backfill review batch and per-folder delta jobs for Inbox/Sent Items, with explicit selected-folder coverage limits.
- Abstract unread/read/first-observation eligibility and saved-sent-copy logic.
- Safe canonicalization, quote/signature separation, participant slots, attachment/link-label metadata, and offset maps.
- `LoopSourceRef` creation, digesting, resolution, re-anchoring, source-unavailable behavior, and message/navigation adapters.
- Page/item transaction protocol, encrypted quarantine locator/retry lane, invalid-cursor bounded rescan, versioned replay/idempotency, and freshness/coverage UI projection.

**Exit gate**

- Duplicate page, reorder, empty page, crash-before/after each checkpoint, throttling, invalid cursor, moved/deleted message, and Unicode range property tests pass.
- No human-readable source/generated text is persisted after job completion; only OWN-07-approved encrypted derived metadata remains.
- Acceptance criteria AC-01, AC-02, the email/message branch of AC-05a, AC-05b, AC-06, AC-12, AC-18, AC-23, and AC-24 have substrate coverage; invitation evidence remains behind G-CAL.

### Phase 4 — Detection, deadlines, correlation, and lifecycle

**Deliverables**

- Versioned extraction/association JSON Schemas and runtime validators.
- Deterministic explicit-request/promise patterns, participant/alias rules, date/time parser, EOD/timezone engine, and deadline precision model.
- Built-in Rust `ollama_local` and `ollama_cloud` adapters plus the provider-neutral approved-HTTPS contract, with exact-origin/provider consent, SSRF/redirect/DNS/proxy/TLS defenses, bounded input/output/time, OS-stored keys, source-sanitized errors, and transcript-free logging.
- Atomic loop splitting, candidate-establishment/promotion table, independent state facets/primary precedence, waiting-party/client evidence, quoted-history dedup, existing-loop retrieval/ranking, complete operative-deadline/aging tables, calendar-invitation hypotheses behind G-CAL, closure/decline/delegation/mootness hypotheses, correction commands, and deterministic state policy.
- Session-only reconstructed explanations/cards and model-unavailable fallback.
- Public train/dev synthetic corpus plus an independently held, sealed release-judge corpus outside the repository and maker context; minimum per-stratum samples, family-leakage checks, confidence intervals/lower bounds, tuning-attempt limits, adversarial cases including schema-valid semantic manipulation, scoring, threshold report, and regression runner.

**Exit gate**

- G-MODEL and all detection thresholds in the product spec pass.
- Prompt-injection bypass, unsupported evidence acceptance, invalid-output mutation, and automatic closure are zero.
- AC-03, AC-04, AC-07, AC-08, AC-09, AC-13, AC-14, AC-15, AC-18, and AC-19 pass on synthetic scenarios.

### Phase 5 — Review UX, To Do, and manual lifecycle

**Deliverables**

- Add-in main view, grouping/filtering, card reconstruction, evidence links, freshness/degraded states, and empathetic copy templates.
- Review flows for simultaneous state facets, ambiguity, undated-confirmed, complete operative-deadline table, possible closure, keep-open/remap/alternative evidence, typed terminal commands, undo/reopen, manual completion, completed outside email, not mine, false detection, duplicate, delegated/shared/retained, moot, and other reason code.
- To Do creation/reconciliation supporting confirmation-first and gated hybrid policy, proven remote marker or user-assisted ambiguous-write flow, per-field ownership/conflicts, compare/reconcile updates, direct-completion evidence flow, deletion/missing handling, and recreate action. Development/preview configuration remains confirmation-first.
- Typed settings from product-spec section 7, grounded client association, correction-to-explicit-rule flow, local indicators, and first-run/disconnect/uninstall UX.
- Manual loop creation either grounded in user-selected message evidence or backed by a user-authored Microsoft artifact as the authoritative readable source; reject ungrounded locally persisted free text.

**Exit gate**

- G-TODO and G-ADDIN pass in integrated scenarios.
- Ambiguous To Do writes produce one artifact; completion/deletion never closes a loop.
- AC-05c passes for the To Do-backed manual-artifact branch.
- AC-07 through AC-12 and AC-14 through AC-19 pass end to end for To Do.

### Phase 6 — Calendar, summaries, reward, and test mode

**Deliverables**

- Calendar reminder and invitation-loop adapters behind G-CAL, using self-only/no-attendee/no-online-meeting behavior and concrete event edit/delete/cancel semantics rather than a fictitious completion state.
- In-add-in daily summary, abstract count-only OS notification option, local product indicators, and optional self-email path behind G-SELFMAIL with canonical self-recipient, generated-message marker/recursion suppression, incremental `Mail.Send`, lost-response operation handling, and no stored summary copy.
- Configurable, non-performance-scoring reward behavior.
- Session-only bounded test mode with simulated reminders, correction/settings application, and no sample/prediction persistence.
- Exact OL-REM-010/OL-REM-017 mode policy and separate feature flags: keep hybrid disabled until G-AUTO and automatic disabled until G-AUTO-FULL; make hybrid the full-MVP new-install default under OWN-03.

**Exit gate**

- G-CAL passes before calendar/invitation features are exposed; failure returns to OWN-04/05.
- AC-05a passes for invitation evidence and AC-05c passes for the calendar-backed manual-artifact branch.
- G-SELFMAIL passes before self-email is exposed; it cannot address another recipient, recurse into detection, or blindly retry an ambiguous send.
- Test mode produces no Graph writes and no persistent sample/labels/output.
- AC-10, AC-20, AC-21, AC-22, and AC-25 pass; Ollama local/cloud provider scenarios pass; G-AUTO and G-AUTO-FULL suites are ready for sealed limited-preview evaluation.

### Phase 7 — Hardening, packaging, and documentation

**Deliverables**

- Signed per-user installer plus signed update metadata independent of package signing: trusted-key rotation/revocation, monotonic anti-downgrade policy, channel binding, expiry, package hash/size, atomic protected staging/replacement, recovery, and provenance-to-source-revision verification; no elevation unless separately justified.
- Dependency locks, license review, vulnerability policy, SBOM, provenance, package/release allowlists, and rollback/recovery design.
- Fresh-machine source-build guide with one supported clone/build/test/run path, pinned Rust/Node prerequisites, synthetic smoke configuration, and expected non-secret outputs.
- BYO Entra registration and Outlook add-in sideloading guide with placeholders only; shared registration remains gated on publisher/governance controls. Neither source-build path accepts secrets in command-line arguments or tracked configuration.
- Permission/privacy/provider disclosures, disconnect/revocation instructions, troubleshooting without content capture, and supported-capability matrix.
- Complete acceptance, contract, privacy, accessibility, reliability, and performance suites.
- Protected-CI and local public-repository gates, explicit release allowlist, no disposable-tenant raw CI artifacts/caches, and source/generated/package/SBOM/provenance/installer/log/history/source-map/diagnostic canary checks that fail without printing the canary.
- Independent G-SEC-AUDIT package covering Rust OAuth/token state, local encrypted state/rollback, add-in bridge, provider keys/endpoints, Graph mutations, updater, and dependency/release supply chain.

**Exit gate**

- G-RELEASE, G-PRIV, and G-SEC-AUDIT pass with no unresolved critical/high finding.
- A clean supported Windows machine can follow the documented public-repository path through build, synthetic tests, placeholder-safe configuration, add-in installation, and local startup without an OpenLoops-operated account/service or undocumented step.
- `git status --short`, staged public-repository check, package dry run/allowlist inspection, and history check pass at their required release points.
- Every advertised acceptance criterion has an executable test and evidence; gated/unsupported features are visibly disabled and not claimed.

### Phase 8 — Limited preview and release decision

**Deliverables**

- Synthetic-first onboarding and explicit privacy acknowledgement for any real mailbox evaluation.
- Supervised limited preview with automatic reminders disabled, no central telemetry, local issue export only, and documented capability/staleness limits.
- Triage of false detection, missed loop, association, deadline, and usability findings using synthetic reproductions; no real content enters the repository.
- Final gate/capability report and release decision.
- After the confirmation-first preview, run the final sealed G-AUTO evaluation; only a passing result enables hybrid as the production default. Run G-AUTO-FULL independently; fully automatic mode remains unavailable if it fails even when hybrid passes.

**Exit gate**

- No unresolved P0 privacy/security/data-loss issue.
- G-AUTO passes before the full MVP ships with hybrid as the approved default. G-AUTO-FULL also passes before the product exposes or claims the PRD's fully automatic option; failure does not weaken that gate or conflate automatic with hybrid.
- No real mailbox content, identifier, log, or transcript has entered source control or release artifacts.
- Product owner signs off the exact capability matrix and any divergence from the PRD.

## 7. Acceptance-criteria traceability

The IDs below correspond to PRD section 32. The authoritative requirement/disposition/test matrix is [PRD traceability](prd-traceability.md); these rows are delivery navigation and do not override the accepted OWN decisions or named gates.

| AC | Requirement and test | Phase/gate |
|---:|---|---|
| 01 | Eligible incoming read/first-observation event is analyzed idempotently, including coalesced state and history policy | Phase 3; OWN-09; G-MAIL |
| 02 | Newly observed saved sent copy is analyzed without claiming delivery; coverage limits tested | Phase 3; OWN-09; G-MAIL |
| 03 | At least one grounded request/promise is detected | Phase 4; G-MODEL |
| 04 | One message produces multiple independent loops | Phase 4; atomicity threshold |
| 05a | Every detected email/invitation loop has valid specific message/invitation evidence references | Phases 3–6; G-MAIL/G-MODEL and G-CAL as applicable; source/span validators |
| 05b | Every email-grounded manual loop has user-selected valid message/component evidence | Phases 3–5; G-MAIL; source/span validators |
| 05c | Every artifact-backed manual loop has a valid `UserAuthoredArtifactRef` and origin-specific field ownership | Phases 5–6; OWN-07; G-PRIV plus G-TODO or G-CAL |
| 06 | Readable task reconstructs in memory with no stored summary | Phases 3–5; G-PRIV |
| 07 | Waiting party resolves through participant/span evidence | Phase 4; identity evaluation |
| 08 | Explicit or inferred deadline remains typed and evidence-backed | Phase 4; deadline suite |
| 09 | Missing deadline produces one review prompt and allowed choices | Phase 5 |
| 10 | One user-selected To Do or gated calendar artifact is created idempotently | Phases 5–6; G-TODO/G-CAL |
| 11 | Add-in lists active/reviewable loops from live reconstruction | Phase 5; G-ADDIN |
| 12 | Evidence navigation opens supporting correspondence where available | Phases 3/5; G-MAIL/G-ADDIN |
| 13 | Later relevant message updates operative deadline and preserves history | Phase 4 |
| 14 | Possible closure evidence is detected and associated per loop | Phase 4 |
| 15 | Inferred closure always requires confirmation | Domain invariant; Phases 4–5 |
| 16 | User can select alternative evidence | Phase 5 |
| 17 | User can choose completed outside email | Phase 5 |
| 18 | Repeated quoted history does not duplicate a loop | Phase 4; dedup threshold |
| 19 | User-defined names/aliases affect reviewable identity resolution | Phases 4–5 |
| 20 | Daily summary reconstructs and renders; optional self-email is scoped | Phase 6 |
| 21 | Optional configurable celebration has no performance profile | Phase 6 |
| 22 | User-configured local/external model works without key/content logging | Phases 2/4; G-MODEL/G-PRIV |
| 23 | State-location feasibility and local/cross-device tradeoff have owner disposition; selected minimized evidence map works | Phases 0–3; OWN-06; G-STATE |
| 24 | State excludes human-readable bodies/summaries and contains only OWN-07-approved derived metadata | All phases; OWN-07; G-PRIV |
| 25 | Bounded session-only test mode simulates results without writes | Phase 6 |

## 8. Test strategy

### 8.1 Deterministic unit/property tests

- State-machine legality and user-confirmation invariant.
- Deadline parsing across locale, timezone, DST, EOD, date-only, range, event-relative, and ambiguous phrases.
- Unicode block/range validation and re-anchoring.
- Quote/signature segmentation, action splitting, keyed dedup, and deterministic association retrieval.
- Checkpoint/operation state machines under every crash boundary.
- Encryption envelope/AAD, random nonce/collision/RNG failure, DB/profile rollback recovery, key inventory/rotation, migration/downgrade, cache concurrency, and secure-store absence.
- Sanitized error/log allowlist rejects unexpected fields/types.

### 8.2 Synthetic model evaluation

Build a versioned corpus covering direct/soft requests, outgoing promises, acknowledgements, questions, multiple actions, identity ambiguity, dates, quote/forward traps, sibling loops, attachments/links, partial/substantive closure, delegation, renegotiation, mootness, hostile prompts/HTML, invalid outputs, unavailable evidence, and operational failures.

At least 30% of held examples are difficult/ambiguous and at least 20% contain quoted history. Split by conversation family. Public train/dev fixtures may live in the repository; the release-judge corpus stays sealed outside the repository, implementation-LLM context, and maker workflow. Define minimum positive/negative samples per stratum, immutable corpus hashes/version, maximum tuning rounds, confidence intervals/lower-bound gates, and contamination checks. Report macro and per-stratum recall/precision/F2, atomic split/merge errors, evidence span F1, attribution, deadline normalization, dedup, association, review routing, calibration, latency/cost, unintended mutations, and privacy canaries. “Zero in 10,000” is an observed finite-sample result, never a guarantee.

### 8.3 Disposable-tenant Graph contracts

Run every experiment applicable to the approved capability matrix in [Microsoft Graph validation plan](../research/microsoft-graph/validation-plan.md), plus calendar and add-in experiments added by this plan. The accepted product decisions supersede that research snapshot's provisional runtime: pure-Rust browser PKCE is required; WAM/MSAL-specific experiments run only if that future adapter is proposed. Work/school is the core matrix; personal accounts are tested separately and remain disabled until the complete capability matrix passes. Persist only sanitized outcome codes and pinned versions outside any real-user environment. Never commit client/tenant/account IDs, tokens, addresses, payloads, HAR files, screenshots, or logs.

### 8.4 Privacy/adversarial suite

Place unique synthetic canaries in subject, body, participants, filename, link label, provider prompt/output, token, authorization header, delta URL, Graph error, task title/body, and UI projection. Exercise success, failure, retry, forced crash/OOM, WebView reload, SQLite WAL/journal, provider SDK retry/error, update, uninstall, export, package, and CI paths; scan enumerated application-controlled artifacts. Add hostile HTML, remote resources, prompt injection, fake system/admin requests, Unicode confusables, SSRF/redirect/DNS/proxy cases, oversized inputs, and schema-valid but semantically malicious model responses. Expected prohibited-canary count and policy bypass count are zero within the enumerated boundary; pagefile/OS/Office caches/admin/snapshots are documented residual risks rather than untestable erasure claims.

### 8.5 End-to-end scenarios

Automate every stable product-spec scenario `AS-01` through `AS-23` for To Do/core/provider behavior, then repeat applicable reminder-neutral/calendar cases after G-CAL. Include add-in closed/open, companion restart, sleep/resume, offline/reconnect, consent revoked, provider timeout, cursor expiry, direct reminder edits, and deleted evidence. A new acceptance scenario MUST update this range or replace it with a generated scenario manifest in the same change.

## 9. Risk register

| ID | Risk | Severity | Mitigation / release rule |
|---|---|---:|---|
| R-01 | Polling cannot meet perceived “immediate” behavior | High | Measure freshness SLO, adaptive cadence, visible staleness; never claim instant delivery. |
| R-02 | Add-in cannot safely/reliably reach local companion across supported clients | Critical | G-ADDIN spike first; ship no unsupported client claim; retain native recovery surface. |
| R-03 | Message IDs/spans fail after moves, send, archive, copy, or formatting change | High | G-MAIL matrix, multiple locator/digest anchors, evidence-unavailable fallback. |
| R-04 | Microsoft-hosted evidence-map storage is unavailable or over-scoped | High | OWN-06 decides whether local-only is acceptable; feasibility spike uses explicit cross-device/permission/conflict/deletion criteria. |
| R-05 | External model exposes confidential content or endpoint enables SSRF/key exfiltration | Critical | Disabled by default, exact-origin disclosure, bounded payload, SSRF/redirect/DNS/proxy/TLS defenses, OS-stored key, source-sanitized errors, canary tests. |
| R-06 | False positives create durable noisy reminders under hybrid or fully automatic mode | High | Confirmation-first development/preview, mandatory G-AUTO for narrow hybrid and separate G-AUTO-FULL for broader automatic, deterministic eligibility constraints, per-stratum thresholds, immediate user fallback to confirmation-first. |
| R-07 | Duplicate/wrong reminder linkage after ambiguous POST | Critical | Local operation key plus proven remote marker; otherwise user-assisted no-retry reconciliation and G-AUTO failure. |
| R-08 | To Do delta/conditional-write documentation is unreliable | High | Ordinary enumeration baseline; endpoint contract tests; no unproven concurrency promise. |
| R-09 | Calendar adds unknown scopes/semantics | High | Separate G-CAL before code exposure or product claim. |
| R-10 | App registration/client ID is abused by a malicious fork | High | BYO registration initially; signed distribution, verified publisher/governance gates before shared ID. |
| R-11 | Prompt injection or hostile HTML crosses policy/UI boundary | Critical | Inert canonical data, no tools, strict schema, CSP/no remote loads, adversarial suite. |
| R-12 | Privacy promise is broken by derived metadata, logs, crashes, browser caches, packaging, or support | Critical | OWN-07/ADR-PRIV-001, accurate application-controlled boundary, allowlist diagnostics, canary scanning, residual-risk disclosure. |
| R-13 | Quarantined item blocks cursor or is discarded after cursor advance | High | Encrypted recoverable quarantine record in same cursor transaction; user retry/dismiss and version replay. |
| R-14 | High-recall candidates overwhelm review | High | Category visibility, confidence lanes, review-burden metric, no lowering of auto-action safety. |
| R-15 | “Learning from corrections” creates hidden profiling/data retention | High | MVP uses explicit rules/settings only; personalization requires new privacy decision. |
| R-16 | Self-email broadens permission, targets wrong alias, duplicates, or recursively creates loops | Critical | G-SELFMAIL canonical recipient, marker, recursion suppression, operation ledger, ambiguous no-retry; add-in default. |
| R-17 | Public repository/CI/package receives secrets or real mailbox artifacts | Critical | Local and protected-CI guardrails, synthetic fixtures, no force-add/raw CI artifacts, staged/history/package/installer/SBOM/provenance checks. |
| R-18 | AES-GCM nonce reuse or ciphertext swap after backup/rollback/migration | Critical | ADR-005 random 96-bit nonce, AAD binding, collision/RNG fail-close, restore/migration/rotation crash matrix. |
| R-19 | Same-user database/profile rollback repeats mutations or regresses state | Critical | Rollback suspicion marker and mandatory recovery/reconciliation; do not claim DPAPI prevents full-profile rollback. |
| R-20 | Cache/key lifecycle leaves reusable credentials or corrupts concurrent Rust OAuth token state | Critical | Pinned audited OAuth primitives, single-instance/locking/atomic replacement, token/key inventory/rotation/deletion/uninstall tests; no command-line keys. |
| R-21 | Signed update is downgraded, manifest-swapped, or staged package replaced | Critical | Signed anti-rollback metadata, key lifecycle, hash/size/channel/expiry, protected atomic staging, provenance verification. |
| R-22 | State/crypto migration crash or old binary corrupts newer data | Critical | Copy/verify/swap journal, per-record schema/key IDs, resumability, old-key retirement gate, downgrade-write refusal. |
| R-23 | Invitation/calendar artifact causes external meeting communication | Critical | G-CAL self-only zero-attendee/no-online-meeting/no-response tests; no automatic invite response. |

## 10. Required architecture decision records

1. **ADR-001 Runtime and self-hosting boundary:** Windows per-user desktop companion; deferred remote modes.
2. **ADR-002 Rust authentication:** selected OAuth crates, system-browser PKCE, exact Entra registration/redirect/scope/refresh/token-cache contract, single-instance behavior, account/cloud matrix; WAM excluded from MVP.
3. **ADR-003 Incremental authorization:** exact feature-to-scope manifest.
4. **ADR-PRIV-001 Derived metadata and source boundary:** approved fields/fingerprints/classifications and `LoopSourceRef` variants, including manual artifact identifiers/digests/source-field ownership/version/source-of-truth exception; purpose, disclosure, retention/deletion/reset/export, threat model, residual erasure limits.
5. **ADR-004 Synchronization:** eligible first observation/backfill, folder delta/coverage loss, reconciliation cadence/bounds, quarantine/cursor transactions.
6. **ADR-005 Persistence/cryptography:** AEAD envelope/AAD/random nonce, DPAPI/Credential Manager/OAuth-token/provider/pairing key inventory, rollback/recovery, migrations/rotation/downgrade.
7. **ADR-006 Evidence identity/anchoring:** exact Graph immutable-ID request behavior, folder/account binding, copy/send/move fallback, Office ID/deep-link contracts, ranges/digests, unavailable behavior.
8. **ADR-007 Model boundary:** local/external endpoint network policy, exact disclosure, bounded input, strict output, semantic adversarial policy, no tools/logs.
9. **ADR-008 Policy/state model:** independent facets, deadline/closure/correction commands, confirmation/reopen invariants.
10. **ADR-009 Reminder adapters:** remote correlation/ambiguous-write fallback, To Do baseline, calendar/invitation gate, per-field ownership/conflicts.
11. **ADR-010 Add-in bridge:** asset hosting/manifest/client matrix, discovery/bootstrap/pairing/session/link/CSP threat model.
12. **ADR-011 Automation/evaluation:** exact OL-REM-017 mode semantics, confirmation-first development/preview, hybrid full-MVP default after G-AUTO, fully automatic only after G-AUTO-FULL, sealed judge corpus/sample/confidence rules, rollback-to-confirmation-first behavior.
13. **ADR-012 Distribution and registration:** signed anti-rollback update/provenance and BYO vs shared Entra registration gates.
14. **ADR-013 Self-email:** canonical self-recipient, marker/recursion, scope, operation/ambiguous-send behavior.

## 11. Definition of done

The implementation is complete only when:

1. OWN-00 through OWN-10 remain accurately reflected in accepted ADRs (including ADR-PRIV-001), and every advertised PRD requirement/acceptance criterion maps to an executable passing test in `docs/prd-traceability.md`;
2. every Graph-dependent claim is either a passing contract test for the supported matrix or visibly disabled and documented;
3. every loop/state change is evidence- or user-action-backed and reconstructable without stored mailbox text;
4. closure confirmation, reminder idempotency, secure-store failure, stale-state, and evidence-unavailable invariants pass end to end;
5. synthetic quality thresholds and zero-tolerance application-controlled privacy/mutation/prompt-injection gates pass on an independently held, sealed release-judge corpus with declared sample sizes/confidence bounds;
6. signed package/update, dependency/SBOM/provenance, accessibility, recovery, and documentation gates pass;
7. a fresh supported Windows environment can clone, build, test, configure with placeholders, and run the companion/add-in through the canonical documented source path without a required OpenLoops-operated service or hidden credential;
8. release/package/repository scans contain no secret, private identifier, real user content, source map, log, transcript, or generated diagnostic artifact; and
9. the product owner approves the exact supported capability matrix and every documented PRD adaptation; failed calendar/add-in/personal/state/self-email gate outcomes are not silently omitted.

## 12. Implementation agent loop

# Loop Spec: OpenLoops MVP implementation
**Problem:** Implement the evidence-grounded, privacy-minimized OpenLoops MVP from the product specification without relying on unvalidated Microsoft Graph or Outlook capabilities.
**Unit of work / volume:** One vertical slice or gate-defined work item at a time; approximately eight phases with multiple independently testable stories.
**Pattern:** Pipeline backbone with maker-checker and adversarial gates — contract research → domain implementation → adapter implementation → automated verification → security review → human release gate.
**Roles:** Orchestrator maintains requirement/gate traceability and dependencies; maker implements one bounded story; checker runs its acceptance/security/privacy tests from fresh context; adversarial reviewer attempts to violate boundaries and reproduce edge failures; release judge verifies phase exit criteria and capability claims. Inputs are this specification, the research package, accepted ADRs, and synthetic fixtures; outputs are code, tests, sanitized decision records, and a capability matrix.
**Done-check:** Functional plus judgment/human-gate — all story tests and invariants pass; a fresh checker grades requirement/security/privacy assertions; Graph features have passing disposable-tenant contracts; the product owner approves outward-facing scope and release claims.
**Guardrails:** Maximum two maker/checker repair rounds per story before escalation; no real mailbox data or secret enters source control; no network mutation outside disposable synthetic tenants; no automatic send/publish/release; stop on a privacy canary, secret, unvalidated scope, or destructive migration.
**Concurrency:** Up to three independent workers within the available four-slot team, but serialize work that changes shared schemas, migrations, ADRs, or the same project files.
**Progress reporting:** Report phase, passed/total exit checks, active blockers, changed capability claims, and next dependency after every work item; never paste sensitive test output.
**Output artifact:** Source and tests in the planned repository layout, ADRs under `docs/adr`, approved OWN dispositions, and the sanitized `docs/prd-traceability.md` requirement/gate/capability matrix.
**Failure handling:** After two failed checks, mark the story blocked with the failing assertion and smallest reproducible synthetic case; do not weaken the gate or silently substitute an unverified implementation.

### Kickoff prompt for an implementation LLM

> **Goal:** Implement OpenLoops according to `docs/product-spec.md`, `docs/implementation-plan.md`, and `docs/prd-traceability.md`, beginning with Phase 0 and completing one gate-bounded vertical work item at a time. Read `AGENTS.md` and all referenced research before changing files. Treat OWN-00 through OWN-10 as approved product decisions; encode them faithfully in ADRs rather than reopening them. Maintain stable requirement, acceptance-criterion, gate, and ADR traceability.
>
> **Done means:** The current work item has its implementation, synthetic tests, privacy/security invariants, sanitized documentation, and exact phase exit checks passing. No feature is advertised or enabled unless its OWN disposition and named Graph/add-in/privacy gate pass. No human-readable source/generated mailbox text, unapproved derived metadata, real identifier, secret, prompt/model output, transcript, log, source map, or generated diagnostic artifact is tracked or packaged.
>
> **Verify by:** Run deterministic unit/property/integration tests, public development fixtures, the independently held release-judge model/adversarial suite, relevant disposable-tenant contract tests using only synthetic accounts/content, application-controlled privacy-canary scans, package allowlist checks, and the repository's public-safety checks at their required commit/release boundaries. Use a fresh checker to grade the work against exact requirement/test IDs and invariants; route every scope/privacy/release decision to the product owner.
>
> **Guardrail:** Do not implement later phases on assumptions from a failed/unrun gate. Do not request broader Graph permissions, persist content, add a plaintext secret fallback, auto-close loops, send to another person, publish, or weaken a security/privacy/repository gate. After two failed repair rounds, stop that work item and report the minimal synthetic reproduction, failed assertion, and decision needed.
