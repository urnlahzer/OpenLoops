# Threat-model routing index

**Status:** Routing baseline for P0-WI-01. The P0-WI-17 Phase 0 exit review
adds the last two missing detailed models — Graph mail content and
disconnect/uninstall — so all nine routing rows below now have at least one
linked detailed threat model; see "Detailed models completed so far." Every
detailed model remains decision-contract-only: no runtime, Graph/Office/model
transport, persistence, or named gate is active.

This index prevents an implementation path from being treated as safe merely
because its detailed ADR has not been written. Every routed capability remains
disabled until its named ADRs and gates pass. External content is always
untrusted input and cannot authorize tools, scope changes, or mutations.

| Flow ID | Flow/boundary | Prohibited behavior before gate | Detailed decision | Blocking gates | Failure owner | Safe fallback |
|---|---|---|---|---|---|---|
| oauth_token_state | OAuth transactions and token state | no secrets, embedded credentials, application permissions, plaintext cache, device-code downgrade, or content-bearing authentication logs | ADR-002, ADR-003, ADR-005 | G-ID, G-PRIV | product_owner | identity-dependent capabilities remain disabled |
| graph_mail_content | Graph mail content and synchronization | no real content in fixtures, logs, or state; no unbounded fetch or complete-event-observation claim | ADR-003, ADR-004, ADR-006 | G-MAIL, G-PRIV | product_owner | mail detection remains disabled |
| model_transmission | Model transmission | no implicit external transmission, redirect, ambient proxy route, key logging, transcript persistence, or model mutation authority | ADR-007, ADR-PRIV-001 | G-MODEL, G-PRIV | product_owner | model-dependent analysis remains unavailable |
| addin_bridge | Outlook add-in and local bridge | no browser persistence, bearer secret in a URL, unpaired command, machine-wide trust, or add-in synchronization authority | ADR-010, ADR-PRIV-001 | G-ADDIN, G-PRIV | product_owner | Outlook add-in remains disabled; native recovery only |
| local_state | Local state and cryptographic keys | no human-readable mailbox text, unapproved derived field, plaintext fallback, rollback-blind mutation, or cross-user access | ADR-PRIV-001, ADR-005 | G-STATE, G-PRIV, G-SEC-AUDIT | product_owner | durable state remains disabled; session-only or fail-closed behavior applies |
| microsoft_artifacts | Microsoft reminder artifacts | no unconfirmed or ungated write, other recipient or attendee, blind retry, automatic delete, or reminder-driven loop closure | ADR-009, ADR-011, ADR-013 | G-TODO, G-CAL, G-AUTO, G-AUTO-FULL, G-SELFMAIL | product_owner | confirmation-first applies where separately gated; otherwise the capability remains disabled |
| diagnostics_release_artifacts | Diagnostics, crashes, support, and CI | no content, identifier, token, raw URL, prompt, model output, transcript, automatic upload, or non-allowlisted diagnostic field | ADR-PRIV-001, ADR-012 | G-PRIV, G-RELEASE, G-SEC-AUDIT | product_owner | diagnostics, export, and release remain blocked |
| installation_update | Installation and update | no unsigned package or metadata, downgrade, unverified staging, hidden credential, source map, or uninspected artifact | ADR-012 | G-RELEASE, G-SEC-AUDIT | product_owner | publishing and update remain disabled |
| disconnect_uninstall | Disconnect and uninstall | no global-logout claim, orphaned bridge trust or secret, silent artifact deletion, or undisclosed residual state | ADR-002, ADR-005, ADR-010, ADR-012 | G-ID, G-ADDIN, G-STATE, G-PRIV, G-RELEASE | product_owner | cleanup failure blocks supported release |

Detailed models completed so far:

- [OAuth transaction and token state](oauth-token-state.md) — ADR-002 decision
  contract only; runtime and G-ID evidence remain unimplemented.
- [Graph mail content and synchronization](graph-mail-content.md) —
  ADR-003/ADR-004/ADR-006/ADR-007 over-fetch, persistence, coalesced-claim,
  cursor-expiry, quarantine, hostile-content, throttling, cross-account, raw-
  locator, folder-identity, retention, coverage-claim, and bounded-projection
  decision contract only; Graph, model, and all named gates remain inactive.
- [Privacy and local state](privacy-and-local-state.md) — ADR-PRIV-001 field
  boundary only; runtime schemas and state gates remain unimplemented.
- [Protected local state](protected-local-state.md) — ADR-005 encryption, key,
  transaction, rollback, migration, and lifecycle decision contract only;
  runtime and state-gate evidence remain unimplemented.
- [Evidence identity and navigation](evidence-identity-and-navigation.md) —
  ADR-006 locator, Unicode anchor, resolution, and navigation decision contract
  only; Graph, Office, persistence, and all named gates remain inactive.
- [Model provider boundary](model-provider-boundary.md) — ADR-007 local/cloud,
  network, consent, strict-output, semantic, and privacy decision contract only;
  provider transport and all named gates remain inactive.
- [Policy and state boundary](policy-state-boundary.md) — ADR-008 independent
  facets, deadlines, hypotheses, typed commands, idempotency, corrections, and
  reminder/automation deferral contract only; runtime and gates remain inactive.
- [Reminder adapter boundary](reminder-adapter-boundary.md) — ADR-009 operation,
  ambiguous-write, ownership, manual-source, Calendar, review-link, and privacy
  decision contract only; adapter runtime and gates remain inactive.
- [Add-in bridge](addin-bridge.md) — ADR-010 transport, first-pair bootstrap,
  least-authority session, network/certificate, review-link activation,
  content, and native-fallback decision contract only; bridge runtime and all
  named gates remain inactive.
- [Automation and evaluation](automation-and-evaluation.md) — ADR-011 mode,
  eligibility-stratum, feature-flag, sealed-corpus-governance, calibration-
  invalidation, rollback, and evaluation-evidence-privacy decision contract
  only; no evaluation, corpus, flag, or mode is active, and all named gates
  remain inactive.
- [Distribution and update](distribution-and-update.md) — ADR-012
  two-registration, update-trust-chain, release-artifact, installer/uninstall,
  and diagnostics-rule decision contract only; nothing is signed, packaged,
  registered, published, or installed, and G-ID/G-RELEASE remain inactive.
- [Self-email](self-email.md) — ADR-013 recipient, consent, generation,
  marker, recursion-suppression, operation-protocol, schedule, and
  failure-boundary decision contract only; no `Mail.Send` scope is requested,
  nothing is sent, no marker mechanism is selected, and G-SELFMAIL/G-PRIV
  remain inactive.
- [Disconnect and uninstall](disconnect-and-uninstall.md) —
  ADR-002/ADR-005/ADR-010/ADR-012/ADR-PRIV-001 orphaned-DPAPI, orphaned-
  bridge-trust, partial-cleanup, reconnect-after-failure, cross-user-deletion,
  no-bulk-artifact-deletion, residual-risk-disclosure, and migration-in-flight
  decision contract only; cleanup runtime and all named gates remain inactive.

`P0-THREAT-001` checks that all nine rows and their ADR, gate, owner, prohibited-
behavior, and fallback routes remain represented in the machine-readable
capability/gate inventory. This index does not claim that any detailed threat
model or technical gate has passed.
