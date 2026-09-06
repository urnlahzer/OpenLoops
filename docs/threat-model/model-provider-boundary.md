# Model-provider boundary threat model

## Boundary

This model covers the future flow from a bounded transient canonical mailbox
projection to an explicitly selected provider and back to a validated hypothesis.
P0-WI-10 is contract-only. No provider adapter, origin, credential, request,
response, persistent record, or model-dependent capability exists yet.
`disabled` is the sole default and produces zero provider network requests.
Ollama local, Ollama Cloud, and an optional approved-HTTPS adapter remain disabled
and unadvertised behind G-MODEL, G-PRIV, G-SEC-AUDIT, and G-RELEASE.
All four gates remain unrun.

The future data flow is:

```text
bounded canonical projections in memory
        |
        v
deterministic relevance selection and explicit consent check
        |
        v
exact local-loopback or consented public-HTTPS transport
        |
        v
bounded untrusted provider bytes
        |
        v
strict schema, evidence, deterministic, and semantic validation
        |
        v
review-only hypothesis or analysis_unavailable
```

There is no tool or mutation edge from the provider. Prompt, request, response,
output, rationale, transcript, and embedding bytes are never durable OpenLoops
state.

## Threats and required controls

| Threat | Control and fail-closed result |
|---|---|
| Provider is selected or contacted before its gates pass | `disabled` is the only configured default; runtime/provider inventories are empty, credentials are not accepted, and zero provider requests occur. Any other state is a gate failure. |
| Silent external transmission or provider fallback | Require explicit profile selection and consent before content. Never fall through from local to cloud, from Ollama Cloud to approved HTTPS, or to another origin/model after any failure. |
| SSRF through a configurable endpoint | Local mode is exact direct IPv4 loopback on `127.0.0.1:11434`; external modes are one canonical consented public HTTPS origin plus adapter-fixed paths. Reject userinfo, query, fragment, IP literals, private, loopback, link-local, multicast, reserved, documentation, benchmark, carrier-grade NAT, unspecified, and metadata destinations. |
| DNS rebinding or address change between validation and connection | Resolve under application control, validate every answer, connect only to an allowed answer, and revalidate the connected peer address. Any mismatch cancels before authorization or content is sent. |
| Redirect leaks content or authorization | Disable redirects in the client and reject every redirect response. Never forward authorization or replay content to another authority. |
| Ambient proxy, PAC, WPAD, or SDK default reroutes traffic | Ignore environment variables, system proxy settings, PAC/WPAD, and provider-SDK proxy defaults. A route that cannot prove the direct approved boundary rejects. |
| TLS interception or hostname bypass weakens the external boundary | Require valid TLS, system trust, and hostname verification; prohibit custom CAs, insecure switches, and certificate bypass. Failure sends no content or key. |
| Loopback is presented as proof that processing is local | Disclose the separately running Ollama process. G-MODEL starts a disposable process with `OLLAMA_NO_CLOUD=1`, confirms a content-free cloud-disabled status and local digest, proves zero non-loopback connection, and rejects cloud models/fallbacks; otherwise `ollama_local` remains disabled and unadvertised. |
| A content-free preflight silently becomes content-bearing or trusts unstable provider identity | Permit only the fixed bounded `/api/tags` model-list request with no mailbox projection. Strictly parse model label/digest, retain no raw response, and disable on API shape, capability, digest, or documented-behavior drift because the Ollama API is not strictly versioned. |
| Ollama Cloud structured-output limitation is mistaken for validated output | Make no server-side schema-enforcement claim. Always reject duplicate JSON members and unknown fields and run the complete application schema, handle, Unicode range, evidence correspondence, participant, deterministic date, consistency, and semantic validation sequence. |
| Approved-HTTPS adapter becomes an arbitrary HTTP client | Require a separately reviewed fixed wire schema, fixed authentication mapping, adapter-owned path, one canonical public HTTPS origin, and no arbitrary header bag. It cannot substitute for the required tested Ollama Cloud path. |
| Credential escapes the OS-protected boundary | Accept one bounded write-only credential only after ADR-005/G-STATE support exists; hold it only in process memory or `provider-credential.dpapi`; construct authorization after final authority validation. Never expose it to the add-in, CLI, logs, errors, diagnostics, browser state, repository, or package. Secure-store failure disables provider use with no plaintext fallback. |
| Content-free credential check accidentally sends mailbox data | Use a content-free request only where supported. Any unavailable or failed check leaves the profile disabled and sends no mailbox content. |
| Excess mailbox context is disclosed | Deterministic code selects one changed message and at most four context projections. Enforce closed field categories and exact per-message/block/participant/attachment/link and total-byte caps before transport. Prohibit whole-mailbox/thread defaults, attachments, linked contents, hrefs/URLs, credentials, unrelated recipients, and Graph locators. |
| Mail prompt injection changes policy, scopes, endpoint, or tools | Length-frame mailbox projections as hostile data. No tool/function/remote-retrieval surface exists, and content cannot alter the policy prompt, schema, scopes, provider, model, or endpoint. |
| Provider output issues a Graph/Office/reminder or lifecycle command | The response schema has no mutation authority. The model can produce hypotheses only; deterministic application code and explicit user commands own state changes. Invalid or hostile output yields `analysis_unavailable` or `needs_review` and zero mutation. |
| Schema-valid semantic manipulation is accepted as safe | Apply semantic-adversarial checks and deterministic positive constraints after structural/evidence validation. Valid spans or confident language alone never authorize automation or closure. Automation stays unavailable until ADR-011 and the applicable G-AUTO gate pass. |
| Malformed JSON exploits parser differences | Require strict UTF-8 and exactly one JSON value; reject duplicate members, unknown fields, unsupported versions, noncanonical values, and all bound violations before semantic use. |
| A provider extension adds tools or bypasses the response contract | Ollama request JSON has only `model`, `messages`, and `stream`; messages have only `role` and `content`; `format`, tools, images, retrieval, and extension fields are absent. Validate responses against the closed Draft 2020-12 `analysis-output-v1` schema in application code. |
| Fabricated or cross-message evidence is accepted | Require supplied opaque-handle membership, canonical block and Unicode-scalar bounds, transient evidence-text correspondence, participant-position membership, deterministic date/timezone reparse, and internal claim/relation consistency. Failure releases no hypothesis for mutation. |
| Oversized, slow, or unbounded streaming response exhausts resources | Use non-streaming requests, exact request/response byte limits, five-second connect and 60-second wall limits, and a bounded response reader that cancels immediately over a byte/time cap. |
| Automatic retry duplicates disclosure or follows a changed route | Perform zero automatic retry after any content-bearing request. A user-visible bounded replay revalidates consent, exact origin, model, policy/schema, and evidence. |
| Hidden or drifted confidence/detection settings silently broaden model effects | Show provider, exact endpoint, model, external-data disclosure, request/deadline/closure sensitivities, and per-claim review thresholds under pinned `model-sensitivity-v1`. Settings changes offer bounded review-only replay, never start replay automatically, preserve existing loops, and authorize zero terminal or external mutation. |
| Endpoint, model, schema, prompt, policy, or provider drift inherits consent or automation calibration | Invalidate consent on every contract-listed change. Drift permits only explicit review-only replay and invalidates all automation calibration. |
| Provider/SDK error contains host, path, key, header, prompt, or response | Disable provider-SDK logging and sanitize at the adapter source before errors enter application types. Expose only fixed content-free status codes and bounded counters. |
| Prompt, response, rationale, transcript, or embedding reaches state or artifacts | Prohibit each class in the privacy allowlist; scan synthetic canaries across durable state, temp, browser, crash, diagnostics, packages, installers, CI, and repository output. The exact enabled test request is the only provider-payload exception. |
| Provider retention is mistaken for OpenLoops-local privacy | Disclose provider identity, exact authority, transmitted categories, context maximum, and provider privacy/retention responsibility before consent. BYO credentials do not make external processing local or erase provider copies. |
| Provider failure corrupts existing loops or causes a mutation | Preserve prior state, expose `analysis_unavailable`, allow only deterministic reduced behavior, and make zero Graph, Office, reminder, lifecycle, or provider mutation. |
| A provider setting or key persists beyond its authorized lifecycle | Encrypt only the approved user-setting fields and secure-store reference; delete the credential when the provider is disabled/reset or the account disconnects. Replacement invalidates active use and does not trigger replay. |
| Hostile content appears in diagnostics or public artifacts | Diagnostics contain no host, path, source identifier, content, prompt, response, model text, key, or header. Public-repository, package, and release canary checks must pass without printing the suspected value. |

