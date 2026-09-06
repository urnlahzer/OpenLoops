# Distribution and update

**Status:** ADR-012 decision contract accepted; runtime, registration, signing,
packaging, and all named gates remain inactive.

The primary safety property is that a shared OpenLoops Entra registration can
never be enabled by a successful sign-in, development convenience, or a
passing test; that an update can never be accepted unless its metadata is
independently signed, on the correct channel, unexpired, monotonically newer,
and matched exactly to a hash- and size-pinned package whose provenance ties
to an inspectable public source revision; and that uninstall always removes
every bridge trust artifact ADR-010 created rather than leaving it orphaned.
No mechanism this ADR pins is guessed ahead of G-RELEASE; exact signing
technology, certificate provider, update transport, and channel names remain
`unresolved_pending_G-RELEASE`.

| Threat | Required control | Fail-closed result |
|---|---|---|
| Malicious fork reuses the shared client ID or impersonates the publisher | Shared registration stays disabled until the complete governance-control list (publisher tenant/verification, owner recovery, incident procedure, BYO documentation for rejecting orgs) is satisfied; BYO remains the only enabled path | A fork or impersonation attempt cannot piggyback on an enabled shared registration that does not yet exist |
| Update downgrade to a vulnerable signed version | Monotonic anti-downgrade version policy; an older binary refuses to write a newer schema or ciphertext version | A validly signed but older-versioned update is refused regardless of signature |
| Update metadata swapped between channels | Update metadata is bound to an exact release channel | Metadata signed for one channel is refused by a client on another channel |
| Staged package replaced after verification | Atomic protected staging and replacement with a recovery path | A package substituted after verification but before atomic replacement is not the package that was verified, and replacement fails closed |
| Signing-key compromise with no rotation/revocation path | Trusted-key inventory with purpose, holder, and rotation/revocation procedure per key; update metadata signed independently of package signing | A compromised key is revoked without silently authorizing the other artifact class; metadata-signing compromise cannot forge a package signature and vice versa |
| Expired metadata replayed as current | Metadata carries an explicit expiry | Expired metadata is refused even if the signature validates |
| Provenance that cannot be tied to a source revision | Provenance must be verifiable to the exact public source revision that produced the package | A package without inspectable source-revision provenance is refused regardless of valid signatures |
| Elevation path smuggled into install or update | No installation or update step elevates privileges by default | An elevation request without a separate explicit justification and review fails closed |
| Supply-chain or lockfile tamper reaching a release | Dependency locks, license review, vulnerability policy, SBOM, and provenance are required for every release | A release missing a lock, SBOM, or provenance record is incomplete and blocked |
| Secret or real identifier entering a package, SBOM, or repository | Explicit per-artifact-class allowlists; canary and public-repository gates run at every required release point and never print a suspected value | A disallowed artifact or a canary hit blocks the release without disclosing the value |
| Uninstall leaving bridge trust or a certificate behind | Uninstall repeats disconnect cleanup and removes every local bridge trust, certificate, and protocol registration ADR-010 created | Orphaned trust after uninstall is a defect, not accepted residual state |
| Publisher-governance failure blocking the shared registration path | The complete governance-control list is required before first use of the shared registration; failure of any control keeps it disabled | A partially satisfied governance list never enables the shared path |
| `npm`/Cargo publish widened to ship an unreviewed package | `npm` stays private with an empty `files` allowlist; every crate stays `publish = false` in Phase 0 | A widened files list or an enabled publish flag fails the build-skeleton and distribution checks |

No package is signed, no update is published, no shared registration is
enabled, and no installer is built by this threat model. A failed or unrun
G-ID/G-RELEASE gate keeps the affected capability disabled and routes to the
product owner.
