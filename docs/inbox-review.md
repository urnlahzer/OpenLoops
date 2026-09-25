# Live loop review

## Screen layout

The nav rail on the left switches between **Review** and **Sources**; its
Review icon carries a badge of how many open loops are `Review`, `Mine`, or
`Watching` and not yet auto-resolved, hidden at zero. The Review screen's
command bar holds **Scan inboxes** (becomes **Stop scan** while a scan runs),
**Rescan loaded mail**, **Clear results and mail**, an **All / Mine / Team**
filter, and the **Show resolved, handled, and dismissed** checkbox. Directly
under it, the scan strip shows one of three states: scanning (phase, "Conversation
i of n", "m / total messages", elapsed time, and a progress bar), finished
(a summary line, and expanding "Coverage details" into the incomplete-source
notices), or idle (the scan-scope disclosure). The list pane groups open
rows as Past due, Due, and No fixed deadline (dot colour and order match
`ListGroup`/`DeadlineView`), with a fourth Resolved/handled/dismissed group
appearing only when the checkbox above is on.

The reading pane, top to bottom, shows: status/aging/reminder pills; the
action-phrase title; a meta grid (Responsible, Waiting on this, Deadline
stated in email, Source); an uncertainty callout when the model reported one;
an **Open email** button for the first evidence message when Outlook provides
an allowed link, plus **Open conversation**, which opens the newest message in
the thread so Outlook on the web shows the chain; the decision buttons for the
selected card; the action-status line; an inline
reminder-draft panel when one is open (task title, remind-at time, quick
picks, the scheduled-instant line, and the disclosure copy below); the
reminder's own state (created or attempted, with reconcile buttons in the
attempted case); one evidence card per anchor; the possible-later-completion
card or note; and the collapsed "Full scanned conversation" disclosure with
one row per message.

Loops extracted from recognized meeting-recap and call-summary mail carry a
**Call summary** tag and are hidden by default. Turn on **Show loops from call
summaries** to include them; their reading-pane metadata ends with a **Meeting
time** cell, marked approximate when only the message timestamp was available.

Open the native app and select **Review**. Saved connections restore
automatically. This is a real Microsoft/Ollama flow; no synthetic cards appear
in the normal application.

1. **Scan inboxes** reuses the current Microsoft session when available (and
   otherwise opens sign-in), reads the visible 30-day history,
   and automatically analyzes conversations with the selected Ollama model.
   When mail and a scan result are already loaded, the same button instead
   checks for new mail: it downloads only unknown messages, analyzes only
   conversations that gained mail, preserves every other result and decision,
   and asks whether new mail closes or changes loops already open.
   There is no individual message selection. Personal Inbox and Sent Items
   are joined by conversation identity, and a split identity is merged back
   in when the subject, an outside participant, and the timing all match.
   Groups keep their own thread identity and never merge.
2. Review each complete action, suggested owner, waiting party, and stated
   deadline. Under the stated deadline the card shows how it ages (due, past
   due, a date range, tied to an event, or soft urgency); past-due open items
   sort first. Expand evidence to inspect original quotations and later
   replies. Resolved requests are closed and hidden by default; the show
   toggle reveals them with the closing evidence. Later messages that look
   like they close an open request, change its deadline, or modify it appear
   on the card as a **Suggested update** with Accept and Reject. Accepting a
   closure takes the Handled path; accepting a deadline change replaces the
   deadline for the current session; rejecting clears the suggestion for the
   current session and is not remembered across rescans. A modification can
   only be dismissed. No suggestion is applied without your confirmation.
   Loops tied to a passed event also close automatically; event times come from Graph
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
   items still become loops, but their meetings never close loops. Recap action items
   default to **Responsible: You (suggested)** when the reported waiting party is the
   owner or recap service, unless the action names a counterparty. Corrections that
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
   the draft simply closes and the changed decision stands. Every closed card
   -- terminal (Handled/Dismissed/No longer relevant) or auto-resolved alike
   -- shows both **Reopen for review** and **Still open — track it**,
   widening the old egui build's terminal-only "Reopen"/auto-resolved-only
   "Still open" split to match spec §4.5 and the Companion; Enter on a
   selected closed card runs **Reopen for review**. The checkbox
   **Show resolved, handled, and dismissed** reveals closed cards. Model
   summaries and source text are not saved locally.
