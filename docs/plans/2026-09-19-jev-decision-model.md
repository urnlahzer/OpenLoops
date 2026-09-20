# Jev decision model: design and phased plan

Status: design, owner-approved scope 2026-09-19. Implementation follows in
one branch per phase.

## Why

The scan makes two kinds of decision today.

1. One large model call per conversation (`analysis::analyze_claims`) finds
   the requests and promises, picks the evidence paragraph, names the waiting
   party, normalizes the deadline, and rates its own confidence. A second call
   per reachable conversation looks for closures. Each call takes 150-300 s
   on a reasoning model, times out on long threads, and costs the most of
   anything the app does.
2. Hand-written rules answer narrow yes/no questions: "is this a meeting
   recap?" (a vendor-domain list plus subject words), "is this request about
   attending a gathering?" (five verbs), "are these two threads one?"
   (subject, shared address, three days), "are these two cards duplicates?"
   (string equality). Each list is patched as real mail finds its gaps.

Jev (TypeSafe, on OpenRouter as `typesafe/jev-1.13`) answers typed questions
about supplied text in 70-500 ms for about $0.04 per million input tokens,
output free. It returns a probability for a yes/no statement (`noul`), a
choice from a list with per-option probabilities and a confidence, or a
score on ordered levels. Every question in a request is evaluated in
parallel, so asking thirty questions costs the same time as one. It cannot
produce text or spans, cannot count, reads dates as text, and reads
instructions literally.

That shape fits decision kind 2 exactly and fits the closure check, whose
inputs are already whole paragraphs. It may also fit decision kind 1, because
the governed pipeline already cites whole paragraph blocks as evidence, so a
claim is (block, type, waiting-party handle, normalized temporal value) and
every field is a choice from a set the code can enumerate. That last step is
unproven and is gated on measurement.

## Owner decisions

- Scope: Jev replaces the rules and the closure check first, with a measuring
  mode that runs Jev-based finding beside the LLM and reports agreement. The
  extraction switch happens only when the numbers justify it.
- Uncertain answers (probability or confidence in the gray band) are escalated
  to the LLM for that item, not shown as uncertain and not dropped.

## Transport and privacy

- OpenRouter only. `POST https://openrouter.ai/api/alpha/decisions`, body
  `{model, state, questions}`, response `{model, answers, usage}`. The
  chat/completions endpoint rejects Jev. `typesafe/jev-1.13` is in
  OpenRouter's zero-data-retention listing (status 0), which the app already
  reads at connect time. Direct TypeSafe (`api.typesafe.ai`) is a new origin
  with ZDR only for enterprise accounts, so it is not used.
- Same origin, same key, same ZDR gate as the chat model. The request carries
  `provider: {"zdr": true}` if the endpoint accepts it; the P0 probe settles
  that. If it is rejected, the model is validated against the ZDR listing
  before any content is sent, as the chat model is today, and the request is
  sent without the field. If neither holds the feature stays disabled.
- Limits: 32k-token state, 64k total per request, 255 options per choice,
  1,200 requests per minute (dynamic). Rate-limited requests are never resent;
  the conversation is reported as not checked, as today.
- The contract (`contracts/model/provider-boundary.json`) treats a second
  model call as a transmitted-field change that invalidates consent. P0 adds
  the decision model as a second call under the `openrouter` profile, with
  the same allowed components and prohibitions, a new disclosure line on the
  Sources screen, and matching checker updates. Contract and checker edits go
  in their own PR for owner review, as with #33.
- Nothing about a Jev answer is persisted. Probabilities live in memory for
  the scan and appear only as pills or routing decisions.

## Architecture

New crate module `crates/openloops-inference/src/decision.rs`:

- `trait DecisionClient: Sync { fn decide(&self, state: &Value, questions:
  &Questions, cancel, deadline) -> Result<Answers, ProviderError> }`. It sits
  beside `ModelClient`, which stays string-in string-out. `RequestControl`,
  `https_client`, `ProviderError` and the byte caps are reused. Deadline for
  a decision request is 20 s, separate from `deadline_for`.
- `Questions` is a typed builder: `noul(id, statement)`, `choice(id,
  instructions, options)`, `score(id, instructions, levels)`. Option keys are
  application-issued handles only, checked with `valid_handle`.
