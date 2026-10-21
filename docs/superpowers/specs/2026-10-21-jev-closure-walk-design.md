# Jev closure walk: design

Status: owner-approved design, 2026-10-21. No implementation yet.

## Why

The Jev passes ask flat batches. Every question in a request sees the same
state, and no answer changes which question comes next. Code then combines
the probabilities.

The closure pass shows the cost. For each open loop and each later
paragraph, Jev answers five questions (`closure.fulfilled`,
`closure.withdrawn`, `closure.deadline_changed`, `closure.modified`,
`closure.outcome`), whether or not the paragraph is about the loop.
`closure.modified` scores at chance on synthetic data. `closure.outcome`
needs a probability of 1.0 to accept.

Past events also fail. `deadline_is_event_time` in
`crates/openloops-desktop/src/review_scan.rs` keeps a loop overdue when its
own deadline falls on the event day. That keeps "send the report before the
9/12 meeting" open, which is correct. It also keeps "prepare for the board
meeting on 9/12" open after the meeting, which is wrong. An ask with a
cutoff before a later event ("RSVP by Friday" for a gala the next week)
never closes at all.

This design replaces the flat closure batch with a walk. The walk follows
one loop forward through later messages in time order. Each answer picks
the next question and adds a fact to the next state, as in the guessing
game "twenty questions".

## Wider plan and where this fits

Every question serves the product question: who is waiting on the user,
for what, by when, and is it still owed? Code supplies every fact it knows:
author, the user's To/Cc role, later replies, attachments, and date and
event candidates. Jev answers one semantic fact per question. Questions
name roles by state path (`user.name`, `waiting_party`, `ask`). Requests
run in sequence only where an answer picks the next question; independent
questions go in one request.

The full chain has four stages. This spec covers Stage 4. The others get
their own specs, in this order (owner decision: build order A).

1. Sender kind and category. Code first, Jev for the residue. Automated
   senders that set a consequence (bills, renewals, expiring access) can
   create loops. Two categories: `billing_payable` (destination: A/P) and
   `billing_receivable` (destination: A/R). Destinations live in local
   settings, never in this repository.
2. Locate the ask paragraphs.
3. Describe each ask: who must act, who is waiting, kind, real or
   courtesy, conditional, and when.
4. **Closure walk (this spec).**

Forwarding to A/P or A/R is a separate later spec. It needs `Mail.Send`
consent beyond self-email (PD-13, ADR-013), and it is an action the user
presses, never an automatic one.

## Rules

### Due and expires

An ask has two times.

- **Due**: when the waiting party wants it. After this time the loop is
  overdue.
- **Expires**: the end of the event the ask serves. After this time the
  loop is expired. Most asks have no such event and never expire.

| Ask | Due | Expires | On 10/20 |
|---|---|---|---|
| RSVP by Friday 10/17 for the gala on 10/30 | 10/17 | 10/30 | overdue |
| Same ask, seen on 10/31 | 10/17 | 10/30 | expired |
| Prepare for the board meeting on 9/12 | 9/12 | 9/12 | expired |
| Pay the invoice by 9/12 | 9/12 | none | overdue |

Jev decides which event the ask serves and whether its purpose ends with
that event. Code compares the times.

### Who can close or change a loop automatically

A confident answer applies automatically when the evidence comes from the
waiting party, or when the ask expires. A confident answer based on the
user's own message or on a third party's message becomes a suggested update
with Accept and Reject. An answer in the gray band never applies; the loop
goes to the chat model.

## How the walk runs

Input: every open loop the user owes (`offered_loops` today), from either
extractor. Loops run in parallel. Steps within one loop run in order.

### Step 1: setup (one request per loop)

- `expiry.event`: choice. "Which event does `ask` help `waiting_party`
  prepare for or take part in?" Options: the event candidates code finds
  (event index and prose parsers), plus `none`. Jev leans to the first
  option, so code asks twice with the options in a different order and
  keeps the answer only when both agree.
- `expiry.ends_with_event`: noul, asked only when an event was chosen.
  "`ask` has no purpose after `event` ends." This drops asks that are about
  an event without serving it ("send the minutes from the meeting").

