# ADR-012: Distribution and registration

- **Status:** Accepted
- **Work item:** P0-WI-15
- **Owner decisions:** OWN-00, OWN-01, OWN-02
- **Blocking gates:** G-ID, G-RELEASE
- **Decision date:** 2026-07-21

## Context

OWN-00/01/02 already select delegated public-client identity, a Windows-first
unelevated single-user companion, and BYO registration as the source-build and
development path (ADR-001, ADR-002, ADR-003). ADR-001 explicitly defers: "A
shared project registration remains gated by ADR-012, publisher governance,
signed distribution, and release controls" to this ADR; it consumes
OL-NFR-011 only to preserve the source-buildability boundary and neither
implements nor completes it. Implementation-plan section 10 item 13 assigns
ADR-012 "signed anti-rollback update/provenance and BYO vs shared Entra
registration gates"; the Phase 7 deliverables list the signed per-user
installer, independently signed update metadata, trusted-key rotation and
revocation, monotonic anti-downgrade policy, channel binding, expiry, package
hash/size, atomic protected staging/replacement, recovery, provenance-to-
source-revision verification, no elevation unless separately justified,
dependency locks, license review, vulnerability policy, SBOM, provenance,
package/release allowlists, and rollback/recovery design this ADR pins.
Product-spec OL-NFR-010 (release integrity), OL-NFR-011 (source-buildability),
OL-NFR-012 (local-state security posture), and the G-RELEASE gate text state
the release-integrity, fresh-machine source-buildability, and audit
prerequisites this ADR closes without satisfying. The research synthesis's
"Entra registration and distribution requirements" records the complete
development rules and the complete governance-control list required "before a
shared production registration." ADR-PRIV-001 fixes the diagnostics allowlist
at empty and names this ADR as the one any future diagnostic export requires
alongside G-PRIV, G-RELEASE, and G-SEC-AUDIT. ADR-005 fixes the exclusive
copy-validate-encrypt-verify-swap migration state machine, the rule that an
older binary refuses to write a newer schema or ciphertext version, and the
R-21/R-22 critical risk rows ("Signed update is downgraded, manifest-swapped,
or staged package replaced"; "State/crypto migration crash or old binary
corrupts newer data") this ADR's update-trust chain must not weaken. ADR-010
fixes that any bridge certificate or trust "is completely removed on
disconnect and uninstall" and that failure of any trust-cleanup step blocks
the add-in rather than leaving orphaned trust behind; this ADR's uninstall
boundary must preserve that rule. AGENTS.md fixes the build/release artifact
allowlist, the `npm pack --dry-run` inspection rule, and the public-repository
canary rule that never prints a suspected value; this ADR's artifact boundary
must not weaken any of them.

This ADR closes that deferral: it records the two-registration model, the
update trust chain, the release-artifact boundary, the installer and uninstall
boundary, and the diagnostics rule without signing, packaging, registering, or
publishing anything.

The executable contract is `contracts/distribution/registration-boundary.json`.
P0-WI-15 signs no package, publishes no update, registers no shared Entra
application, builds no installer, and completes no acceptance criterion or
scenario.

## Decision

### Two-registration model

BYO public-client registration is the only enabled Phase 0/source-build path.
Its tracked configuration is placeholder-only: no real client ID, tenant ID,
or other Microsoft identifier appears in the repository, and no secret ever
appears on a command line or in tracked configuration. A shared OpenLoops
registration is a separate, disabled, gated future option. It may not be
enabled by a successful sign-in, development convenience, or a passing test;
it requires every one of the following governance controls, matching the
research synthesis exactly, before first use:

- a dedicated organizational publisher tenant and a verified domain;
- Microsoft publisher verification;
- accurate name, logo, homepage, privacy statement, terms, and support
  contact;
- at least two controlled registration owners and an owner-recovery
  procedure;
- exact registered redirects with no unused platform configuration;
- incremental consent and plain-language permission explanations;
- support for tenants that disable user consent or require admin approval;
- sign-in/consent-failure monitoring without message or task content
  collection;
- a malicious-fork/client-ID-abuse and incident-response procedure;
- signed official builds and updates with published provenance; and
- BYO registration documentation for organizations that reject the shared
  application.

PKCE does not authenticate the binary requesting a token; it authenticates
only the authorization-code exchange. This limitation is preserved regardless
of registration mode and is never represented as solved by registration
governance, code signing, or any other control in this ADR.

### Update trust chain

Update metadata is signed independently of package signing, so compromising
one signing key does not silently authorize the other artifact class. A
trusted-key inventory records every signing key's purpose, holder, and
rotation/revocation procedure; a revoked or rotated key can no longer produce
metadata or a package the client accepts. Version policy is monotonic and
anti-downgrade: an older binary refuses to write a newer schema or ciphertext
version, and the client refuses to install update metadata whose version is
not strictly greater than the currently installed version, regardless of
signature validity. Update metadata is bound to an exact release channel; a
client on one channel never accepts metadata signed for another channel.
Metadata carries an explicit expiry, and expired metadata is refused even if
otherwise validly signed; a valid signature is never a substitute for
freshness. Metadata pins the exact package hash and exact package size; a
package that does not match both is refused before any part of it is applied.
Staging and replacement are atomic and protected: a downloaded package is
verified completely before being staged, staging occurs in a location the
current build cannot yet execute or the current data cannot yet reach, and
replacement of the running installation is a single atomic step with a
recovery path if it does not complete. Provenance must be verifiable to the
exact public source revision that produced the package; a package that cannot
be tied to an inspectable source revision is refused regardless of valid
signatures. No installation or update step elevates privileges by default;
any future elevation requires a separate, explicit justification and its own
review, not an implicit escalation folded into this ADR.

### Release-artifact boundary

Every artifact class released or packaged (installer, update metadata, SBOM,
provenance attestation, release notes) has an explicit allowlist; an artifact
outside its class's allowlist is rejected before release rather than filtered
afterward. An SBOM and a provenance attestation are required for every
release; a release without either is incomplete. Source maps and generated
diagnostic artifacts are prohibited in any release, package, or installer
artifact, matching ADR-PRIV-001's empty diagnostics allowlist and AGENTS.md's
source-map prohibition. `npm` and Cargo publishability remain disabled in
Phase 0: `package.json` stays `"private": true` with an empty `files`
allowlist, and every workspace crate stays `publish = false`; enabling either
is a distribution decision this ADR does not make. The canary and
public-repository gates run at every required release point (staged commit,
push, and package dry run) and never print a suspected secret or identifier
value when they fail; a failing gate blocks the release rather than being
silenced or bypassed.

### Installer and uninstall boundary

The installer is per-user and unelevated, matching ADR-001's runtime
boundary; it does not require or request administrator rights by default.
Uninstall is complete: it repeats disconnect cleanup where possible and
removes every local bridge trust, certificate, and protocol registration
ADR-010 created, leaving no orphaned trust behind, matching ADR-010's
disconnect-and-uninstall certificate-cleanup rule. Uninstall discloses its
residual-risk boundary rather than claiming physical erasure: OS paging,
backups, snapshots, and endpoint-security exposure are disclosed limitations,
matching ADR-005's residual-risk framing and ADR-PRIV-001's no-physical-
erasure rule. Uninstall performs zero Microsoft Graph mutation and never
bulk-deletes a Microsoft artifact; any existing Microsoft reminder artifact is
handled per the user's own disconnect choice, not silently by the uninstaller.

### Diagnostics rule

The diagnostic-event allowlist stays empty. Any future diagnostic export
requires an exact content-free schema plus this ADR, G-PRIV, G-RELEASE, and
G-SEC-AUDIT; none of those four is satisfied by this ADR, and none may be
represented as satisfied by a partial subset of them.

### Unresolved mechanics

The exact signing technology, certificate provider, update transport, and
release-channel names remain `unresolved_pending_G-RELEASE`. This ADR pins the
envelope those mechanics must satisfy; it does not guess or pre-select any of
them, and no checker or manifest may fill in a concrete value for any of them
before G-RELEASE runs.

## Consequences

P0-WI-15 accepts only this disabled decision contract. ADR-012 alone advances
from planned to accepted; ADR-013 remains planned. Nothing is signed, packaged,
published, registered, or installed. No update mechanism exists. G-ID and
G-RELEASE remain unrun, and every acceptance criterion or scenario that
depends on either gate remains unpassed. ADR-001's deferred wording is
satisfied without ADR-001 being reopened; ADR-012 alone owns the shared-
registration governance list, the update trust chain, the release-artifact
boundary, the installer/uninstall boundary, and the diagnostics-export ADR
requirement, and ADR-001 continues to reference the two-registration split
only as a disabled boundary it does not complete. ADR-005's migration/rollback
mechanics, ADR-010's certificate-cleanup rule, and ADR-PRIV-001's empty
diagnostics allowlist are consumed, not reopened or weakened.

## Verification

The deterministic checker pins the complete manifest and accepted ADR, checks
the exact requirement/source/input-authority inventories, validates the closed
registration-mode, shared-registration-governance, update-trust-chain,
artifact-allowlist, installer-boundary, and diagnostics-rule catalogs, and
reconciles ADR-001's registration-boundary wording, ADR-005's migration and
R-21/R-22 risk rows, ADR-010's certificate-cleanup rule, ADR-PRIV-001's empty
diagnostics allowlist, the governance registry, the support matrix, and
build-skeleton package rules (`package.json` `files: []`, crate
`publish = false`). Synthetic mutations must reject a tracked client ID or
secret or any non-placeholder registration value, shared registration enabled
without the complete governance-control list, update metadata signed by the
package key, a permitted downgrade, missing expiry/hash/size/channel binding,
non-atomic staging, removed provenance, default elevation, an allowlisted
source map or generated diagnostic, a widened `npm` files list or an enabled
crate publish flag, a non-empty diagnostics allowlist, an uninstall that
leaves bridge trust behind, a gate-passed or capability-enabled claim,
fresh-checker preapproval, and additive documentation contradiction. Fresh
security, registration-governance, governance, and adversarial judges are
required for closure; their prompts, transcripts, and output are not
repository evidence.
