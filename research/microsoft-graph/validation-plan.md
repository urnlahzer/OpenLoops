# Validation plan and unresolved Microsoft Graph behavior

All validation must use disposable tenants/accounts and synthetic content.
Never commit tokens, tenant IDs, account IDs, email addresses, response payloads,
or raw diagnostic captures. Record only sanitized test outcomes and API/service
versions.

## Known documentation conflicts

### Microsoft To Do delta permissions

- Ordinary list and task reads document delegated `Tasks.Read`.
- [Task-list delta](https://learn.microsoft.com/en-us/graph/api/todotasklist-delta?view=graph-rest-1.0)
  documents delegated `Tasks.ReadWrite`.
- [Task delta](https://learn.microsoft.com/en-us/graph/api/todotask-delta?view=graph-rest-1.0)
  lists `Tasks.ReadWrite` as least privileged and `Tasks.Read` as higher
  privileged, an internally reversed ordering.
- The task-delta documentation also contains filter language for properties that
  do not belong to a To Do task, suggesting copied documentation.

Until tested, read-only To Do mode should use ordinary list/task enumeration
under `Tasks.Read`. Write-enabled mode may use delta under `Tasks.ReadWrite` only
after contract tests pass.

### Microsoft To Do application permissions

- The permission reference defines `Tasks.Read.All` and `Tasks.ReadWrite.All`.
- Read and delta pages expose application permissions.
- Create/delete task pages document application permissions as unsupported.
- Update task documentation distinguishes app-only behavior for "self" and
  "other users," even though an app-only request has no signed-in self.

Do not ship app-only To Do mutation based on inference or one successful test.
Treat it as endpoint-specific and unsupported until Microsoft provides a stable
documented contract.

### Redirect URI selection

Microsoft's general redirect guidance recommends `127.0.0.1`, while common
MSAL.NET desktop configuration and portal flows use `http://localhost` with a
random port. HTTP `127.0.0.1` may require manifest editing. The selected runtime,
portal registration, IPv4/IPv6 behavior, firewall behavior, and port collision
handling must be tested together before the redirect contract is frozen.

### Conditional writes

Mail and To Do resources expose version information such as `changeKey` and
`@odata.etag`, but the v1.0 PATCH documentation does not clearly establish
`If-Match` behavior. Do not promise optimistic concurrency until stale/current
precondition tests prove it for each intended operation.

## Identity and consent experiments

1. Work/school and personal-account WAM sign-in.
2. Work/school and personal-account browser PKCE sign-in.
3. `localhost` and `127.0.0.1` redirects with the selected MSAL runtime.
4. Missing, incorrect, replayed, duplicated, and expired OAuth state.
5. Loopback port races and non-loopback/LAN reachability.
6. User consent enabled, disabled, admin-gated, and verified-publisher-only.
7. Incremental consent denial and later grant.
8. Consent/session revocation and Conditional Access changes.
9. Guest-account and multiple-tenant account selection.
10. Cache loss, secure-store lock, reconnect, and disconnect.

## Mail experiments

1. `Mail.ReadBasic` list, get, and per-folder delta for both account categories.
2. Assert that prohibited body, preview, attachment, and extended-property data
   is unavailable under `Mail.ReadBasic`.
3. `Mail.Read` content access using synthetic messages.
4. `Mail.ReadWrite` mutation without `Mail.Send`.
5. `Mail.Send` behavior without read/write permissions.
6. Immutable IDs across folder moves, copy, delete, and draft send.
7. Inbox, Sent Items, and user-selected folder delta behavior.
8. Empty pages, paging, duplicate page application, cursor expiry, and bounded
   resynchronization.
9. `If-Match` behavior for every intended mutation.
10. Bounded initial-history filters and exact `$select` field sets.

## Microsoft To Do experiments

Run each test separately for work/school and personal accounts.

1. With `Tasks.Read`, list lists and tasks.
2. With `Tasks.Read`, call list delta and task delta and record sanitized status
   and permission outcomes.
3. With `Tasks.ReadWrite`, repeat reads/deltas and test create/update/delete.
4. Test built-in Tasks and Flagged Email lists.
5. Test task movement or delete/add behavior between lists.
6. Test shared list/task behavior only if it becomes product scope.
7. Test stale/current `If-Match` behavior.
8. Simulate ambiguous POST completion and verify idempotency handling.

App-only To Do experiments, if ever authorized, require a separate confidential
registration and disposable organization tenant. Test every endpoint separately
with `Tasks.Read.All` and `Tasks.ReadWrite.All` and do not treat observed success
as a documented support guarantee.

## Reliability and privacy experiments

1. Crash before and after every cursor/idempotency checkpoint boundary.
2. Inject 429, 503, 504, timeout, malformed retry, and partial batch-throttling
   responses without deliberately overloading Graph.
3. Validate bounded exponential backoff, jitter, stale-state reporting, and
   recovery from invalid delta state.
4. Place unique synthetic canaries in subject, body, participant, task title,
   task body, token, delta URL, and authorization header positions.
5. Exercise success, denial, error, crash, update, uninstall, and support-bundle
   paths; scan all application-controlled artifacts for raw canaries.
6. Attempt cache access as another OS user and after backup/restore or machine
   transfer.
7. Run without a usable secure store and confirm session-only/fail-closed
   behavior with no cache artifact.
8. Tamper with installers, updates, dependencies, and lockfiles; unsigned or
   inconsistent artifacts must be rejected.
9. Use hostile synthetic message content containing HTML, scripts, remote
   images, prompt injection, shell-like instructions, and fake administrator
   requests. None may execute or override product policy.

## Future notification experiments

These do not block a polling-based first release:

- Mail and per-list To Do webhook creation for both account categories.
- Subscription expiry just below and above documented limits.
- Duplicate, delayed, missing, reordered, forged, replayed, and oversized
  notifications.
- Lifecycle notification differences between Outlook and To Do.
- Webhook outage followed by delta reconciliation.
- Event Hubs and Event Grid tenant/Azure prerequisites and MSA limitations.
