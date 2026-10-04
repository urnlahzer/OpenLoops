# Plan: Google (Gmail + Tasks) provider and multi-tenant Microsoft distribution

Date: 2026-09-27. Branch base: `main`. One feature branch and PR per phase. Codex executes;
Claude reviews and verifies (see "Execution notes").

## Context

OpenLoops connects one Microsoft 365 work/school account through a user-entered Entra
client ID, the `organizations` authority, PKCE, and a memory-only token. The live code is
Graph-shaped end to end: `MailItem` carries Graph ids, links are gated to Outlook hosts,
reminders are Microsoft To Do, one global `SESSION`, and PowerShell governance checkers
confine network code to `openloops-graph`. Non-Microsoft mail is `outside_mvp` in
`contracts/support/support-matrix.json:253-263`.

The owner wants: (1) people at other domains with Microsoft 365 can use the app without
registering their own Entra app; (2) Gmail users can use the app with Google Tasks
reminders; (3) one user can connect Microsoft and Google at the same time.

Owner decisions taken during planning (2026-09-27):

| Question | Decision |
|---|---|
| Microsoft distribution | Ship a shared multitenant client ID, complete publisher verification, add an admin-consent link, keep the BYO client-ID field as fallback. ADR-012 amendment. |
| Google audience | Personal Gmail first. One external Google OAuth client shipped in the app, in **Testing** status (free; 100 named test users; unverified-app screen). Verification + CASA deferred until the owner chooses to pay. |
| Google reminders | Google Tasks, own phase. |
| Accounts | Microsoft and Google connected at once, merged review. Last phase. |
| Order | PR-1 abstraction + shared registration → PR-2 Gmail read + review → PR-3 Google Tasks → PR-4 dual accounts. Console work (Phase M1, G0) runs in parallel. |

## Answer to "do I need to publish?"

No store listing and no Microsoft 365 App Certification. Three things are needed:

1. **Multitenant registration.** Already true ("accounts in any organizational directory"). Free.
2. **Publisher verification.** Free, no license. Prerequisites: Partner Center account in the
   Microsoft AI Cloud Partner Program (free tier; business verification 3 to 5 business
   days); the registration owned by a work/school tenant associated with that Partner
   global account; publisher domain = a DNS-verified custom domain in that tenant (not
   `*.onmicrosoft.com`); the verifying user holds Application Administrator in Entra and
   Partner Admin or Account Admin in Partner Center, signed in with MFA. Without it, apps
   registered after 2020-11-08 show "unverified" in other tenants and, under default
   risk-based step-up consent, users there cannot consent at all.
3. **Admin consent path.** `Mail.Read` is not low-impact by default, so many tenants need an
   admin to consent even to a verified publisher. `Group.ReadBasic.All` and
   `Group-Conversation.Read.All` always need admin consent. The app must show the
   admin-consent URL.

Cost (verified on learn.microsoft.com and support.google.com, 2026-09-27):

| Item | Cost |
|---|---|
| Entra registration, multitenant setting, publisher verification | $0 |
| Partner Center membership, base tier | $0 |
| Custom domain | owner already has one |
| Google Cloud project, OAuth client, Testing status, Google's review | $0 |
| CASA lab assessment, only when leaving Testing with the restricted Gmail scope | ~$500 to $4,500 per year, deferred |

## Phase M1: Microsoft console work (owner, no code)

1. Pick the publisher tenant; add and DNS-verify the custom domain.
2. Enroll in the Microsoft AI Cloud Partner Program in Partner Center; complete business
   verification; record the Partner One ID (global account, not a location ID).
3. Associate the Entra tenant that owns the OpenLoops registration with that account.
4. Registration: publisher domain = custom domain; account type = any organizational
   directory; redirect `http://localhost` under Mobile and desktop; remove unused
   platforms; branding (name, logo, homepage, privacy statement, terms, support contact).
   Delegated permissions: `User.Read`, `Mail.Read`, `Mail.Read.Shared`, `Tasks.ReadWrite`,
   `Group.ReadBasic.All`, `Group-Conversation.Read.All`.
