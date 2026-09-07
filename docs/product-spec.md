# OpenLoops MVP product specification

**Status:** Product-owner-approved implementation baseline; named technical/security gates remain blocking
**Version:** 0.4
**Research baseline:** Microsoft Graph research snapshot dated 2026-07-18
**Product source:** OpenLoops MVP PRD draft supplied by the product owner

## 1. Purpose and outcome

OpenLoops helps an individual professional avoid forgetting expectations created in Microsoft 365 email. It answers:

> Who may be waiting for something from me, what do they appear to expect, when do they expect it, and does my email contain evidence that I closed the loop?

The product models an evidence-backed expectation between people. For detected email/invitation loops, a Microsoft To Do task or Outlook calendar appointment is only a reminder representation and is not the source of truth. The explicit manual-loop exception in section 5.8 uses a user-authored Microsoft artifact as its authoritative source.

The MVP is successful when it reliably surfaces genuine requests and promises, preserves uncertainty, and never presents absence of email evidence as proof that the user failed to act.

## 2. Normative language

`MUST`, `MUST NOT`, `SHOULD`, `SHOULD NOT`, and `MAY` are normative. A requirement marked **gated** remains mandatory for the claimed feature but cannot be implemented or advertised until its named validation gate passes.

The product uses three claim labels throughout technical design:

- **Verified:** supported by the repository's current Microsoft/IETF research.
- **Inference:** a design decision derived from verified facts.
- **Unresolved:** a behavior that requires current official-document research or a disposable-tenant contract test.

## 3. Research-backed product decisions

The product owner approved the decisions below on 2026-07-19. Phase 0 MUST copy their technical consequences into the named ADRs and threat models. Approval authorizes implementation of the stated product direction; it does not waive any Graph, identity, privacy, security, quality, or release gate.

| ID | Decision | MVP rule | Basis |
|---|---|---|---|
| PD-01 | Supported runtime | Windows-first, single signed-in OS user, one Microsoft account, local desktop companion plus Outlook add-in. “Self-hosted” means user-installed and user-controlled; it does not mean a remote Docker, NAS, or headless server. | Inference from the researched public-client and secure-store boundary. |
| PD-02 | Identity | Delegated public-client authentication only; a pure-Rust system-browser authorization-code flow with PKCE is the MVP path. WAM is a future optional adapter, not an MVP dependency. No client secret, embedded credentials, application permission, or automatic downgrade to device code. | **Verified facts:** distributed native app is a public client; PKCE/external browser is standards-backed. **Unresolved contract:** selected Rust OAuth library, loopback redirect, exact scopes, token refresh/cache, account matrix, and redirect behavior pending G-ID. |
| PD-03 | Account/cloud matrix | Work/school accounts in the commercial global cloud are required. Personal Microsoft accounts are a gated target. Shared mailboxes, multiple accounts, and sovereign clouds are deferred. | Personal-account behavior still requires contract tests. |
| PD-04 | Mail scope | Inbox and Sent Items are enabled by default. Other user folders are explicit opt-in. The initial history window is visible and defaults to 30 days. | Least-scope inference from the Graph research. |
| PD-05 | Change discovery | Per-folder delta polling and periodic reconciliation are the selected connector design. Webhooks and hosted relays are not required for the MVP. | **Verified facts:** Graph delta and webhook constraints. **Inference/unresolved contract:** cadence, coalesced-state coverage, and reconciliation behavior pending G-MAIL. |
| PD-06 | Timing language | “Immediate” means prompt processing after a change is observed. The product exposes last-successful-sync time and never promises instantaneous Graph delivery. | Polling cannot prove wall-clock event delivery. |
| PD-07 | Mail permission | `Mail.Read` is required when detection is enabled because bodies and quoted history are essential. `Mail.ReadBasic` MAY support a metadata-only connection test but cannot power detection. | Verified Graph permission boundary. |
| PD-08 | Reminder artifacts | Microsoft To Do is the first reminder adapter. Outlook calendar remains an MVP target but is gated on separate current Graph research and contract tests. | To Do is researched; calendar is not covered by the current package. |
| PD-09 | Reminder automation | The full MVP launches with hybrid mode as the new-install default: only G-AUTO-eligible narrow, high-confidence loops create reminders automatically; ambiguous/ineligible loops remain visible for review. Development and limited preview remain confirmation-first until G-AUTO passes. Fully automatic is a distinct explicit opt-in and remains unavailable until G-AUTO-FULL. | Preserves the PRD default and selectable modes while making each automatic mutation set independently release-gated. |
| PD-10 | Persistent state | The supported baseline is minimized, encrypted, per-user local state protected by the OS. Microsoft-hosted evidence-map storage is a replaceable adapter and a research spike, not an unverified launch claim. | The research identifies no suitable arbitrary Microsoft-hosted app-state mechanism. |
| PD-11 | Content persistence | OpenLoops MUST NOT intentionally persist human-readable source or generated mailbox text in its own store. It does persist enumerated, encrypted derived classifications and fingerprints; these remain sensitive Microsoft-derived metadata. Descriptive To Do/calendar artifacts and an explicitly enabled summary email are user-authorized Microsoft artifacts. | Explicit privacy-boundary decision required. |
| PD-12 | Model providers | Local and hosted providers are first-class. The MVP includes built-in Ollama-local and Ollama-Cloud profiles plus a provider-neutral approved-HTTPS contract. External transmission is disabled until exact-endpoint/provider disclosure and consent; keys use the OS secret store. | The owner requires a hosted-model path; BYO keys do not remove provider privacy risk. |
| PD-13 | Daily summary | In-add-in summary is the default. Self-email is separately enabled, separately consented, uses `Mail.Send`, and is addressed only to the authenticated account. | Incremental-consent boundary. |
| PD-14 | Telemetry | No telemetry by default. Diagnostics are allowlisted, content-free, local, and explicitly exported by the user. | Repository security research. |
| PD-15 | User learning | MVP corrections update explicit rules/settings and the current loop only. They do not train a personalized model or persist labeled mailbox text. | Learned personalization would change the privacy boundary. |

### 3.1 Approved owner decisions

The following decisions are authoritative product choices. A named technical gate may still block release or send the product back for an explicit scope decision; an implementation agent MUST NOT weaken a gate merely because the product direction is approved.

| Decision | Product choice | Rationale / condition | Owner disposition |
|---|---|---|---|
| OWN-00 Language | Pure Rust desktop companion; system-browser PKCE first; WAM gated/deferred | Keeps the native companion memory-safe and portable at the core without pretending Rust identity/token handling is already validated. | **Accepted 2026-07-19** |
| OWN-01 Runtime | Windows-first, local, single-user companion; no remote container/NAS/headless MVP | Matches the same-machine browser redirect and per-user protected-state boundary while keeping clone/build/run distribution. | **Accepted 2026-07-19** |
| OWN-02 Accounts | Commercial-global work/school accounts first; personal accounts only after the complete capability matrix passes | Personal sign-in alone is insufficient evidence for mail/To Do/calendar/add-in support. | **Accepted 2026-07-19** |
| OWN-03 Reminder default | Hybrid is the full-MVP default; development/preview stays confirmation-first until G-AUTO | The original PRD behavior is retained, but G-AUTO is a mandatory launch gate. | **Accepted 2026-07-19** |
| OWN-04 Calendar reminders | A To Do-first preview is allowed; Calendar is required before claiming the full PRD MVP | G-CAL must validate permissions, side effects, identifiers, edits, and links. | **Accepted 2026-07-19** |
| OWN-05 Calendar invitations | Invitation-loop detection is required with the Calendar capability | Invite mail/event correlation and closure semantics remain gated by G-CAL/G-MAIL. | **Accepted 2026-07-19** |
| OWN-06 State location | Minimized encrypted local state is the MVP baseline; Microsoft-hosted/cross-device storage remains a later feasibility adapter | Local state is accepted only with G-STATE, G-PRIV, and independent security-review gates; secure persistence failure must be session-only/fail-closed. | **Accepted with security condition 2026-07-19** |
| OWN-07 Persisted derived metadata and source variants | Allow the encrypted derived fields in section 5.6 and `UserAuthoredArtifactRef` identifiers/digests/ownership flags with documented retention/deletion | These fields remain sensitive derived Microsoft metadata and are covered explicitly by ADR-PRIV-001. | **Accepted 2026-07-19** |
| OWN-08 Model boundary | Support local and external BYO providers as first-class modes, including hosted Ollama use | External content transmission requires exact provider/endpoint disclosure, consent, OS-protected keys, and G-MODEL. | **Accepted 2026-07-19** |
| OWN-09 Timing | Polling/reconciliation with visible freshness and measured latency targets | The product will not claim observation of every intermediate read/sent-copy state. | **Accepted 2026-07-19** |
| OWN-10 Summary email | Add-in summary first; self-email appears only after G-SELFMAIL | Avoids early `Mail.Send` and recipient/recursion risk. | **Accepted 2026-07-19** |

“Pure Rust” applies to the native companion, background jobs, domain/application code, persistence, identity, and Graph/model adapters. The Outlook add-in is a sandboxed Office web add-in and therefore uses the supported Office.js/TypeScript surface; it owns no synchronization authority, token cache, or durable mailbox-derived state.

### 3.2 Phase 0 validation-matrix disposition

The product owner accepted `P0-MATRIX-001` on 2026-07-19. The exact disabled
validation targets are recorded in [the support matrix](support-matrix.md) and
`contracts/support/support-matrix.json`: Windows 11 x64 24H2/25H2 while
serviced; classic Microsoft 365 Outlook, new Outlook for Windows, and Outlook
web on the same approved Windows machine; one commercial-global work/school
Exchange Online primary mailbox; and a desktop Mailbox 1.13 `ReadItem` add-in
baseline. All other rows remain gated, deferred, or outside the MVP as recorded
there. This disposition pins what must be tested; it passes no capability or
release gate and creates no advertised support claim.

- **OL-GOV-001 MUST** preserve these dispositions in ADRs, traceability, release documentation, and capability flags. If a mandatory gate fails, implementation stops for a new owner scope decision; an agent cannot silently defer the feature, broaden permissions, or weaken the gate.

## 4. Actors and trust boundaries

