# ADR-006: Evidence identity and anchoring

- **Status:** Accepted
- **Date:** 2026-07-20
- **Work item:** P0-WI-09
- **Repair item:** P0-WI-09R (product-owner approved 2026-07-20)
- **Owner decisions:** OWN-05, OWN-07
- **Blocking gates:** G-MAIL, G-CAL, G-ADDIN, G-STATE, G-PRIV

## Context

OpenLoops must later re-fetch supporting Microsoft evidence without retaining a
readable copy. Folder, conversation, subject, Internet Message-ID, web link, and
content equality are not item identity. Default Graph IDs can change; copies can
have identical content; Office.js exposes EWS-format IDs; and navigation differs
by client. OWN-07 nevertheless requires typed source references with no content
fallback. OWN-05 keeps invitation evidence behind G-MAIL and G-CAL. These
accepted product choices are not reopened here.

Current Microsoft documentation leaves two gated conflicts. The mail overview
names draft send as an immutable-ID exception, while the immutable-ID article
describes finding the sent copy with the draft immutable ID. The current
`translateExchangeIds` v1.0 permission table also requires a user-directory
permission not approved by ADR-003. OpenLoops resolves neither by assuming
favorable behavior or asking for broader permission.

## Decision

`contracts/evidence/identity-boundary.json` is the closed P0-WI-09 contract. It
activates no Graph request, Office call, persistence, permission, source variant,
client support, or navigation path.

P0-WI-09R closes only three reproduced final-gate defects. While the runtime is
inactive, the complete current product and package-script-reachable build/test
execution surface—including generator, schema, test, compiler/configuration,
Cargo manifests/lock, and npm lock graph—has an exact file inventory and aggregate
fingerprint. Rust/TypeScript import lines, root package fields, npm scripts,
dependencies, and forbidden dynamic-load constructs are exact allowlists. An
added reachable file, `node:https`/`node:http`,
`undici`, `got`, arbitrary dependency, new script, dynamic import, process
launcher, Graph/Office call, or other inventory drift fails the zero-runtime
claim. The approved Prettier command disables implicit configuration and
EditorConfig discovery. Signature scanning is secondary and cannot substitute
for the allowlists.

For email evidence, primary identity is a case-sensitive Graph REST immutable
entry ID bound to the opaque local account and a keyed authenticated-mailbox
binding. That binding is a full HMAC-SHA-256 under the distinct per-account
ADR-005 content-digest key and its own closed purpose tag. Its single ADR-005
envelope contains exactly the commercial-cloud code and the network-order
validated `tid` and `oid` UUID bytes from ADR-002/G-ID; raw claims never persist.
Missing, malformed, guest, personal, shared, sovereign, or mismatched claims
reject locally. The exact immutable-ID preference accompanies every request that can
return a message ID. Folder and conversation locators are encrypted coverage or
candidate hints, never identity. A default REST ID is an encrypted candidate-only
fallback. `internetMessageId` is not selected or persisted.
`/me/translateExchangeIds` is prohibited because its current permission contract
would add an unapproved directory scope; any scope change returns to the product
owner.

Same-primary-mailbox moves may retain identity only after G-MAIL passes. Moving
to the ordinary Archive folder is one such gated folder move. The Exchange
in-place/online archive mailbox is a different, unsupported mailbox boundary.
A copy is always distinct even when all anchors match. Export/import, delete,
retention loss, permission loss, and unobserved sent-copy paths become unavailable
or known loss. Draft/compose creates no evidence. A saved sent copy
can create evidence only from its first G-MAIL-validated immutable-ID observation
and never proves delivery.

`email_evidence_ref` has the exact closed schema in the contract and matches the
ADR-PRIV-001 allowlist. Values reject unknown/duplicate fields, invalid nulls,
types, catalogs, bounds, versions, and account/source bindings before encryption
and after authenticated decryption. Raw Graph/Office identifiers exist only in
the exact AEAD-protected locator fields—never plaintext SQL/indexes, logs,
diagnostics, fixtures, packages, links, or the repository.

Ranges use zero-based Unicode scalar indices with an exclusive end in one
NFC-normalized canonical block. Empty or cross-block ranges reject. The closed
numeric component, direction, read-state, and first-observation maps are part of
the byte contract. Ordered canonicalization uses the versioned WHATWG-fragment
inert-text profile, exact Graph body/participant/attachment shapes, closed block
and quote segmentation, forbidden-control/noncharacter rules, CR/LF handling,
and NFC. It rejects unpaired surrogates and incomplete attachment paging, never
loads a resource or reads an `href`, treats hostile-looking text only as data,
and creates only a transient offset map. Public synthetic conformance vectors
pin entities, breaks, quotes, hidden elements, links, composed/decomposed Unicode,
astral scalars, invalid surrogates/noncharacters, participants, and page caps.
The DOM walker is byte-deterministic: depth-first tree order; exact entry and exit
LF events for every closed block element and `td`/`th`; one LF for `br`/`hr`; no
event for accepted inline elements; suppressed outermost-blockquote boundary
events; and one exact whitespace finalization per body, quote, or link-label
buffer. Sibling-block, nested-block, and table-cell vectors pin the emitted bytes.

