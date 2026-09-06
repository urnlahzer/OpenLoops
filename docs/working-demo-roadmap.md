# Implementation roadmap

## Delivery A: complete native preview (current work)

Deliver conversation-aware history scanning, useful evidence-backed expectation
cards, saved corrections, and reviewed personal To Do reminders. Acceptance is
the workflow in working-demo-spec.md, not merely successful HTTP requests.
Record implemented behavior and verification at the end of this document.

Native preview persistence exception: an OS-protected, versioned, bounded
credential holds a random digest key and keyed per-account source/action
fingerprints with decision, update time, and reminder attempt/outcome codes.
There is no readable source/model text, Graph identifier, email address, token,
or arbitrary extension field. This is a narrow explicit native-preview adapter
for the user-requested saved decisions, not activation of the production
persistence manifest or a relaxation of public-repository safeguards. Descriptive
Microsoft To Do artifacts are created only by the user's reviewed UI command.
Completed/dismissed decisions expire after 30 days. Unresolved reminder attempts
are retained until user reconciliation or explicit local reset.

### Implemented and checked

- Inbox/Sent Items conversation assembly, participant ownership facts, bounded
  30-day history, Group thread context, and visible coverage failures.
- Action-first cards with exact quotations, atomic source action phrases,
  waiting parties, stated deadlines, and possible later completion evidence.
- Track personal work, watch team work, handle, dismiss, and reopen; keyed
  decisions survive restart without storing readable source/model content.
- Reviewed personal To Do title/time creation, account binding, durable
  pre-dispatch attempt state, and explicit uncertain-outcome reconciliation.
- Verification: 99 inference tests, 104 Graph tests, 20 desktop tests (including
  two child-process credential checks invoked by their parent tests); native
  all-target/all-feature lint; native visual inspection; public-repository gate.
- The selected live Ollama model passed nine synthetic semantic cases: request,
  promise, irrelevant recap, third-party promise, team request, two actions,
  stale quote, later completion, and acknowledgement without completion.

The real mailbox needs a fresh scan in the updated app to evaluate usefulness.
Actual To Do consent, task creation, and notification delivery must be exercised
with a user-reviewed reminder; no test task was silently created in the account.
These are live acceptance checks, not claims established by synthetic tests.

### Remaining preview limits

The current projection holds 50 saved decisions. Descriptions are reconstructed
from the next scan; an older source outside the scan window cannot yet be
rehydrated on demand. A changed selected source action phrase may need new
review even if a human would associate it with the earlier loop. A content-free
exclusive Windows file handle serializes credential writes; stale windows fail
instead of overwriting newer state. Delivery B supplies the full transactional
lifecycle/store/association integration.

## Delivery B: continuously reconciled product

Wire folder delta checkpoints, secure token renewal, durable source locators,
source refetch beyond the history window, message moves/deletions, and the
existing full domain lifecycle to the hardened production store. Retain user
corrections across model changes and reassociate later evidence with established
atomic actions. Reconcile external To Do edits/completion/deletion and ambiguous
writes through the existing typed operation ledger and adapter contract.

Exit: restart/offline/recovery and cross-account tests pass; foreground and
background reconciliation use the same policy; expired content is not retained.

## Delivery C: Outlook experience and release

Outlook add-in, signed companion packaging, incremental onboarding, background
monitoring, timezone/deadline policy, secure update/uninstall, operational status,
and independently evaluated detection quality. Complete the existing security,
privacy, state, mail, reminder, and distribution gates. Enable hybrid automatic
reminders only after the separately specified held-out precision gate passes.

The preview must state these boundaries accurately. Neither synthetic provider
checks nor this roadmap certify the full product or production release gates.