### Step 2: walk each later message (one request per message)

Later messages are those in the same thread after the loop's evidence
message, plus the user's later messages to the waiting party in other
threads (the existing cross-thread route). Order is by timestamp.

State: `user.name`, `waiting_party`, `ask` (with any amendment found so
far), `due`, `later.author`, `later.author_role` (`waiting_party`, `user`,
or `third_party`, from code), and `later.paragraphs[]` (current text only,
no quoted history, first 8 paragraphs).

Gate, in the same request:

- `walk.responds_paragraph`: choice over the paragraph indexes plus `none`.
  "Which paragraph of `later` responds to `ask`?"
- `walk.responds`: noul. "A paragraph of `later.paragraphs` responds to
  `ask`."

Branch questions, asked about every paragraph in the same request
(speculative fan-out). Code reads only the answers for the paragraph the
gate chose. The question set depends on `later.author_role`.

| Author | Question id | Statement intent | Effect |
|---|---|---|---|
| Waiting party | `walk.wp_received` | says they received what `ask` requests | closes automatically |
| Waiting party | `walk.wp_no_longer_needed` | says they no longer need `ask` | moot, closes automatically |
| Waiting party | `walk.wp_asks_again` | asks again for `ask` | "Asked again" |
| Waiting party | `walk.wp_changes_terms` | changes what `ask` requests | then `walk.wp_still_owed` |
| Waiting party | `walk.wp_still_owed` | after this paragraph, `user.name` still owes `waiting_party` something | yes: "Changed", `ask` amended; no: moot, closes automatically |
| Waiting party | `walk.wp_changes_due` | changes when `ask` is due | then `walk.new_date`; new due date applies |
| User | `walk.user_delivers` | gives `waiting_party` what `ask` requests | suggestion: Handled |
| User | `walk.user_promises` | says they will do `ask` later | then `walk.new_date`; suggestion: promised date |
| User | `walk.user_declines` | declines `ask` | suggestion: Declined |
| User | `walk.user_hands_off` | says someone else will do `ask` | then `walk.handoff_to`; suggestion: Delegated |
| Third party | `walk.third_party_done` | says they did `ask` or will do it | suggestion: Handled |

Follow-up choices need the chosen paragraph's candidates, so they go in a
second request, only when their parent noul accepts:

- `walk.new_date`: options are the date candidates the prose parser finds
  in the chosen paragraph, normalized to the `deadline_parse` grammar, plus
  `none`. `none` or a gray answer escalates the loop.
- `walk.handoff_to`: options are the conversation's participant handles
  (`waiting_party_display` text) plus `none`.

`walk.wp_still_owed` is a noul about the paragraph, so it goes in the same
request as the other branch questions. Code reads it only when
`walk.wp_changes_terms` accepts.

Reading the answers:

- `walk.responds` below its escalate threshold: no paragraph responds. Move
  to the next message with no change.
- `walk.responds` accepted and `walk.responds_paragraph` names a paragraph
  above its accept threshold: read that paragraph's branch answers.
- Any other gate result: escalate the loop.
- Terminal answers (`wp_received`, `wp_no_longer_needed`, a `wp_still_owed`
  no, `user_delivers`, `user_declines`, `user_hands_off`,
  `third_party_done`) take precedence over non-terminal ones. Two
  conflicting terminal answers for one paragraph escalate the loop.
- Non-terminal answers combine: "any update? Wednesday works better" is
  both Asked again and a new due date.
- A loop that closes stops walking.

### Step 3: time

After the last message, code compares now with `due` and with the chosen
event's end, using the dates as they stand after the walk. A rescheduled
due date or a rescheduled event (a newer invite) therefore counts.

- Event end passed and `expiry.ends_with_event` accepted: expired.
- Otherwise `due` passed: overdue, as today.

This replaces the `deadline_is_event_time` guard. The existing word-list
fast path (attend, bring, arrive) may stay as a free yes for
`expiry.ends_with_event`. Whether `rules.scoped_event`, `rules.event_match`
and the four-source matching in `close_passed_events` can be retired is a
question for the implementation plan, after the walk is measured.