Every accepted message enumerates the exact bounded metadata-only
`GET /me/messages/{immutable-id}/attachments?$select=id,name,contentType,isInline,size`
collection through its terminal opaque page sequence. `hasAttachments` is only a
hint and never suppresses enumeration because Microsoft documents that it is
false for inline-only attachments. Attachment bytes remain prohibited; a public
inline-only synthetic vector pins the filename block.

Content, prefix, and suffix anchors are full HMAC-SHA-256 outputs under the
distinct per-account ADR-005 digest key and purpose tags. Exactly one ADR-005
outer envelope binds purpose, account, owning schema, and component count;
ADR-006 defines only the ordered source/component/block/range/version/text
components inside it, with no repeated purpose or account frame. The first six
anchor components and the distinct exact seventh content/prefix/suffix text
component are closed for each purpose.
Prefix/suffix use at most 64 adjacent scalars in the same block. Constant-time
full-output comparison is mandatory. A digest match proves only equality of the
framed bytes under the key—not item identity, ownership, account, folder, or copy
lineage.

Resolution rejects binding/version mismatches locally, refetches the primary,
and requires all three anchors. Resolved-but-different content becomes `changed`.
A missing primary may enumerate only the complete candidate universe inside
already-authorized selected folders and the configured history window. Every
page and candidate must reach a terminal result within the fixed G-MAIL-tested
caps. Only a completed enumeration can classify zero, one, or many verified
candidates. Any cap, throttle, failure, missing terminal page, or incomplete
attachment sequence is ambiguous/unavailable and can never offer a singleton.
One triple-anchor candidate from a completed enumeration can be offered as
replacement evidence, but automatic reanchoring is prohibited. User confirmation plus one
ADR-005 transaction is required. Changed, ambiguous, stale, and unavailable
evidence freezes evidence-dependent automation. Missing evidence is never
negative or closure evidence, and no readable copy is a fallback.

Office and Graph authority remain separate. Under G-ADDIN, a read-mode EWS item
ID may be converted transiently with `convertToRestId(..., v2_0)` and used only
for a bound Graph GET under existing `Mail.Read` plus the immutable preference.
Only the returned validated immutable locator may persist. Graph-to-Office
display remains unavailable until G-ADDIN/G-MAIL prove it. The gated display
path must use `displayMessageFormAsync` with Mailbox 1.9 and treat every callback
error or unconfirmed display as unavailable; the synchronous call is prohibited.
A Graph `webLink` is retrieved transiently only after identity/account validation.
It rejects controls, surrounding whitespace, credentials, nondefault ports, IPs,
trailing dots, punycode/lookalikes, and any normalized origin outside the exact
HTTPS allowlist. OpenLoops validates only the initial URL, never prefetches or
follows redirects, opens it as top-level external navigation, and never
persisted, logged, rewritten, embedded, or used as identity. Review links contain
only a random non-secret local loop handle.

Calendar event/response values remain unavailable pending ADR-009 and G-CAL.
User-authored To Do/calendar artifact locator and field-version values remain
unavailable pending ADR-009 and its adapter gate. The three variant shapes stay
those approved by ADR-PRIV-001; none is enabled here.

## Consequences

The design prefers visible unavailability or user-confirmed replacement over a
silent wrong-source link. P0-WI-09/P0-WI-09R accept only a disabled logical contract.
AS-10, AS-14, AS-16, and AS-19 remain unpassed. No gate, capability, support row,
permission, acceptance criterion, or product claim advances.

## Verification

P0-WI-09/P0-WI-09R are graded by exactly twelve executable checks:
P0-EVID-INVENTORY-001, P0-EVID-SCHEMA-001, P0-EVID-IDENTITY-001,
P0-EVID-GRAPH-001, P0-EVID-ANCHOR-001, P0-EVID-RESOLUTION-001,
P0-EVID-LIFECYCLE-001, P0-EVID-OFFICE-001, P0-EVID-UNAVAILABLE-001,
P0-EVID-PRIVACY-001, P0-EVID-CROSS-CONTRACT-001, and
P0-EVID-CLAIMS-001. P0-EVID-FRESH-CHECKER-001 is the separate closure check.
