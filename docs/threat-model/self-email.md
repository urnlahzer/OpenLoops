# Self-email

**Status:** ADR-013 decision contract accepted; runtime, `Mail.Send` consent,
sending, marker selection, and all named gates remain inactive.

The primary safety properties are that a self-email can never be addressed to
anyone other than the canonical contract-tested address of the authenticated
account; that a crashed or ambiguous send is never blindly retried but instead
reconciles against the durable operation ledger and the Sent Items delta
observation before any retry decision; and that the summary's own saved sent
copy never becomes a loop, a summary-of-summary, or a re-summarization target
because a verified OpenLoops-generated marker excludes it from detection
without excluding any user-authored message. No canonical-recipient source or
marker mechanism is guessed ahead of G-SELFMAIL; both remain
`unresolved_pending_G-SELFMAIL`.

| Threat | Required control | Fail-closed result |
|---|---|---|
| Wrong-recipient, alias, guest, or personal-account resolution ambiguity | Canonical contract-tested recipient source proven per account type before G-SELFMAIL; no recipient sourced from user input, Cc, Bcc, reply-to, or a distribution list | An unproven or ambiguous recipient source blocks the send rather than guessing an address |
| Duplicate summary after a lost response | Durable pending operation-ledger entry committed before the send request; an ambiguous outcome reconciles against the ledger and the Sent Items observation before any retry; a lost response is never blindly retried | A crash or timeout produces at most one send, confirmed by reconciliation, never a silent duplicate |
| Recursion loop: the summary's own saved copy is detected as a loop or re-summarized | Verified OpenLoops-generated marker checked before any loop-creation or summarization path; suppression failure fails closed to no-send | An unmarked or unverifiably marked message is withheld rather than risk it entering detection; a marked message never recurses |
| Marker forgery by hostile inbound mail | Marker verification is treated as an authenticated check gated on a proven mechanism, not a plaintext content signal; a forged or unverifiable marker on inbound mail does not exempt that mail from ordinary detection | A hostile sender cannot spoof exclusion from detection by copying a marker string; only a message OpenLoops itself generated and can prove it generated is excluded |
| Consent creep (`Mail.Send` bundled into initial or another consent) | Separate explicit feature enablement and separate incremental `Mail.Send` consent required after G-SELFMAIL, matching ADR-003 | A combined or inferred consent grant never activates self-email |
| Summary content leak via stored copy, draft, template output, or diagnostics | No summary copy, draft, or template output stored under OL-SUM-003; content rules inherit the ADR-PRIV-001 privacy boundary, which prohibits persisting generated summaries or explanations | A stored summary, draft, or diagnostic field is a defect, not accepted residual state |
| Scheduled send while disconnected, unconsented, or unauthenticated | A scheduled send runs only while enabled, consented, and authenticated; missed schedules coalesce into the next eligible run with no catch-up burst | Disconnection, consent loss, or missing authentication stops the scheduled send before a request, and no burst of missed sends follows reconnection |
| Secure-store loss, rollback suspicion, or account mismatch during a pending send | Each condition independently causes zero send requests for the affected operation | A compromised or inconsistent local state never allows a send to proceed |

No `Mail.Send` scope is requested, no message is sent, and no marker mechanism
is selected by this threat model. A failed or unrun G-SELFMAIL/G-PRIV gate
keeps self-email disabled and routes to the product owner; the in-add-in
summary remains the approved default direction throughout.