- `Answers` parses the response strictly: unknown fields reject, every
  requested id must be answered with its requested type, probabilities are
  in `[0,1]`, choice values must be issued keys. Anything else is
  `ProviderError::InvalidResponse`.
- `OpenRouterDecisions` implements it for `/api/alpha/decisions`.
- A `FixedDecisionClient` test double returns canned answers, as
  `FixedAnalysisClient` does for the chat path.

State discipline, from the Jev known-issues page: send only the text the
question is about (accuracy falls with irrelevant state), phrase statements
literally and positively (no double negatives), enumerate every value the
code needs back as an option (no numbers, no dates as free text), and keep
arithmetic and ordering in code.

Thresholds are named constants in one table, `decision::thresholds`, and
are tuned from the measuring mode, not from the docs' examples. Initial
values: accept above 0.70, escalate 0.30-0.70, reject below 0.30, per the
consistency cookbook's band.

## Where Jev is used

Each item names the function it sits behind. Signatures do not change;
callers do not know which backend answered.

### P1. Closure and suggested updates (`scan_closures`)

Today: one chat-model call per reachable conversation, at most 8 loop
handles per call and 40 calls per scan.

New: for each open, you-owed loop and each later paragraph in a reachable
message (same thread, or a later user-sent message to the waiting party,
scoping unchanged), one state `{obligation: {title, evidence_text},
later: {paragraph_text, from_user, days_later}}` and four nouls:

- `fulfilled`: "The later text shows the obligation in `obligation` has been
  carried out."
- `withdrawn`: "The later text cancels or withdraws the obligation."
- `deadline_changed`: "The later text sets a different deadline for the
  obligation."
- `modified`: "The later text changes what the obligation requires."

Paragraphs are batched: one request per (loop, message) with the message's
paragraphs as an array in state and the four questions asked per paragraph
(question ids carry the paragraph ordinal). Pairs are evaluated concurrently
under the existing parallel limiter.

Recombination in code: highest probability above the accept threshold wins,
closure preferred on ties, one `SuggestedUpdate` per loop. `deadline_changed`
needs a temporal value the schema can carry; the code enumerates date
candidates from the paragraph with the existing prose parsers and asks a
`choice` over them plus `none`; no candidate means no deadline-change
suggestion. Gray-band pairs go to the chat model exactly as the closure pass
does today, restricted to those pairs. Every suggestion still needs review;
`validation::route` is unchanged.

Result: no handle cap, no 40-call cap, and the same-thread and cross-thread
routes both become exhaustive.

### P2. Triage before the primary pass

Per message, one request with the message's paragraphs as array state and,
per paragraph, nouls: `asks_recipient`, `commits_sender`, `asks_question`,
`names_time`, `boilerplate` (signature, legal footer, unsubscribe,
disclaimer), `automated_notification`.

Uses, all in code:

- A conversation with no paragraph above the accept threshold on
  `asks_recipient`, `commits_sender` or `asks_question` skips the primary
  pass. The coverage note counts skipped conversations.
- Paragraphs above threshold on `boilerplate` are dropped from the primary
  projection. Ordinals are preserved as with quote trimming.
- Conversations that were skipped show a "no obligations found" note on
  Rescan, so an owner can see why nothing surfaced.

This is the second lever on timeouts after quote trimming, and it removes
signature and footer text from every card title.

### P3. Rule replacements

Each keeps its current fast path and asks Jev only when the fast path is
silent or ambiguous. Each question is answered once per input and cached in
memory for the scan.

| Function | Question |
|---|---|
| `is_meeting_recap_artifact` | noul over `{sender, subject, first_paragraph}`: "This email is an automatically generated meeting summary, recap, or transcript." Vendor list stays as the fast yes. |
| `has_scoped_event_language` | noul over `{request_text}`: "This request is about attending, preparing for, or bringing something to a meeting or event." |
| `match_event` / `match_event_by_text` | noul over `{phrase, event_name}`: "The phrase refers to the named event." Token overlap stays as the fast yes; Jev decides the residue. |
| `push_unique_expectation` and validation step 9 | noul over `{action_a, action_b}` for pairs in one conversation: "These two sentences ask for the same thing." Exact match stays as the fast yes. |
| `should_merge` (thread merge) | noul over `{subject_a, subject_b, first_paragraph_a, first_paragraph_b}` only for pairs that pass every rule except subject strength or the shared-mailbox test. |
| `correct_recap_attribution` | Removed once P3's recap noul and P4's waiting-party choice exist; until then unchanged. |
| `deadline_view::classify` Unknown fallback | choice over `{event_tied, soft, unknown}` for a phrase `reparse` rejects. |