4. **Set To Do reminder** is available on any open card and does not require
   tracking or watching it first; setting a reminder on a card still under
   review implies tracking it (a card already being watched stays watched).
   It previews an editable title and future local reminder time directly
   under the card that opened it. The explicit create button authorizes
   one task in the same Microsoft account's default personal Tasks list.
   Microsoft To Do owns notification delivery, including when OpenLoops closes.
   This first requires delegated Tasks.ReadWrite; the browser opens only when
   the current session does not already cover that incremental scope.
5. **Scan inboxes** with loaded mail checks for new mail while retaining all
   messages loaded earlier in the session. For a full reload, choose **Clear
   results and mail** and then **Scan inboxes**. **Rescan loaded mail** uses
   current in-memory mail without another Graph read. **Stop scan** stops
   dispatching and abandons the requests in flight. **Clear results and mail**
   removes memory content, preserving saved decisions and To Do tasks.

## Coverage and interpretation

Personal/shared Inbox and Sent Items each load up to 100 messages from the last
30 days: message headers are listed newest-first in pages of 100, then bodies
are fetched four at a time per folder and placed back in listing order, so one
oversized message cannot fail the folder. The download shows a running message
count and can be stopped before the model scan begins. Bodies already downloaded
in this session are reused without another Graph read until Clear results,
Forget, or a client-ID change clears them, or until the app exits. This cache is
in memory only. Quoted Outlook reply history inside a message is treated as
context, not current text.
Groups load up to 20 recent threads, each with up to 40
posts; threads active within 30 days can include earlier posts as context.
There are at most 10 configured sources. Pagination is confined to the same
Graph origin and collection path. A capped, inaccessible, or oversized source
is visible as incomplete coverage. Conversations above 40 messages or the
provider payload bound fail visibly instead of silently losing context.
Each per-source coverage note is cumulative for the session and adds
`; N new` when that source already had loaded messages before the check.
Read-only Graph listings and body fetches retry once after two seconds for a
timeout, interrupted connection, HTTP 503, or HTTP 504 (never HTTP 429), and a
body failure skips only that message while reporting the source's failed count.
When a provider error (quota, rate limiting, an unreachable or unauthorized
provider) stops a scan early, the summary and the "Scan coverage and errors"
panel count every affected conversation -- analyzed, failed, or never
dispatched -- rather than counting failure lines, so conversations queued
behind a single repeated error still show up in the totals instead of
silently disappearing from the count.

The model sees canonical message bodies, subjects, quoted history, participants,
timestamps and ownership facts. It never receives a token or raw Graph IDs.
Attachments are not sent. HTML scripts and remote images are never executed.
Current-body evidence is paragraph-sized for HTML and plain-text mail alike.
Outgoing user promises, incoming requests, and unassigned team work differ.
The model supplies governed claims with whole-block evidence ranges; the app
resolves the cited source text locally and derives card titles and stable action
phrases from it. Original evidence cannot come from quoted history. Participant
and loop references must exist, temporal values must pass the deterministic
parser, and suggested updates remain review-only.

Structural evidence checks do not prove semantic correctness. Suggestions need
review. Missing replies in these configured folders/window never prove that
work is unfinished. No reminders are created by scanning or inference alone.
Conversations are analyzed in parallel, up to the concurrency the selected
provider allows (the Ollama plan's slots, or the OpenRouter parallel ceiling on
the Sources screen), and the busy view shows how many are done and how many
are in flight; the merged result is identical to analyzing them one at a time.
Authentication, network, timeout, and HTTP server failures stop later requests.
Rate limiting and quota instead narrow the concurrency, failing only the
affected conversation. Other conversation failures remain visible while the
scan continues. There are no automatic request retries or silent model
substitutions.

