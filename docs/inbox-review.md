# Live expectation review

Open the native app and choose **Review inboxes**. Saved connections restore
automatically. This is a real Microsoft/Ollama flow; no synthetic cards appear
in the normal application.

1. **Scan inboxes** opens Microsoft sign-in, reads the visible 30-day history,
   and automatically analyzes conversations with the selected Ollama model.
   There is no individual message selection. Personal Inbox and Sent Items
   are joined by conversation identity, and a split identity is merged back
   in when the subject, an outside participant, and the timing all match.
   Groups keep their own thread identity and never merge.
2. Review each complete action, suggested owner, waiting party, and stated
   deadline. Under the stated deadline the card shows how it ages (due, past
   due, a date range, tied to an event, or soft urgency); past-due open items
   sort first. Expand evidence to inspect original quotations and later
   replies. Resolved requests are closed and hidden by default; the show
   toggle reveals them with the closing evidence. Every open request is
   rechecked against later replies you sent in the same thread before any
   cross-thread search. Closing evidence can also
   come from a reply you sent the same person in a different conversation;
   the card marks these "evidence in another conversation." Expectations tied
   to a passed event also close automatically; event times come from Graph
   meeting messages, calendar-invite subject lines, or a date or time stated
   in an email body. Requests in a meeting-metadata or subject event message,
   and event-worded requests elsewhere in a conversation containing exactly
   one such event, are tied to that scoped event without relying on a
   model-supplied event anchor; body-prose events are otherwise excluded. A
   request the analysis tied to a named event and its time closes once that
   event has passed; a request whose only time is its own deadline stays open
   when overdue. Both anchors let the request use its own message's event
   without event-shaped wording, admit body-prose event evidence when the time
   anchor points to that message, and supply the time when no event-index entry
   exists. A request with
   its own stated deadline later than the event end stays open. An event can
   only close a request if the event had not yet ended when the request's own
   message was sent, and a request found only in an event-bearing message
   closes with that event only when it is itself worded about the event (or
   carries a deadline tied to it) — so a meeting recap's unrelated action
   items are never closed just because the meeting is over. Emails from
   meeting-recap and call-transcription services (Fathom, Otter, Fireflies,
   Read.ai, tl;dv, Grain, Avoma, Gong, Chorus, and notetaker bots on Zoom,
   Teams, or Meet) are treated as records of a past meeting: their action
   items still become loops, but their meetings never close loops. Corrections that
   leave the action owed keep the card open with updated terms. If a quoted deadline or
   completion could not be validated against the mail, the card says so and
   keeps the item open.
3. **Track — this is mine** confirms personal responsibility. **Keep an eye on
   this** watches an unassigned team expectation without claiming ownership.
   **Handled**, **Not mine / dismiss**, **No longer relevant**, and **Reopen for
   review** save abstract decisions. Opening a reminder draft on a card still
   under review implies the same tracking as **Track — this is mine**;
   cancelling that draft restores the decision to what it was before the
   draft opened, unless you have changed the decision since (an action
   button, or a rescan closing the card by later evidence), in which case
   the draft simply closes and the changed decision stands. The checkbox
   **Show resolved, handled, dismissed, and
   no-longer-relevant items** reveals closed cards. Model summaries and
   source text are not saved locally.
4. **Set To Do reminder** is available on any open card and does not require
   tracking or watching it first; setting a reminder on a card still under
   review implies tracking it (a card already being watched stays watched).
   It previews an editable title and future local reminder time directly
   under the card that opened it. The explicit create button authorizes
   one task in the same Microsoft account's default personal Tasks list.
   Microsoft To Do owns notification delivery, including when OpenLoops closes.
   This requires delegated Tasks.ReadWrite and a separate browser sign-in.
5. **Rescan loaded mail** uses current in-memory mail without another Graph
   read. **Stop scan** stops dispatching and abandons the requests in flight. **Clear results
   and mail** removes memory content, preserving saved decisions and To Do tasks.

