# Ollama Cloud provider

[OpenRouter](openrouter.md) is the other selectable provider; this page covers
Ollama Cloud.

Use the [native setup window](native-setup.md) to enter a key, load models, and
switch between them without a terminal. The window restores the key and selected
model from the current user's Windows Credential Manager on subsequent launches.

For command-line diagnostics, run `pwsh ./tools/connect-ollama.ps1` in a PowerShell 7 terminal. Enter an Ollama
API key using the masked prompt. The command fetches available model names from
`https://ollama.com/api/tags` and presents a numbered chooser. Enter selects
`deepseek-v4-flash` when listed, including a dated version when that is the
available label. Select another number to switch models. Rerun to
change the selection; this development command does not yet save settings.

The command sends a content-free generation request to `https://ollama.com/api/chat`
and verifies an empty analysis object. It does not read Microsoft messages or
create tasks. A successful check does not establish extraction quality for that
model. Some listed models may not support the required text-chat response;
unsupported or invalid output fails visibly with no automatic model fallback.
Failures distinguish timeouts, connection errors, rate limiting, account balance,
HTTP request/server status codes, malformed JSON, and invalid response fields.
Raw upstream error bodies are not exposed.

The opt-in `ollama-cloud` inference feature uses the provider-neutral governed
analysis path, which serializes bounded canonical message projections and sends
them for analysis. It applies the ADR-007 schema, evidence, participant,
temporal, and loop-handle validation before returning reviewable claims. This is
cloud processing: selected subjects, message blocks, participant text,
attachment names, and link labels leave the computer. No attachment content is
sent. The caller must provide only messages within the user-authorized scan
scope and disclose the external transmission. The native [inbox
review](inbox-review.md) sends complete bounded conversations, recipient
context, signed-in ownership facts, and only the opaque open-loop handles that
the bounded suggested-update pass may reference. Source text and card titles
are resolved locally. Scanning requires no manual message selection.
The projection includes the signed-in user's display and given name and
authoritative per-message facts for whether the user is the sender, a direct
recipient, or a cc recipient; participant slots also state whether they are the
user.
The governed projection removes a quoted-history block only when collapsed
Unicode whitespace makes it exactly duplicate an earlier message body or a
quote already emitted; quotes with edits or inline replies remain.

## Plan and concurrent requests

Ollama Cloud allots concurrent request slots per plan: Free 1, Pro 3, Max/Team
10. Requests past your plan's allotment are queued on Ollama's side and then
rejected once that queue fills, so OpenLoops never dispatches more at once
than the plan allows.

Set **Ollama plan** on the Connections tab, under the API key. It defaults to
**Free**, which means one request at a time — the same sequential behaviour
OpenLoops had before this setting existed, and the only assumption that is
safe without knowing your account. Saved settings written before the plan
selector existed load as Free. Raise it only to the plan you actually have:
choosing a higher plan than your account holds does not buy more slots, it
just makes Ollama queue and then reject the extra requests, and each rejected
conversation is reported as failed for that scan.

A rate-limited request is never resent. `network_policy.retries` in
`contracts/model/provider-boundary.json` forbids automatically retrying any
request that already carried message content, so the affected conversation is
reported as a failed conversation for that scan and the remaining
conversations continue. OpenLoops also halves how many requests it keeps in
flight each time it is rate-limited, and widens back up as requests succeed.

The command-line checker does not persist credentials, payloads, or outputs. The
native setup window stores its key and settings in Windows Credential Manager;
it does not save provider payloads or outputs. The launcher restores
its process environment after exit. The key and raw response buffers are held
in zeroizing wrappers, without claiming removal of every allocator, TLS, OS, or
provider copy. Requests use fixed HTTPS endpoints, disabled redirects/proxies,
a 5-second connection timeout, a 300-second transport timeout, and a 60-second
response-body idle guard, no tools, no automatic retries, bounded request/response sizes, and at most the plan's
concurrent requests in flight. Model names are
validated before display.
Responses must match the selected model, complete normally, and contain no tool
calls or generated images/audio. Duplicate JSON members are rejected at every
depth, including the provider envelope. One outer Markdown JSON fence is removed
before strict validation; surrounding prose and unsupported fields are not accepted.
Canonical participant handles are mapped
to validated message slots before transmission, and source/loop handles are
checked before constructing the request. The adapter does not enable automatic
actions or pass a release gate.

Every conversation request — from the moment it is sent to the last byte of
the response — has a size-scaled wall deadline: 150 seconds for up to three
messages, plus 15 seconds for each message beyond three, capped at 300 seconds.
Content-free checks and listings retain the 150-second default. These deadlines
are independent of the 60-second response-body idle guard above. That guard
detects a body that stops producing chunks, but it does not constrain the
response-header wait and a provider can keep it armed by trickling bytes. This
matters especially for Ollama Cloud: a slow model can
send no response headers at all until generation has finished, so a bound
that only watched the body would never engage. OpenLoops runs the blocking
`send()` (the connection and the full header wait) and the blocking body
reads each on their own background thread and polls it every 250
milliseconds, so the selected deadline is enforced within about a quarter
second of expiry whether the connection is silently withholding headers,
silently withholding body bytes, or trickling either, and reports a timeout
once it is exceeded. A slow reasoning model working through a large
conversation can hit this bound; when it does, that one conversation is
reported as a failed conversation, not a failed scan, and the rest of the
scan continues. Clicking Stop in the native setup window abandons the request
currently in flight — within about a second, not only between conversations.
Abandoning a request this way does not instantly free the resources behind
it: the background thread's one blocking `send()`/`read()` call, and the
socket underneath it, can still linger for up to the 300-second transport
timeout, since that one call cannot be interrupted from outside.

Validation: `cargo test -p openloops-inference --features ollama-cloud --locked`.
The unit tests do not contact Ollama; live authentication and generation require
the user's API key entered locally. Repository Phase 0 no-network checks remain
unchanged and are not passing runtime-integration gates.

For developer diagnosis, `openloops-ui.exe --probe-saved-model` runs twelve synthetic
conversation cases with the saved model/key: requests, promises, non-actionable
recaps, third-party promises, team responsibility, independent actions, quoted
history, completion, acknowledgement, agreed closure, an amended request, and
completed closure. Every case uses the ADR-007 governed call used by scanning.
The update cases offer one opaque synthetic loop handle and require the
appropriate review-only closure or modification claim; acknowledgement must not
produce a closure. It reads only OpenLoops setup
credentials, never Microsoft mail, and reports fixed case/count/validation
diagnostics. It does not start the GUI or persist provider output. This is a
semantic smoke suite, not a held-out estimate of production accuracy.

References: [Ollama Cloud API](https://docs.ollama.com/cloud),
[authentication](https://docs.ollama.com/api/authentication),
[chat](https://docs.ollama.com/api/chat).
