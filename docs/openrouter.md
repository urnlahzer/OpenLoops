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
4. Click **Test selected model** for a content-free generation check against
   the selected model. It sends no email and creates no task.

Create a key at <https://openrouter.ai/settings/keys>.

## What is sent, and where

The listing names every ZDR endpoint of every model, so many rows share a
model. OpenLoops keeps the first healthy row per `model_id`, drops every
endpoint whose `status` is non-zero, and never persists the raw response. Only
models with a working ZDR endpoint are selectable.

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

## Contract and limits

No `response_format` or `structured_outputs` member is sent. Strict application
validation of the answer stays authoritative, exactly as on the Ollama Cloud
path, and no server-side schema-enforcement claim is made.

An answer is accepted only when it carries exactly one assistant choice from the
exact selected model label. Tool calls, refusals, generated images or audio, an
`error` member, an empty message, and any finish reason other than `stop` are
rejected. Duplicate JSON members are rejected before validation.

Requests use fixed HTTPS endpoints, disabled redirects and proxies, a 5-second
connection limit, a 60-second total limit, no tools, no automatic retries, and
bounded request and response sizes. The key and response buffers are held in
zeroizing wrappers. Failures report fixed codes — invalid key, quota, rate
limit, timeout, network, HTTP status, malformed JSON, invalid fields — and never
expose a raw upstream body.

The ZDR listing is much larger than a completion (several hundred kilobytes),
so it has its own larger read bound; the completion bound is unchanged.

## Validation

`cargo test -p openloops-inference --features ollama-cloud,openrouter --locked`.

The unit tests answer from a loopback socket and never contact OpenRouter. They
cover listing parse and deduplication, malformed listings, terminal-escape
rejection, the exact request bytes (`provider.zdr` present, `response_format`
absent), response binding, an invalid key, and an oversized answer. Live
authentication and generation require your own key, entered locally.

`openloops-ui.exe --probe-saved-model` runs the semantic smoke suite against
whichever provider is saved, including OpenRouter.

References: [zero data retention](https://openrouter.ai/docs/features/zdr),
[API overview](https://openrouter.ai/docs/api-reference/overview).
