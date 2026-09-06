# OpenLoops validation and support matrix

**Disposition:** P0-MATRIX-001 accepted by the product owner on 2026-07-19
**Work item:** P0-WI-03
**Current support claim:** none

This document pins the Phase 0 validation targets required by the implementation
plan. It does not advertise or enable a platform, account, Outlook client,
Microsoft cloud, or model provider. The exact machine-readable decision is
`contracts/support/support-matrix.json`; a row becomes supportable only after
all of its named gates, fresh-version checks, contract tests, security/privacy
checks, and release approval pass.

## Approved validation targets

The Windows validation matrix is Windows 11 x64 versions 24H2 and 25H2, only
while the installed edition remains in Microsoft servicing and the exact build
passes release testing. The runtime is local, unelevated, per-user, and
single-account. Windows 26H1, Windows on Arm, Windows 10, x86, macOS, Linux,
services, remote browsers, containers, NAS, LAN-hosted, and headless modes are
not part of this matrix.

The Outlook validation matrix is:

- classic Outlook on Windows with a current supported Microsoft 365
  subscription build;
- new Outlook for Windows at a current serviced build; and
- Outlook on the web in current Edge, Chrome, and Firefox on the same approved
  Windows machine as the companion.

Each uses one commercial-global work/school member account with its own Exchange
Online primary mailbox. The proposed add-in floor is desktop `Mailbox` 1.13
with `ReadItem` only. `ReadWriteMailbox`, shared-folder permission, mailbox-token
ownership, event-based loop creation, and synchronization authority are
prohibited. Classic Outlook must test selected-item and no-item context. New
Outlook and web use selected-item activation and must fall back to the native
companion when no item is selected; they do not claim an always-available
dashboard.

Personal Microsoft accounts remain a gated target. Shared/delegated mailboxes,
multiple accounts, guest profiles without a proven mailbox, non-Microsoft
mailboxes, on-premises/hybrid Exchange, GCC/GCC High/DoD, and China remain
disabled or outside the MVP. Sign-in success never substitutes for the complete
identity, mail, To Do, calendar, add-in, state, and privacy matrix.

## Model-provider matrix

No provider is the only default and makes zero provider requests. The approved
but disabled targets are built-in Ollama local, built-in Ollama Cloud, and an
optional adapter-specific approved-HTTPS provider. ADR-007, G-MODEL, and G-PRIV
remain mandatory.

Ollama local documents `http://localhost:11434/api`, but loopback does not prove
local processing because Ollama can route cloud models through its local API.
The profile stays disabled until ADR-007 defines the exact canonical loopback
origin and tests verifiable cloud-disable behavior, cloud-model rejection,
redirect/DNS/proxy defenses, version probing, selected-model identity, strict
output validation, and privacy canaries. OpenLoops accepts no local-profile
credential and never falls through to cloud.

Ollama Cloud has the fixed API base `https://ollama.com/api`. Its user API key
is write-only and OS-protected, and may be sent only as a bearer credential to
that exact consented origin after TLS/address policy checks. Redirects, implicit
proxy inheritance, and cross-origin authorization forwarding are prohibited.
Current official documentation does not justify a server-side structured-output
claim for cloud; application-side schema/evidence/semantic validation remains
mandatory and live synthetic BYO-key testing stays outside CI.

An approved-HTTPS adapter has one canonical public HTTPS origin and a fixed,
reviewed wire/auth schema. It cannot accept arbitrary headers, private or
metadata targets, or substitute for the required tested Ollama Cloud path. All
provider endpoint, model, credential, or transmitted-field changes invalidate
consent and calibrated use.

## Freshness and failure behavior

Windows servicing, Office builds, requirement sets, Outlook client behavior,
browser support, account/cloud availability, and Ollama contracts are unstable.
They must be rechecked against the official sources named in the manifest at
release. Changed or stale evidence disables the affected row. A scope change is
routed to the product owner; it is never handled by silently broadening a row.

Every unsupported or failed row falls back before secrets, durable state,
provider content, or Graph mutation occurs. Native companion status/recovery is
the add-in fallback, no-provider reduced behavior is the model fallback, and an
unknown account/cloud/platform is rejected. No fallback broadens permissions,
selects another endpoint, or implies that a mandatory PRD feature passed.