### Gray band and errors

- A gray answer at the gate, at a branch question, or at a follow-up choice
  stops that loop's walk. The loop goes to the existing chat-model closure
  call, which keeps its 40-conversation cap.
- A rate-limited or failed request marks the loop as not checked, as today.
  No retries.
- Expiry answers in the gray band leave the loop overdue. No new card type.

### Scope

Only loops the user owes. Team loops (Watching) keep today's behaviour.

## What the card shows

| Outcome | Type | User sees |
|---|---|---|
| Waiting party confirms receipt | `resolution`, `ResolutionKind::Completed` | Hidden under Show resolved with the confirming paragraph; Still open — track it reopens |
| Waiting party withdraws, or the amended ask is no longer owed | `resolution`, `ResolutionKind::Withdrawn` | Same |
| Expired | `event_passed`, new label | "Expired with the gala on 10/30", hidden under Show resolved |
| Waiting party asks again | `Mention`, origin `Restated` | "Asked again" evidence card; stays open |
| Waiting party amends the terms | `Mention`, **new origin `Amended`** | "Changed" chip; amendment as an evidence card; later steps use the amended ask |
| Waiting party moves the date | `deadline` replaced; old date kept as a `Mention` | "Due date changed" chip; overdue grouping uses the new date |
| User's reply delivers it | `SuggestedUpdateKind::Closure` | Accept or Reject; Accept means Handled |
| User promises a date | `SuggestedUpdateKind::DeadlineChange` | "You said you'd send it by Monday": Accept or Reject |
| User declines | **new `SuggestedUpdateKind::Declined`** | Accept closes with `ResolutionKind::Declined` |
| User hands it off | **new `SuggestedUpdateKind::Delegated`** | Accept moves the card to Watching (Keep an eye on) |
| Third party did it | `SuggestedUpdateKind::Closure` | Accept or Reject |

No new buttons. Walk results are recomputed on every scan and nothing new
is persisted; saved decisions remain HMAC fingerprints and enums. Reject
clears a suggestion for the session, as today. Coverage details add one
content-free line: counts per outcome and escalated loops.

## Registry

New ids, in one contract PR with the hash re-pin:

- Gate: `walk.responds_paragraph` (choice, options per request),
  `walk.responds` (noul).
- Waiting party: `walk.wp_received`, `walk.wp_no_longer_needed`,
  `walk.wp_asks_again`, `walk.wp_changes_terms`, `walk.wp_still_owed`,
  `walk.wp_changes_due` (noul).
- User: `walk.user_delivers`, `walk.user_promises`, `walk.user_declines`,
  `walk.user_hands_off` (noul).
- Third party: `walk.third_party_done` (noul).
- Choices with options per request: `walk.new_date`, `walk.handoff_to`,
  `expiry.event`.
- Expiry: `expiry.ends_with_event` (noul).

Retired after the walk ships: `closure.fulfilled`, `closure.withdrawn`,
`closure.deadline_changed`, `closure.modified`, `closure.outcome`.

Bootstrap wording follows the harness proposer rules: one or two literal,
present-tense declarative sentences, at most 45 words, naming state fields
by path.

## Training data

The current corpus has single paragraphs. Walk questions need threads.

- The synthetic generator writes short invented threads: one ask plus one
  to three later messages, each thread with its true path through the walk.
  Rows for each question come from the paths, so a question trains only on
  threads that reach it. Paths also label the choice questions whose options
  are built per request.
- Required hard cases:
  - same date shape, different purpose: "RSVP by Friday" for a later
    event; "pay by 9/12"; "prepare for the 9/12 meeting";
  - a correction that leaves the action owed ("the fee is 350, not 359")
    against one that ends it ("forget the fee");
  - "thanks, will look"; partial delivery; a chaser after a delivery;
  - a later message in the thread about something else.
- The owner's Accept and Reject decisions on suggested updates
  (`closure.jsonl` in the training export) give real labels for the user
  branch.
