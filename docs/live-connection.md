# Microsoft connection development slice

This opt-in executable implements browser sign-in and read-only access checks
against Microsoft Graph. It is not yet the reminder application. No live gate
has been passed by compiling it or running its local tests.

## Registration

Use a Microsoft Entra public-client registration accepting accounts in any
organizational directory. Register `http://localhost` under Mobile and desktop
applications. No client secret is used. The browser flow uses authorization code
with PKCE S256 against the commercial `organizations` authority. Personal
Microsoft accounts and sovereign clouds are outside this slice.

The connection check requests delegated `User.Read` and `Mail.Read` for a personal inbox, or
`Mail.Read.Shared` when shared mailboxes are selected. Shared mailbox
access must already be assigned to the signed-in user. Registering a scope does
not grant access; consent is handled by Microsoft at sign-in according to the
organization's policies. No task scope is requested. Group lookup permission is requested only in Groups mode.

## Run

The [native setup window](native-setup.md) now provides visible application-ID,
Groups, and shared-mailbox fields and a browser sign-in button. The command below
is an optional development diagnostic.

### Outlook Groups

Inboxes under Outlook's **Groups** are Microsoft 365 Groups, not shared
mailboxes. Enter their primary email addresses at the separate Groups prompt.
This mode requests delegated `Group.ReadBasic.All` to resolve group IDs by
exact primary mail address and `Group-Conversation.Read.All` to read group
conversation threads. Group conversation access requires administrator consent. The lookup selects
only IDs from an exact-address query. It checks only selected groups,
requests at most one thread ID, and persists neither IDs nor content. Group
permissions are requested only when group addresses are entered. Group tests
use synthetic responses; live tenant validation remains pending.

The launcher prints a clean failure summary and retains a nonzero exit status
if any check fails. Authentication failures (401), authorization failures (403),
throttling (429), and missing shared-mail scope have separate fixed diagnostics.

From a PowerShell 7 terminal in the repository, run:

```powershell
pwsh ./tools/connect-microsoft.ps1
```

The launcher builds the explicit `live-connection` feature, then asks for the
application ID with masked input and optional shared inbox addresses separated by
commas or semicolons with visible input so typing errors can be corrected. These
values remain process-local and are restored to their previous environment
values when the launcher exits. Do not paste registration values into source,
command history, screenshots, issues, or committed files.

Complete sign-in in the system browser and return to the terminal. Success
reports only personal inbox access and a numbered result for each shared inbox, in input order. Duplicate addresses are checked once; a failed shared inbox does not skip the others. The check asks Graph for at most one
message identifier per inbox and discards the bounded response. An empty inbox
is a successful access check. No message body or subject is requested. Tokens
remain in memory for this command; no refresh token or durable cache is requested.

## Boundaries and remaining work

The separate [inbox review](inbox-review.md) now reuses this browser sign-in
boundary to retrieve bounded message bodies on explicit user action, followed
by selected-message analysis in the native UI. The checks described above remain
content-free.

- HTTPS is restricted to the fixed Microsoft token endpoint and Graph inbox
  endpoints. Redirect following and automatic proxy discovery are disabled.
- Callback handling accepts GET at the exact root path and localhost authority,
  validates state before accepting a code, rejects duplicate decoded parameters,
  and bounds request size, per-connection time, and total sign-in time. Invalid
  requests do not consume the legitimate pending sign-in.
- Errors and progress contain no identifiers, server payloads, or OAuth values.
  Browser callback responses disable caching and referrer transmission.
- The default executable remains the synthetic build probe. Existing Phase 0
  checkers deliberately reject activated network dependencies. This slice does
  not change or weaken those checkers, claim release readiness, or enable product
  capability flags. Their transition to runtime confinement checks needs explicit
  review before integration into the canonical source-build path.
- A process-local guard serializes calls. The [native review](inbox-review.md)
  adds account binding, saved abstract decisions, and separately authorized
  personal To Do writes. Protected token renewal, discovered mailbox selection,
  full production loop persistence, and unattended monitoring remain on the
  roadmap. Shared sources do not constitute a shared team state store.

## Validation

```powershell
cargo test -p openloops-graph -p openloops-desktop --features openloops-desktop/live-connection --locked
```

These tests use only invented configuration and loopback peers. They do not
contact Microsoft or validate tenant consent. A successful real run is evidence
for that particular account and configuration only.

References: [Microsoft authorization-code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow),
[desktop registration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration),
[shared mail access](https://learn.microsoft.com/en-us/graph/outlook-share-messages-folders),
[OAuth library](https://docs.rs/oauth2/5.0.0/oauth2/).
