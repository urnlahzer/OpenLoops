# Evidence identity and navigation threat model

## Boundary

This model covers the future path from encrypted `email_evidence_ref` to bounded
Graph refetch, transient evidence projection, and optional Outlook navigation.
P0-WI-09/P0-WI-09R are contract-only. Graph, Office, state, and navigation stay inactive
behind G-MAIL, G-ADDIN, G-STATE, and G-PRIV; calendar also requires G-CAL.

## Threats and required controls

| Threat | Control and fail-closed result |
|---|---|
| Cross-account/mailbox locator substitution | Compare opaque account and the exact purpose-separated ADR-005 mailbox-binding HMAC before any request; raw validated `tid`/`oid` claims never persist and mismatch makes zero network/Office call. |
| Default ID changes after move | Immutable preference on every ID-returning request; default ID is candidate-only; require G-MAIL move matrix. |
| Copy mistaken for original | Copy is always distinct; digest/conversation/folder matches never replace identity automatically. |
| Draft/send documentation conflict | Draft/compose creates no source; saved-sent observations are tested independently. |
| Archive/export/import drift | Treat the ordinary Archive folder as a gated same-primary-mailbox move, but project the separate in-place/online archive mailbox unavailable; export/import gets new identity and no inferred lineage. |
| Changed content shown as original | Require exact content/prefix/suffix anchors and valid Unicode range; mismatch freezes automation. |
| Digest guessing/oracle | Random per-account HMAC key, purpose separation, AEAD, no export/log/diagnostic, full-output comparison. |
| Unicode offset confusion | Closed numeric maps, versioned canonical profile/vectors, NFC scalar-index ranges, strict bounds, forbidden-scalar and unpaired-surrogate rejection, transient offset map. |
| Hostile HTML/link/prompt text | Exact inert WHATWG-fragment projection, closed block/quote rules, fetch no remote resources/attachment bytes, read no `href`, store no URL, execute no content instruction. |
| Different canonical bytes across implementations | One depth-first DOM event algorithm pins block and table-cell entry/exit LF events, outermost quote segmentation, whitespace finalization, and sibling/nested/table synthetic vectors. |
| Inline-only attachment name silently skipped | Enumerate the exact bounded metadata-only attachment endpoint for every accepted message regardless of `hasAttachments`; require terminal paging and never fetch bytes. |
| Inactive runtime gains hidden transport | Exact product/package-script execution-surface inventory plus aggregate fingerprint covers source, generator, config, tests, schemas, Cargo manifests/lock, and the full npm lock graph. Exact import/dependency/script allowlists precede secondary signatures; Prettier disables implicit configuration and EditorConfig discovery; added Node/Rust transport, dynamic loading, process launch, Office/Graph access, or arbitrary dependency fails the zero-runtime claim. |
| Incomplete fallback mistaken for singleton | Only terminal enumeration across the complete authorized bounded universe may classify zero/one/many; every cap or partial page is ambiguous/unavailable and can never offer replacement. |
| Stale Office item ID | Treat EWS ID as transient; bind through Graph; require `displayMessageFormAsync` callback success; cached/client/not-found failure becomes navigation unavailable. |
| Broader ID-translation permission | Prohibit `translateExchangeIds`; request no directory permission; route scope change to product owner. |
| Malicious `webLink` | Fetch after identity validation; parse once; reject controls/whitespace/userinfo/nondefault port/IP/trailing-dot/punycode/origin drift; validate only the initial exact HTTPS origin; top-level external navigation with no application redirect, iframe, rewrite, persistence, log, or token. |
| Raw ID/content leakage | Exact IDs only in AEAD locator fields; selected fields/IDs/links/offsets/anchor inputs stay bounded in memory; canary scan artifacts. |
| Missing evidence treated as completion | Preserve abstract state, show unavailable, freeze dependent automation, infer neither closure nor failure. |
| Wrong automatic reanchor | Zero/multiple candidates remain unavailable/ambiguous; one candidate still requires explicit user confirmation. |

## Residual risk and gates

Service/client behavior can differ by mailbox, cached mode, retention policy, and
version. Paging, Office/WebView/browser caches, same-user malware, administrators,
endpoint tools, backups, snapshots, and platform crashes are not claimed erased.

G-MAIL must test moves, copies, archive, delete, draft/send, saved-sent timing,
fallback ambiguity, selected fields, preference propagation, and links using
synthetic disposable mailboxes. G-ADDIN tests EWS/REST conversion, stale IDs,
display failure, account switch, and every claimed client. G-PRIV places unique
synthetic canaries in each content/ID/link/error position and requires zero in
application-controlled durable, diagnostic, package, installer, CI, and repo
artifacts. No raw capture, address, payload, screenshot, log, transcript, or
model output is retained as evidence.
