# ADR-007: Model-provider boundary

- **Status:** Accepted
- **Date:** 2026-07-20
- **Work item:** P0-WI-10
- **Owner decision:** OWN-08
- **Blocking gates:** G-MODEL, G-PRIV, G-SEC-AUDIT, G-RELEASE

## Context

OWN-08 settles the product direction: local and external bring-your-own model
providers are first-class modes, including hosted Ollama use. Approval does not
authorize transmission. External use still requires exact provider, endpoint,
model, transmitted-field, and privacy disclosure; explicit consent; an
OS-protected write-only credential; and passing G-MODEL and G-PRIV. Broad public
release additionally remains blocked by G-SEC-AUDIT and G-RELEASE. These product
choices are encoded here rather than reopened.

The model is an untrusted transient hypothesis producer. It cannot become the
authority for evidence, lifecycle, policy, endpoint selection, tools, Microsoft
Graph or Office operations, or any other mutation. A provider-side structured
output feature is only defense in depth. In particular, the current Ollama Cloud
boundary does not enforce structured output, so strict application validation
remains authoritative even when a provider accepts a schema-shaped request.

## Decision

`contracts/model/provider-boundary.json` is the closed P0-WI-10 decision
contract. This work item implements no adapter, transport, credential store,
provider request, persistent record, provider configuration, or model-dependent
analysis. `disabled` is the only default and configured profile. It performs
zero provider network requests, accepts no credential, preserves existing loops,
returns `analysis_unavailable` for model-dependent work, and causes zero
mutation. There is no implicit provider selection or fallback to another
profile, origin, model, or transport.

The future profiles are fixed as follows and remain disabled and unadvertised:

- `ollama_local` uses exactly `http://127.0.0.1:11434` with adapter-owned
  `/api/chat` and `/api/tags` paths. It is direct IPv4 loopback only, accepts and
  sends no provider credential, and permits no DNS, proxy, redirect, alternate
  port, LAN address, tunnel, or fallback. Loopback is not proof that processing
  stays local: G-MODEL must prove cloud use is disabled and cloud models are
  rejected before this profile may be claimed.
- `ollama_cloud` uses the non-editable exact authority `https://ollama.com` with
  adapter-owned `/api/chat` and `/api/tags` paths. Its one bounded user API key
  is write-only, is held only in process memory or the ADR-005
  `provider-credential.dpapi` store, and becomes an `Authorization: Bearer`
  value only after final authority validation. It is never sent to another
  authority. Because Ollama Cloud does not currently enforce structured
  outputs, the application always performs the complete validation sequence
  below and makes no server-side schema-enforcement claim.
- `approved_https` is an optional adapter for one canonical, explicitly
  consented, public HTTPS origin plus an adapter-fixed, separately reviewed path
  and wire/authentication schema. It supports only one OS-protected write-only
  credential with a fixed authentication mapping, never an arbitrary header
  bag. It cannot substitute for the required tested `ollama_cloud` hosted path.
  Its one filled adapter is `openrouter`, with the non-editable exact authority
  `https://openrouter.ai`, the adapter-owned chat path
  `/api/v1/chat/completions`, and the adapter-owned model-listing path
  `/api/v1/endpoints/zdr`. Its credential handling is identical to
  `ollama_cloud` in every respect. Every chat request carries
  `provider.zdr=true`, so OpenRouter routes only to zero-data-retention
  endpoints; that request flag ORs with the account setting and is never
  relaxed per request, and the selected model is revalidated against the
  zero-data-retention listing before any content is sent. The listing request
  is content-free, carries no credential, and its raw response is never
  persisted. No `response_format` or `structured_outputs` member is sent, so
  application validation remains authoritative here exactly as it is for
  `ollama_cloud`. Zero data retention is OpenRouter's routing property, not an
  OpenLoops guarantee: OpenRouter's own retention and administrative policy
  still governs what it does with a request.

Provider preflight is content-free. Ollama model discovery uses only the fixed
`GET /api/tags` path after authority validation and strictly bounds/parses the
selected model label and digest without retaining the raw response. The selected
identity combines profile, label, provider-reported digest when available, and
adapter schema/policy versions. Because Ollama's API is not strictly versioned,
any wire shape, capability, model identity, or documented-behavior drift disables
the profile pending review. Local-only release proof requires a disposable
Ollama process with `OLLAMA_NO_CLOUD=1`, a content-free cloud-disabled status,
a locally resident selected digest, rejection of cloud models/fallbacks, and
zero non-loopback connection; ordinary loopback reachability is insufficient.

### Bounded request and response

One request contains one changed-message projection and no more than four
context-message projections selected by deterministic relevance code. The only
content categories are subject, body block, quote block, sender, To and Cc
participant positions, attachment name, and link label. Whole-mailbox and
whole-thread-by-default input, attachment bytes, linked content, URLs or href
values, credentials, unrelated recipients, Graph locators, provider keys,
images, tools, functions, remote retrieval, and arbitrary fields are prohibited.
Mailbox projections are length-framed untrusted data and cannot change the
policy prompt, schema, scopes, tools, endpoint, or model.

