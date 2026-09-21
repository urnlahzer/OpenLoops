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
- Question wording is tuned with DSPy (GEPA) on a synthetic corpus, a public
  corpus, and an opt-in export of the owner's mail; see "Question
  optimization with DSPy".

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

## Question registry

The wording of every Jev question is data, not code.
`contracts/model/decision-questions.json` holds one entry per question id:
type, instructions, criteria, the accept and escalate thresholds, the Jev
model version it was tuned against, and the metric numbers from the last
optimization run. `decision.rs` loads it with `include_str!` and refuses to
build if an id the code uses is missing or the type differs. A checker pins
the file's hash like the other contracts, so a wording change is a reviewed
contract edit. The optimizer (below) is the only thing that writes this
file; hand edits are for bootstrapping P0 only.

## Question optimization with DSPy

Owner decision 2026-09-19: tune the questions with DSPy; train and score on a
synthetic corpus, a public business-mail corpus, and an opt-in export of the
owner's own mail.

Harness: `tools/jev-optimize/`, a Python project (uv, pinned lock) outside
the Rust build. It never runs in CI or at app runtime.

- **Program.** One `dspy.Module` per question set (P1 closure, P2 triage,
  each P3 rule, P4 extraction). Each Jev question is a `dspy.Predict` whose
  signature docstring is the instructions and whose output-field
  descriptions are the criteria, the mapping `dspy-typesafeify` uses. A
  small adapter turns the signature into a `/api/alpha/decisions` request
  and the answers back into typed outputs; no chat model is involved in
  answering.
- **Optimizer.** `dspy.GEPA`. It rewrites instructions and criteria from
  textual feedback; Jev takes no few-shot demonstrations, so MIPROv2's demo
  search has nothing to act on. The metric returns a score and a feedback
  string per example ("predicted fulfilled 0.82, label none: the paragraph
  thanks the sender and asks nothing"). The reflection model is the chat
  model already configured in the app, called through OpenRouter with
  `provider.zdr` so exported mail never leaves ZDR routes. Budget `auto=
  "light"` first; Jev calls cost cents, the reflection model is the cost.
- **Metrics.** Per question: accuracy against labels, Brier score and
  expected calibration error, and a threshold sweep at 0.05 steps that
  reports coverage and selective risk (the share of wrong answers among
  those above threshold). The accept and escalate thresholds written to the
  registry are chosen from the sweep to hold selective risk under 5% on the
  validation split, not copied from the docs.
- **Data.** Three sources, all paragraph-level JSONL with the same schema
  (`text`, `context` fields the question needs, `label`, `source`):
  1. Synthetic: a generated corpus of business email threads with known
     obligations, closures, recaps and boilerplate, committed under
     `tools/jev-optimize/data/synthetic/`. Used for bootstrapping and as the
     regression set the harness's own tests run against.
  2. Public: the Enron corpus, downloaded by a script into a git-ignored
     folder, never committed. Volume and real phrasing.
  3. Owner export: `--probe-saved-model --export-training <dir>` writes the
     loaded mail's paragraphs, participant handles, the chat model's
     accepted claims (silver labels for triage and extraction), and the
     owner's Accept/Reject on suggested updates and Handled/Dismissed
     decisions (gold labels for closure) to a folder the owner names. The
     flag prints a one-line warning that mail text is being written to disk,
     the folder is outside the repo, and the app's own state stays
     text-free. This is a deliberate, owner-invoked exception to the
     no-persistence rule and is documented as such.
  Splits: train, validation and a held-out test set that the optimizer never
  sees; the registry records test numbers only.
- **Output.** The harness writes `decision-questions.json` with the tuned
  wording, thresholds, `jev-1.13` pinned, dataset sizes and test metrics.
  The owner reviews the diff in a contract PR. A Jev version bump re-runs
  the harness before the registry is repinned.
- **Order.** The harness lands as phase O after P0 and before P1, so P1
  ships tuned wording. It is re-run after P4's compare mode produces owner
  export data at volume.

## Where Jev is used

Each item names the function it sits behind. Signatures do not change;
callers do not know which backend answered.

### P1. Closure and suggested updates (`scan_closures`)

Status: implemented on `feat/jev-closure-pass`.

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

The tuned state shape is preserved with one request per (loop, message,
paragraph), capped at the first 8 body paragraphs of each later message.
Question ids carry the paragraph ordinal. Requests are evaluated concurrently
under the decision client's parallel limit.

Recombination in code: highest probability above the accept threshold wins,
closure preferred on ties, one `SuggestedUpdate` per loop. `deadline_changed`
needs a temporal value the schema can carry; the code enumerates date
candidates from the winning paragraph with the existing prose parsers. Exactly
one normalizable candidate supplies the value, no candidate drops that outcome,
and multiple candidates escalate the pair. Gray-band pairs go to the chat model
exactly as the closure pass does today, restricted to those loops and grouped by
conversation. Every suggestion still needs review; `validation::route` is
unchanged.

Follow-up: `closure.modified` remains at chance on synthetic data. The next
tuning round plans to replace the four nouls with one `closure.outcome` choice
question.

Result: decision requests have no handle or 40-call cap, while escalated chat
work retains the 40-conversation cap; the same-thread and cross-thread routes
both become exhaustive.

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
| P0 | `feat/jev-transport` | `decision.rs`, `OpenRouterDecisions`, the question registry with hand-written bootstrap wording, settings toggle (off by default), Sources disclosure line, content-free "Check decision model" probe that sends a synthetic state, docs. Contract and checker edits in a sibling `chore/contract-decision-model` PR. |
| O | `feat/jev-optimize` | `tools/jev-optimize/` harness, synthetic corpus, Enron download script, the `--export-training` probe flag, first tuned registry in a contract PR. |
| P1 | `feat/jev-closure-pass` | Replace `scan_closures` internals. |
| P2 | `feat/jev-triage` | Triage request and projection trimming. |
| P3 | `feat/jev-rule-residue` | The seven rule replacements, one commit each. |
| P4 | `feat/jev-compare-probe` | Measuring mode. |
| P5 | `feat/jev-extraction` | The switch, after P4 numbers. |

O follows P0. P1 and P2 are independent after O. P3 items are independent of
each other. P5 waits on P4 and on a re-run of O with owner export data.

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