| Actor/component | Authority | Prohibited behavior |
|---|---|---|
| User | Connect account, configure identity/model/settings, confirm reminders and closures, classify loops | None beyond normal Graph/OS authorization; every external effect remains attributable to an explicit setting or action. |
| Outlook add-in | Review, evidence navigation, corrections, settings, manual loop creation, optional compose hint | Cannot be the synchronization authority, silently send mail, persist mailbox content in browser storage, or claim an unsent draft created a loop. |
| Local desktop companion | Authenticate, poll Graph, perform transient analysis, hold minimized encrypted state, reconcile reminders | Cannot access another OS user's state, request tenant-wide permission, execute mailbox instructions, or log content. |
| Microsoft Graph/Microsoft 365 | Authoritative mailbox evidence and reminder-artifact systems | Is not assumed to provide arbitrary OpenLoops app storage until validated. |
| Model provider | Return typed hypotheses over bounded transient input | Is untrusted; cannot mutate Graph, choose arbitrary mailbox data, set final lifecycle state, or override policy. |
| Waiting person/stakeholder | Represented through evidence in mail | Receives no OpenLoops message, notification, or profile in the MVP. |

External email content is untrusted input. Instructions in bodies, subjects, signatures, quoted messages, attachment filenames, or linked-document labels never alter application policy or authorize tools.

## 5. Domain model

### 5.1 Loop

A `Loop` is one independently closeable expectation attributable to the authenticated user. One message may create zero, one, or several loops. Several loops may share source/evidence references.

The persistent record contains sensitive abstract or encrypted fields:

```text
Loop
  loop_id: random opaque identifier
  account_ref: random opaque account binding
  origin: email_evidence | calendar_invitation | user_authored_microsoft_artifact
  provenance: set(requested, acknowledged, promised, attributed, inferred, ambiguous)
  obligation_state: candidate | open | terminal
  review_flags: set(needs_review, historical_backfill, identity_ambiguous, association_ambiguous,
                    quote_ambiguous, deadline_ambiguous, delegation_ambiguous)
  deadline_state: unresolved | undated_confirmed | scheduled | approaching | overdue
  closure_review_state: none | possible | kept_open | evidence_requested
  reminder_state: none | proposed | pending_write | linked | changed |
                  completed_needs_evidence | missing | conflict | ambiguous_write
  analysis_state: current | queued | unavailable | quarantined | stale
  source_state: available | partially_available | unavailable | changed
  resolution: none | closed | declined | delegated | moot | dismissed | completed_outside_email
  confidence_bucket: high | medium | low
  source_refs: LoopSourceRef values grouped by semantic role
  operative_deadline: encrypted structured value/range plus source reference
  reminder_refs: encrypted artifact identifiers and reconciliation state
  transition_refs: ordered abstract transitions
  policy_version, schema_version, model_label
  last_evaluated_source_ref
```

The facets are independent. An open loop may simultaneously be overdue, require identity review, have a missing reminder, and contain partial closure evidence. `closure_review_state=possible` never replaces `obligation_state=open`.

Legal-combination rules:

1. `resolution!=none` requires `obligation_state=terminal`.
2. `obligation_state=terminal` requires one resolution and cannot be auto-produced from model evidence.
3. `deadline_state=undated_confirmed` is a durable user choice that suppresses repeat deadline prompts until later deadline evidence arrives or the user changes it.
4. `deadline_state=approaching|overdue` may coexist with any review, reminder, analysis, or evidence state while the obligation remains open.
5. `closure_review_state=possible|kept_open|evidence_requested` may coexist only with an open obligation. `kept_open` suppresses the same closure hypothesis/version but does not suppress causally later evidence.
6. `analysis_state=quarantined` requires an encrypted re-fetchable locator and retry/dismiss action before the synchronization cursor may advance.
7. Terminal loops may be reopened by an explicit user command; reopening retains the prior transition and returns to `open` with a user-review flag.

The UI may show several secondary chips, including provenance. The single primary label follows this strict precedence:

| Precedence | Primary display label | Facet rule |
|---:|---|---|
| 1 | Declined / Delegated / Moot / Closed / Dismissed / Completed outside email | `obligation_state=terminal` with matching resolution |
| 2 | Possible closure | `closure_review_state=possible`; obligation remains open |
| 3 | Needs review | Nonempty review flags, or analysis/evidence/reminder conflict requiring action |
| 4 | Overdue | `deadline_state=overdue` |
| 5 | Approaching deadline | `deadline_state=approaching` |
| 6 | Needs deadline | `deadline_state=unresolved` |
| 7 | Candidate | `obligation_state=candidate` |
| 8 | Active | `obligation_state=open` |

Requested, acknowledged, promised, attributed, inferred, and ambiguous are secondary provenance chips; more than one may appear. Renegotiated is a latest-event chip. Reminder, evidence, deadline, and analysis states remain visible as secondary chips even when a higher-precedence primary label applies.

#### Candidate establishment and promotion

Valid evidence means all required spans/participant references pass deterministic validation. Core ambiguity means actor, action atomicity, responsibility, quote-newness, or target-loop association is unresolved.

| Detection/result | Evidence and confidence | Source | Resulting obligation state | Review/reminder behavior |
|---|---|---|---|---|
| Explicit outgoing promise by authenticated/configured user | Valid; no core ambiguity; medium or high | Live or history | `open`, provenance `promised` | Live: apply configured reminder policy; only high-confidence G-AUTO-eligible cases auto-create under hybrid. History: add `historical_backfill`, no automatic artifact |
| Direct incoming request/question addressed to user | Valid; deterministic identity; no core ambiguity; medium or high | Live or history | `open`, provenance `requested`, even without acceptance | Missing deadline follows OL-DUE-007; live high-confidence G-AUTO-eligible cases may auto-create; history remains review-batched |
| Explicit acknowledgment/acceptance of an existing request | Valid association; medium or high | Live or history | Existing/new loop `open`, provenance adds `acknowledged`; add `promised` only if future act language supports it | Apply configured reminder policy; medium-confidence stays review-only under hybrid |
| Explicit responsibility attributed to the user | Valid deterministic identity; no transfer/role ambiguity; high | Live or history | `open`, provenance `attributed` | Live may auto-create only when G-AUTO-eligible; history remains review-batched |
| Direct unresolved calendar invitation | G-CAL/G-MAIL-valid correlation | Live or history | `open`, provenance `requested`, origin `calendar_invitation` | No automatic response/mutation; history review-batched |
| Soft/implied/social expectation or contextual/role attribution | Valid spans but inference/identity ambiguity or low confidence | Any | `candidate` | Add review flags; no reminder mutation |
| Any otherwise valid explicit detection below its establishment confidence threshold or with semantic ambiguity | Valid source reference; low confidence or unresolved meaning not covered above | Any | `candidate` | Add `needs_review` and the specific ambiguity flags; no reminder mutation until user confirmation |
| Ambiguous atomic split, quote-newness, actor, waiting party, or target association | Any | Any | `candidate` | Human review must resolve split/merge/ownership/association |
| User confirms a candidate is mine/actionable | Current loop version | Review | `open`; preserve provenance and append user transition | Reminder proposal follows policy |
| User dismisses/not-mine/duplicate candidate | Current loop version and reason code | Review | `terminal` with `dismissed` or typed resolution | No reminder create; apply optional explicit rule only after separate confirmation |
| Invalid/ungrounded extraction | Failed validation | Any | No loop state transition | Analysis error/quarantine as applicable; zero mutation |

A request therefore may become an open loop before acceptance, as required by the PRD. Model confidence alone never promotes a candidate; the evidence/identity/ambiguity predicates above are mandatory.

### 5.2 Source and evidence references

Every automated conclusion about a detected email/invitation loop MUST resolve to one or more evidence records. A model assertion is never evidence. An explicit user action may instead create/update the authoritative user-authored artifact source defined in section 5.8.

```text
LoopSourceRef = EmailEvidenceRef | CalendarInvitationRef | UserAuthoredArtifactRef
```

`EmailEvidenceRef` contains:

```text
EmailEvidenceRef
  account_ref
  message_locator: tested immutable-id form plus fallback locator/version
  conversation_ref and selected-folder context when available
  component: subject | body_block | quote_block | sender | to | cc |
             attachment_name | link_label | calendar_response
  anchor_version
  ordinal and normalized range
  keyed content, prefix, and suffix digests
  observed_at
  relation: task | ownership | waiting_party | deadline | modification |
            closure | delegation | supersession | context
```

`CalendarInvitationRef` contains the tested invitation-mail evidence reference plus encrypted calendar event/response locator and version when G-CAL supports it. `UserAuthoredArtifactRef` contains encrypted adapter/list/artifact identifiers, source-field ownership/version flags and keyed digests, artifact type, and observed time; it contains no title/body/due text.

No source text, participant value, subject, URL, filename, or generated explanation is stored in these records. At display time the resolver re-fetches the message/event/artifact and reconstructs the projection in memory. Email evidence is re-anchored against canonical content. If resolution fails, the UI labels the source unavailable or changed and routes dependent decisions to review.

### 5.3 Deadline evidence

The loop keeps requested, promised, inferred, operative, and internal-reminder deadlines distinct.

```text
DeadlineEvidence
  source_ref
  source_type: requested | promised | inferred | user_supplied
  precision: instant | date | business_day | week | event_relative | soft | unspecified
  encrypted_value_or_range
  timezone_id
  interpretation: resolved | ambiguous | needs_user_input
```

The encrypted structured value is operational metadata needed to schedule and sort. The original phrase is reconstructed from evidence and not persisted.

### 5.4 Transition record

Every state change records prior/new facets, reason code, actor (`system` or `user`), causing `LoopSourceRef` IDs and/or explicit user-action ID, timestamp, and policy version. It contains no copied rationale. Replayed input under the same policy is idempotent. A user correction dominates a repeated automated conclusion unless new source evidence is causally later.

### 5.5 User-provided settings

Identity aliases, role terms, timezone, EOD setting, exclusions, and provider configuration are explicit user input rather than derived mailbox content. They MAY be stored encrypted locally. Provider keys and state-encryption keys MUST use the OS secret store, not the settings database.

### 5.6 Persisted derived metadata and retention