- All names are invented; addresses use `example.invalid`.

## Metrics

- Each question: ReAnchor accept and escalate thresholds with
  `WRONG_DECISION_PENALTY = 9.0`, as today.
- The walk: the share of held-out threads that end in the correct status,
  and the share escalated. Errors compound along a path, so this number
  decides go or no-go.

## Testing

Rust, with `FixedDecisionClient`:

- each transition in the branch table;
- message order, and a closed loop that stops walking;
- an amended ask carried into later steps;
- a rescheduled due date and a rescheduled event;
- each row of the due and expires table;
- gray-band escalation at the gate, a branch and a follow-up choice;
- rate limit and failure handling;
- the cross-thread step;
- the option-order check on `expiry.event` (disagreement means no event).

Python harness: an offline end-to-end run against a fake endpoint with
realistic replies, one malformed reply and rejected rows. It passes before
any paid command.

## Cost gates

Every paid step starts with a smoke run. The owner sees its output before
the full run.

1. Generate about 10 threads. The owner checks label counts per question and
   a text sample per label, and checks for leakage between labels and
   metadata. Then the full corpus.
2. `optimize` one question on the light budget. Owner review. Then all
   questions.
3. Compare the walk with the current closure pass on the owner's export (Jev
   calls, cents). The walk replaces the current pass only if it agrees with
   more of the owner's Accept and Reject decisions.

## Delivery

| Order | Branch | Contents |
|---|---|---|
| 1 | `feat/jev-walk-harness` | Thread corpus generator, question specs, offline test |
| 2 | `chore/contract-walk-registry` | Bootstrap wording, hash re-pin (CONTRACT EDIT FOR OWNER REVIEW) |
| 3 | `feat/jev-closure-walk` | Rust walk, new types and chips, tests; behind the existing decision-model toggle, no new setting |
| 4 | tuned registry | After the paid runs in Cost gates |

## Amendment, 2026-10-21 (owner review of the plan)

The owner reviewed the plan's six adjustments. Adjustments 3 to 6 stand.
This section replaces adjustments 1 and 2.

### Gate per paragraph and per message

A later message has these units:

- each paragraph;
- the whole message, when it has two or more paragraphs. Its text is the
  paragraphs in order, joined by `"\n\n"`.

Each unit gets its own request with `walk.responds` and the author role's
branch nouls. The state names the unit's text `later.text`:

```json
{"user":{"name":…},"waiting_party":…,"ask":…,"due":…,"later":{"author_role":…,"text":…}}
```

The date follow-up state is `{"ask":…,"later":{"text":…}}`.

The reading rules apply to all units of the message:

- A unit responds when `walk.responds` accepts.
- No unit responds: escalate when a unit's gate is gray. Otherwise the
  message has no effect.
- Terminal answers from different units that disagree escalate the loop.
- Non-terminal effects keep one effect per kind. Paragraphs come before the
  whole message, so evidence cites a paragraph when a paragraph carries the
  change.
- An effect from the whole-message unit cites the message's first
  paragraph. Its date candidates are the candidates of all its paragraphs,
  without duplicates.

The whole-message unit finds a response that no single paragraph carries.
It adds one request per later message that has two or more paragraphs.

### `walk.handoff_to`

When `walk.user_hands_off` accepts in a unit, a follow-up request asks
`walk.handoff_to` with that unit's walk state. The options are the later
message's participants (From, To, Cc) without the user's own addresses and
without the waiting party. Each option has a handle (`p0`, `p1`, …) and the
participant's display name, or the address local part when there is no
display name. `none` is the last option.

Code marks the chosen person:

- **internal** when the address domain is the domain of one of the user's
  own account addresses and the domain is not a public mail domain;
- **external** in all other cases.

The Delegated suggestion names the person and the mark: "Handed to Amara
Okafor (internal)". `none` or a gray answer gives "Handed to someone not on
the message". Accept moves the card to Watching in all three cases, and the
Watching card names the person.

`walk.handoff_to` has options per request, so it keeps bootstrap wording
(adjustment 3). The thread corpus records the true person and mark.
