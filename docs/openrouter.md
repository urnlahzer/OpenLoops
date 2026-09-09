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
`Authorization: Bearer` header and exactly four members: `model`, `messages`
(one `system` and one `user`), `stream: false`, and `provider: {"zdr": true}`.
That last member is the routing pin: OpenRouter routes the request only to
zero-data-retention endpoints. It ORs with the ZDR setting on your account, so
it holds whether or not your account is configured for ZDR, and OpenLoops never
relaxes it for an individual request. Before any message text is sent,
connecting re-checks that the selected model still appears in the ZDR listing.

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
scan; it does not stop it. HTTP 402 (out of credits), unauthorized, network,
timeout, and HTTP server errors do stop it: no further conversation is
dispatched, requests already in flight finish, and the scan is reported as
incomplete.

## Contract and limits

No `response_format` or `structured_outputs` member is sent. Strict application
validation of the answer stays authoritative, exactly as on the Ollama Cloud
path, and no server-side schema-enforcement claim is made.

An answer is accepted only when it carries exactly one assistant choice from the
exact selected model label. Tool calls, refusals, generated images or audio, an
`error` member, an empty message, and any finish reason other than `stop` are
rejected. Duplicate JSON members are rejected before validation.

Requests use fixed HTTPS endpoints, disabled redirects and proxies, a 5-second
connection limit, a 60-second per-read idle limit, no tools, no automatic
retries, bounded request and response sizes, and at most your configured
parallel requests in flight. The key and response buffers
are held in zeroizing wrappers. Failures report fixed codes — invalid key,
quota, rate limit, timeout, network, HTTP status, malformed JSON, invalid
fields — and never expose a raw upstream body.

The ZDR listing is much larger than a completion (several hundred kilobytes),
so it has its own larger read bound; the completion bound is unchanged.

Every request — from the moment it is sent to the last byte of the response —
is additionally bounded to 150 seconds of wall time, independently of the
60-second per-read idle limit above. That per-read limit only bounds one
`send()` or `read()` call and resets on every byte a connection sends, so a
provider that trickles occasional keep-alive bytes while a slow model keeps
working could otherwise hold a request open far longer than 60 seconds.
OpenLoops runs the blocking `send()` (the connection and the full header
wait) and the blocking body reads each on their own background thread and
polls it every 250 milliseconds, so the 150-second bound is enforced within
about a quarter second of expiry whether the connection is silently
withholding response headers, silently withholding body bytes, or trickling
either — and reports `Timeout` once it is exceeded. A slow reasoning model
working through a large conversation can hit this bound; when it does, that
one conversation is reported as a failed conversation, not a failed scan, and
the rest of the scan continues. Clicking Stop abandons the request currently
in flight — within about a second, not only between conversations. Abandoning
a request this way does not instantly free the resources behind it: the
background thread's one blocking `send()`/`read()` call, and the socket
underneath it, can still linger for up to the 60-second per-read idle limit
above, since that one call cannot be interrupted from outside.

## Validation

`cargo test -p openloops-inference --features ollama-cloud,openrouter --locked`.

The unit tests answer from a loopback socket and never contact OpenRouter. They
cover listing parse and deduplication, malformed listings, terminal-escape
rejection, the exact request bytes (`provider.zdr` present, `response_format`
absent), response binding, an invalid key, an oversized answer, the parallel
ceiling, and the key-status budget parse including absent and out-of-range
members. Live
authentication and generation require your own key, entered locally.

`openloops-ui.exe --probe-saved-model` runs the semantic smoke suite against
whichever provider is saved, including OpenRouter.

References: [zero data retention](https://openrouter.ai/docs/features/zdr),
[API overview](https://openrouter.ai/docs/api-reference/overview).