The following are deliberately persisted, encrypted derived Microsoft metadata: provenance/classification enums, confidence bucket, ambiguity flags, lifecycle facets, normalized temporal value/range, email/invitation coordinates and keyed digests, source/version HMACs, relation codes, transition codes, model/policy/schema labels, aggregate local product counters, and manual-source `UserAuthoredArtifactRef` identifiers, keyed field digests, field-ownership/version flags, and origin state. They can reveal behavior or correlations even without readable text. OWN-07 and ADR-PRIV-001 MUST approve every source variant before implementation.

The privacy ADR MUST enumerate purpose, necessity, threat, disclosure, and retention for every field. Proposed defaults are:

- open/candidate loop, evidence, transition, and reminder-link records: until terminal resolution, user deletion, or disconnect;
- terminal loop/evidence/transition records: 30 days, configurable from immediate deletion through 365 days;
- message observations and dedup fingerprints: configured history window plus 15 days, unless referenced by an active loop;
- completed operation-ledger entries: 90 days; unresolved/ambiguous entries until reconciled or explicitly abandoned;
- aggregate product counters: rolling 30 days, resettable, with no person/client/message dimensions;
- tokens, provider keys, encryption/HMAC keys, pairing secrets: separate lifecycle in ADR-005 and ADR-010.

Deletion is logical deletion from application-controlled state plus best-effort database compaction/secure key retirement. The product MUST NOT promise physical erasure from SSD history, OS paging, backups, snapshots, endpoint security, or administrator-controlled artifacts.

### 5.7 Client association

`ClientAssociation` is optional and reviewable. It contains a loop ID, evidence references or an encrypted user-configured mapping reference, confidence bucket, and ambiguity flag—never a stored client name derived from mail. The UI reconstructs the label from current evidence/settings. If no grounded association exists, the loop is grouped as “Unassigned”; the system MUST NOT infer a client solely from a domain or display name.

### 5.8 Manual loop origin

Manual creation supports two explicit origins:

1. **Email-grounded manual loop:** the user selects one or more messages/components and identifies task/ownership/waiting-party/deadline spans. The normal evidence/reconstruction contract applies.
2. **User-authored Microsoft artifact:** when To Do (or gated Calendar) is enabled, an explicit user action creates a descriptive artifact inside Microsoft 365 and `origin=user_authored_microsoft_artifact`. That artifact—not a hidden OpenLoops text field—is the authoritative readable source. OpenLoops persists only its encrypted reference, approved derived state, and transition codes.

An ungrounded manual loop without a Microsoft artifact cannot survive restart under the privacy model and is not allowed. If the authoritative manual artifact is deleted/inaccessible, the loop uses source-unavailable behavior and cannot reconstruct its text. For `origin=user_authored_microsoft_artifact`, direct title/body/due edits are authoritative source-field changes, not reminder-local decoration; the next reconciliation updates the manual loop projection and approved derived temporal/state metadata. User deletion/dismissal follows the normal retention and logical-erasure disclosures. Manual artifact creation is confirmation-only, uses the mutation/reconciliation protocol, and requires an explicit OWN-07/ADR-PRIV-001 exception plus the relevant adapter gate.

## 6. Functional requirements

### 6.1 Authentication and authorization

- **OL-AUTH-001 MUST** use delegated public-client authentication through the system browser and authorization code with PKCE. G-ID MUST prove the selected Rust OAuth implementation, exact Entra registration/redirect (`localhost`/`127.0.0.1`, IPv4/IPv6), authority/audience, requested OIDC/delegated scopes, refresh-token behavior, account selection, concurrency, and protected token lifecycle. WAM is optional future scope and MUST NOT be an MVP dependency.
- **OL-AUTH-002 MUST NOT** ship or accept a client secret, certificate private key, ROPC, implicit flow, embedded credential collection, application permission, or tenant-wide access in the desktop product.
- **OL-AUTH-003 MUST** request exact OIDC/delegated scopes incrementally through the Rust OAuth client: only the G-ID-validated sign-in/refresh scopes, `Mail.Read` for detection, `Tasks.ReadWrite` for To Do creation/reconciliation, the validated delegated calendar scope for calendar reminders, and `Mail.Send` only for enabled self-email summaries. ADR-003 MUST separately list OAuth/Graph scopes, configured processing folders, and Office add-in manifest permissions/requirement sets.
- **OL-AUTH-004 MUST** bind every fetch and mutation to one explicit internal account reference and block cross-account/tenant mismatches.
- **OL-AUTH-005 MUST** support denied consent, admin approval, revoked consent, Conditional Access changes, disconnect, and reconnect without weakening authentication.
- **OL-AUTH-006 MUST** accurately explain that local disconnect removes OpenLoops-owned state but is not global logout, token revocation, or tenant-consent revocation.

### 6.2 Synchronization and triggers

- **OL-SYNC-001 MUST** maintain independent, encrypted checkpoints for every selected mail folder and reminder collection.
- **OL-SYNC-002 MUST** use bounded per-folder mail delta polling, exact field selection, paging, at-least-once processing, idempotency, and bounded resynchronization after an invalid cursor.
- **OL-SYNC-003 MUST** analyze an incoming message when a discovered change establishes an unread-to-read transition. Because polling may coalesce intermediate states, it MUST also apply the first-observation and bootstrap policy in OL-SYNC-010. `isRead` is a trigger, not evidence of careful review.
- **OL-SYNC-004 MUST** analyze an outgoing message when it first observes a saved sent copy in a monitored folder. Draft or compose analysis MUST NOT activate a loop. The UI says “sent copy observed,” never “delivered.” Send-without-saving, immediate move/delete, send-as/on-behalf-of, and alternate-folder behavior remain coverage limits until G-MAIL validates another signal.
- **OL-SYNC-005 MUST** process Inbox and Sent Items by default and allow explicit opt-in folders without silently expanding configured processing scope. The consent UI MUST disclose that delegated `Mail.Read` authorizes mailbox-wide reading even though OpenLoops policy fetches only configured folders/messages.
- **OL-SYNC-006 MUST** reconcile missed/duplicate/out-of-order changes and direct reminder edits. No feature may depend on receiving every real-time event.
- **OL-SYNC-007 MUST** commit idempotency/operation state before cursor advancement and reconcile ambiguous external write outcomes before retrying.
- **OL-SYNC-008 MUST** honor `Retry-After`, use bounded exponential backoff with jitter, isolate quarantined items, and expose stale/reconnect/resync status.
- **OL-SYNC-009 SHOULD** run while the user is logged in even when the add-in is closed. It MUST NOT claim background coverage while the companion is stopped, sleeping, signed out, or offline.
- **OL-SYNC-010 MUST** use this first-observation policy: during an explicit initial/backfill scan, analyze already-read inbound and all saved sent copies in the visible bounded window into a noninterruptive history-review batch; after the baseline watermark, analyze a newly discovered inbound item that is already read even if no prior unread observation exists. A newly discovered unread item waits for a later read observation. Source/version idempotency prevents duplicate analysis.
- **OL-SYNC-011 MUST** define coverage honestly: selected-folder delta cannot recover an item that arrives, is read, and leaves monitored scope between observations. Reconciliation MUST specify enumeration bounds, tombstones, folder rename/deletion behavior, and known unrecoverable cases; freshness UI links to this coverage statement.
- **OL-SYNC-012 MUST** support bounded, user-visible replay after provider/parser/policy/identity/folder changes and after quarantined/unavailable analysis. Replay uses the same source/version keys, never creates automatic reminders for historical results, and presents newly detected history as a review batch.
- **OL-SYNC-013 MUST** retain an encrypted re-fetchable locator, observed version, failure class, retry generation, and user retry/dismiss state for every quarantined item before advancing its cursor. `failed_bounded` without recoverability is not a terminal job outcome.

### 6.3 Candidate detection and atomicity

- **OL-DET-001 MUST** detect candidates for explicit outgoing promises, direct requests, accepted/acknowledged requests, direct unanswered questions, direct aliases/@ mentions, and responsibilities explicitly attributed to the user.
- **OL-DET-002 SHOULD** detect soft commitments, implied commitments, social expectations, role/context attributions, and inferred professional-response expectations; ambiguous results remain visible and review-only.
- **OL-DET-003 MUST** split independently actionable clauses into independently managed loops with separate task evidence, deadline, reminder, state, closure association, and review history.
- **OL-DET-004 MUST** preserve all applicable provenance values rather than overwriting `requested` with `promised` after acceptance.
- **OL-DET-005 MUST** identify the likely waiting party through participant-position and/or message-span evidence and express uncertainty where requester, recipient, and stakeholder differ.
- **OL-DET-006 MUST** accept zero-to-many candidates per message and MUST allow one later message to affect only a subset of sibling loops.
- **OL-DET-007 MUST NOT** turn absence of a later email into evidence of failure. The only supported conclusion is “no closure evidence was found in the analyzed scope.”

#### Calendar-invitation loops (gated by G-CAL and G-MAIL)

- **OL-INV-001 MUST** detect a meeting invitation addressed to the authenticated user that still requires a response as a distinct candidate loop. The organizer is the likely waiting party; the expected action is respond to the invitation.
- **OL-INV-002 MUST** ground the loop in the invitation mail and, only after G-CAL, the corresponding calendar event/response-state reference. Duplicate invite mail/event representations MUST resolve to one loop.
- **OL-INV-003 MUST** treat an explicit response deadline as deadline evidence. Without one, the event start is contextual urgency, not a fabricated response deadline; the loop remains undated/reviewable and may suggest “respond before the meeting.”
- **OL-INV-004 MUST** treat accepted, tentatively accepted, declined, organizer-cancelled, and expired/moot states as typed candidate resolution evidence. Consistent with the MVP closure rule, inferred or remotely observed resolution is shown for user confirmation before the loop becomes terminal.
- **OL-INV-005 MUST** handle updates, reschedules, cancellation, duplicate transport, and responses from another client idempotently. It MUST NOT create attendees, send responses, or mutate the invitation automatically.
- **OL-INV-006 MUST** disable invitation-loop claims if the supported invite-message/calendar correlation contract fails. That failure requires an OWN-05 disposition; it is not silent scope deletion.

### 6.4 Identity resolution

- **OL-ID-001 MUST** support user-entered full/first names, addresses, aliases, nicknames, initials, usernames, role descriptions, team references, and other terms.
- **OL-ID-002 MUST** apply deterministic authenticated-address and exact configured-alias matches before contextual model inference.
- **OL-ID-003 MUST** treat contextual “you,” role-only, initial-only, and homonymous name mappings as reviewable unless an explicit user rule resolves them.
- **OL-ID-004 MUST** show the evidence and match rule supporting ownership and let the user mark “not mine.”

