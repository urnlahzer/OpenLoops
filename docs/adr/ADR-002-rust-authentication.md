# ADR-002: Rust authentication contract

- **Status:** Accepted
- **Date:** 2026-07-19
- **Work item:** P0-WI-05
- **Owner decisions:** OWN-00, OWN-02
- **Requirements:** OL-GOV-001, OL-AUTH-001–002, OL-AUTH-004–006,
  OL-NFR-005–006, OL-NFR-011–012
- **Blocking gates:** G-ID, G-PRIV; G-SEC-AUDIT before broad release

## Context

OWN-00 already selects a pure-Rust native companion and a system-browser
authorization-code flow with PKCE. OWN-02 already selects commercial-global
work/school accounts as the core validation target and keeps personal accounts
disabled until their complete matrix passes. This ADR records those accepted
consequences; it does not reopen them or claim that Microsoft identity behavior
has been validated.

The research establishes that a distributed native application is a public
client, cannot protect a client secret, and should use an external browser with
PKCE. It leaves the exact Entra authority/audience, loopback host/path and
IPv4/IPv6 behavior, token claims, refresh rotation, consent/admin behavior, and
protected-cache lifecycle to G-ID experiments with synthetic accounts.

## Decision

`contracts/identity/authentication-boundary.json` is the closed Phase 0 identity
decision contract. The sole MVP direction is a delegated public client using the
system browser, authorization code, PKCE S256, and a same-machine ephemeral
loopback callback. There is one pending transaction. State and the PKCE verifier
are fresh, high-entropy, memory-only, one-use values. Missing, mismatched,
duplicated, replayed, expired, cancelled, cross-process, or account-switched
callbacks fail closed and tear down the listener and ephemeral transaction.
A second same-process sign-in attempt is rejected without cancelling, replacing,
or mutating the incumbent transaction. An unsolicited or malformed callback
cannot replace or consume a valid pending transaction.

The implementation may not ship or silently fall back to WAM, an embedded
browser, device code, ROPC, implicit flow, client credentials, application
permissions, tenant-wide access, embedded credential collection, or any client
secret/private key. WAM remains a separately gated future adapter.

The callback binds only to the same machine, uses an operating-system-assigned
ephemeral port, accepts only the exact pending path and state, and shuts down
immediately after completion or failure. LAN binding, remote callbacks, wildcard
redirects, and listener reuse are prohibited. The choice between `localhost`
and `127.0.0.1`, exact registered path, IPv4/IPv6 behavior, and port/firewall
behavior remain `unresolved_pending_G-ID`; this ADR does not guess them. The
initial callback contract is query response mode over GET only. `form_post` is
prohibited unless a future owner-reviewed ADR revision and G-ID evidence add its
POST-body privacy and parsing contract. Fragments are prohibited. Every callback
must match the exact pending Host/authority and reject malformed, duplicate, or
mixed success/error parameters. Bounded unrecognized parameters are ignored as
OAuth requires, without logging, persistence, diagnostics, or exposure. Success
is exactly one code plus state and no error; failure is exactly one error plus
state and no code, with bounded optional error fields sanitized by G-ID.

The core validation target is exactly one commercial-global work/school account
and primary mailbox per signed-in OS user. Personal, guest, shared-mailbox,
sovereign-cloud, multiple-account, and cross-account inheritance paths stay
disabled or unsupported as recorded in the manifest. Every later fetch and
mutation must bind to an explicit opaque internal account reference.

No token persistence exists in this work item. Authorization codes, tokens,
PKCE verifiers, cookies, authorization headers, and real account/tenant
identifiers may not enter command-line arguments, repository files, fixtures,
logs, diagnostics, telemetry, application-controlled browser storage, or
plaintext files. The protocol necessarily carries opaque random `state` in the
system-browser authorization URI and returns opaque `code` plus the exact state
in the loopback callback URI. Those are the only URL exceptions: they are
transient, contain no content/account/return-URL data, and are never copied,
logged, diagnosed, persisted, or retained after terminal handling.
ADR-005 now establishes the protected token/cache storage contract, while its
runtime and G-ID/G-PRIV evidence remain unimplemented. Absence of a usable
protected store permits only ephemeral non-mutating/session-only behavior with
no plaintext fallback.

Session reuse (2026-09-10): the access token is held in process memory for the
token lifetime and reused across the connection check, mail load, scan and
reminders; never persisted; cleared on Forget, client-ID change, a 401, and
exit.

Disconnect stops new identity work, clears ephemeral state, removes
application-owned cache material where the future adapter supports it, and
deletes account-bound application state under ADR-005. It is never described as
global logout, token/session revocation, browser-cookie removal, or tenant
consent revocation. If application-controlled protected-cache deletion fails,
identity and reconnect remain disabled until owner-approved recovery completes;
the UI discloses both incomplete local cleanup and residual Microsoft
browser/session/consent state.

The manifest records exact reviewed direct-crate candidates for typed OAuth,
redirect-disabled HTTP, bounded async I/O, strict URL and callback parsing, and
hardened system-browser launch. They are
`selected_not_activated`: no OAuth/network dependency or code is added to the
workspace by this ADR. Exact Cargo activation and its full transitive lock,
license/security review, redirects-disabled HTTP construction, and Windows
contract tests are mandatory before implementation. Git dependencies, wildcard
versions, ad hoc OAuth, and ad hoc cryptography are prohibited.

ADR-003 separately owns incremental OIDC/Graph scopes, processing folders, and
Office manifest permissions. ADR-005 separately owns protected token/key state.
ADR-012 separately owns shared registration and distribution. This ADR requests
no permission and contacts no service.

## Consequences

- Identity, Graph, and account-dependent capabilities remain disabled.
- G-ID, G-PRIV, and G-SEC-AUDIT remain unrun.
- Synthetic G-ID tests must cover callback injection, replay, duplication,
  expiry, port races, concurrency, account selection, consent/admin/revocation,
  cache loss, disconnect, and secure-store failure without recording values.
- PKCE does not authenticate the installed binary; signed distribution and
  publisher governance remain ADR-012/G-RELEASE work.
- Same-user malware, administrators, browser/platform state, paging, crash
  capture, and compromised dependencies remain residual risks.

## Verification

P0-WI-05 is graded by P0-AUTH-INVENTORY-001,
P0-AUTH-FLOW-001, P0-AUTH-REDIRECT-001, P0-AUTH-ACCOUNT-001,
P0-AUTH-TOKEN-001, P0-AUTH-CONCURRENCY-001,
P0-AUTH-DISCONNECT-001, P0-AUTH-DEPENDENCIES-001,
P0-AUTH-CLAIMS-001, and P0-AUTH-FRESH-CHECKER-001. It completes no
acceptance criterion, enables or advertises no capability, requests no scope,
and passes no gate.