With the `OpenRouter` decision-model setting enabled, a triage pass runs before
the per-conversation analysis. `Jev` receives one body paragraph per request
(up to 40 per message) and decides whether it contains a request, commitment,
question, time reference, boilerplate, or an automated notification. Threads
with no accepted or gray-band obligation signal, and notification-only threads,
skip the chat-model pass. Accepted boilerplate is omitted from conversations
that continue, while surviving body blocks retain their original ordinals.
Coverage details report content-free triage counts and identify skipped threads
as having no obligations found by triage; **Rescan** with the setting off sends
those threads through the unchanged chat-model path.

With **Find loops with** additionally set to **Decision model** (a control
shown under the decision-model toggle, enabled only while that toggle is
on), the triage pass above is replaced: one combined per-paragraph request
carries the triage questions plus a claim-type choice, a waiting-party
choice over the conversation's own participants, and (when the paragraph
names a date) a temporal choice, and a confident, non-gray-band answer is
assembled into a card through the same review path a chat-model card takes.
A paragraph whose claim type lands in the gray band sends its whole
conversation to the chat-model pass instead (the owner-set gate: flip this
switch only after the compare-decisions probe shows at least 90% claim-type
agreement on 200 or more paragraphs of your own mail). It is off (chat
model) by default.

The same setting also lets `Jev` answer only the residue left by the existing
deterministic recap, event-scoping, event-name, duplicate-action, thread-merge,
and deadline-kind rules. Deterministic matches still short-circuit. Distinct
rule inputs are asked at most once per scan, failures and uncertain answers
keep the prior deterministic result, and deadline answers are computed during
the scan rather than from the UI thread. Coverage details add one content-free
line with answered and skipped rule-question counts; turning the setting off
keeps the previous rule behavior and creates no decision client.

After the per-conversation pass, the closure pass checks later messages that can
reach open requests: a later message in the same thread, or a conversation
reached through a later message you sent to the waiting party. With the
`OpenRouter` decision-model setting enabled, `Jev` checks one later paragraph
per request (the first 8 body paragraphs per message). Accepted answers become
pending suggested updates. Gray-band loop/message pairs are grouped by
conversation and sent to the existing governed chat-model closure call, capped
at 40 conversations per scan. Coverage notes report content-free counts for
decision-model suggestions, escalated pairs, and skipped pairs. With the
setting off, the existing chat-model closure pass runs unchanged. Requests
found earlier in the same scan are the only ones offered, because no mail text
is stored between scans.

## Saved decisions and reminders

The native preview stores a bounded versioned Windows Credential Manager record
containing a random HMAC key, up to 55 keyed per-account source/action
fingerprints, decision enums, reminder-attempt enums, and timestamps. No names,
mail addresses, subjects, descriptions, source quotations, or Graph identifiers
are in that record. Descriptions are reconstructed after the next scan.
Different model wording over the same normalized source evidence matches the
same decision. Distinct source evidence remains independent. Semantic
reassociation when the model selects a different source range is a later
roadmap item.

An attempt is saved before a task write. A missing/uncertain response blocks
another attempt until the user inspects To Do and explicitly reconciles it.
Signing in with a different account prevents creation. A local handled/dismissed
decision does not complete or delete a Microsoft task. All remote content is
limited to the reviewed task title/time and an opaque correlation reference.
No email is sent and no shared task list is modified.

Completed/dismissed records without reminders expire after 180 days when state
is updated. Reminder records remain available for user reconciliation. Hitting
the storage bound fails visibly and preserves existing records. A failed/corrupt
read or detected concurrent edit stops saving rather than overwriting data.

