# Working OpenLoops preview

This delivery implements the first complete user workflow from the product spec,
sections 1, 5, and 6. It replaces the live claim-fragment viewer. The existing
product spec remains the full product target; this document fixes the acceptance
boundary for the native preview.

## User outcome

After connecting Microsoft and choosing any available Ollama Cloud model, the
user scans their configured mail sources without choosing individual messages.
Each result explains an independently actionable expectation: what to do, who
is waiting, why it belongs to the user or their team, any stated deadline, and
later evidence that it may already be handled. Evidence is readable and directly
inspectable. Merely mentioning an activity is not a commitment.

## Required behavior

- Scan a visible 30-day window of personal Inbox and Sent Items, configured
  shared mailbox Inbox/Sent Items, and configured Microsoft 365 Group threads.
  Preserve participants, timestamps, source identity, and conversation identity.
  Enforce disclosed resource limits and show incomplete coverage explicitly.
- Analyze conversations in time order. Never split unrelated messages into an
  arbitrary batch and assume the batch is a conversation. Missing replies or
  coverage mean uncertainty, never proof that an action was not performed.
- Resolve the signed-in identity from Microsoft. Incoming requests directly to
  the user, outgoing user promises, and unassigned team requests are distinct.
  A Group membership does not establish personal responsibility. Other people's
  promises, newsletters, biographies, signatures, and quoted stale requests are
  not personal obligations.
- Model output contains a concise action, ownership, a supplied participant
  reference, exact evidence quotations, deadline evidence, and optional later
  completion evidence. Resolve quotations locally; never ask a model to count
  character positions. Reject fabricated or ambiguous evidence matches.
- All results are reviewable hypotheses. Show clear ownership, quoted evidence,
  missing deadlines, possible completion, and source coverage. User decisions
  override repeated inference; completion is never inferred as final.
- Save abstract decisions and reminder state across launches in Windows
  Credential Manager. Keep mail, names, subjects, model summaries, and evidence
  text in memory only. Reconstruct descriptive results on the next scan.
- Support confirming ownership, dismissing/not-mine, completion outside email,
  reopening, and scheduling a personal Microsoft To Do reminder from a reviewed
  action. Preview its exact title and time before the user creates it. Request
  delegated Tasks.ReadWrite only for this action. Never send email or mutate
  shared tasks. A failed or uncertain write cannot trigger an automatic retry.
- Offer “Keep an eye on this” for team requests without assigning their work
  to the user. A reviewed personal reminder can be a follow-up to that team task.

## Acceptance examples

Use synthetic conversations for automated tests; never copy a user's mail into
fixtures. Required cases: direct request; outgoing promise; team request with
unknown owner; third-party promise; irrelevant recap; request followed by sent
fulfilment; acknowledgement that does not fulfil; two independent actions;
quoted-history repetition; invented quotation; Unicode evidence; unknown
participant; wrong-account reminder; partial history; cancelled scan; duplicate
scan after dismissal; restart after decision; uncertain reminder write.

Transport/JSON test success is not detection-quality success. Report both.
Live model evaluation uses the selected provider with synthetic conversations;
real mailbox usefulness is evaluated by the user in the running app.

## Scope honesty

The preview is a native per-user app, not yet an Outlook add-in or unattended
Windows service. Microsoft To Do owns scheduled alerts after creation. A scan
describes its window and caps; it must not claim exhaustive mailbox coverage or
automatic closure. Production hybrid reminders still require the existing
automation and privacy release gates.
