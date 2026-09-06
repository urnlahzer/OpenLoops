# Microsoft Graph connection research

**Snapshot date:** 2026-07-18

**Status:** Research snapshot complete; product decisions recorded 2026-07-19

**Scope:** Connecting a public, local-first OpenLoops application to Outlook
Mail and Microsoft To Do through Microsoft Graph

This package memorializes the research as it stood on the snapshot date. It is
not itself the approved architecture or product specification. The accepted
decisions are recorded in [product decisions](product-decisions.md) and the
current normative documents under `docs/`; disposable-tenant gates still resolve
unverified implementation contracts.

## Documents

- [Connection options and provisional recommendation](connection-options.md)
- [Security, privacy, and local state](security-and-local-state.md)
- [Validation plan and Microsoft documentation conflicts](validation-plan.md)
- [Product decisions](product-decisions.md)

## Research method

The research used three independent lanes:

1. Microsoft identity, OAuth, Entra registration, and consent.
2. Outlook Mail and Microsoft To Do permissions, APIs, and synchronization.
3. Local-first deployment, token storage, privacy, and threat modeling.

Three separate adversarial reviews attempted to refute the findings from
identity, API, and security perspectives. A final synthesis evaluated the
research and the critiques together. Claims were classified as:

- **Verified:** supported by current Microsoft or IETF documentation.
- **Inference:** an architecture or product recommendation derived from the
  verified facts.
- **Unresolved:** documentation is contradictory or runtime behavior must be
  confirmed with synthetic accounts in a disposable tenant.

## Provisional synthesis

The research snapshot's strongest starting point was a per-user desktop
connector using delegated Microsoft Graph permissions, a public-client Entra
registration, WAM or system-browser authorization code with PKCE, protected
local token state, and outbound polling rather than a public webhook. The owner
subsequently selected a pure-Rust system-browser PKCE implementation; WAM is a
future gated adapter, not an MVP dependency.

The final research review recommended a Windows-first release to reduce the
initial security and packaging surface. The product owner accepted that boundary
on 2026-07-19, subject to the security and capability gates in the specification.

### Confidence at the snapshot date

| Area | Confidence | Reason |
|---|---:|---|
| Delegated public-client identity architecture | High | Stable Microsoft and OAuth guidance |
| Outlook Mail permission and delta design | High | Consistent v1.0 endpoint documentation |
| Minimal local-state architecture | High as a design | Implementation and OS behavior still require testing |
| Basic delegated Microsoft To Do access | Medium-high | Ordinary read/write endpoints are documented |
| To Do delta and app-only mutation | Medium-low | Microsoft documentation contradicts itself |
| Centrally operated OpenLoops registration | Medium | Publisher, consent, abuse, and governance work remains |