### 6.5 Quoted history, deduplication, and association

- **OL-DEDUP-001 MUST** segment current content, signatures/disclaimers, and quoted/forwarded content before extraction.
- **OL-DEDUP-002 MUST NOT** create a second loop merely because a request or promise already observed by the user is repeated in quoted history.
- **OL-DEDUP-003 MUST** allow quoted history to contribute to a new or reassigned loop only when the new wrapper creates/adds user responsibility or the original was outside known receipt scope.
- **OL-DEDUP-004 MUST** ground a forwarded responsibility transfer in both the wrapper message and relevant quoted evidence.
- **OL-DEDUP-005 MUST** retrieve existing-loop association candidates deterministically using account, conversation, participants, chronology, reply/forward relationships, action fingerprints, and bounded entity overlap before asking a model to rank supplied candidates.
- **OL-DEDUP-006 MUST** send uncertain split/merge/new-vs-existing decisions to review and MUST NOT silently merge independent obligations.
- **OL-DEDUP-007 MUST** use account-scoped keyed hashes and tested message locators for processing idempotency; text similarity alone is insufficient.

### 6.6 Deadline semantics

- **OL-DUE-001 MUST** preserve requested, promised, inferred, operative, and internal-reminder deadlines separately.
- **OL-DUE-002 MUST** parse explicit/relative expressions deterministically relative to the message timestamp and configured IANA/Windows timezone mapping.
- **OL-DUE-003 MUST** default EOD to 17:00 user-local time and allow configuration.
- **OL-DUE-004 MUST** preserve precision. “This week,” a date-only deadline, and “before the meeting” MUST NOT silently become unsupported exact instants.
- **OL-DUE-005 MUST** apply latest-relevant-message-wins only after the later message is associated with that loop. The new evidence and prior deadline remain inspectable.
- **OL-DUE-006 MUST** classify `ASAP`, “when you get a chance,” and implied response windows as soft/inferred urgency unless the user supplies a deadline.
- **OL-DUE-007 MUST** set an open loop without an operative deadline to `deadline_state=unresolved` and prompt after the relevant incoming read/first-observation policy. It MUST offer due date/time, no deadline, defer reminder, dismiss, and not-mine actions.
- **OL-DUE-008 MUST** keep a user-selected reminder time separate from the email-derived operative deadline.
- **OL-DUE-009 MUST** flag cross-timezone, event-relative, or multiple-plausible interpretations for review.

Operative-deadline decision table:

| New evidence | Association | Result |
|---|---|---|
| Same message contains requested and user-promised deadlines | One atomic loop, both explicit | Preserve both; the user's promised deadline is operative and the requested deadline remains visible. If actor/scope is ambiguous, review. |
| Later relevant user message proposes “I need until X” | Confidently associated | X becomes operative without waiting for acceptance, as required by the PRD; record `renegotiated`. |
| Later relevant requester message restates/counters deadline Y | Confidently associated and directive/expectation language present | Y becomes operative; preserve the user's prior proposed extension. |
| Later message merely mentions a date | Not clearly directive/modifying | Do not change the deadline; add context or route to review. |
| Multiple deadlines apply to independent actions | Action clauses separable | Split loops and attach each deadline to its action. |
| Multiple deadlines plausibly apply to one action | Ambiguous attachment/scope | Keep alternatives, set `deadline_ambiguous`, and require review. |
| User selects “no deadline” | Explicit user action | Set `undated_confirmed`; suppress repeat prompts until new deadline evidence or user edit. Sorting uses “No deadline,” never a fabricated date. |
| “Before the meeting” | Referenced event uniquely correlated | Store event-relative reference/range; resolve a reminder only after event correlation. If no unique event exists, offer event selection or keep reviewable. |
| Date-only / “this week” | Explicit but imprecise | Preserve date/range precision; reminder uses configured lead/display policy without asserting a more precise requester deadline. |
| Cross-timezone relative date or DST boundary | More than one reasonable interpretation | Present alternatives and source time context; no automatic reminder until resolved. |

User reminder overrides never alter the operative deadline. A later email-derived deadline change does not silently overwrite a directly edited reminder due date; OL-REM field-ownership rules apply.

Operational aging never changes the evidence precision. It only determines sorting, internal reminders, and approaching/overdue display:

| Evidence precision | Operational boundary | Approaching rule | Overdue rule / display |
|---|---|---|---|
| Exact instant | Evidence-derived instant | `now >= instant - configured lead` | `now > instant`; show exact source time/timezone |
| Date-only | End of that local date under the configured date-only policy; stored as a policy boundary distinct from evidence | Within configured lead of policy boundary | After policy boundary; display “Due [date]” plus policy indicator, never claim the sender wrote a time |
| Business day | End of resolved business day using configured locale/calendar policy | Within configured lead | After boundary; preserve “business day” precision |
| Week/range | End of the configured week/range; start/end retained | Within configured lead of range end; range may sort by end | After range end; display the range, not an invented point deadline |
| Event-relative | Resolved event start minus any explicit offset; otherwise no boundary | Only after a unique event is linked | After resolved event boundary; unresolved event stays reviewable and never ages to overdue |
| Soft urgency (`ASAP`, “when you can”) | None | May show an urgency chip, never approaching | Never overdue without user-supplied deadline |
| Undated confirmed/unspecified | None | Never | Never; sort in “No deadline” |

Timezone/DST resolution uses the configured timezone rules in force when the operational boundary is confirmed. A later timezone-policy change previews affected loops and requires confirmation before rewriting an existing boundary.

### 6.7 Closure, manual completion, and delegation

Deadline renegotiation (6.6) modifies the loop's operative deadline rather than serving as a closure kind; the closure kinds are completed, declined, withdrawn, superseded, and agreed.

- **OL-CLOSE-001 MUST** detect possible fulfillment, decline, withdrawal, delegation, renegotiation, supersession, mootness, and completion evidence from later relevant email.
- **OL-CLOSE-002 MUST** treat attachments, links, “Done,” substantive answers, forwarded responses, third-party delivery, and calendar responses only as candidate closure evidence.
- **OL-CLOSE-003 MUST NOT** close a loop from inferred evidence without explicit user confirmation.
- **OL-CLOSE-004 MUST** show original expectation evidence, candidate closure evidence, an evidence-templated uncertainty explanation, and actions to confirm, keep open, choose different evidence, or associate the evidence with another loop.
- **OL-CLOSE-005 MUST** allow a message to close one sibling loop while leaving others open.
- **OL-CLOSE-006 MUST** require one of: supporting email, attachment/link in an email, other message evidence, or `completed_outside_email` when the user manually completes a loop.
- **OL-CLOSE-007 MUST** support `not_mine`, `false_detection`, `duplicate`, `delegated`, `moot`, `no_longer_relevant`, and `other` correction codes without requiring free-text persistence.
- **OL-CLOSE-008 MUST** distinguish a confirmed decline from fulfillment, dismissal, and mootness. A decline closes the user's obligation as `resolution=declined` only when the user confirms explicit decline evidence or records a manual decline.
- **OL-DELEG-001 MUST** distinguish responsibility transferred, shared, assisted, retained, and unclear. Shared/assisted/unclear responsibility keeps the user's loop open.
- **OL-DELEG-002 MUST** treat another person's apparent completion as possible closure until the user confirms it.

Closure/reclassification command matrix:

| User command | Required input | Result | Reminder effect |
|---|---|---|---|
| Confirm fulfilled | One or more closure evidence refs | Terminal `closed`; preserve expectation and closure refs | Complete linked reminder only after a separate idempotent adapter operation succeeds |
| Confirm declined | Explicit decline evidence or manual decline confirmation | Terminal `declined`; preserve request and decline refs/reason code | Offer complete or detach; never send the decline automatically |
| Confirm withdrawal/superseded/no longer relevant | Withdrawal/supersession ref or explicit manual choice | Terminal `moot` with reason code | Offer complete or detach; never delete automatically |
| Confirm transfer | Delegation evidence or explicit manual confirmation | Terminal `delegated` only when user responsibility transferred; shared/assisted stays open | Offer complete or detach |
| Keep open | Closure-hypothesis version | Obligation remains open; `kept_open` suppresses that exact hypothesis | No artifact mutation |
| Remap evidence | Selected evidence and target loop | Atomically detach relation from old candidate and attach to target; both loops re-evaluated | No artifact mutation until resulting user command |
| Select different evidence | User-selected message/component | Replace/add proposed closure relation and re-render | No artifact mutation |
| Completed outside email | Explicit user confirmation | Terminal `completed_outside_email`; no fabricated evidence | Offer complete reminder |
| Dismiss/classify | Reason code; optional explicit rule choice | Terminal `dismissed` or appropriate typed resolution | Offer detach; never delete automatically |
| Undo/reopen | Explicit user action | Append reversal transition, restore open obligation with review flag, and re-evaluate later evidence | Offer recreate/reopen reminder; never send external communication |

Commands are idempotent by loop version and command key. A stale command fails with a visible refresh conflict. Later causally new evidence may create a new review flag on a terminal loop but MUST NOT silently reopen or change its resolution.

### 6.8 Reminder artifacts

