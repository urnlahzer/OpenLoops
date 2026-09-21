# OpenRouter provider

OpenRouter is the second selectable model provider, alongside
[Ollama Cloud](ollama-cloud.md). Every OpenLoops request to it is pinned to
OpenRouter's zero-data-retention (ZDR) routing.

## Setup

In the [native setup window](native-setup.md), on the Connections tab:

1. Under **Choose your AI model**, select **OpenRouter** as the provider.
2. Paste an OpenRouter API key. The field is masked, and the key is stored
   write-only in the current user's Windows Credential Manager, in the same
   record and the same way as the Ollama key. Each provider keeps its own key
   and its own model choice, so switching between them loses neither, and one
   provider's key is never sent to the other.
3. Click **Load ZDR models**. This fetches
   `https://openrouter.ai/api/v1/endpoints/zdr`, which is public: no key and no
   message content is sent. The menu shows one entry per model as
   `label (id)`, sorted by label.
4. Set **Parallel requests (max)** — how many conversations a scan analyzes
   at once. It defaults to 32 and accepts 1 to 100. See
   [Concurrency and rate limits](#concurrency-and-rate-limits).
5. Click **Test selected model** for a content-free generation check against
   the selected model. It sends no email and creates no task.
6. Optionally turn on **Use the Jev decision model for closure checks and
   triage**. The same toggle also enables rule-residue questions. It is off by
   default.
7. Click **Check decision model** to run the content-free endpoint check and
   report whether that endpoint accepts the `provider.zdr` member.
8. With the decision model on, **Find loops with** switches between **Chat
   model** (default) and **Decision model**. This is the gated P5 switch:
   flip it only after the `--compare-decisions` probe shows at least 90%
   claim-type agreement on 200 or more paragraphs of your own mail. Gray-band
   paragraphs still go to the chat model either way.
9. Optionally, with a scan result loaded, type an absolute folder path
   outside the repository into **Training export folder** and click
   **Export training data** twice (the first click only arms a warning) to
   write the current scan's mail text as training data for the `Jev`
   question-optimization harness. See [Inbox review](inbox-review.md) and
   `tools/jev-optimize/README.md`.

Create a key at <https://openrouter.ai/settings/keys>.

## What is sent, and where

The listing names every ZDR endpoint of every model, so many rows share a
model. OpenLoops keeps the first healthy row per `model_id`, drops every
endpoint whose `status` is non-zero (an absent or null `status` counts as
healthy), and never persists the raw response. Only
models with a working ZDR endpoint are selectable. A row OpenLoops cannot read
is skipped rather than failing the whole menu, since one new or malformed row
among hundreds must not make every model unselectable; a listing that is not a
bounded `data` array does fail.

Unlike a completion answer, the listing is not checked for duplicate JSON
members. It is several hundred kilobytes, past the strict parser's bound, so it
is parsed with a plain JSON reader where a repeated member silently takes its
last value. That is acceptable only because the listing is never an authority:
it populates a menu you choose from, the id you choose is revalidated before it
reaches a request, and `provider.zdr=true` on every request enforces the routing
whatever the listing said. Completion answers still go through the strict
duplicate-rejecting parser.

Each analysis request is a single non-streaming
`POST https://openrouter.ai/api/v1/chat/completions` with the key as an
`Authorization: Bearer` header and exactly five members: `model`, `messages`
(one `system` and one `user`), `stream: false`, `provider: {"zdr": true}`, and
`max_tokens: 16384`. The `provider` member is the routing pin: OpenRouter
routes the request only to zero-data-retention endpoints. It ORs with the ZDR
setting on your account, so it holds whether or not your account is
configured for ZDR, and OpenLoops never relaxes it for an individual request.
Before any message text is sent, connecting re-checks that the selected model
still appears in the ZDR listing.

The governed projection also sends the signed-in user's display and given name,
plus authoritative per-message facts for whether the user is the sender, a
direct recipient, or a cc recipient. Participant slots carry a user marker so
requests addressed to someone else are not attributed to the signed-in user.

When the decision model is on, triage requests are sent before the governed
conversation request. Each triage request contains exactly the message subject,
one body paragraph as `paragraph_text`, and whether the message was sent by the
user as `from_user`; at most the first 40 body paragraphs of each message are
checked. The answer contains probabilities for the six registered triage
questions and no generated text. Conversations that continue to the chat model
omit accepted boilerplate paragraphs without renumbering the surviving blocks.

When **Find loops with** is set to **Decision model**, the triage request
above is replaced by one combined per-paragraph request: the same six
triage nouls, plus `extract.claim_type` (the same choice P4 already sends
in `--compare-decisions`), plus `extract.waiting_party` (a choice over that
conversation's own participant handles, capped at 254, plus `none`) and
`extract.temporal` (a choice over date candidates the app's own prose
parser found in that paragraph, normalized before sending, plus `none`;
sent only when at least one candidate exists). The request additionally
carries `to_user`, `cc_user`, the signed-in user's display and given name,
and the participant catalog's own display text -- the same facts the
governed chat request already carries, reshaped for a typed choice instead
of free text. Nothing about a Jev answer is persisted; a confident claim is
assembled locally and never re-sent. A gray-band `extract.claim_type`
answer, or a request error, sends that paragraph's whole conversation to
the unchanged chat-model request instead.

Closure checks may also be sent to
`POST /api/alpha/decisions` with request members `model`, `state`, `questions`,
and `provider: {"zdr": true}` when the endpoint accepts it (which the check
reports). Each request contains the obligation title, its resolved evidence
paragraph, and one later body paragraph, plus whether that later message was
sent by the user and the whole days between the messages. At most the first 8
body paragraphs of a later message are checked. The model is
`typesafe/jev-1.13` and is validated against the ZDR listing before any content
is sent. Its answers are probabilities, never text, and nothing from them is
persisted. Gray-band pairs go to the governed chat-model closure call.

Rule-residue requests use the same Decisions endpoint, ZDR routing, concurrency
limit, cancellation, 20-second deadline, and never-resend policy. They send
only the fields named by the applicable registered question: recap detection
sends `sender`, `subject`, and `first_paragraph`; event scoping sends
`request_text` (the card action plus its evidence quote); event matching sends
`phrase` and `event_name`; duplicate detection sends `action_a` and `action_b`;
thread merging sends `subject_a`, `subject_b`, `first_paragraph_a`, and
`first_paragraph_b`; and deadline classification sends `phrase`. Answers and
the per-scan state cache remain in memory and are not persisted.

The governed conversation projection omits a quoted-history block only when
Unicode-whitespace normalization makes it exactly duplicate an earlier message
body or a quote already emitted; edited quotes and inline replies remain.

`max_tokens` is fixed, not user-configurable. OpenRouter's pre-request credit
check reserves credit for each request that is still in flight, sized by the
request's output ceiling. Without an explicit `max_tokens` that ceiling is
the selected model's own maximum (65,536 tokens on some models) rather than
what a bounded `analysis-output-v1` answer could ever need, so a handful of
concurrent requests can reserve past a modest balance. 16,384 tokens is well
above what a full 64-claim answer needs, with headroom for a reasoning
model's hidden thinking tokens, which OpenRouter counts against the same
ceiling. See [Concurrency and rate limits](#concurrency-and-rate-limits) for
what happens when the reservation still overruns the balance.

Subjects, current message text, quoted history, and participants for the
messages in your configured scan scope leave the computer. Attachments and
linked content are not sent. This is external processing.

**OpenRouter's own retention policy applies.** Zero data retention is a routing
property of the endpoints OpenRouter selects, not a promise OpenLoops can make
on OpenRouter's behalf. OpenLoops can minimize and disclose what it transmits;
it cannot promise deletion by OpenRouter or by the endpoint provider behind it.
Read <https://openrouter.ai/docs/features/zdr> before enabling this provider.

## Concurrency and rate limits

OpenRouter publishes no concurrency cap for a paid key, so **Parallel requests
(max)** is your own ceiling rather than a provider-imposed one. A scan runs
that many model requests at once; conversations are independent, so the result
is identical to analyzing them one at a time, only faster. Free models are
separately limited to 20 requests per minute by OpenRouter, and the upstream
provider behind any given model may return HTTP 429 under load whatever your
key allows.

When a request comes back rate-limited, OpenLoops halves how many requests it
keeps in flight, down to a floor of one. It widens the concurrency back by a
quarter after eight consecutive completed requests, up to your configured
ceiling.

**A rate-limited request is never resent.** `network_policy.retries` in
`contracts/model/provider-boundary.json` forbids automatically retrying any
request that already carried message content. The affected conversation is
reported in the scan's failure list as

> Conversation *n* (*k* messages; subject: …): The provider rate-limited this
> request; it was not resent.

and the scan continues with the remaining conversations. HTTP 429 narrows the
scan; it does not stop it.

Decision requests share this never-resent rule and count toward OpenRouter's
rate limits.

OpenRouter uses HTTP 402 for two different conditions, and OpenLoops tells
them apart by the reason OpenRouter states in the response body, which the
failure line repeats verbatim.

The first is a per-request credit reservation overrun:

> This request would exceed your available credits given your current
> in-flight requests. Retry after in-flight requests settle, or add credits.

OpenRouter holds credit against every request still in flight, sized by its
`max_tokens` (see above), and rejects a new request when the held total plus
this request's reservation would pass the balance. The requests it does
accept are forwarded, answered, and billed as usual, so the activity log
shows them as successful. This is a too-many-at-once condition, and OpenLoops
treats it exactly like HTTP 429: the concurrency narrows, the request is not
resent, the conversation is reported failed, and the scan continues. A
larger balance raises how many requests fit at once; so does a lower
**Parallel requests (max)**.

Any other 402 (an exhausted balance, a key spend limit) is reported per
conversation and does not stop the scan either; the failure line carries
OpenRouter's stated reason. Unauthorized, network, timeout, and HTTP server errors do still
stop the scan: no further conversation is dispatched, requests already in
flight finish, and the scan is reported as incomplete. Unlike a 402, these
have not been observed to be conversation-specific -- an invalid key or a
down network affects every subsequent request identically, so continuing to
dispatch into them would only produce the same failure repeated for no
benefit.

## Contract and limits

No `response_format` or `structured_outputs` member is sent. Strict application
validation of the answer stays authoritative, exactly as on the Ollama Cloud
path, and no server-side schema-enforcement claim is made.

An answer is accepted only when it carries exactly one assistant choice from the
exact selected model label. Tool calls, refusals, generated images or audio, an
`error` member, an empty message, and any finish reason other than `stop` are
rejected. Duplicate JSON members are rejected before validation.

Requests use fixed HTTPS endpoints, disabled redirects and proxies, a 5-second
connection limit, a 300-second transport timeout, a 60-second response-body
idle guard, no tools, no automatic retries, bounded request and response sizes, and at most your configured
parallel requests in flight. The key and response buffers
are held in zeroizing wrappers. Failures report fixed codes — invalid key,
quota, rate limit, timeout, network, HTTP status, malformed JSON, invalid
fields — and never expose a raw upstream body.

The ZDR listing is much larger than a completion (several hundred kilobytes),
so it has its own larger read bound; the completion bound is unchanged.

Every conversation request — from the moment it is sent to the last byte of
the response — has a size-scaled wall deadline: 150 seconds for up to three
messages, plus 15 seconds for each message beyond three, capped at 300 seconds.
Content-free checks and listings retain the 150-second default. These deadlines
are independent of the 60-second response-body idle guard above. That guard
detects a body that stops producing chunks, but it does not constrain the
response-header wait and a provider can keep it armed by trickling bytes.
OpenLoops runs the blocking `send()` (the connection and the full header
wait) and the blocking body reads each on their own background thread and
polls it every 250 milliseconds, so the selected deadline is enforced within
about a quarter second of expiry whether the connection is silently
withholding response headers, silently withholding body bytes, or trickling
either — and reports `Timeout` once it is exceeded. A slow reasoning model
working through a large conversation can hit this bound; when it does, that
one conversation is reported as a failed conversation, not a failed scan, and
the rest of the scan continues. Clicking Stop abandons the request currently
in flight — within about a second, not only between conversations. Abandoning
a request this way does not instantly free the resources behind it: the
background thread's one blocking `send()`/`read()` call, and the socket
underneath it, can still linger for up to the 300-second transport timeout,
since that one call cannot be interrupted from outside.

Decision requests have their own 20-second wall deadline.

## Validation

The owner-run [Jev optimization harness](../tools/jev-optimize/README.md) tunes
the decision questions using synthetic or public text, and owner-exported text
only when the owner explicitly runs that path. It sends those inputs to the
same ZDR Decisions and reflection endpoints described above; downloaded data,
recordings, and owner exports are not committed.

`cargo test -p openloops-inference --features ollama-cloud,openrouter --locked`.

The unit tests answer from a loopback socket and never contact OpenRouter. They
cover ZDR menu parsing, model revalidation, request shape, strict decision
request and answer validation, terminal HTTP 402, and concurrency narrowing
after HTTP 429. Live
authentication and generation require your own key, entered locally.

`openloops-ui.exe --probe-saved-model` runs the ADR-007 governed semantic smoke
suite against whichever provider is saved, including OpenRouter. Its synthetic
update cases use only opaque loop handles and report content-free counts and
fixed validation reasons.

`openloops-ui.exe --probe-saved-model --compare-decisions [--data <dir>]` is
the P4 measuring-mode probe: it requires the saved provider to be OpenRouter
with the decision model on, then runs the chat extractor and the Jev-native
triage/`extract.claim_type` questions over a corpus and prints one
content-free line per question (`n`, agreement rate, gray-band rate, wall
time and input tokens per side). Without `--data` the corpus is the built-in
synthetic probe cases; with `--data <dir>` it is `<dir>/triage.jsonl`, as
written by **Export training data** (see [Setup](#setup) above and
`tools/jev-optimize/README.md`). Nothing is written to disk and no row
content appears in the output.

References: [zero data retention](https://openrouter.ai/docs/features/zdr),
[API overview](https://openrouter.ai/docs/api-reference/overview).
