# ADR-001: Runtime and self-hosting boundary

- **Status:** Accepted
- **Date:** 2026-07-19
- **Decision owners:** OWN-00, OWN-01; boundary consequences from OWN-02 and OWN-06
- **Requirements:** OL-GOV-001, OL-NFR-011, OL-NFR-012
- **Related gates:** G-ID, G-ADDIN, G-STATE, G-PRIV, G-SEC-AUDIT, G-RELEASE

## Context

OpenLoops needs delegated access to one user's Microsoft 365 data while keeping
the product source-buildable and avoiding a required OpenLoops-operated service.
Native OAuth callbacks, protected token/state storage, background execution, the
Outlook add-in bridge, installation, and updates are platform-sensitive security
boundaries. The research snapshot recommended a per-user desktop connector and
the product owner subsequently accepted the exact MVP boundary recorded here.

## Decision

The MVP is a Windows-first, unelevated, per-user desktop product for one signed-in
operating-system user and one Microsoft account in the commercial global cloud.
The desktop companion, background jobs, domain and application layers,
persistence, identity, and Graph/model adapters are implemented in pure Rust.

The Outlook add-in is a sandboxed Office web add-in and therefore uses the
supported Office.js/TypeScript surface. It owns no authentication cache,
synchronization authority, Graph mutation authority, or durable mailbox-derived
state. Its communication with the companion remains disabled until G-ADDIN and
ADR-010 establish the packaged client and bridge contract.

The MVP identity direction is a delegated public client using the system browser
and authorization code with PKCE. WAM is not an MVP dependency and cannot be
silently introduced as a fallback. Exact crates, registration, redirect,
authority, scope, refresh, cache, and account behavior belong to ADR-002,
ADR-003, and G-ID.

"Self-hosted" means user-installed, user-controlled, and runnable from public
source with a user-owned Entra registration and no required OpenLoops account or
service. It does not mean a remote browser, Docker container, NAS, LAN server,
headless daemon, hosted relay, central database, webhook receiver, or shared
multi-user installation.

The runtime MUST NOT use a client secret, application permission, tenant-wide
access, ROPC, implicit flow, embedded credential collection, device-code
downgrade, remote callback, or machine-account authority. Shared mailboxes,
multiple accounts, sovereign clouds, and personal Microsoft accounts are not in
the core enabled matrix. Personal accounts remain a separately gated target;
successful sign-in alone never enables them.

Minimized encrypted per-user local state is the approved baseline direction, but
no durable state is enabled by this ADR. ADR-PRIV-001 and ADR-005 must define the
complete field, source-variant, key, encryption, rollback, retention, deletion,
and recovery contracts, and G-STATE/G-PRIV must pass before persistence is
available. Broad public release additionally requires G-SEC-AUDIT and G-RELEASE.

## Consequences

- Cross-platform domain seams may be designed, but macOS and Linux are not
  advertised or release-tested MVP platforms.
- The companion may run in the signed-in user's session while the add-in is
  closed; it cannot claim coverage while stopped, sleeping, offline, or signed
  out.
- A native status/settings/recovery surface is required, but it is not a second
  task manager and cannot silently replace the PRD-required Outlook add-in.
- BYO registration is the source-build and development path. A shared project
  registration remains gated by ADR-012, publisher governance, signed
  distribution, and release controls.
- Failure of a mandatory gate keeps the affected capability disabled and returns
  any scope change to the product owner. It does not broaden permissions or
  redefine the MVP.

## Rejected alternatives

- **WAM-first MVP:** rejected by OWN-00; a future Rust-compatible adapter must be
  independently gated.
- **Remote/container/NAS/headless MVP:** rejected by OWN-01 because it changes
  callback, secret, multi-user, network, update, and storage boundaries.
- **Hosted confidential service or application permissions:** rejected for the
  MVP because it creates a central trust boundary and tenant-wide authority.
- **Native-only UI without the Outlook add-in:** not a silent fallback; failure
  of G-ADDIN requires a product-owner decision.
- **Plaintext or best-effort secret/state storage:** prohibited. Secure-store
  absence yields session-only or fail-closed behavior.

## Verification

- `P0-ADR-OWN-001` proves OWN-00 through OWN-10 are present exactly once and
  route to ADRs and gates.
- `P0-CAP-001` proves all gated outward capabilities default disabled, are not
  advertised, and name a product-owner failure path and safe fallback.
- `P0-TRACE-001` proves the exact ADR and gate inventories and rejects unknown or
  missing references.
- This ADR completes no product acceptance criterion and does not pass any
  Microsoft Graph, add-in, privacy, security, or release gate.