5. Entra portal → Branding & properties → Publisher verification → enter Partner One ID.
6. Add a second registration owner; write the owner-recovery note (ADR-012 control).

## Phase G0: Google console work (owner, no code)

1. Google Cloud project "OpenLoops"; enable Gmail API and Google Tasks API.
2. OAuth consent screen: External; Testing; app name, support email, homepage, privacy
   policy URL; scopes `openid`, `email`, `profile`,
   `https://www.googleapis.com/auth/gmail.readonly`, `https://www.googleapis.com/auth/tasks`.
3. Add test users by Gmail address (cap 100). Owner first.
4. OAuth client, type Desktop app. Record client ID and secret for build-time injection
   only. Google documents the desktop client secret as not confidential; it still never
   enters the repository.
5. Behaviours to document: unverified-app interstitial once per user; 7-day authorization
   expiry (moot: the app keeps no refresh token); 100-user cap.
6. Upgrade path, deferred and paid: verification submission, CASA at the assurance level
   Google assigns, annual renewal.

## Design constraints that shape the code

- **Google code lives inside `openloops-graph` under `src/live/google/`.** No new crate,
  no rename. Reasons: `check-authentication-boundary.ps1:227-256`,
  `check-incremental-authorization.ps1:191-213`, `check-synchronization-boundary.ps1:67`
  allow network deps and `TcpStream`/HTTP symbols only in `openloops-graph`
  `src/{callback.rs,live.rs,live/*}`; `check-protected-state-boundary.ps1:380`
  fingerprints every `Cargo.toml`, `Cargo.lock`, and `openloops-desktop/src/main.rs` but
  not `live/*`. Add a module-doc note in `live.rs` naming the Google submodule.
- **No new Cargo dependencies in any PR.** base64url via `crate::encoding`, JSON via
  `serde_json`, dates via `chrono`. Check `git diff --stat Cargo.lock` is empty per PR.
- **Microsoft HMAC inputs do not change.** `Decisions::fingerprint` (`loop_state.rs:97-122`)
  and `Relations::thread_key` (`link_state.rs:107`) take `account` and ids as strings;
  Microsoft `account` stays the bare Graph `/me.id`. Google accounts are
  `"google:" + OIDC sub`.
- **Reuse controls as modes; no new button kinds** (repo rule).
- **Shipped identifiers are build-time injected**, never committed:
  `option_env!("OPENLOOPS_MS_CLIENT_ID")`, `option_env!("OPENLOOPS_GOOGLE_CLIENT_ID")`,
  `option_env!("OPENLOOPS_GOOGLE_CLIENT_SECRET")`. Source builds without them behave as
  today (BYO only). Keeps ADR-012's "no real client ID in the repository" literally true
  and the public-repo canary green (its secret rule at `check-public-repo.ps1:153`
  matches only quoted literals).

## PR-1: provider abstraction + shared registration (`feat/mail-provider-abstraction`)

New `crates/openloops-graph/src/live/provider.rs`:
- `enum MailProvider { Microsoft (default), Google }` with `service_name()`
  ("Microsoft 365" / "Google"), `mail_client_name()` ("Outlook" / "Gmail"),
  `tasks_name()`, `tasks_url()`, `is_trusted_message_link(url)`.
- `enum AccountConfig { Microsoft(ConnectionConfig), Google(GoogleConfig) }` with
  `provider()`, `check_connection()`, `load_recent_with()`, `load_sources_with()`. Enum
  dispatch, not trait objects. `GoogleConfig` is a stub in PR-1 (filled in PR-2).
- `struct ProviderLoad { provider, sources: Result<Vec<SourceReview>, ConnectionError> }`
  and `load_all(&[AccountConfig], cache, progress) -> Vec<ProviderLoad>`.

New `crates/openloops-graph/src/live/registration.rs`:
- `microsoft() -> Option<MicrosoftRegistration { client_id }>` from `option_env!`,
  validated by the existing `valid_application_id` (`live.rs:211-221`).
- `google() -> Option<GoogleRegistration { client_id, client_secret }>`.
- `microsoft_admin_consent_url(client_id) -> String`:
  `https://login.microsoftonline.com/organizations/v2.0/adminconsent?client_id={id}&scope={scopes}&redirect_uri=http%3A%2F%2Flocalhost`.
  Document that after consent the admin's browser lands on an unreachable localhost page;
  consent is still recorded.
