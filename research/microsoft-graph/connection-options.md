# Microsoft Graph connection options

## Research framing

"Connecting to Microsoft Graph" contains four independent choices:

1. Who owns the Entra application registration.
2. Which OAuth flow authenticates the user or application.
3. Where the OpenLoops process runs.
4. How OpenLoops discovers changes.

WAM and browser PKCE, for example, can both use either a centrally controlled
OpenLoops registration or a bring-your-own registration. Webhooks are a change
delivery mechanism, not an authentication flow.

## Authentication and deployment options

| Model | Work/school accounts | Personal Microsoft accounts | Requirements | Research verdict |
|---|---:|---:|---|---|
| Shared OpenLoops delegated public client | Yes | Yes when configured for both audiences | OpenLoops-controlled multitenant registration, no client secret, publisher and consent governance | Candidate public default after governance gates |
| Bring-your-own delegated public client | Yes | Only if the registration supports them | User/admin supplies public client ID, authority/audience, and exact redirect configuration | Development default and first-class enterprise option |
| Windows Web Account Manager (WAM) | Yes | Supported with a compatible authority/audience | Interactive Windows user, MSAL broker integration, broker redirect | Preferred Windows sign-in candidate |
| System browser, authorization code and PKCE | Yes | Yes | Public client, external browser, same-machine loopback redirect, protected token cache | Cross-platform desktop baseline |
| Device-code flow | Yes | Yes | User completes sign-in elsewhere; tenant policy must permit the flow | Headless-only future option; not an automatic fallback |
| Remote container or NAS | Not through ordinary desktop loopback | Not through ordinary desktop loopback | Stable HTTPS callback, remote session security, protected server-side state | Separate future web product |
| Client credentials/app-only | Organizations only | No | Confidential client, certificate/managed identity, administrator consent | Separate enterprise daemon only |
| Hosted confidential web app | Yes | Yes | Public HTTPS callback, server credential and per-user session/token protection | Separate hosted product |
| On-behalf-of flow | Yes | Yes at the identity-platform level | Upstream client, confidential OpenLoops API, downstream token exchange | Only relevant to a future hosted API |
| ROPC, implicit, or embedded credential collection | Limited or legacy | No dependable general support | Application handles credentials or uses obsolete front-channel behavior | Prohibited |

Primary identity sources:

