# Privacy and local-state boundary

This model implements ADR-PRIV-001 and is authoritative only together with
`contracts/privacy/persistence-boundary.json`. All examples and tests are
synthetic.

The Phase 0 privacy manifest is a field allowlist, not a runtime serialization
schema, and approves no runtime values. ADR-005 fixes the envelope, algorithms,
key boundaries, and exact validation order; dependent domain ADRs must still add
their logical value shapes, catalogs, and bounds before the pre-encryption or
post-decryption controls below can be implemented or claimed.

| Boundary | Threat | Required control | Fail-closed result |
|---|---|---|---|
| Logical record to encrypted record | An unapproved field is hidden inside ciphertext | Validate the exact typed record before encryption | Reject; do not persist |
| Encrypted record to application | Old, corrupt, or attacker-crafted data bypasses policy | Authenticate, decrypt, then validate exact schema before use | Quarantine without content-bearing diagnostics |
| Schema or migration | Unknown keys or versions are ignored | Reject duplicate/unknown keys, variants, and versions; no silent dropping | Migration stops; dependent state remains unavailable |
| Account boundary | Ciphertext, locator, or operation is replayed across accounts | Account-bound authenticated data and local opaque references | Reject cross-account use |
| Microsoft content | Local state becomes a readable mailbox or task copy | Store only exact locators, keyed anchors/digests, typed temporal values, codes, and references | Refetch readable text transiently from Microsoft 365 or show unavailable |
| Manual-origin artifact | OpenLoops overwrites user-authored title/body/due or treats deletion as completion | Exact single authoritative artifact, field ownership, version reconciliation | Preserve user value; deletion produces unavailable/open review state |
| Retry and reconciliation | Blind retry duplicates or misdirects a mutation | Idempotency digest, target locator, expected remote version, field masks, ownership, bounded attempts, ambiguity state | Reconcile or require review; never assume success |
| Retention and deletion | State outlives its purpose or delete mutates Microsoft data | Exact retention policies and local-only delete/reset/disconnect operations | Remove eligible local records; zero Graph mutations |
| Metrics | Counters become employee monitoring or event history | Fixed rolling aggregate, no person/client/message/task/exact-event dimensions, independent reset, no export | Indicator unavailable |
| Diagnostics | Error handling leaks content, identifiers, prompts, or secrets | Empty diagnostic-event allowlist until separate content-free contract and gates | No diagnostic export |
| Secret material | Credentials enter the database or plaintext fallback | OS-protected secret store or memory only; ADR-005/ADR-010 | Capability disabled |

The desktop window (`openloops-desktop`, `native-ui`) renders with Slint's
native Windows backend; there is no embedded webview or browser engine, so
mail content and the provider API key never pass through DOM state, a
JavaScript heap, or any process boundary a web renderer would add. The key is
held in a Rust `Zeroizing<String>` and mirrored into the Slint password
field's text property only while the Sources screen is the active screen; it
is plain in that field's own memory while mirrored (masked on screen unless
"Show key" is on), is never written to disk or logs, and is cleared from the
field on Forget/Reload or when Sources is not the active screen -- the key
does not stay confined to the `Zeroizing` buffer for its whole lifetime.
Links the window can open are limited to fixed provider
and Microsoft URLs (Entra, the Ollama/OpenRouter key pages, To Do) and,
for a message deep link, `https://outlook.office.com/...` or
`https://outlook.office365.com/...` specifically; every other URL is rejected
before the `opener` crate is invoked.

Application-level authenticated encryption cannot prevent disclosure to a
compromised same-user process, administrator, dependency, or endpoint. It also
does not promise physical erasure from media, backups, snapshots, paging, or
crash residue. Ciphertext size, access timing, opaque references, digests, and
state transitions may still reveal correlations. These residual risks keep
G-STATE, G-PRIV, and G-SEC-AUDIT unrun until their independent evidence exists.