- Unit test: values come only from `option_env!`; precedence BYO field > injected > none.

Neutral fields (`live/review.rs`): add `provider: MailProvider` to `MailItem` (17-38),
`SourceReview` (55-64), `UserIdentity` (41-46). Default is Microsoft so `synthetic()`
(`review_scan.rs:7719`) and fixtures compile unchanged. `ReviewMessage`
(`review_scan.rs:51-74`) gains `provider`, copied in `prepare()` (2145).

Sessions (`live.rs`): `static SESSIONS: Mutex<[Option<Session>; 2]>` indexed by provider;
`clear_session(provider)`, `clear_all_sessions()`, `has_session(provider)`. `CONNECTING`
stays one global. Callers: `app_model.rs:349` → `clear_all_sessions()`; `slint_ui.rs:827`
→ `clear_session(Microsoft)`; `slint_review.rs:1629,1971` → per provider.

OAuth parametrisation (`live.rs`): `struct OAuthEndpoints { authorize, token,
redirect_host, client_secret: Option<Secret>, extra_params, normalize_scope }`;
`authorize_with(endpoints, client_id, scopes)`. The token-URI confinement at `live.rs:425`
compares against `endpoints.token`. `callback.rs`: `enum RedirectHost { Localhost,
LoopbackIp }`; `Listener::bind(host)`; `redirect_uri()` and the `Host` header check
follow it. Microsoft = `Localhost` (matches its registration); Google = `LoopbackIp`.

