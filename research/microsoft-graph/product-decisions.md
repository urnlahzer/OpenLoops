# Microsoft Graph product decisions

**Status:** Product-owner decisions recorded; implementation contracts remain gated

**Owner:** OpenLoops project

**Research snapshot:** 2026-07-18

Product-owner decisions recorded on 2026-07-19 govern the MVP. Technical details
that still require a gate or ADR must not be treated as permission to change an
approved product boundary.

## Stable owner-decision crosswalk

The normative wording remains in `docs/product-spec.md`. This crosswalk gives the
research snapshot stable identifiers so implementation work cannot reopen an
accepted product choice or mistake it for a passing technical gate.

| Decision | Accepted consequence | Governing ADRs | Blocking gates |
|---|---|---|---|
| OWN-00 | The native companion is pure Rust. System-browser authorization code with PKCE is the MVP identity path; WAM is deferred. The Office.js/TypeScript add-in is the supported sandbox exception. | ADR-001, ADR-002 | G-ID, G-ADDIN |
| OWN-01 | The MVP is Windows-first, local, unelevated, per-user, and single-account. Remote container, NAS, and headless modes are outside the boundary. | ADR-001, ADR-010, ADR-012 | G-ADDIN, G-RELEASE |
| OWN-02 | Commercial-global work/school accounts are the core matrix. Personal accounts stay disabled until their complete capability matrix passes. | ADR-002, ADR-003 | G-ID, G-MAIL, G-TODO, G-CAL, G-ADDIN |
| OWN-03 | Development and preview are confirmation-first. Hybrid becomes the full-MVP default only after G-AUTO; fully automatic remains unavailable until G-AUTO-FULL. | ADR-008, ADR-009, ADR-011 | G-AUTO, G-AUTO-FULL |
| OWN-04 | To Do-first preview is permitted. Calendar is required before claiming the complete PRD MVP. | ADR-009 | G-TODO, G-CAL |
| OWN-05 | Invitation-loop detection ships only with the validated Calendar capability. | ADR-004, ADR-006, ADR-009 | G-CAL, G-MAIL |
| OWN-06 | Minimized encrypted local state is the MVP baseline. Microsoft-hosted/cross-device state is a later feasibility adapter. Secure persistence failure is session-only or fail-closed. | ADR-PRIV-001, ADR-005 | G-STATE, G-PRIV, G-SEC-AUDIT |
| OWN-07 | Only the enumerated encrypted derived metadata and approved source variants may persist, including the manual-artifact exception with identifiers, digests, and ownership/version flags. | ADR-PRIV-001, ADR-006, ADR-009 | G-STATE, G-PRIV |
| OWN-08 | Local and external BYO model providers are first-class, including hosted Ollama use. External transmission requires exact disclosure, consent, protected keys, and a passing model gate. | ADR-007 | G-MODEL, G-PRIV |
| OWN-09 | Polling and reconciliation are the timing model. Freshness is visible and no intermediate-state or instant-delivery guarantee is made. | ADR-004 | G-MAIL |
| OWN-10 | The add-in summary is first. Self-email stays disabled until its separate gate passes. | ADR-013 | G-SELFMAIL |

All eleven decisions are accepted. A listed gate that is unrun or failed keeps
the dependent capability disabled and routes any scope change back to the
product owner.

### Phase 0 matrix disposition

`P0-MATRIX-001` was accepted by the product owner on 2026-07-19 without adding
or reopening an OWN decision. It pins the disabled Phase 0 validation targets:
Windows 11 x64 24H2/25H2 while serviced; classic Microsoft 365 Outlook, new
Outlook for Windows, and Outlook web on the same approved Windows machine; one
commercial-global work/school Exchange Online primary mailbox; and a desktop
Mailbox 1.13 `ReadItem` baseline. The model rows preserve OWN-08 but remain
disabled. The exact matrix and fallbacks live in
`contracts/support/support-matrix.json`; no gate or support claim passes here.