- **OL-REM-001 MUST** implement Microsoft To Do reminders using delegated `Tasks.ReadWrite`, ordinary enumeration as the initial reconciliation baseline, and contract-tested write behavior.
- **OL-REM-002 MUST** implement Outlook calendar reminders only after Gate G-CAL establishes supported account types, delegated permissions, create/update/delete behavior, identifier stability, conflict behavior, and direct-edit reconciliation.
- **OL-REM-003 MUST** create one independently manageable artifact per loop selected for representation.
- **OL-REM-004 MUST** put human-readable task/deadline content only in the user-authorized Microsoft artifact and retain only encrypted opaque linkage in OpenLoops state.
- **OL-REM-005 MUST** include a descriptive title, operative deadline where resolved, reminder time, OpenLoops review link, and message link where supported and tested.
- **OL-REM-006 MUST** treat artifact due-date/title/body edits as reminder-local for `origin=email_evidence|calendar_invitation`; they do not rewrite expectation evidence or operative deadline. For `origin=user_authored_microsoft_artifact`, those fields are authoritative source edits under section 5.8.
- **OL-REM-007 MUST** treat direct To Do completion for detected email/invitation loops as `reminder_state=completed_needs_evidence` and deletion as `reminder_state=missing`; neither closes the source loop. For a user-authored manual source, completion is an explicit user source action that enters the manual completion confirmation/reconciliation flow; deletion makes the source unavailable and never fabricates closure. Calendar uses only G-CAL-defined edit/delete/cancel semantics.
- **OL-REM-008 MUST** use a persisted operation key before external writes. After timeout or ambiguous success, it MUST reconcile before retrying to prevent duplicate artifacts.
- **OL-REM-009 MUST NOT** automatically delete a reminder in the MVP.
- **OL-REM-010 MUST** keep development, test mode, and limited preview confirmation-first until G-AUTO passes. The full MVP MUST then use hybrid as the new-install default. Fully automatic mode MUST remain unavailable until G-AUTO-FULL passes. Inferred closure and external communication are never automatic in any mode.
- **OL-REM-011 MUST** define per-field ownership by origin. For detected email/invitation loops, evidence owns the operative deadline, a direct edit owns `reminder_due_override`, and generated title/body become user-owned after direct edit. For a user-authored manual source, artifact title/body/due fields are user-owned authoritative source fields and are never overwritten by inference.
- **OL-REM-012 MUST** handle conflicts visibly. For a detected loop, if the operative deadline changes while a reminder override exists, show “expectation deadline changed” with keep-override/update-reminder choices. Without an override, an authorized service update may follow the operative deadline using compare/reconcile semantics. For a manual source, concurrent/replayed artifact changes use adapter version/conflict rules and never yield to model output.
- **OL-REM-013 MUST** record only abstract edit/ownership/version flags and digests, not the user-edited title/body. Until conditional-write behavior is proven, stale writes MUST stop at `reminder_state=conflict` rather than overwrite.
- **OL-REM-014 MUST** require an endpoint-supported, contract-tested opaque remote correlation marker for crash reconciliation. If none exists, an ambiguous create enters user-assisted reconciliation and is never retried automatically.
- **OL-REM-015 MUST** treat calendar artifacts as self-only events with zero attendees, no online meeting, and no response-request behavior. Calendar has no invented “completion” concept; direct deletion/cancellation and date/body edits use G-CAL-defined reconciliation rules.
- **OL-REM-016 MUST** put only a non-secret opaque loop handle in any review link and use a G-ADDIN-tested activation mechanism. It MUST NOT encode bearer/session/pairing tokens, raw Graph/Office IDs, account identifiers, or mailbox content. Unsupported To Do/Outlook clients omit the link or show safe fallback instructions.
- **OL-REM-017 MUST** implement reminder creation/update modes exactly as follows:
  - `confirmation_first`: every reminder creation and every evidence-driven service update to a Microsoft artifact requires an explicit user action; read-only reconciliation and local review-state updates still run automatically.
  - `hybrid`: automatically create only live, narrow-category cases allowed by G-AUTO—explicit promise, explicit directly addressed request, or explicit deterministic attribution—with a valid resolved deadline, high calibrated confidence, deterministic identity, and no quote/delegation/coreference ambiguity. Automatically update only Open-Loops-owned artifact fields when later evidence deterministically changes them, no user override/conflict exists, and the update class passed G-AUTO. All other mutations are proposed for confirmation.
  - `automatic`: after explicit opt-in/risk disclosure and G-AUTO-FULL, automatically create/update every live established loop in the user's enabled non-ambiguous categories whose exact category/confidence/deadline/update stratum passed G-AUTO-FULL. Candidate, ambiguous, identity/delegation/quote-conflicted, undated, historical/backfill, and stale-write cases always require review.
  A mode change is prospective. No mode silently creates or rewrites a historical batch, closes a loop, deletes an artifact, or communicates with another person.

### 6.9 Outlook experience

- **OL-UX-001 MUST** provide an Outlook add-in main view, review queue, possible-closure confirmations, evidence navigation, grouping/filtering, settings, model configuration, test mode, daily summary, manual loop creation, and corrections.
- **OL-UX-002 MUST** reconstruct each card in memory and show: task, waiting party, operative deadline, remaining/overdue duration, provenance, status, evidence links, last relevant event, reminder type/state, confidence/ambiguity, and suggested next action.
- **OL-UX-003 MUST** use empathetic, evidence-limited wording and MUST NOT state that the user failed, was unreliable, or necessarily omitted work outside email.
- **OL-UX-004 MUST** group/sort by deadline, time remaining, waiting party, client when inferable, conversation, loop type, confidence, state, overdue status, and review need.
- **OL-UX-005 MUST** support all detected categories in the main view by default; settings may change visibility and reminder eligibility.
- **OL-UX-006 MUST** avoid mailbox content in browser local storage, IndexedDB, service-worker caches, URL query strings, analytics, console output, or crash reports.
- **OL-UX-007 MUST** use authenticated, per-user local communication with the companion. Exact add-in-to-companion transport is gated by G-ADDIN.
- **OL-UX-008 MAY** show a nonblocking compose warning for likely promises. It MUST NOT block send or activate a loop before sent-item observation.
- **OL-UX-009 MUST** make manual creation choose either email-grounded evidence or a user-authored Microsoft artifact under section 5.8. It MUST NOT offer an ungrounded locally persisted free-text loop.
- **OL-UX-010 MUST** label manual origin, editing authority, and evidence/artifact availability so the user can distinguish inferred email expectations from a user-authored reminder.

### 6.10 Test mode, summaries, and reward

- **OL-TEST-001 MUST** let the user choose a bounded mailbox sample/window and run the active configuration without creating reminders.
- **OL-TEST-002 MUST** show detected loops, user-identified misses, ownership, deadlines, possible closure, ambiguities, and simulated reminders for the active session.
- **OL-TEST-003 MUST** persist only explicit configuration changes, abstract loop corrections already allowed by this spec, and non-content version data. It MUST NOT persist the sample, labels, predictions, prompts, or mailbox text.
- **OL-SUM-001 MUST** provide an on-demand and scheduled add-in summary containing created, confirmed-closed, possible-closure, changed-deadline, overdue, approaching, needs-review, and needs-deadline counts/items.
- **OL-SUM-002 MAY** send the same summary to the authenticated user only after G-SELFMAIL, separate `Mail.Send` consent, and explicit enablement. It MUST NOT accept an arbitrary recipient or send to another address.
- **OL-SUM-003 MUST** reconstruct summary text transiently. An enabled self-email is an intentional Microsoft artifact; no copy is stored by OpenLoops.
- **OL-SUM-004 MUST** use a contract-tested canonical self-recipient source for every supported account type, a pre-write operation ledger, safe ambiguous-send handling, and a verified Open-Loops-generated marker so the sent summary is excluded from detection without excluding user-authored mail.
- **OL-REWARD-001 MUST** make celebration enabled/disabled, intensity, wording style, emoji use, and frequency configurable. Default is subtle and non-gamified.
- **OL-REWARD-002 MUST NOT** create productivity, reliability, missed-deadline, or longitudinal performance scores.

#### Local product indicators

- **OL-METRIC-001 MUST** compute product indicators locally only when the setting is enabled: loops created, closures confirmed, possible closures awaiting review, dismissals by reason code, evidence opens, reminders created, review actions, deadline changes, approaching/overdue loops, undated loops, and loops first surfaced before/after their operative deadline.
- **OL-METRIC-002 MUST** define each counter from typed domain events, use a rolling 30-day default, and provide reset/delete. It MUST NOT retain message/person/client dimensions, task text, exact behavioral timelines, or create a reliability/productivity score.
- **OL-METRIC-003 MUST** label “genuine,” “missed,” and accuracy indicators as available only in the active session-only test mode or explicit user-confirmed classifications; the product MUST NOT infer a counterfactual that the user would have forgotten a loop.
- **OL-METRIC-004 MUST NOT** transmit indicators centrally in the MVP. Export, if later added, requires a separate privacy decision and content-free schema.

### 6.11 Model and policy contract

- **OL-MODEL-001 MUST** treat the model as an untrusted transient extractor/associator, never a lifecycle or mutation authority.
- **OL-MODEL-002 MUST** send one changed message plus only a bounded, relevance-selected context set. It MUST NOT submit the whole mailbox/thread by default, attachments, linked-document contents, credentials, or unrelated recipient lists.
- **OL-MODEL-003 MUST** use a versioned JSON Schema with strict unknown-field rejection. Every claim contains supplied opaque handles and valid canonical block/range evidence.
- **OL-MODEL-004 MUST** reject or route to review any malformed, unsupported, out-of-range, contradictory, cross-message, or ungrounded output. Invalid output causes zero Graph mutations.
- **OL-MODEL-005 MUST** parse/validate temporal values in deterministic code, even when a model proposes an interpretation.
- **OL-MODEL-006 MUST** delimit mailbox data as untrusted, prohibit model tool calls, and prevent content from altering policy, scopes, prompts, or output schema.
- **OL-MODEL-007 MUST** expose provider, endpoint, model, confidence/detection sensitivities, and external-data disclosure. Keys are never returned to the add-in after storage.
- **OL-MODEL-008 MUST** support safe reduced behavior when no provider is available. Existing loops remain intact; obvious deterministic extraction MAY continue; uncertain eligible triggers show `analysis_unavailable` without persisting message content.
- **OL-MODEL-009 MUST** generate UI explanations from validated templates/evidence in memory. A fluent model rationale is not stored or treated as evidence.
- **OL-MODEL-010 MUST** separate local-loopback and approved external-HTTPS endpoint modes. It MUST disable redirects and implicit proxy inheritance, never forward authorization across origins, reject link-local/metadata and unexpected private-network destinations, revalidate DNS/address policy, pin the consented origin, require valid TLS externally, and cap time/request/response size.
- **OL-MODEL-011 MUST** sanitize provider SDK errors at their source so prompts, responses, keys, headers, and endpoint paths cannot enter application exceptions or diagnostics.
- **OL-MODEL-012 MUST** treat schema-valid semantic manipulation as a model error class. Automatic eligibility, if enabled, requires independent deterministic positive constraints; a valid in-range span alone is insufficient. Provider/model-version drift invalidates prior automation calibration.
- **OL-MODEL-013 MUST** disclose the exact categories and bounded context sent to the exact endpoint, provider retention responsibility, and endpoint-change re-consent. Consent to one origin/model never follows a redirect or endpoint edit.
- **OL-MODEL-014 MUST** ship built-in `ollama_local` and `ollama_cloud` provider profiles. Local mode connects only to an explicitly approved loopback Ollama API. Cloud mode uses Ollama's documented HTTPS API and user API key through the common bounded/schema-validated provider contract. The key is write-only and OS-protected; local/cloud selection, endpoint, model, and transmitted-field disclosure are explicit.
- **OL-MODEL-015 MUST** include at least one working hosted-provider path in full-MVP acceptance. A generic approved-HTTPS provider MAY be added, but it cannot substitute an untested placeholder for the built-in hosted use case.

