# ADR-013: Self-email

- **Status:** Accepted
- **Work item:** P0-WI-16
- **Owner decisions:** OWN-10
- **Blocking gates:** G-SELFMAIL, G-PRIV
- **Decision date:** 2026-07-21

## Context

OWN-10 already selects an add-in-first daily summary with self-email as a
separately enabled, separately consented, `Mail.Send`-only path (product-spec
PD-13, §3). Product-spec OL-SUM-001 fixes the in-add-in summary as the default;
OL-SUM-002 permits sending "the same summary to the authenticated user only
after G-SELFMAIL, separate `Mail.Send` consent, and explicit enablement" and
forbids an arbitrary recipient; OL-SUM-003 requires transient reconstruction
with no stored copy; OL-SUM-004 requires "a contract-tested canonical
self-recipient source for every supported account type, a pre-write operation
ledger, safe ambiguous-send handling, and a verified ... marker so the sent
summary is excluded from detection without excluding user-authored mail" (§6.10).
The G-SELFMAIL gate text (§10) requires the canonical self-recipient source,
alias/guest/personal-account coverage, the generated-message marker, recursive
detection suppression, operation idempotency, and ambiguous-send handling to
pass before self-email is anything but disabled. Implementation-plan §4 item 16
keeps self-email disabled until its gate and OWN disposition pass; the Phase 1
self-email spike (self-email spike bullet) requires proving the canonical
authenticated-mailbox recipient for every account type, the generated-message
marker and recursion suppression, `Mail.Send` consent, operation idempotency,
and lost-response behavior "without accepting arbitrary recipients"; the
Phase 6 deliverable places the optional self-email path "behind G-SELFMAIL with
canonical self-recipient, generated-message marker/recursion suppression,
incremental `Mail.Send`, lost-response operation handling, and no stored summary
copy"; §10 item 14 assigns this ADR the canonical self-recipient, marker/
recursion, scope, and operation/ambiguous-send behavior; and risk R-16
("Self-email broadens permission, targets wrong alias, duplicates, or
recursively creates loops", Critical) requires "G-SELFMAIL canonical recipient,
marker, recursion suppression, operation ledger, ambiguous no-retry; add-in
default."

ADR-003 already fixes that "Self-email adds `Mail.Send` only through separate
explicit consent after G-SELFMAIL and may address only the contract-tested
authenticated self recipient" and assigns this ADR self-email safety. ADR-009
and ADR-005 already fix the durable-before-request protocol this send
operation must reuse without redefinition: "Every remote mutation begins with
a pending encrypted operation-ledger record committed under ADR-005 before a
request" (ADR-009), and "pending operation identity precedes an external
request; ambiguous writes reconcile before retry" (ADR-005). ADR-004 already
fixes that "A saved sent copy may be analyzed once, but drafts and compose
activity never activate loops" and that the product only ever says "sent copy
observed," never "delivered" — the sent summary's own saved copy is therefore
an ordinary Sent Items delta observation unless a verified marker excludes it.
ADR-PRIV-001 already prohibits persisting "generated summaries or
explanations" among other mailbox-derived text, fixing the no-stored-copy rule
OL-SUM-003 requires. The threat-model routing index already names ADR-013 on
the `microsoft_artifacts` row alongside ADR-009/ADR-011 and gates
G-TODO/G-CAL/G-AUTO/G-AUTO-FULL/G-SELFMAIL.

The executable contract is `contracts/selfemail/self-email-boundary.json`.
P0-WI-16 requests no `Mail.Send` scope, sends no message, selects no marker
mechanism, and completes no acceptance criterion or scenario; G-SELFMAIL and
G-PRIV remain unrun.

## Decision

### Recipient policy

The only legal recipient of a self-email is the canonical contract-tested
address of the authenticated account. The source of that canonical address
(for example a Graph `/me` field, a primary SMTP proxy address, or an
authenticated-token claim) is `unresolved_pending_G-SELFMAIL`: candidate
sources may be listed for G-SELFMAIL to test, but none is selected or treated
as authoritative before the gate proves it correct across work/school,
personal, alias, and guest identities. Arbitrary or additional recipients, a
recipient sourced from user input, Cc, Bcc, reply-to redirection, and
distribution lists are prohibited absolutely; no future revision of this ADR
may widen the recipient set without a new reviewed decision.

### Consent policy

Self-email requires separate explicit enablement of the feature and a
separate incremental `Mail.Send` consent granted only after G-SELFMAIL passes,
matching ADR-003 exactly. Neither may be bundled into another consent request
or inferred from another grant. Disablement of the feature or loss/revocation
of `Mail.Send` consent stops every subsequent send before any request is
made; a send in flight when consent is lost is not exempted.

### Generation policy

The summary body is reconstructed transiently under OL-SUM-003 from the same
approved evidence and policy state that renders the in-add-in summary; no
summary copy, draft, or template output is stored by OpenLoops before, during,
or after a send. Content rules inherit the ADR-PRIV-001 privacy boundary in
full: a self-email may not carry any field that boundary prohibits, and
enabling self-email creates no new persistence exception.

### Marker policy

A verified OpenLoops-generated marker must accompany every self-email.
Candidate mechanisms (a custom Internet message header, a Graph
single-value extended property, or an equivalent proven field) are listed for
G-SELFMAIL to evaluate; selection of one is `unresolved_pending_G-SELFMAIL`
and no checker or manifest may pre-select a mechanism. Whichever mechanism is
eventually proven must survive the full round trip from send through the
Sent Items saved copy, be verifiable on that saved copy without relying on
subject text, exclude from loop detection only messages OpenLoops itself
generated, and never exclude a user-authored message that merely resembles or
quotes a summary. The marker never carries summary content, an identifier, or
a secret; it is, like the ADR-009 reminder marker, recoverable only through a
keyed derivation, never a plaintext content signal.

### Recursion suppression

A message carrying a verified marker must never create a loop, be folded into
a later summary as a summary-of-summary, or be re-summarized. Detection logic
must check the marker before any other loop-creation or summarization path
considers the message. If marker verification cannot be completed with
confidence, the fail-closed answer is to withhold the send entirely rather
than risk an unmarked or unverifiably marked message entering detection; a
suppression failure is a no-send, not a best-effort filter.

### Send operation protocol

The self-email send reuses the exact ADR-009/ADR-005 durable-before-request
protocol shape without a parallel implementation: a pending encrypted
operation-ledger entry is committed before any send request, and exactly one
request is issued per recorded attempt. An ambiguous outcome (timeout or an
unknown remote result) is never blindly retried; it reconciles against both
the durable ledger entry and the Sent Items delta observation before any
retry decision is made, matching ADR-004's sent-copy observation contract. A
definitively failed send surfaces visibly to the user rather than being
silently swallowed or silently retried.

### Schedule policy

A scheduled self-email may run only while the feature is enabled, `Mail.Send`
consent is current, and the account remains authenticated; any one of those
conditions failing stops the scheduled send before a request. Missed
scheduled sends (device offline, application not running, account
disconnected) coalesce into the next eligible run; there is no catch-up burst
that reissues every missed occurrence.

### Failure boundary

Secure-store loss, rollback suspicion, an account mismatch, or a
marker-verification failure each independently cause zero send requests for
the affected operation. None of these conditions is treated as a partial or
best-effort success; each is a complete stop before any Graph call.

### Unresolved mechanics

The canonical self-recipient source and the marker mechanism remain
`unresolved_pending_G-SELFMAIL`. This ADR pins the envelope both must satisfy;
it does not guess or pre-select either, and no checker, manifest, or later
document may fill in a concrete value for either before G-SELFMAIL runs.

## Consequences

P0-WI-16 accepts only this disabled decision contract. ADR-013 alone advances
from planned to accepted; every other Phase 0 ADR keeps its current status.
No `Mail.Send` scope is requested, no message is sent, no marker mechanism is
selected, and G-SELFMAIL and G-PRIV remain unrun; every acceptance criterion
or scenario that depends on either gate remains unpassed. ADR-003's Mail.Send
consent wording, ADR-004's sent-copy observation and delivery-claim rules,
ADR-005/ADR-009's durable-before-request operation-ledger protocol, and
ADR-PRIV-001's prohibition on a persisted generated summary are consumed
here, not reopened or weakened; this ADR adds no new persistence exception
and no new retry path. The `self_email_summary` capability in
`contracts/governance/capabilities.json` stays disabled and unadvertised
pending G-SELFMAIL and G-PRIV.

## Verification

The deterministic checker pins the complete self-email manifest and the
accepted-ADR/owner-decision inventory, validates the closed recipient,
consent, generation, marker, recursion-suppression, operation-protocol,
schedule, and failure-boundary catalogs, and reconciles ADR-003's `Mail.Send`
consent wording, ADR-004's sent-copy observation rule, ADR-005/ADR-009's
operation-ledger protocol, ADR-PRIV-001's prohibition on a persisted generated
summary, the governance registry, the support matrix, and build-skeleton
inactivity. Synthetic mutations must reject an arbitrary or second recipient
or Cc/Bcc, a recipient sourced from user input, `Mail.Send` bundled into
initial consent, a send without a durable pending operation, a blind retry
after an ambiguous send, a marker carrying content or a secret, a
marker-by-subject-text substitute, removed recursion suppression, exclusion
of user-authored mail from detection, a stored summary copy, a catch-up burst
of missed schedules, a gate-passed or capability-enabled claim, fresh-checker
preapproval, and additive documentation contradiction. Fresh send-safety,
privacy, governance, and adversarial judges are required for closure; their
prompts, transcripts, and output are not repository evidence.