## Consent and disclosure checkpoint

Before any content-bearing request, the application must confirm the exact
provider profile, authority, model label, transmitted field categories, maximum
context count, provider privacy/retention responsibility, and local-process or
external-service boundary. Profile, authority, adapter path, model label/digest,
field set, schema/policy version, or capability drift invalidates that consent.
Credential replacement instead requires content-free revalidation; deletion
disables active use, and neither triggers replay. Consent never follows a
redirect or endpoint edit.

## Residual risk and gates

Even a compliant local provider is a separate process and may have its own
configuration, logs, plugins, cloud features, or compromise. An external
provider controls its own retention and administrative plane. OpenLoops cannot
promise erasure from provider systems, OS paging, same-user malware,
administrators, endpoint tooling, backups, snapshots, or platform crashes.

G-MODEL must prove the strict wire/schema/evidence/semantic contract, exact
local/cloud/approved-HTTPS origins, SSRF/DNS/redirect/proxy/TLS defenses,
timeouts and fallback, model/provider drift, and source-sanitized errors. G-PRIV
must find zero prohibited synthetic canaries outside the exact enabled test
request. G-SEC-AUDIT reviews the compiled provider/network/secret-store path,
and G-RELEASE verifies the shipped dependency and package boundary. Until all
applicable gates pass, no profile is enabled or advertised and AS-12, AS-16,
AS-22, and every product acceptance criterion remain unpassed. Gate failure is
routed to the product owner; the safe fallback is disabled model-dependent
analysis, never broader network access, weaker validation, plaintext secrets, or
another provider.