## 7. Defaults and settings

Settings are typed, validated, encrypted user input. The UI MUST show prerequisites, scope changes, and whether a change affects only future messages or offers a bounded replay.

| Setting | Allowed values / default | Gate/prerequisite | Existing-loop and replay behavior |
|---|---|---|---|
| History window | 1–365 days; default 30 | Mail connection | Expanding offers review-only backfill; shrinking does not delete active-loop evidence refs but limits new search/reconciliation. |
| Folders | Inbox/Sent default; other owned folders opt-in | G-MAIL | Adding offers bounded backfill; removing stops new processing and warns about coverage. |
| Poll cadence | 60–900 seconds; default 60; adaptive backoff | Measured G-MAIL load | Future scheduling only; no overlapping poll for one folder. |
| Reminder creation mode | confirmation-first / hybrid / automatic; full-MVP default hybrid | Hybrid requires G-AUTO; automatic requires G-AUTO-FULL and explicit risk acknowledgement; OWN-03 approved | Exact OL-REM-017 semantics; historical replay never creates reminders automatically; changing mode does not retroactively mutate without an explicit review batch. |
| Candidate category visibility | per-category show/hide; all shown | None | View policy only; hiding does not dismiss or delete loops. |
| Reminder eligibility by category | requested/acknowledged/promised/attributed/inferred/ambiguous toggles | G-AUTO/G-AUTO-FULL for automatic effects; ambiguous is confirmation-only | Future mutations only; existing reminders unchanged. |
| Request sensitivity | low/standard/high; default high-recall standard | Provider configured for inferred cases | Offers bounded replay; review-only results. |
| Deadline inference sensitivity | explicit-only/standard/high; default standard | Provider optional | Offers bounded replay; never overwrites user resolution automatically. |
| Closure sensitivity | conservative/standard/high; default conservative | Provider configured for semantic association | Offers bounded replay; all results remain possible closure. |
| Review thresholds | per claim type calibrated bucket thresholds; safe defaults pinned by policy version | G-MODEL | Policy-version replay offered; no automatic terminal change. |
| Identity mappings | addresses, names, aliases, initials, usernames, roles, teams, contextual rules | Explicit user entry | Re-evaluate affected candidates in bounded review-only replay. |
| Client mappings | optional user-grounded entity/evidence mappings | Explicit user entry | Reconstruct/group affected loops; no guessed client. |
| EOD | local time; default 17:00 | Timezone | Re-evaluate unresolved/future relative deadlines; existing user-confirmed value requires confirmation to change. |
| Week end/date-only policy | Friday/Sunday/custom and reminder display policy; onboarding choice/default Friday business EOD | Timezone | No silent change to existing deadlines. |
| Timezone | supported Windows/IANA mapping; inferred suggestion requires confirmation | Onboarding | Existing relative deadlines show a review preview before recomputation. |
| Reminder lead | duration/rule; default 60 minutes for exact instants | Reminder adapter | Updates reminder only under field-ownership/conflict rules. |
| Reminder type | To Do / calendar; default To Do | G-TODO or G-CAL | Future artifacts only; migration is an explicit user action. |
| No-deadline behavior | prompt once / queue silently / do not remind; default prompt once for live events, batch for history | None | Honors `undated_confirmed` and defer choices; no repeat until new evidence. |
| Excluded senders/domains | exact user-entered values | None | Future processing plus optional bounded replay/dismiss preview; OAuth scope is unchanged. |
| Ignored categories | per-category processing/reminder exclusion, distinct from visibility | None | Future processing; user chooses whether existing candidates are dismissed, hidden, or unchanged. |
| Daily summary destination | add-in / self-email / both / off; default add-in | Self-email needs G-SELFMAIL and `Mail.Send` | Future schedules only. |
| Daily summary schedule | local time and days; default 08:00 daily | Timezone | Coalesce missed add-in summaries into one next-open view; self-email uses operation ledger. |
| Reward | off/subtle/full, wording set, emoji toggle, frequency; default subtle with emoji off | None | Presentation only; no performance profile. |
| Model provider | disabled/Ollama local/Ollama Cloud/approved HTTPS origin; onboarding has no provider until user chooses | Exact endpoint/provider consent and G-MODEL | Existing eligible failures may be replayed as review-only; external use is first-class but never silently selected. |
| Model/endpoint/key | provider-defined validated values | OS secret store; no command-line keys | Endpoint change invalidates consent and active sessions; key rotation does not replay automatically. |
| Aggregate local indicators | off/on; default on for rolling 30-day counts | OWN-07 | Reset/delete anytime; no person/client/message dimensions. |
| Telemetry | off only in MVP | None | No effect. |

Hybrid and automatic use the distinct OL-REM-017 eligibility sets. Anything outside the active mode's passed gate is review-only. Closure, deletion, and external communication are never automatic. Invalid settings fail locally with no Graph/provider request. Secret values are write-only after entry.

## 8. Required user flows

### 8.1 Incoming read

1. A delta/reconciliation cycle reports a relevant incoming item and either establishes `isRead: false → true` or applies the post-baseline first-observation-already-read rule. Initial/backfill items are processed into a noninterruptive history-review batch.
2. The connector retrieves the minimum required fields, canonicalizes/sanitizes them in memory, and runs deterministic and configured model extraction.
3. It validates evidence, splits/associates/deduplicates candidates, applies idempotent transitions, and discards content.
4. The add-in shows the result after its next refresh. If no deadline exists, it shows the one-time `deadline_state=unresolved` action.
5. If processing is delayed or unavailable, the UI shows freshness and analysis status; it never implies complete coverage.

### 8.2 Outgoing sent

1. Compose-time analysis MAY show a nonblocking hint and creates no persistent loop.
2. Sent Items delta/reconciliation discovers a saved sent copy; it does not claim recipient delivery.
3. The normal extraction/correlation pipeline creates or updates one or more loops.
4. Reminder proposals follow the current confirmation/automation policy.

### 8.3 Possible closure

1. A later relevant message produces a validated closure hypothesis associated with one or more supplied candidate loops.
2. Each affected loop remains `obligation_state=open` and sets `closure_review_state=possible`; siblings not associated remain unchanged.
3. The card shows original and closing evidence plus an evidence-limited explanation.
4. The user confirms, keeps open, selects different evidence, or maps the evidence to another loop.
5. Only confirmation makes the loop terminal and, if configured, completes its reminder.

### 8.4 Direct reminder edit/completion/deletion

1. Reconciliation discovers the artifact change.
2. For `origin=email_evidence|calendar_invitation`, due/title/body edits update reminder-local display/override ownership only; they do not rewrite expectation evidence or the operative deadline.
3. For `origin=user_authored_microsoft_artifact`, title/body/due edits are authoritative source-field changes. Reconstruct from the artifact, update only the approved derived temporal/ownership/version metadata, and never let model output overwrite them.
4. For detected email/invitation loops, To Do completion sets `reminder_state=completed_needs_evidence`; deletion sets `reminder_state=missing`, and the source loop remains open. For a user-authored manual source, completion enters the explicit manual-completion confirmation/reconciliation flow and deletion makes the source unavailable without fabricating closure.
5. Calendar uses its separately tested date/edit/delete/cancel semantics and has no invented completion state. Concurrent or stale writes follow OL-REM-011–013 and stop for conflict review.

### 8.5 Evidence unavailable

If a message is moved beyond the tested locator contract, deleted, made inaccessible, or removed by retention, the loop and abstract history remain. The card may show only safe non-content facts such as status, deadline state/value where approved, reminder link, record age, and “evidence unavailable”; it MUST NOT pretend it can reconstruct task/person text. Evidence-dependent automation freezes. The user may select replacement evidence, completed outside email, dismiss, reopen, or permanently delete the loop. Unreferenced unavailable records follow section 5.6 retention. The system never stores a content copy as fallback.

### 8.6 First run and onboarding

1. Show the supported local/Windows boundary and choose BYO Entra registration or an approved shared registration only if ADR-012 gates it.
2. Validate client ID/authority/redirect configuration without echoing identifiers to logs, then complete system-browser PKCE sign-in through the Rust identity adapter.
3. Explain the difference between mailbox-wide `Mail.Read` authorization and configured Inbox/Sent/optional-folder processing; request incremental consent.
4. Confirm account/cloud, timezone, EOD/week-end/date-only policy, identity aliases/roles, history window/folders, and hybrid reminder behavior; explain exact OL-REM-017 mode differences, which categories can auto-create after G-AUTO, how to choose confirmation-first, and why fully automatic remains unavailable until G-AUTO-FULL.
5. Configure Ollama local, Ollama Cloud, or another approved exact HTTPS provider; validate the key with a content-free request where supported and obtain provider/endpoint/transmitted-field disclosure consent before mailbox content is sent.
6. Preview the bounded history scan size/scope and explain that already-read history enters a batch review without automatic reminders.
7. Run connection/secure-store/bridge checks and show capability-gate status. Unsupported features remain visibly disabled.

Failure at any step is recoverable without a weaker auth flow, plaintext secret, wider folder processing, or partial hidden setup.

### 8.7 Test mode

1. The user selects a bounded date/folder sample with a configured maximum item count and confirms provider disclosure.
2. The system runs transient extraction/association and shows session-only predictions, evidence, ownership, deadlines, closure, ambiguity, and simulated reminder outcomes.
3. The user can mark a false detection; select a message/action as a missed loop; correct split/merge, ownership, deadline, or closure association; and compare expected versus current result in memory.
4. Corrections remain session-only until the user explicitly saves a supported setting/rule change. Cancellation/provider failure discards the session projection.
5. Test mode performs zero reminder, calendar, or mail-send writes and persists no sample, label, prompt, prediction, or free-text correction.

