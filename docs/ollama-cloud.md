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

The opt-in `ollama-cloud` inference feature also provides `OllamaCloud::analyze`,
which serializes bounded canonical message projections and sends them for
analysis. It applies the existing schema/evidence validation before returning
reviewable hypotheses. This is cloud processing: selected subjects, message
blocks, participant text, attachment names, and link labels leave the computer.
No attachment content is sent. The caller must provide only messages within the
user-authorized scan scope and disclose the external transmission. The native
[inbox review](inbox-review.md) now calls `expectations` with complete bounded
conversations, recipient context, and signed-in ownership facts. Its separate
live contract returns action summaries and exact source quotations resolved
locally, rather than model-supplied character offsets. Scanning requires no
manual message selection. Attachments are not sent by this path.

The command-line checker does not persist credentials, payloads, or outputs. The
native setup window stores its key and settings in Windows Credential Manager;
it does not save provider payloads or outputs. The launcher restores
its process environment after exit. The key and raw response buffers are held
in zeroizing wrappers, without claiming removal of every allocator, TLS, OS, or
provider copy. Requests use fixed HTTPS endpoints, disabled redirects/proxies,
a 5-second connection timeout and a 60-second per-read idle timeout, no tools,
no automatic retries, and bounded request/response sizes. Model names are
validated before display.
Responses must match the selected model, complete normally, and contain no tool
calls or generated images/audio. Duplicate JSON members are rejected at every
depth, including the provider envelope. One outer Markdown JSON fence is removed
before strict validation; surrounding prose and unsupported fields are not accepted.
Canonical participant handles are mapped
to validated message slots before transmission, and source/loop handles are
checked before constructing the request. The adapter does not enable automatic
actions or pass a release gate.

Every request — from the moment it is sent to the last byte of the response —
is additionally bounded to 150 seconds of wall time, independently of the
60-second per-read idle timeout above. That per-read timeout only bounds one
`send()` or `read()` call and resets on every byte a connection sends, so a
slow keep-alive connection could otherwise hold a request open far longer
than 60 seconds. This matters especially for Ollama Cloud: a slow model can
send no response headers at all until generation has finished, so a bound
that only watched the body would never engage. OpenLoops runs the blocking
`send()` (the connection and the full header wait) and the blocking body
reads each on their own background thread and polls it every 250
milliseconds, so the 150-second bound is enforced within about a quarter
second of expiry whether the connection is silently withholding headers,
silently withholding body bytes, or trickling either, and reports a timeout
once it is exceeded. A slow reasoning model working through a large
conversation can hit this bound; when it does, that one conversation is
reported as a failed conversation, not a failed scan, and the rest of the
scan continues. Clicking Stop in the native setup window abandons the request
currently in flight — within about a second, not only between conversations.
Abandoning a request this way does not instantly free the resources behind
it: the background thread's one blocking `send()`/`read()` call, and the
socket underneath it, can still linger for up to the 60-second per-read idle
timeout above, since that one call cannot be interrupted from outside.

Validation: `cargo test -p openloops-inference --features ollama-cloud --locked`.
The unit tests do not contact Ollama; live authentication and generation require
the user's API key entered locally. Repository Phase 0 no-network checks remain
unchanged and are not passing runtime-integration gates.

For developer diagnosis, `openloops-ui.exe --probe-saved-model` runs twelve synthetic
conversation cases with the saved model/key: requests, promises, non-actionable
recaps, third-party promises, team responsibility, independent actions, quoted
history, completion, acknowledgement, agreed closure, an amended request, and
completed closure. The last three exercise `resolution_kind` and closure
semantics: a request that asked for the user's agreement and got it must
resolve to `Agreed`; a correction that leaves the action owed (only the
amount changed) must stay open, with `resolution` null and the corrected
amount reflected in `action`; and a plain completion must still resolve to
`Completed`. It reads only OpenLoops setup
credentials, never Microsoft mail, and reports fixed case/count/validation
diagnostics. It does not start the GUI or persist provider output. This is a
semantic smoke suite, not a held-out estimate of production accuracy.

References: [Ollama Cloud API](https://docs.ollama.com/cloud),
[authentication](https://docs.ollama.com/api/authentication),
[chat](https://docs.ollama.com/api/chat).