## Coverage and interpretation

Personal/shared Inbox and Sent Items each load up to 100 messages from the last
30 days: message headers are listed newest-first in pages of 100 and each body
is fetched separately, so one oversized message cannot fail the folder. Quoted
Outlook reply history inside a message is treated as context, not current text.
Groups load up to 20 recent threads, each with up to 40
posts; threads active within 30 days can include earlier posts as context.
There are at most 10 configured sources. Pagination is confined to the same
Graph origin and collection path. A capped, inaccessible, or oversized source
is visible as incomplete coverage. Conversations above 40 messages or the
provider payload bound fail visibly instead of silently losing context.

The model sees canonical message bodies, subjects, quoted history, participants,
timestamps and ownership facts. It never receives a token or raw Graph IDs.
Attachments are not sent. HTML scripts and remote images are never executed.
Outgoing user promises, incoming requests, and unassigned team work differ.
The model supplies whole evidence quotations plus an exact atomic action phrase;
the app resolves them against source blocks. It does not ask for character counts.
Original evidence cannot come from quoted history. Participant references must
exist, and possible resolution must be later than the original expectation.

Structural evidence checks do not prove semantic correctness. Suggestions need
review. Missing replies in these configured folders/window never prove that
work is unfinished. No reminders are created by scanning or inference alone.
Conversations are analyzed in parallel, up to the concurrency the selected
provider allows (the Ollama plan's slots, or the OpenRouter parallel ceiling on
the Connections tab), and the busy view shows how many are done and how many
are in flight; the merged result is identical to analyzing them one at a time.
Authentication, network, timeout, and HTTP server failures stop later requests.
Rate limiting and quota instead narrow the concurrency, failing only the
affected conversation. Other conversation failures remain visible while the
scan continues. There are no automatic request retries or silent model
substitutions.

After the per-conversation pass, a bounded closure pass checks each remaining
open request against up to 8 newest later same-thread replies before checking
still-open requests against later messages to the same person from other
conversations; across both stages it makes at most 40 model calls per scan,
and only the second stage shares conversations in one model request.

## Saved decisions and reminders

The native preview stores a bounded versioned Windows Credential Manager record
containing a random HMAC key, up to 50 keyed per-account source/action
fingerprints, decision enums, reminder-attempt enums, and timestamps. No names,
mail addresses, subjects, descriptions, source quotations, or Graph identifiers
are in that record. Descriptions are reconstructed after the next scan.
Different model summaries of the same exact source action phrase match the same
decision. Different action phrases remain independent. Semantic reassociation
when the model selects a different source phrase is a later roadmap item.

An attempt is saved before a task write. A missing/uncertain response blocks
another attempt until the user inspects To Do and explicitly reconciles it.
Signing in with a different account prevents creation. A local handled/dismissed
decision does not complete or delete a Microsoft task. All remote content is
limited to the reviewed task title/time and an opaque correlation reference.
No email is sent and no shared task list is modified.

Completed/dismissed records without reminders expire after 30 days when state
is updated. Reminder records remain available for user reconciliation. Hitting
the storage bound fails visibly and preserves existing records. A failed/corrupt
read or detected concurrent edit stops saving rather than overwriting data.

This is a foreground native preview. New scans require sign-in; there is no
unattended monitoring, Outlook add-in, or automatic external-task reconciliation.
See [spec](working-demo-spec.md), [plan](working-demo-plan.md), and
[roadmap](working-demo-roadmap.md) for the production path and remaining gates.

API references: [messages](https://learn.microsoft.com/en-us/graph/api/user-list-messages?view=graph-rest-1.0),
[Group posts](https://learn.microsoft.com/en-us/graph/api/conversationthread-list-posts?view=graph-rest-1.0),
[To Do creation](https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks?view=graph-rest-1.0),
[Ollama structured-output limits](https://docs.ollama.com/capabilities/structured-outputs).