- [Microsoft authentication flows and application scenarios](https://learn.microsoft.com/en-us/entra/identity-platform/authentication-flows-app-scenarios)
- [Microsoft public and confidential client applications](https://learn.microsoft.com/en-us/entra/msal/msal-client-applications)
- [MSAL.NET WAM guidance](https://learn.microsoft.com/en-us/entra/msal/dotnet/acquiring-tokens/desktop-mobile/wam)
- [Microsoft redirect URI restrictions](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- [IETF OAuth 2.0 for Native Apps, RFC 8252](https://datatracker.ietf.org/doc/html/rfc8252)
- [Microsoft Conditional Access authentication-flow guidance](https://learn.microsoft.com/en-us/entra/identity/conditional-access/concept-authentication-flows)

### Important identity conclusions

- **Verified:** A distributed native application is a public client and cannot
  keep a client secret.
- **Verified:** Authorization code with PKCE and an external browser is the
  standards-backed native application flow.
- **Verified:** Desktop loopback authentication assumes the browser and
  application run on the same machine. A NAS callback to `localhost` reaches
  the browser's machine, not the NAS.
- **Verified:** Device-code flow is supported but is classified by Microsoft as
  high risk and can be blocked by Conditional Access.
- **Verified:** App-only/client-credentials access does not support personal
  Microsoft accounts and requires organization administrator consent.
- **Verified:** Local disconnect is not equivalent to global session, refresh
  token, or tenant-consent revocation.
- **Verified:** MSAL may add standard OIDC scopes, including `offline_access`,
  automatically. The implementation must follow the pinned library rather than
  blindly duplicating them.
- **Inference:** BYO registration should be the development default and remain
  a first-class enterprise option.
- **Inference:** A shared OpenLoops client ID should be offered only after
  publisher verification, registration governance, signed distribution, and
  incident procedures exist.
- **Known limitation:** PKCE protects an authorization code. It does not
  authenticate the installed binary. A malicious fork can reuse a public client
  ID and initiate its own valid PKCE transaction.

## Change-discovery options

| Model | Infrastructure | Persistent state | Research verdict |
|---|---|---|---|
| Local polling and delta queries | Outbound Graph HTTPS only | Cursors, resource IDs, timestamps, idempotency state | Recommended baseline |
| Direct Graph webhook | Public HTTPS receiver | Subscription IDs, expiry, secret `clientState`, queue/checkpoints, delta cursors | Future optional mode |
| OpenLoops-hosted relay | Centrally operated service | Routing metadata and service secrets in addition to local state | Separate trust/compliance boundary |
| Azure Event Hubs | Azure subscription, Event Hub and RBAC/configuration | Azure consumer offsets plus Graph subscription/delta state | Future enterprise option |
| Azure Event Grid | Azure subscription, partner topic and event subscription | Topic/subscription/checkpoint state | Future enterprise option |

Graph webhooks require a publicly reachable HTTPS receiver, validation,
renewal, prompt acknowledgement, replay/duplicate handling, and delta
reconciliation. Outlook subscriptions expire in under seven days and To Do
subscriptions in under three days. To Do task notifications are per task list,
not mailbox-wide.

Sources:

- [Graph change notifications overview](https://learn.microsoft.com/en-us/graph/change-notifications-overview)
- [Webhook delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks)
- [Create subscription](https://learn.microsoft.com/en-us/graph/api/subscription-post-subscriptions?view=graph-rest-1.0)
- [Event Hubs delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-event-hubs)
- [Event Grid delivery](https://learn.microsoft.com/en-us/azure/event-grid/subscribe-to-graph-api-events?context=graph%2Fcontext)

## Delegated permissions by feature

Permissions should be requested when the user enables a feature rather than as
one combined initial grant.

| Feature | Permission | Evidence and boundary |
|---|---|---|
| Sign-in | OIDC scopes managed by MSAL | Do not add `User.Read` unless OpenLoops actually needs `/me` |
| Mail metadata and message delta | `Mail.ReadBasic` | Excludes message body, body preview, attachments, and extended properties; metadata remains sensitive |
| Mail content analysis | `Mail.Read` | Required when OpenLoops reads bodies or other excluded content |
| Modify mail | `Mail.ReadWrite` | Allows mail mutation but not sending |
| Send mail | `Mail.Send` | Independent permission; should be separately enabled |
| Read To Do lists and tasks | `Tasks.Read` | Documented for work/school and personal accounts |
| Create/update/delete To Do data | `Tasks.ReadWrite` | Documented delegated write permission for both account categories |

Sources:

- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference)
- [List messages](https://learn.microsoft.com/en-us/graph/api/user-list-messages?view=graph-rest-1.0)
- [List To Do lists](https://learn.microsoft.com/en-us/graph/api/todo-list-lists?view=graph-rest-1.0)
- [List To Do tasks](https://learn.microsoft.com/en-us/graph/api/todotasklist-list-tasks?view=graph-rest-1.0)
- [Create a To Do task](https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks?view=graph-rest-1.0)

## Provisional desktop architecture

The final research synthesis recommended the following starting boundary:

1. One signed-in human using a signed, unelevated desktop application.
2. Delegated public-client authentication only.
3. WAM first on Windows, with system-browser authorization code and PKCE as a
   fallback.
4. BYO registration for development; a central OpenLoops registration only
   after its operational gates are satisfied.
5. Commercial global Microsoft cloud only.
6. Outbound polling rather than webhooks.
7. OS-protected MSAL/WAM token cache and encrypted synchronization state.
8. No intentional persistence of raw Microsoft 365 content by default.

The recommendation was Windows-first to reduce the initial signing, updating,
token-cache, and platform test surface. Graph does not require Windows. A
cross-platform desktop design using browser PKCE and OS secret stores is
technically defensible; it simply creates more launch surfaces.

## Entra registration and distribution requirements

### Development

- Use separate development and production registrations.
- Use BYO public-client configuration and synthetic accounts.
- Configure delegated permissions only.
- Register only the exact desktop redirects being tested.
- Do not create or ship a client secret.
- Keep tenant IDs, client IDs used for private testing, and account identifiers
  out of the public repository.

### Before a shared production registration

- Dedicated organizational publisher tenant and verified domain.
- Microsoft publisher verification.
- Accurate name, logo, homepage, privacy statement, terms, and support contact.
- At least two controlled registration owners and owner-recovery procedure.
- Exact registered redirects and no unused platform configuration.
- Incremental consent and plain-language permission explanations.
- Support for tenants that disable user consent or require admin approval.
- Sign-in/consent-failure monitoring without message/task content collection.
- Malicious-fork/client-ID abuse and incident-response procedures.
- Signed official builds and updates with published provenance.
- BYO registration documentation for organizations that reject the shared app.

Sources:

- [Publisher verification](https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview)
- [Configure user consent](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/configure-user-consent)
- [Application consent experience](https://learn.microsoft.com/en-us/entra/identity-platform/application-consent-experience)
- [Microsoft identity-platform terms](https://learn.microsoft.com/en-us/legal/microsoft-identity-platform/terms-of-use)

## Explicitly deferred by the final research synthesis

- Remote browser, NAS, container, or LAN-hosted deployments.
- Device-code authentication.
- Windows services or machine accounts.
- Application permissions and tenant-wide access.
- Shared mailbox notifications.
- Webhooks or an OpenLoops-hosted relay.
- Event Hubs and Event Grid.
- National/sovereign Microsoft clouds.
- Multiple OS users sharing a token cache.
- Automatic sending, deletion, or unconfirmed task creation.

These are research recommendations, not approved product exclusions. Each
deferred mode needs a separate threat model and decision before implementation.