### P4. Measuring mode (probe) for Jev-native extraction

`--probe-saved-model` gains a `--compare-decisions` flag. On loaded mail it
runs both extractors and prints content-free counts: conversations,
paragraphs, claims per type from each side, per-block agreement on
(has claim, claim type, waiting-party handle, temporal candidate), gray-band
rate, wall time and token usage per side. Nothing is written to disk.

The Jev-native extractor, per message: state is `{subject, paragraphs[],
participants[]}` with handles the code issued; per paragraph, `claim_type`
choice over the eight types plus `none`; `waiting_party` choice over the
participant handles plus `none`; `temporal` choice over candidates the
existing prose parsers found in that paragraph, already normalized into the
grammar `deadline_parse::reparse` accepts, plus `none`; one noul per
ambiguity code. Code assembles a `Claim` with the paragraph as whole-block
evidence and runs it through `validation::validate` unchanged.

### P5. Extraction switch, gated

Flip the primary pass to the Jev-native extractor when P4 shows at least 90%
agreement on (has claim, claim type) and 85% on waiting party across 200 or
more paragraphs of the owner's real mail, with the chat model kept for
gray-band paragraphs (owner decision). The chat model remains selectable as
the extractor on the Sources screen. Thresholds move to the settings store
only if the owner asks.

## What does not change

- `validation::validate`, the analysis-output schema, whole-block evidence,
  `deadline_parse`, fingerprints and saved decisions, review-only routing,
  the Slint UI except one disclosure line and one settings toggle.
- No Jev output is ever authority for a mutation. Accept and Reject on the
  card are unchanged.

## Testing

- `decision.rs`: request builder byte-exact tests, strict response parsing
  (unknown field, missing id, wrong type, out-of-range probability, unissued
  option each reject), deadline and cancel behaviour through
  `RequestControl`.
- Each P3 function: fixture tests with `FixedDecisionClient` showing the fast
  path still short-circuits and the Jev answer decides the residue.
- P1: fixtures for fulfilled, withdrawn, deadline-changed with and without a
  date candidate, gray-band escalation, rate-limit handling, per-loop
  winner selection.
- P2: fixtures for skip, boilerplate drop with preserved ordinals, note text.
- P4: the comparison report on a synthetic corpus is deterministic and
  content-free.
- Live: a rescan of the owner's mail after P1 finds the sent invitation and
  reply closures the LLM pass found, in less wall time; after P2 the two
  conversations that timed out complete.

## Phases and branches

| Phase | Branch | Contents |
|---|---|---|
| P0 | `feat/jev-transport` | `decision.rs`, `OpenRouterDecisions`, settings toggle (off by default), Sources disclosure line, content-free "Check decision model" probe that sends a synthetic state, docs. Contract and checker edits in a sibling `chore/contract-decision-model` PR. |
| P1 | `feat/jev-closure-pass` | Replace `scan_closures` internals. |
| P2 | `feat/jev-triage` | Triage request and projection trimming. |
| P3 | `feat/jev-rule-residue` | The seven rule replacements, one commit each. |
| P4 | `feat/jev-compare-probe` | Measuring mode. |
| P5 | `feat/jev-extraction` | The switch, after P4 numbers. |

P1 and P2 are independent after P0. P3 items are independent of each other.
P5 waits on P4.

## Open items the P0 probe must answer

- Whether `/api/alpha/decisions` accepts `provider.zdr`.
- The practical cap on questions per request (the docs state none).
- Whether 20 s is enough headroom for a 40-paragraph request under load.
- The contract's `maximum_context_messages: 4` against
  `MAX_CONVERSATION_MESSAGES = 40` in code is a pre-existing divergence to
  reconcile in the contract PR.

## Sources

- TypeSafe docs: API (`/api.md`), models, state, primitives, confidence,
  patterns (fan-out, confidence routing, composite scoring), cookbooks
  (citation check, date extraction, pre-parsed value extraction, parallel
  questions, self-consistency, re-ranking, RAG passage classification),
  and the Jev 1.13 known-issues page.
- OpenRouter: Typesafe listing, the ZDR endpoint listing, and community
  integrations documenting the `/api/alpha/decisions` contract.