The Ollama request has exactly the ordered top-level fields `model`, `messages`,
and `stream`; each message has only `role` and `content`, with exact `system` then
`user` roles. No provider-side `format`, tools, or extension field is sent in the
MVP contract. The request contract is one non-streaming JSON `POST`, at most 524,288 bytes,
with at most 64 blocks per message, 8,192 Unicode scalars per block, 500
participants, 256 attachment names, and 256 link labels per message. The
response is at most 262,144 bytes, with a five-second connection limit, a
60-second per-read idle-guard limit, a 150-second total wall-time limit on
the whole request, and no more than 64 claims. A bounded reader cancels
immediately when a byte or time limit is crossed.

Application validation occurs in this exact order:

1. require strict UTF-8 and exactly one JSON value;
2. reject duplicate members and unknown fields;
3. validate the exact schema version, catalogs, numeric bounds, and collection
   bounds;
4. require membership in the opaque handles supplied by the application;
5. validate canonical-block and Unicode-scalar range bounds;
6. verify evidence-text correspondence against the transient canonical input;
7. require participant-position membership;
8. deterministically reparse dates and resolve timezones;
9. check internal claim and relation consistency; and
10. apply semantic-adversarial and deterministic positive-policy checks.

The closed Draft 2020-12 schema is
`contracts/model/analysis-output.schema.json`. Its only top-level members are
`schema_version` and `claims`; every claim has the closed hypothesis, evidence,
participant, related-loop, temporal, confidence, and ambiguity shape. It rejects
unknown fields and cannot encode a command or mutation.

Malformed, oversized, out-of-range, contradictory, cross-message, ungrounded,
or semantically unsafe output becomes `analysis_unavailable` or `needs_review`
and causes zero provider follow-up, Graph/Office/reminder operation, lifecycle
transition, or other mutation. A schema-valid span is not sufficient evidence
of semantic safety. UI explanations are reconstructed transiently from validated
templates and evidence; provider rationale is neither evidence nor durable
state.

### Network, consent, and credential policy

External network policy requires direct valid-TLS public HTTPS using system
trust and hostname verification. Userinfo, query, fragment, IP-literal,
private, loopback, link-local, multicast, reserved, documentation, benchmark,
carrier-grade NAT, unspecified, and cloud-metadata destinations reject. There
is no custom CA, insecure option, or certificate bypass. The application
resolves all external addresses under its own policy, validates every answer,
connects only to an allowed answer, and revalidates the connected peer address.