Owner-taught links use a second generic credential,
`OpenLoops/Relations/v1`, containing at most 34 canonical pairs of HMAC
fingerprints, link-kind tags, and timestamps. It contains no readable mail,
names, addresses, subjects, or quotations. Resetting the decisions credential
also replaces the HMAC key, so any saved relation fingerprints become orphaned
and no longer match cards from later scans.

This is a foreground native preview. Scans require a current in-process Microsoft
session; there is no
unattended monitoring, Outlook add-in, or automatic external-task reconciliation.
See [spec](working-demo-spec.md), [plan](working-demo-plan.md), and
[roadmap](working-demo-roadmap.md) for the production path and remaining gates.

API references: [messages](https://learn.microsoft.com/en-us/graph/api/user-list-messages?view=graph-rest-1.0),
[Group posts](https://learn.microsoft.com/en-us/graph/api/conversationthread-list-posts?view=graph-rest-1.0),
[To Do creation](https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks?view=graph-rest-1.0),
[Ollama structured-output limits](https://docs.ollama.com/capabilities/structured-outputs).

# Linking loops and conversations

**Same loop as…** and **Same conversation as…** start a two-step selection:
choose the action, then select the other card. Escape cancels. **Not the same**
records the opposite relationship for every current link involving the selected
card. Same-loop links apply immediately by folding the newer card into the older
card and showing the later evidence as **Asked again**. Conversation links are
saved now and take effect during the next scan once rule consumption is enabled
in PR 2. Only HMAC fingerprints, link-kind tags, and timestamps are stored.

### Training data export (deliberate exception)

The Sources screen's OpenRouter card has an owner-invoked **Export training
data** control: with a scan result loaded and a folder path typed into
**Training export folder**, it writes `triage.jsonl`, `closure.jsonl`, and
`rules.jsonl` --
real mail text (subjects, paragraphs, participants) from the currently
loaded scan, plus the owner's own accept/reject decisions on suggested
updates -- for the `Jev` question-optimization harness in
`tools/jev-optimize/` (see that tool's README). This is the one deliberate
exception to the no-persistence rule described above: it writes real content
to disk only when the owner explicitly asks, only to a folder the owner
names outside this repository, and the button requires two presses (the
first only arms a warning). The folder path itself lives in memory only for
the running session and is never saved to Windows Credential Manager or any
other settings record; nothing about the export is logged.

`rules.jsonl` contains two gold owner-label shapes. Loop rows carry
`action_a`, `action_b`, and `label.rules.duplicate_action`. Conversation rows
carry `subject_a`, `subject_b`, `first_paragraph_a`, `first_paragraph_b`, and
`label.rules.thread_merge`. Both also carry `id`, `label_source`, `set`, and
`source`.

# Retry failed conversations and sources

After a Review scan, **Retry failed (N)** reloads only sources whose listing failed and re-analyzes only conversations that timed out, hit a transport or retryable provider failure, panicked, or were not started; newly loaded conversations from those sources are included automatically. It never resends rate-limited or quota-blocked conversations, and decisions on untouched conversations—as well as decisions whose retried item keeps the same fingerprint—are preserved.

# Checking for new mail

An in-session check builds its changed set from conversations containing an
appended message plus the post-merge conversations of existing messages whose
thread identity changed. The latter matters when a new message bridges two
previously separate threads: the absorbed conversation must be rescanned so
stale items are replaced.

The primary pass analyzes only that changed set. Its closure pass can also
offer old open loops from untouched conversations, but pairs those old loops
only with messages appended by this check. Handled, dismissed, resolved, and
event-passed loops are not re-offered. Results and decisions for every other
conversation remain in memory unchanged.

Stopping during mail loading appends nothing. Stopping during analysis merges
the conversations that completed before cancellation. Known limitations are
that old-versus-new cross-thread duplicate loops are not collapsed until a full
scan, folders remain capped at their newest 100 rows, and messages that age out
of the rolling 30-day window remain loaded until **Clear results and mail** or
the app exits.
