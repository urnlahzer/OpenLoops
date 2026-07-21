# ADR-010: Add-in bridge

- **Status:** Accepted
- **Work item:** P0-WI-13
- **Owner decisions:** OWN-01
- **Blocking gates:** G-ADDIN, G-PRIV
- **Decision date:** 2026-07-21

## Context

OWN-01 already selects a Windows-first, local, single-user companion with an
Outlook add-in front end; no remote container, NAS, or headless MVP. The add-in
is a sandboxed Office web add-in with no synchronization authority, token
cache, or durable mailbox-derived state, and its communication with the
companion remains disabled until G-ADDIN and this ADR establish the packaged
client and bridge contract. This ADR records that boundary without selecting
or implementing the underlying transport.

The executable contract is `contracts/addin/bridge-boundary.json`. It pins the
security envelope any later transport MUST satisfy, and it pins as
`unresolved_pending_G-ADDIN` every fact the implementation-plan section 5
prototype spike and the product-spec G-ADDIN gate text have not yet proven:
exact transport selection, loopback host/port, certificate mechanics,
discovery mechanism, and per-client behavior. P0-WI-13 accepts none of these
records for runtime and makes no listener, pipe, certificate, pairing, or
manifest request.

## Decision

### Transport boundary

Named pipes are preferred for companion-internal and native-UI traffic. A
same-user loopback web bridge is the sole candidate transport for the Office
add-in, and only if the runnable packaged G-ADDIN spike proves asset hosting,
certificate/network, discovery, and bootstrap boundaries on every advertised
client. OpenLoops never assumes a named pipe is reachable from Office.js and
never assumes Graph delegated scopes authorize add-in operations. No remote,
hosted, or LAN-reachable bridge exists; the bridge is a same-user loopback
listener only, never a remotely reachable service. Exact loopback host, port,
certificate mechanics, discovery mechanism, and per-client behavior remain
`unresolved_pending_G-ADDIN`; this ADR pins no guessed value for any of them.

### First-pair bootstrap

A first pairing binds, in one explicit-user-verifiable action, the expected
packaged companion, the Office add-in origin and client, the current OS user,
an opaque account reference, a single-use nonce, and an explicit expiry. No
bearer, session, or pairing value may appear in a URL parameter, Office
roaming settings, `localStorage`, or repository configuration. Two concurrent
first-pair attempts on the same port resolve to at most one bound pairing; the
loser observes a fail-closed rejection rather than a second silent binding. A
replayed nonce is rejected once its single use is consumed or its expiry
passes. Pairing verification is rate- and attempt-bounded so that brute-forcing
the verification value fails closed rather than degrading into an open
window. Malicious local pages issuing loopback requests before pairing
completes are rejected because no unpaired command exists: every command
requires a bound, live session.

### Least-authority sessions

A paired session is least-authority and command-scoped: it authorizes only the
specific companion commands the add-in surface requires (review, evidence
navigation, settings, status), never generic or synchronization authority. Every
session secret is held in memory only and is never written to the database, a
DPAPI blob, disk, or a log. A session rotates on reconnect and is revoked on
account change; a session bound to a prior account or a stale companion
generation is rejected, not silently reused. This is a same-user boundary
only: it defends against a malicious local web page or a different local
account, not against code already running as the same authenticated OS user.

### Network, CSRF, and certificate boundary

Any selected mechanism MUST validate exact Host and Origin values against a
closed allowlist, apply an explicit CSRF defense independent of cookies, bind
only to loopback addresses (no LAN, wildcard, or `0.0.0.0` bind), enforce
request rate and size limits, expire sessions, and reject DNS-rebinding
attempts that resolve an external-looking hostname to a loopback address after
the fact. It MUST NOT install machine-wide certificate trust or a
general-purpose trusted root with a retained signing key. Any custom loopback
certificate or trust is current-user only, limited to the exact tested
loopback names, uses a non-exportable private key ACL'd to the user and
application, has explicit issue, rotation, expiry, and replacement behavior,
and is completely removed on disconnect and uninstall. Failure of any trust
cleanup step or client path blocks the add-in rather than leaving orphaned
trust behind.

### Review-link activation

A review link carries exactly the ADR-009 payload: one random, non-secret,
opaque loop handle and no authority. Activation is an authenticated,
per-user, account-bound resolution of that handle against the current loop
state; the link itself grants nothing and is never a credential. Resolution
never widens ADR-009's prohibited list: no bearer, session, or pairing token,
no Graph or Office ID, no account identifier, no mailbox content, and no raw
evidence link may travel with, or be inferred from, the link. An unsupported
To Do or Outlook client omits the link or shows a fixed safe-fallback
instruction; it never falls back to embedding authority in the link to
compensate.

### Content boundary

No mailbox content, identifier, token, or URL carrying authority may reach
browser `localStorage`, IndexedDB, a service-worker cache, a URL, console
output, analytics, or a crash report. This applies to every add-in surface,
not only the review link.

### Native fallback and gate-failure routing

When the bridge or add-in is unavailable — disconnected, uninstalled, an
unsupported client, or before G-ADDIN passes — the companion's native
status/review/recovery surface is the fallback, matching the support-matrix
native-companion-fallback rule. If G-ADDIN fails outright, the PRD-required
Outlook add-in capability is blocked and routed to OWN-01 for a product-owner
decision; native UI is never a silent substitute for the blocked add-in
capability, and no claim advertises add-in support in its place.

### Privacy boundary

This ADR introduces no new persisted database record. Any future pairing
root remains the already-reserved `pairing-root.dpapi` blob under ADR-005's
DPAPI boundary, owned by ADR-010 with ADR-005's storage constraints; session
secrets remain memory-only and are never a database record. Introducing any
future persisted bridge record requires a separate ADR-PRIV-001 revision;
this ADR approves none.

## Consequences

P0-WI-13 accepts only this disabled decision contract. ADR-010 alone advances
from planned to accepted. Bridge runtime, a deployed manifest, an active
listener or pipe, an issued certificate, a created pairing, a requested
permission, a support claim, capabilities, acceptance criteria, scenarios, and
gates remain empty, inactive, or unrun. The exact transport, loopback
host/port, certificate mechanics, discovery mechanism, and per-client
behavior remain owned by the G-ADDIN prototype spike; this ADR pins only the
security envelope any selected mechanism must satisfy.

## Verification

The deterministic checker pins the complete manifest and accepted ADR, checks
exact inventories, the closed transport/bootstrap/session/certificate/review-
link/content/fallback catalogs, and reconciles ADR-005's pairing-root
ownership, ADR-006's Office/link boundary, ADR-009's review-link deferral,
the governance registry, the support-matrix client floor, and build-skeleton
inactivity. Synthetic mutations must reject machine-wide certificate trust, an
exportable private key, a retained general-purpose signing root, a
bearer/session/pairing token in a URL or review link, a secret in
`localStorage` or roaming settings, a LAN or wildcard bind, a missing
Host/Origin/CSRF rule, a session without rotation or revocation, a pairing
without an explicit user action or without a nonce and expiry, a review link
carrying authority, mailbox content in browser storage, any runtime
activation, a gate-passed or capability-enabled claim, fresh-checker
preapproval, and additive documentation contradiction. Fresh-context
security, privacy, governance, and adversarial judges are required for
closure; their prompts, transcripts, and output are not repository evidence.
