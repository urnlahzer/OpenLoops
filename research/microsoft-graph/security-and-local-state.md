# Security, privacy, and local state

## Core correction

"No sensitive database" is not a meaningful security boundary. A reliable
delegated Graph connection must retain security-sensitive authentication and
synchronization state somewhere.

The defensible design objective is:

> By default, OpenLoops does not intentionally persist raw Microsoft 365
> content in its own state store. It persists the minimum authentication and
> synchronization state required for operation, protected by the current
> user's operating-system security boundary.

The project must not claim that no sensitive information is stored locally.

## Proposed data flow

```text
Microsoft Graph mail
        |
        v
bounded in-memory extraction
        |
        v
candidate open loop + user confirmation
        |
        v
Microsoft To Do write
        |
        v
encrypted opaque idempotency/checkpoint record
```

Any persistent candidate queue, description, summary, embedding, prompt, or
classification output is derived Microsoft 365 content and changes the privacy
promise.

## Minimum persistent state

| Store | Minimum contents | Sensitivity |
|---|---|---|
| MSAL/WAM cache | Account metadata, access/refresh-token material, MSAL state | Credential-equivalent |
| OS keystore | State-encryption key when a separate application key is needed | Credential-equivalent |
| Account record | Random local account reference, cloud enum, enabled features | Private metadata |
| Mail checkpoint | Selected folder ID, opaque next/delta URL, query fingerprint, last complete time | Private synchronization state |
| To Do checkpoint | Selected list ID, opaque next/delta URL when used, query fingerprint, last complete time | Private synchronization state |
| Idempotency map | Keyed hash of source immutable message ID, encrypted destination task ID, operation state and expiry | Correlatable private metadata |
| Job health | Error class, retry time, coarse health status | Must exclude raw request/error bodies and identity |
| UI session | Per-launch high-entropy secret | In-memory secret; do not persist |

The state may be an authenticated encrypted file/blob rather than a relational
database. Its format does not change its sensitivity.

## Content not persisted by default

- Outlook message bodies and body previews.
- Attachment bytes.
- Subjects and participant addresses.
- Raw Graph request or response bodies.
- Microsoft To Do titles, bodies, checklist text, or linked-resource content.
- Derived summaries, embeddings, prompts, or candidate descriptions.
- Browser local storage, service-worker caches, or offline UI content.
- Full Graph errors or diagnostic bundles containing request URLs.

Selected content will still exist transiently in process memory. OpenLoops
cannot guarantee removal from operating-system paging, same-user malware,
administrator/root access, endpoint security products, hardware/VM snapshots,
or platform crash behavior.

## Threats and required controls

| Threat | Required control |
|---|---|
| Malicious fork reuses the public client ID | Signed official distribution, verified publisher, BYO option, clear documentation that PKCE does not authenticate the binary |
| Authorization-code interception or callback injection | PKCE S256, high-entropy state, exact callback/path, ephemeral loopback listener, one pending transaction, immediate shutdown |
| Token theft from disk | WAM or OS-protected MSAL cache, strict per-user ACL, no plaintext fallback, no token logging |
| Same-user malware or compromised dependency | Explicit threat-model limitation, least privilege, signed updates, dependency review, SBOM and provenance |
| Loopback UI takeover or DNS rebinding | Prefer authenticated native IPC; otherwise per-launch secret, exact Host/Origin validation, CSRF defense, restrictive CSP and loopback-only binding |
| Hostile message/task content or prompt injection | Treat Graph content as untrusted data; never execute embedded instructions, render raw remote HTML, or load remote resources |
| Logs, crash dumps, telemetry or support bundles | Structured allowlisted logging, no raw bodies/URLs, no automatic uploads, synthetic canary scanning |
| Scope creep | Per-feature incremental consent and graceful behavior when consent is denied |
| Cross-account or cross-tenant confusion | Stable internal account references and explicit account selection for every mutation |
| Duplicate writes after retry/crash | Atomic checkpointing and idempotency records committed before cursor advancement |
| Throttling or outage | Honor `Retry-After`, bounded exponential backoff with jitter, stale-state UI and circuit breaking |
| Revoked consent or Conditional Access change | Stop access, surface reconnect/admin state, and never downgrade to a weaker auth flow automatically |

## OS storage boundary

An OS keystore protects data at rest; it is not a sandbox against code already
running as the same user or with administrator/root privileges.

- WAM and Windows protection are the preferred first implementation boundary.
- macOS Keychain can support a later desktop implementation after accessibility,
  backup, and lifecycle behavior are tested.
- Linux persistent login requires a supported and unlocked Secret Service or
  LibSecret-compatible store.
- If secure persistence is unavailable, OpenLoops should fail closed to
  session-only operation rather than silently write a plaintext cache.
- Containers and NAS volumes add host-admin, snapshot, backup, UID, and
  cross-user risks that cannot be solved merely with file mode `0700`.

Sources:

- [MSAL.NET token-cache serialization](https://learn.microsoft.com/en-us/entra/msal/dotnet/how-to/token-cache-serialization)
- [MSAL Node cache extensions](https://learn.microsoft.com/en-us/entra/identity-platform/msal-node-extensions)
- [Microsoft refresh-token behavior](https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens)
- [Windows DPAPI](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)

## Disconnect semantics

Disconnect should:

1. Remove the account from the application-owned MSAL cache where supported.
2. Delete all account checkpoints, mappings, UI sessions, and subscription
   secrets owned by OpenLoops.
3. Best-effort delete any future Graph subscriptions.
4. Explain how the user or administrator can revoke consent or sessions in
   Microsoft portals.

Disconnect must not be described as global logout or universal revocation.
Browser cookies, WAM/broker state, consent grants, access tokens, and old
refresh tokens can have independent lifecycles.

## Release gates

- No client secret, certificate private key, ROPC, implicit flow, embedded
  credential collection, or application permission in the desktop product.
- Feature-to-scope manifest verified against token `scp` claims and Graph calls.
- No scope escalation without a separate user action and explanation.
- Token-cache protection verified on every supported OS; no plaintext fallback.
- Cross-user, backup/restore, and second-account state isolation tests.
- Local UI authentication, CSRF, Host/Origin validation, CSP, and loopback-only
  binding tests.
- Synthetic canaries prove content, credentials, headers, delta URLs, and IDs do
  not enter logs, telemetry, temp files, application-controlled crash artifacts,
  release archives, installers, or CI artifacts.
- Signed releases and updates, locked dependencies, SBOM, provenance, and a
  dependency-compromise response process.
- Clear documentation that local cleanup is not global revocation.