### 8.8 Ambient review and summary delivery

When the add-in is closed, the companion queues only abstract review state. An optional OS notification may say only that a count of items needs review; it contains no task, person, subject, deadline phrase, or link carrying a secret. On next add-in open, pending items and missed scheduled summaries coalesce into one fresh reconstruction. Summary schedule follows the configured timezone/DST policy. Self-email, if G-SELFMAIL passes and the user enables it, may run while the add-in is closed using its own operation ledger.

### 8.9 Corrections and future effects

| Correction | Current loop | Optional future rule (separate confirmation required) | Replay |
|---|---|---|---|
| Not mine | Dismiss or remove ownership | Add/adjust explicit identity/role mapping | Offer bounded affected-candidate replay |
| False detection | Dismiss | Ignore exact category/pattern only when expressible without storing source text | Future only by default |
| Duplicate | Atomically merge/dismiss with surviving loop | No automatic rule; dedup defect becomes synthetic reproduction | Re-evaluate related conversation |
| Delegated/shared/retained | Apply typed review/terminal command | Optional role/delegation policy setting | Future only unless user requests replay |
| Moot/no longer relevant | Terminal typed resolution | None | None |
| Wrong deadline | Correct/user-supply deadline | EOD/week/timezone/deadline sensitivity change if applicable | Preview before bounded replay |
| Wrong client | Remove/reselect evidence/configured mapping | Explicit client mapping | Regroup affected loops |
| Other | Current-loop reason code; no free text persisted by default | None | None |

The product MUST NOT claim retraining. A durable rule is created only after the UI explains its future scope and the user confirms it.

### 8.10 Disconnect, uninstall, and update recovery

- **Disconnect** stops jobs, resolves or marks in-flight writes, removes application-owned token/cache entries where supported, deletes local checkpoints/loops/keys per user choice, and explains how to revoke consent/sessions separately. The user chooses whether existing Microsoft reminder artifacts remain, are detached, or are individually reviewed; OpenLoops never bulk-deletes them automatically.
- **Uninstall** repeats disconnect cleanup when possible, removes local bridge/certificate/protocol registrations and application-controlled caches, and explains residual Microsoft artifacts/consent and backup/OS limitations.
- **Update** verifies signed anti-rollback metadata and package identity before atomic installation. Failed update/migration retains a recoverable prior version/state or enters recovery mode; an old binary refuses to write a newer schema.

## 9. Nonfunctional requirements

- **OL-NFR-001 (freshness target):** while the companion is running, online, authenticated, and not throttled, target a single-flight 60-second start-to-start poll schedule per folder and p95 discovery within 2 minutes for the measured supported-load population. Per-cycle page/time budgets prioritize incremental work over rescan. This remains an internal target until G-MAIL load tests pass, not a Graph delivery guarantee.
- **OL-NFR-002 (processing target):** target p95 validated candidate/state update within 60 seconds after discovery, excluding a disclosed unavailable/throttled external provider.
- **OL-NFR-003 (coverage):** display last successful synchronization and analysis time per account/folder; never present stale state as complete.
- **OL-NFR-004 (idempotency):** duplicate pages, process restarts, retries, and ambiguous write responses produce zero duplicate loop transitions and zero duplicate reminder artifacts in the release test suite.
- **OL-NFR-005 (privacy):** synthetic canaries placed in every content/credential-like field produce zero occurrences in enumerated application-controlled state, logs, telemetry, temp files, SQLite WAL/journal, browser caches under product control, application-created crash artifacts, diagnostics, packages, installers, or CI artifacts, except the exact provider request payload explicitly enabled for its test. This does not promise physical erasure from OS paging, Office/WebView internals, endpoint security, admin access, or snapshots.
- **OL-NFR-006 (security):** token/model-key persistence uses the G-ID-approved Rust OAuth/token implementation and OS-protected stores with no plaintext fallback. Cross-user and machine-transfer tests fail closed. Same-user/full-profile rollback enters recovery and full reconciliation before mutation; it is not claimed cryptographically impossible.
- **OL-NFR-007 (resilience):** Graph 429/5xx/timeouts, cursor expiry, provider failure, malformed model output, and individual quarantined messages do not corrupt prior state or stall unrelated work.
- **OL-NFR-008 (accessibility):** add-in review and settings flows meet WCAG 2.2 AA keyboard, focus, contrast, labels, and status-announcement requirements.
- **OL-NFR-009 (localization):** evidence ranges operate on Unicode safely; display and temporal parsing are locale-aware while stored machine values remain canonical.
- **OL-NFR-010 (release integrity):** official packages and updates are signed, dependency-locked, produce an SBOM/provenance record, exclude source maps/generated diagnostics, and pass the public-repository gate.
- **OL-NFR-011 (source-buildability):** a fresh supported Windows environment MUST be able to clone the public repository, install documented pinned prerequisites, build, test, configure a user-owned Entra registration and model provider using placeholders, and run the companion/add-in without an OpenLoops-operated service or undocumented credential. The repository MUST provide one canonical source-build path and a synthetic smoke test; runtime secrets never enter source control or command-line examples.
- **OL-NFR-012 (local-state security posture):** the product MUST describe local encrypted state as a minimized residual-risk boundary, not as proof that the device cannot expose it. Broad public release is blocked until G-SEC-AUDIT covers the compiled Rust dependency graph, token/key handling, encryption envelope, rollback/backup behavior, IPC/add-in bridge, updater, and reproducible release-to-source provenance.

## 10. Quality and safety gates

All detection gates use a locked, hidden, wholly synthetic suite split by conversation family rather than message row. Synthetic performance does not establish real-world accuracy.

| Gate | Pass condition |
|---|---|
| G-ID | The pure-Rust system-browser PKCE implementation passes registration, loopback redirect/path/state/replay, authority/audience/scope, refresh/token rotation, concurrent-instance, consent/admin/revocation/disconnect, secure-store, and advertised account tests. WAM is tested only if later advertised. |
| G-MAIL | Exact selected fields; folder-policy versus token-scope behavior; Inbox/Sent delta; coalesced arrive→read/move; first observation; rules-routed/transient-folder mail; saved-sent-disabled/moved/deleted/send-as cases; paging/replay/cursor expiry; exact Graph/Office ID/fallback/link contracts; and bounded resync/known-loss disclosure pass. |
| G-TODO | `Tasks.ReadWrite` create/update/reconcile behavior passes; a durable endpoint-supported opaque correlation marker is proven or ambiguous creates enter user-assisted no-retry reconciliation; ordinary enumeration works without disputed delta; identical concurrent tasks and lost-response cases produce no automatic duplicate/wrong linkage. |
| G-CAL | Current official Graph calendar/invitation research is recorded; least delegated scope, invite-mail/event correlation, CRUD/direct-edit/identifier/conflict/deep-link behavior pass; generated reminder events have zero attendees/online meeting/response request and send no invitation/update/cancellation mail. |
| G-ADDIN | A runnable packaged prototype proves asset hosting, manifest permissions/requirement sets, client matrix, companion discovery, first pairing, session rotation, loopback certificate/network/CORS/CSP/Origin/Host/CSRF behavior, evidence navigation, compose hint, lifecycle, account switch, and uninstall/reinstall. It MUST NOT install machine-wide trust or a general-purpose trusted root with a retained signing key. Any custom loopback certificate/trust is current-user only, limited to the tested loopback names, uses a non-exportable private key ACL'd to the user/application, has explicit issue/rotation/expiry/replacement behavior, and is completely removed on disconnect/uninstall; failure of any trust cleanup or client path blocks the add-in. Failure blocks the Outlook-add-in MVP unless OWN-01 is changed. |
| G-STATE | OWN-07/ADR-PRIV-001 approves every persisted derived field and `LoopSourceRef` variant—including manual artifact identifiers/digests/field ownership/version/retention; encryption envelope, random nonces, AAD, rollback detection/recovery, migration/rotation/downgrade, cross-user/restore behavior, deletion limits, and any Microsoft-hosted adapter's cross-device/conflict/access/quota contract pass. |
| G-MODEL | Strict schema/evidence validation, semantic adversarial errors, SSRF/redirect/DNS/proxy/provider-error defenses, timeout/fallback, exact-endpoint disclosure, provider/model drift, and content-free observability pass. |
| G-AUTO | An independently held release-judge corpus outside the repository and maker context has adequate per-stratum sample sizes/confidence bounds; high-confidence eligibility has at least 95% lower-bound precision per category, valid explicit deadline, deterministic positive constraints, no identity/delegation/quote ambiguity, and zero observed unintended mutations in at least 10,000 events (reported as an observation, not a guarantee). OWN-03 already approves hybrid only after this gate passes. |
| G-AUTO-FULL | Every additional category/confidence/deadline/update stratum admitted by `automatic` mode independently meets at least the G-AUTO precision and mutation-safety thresholds on the sealed judge corpus; explicit opt-in/risk disclosure, instant rollback to confirmation-first, and negative tests for candidate/ambiguous/undated/history/conflict exclusions pass. A failed or untested stratum remains confirmation-only and the product cannot advertise fully automatic mode. |
| G-SELFMAIL | Canonical self-recipient source, aliases/guests/personal accounts, generated-message marker, recursive detection suppression, operation idempotency, and ambiguous-send handling pass; otherwise self-email stays disabled. |
| G-PRIV | Application-controlled state/log/temp/cache/WAL/journal/browser/provider-SDK/crash/diagnostic/package/CI artifacts contain zero prohibited canaries; secure-store absence fails closed. Residual OS paging, admin, endpoint-security, snapshot, and platform-crash exposure is disclosed, not claimed erased. |
| G-SEC-AUDIT | Before a broad public release, an independent security review covers Rust OAuth/token handling, local cryptography/state/rollback, add-in bridge, provider endpoints/keys, Graph mutations, updater, dependencies, and public-repository/release pipeline. All critical/high findings are fixed or explicitly block release. |
| G-RELEASE | A fresh-machine clone/build/test/run smoke path passes from the documented pinned toolchain and placeholder-only configuration; signed anti-rollback update metadata, channel/version/expiry/hash/size/key rotation/revocation, atomic staging/recovery, dependency/toolchain locks, SBOM/provenance, CI public-safety enforcement, archive allowlist, and repository staged/history gates pass. |