Redirects are disabled at the client and every redirect response rejects.
Ambient `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, system proxy, PAC,
WPAD, and provider-SDK proxy defaults are ignored. Authorization is constructed
only after final authority validation and is never forwarded after a redirect,
retry, endpoint edit, or DNS-policy failure. There is zero automatic retry after
any request that may contain mailbox content. A user-visible bounded replay must
revalidate consent, origin, model, and evidence before a new request.

Before any content crosses the process boundary, the user must see and consent
to the provider profile, exact authority, model label, transmitted field
categories, maximum context count, provider privacy and retention responsibility,
and whether the boundary is a separate local process or an external service.
Provider profile, authority, adapter path, model label or digest, transmitted
fields, schema or policy version, or provider-capability drift invalidates
consent. Endpoint changes never inherit consent. A credential may be checked
only with a content-free request where supported; replacement requires the same
check, deletion or secure-store loss disables active use, and neither action
triggers replay. Failure sends no mailbox content. The credential is never returned to the add-in, command
line, diagnostics, logs, errors, repository, or package.

The settings surface is a closed, non-secret disclosure contract pinned as
`model-sensitivity-v1`. It shows the provider profile, exact endpoint, model
label, external-data disclosure, and every confidence or detection sensitivity
before provider use and whenever settings are reviewed:

- Request sensitivity is `low`, `standard`, or `high`; its default is the
  high-recall `standard` setting. A change offers bounded review-only replay.
- Deadline inference sensitivity is `explicit-only`, `standard`, or `high`;
  its default is `standard`. A change offers bounded replay, but never
  overwrites a user resolution automatically.
- Closure sensitivity is `conservative`, `standard`, or `high`; its default is
  `conservative`. A change offers bounded replay, and every result remains a
  possible closure.
- Review thresholds exist for each closed response claim type. Their safe
  `model-sensitivity-v1` default is `review_required`; G-MODEL must validate
  calibrated confidence buckets before any broader policy is eligible.

No settings change starts replay automatically. Every offered replay is
review-only, existing loops remain intact, and no automatic terminal, Graph,
Office, or reminder mutation follows a sensitivity or threshold change.

### Semantic, privacy, and degraded-mode policy

Provider content is hostile input. No tool surface exists. Instructions in mail
or provider output cannot select data, issue network calls, alter policy or
scopes, authorize mutations, or make a loop terminal. Automatic eligibility is
unavailable until ADR-011 and the applicable G-AUTO gate independently pass;
deterministic positive constraints and an independently held adversarial suite
remain mandatory. Provider, model digest or label, schema, prompt, or policy
drift invalidates automation calibration and permits only an explicit
review-only replay.

Prompts, requests, responses, outputs, rationales, transcripts, and embeddings
are never persisted. Provider-SDK logging is disabled, and SDK/provider errors
are sanitized at the adapter boundary before they enter application exceptions.
Diagnostics are limited to `provider_disabled`, `analysis_unavailable`,
`timeout`, `transport_policy_rejected`, `response_too_large`, `invalid_utf8`,
`invalid_json`, `invalid_schema`, `invalid_evidence`, and `semantic_rejected`,
plus bounded content-free counters. They contain no authority, host, path, key,
header, body, prompt, response, model text, or source identifier. G-PRIV must place synthetic
canaries in every content and credential position and find zero occurrences in
application-controlled durable, temporary, browser, crash, diagnostic, package,
installer, CI, and repository artifacts. The exact enabled provider request is
the sole test-time exception.

If no provider is selected, the provider is unavailable, validation fails, the
secure store is unavailable, or network/consent policy rejects, existing loops
remain intact. Deterministic reduced behavior may continue, but uncertain work
is visibly `analysis_unavailable`; no plaintext secret fallback, content
persistence, automatic retry, provider substitution, or mutation is allowed.

## Consequences

OWN-08 is faithfully accepted without prematurely shipping it. Local and hosted
provider modes have exact future validation targets, but P0-WI-10 passes no
provider, privacy, security-audit, or release gate; every named gate remains
unrun. It completes no acceptance
scenario or acceptance criterion. AS-12, AS-16, and AS-22 remain unpassed. No
provider capability is enabled or advertised, no network origin is configured,
and no credential or mailbox content is accepted or transmitted.

AS-16 is not owned by ADR-007 alone: its recovery path also depends on ADR-004
and ADR-005 plus OL-SYNC-012/013 and G-MAIL/G-STATE/G-PRIV. P0-WI-10 records
that dependency and does not claim the persistent retry state or recovery path.

Provider privacy and retention remain the selected provider's responsibility.
OpenLoops can minimize and disclose transmission but cannot promise deletion by
the provider, removal from OS paging, or protection from same-user malware,
administrators, endpoint tooling, snapshots, or platform crashes.

## Verification

P0-WI-10 must be graded against the exact contract, requirement, scenario, gate,
support, governance, privacy, protected-state, network, consent, error, semantic,
and zero-runtime/zero-claim inventories. Deterministic negative cases must prove
that any unknown profile or field, looser bound, alternate origin/path, implicit
proxy, redirect, weaker TLS/DNS/address rule, arbitrary header, credential leak,
persisted model material, mutation authority, completed scenario, passed gate,
or advertised capability fails closed. A fresh security/privacy/adversarial
checker is required before the work item can close; its prompt, transcript, and
model output are not repository evidence.

## Amendment (2026-09-08)

The original `response_contract.maximum_wall_time_seconds: 60` conflated two
different bounds. reqwest's blocking client applies one configured timeout to
both the header wait and every individual body `read()` call; a slow model
that trickles occasional keep-alive bytes re-arms that timeout on each byte,
so a non-streaming request could in practice run far longer than 60 seconds
with no single `read()` ever exceeding it. This was found with a debugger
attached to the live app: a scan worker sat inside one body read for over ten
minutes on one conversation, and Stop had no effect until that read finally
returned, because the abort check lived only between conversations.

The contract now separates the two bounds:

- `maximum_idle_read_seconds: 60` is the existing per-read idle guard
  (`https_client`'s `.timeout(...)`), covering both the header wait and any
  single body read that receives nothing at all.
- `maximum_wall_time_seconds: 150` is a new, independent hard cap on the
  whole request -- from `send()` to the last body byte -- enforced in
  application code (`openloops-inference::provider::read_body`), since the
  transport has no native concept of total request wall time. The actual
  (potentially long-blocking) reads happen on a background thread while the
  caller polls every 250 milliseconds, so the bound is observed within about
  one poll tick even while the connection is completely silent, not only
  while it is slowly trickling bytes. The same check point is what a Stop
  click aborts through (`ProviderError::Cancelled`), so Stop now takes
  effect within about a second rather than only between conversations.

A slow reasoning model working through a large conversation can still hit 150
seconds; that is a failed conversation, not a failed scan, and the rest of
the scan continues. This is a mechanical correction to an inaccurate bound,
made during code review of the wall-time fix; per `AGENTS.md`, the product
owner should still review it before the next release.