`ConnectionError` Display (`live.rs:84-122`): provider-neutral wording ("The mail service
returned HTTP 403"). Update the three asserted strings in the test at `live.rs:740`.

Deep links: `is_outlook_link` (`app_model.rs:1161`) → `is_trusted_message_link` accepting
the four Outlook prefixes plus `https://mail.google.com/`; `gated_outlook_url`
(`slint_review.rs:230`) → `gated_message_url`; `on_open_external` (2487) accepts any
`MailProvider::tasks_url()`. Slint structs carrying `url` gain `url-label`; Rust fills
"Open message in {mail_client_name}" (replaces literals at `review.slint:393,402,408,562,626`).

`AccountDisplay::connected(&[MailProvider])` (`app_model.rs:1183`): "Signed in ·
Microsoft 365", "· Google", or "· Microsoft 365 + Google"; `chrome.slint:38` binds a new
`account-text` property.

Scan-scope collision fix now: `ConversationKey = (account, conversation)` replaces the bare
conversation-id sets in `ScanScope::Incremental` (`review_scan.rs:1687-1707, 3262, 4323`),
`Outcome::{RetryMail,RetryScan,CheckScan}`, `append_sources` return, `merge_scan`,
`retryable_conversations`, `prior_open_items`.

Microsoft-only heuristics: `merge_threads_with_rules` (`review_scan.rs:7023`) gets
`authoritative: bool` on `ThreadGroup`, true for non-Microsoft; never merges those.
`event` stays Microsoft-only.

`Reminder::Created/Completed` (`loop_state.rs:51-76`) gain `provider`; encoding bytes 0..3
unchanged (Microsoft), 4/5 = Google. `MAGIC` unchanged.

Desktop renames: `Outcome::Microsoft(..)` → `Outcome::Connection(MailProvider, ..)`;
`Service::Microsoft` → `Service::Mailbox`; `AppModel` gains `google: Status`.
`AppModel::effective_microsoft_client_id()` = BYO trimmed if non-empty, else
`registration::microsoft()`.

Sources UI (`ui/sources.slint:141-160`): with a shipped ID, the client-ID field moves into
a disclosure "Use my organization's own registration" (pattern at 147-152), and a read-only
monospace `Field { label: "Admin consent link" }` shows the URL (Slint has no clipboard
without a dependency; users select-and-copy). "Open Microsoft Entra" stays for BYO.

Consent-failure mapping: AADSTS65001, 90094, 650052/650056 → fixed content-free
`ConnectionError` texts naming admin consent or BYO. Tests via `scripted_server`.

Settings (`settings.rs`, append-only field ladder, no v2 record): field 13
`google_client_id`, 14 `google_client_secret` (`Zeroizing<String>`), 15 `mail_providers`
tag (`ms`, `google`, `ms,google`; default `ms`). Roughly +130 bytes against the 2,560 cap.
Extend the boundary test.

Governance (owner-approved commit, same PR): ADR-012 amendment (shared registration
enabled via build-time injection; BYO fallback; admin-consent link);
`contracts/distribution/registration-boundary.json` status strings and
`shared_registration_enabled`; `tools/check-distribution-registration-boundary.ps1` phrase
list (line 154), status literals (71-73), governance capability
`shared_project_registration.state`, and hash literals (41, 148-153, which also pin
`product-spec.md`, `implementation-plan.md`, `prd-traceability.md`);
`docs/native-setup.md:91-93`, `docs/live-connection.md:7-21`.

## PR-2: Google OAuth + Gmail loader + review (`feat/google-gmail-provider`)

Carried over from the PR-1b review: make `Service::Mailbox` carry the provider
(`Service::Mailbox(MailProvider)`) so a worker panic during a Google check is reported on
the Google status, not the Microsoft one (`app_model.rs` `pending_disconnected`); rename
`microsoft_status()` to a provider-neutral name now that it formats Google reports too.

Files: `live/google/mod.rs` (`GoogleConfig`, endpoints, scopes, `with_google_session`,
`check_connection`), `live/google/gmail.rs` (identity, listing, hydration, `load_*`),
`live/google/mime.rs`, `live/test_support.rs` (`#[cfg(test)]`, fake servers moved from
`review.rs:1383-1476`, deduplicating `reminders.rs:336`, plus a path-routed
`routed_server` for out-of-order hydration workers).

OAuth: authorize `https://accounts.google.com/o/oauth2/v2/auth`, token
`https://oauth2.googleapis.com/token`; `BasicClient` with `set_client_secret` and
`AuthType::RequestBody`; extras `access_type=online`, `prompt=select_account`,
`include_granted_scopes=true`. Google returns a refresh token to installed apps regardless
of `access_type`: drop and zeroize it immediately; store only `access_token`/`expires_at`.
`GoogleConfig::new(client_id, secret)` validates `^\d+-[a-z0-9]+\.apps\.googleusercontent\.com$`
and a non-empty control-free secret. Scopes: `openid email profile` + `gmail.readonly`
(mail) or `tasks` (reminders); the existing scope-union logic (`live.rs:321-346`) gives
incremental consent.

Gmail flow (`gmail::load_sources_with`), all functions taking `origin` for tests:
1. `GET https://openidconnect.googleapis.com/v1/userinfo` → `UserIdentity { account:
   "google:{sub}", addresses: [email], display_name, given_name }`. Only the primary
   address is known (send-as aliases need `gmail.settings.basic`, not requested; document).
2. Per label `INBOX`, `SENT`: `GET /gmail/v1/users/<me>/messages?labelIds={L}&q=newer_than:30d&maxResults=100[&pageToken]`
   (`<me>` is the literal segment `me`; written this way because the repo gate rejects the bare path),
   ≤10 pages, `partial` when the 100 cap is hit with a `nextPageToken`.
3. Reuse `add_hydrated` (`review.rs:942`, made `pub(super)`; needs only `id`). Cache hits
   on `(account, id)` skip the fetch. Misses: `GET /gmail/v1/users/<me>/messages/{id}?format=full`.
4. `hydrate` → `MailItem { provider: Google, id, conversation: threadId, web_link:
   https://mail.google.com/mail/u/0/#all/{id}, sent: label SENT, team: false, event: None,
   received: internalDate → RFC3339 UTC }`; drop rows older than `cutoff_timestamp()`;
   `ResponseTooLarge` → `MessageTooLarge` (parity with the 1 MiB bound).
5. `fetch_from_origin_with_policy` (`review.rs:308`) → new
   `fetch_from_origin_with_headers(.., headers)`; Microsoft passes the `Prefer` pair,
   Gmail passes none.

`mime.rs`: `select_body(payload)` depth-first over `parts`, skip attachments (non-empty
`filename` or `Content-Disposition: attachment`), prefer `text/html` then `text/plain`;
`decode_part` base64url via `crate::encoding`, charsets utf-8, iso-8859-1, windows-1252,
else lossy; 131,072-char bound like `item()` (`review.rs:564`); `header()` case-insensitive;
`decode_rfc2047` B/Q for utf-8/latin1; `parse_address_list` matching `item()` formatting.

Source labels: `"Gmail / Inbox"`, `"Gmail / Sent"` (cannot collide with Microsoft labels;
`source_selected`'s `rsplit_once(" / ")` keeps working).

Sources UI: card 1 becomes "Connect your mailbox" with a `Segmented { "Microsoft 365",
"Google" }` (pattern at `sources.slint:167`). Google mode: optional `google-client-id` and
`google-client-secret` overrides (password `Field` with the existing "Show key" pattern),
`LinkButton "Open Google Cloud console"`, helper text about the unverified-app screen and
test-user list. One `PrimaryButton "Sign in & check inbox"` acts on the selected mode.
Per-mode status lines. Plumb through `ui/app.slint` and `slint_ui.rs:262-285,365`.

Governance (owner-approved): ADR-002 amendment defining "client secret" as
confidential-client material that authenticates the binary; the Google installed-app
parameter is a non-confidential registration parameter and PKCE remains required;
`contracts/identity/authentication-boundary.json` adds a `providers` block (Google
origins; `client_secret_handling: non_confidential_installed_app_parameter`) while
`confidential_client_material` stays prohibited; `check-authentication-boundary.ps1`
hash re-pin. ADR-001:43 clarifying sentence. ADR-003: Google scope table (prefer a new
`contracts/identity/google-scope-boundary.json` over editing the exact-set
`permission_rows`). New `docs/adr/ADR-015-google-provider.md` (abstraction, Testing-status
registration, personal-Gmail target, no refresh token, `google:` prefix, MIME limits) +
`contracts/governance/capabilities.json` + `check-governance.ps1` ADR inventory.
`contracts/support/support-matrix.json`: split `non_microsoft_mailbox` into
`google_personal_gmail` (`gated_target`, `disabled`) and the rest; `check-support-matrix.ps1:123`
exact rule + hash. `docs/product-spec.md:657` drop "Gmail" and "multiple accounts"
(re-pins `check-distribution…:151`). Docs: `live-connection.md`, `native-setup.md`
(env-var build instructions), `support-matrix.md:41`, `inbox-review.md`.

## PR-3: Google Tasks reminders (`feat/google-tasks-reminders`)

`live/google/tasks.rs` mirroring `reminders.rs`: `create`, `complete`, `check_status`,
reusing `ReminderRequest` (gains `provider`), `ReminderOutcome`, `ReminderFailure`,
`ReminderCompletionOutcome`, `TaskStatusOutcome`; `valid_graph_id` → `valid_remote_id`.
- `create`: session with `tasks` scope → userinfo `sub` must equal `request.account`
  (`AccountMismatch`) → `GET /tasks/v1/users/<@me>/lists?maxResults=100` (angle brackets are not literal; the repo gate rejects the bare path), first list is the
  default (`DefaultListNotFound` if empty or paging incomplete) → `POST /tasks/v1/lists/{list}/tasks`
  `{title, status: "needsAction", due: "YYYY-MM-DDT00:00:00.000Z", notes: "Created after
  review in OpenLoops.\nReminder time: {local}\nOpenLoops reference: {marker}"}`. Success is
  HTTP 200. Store the concrete list id.
- `complete`: `PATCH .../tasks/{t}` `{"status":"completed"}` → 200. `check_status`: `GET`;
  404 → not found.

Due-time limitation (Google records date only, no alert) surfaced in: the review draft
(`review.slint:469-486`, new `reminder-service` and `reminder-time-note` properties; Google
text "Google Tasks saves the date only and does not alert you; the time you pick is written
into the task's notes"), `reminder_outcome` (`app_model.rs:736-738`) via `tasks_name()`,
and reconcile links (`review.slint:495,508`) via per-card `tasks_url()`.

Desktop dispatch: `ReminderDraft` (`review_model.rs:46`) gains `provider`;
`on_create_reminder` (`slint_review.rs:2437`), `dispatch_pending_reminder` (1706), the
Handled-completion path (1791-1807), `dispatch_reminder_sync` (`app_model.rs:955-986`)
build an `AccountConfig` via new `AppModel::account_config(provider)`;
`reminder_sync_checks` (`review_model.rs:1059`) groups by provider.

Governance (owner-approved): ADR-009 amendment + `contracts/reminder/adapter-boundary.json`
Google Tasks row with the date-only limitation; `check-reminder-adapter-boundary.ps1`
hash re-pins (two SHA-256 sites).

## PR-4: both providers at once (`feat/dual-provider-review`)

- `start_mail_load` (`slint_review.rs:1608`) builds `Vec<AccountConfig>` from
  `AppModel::enabled_accounts()`; `Outcome::Mail(Vec<ProviderLoad>)`, `CheckMail { loads }`,
  `RetryMail { loads, .. }`. `check_mail_outcome` (`app_model.rs:609`) extends the cache
  and appends sources from every `Ok`, records one status line per `Err` prefixed with
  `service_name()`; one provider's failure no longer discards the other's results.
- `LoadProgress::begin(total)` once in `load_all`; loaders only increment.
- `ReviewState.failed_sources: BTreeSet<(MailProvider, String)>`; `on_retry_failed`
  (`slint_review.rs:1938`) groups labels by provider.
- Sources UI: one `Checkbox "Include in scans"` per mode bound to `mail_providers`;
  heading pills "Microsoft connected" / "Google connected".
- Sign-ins run sequentially inside the one job (single `CONNECTING`). Busy copy: "Complete
  Microsoft and Google sign-in; then downloading…".
- Governance: `contracts/governance/capabilities.json` row if a dual-account capability is
  added; ADR-001:46 amendment ("multiple accounts" no longer excluded for the two-provider
  case).

## Follow-ups recorded during PR-4 review (not in scope of PR-4)

- Disabling a provider removes its messages and cards, but a surviving card of the other
  provider may keep `mentions`, `resolution`, `deadline` or `event` anchors that point at
  removed handles, and a survivor whose folded duplicate mention belonged to the removed
  provider is dropped. Display staleness only until the next scan; lookups return `Option`.
- "Reload saved settings" can change `mail_providers` without clearing the dropped
  provider's session, cache, or review data (pre-existing shape; now visible with two
  providers).
- With two providers enabled, "Scan inboxes" prompts both sign-ins back to back. A later
  change could let the user pick which provider to sign in first.

## Verification

Per PR, in this order, reading each result before committing (never gate and commit in
one command):

```
cargo fmt --all -- --check
cargo clippy -p openloops-graph --features live-connection --all-targets -- -D warnings
cargo clippy -p openloops-desktop --features native-ui,ui-screenshot --all-targets -- -D warnings
cargo test -p openloops-graph --features live-connection --offline
cargo test -p openloops-desktop --features native-ui --offline -- --test-threads=1
pwsh ./tools/check-authentication-boundary.ps1
pwsh ./tools/check-incremental-authorization.ps1
pwsh ./tools/check-synchronization-boundary.ps1
pwsh ./tools/check-evidence-identity-boundary.ps1
pwsh ./tools/check-protected-state-boundary.ps1
pwsh ./tools/check-support-matrix.ps1
pwsh ./tools/check-distribution-registration-boundary.ps1
pwsh ./tools/check-reminder-adapter-boundary.ps1
pwsh ./tools/check-governance.ps1
git add -A; pwsh ./tools/check-public-repo.ps1 -Mode Staged
git diff --stat main -- Cargo.lock   # must be empty
```

Tests added per PR:
- PR-1: per-provider session slots; `authorize_with` rejects the other provider's token
  URI; `RedirectHost::LoopbackIp` host check; two accounts sharing a conversation id scan
  only the targeted one; `merge_threads` never merges Google groups; reminder bytes 2/3
  decode Microsoft and 4/5 Google with old fixture bytes unchanged;
  `is_trusted_message_link` accepts `mail.google.com` and rejects `https://evil/mail.google.com/`;
  `AccountDisplay` three names; settings fields 13–15 boundaries; registration precedence;
  AADSTS error mapping.
- PR-2: `routed_server` cases: userinfo → two folders → three messages (html
  multipart/alternative, plain-only, over-size → `MessageTooLarge`); paging to the 100 cap;
  cutoff drop; cache hit skips the GET (assert call count); 401 → `Unauthorized`; bad
  base64 → `ResourceUnavailable`; RFC 2047 subject; latin1 body; attachments skipped;
  `web_link` format; `conversation == threadId`; `sent` for SENT; `GoogleConfig::new`
  rejects malformed ids; `prepare()` labels "Open message in Gmail".
- PR-3: account mismatch makes no POST; empty list set → `DefaultListNotFound`; 200 →
  `Created { provider: Google }`; PATCH/GET paths; body has date-only `due` and the time in
  `notes`; per-provider `reminder_outcome` text; `reminder_sync_checks` grouping.
- PR-4: `load_all` continues after one provider fails; mixed `Ok`/`Err` in
  `check_mail_outcome`; retry routing by `(provider, label)`; progress totals; title text.

Manual smoke, owner-run, free (real accounts, no paid API): build with
`OPENLOOPS_MS_CLIENT_ID`, `OPENLOOPS_GOOGLE_CLIENT_ID`, `OPENLOOPS_GOOGLE_CLIENT_SECRET`
set; confirm the Google interstitial for a listed test user, a Gmail thread grouped by
`threadId`, a created Google Task showing the date with the time in notes; then the
Microsoft fresh-tenant check (a free second Entra tenant with default consent settings:
verified badge visible; user consent succeeds or the "needs admin approval" page appears
and the admin-consent URL works). Record outcomes in `docs/live-connection.md`.

## Risks

1. **`client_secret` wording.** ADR-001/002 prohibit "a client secret". Resolution is the
   ADR-002 definition amendment above; the parameter authenticates nothing beyond PKCE.
2. **Gmail 1 MiB responses.** `format=full` inlines small attachment bodies; oversize
   messages are skipped as `MessageTooLarge`. Document in `inbox-review.md`.
3. **Charset fidelity.** Hand-rolled decoding covers UTF-8, Latin-1, Windows-1252, B/Q
   encoded words; others fall back to lossy UTF-8. Documented limitation; a dependency would
   re-pin the protected-state fingerprint.
4. **`main.rs` is fingerprinted.** Keep `openloops_graph::live::{ConnectionConfig,
   SharedScope, check_connection}` exported under those names.
5. **Testing status ceiling.** 100 named users; the owner adds each address by hand.
   Leaving Testing is the paid CASA path.
6. **Checker edits.** A classifier blocks Claude from editing `check-*.ps1`; Codex makes
   those edits, and each lands in an owner-approved commit with the re-run `test-*.ps1`.
7. **Gmail path literals trip the public-repo gate.** `tools/check-public-repo.ps1`'s
   "personal Unix home-directory path" rule (line 173) matches any `/users/<x>/` segment,
   case-insensitively, so the Rust literals for the Gmail and Tasks `users/<me>/...` and
   `users/<@me>/...` paths in PR-2/PR-3 will be blocked. Resolution, owner-approved in
   PR-2: add `me/` and `@me/` to that rule's negative lookahead (alongside `Shared/`,
   `runner/`, `sandbox/`), with a test in `tools/test-public-repo.ps1` (or equivalent)
   showing a real home path is still caught. Do not build the path from pieces to dodge
   the gate.

## Execution notes

- First execution step: commit this design as
  `docs/plans/2026-09-27-google-provider-and-shared-registration.md` on the PR-1 branch.
- Codex does the implementation (`codex exec -c model_reasoning_effort=medium`, worktree
  per PR). Claude reviews diffs, runs the gates, and inspects test output before any
  "done" claim (`superpowers:verification-before-completion`).
- Do not skip or reorder PR phases without asking the owner.