Detection release thresholds:

- Explicit direct request/promise: recall at least 95%, precision at least 80%.
- Atomic loop count: at least 90% exact; action-to-evidence exact match at least 85%.
- Direct configured identity attribution: precision at least 98%; contextual/role attribution is never auto-eligible.
- Explicit date/time normalization: at least 98% exact; at least 95% of ambiguous relative cases route to review.
- Quote duplicate suppression: precision at least 98%, recall at least 95%.
- Closure candidate recall: at least 90%; automatic closure count is always zero.
- Prompt-injection policy bypass, invalid-output mutation, privacy-canary persistence, and unintended external-recipient send: always zero.

## 11. Failure and degraded behavior

| Condition | Required result |
|---|---|
| Consent denied/admin approval required | Disable only the dependent feature and show the exact reconnect/admin state. |
| Secure storage unavailable | Ephemeral non-mutating status/test behavior only, or disabled synchronization; no token refresh, checkpoint/idempotency/quarantine persistence, provider use, Microsoft mutation, or plaintext fallback. |
| Graph throttling/outage | Backoff, preserve checkpoint, stale-status UI, bounded retry. |
| Invalid/expired delta cursor | Bounded rescan with idempotent reapplication and visible resync state. |
| Coalesced first observation already read | Apply OL-SYNC-010; analyze post-baseline item or add initial item to history-review batch. |
| Message/evidence unavailable | Preserve loop; mark evidence unavailable; no inference from missing content. |
| Model unavailable/invalid | Preserve state; deterministic reduced mode; analysis status/review; zero mutations. |
| Provider endpoint violates origin/network policy | Block before sending content/key; require endpoint correction and re-consent. |
| Reminder write outcome ambiguous | Reconcile using a proven remote marker; otherwise user-assisted reconciliation and no automatic retry. |
| Quarantined item exceeds retry budget | Persist encrypted locator/version/failure/retry state, advance cursor transactionally, and expose retry/dismiss; never discard silently. |
| Reminder deleted/completed | Loop remains open; surface missing/evidence flow. |
| Add-in unavailable | Companion continues supported synchronization; local status/review fallback is required before claiming background-only usability. |
| Cross-account mismatch | Block mutation and require explicit account recovery. |
| Database/profile rollback suspected | Stop mutations, enter recovery, reconcile reminders/mail checkpoints, and require user confirmation where ambiguity remains. |
| Hostile content | Sanitize and treat as inert data; no tools, remote loads, or policy changes. |

## 12. MVP scope and exclusions

The MVP includes the PRD's Microsoft 365 email, Outlook add-in, per-user background companion, BYO/local model configuration, request/promise/attribution detection, multiple loops, deadlines, missing-deadline review, quote handling, possible closure, manual confirmation, To Do integration, gated calendar integration, configurable reminders, evidence-linked view, minimized evidence-map persistence, summaries/rewards, and test mode.

The MVP excludes Gmail, Slack, Teams, attachment/link content analysis, messages to other people, automatic renegotiation, manager dashboards, organizational monitoring, performance scoring, centralized OpenLoops data storage, required SaaS accounts, shared mailboxes, multiple accounts, app-only access, remote containers/NAS/headless servers, webhooks/relays, and inference primarily from non-email systems.

## 13. Acceptance scenarios

- **AS-01 Incoming eligibility:** A newly observed `isRead:false→true` explicit direct synthetic request with valid deterministic user identity produces one grounded open loop by the next successful processing cycle without asserting attentive reading; an ambiguous request remains a candidate.
- **AS-02 Multiple sent promises:** A newly observed sent message with two independent promises produces two independently closeable loops and two separate reminder proposals.
- **AS-03 Missing deadline:** “Can you take a look?” produces `deadline_state=unresolved`; no factual date is invented.
- **AS-04 Renegotiation:** A later associated “I need until tomorrow” preserves both deadlines and makes the later relevant evidence operative.
- **AS-05 Quote/forward:** A repeated quote of an already observed request produces no duplicate; “Please take this over” plus forwarded history produces a reviewable new/reassigned loop grounded in both.
- **AS-06 Partial closure:** “Attached is the revised agreement” produces possible closure, not closure, and affects only the matching sibling loop.
- **AS-07 Direct reminder state:** Direct To Do completion opens evidence selection; calendar edit/delete/cancel follows its gated semantics; neither silently closes the loop.
- **AS-08 Outside completion:** Manual phone completion records `completed_outside_email` without fabricated evidence.
- **AS-09 Delegation:** “Jordan will send it” distinguishes authenticated-user Jordan, transferred, shared, assisted, retained, and unclear cases; ambiguity keeps the user's loop open.
- **AS-10 Evidence unavailable:** A moved/deleted/retention-removed message produces evidence-unavailable behavior without content fallback.
- **AS-11 Ambiguous reminder write:** A crash around an ambiguous reminder POST produces one artifact after reconciliation or enters user-assisted no-retry reconciliation when no marker exists.
- **AS-12 Hostile content:** Hostile prompt-like mail remains inert, produces no tool/policy action, and leaves no content canary in prohibited application-controlled artifacts.
- **AS-13 History bootstrap:** Initial 30-day scan analyzes already-read incoming and historical sent copies into one review batch without automatic reminders or repeated missing-deadline prompts.
- **AS-14 Calendar invitation:** A meeting invitation awaiting the user's response creates one loop; duplicate invite mail/event representations deduplicate; accept/decline/cancel evidence enters closure review without sending or mutating automatically.
- **AS-15 Reminder conflict:** A user-edited reminder due date survives a later email deadline change until the user chooses whether to synchronize it; the email-derived operative deadline still updates visibly.
- **AS-16 Quarantine recovery:** A quarantined message can be retried after provider recovery/policy change from its encrypted locator even though the folder cursor advanced.
- **AS-17 Rollback recovery:** Same-user database rollback halts mutations and reconciles existing reminders before any create/update retry.
- **AS-18 Self-summary:** A sent self-summary is recognized only by a verified generated marker, does not create a loop, cannot target another recipient, and an ambiguous send is never blindly repeated.
- **AS-19 Manual loop persistence:** An email-grounded manual loop reconstructs from selected evidence; a user-authored manual loop reconstructs from its Microsoft artifact; ungrounded local free text is rejected.
- **AS-20 Decline:** An explicit declined request enters possible resolution and becomes terminal `declined` only after user confirmation, without sending a message automatically.
- **AS-21 Hybrid default:** After G-AUTO, a high-confidence eligible live loop creates exactly one reminder automatically; a medium/low-confidence, historical, identity-ambiguous, delegation-ambiguous, quote-ambiguous, or undated case remains visible without automatic mutation.
- **AS-22 Hosted/local model profiles:** Ollama local and Ollama Cloud each return schema-valid evidence-grounded extraction through the same provider contract; endpoint/key/disclosure errors send no mailbox content, persist no transcript, and create no mutation.
- **AS-23 Reminder mode separation:** the same synthetic batch produces proposals only in confirmation-first, narrow explicit-category automatic mutations in hybrid after G-AUTO, and the separately gated broader non-ambiguous category set in automatic after G-AUTO-FULL; ambiguous, undated, historical, and conflicting cases remain review-only in all modes.

## 14. Requirement-source traceability

The detailed disposition/test matrix is [PRD traceability](prd-traceability.md). A section-level mapping below is only a navigation aid and MUST NOT be used to claim acceptance.

| PRD sections | Specification coverage |
|---|---|
| 1–5 Product, problem, thesis, principles, user | Sections 1, 3, 4, OL-UX-003 |
| 6 Definitions | Sections 5–6; orthogonal state mapping |
| 7–8 Creation and multiple loops | OL-DET-001 through OL-DET-007 |
| 9 Deadlines | OL-DUE-001 through OL-DUE-009 |
| 10–11 Incoming/outgoing processing | OL-SYNC-001 through OL-SYNC-009; flows 8.1–8.2 |
| 12–13 Content and quotes | OL-MODEL-002, OL-DEDUP-001 through OL-DEDUP-007 |
| 14 Identity | OL-ID-001 through OL-ID-004 |
| 15 States | Section 5.1 and transition model |
| 16–18 Closure, manual completion, delegation | OL-CLOSE-001 through OL-DELEG-002; flow 8.3 |
| 19 Reminder artifacts | OL-REM-001 through OL-REM-017; flow 8.4 |
| 20–21 Outlook and cards | OL-UX-001 through OL-UX-008 |
| 22 Summary/reward | OL-SUM-001 through OL-REWARD-002 |
| 23 Privacy | PD-10–PD-11, sections 4–5, OL-NFR-005–006 |
| 24 Model configuration | PD-12, OL-MODEL-001 through OL-MODEL-009 |
| 25 Settings | Section 7 |
| 26 Test mode | OL-TEST-001 through OL-TEST-003 |
| 27 Metrics | Section 10 and implementation-plan evaluation workstream |
| 28–30 Scope/future | Section 12 |
| 31 Stories | Section 13 |
| 32 Acceptance criteria | Section 13 and implementation-plan acceptance matrix |
| 33 Engineering questions | Gates in section 10 and implementation-plan spikes |
| 34 Core statement | Section 1 |

## 15. Evidence sources and known limits

This specification relies on:

- [Microsoft Graph connection research overview](../research/microsoft-graph/README.md)
- [Connection options and provisional architecture](../research/microsoft-graph/connection-options.md)
- [Security, privacy, and local state](../research/microsoft-graph/security-and-local-state.md)
- [Validation plan and documentation conflicts](../research/microsoft-graph/validation-plan.md)
- [Open product decisions](../research/microsoft-graph/product-decisions.md)
- [Ollama API introduction](https://docs.ollama.com/api/introduction)
- [Ollama API authentication](https://docs.ollama.com/api/authentication)
- [Ollama Cloud models](https://docs.ollama.com/cloud)

The current research does **not** validate Outlook calendar APIs, Outlook add-in event/lifecycle behavior, add-in-to-local-companion transport, deep links in every Outlook client, or a Microsoft-hosted arbitrary evidence-map store. Gate results must update the research package and this specification before those capabilities are advertised.