| # | Product question | Research default | Decision | Rationale |
|---:|---|---|---|---|
| 1 | Must OpenLoops read email bodies, or can it operate from metadata? | Bodies likely required to infer commitments; start permission tests with `Mail.ReadBasic`, then add `Mail.Read` explicitly | Yes; `Mail.Read` is required when detection is enabled | The PRD requires body, quoted-history, signature, and link-label analysis. `Mail.ReadBasic` remains only a connection/permission diagnostic. |
| 2 | Which mail folders are in scope? | Inbox and Sent Items, with other folders opt-in | Inbox and Sent Items by default; other owned folders opt-in | Matches the approved polling scope and makes processing coverage visible even though the Graph token permission is mailbox-wide. |
| 3 | How much history is inspected on first connection? | Bounded, visible 30-day default | Configurable; visible 30-day default | Backfill is review-batched and never creates reminder artifacts automatically. |
| 4 | Does OpenLoops create/update/delete Microsoft To Do tasks? | User-confirmed create/update; no autonomous delete | Create/update according to the approved confirmation-first/hybrid policy; never autonomously delete | Reminder mutations remain user-visible and idempotent; direct deletion does not close a loop. |
| 5 | Must every task creation be confirmed by the user? | Yes for the first release | Development and limited preview: yes. Full MVP: hybrid by default only after G-AUTO; confirmation-first remains selectable, and the distinct fully automatic option appears only after G-AUTO-FULL | Retains the PRD modes while independently gating the narrow hybrid and broader automatic mutation sets. |
| 6 | May OpenLoops persist human-readable candidate loops awaiting review? | No persistent review queue by default | No | Reconstruct readable content ephemerally from Microsoft evidence; Microsoft reminder artifacts may remain readable inside Microsoft 365. |
| 7 | May OpenLoops persist derived summaries, embeddings, prompts, or classifications? | No; they are derived Microsoft 365 content | Persist only the encrypted, enumerated abstract classifications, temporal values, fingerprints, source references, state, and history in product spec §5.6; no readable summaries, embeddings, prompts, raw model output, or copied text | The minimized local evidence map is needed for durable reconciliation, but remains sensitive and is subject to ADR-PRIV-001, retention/deletion, G-STATE, G-PRIV, and G-SEC-AUDIT. |
| 8 | Will Microsoft 365 content be sent to an external model or service? | No initially; local/transient processing only | Yes, only by explicit provider selection and consent; local and hosted BYO providers are first-class, with built-in Ollama local/cloud profiles | The user expects hosted Ollama-class operation. Exact provider, endpoint, model, transmitted fields, and provider privacy responsibility must be disclosed; keys remain OS-protected. |
| 9 | Is background synchronization required while the UI is closed? | Per-user background process only while the user is logged in | Yes, for a Windows per-user companion while the user is logged in | Supports the ambient experience without introducing a hosted OpenLoops service. |
| 10 | Must personal Microsoft accounts work at launch? | Yes, subject to contract tests | Not in the core launch matrix; enable only after the complete identity/mail/To Do/calendar/add-in capability matrix passes | Successful sign-in alone does not prove feature support. |
| 11 | Is a Windows-first release acceptable? | Yes; keep cross-platform interfaces internally | Yes | The MVP is a Windows-first, single-user native companion. |
| 12 | Are macOS and Linux launch requirements? | macOS later; Linux only with a supported Secret Service and no plaintext fallback | No | Preserve portable Rust domain/adapter boundaries, but do not advertise unsupported platforms. |
| 13 | Are shared mailboxes or shared To Do lists launch requirements? | No | No | The MVP is private to one professional and does not claim shared-mailbox/team workflows. |
| 14 | Are multiple Microsoft accounts per OS user required? | No for the first release | No | The supported MVP boundary is one Microsoft account per OS user. |
| 15 | Are Docker, NAS, remote-browser, or headless deployments launch requirements? | No; separate future threat model | No | The accepted local-companion security boundary is same-user/same-machine; remote modes require a separate threat model. |
| 16 | Is near-real-time notification worth a webhook or Azure dependency? | No; polling first | No | Use polling plus delta/reconciliation, visible freshness, and measured latency targets. |
| 17 | Will the project operate a central Entra registration? | Eventually, after publisher/governance gates; BYO during development | Not required for source builds or MVP development; shared registration is a separately gated distribution option | A user can clone/build/run with a documented user-owned registration. No undocumented OpenLoops service is required. |
| 18 | Can the project maintain a publisher domain and Microsoft publisher verification? | Required before broad use of a shared registration | Required only before offering a shared project registration | This is a release-governance prerequisite, not permission to block self-built BYO registration. |
| 19 | Are national/sovereign Microsoft clouds in scope? | No for the first release | No | The core launch matrix is commercial global cloud only. |
| 20 | Is any telemetry acceptable? | None by default; explicit redacted diagnostics only | None by default | Any future diagnostics must be explicit, local-first, redacted, and pass the public/privacy gates. |

## Complementary product-owner decisions

- The desktop companion is implemented in pure Rust. The MVP identity path is a
  system-browser authorization-code flow with PKCE; WAM is deferred unless a
  later Rust-compatible adapter passes G-ID.
- Microsoft To Do may ship in a preview before Calendar, but calendar reminders
  and unresolved-invitation detection are required before claiming the complete
  PRD MVP.
- The MVP state baseline is minimized, encrypted local storage. This is accepted
  only with fail-closed OS secret storage, privacy/state gates, and an independent
  security review before broad public release. Microsoft-hosted/cross-device
  state remains a later feasibility adapter.
- The add-in summary ships first. Self-email remains disabled until G-SELFMAIL
  proves canonical self-addressing, recursion suppression, and ambiguous-send
  behavior.

## Decisions that alter the privacy boundary

The security and privacy documents must be revised before implementing any
decision that enables:

- Persistent email/task content or a review queue.
- Derived summaries, embeddings, prompts, or classification history.
- External model or telemetry transmission.
- Automatic actions without user confirmation.
- A public webhook, hosted relay, remote UI, NAS, container, or multi-user mode.
- Application permissions or tenant-wide access.
