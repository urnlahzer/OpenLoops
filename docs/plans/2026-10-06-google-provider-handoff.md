# Handoff: Google provider and shared Microsoft registration (October 2026)

State on 2026-10-06: `main` = 53a4871. Every code slice of
`docs/plans/2026-09-27-google-provider-and-shared-registration.md` is merged:

| PR | Content | Merged as |
|---|---|---|
| #58 | 13 scanning tests re-derived from the Jev registry (were red on main) | 482d7d4 |
| #59 | PR-1a/1b: provider abstraction, per-provider sessions, shipped-registration hooks, account-qualified scan scope, provider-aware reminders and links, settings fields | ac9d71d |
| #60 | PR-1c: Sources shared-registration mode, admin-consent link, ADR-012 amendment | d474309 |
| #61 | PR-2: Google OAuth, Gmail loader, Sources Google mode; public-repo gate allowance for `users/me/` paths | 3d7ad26 |
| #63 | PR-3 + PR-4 (stacked): Google Tasks reminders, ADR-009 amendment, both providers connected at once | 53a4871 |

#62 (PR-3 alone) was closed unmerged because #63 carried it. Gate state at 53a4871: fmt,
clippy on both crates, graph 205 tests, desktop 479 tests, public-repo gate, lockfile
unchanged. Four `tools/check-*.ps1` checks fail on `main` and have since before this work:
evidence-identity CLAIMS, protected-state CROSS-CONTRACT and CLAIMS, distribution INVENTORY,
reminder INVENTORY. Compare a branch against that list, not against "all green".

Nothing in the Google path has run against a real Google account. Every request and response
shape is pinned by loopback tests only.

## Owner to-dos

### 1. Review two contract edits that are already on main
- `docs/adr/ADR-009-reminder-adapters.md`, section "Amendment (2026-10-04)": both reminder
  adapters carry the opaque marker in the task's free-text body (To Do `body.content`,
  Google Tasks `notes`) because neither service offers a proven opaque field. It reached
  `main` through #63, whose title lacked the review flag.
- `docs/adr/ADR-012-distribution-and-registration.md`, section "Amendment (2026-10-02)":
  build-time injection (`OPENLOOPS_MS_CLIENT_ID`, `OPENLOOPS_GOOGLE_CLIENT_ID`,
  `OPENLOOPS_GOOGLE_CLIENT_SECRET` via `option_env!`) is the enablement switch for the
  shared registration; the repository stays placeholder-only. Reviewed in #60.

### 2. Google Cloud console (free; needed before any Gmail test)
1. Create a Google Cloud project. Enable the Gmail API and the Google Tasks API.
2. OAuth consent screen: User type External, publishing status Testing. Fill app name,
   support email, homepage, privacy policy URL. Scopes: `openid`, `email`, `profile`,
   `https://www.googleapis.com/auth/gmail.readonly`, `https://www.googleapis.com/auth/tasks`.
3. Test users: add your Gmail address (cap 100).
4. Credentials: create an OAuth client of type Desktop app. Keep the client ID and secret
   out of the repository. Paste them into the app's Google fields on the Sources screen, or
   set them as the two `OPENLOOPS_GOOGLE_*` variables when building a release.
5. Expect Google's "unverified app" notice once per user while in Testing. Leaving Testing
   needs Google verification plus an annual CASA assessment (about $500 to $4,500 per year);
   deferred by decision.

### 3. Gmail smoke test (after step 2)
1. Build and run: `cargo build -p openloops-desktop --bin openloops-ui --features native-ui --locked`.
2. Sources: Mail provider → Google; paste the client ID and secret; Sign in & check inbox.
   Expect "Personal inbox: access confirmed".
3. Review → Scan inboxes. Expect Gmail cards whose links read "Open message in Gmail" and
   open in Gmail.
4. On a Gmail card: Set Google Tasks reminder…, pick a time, create. Expect the task in Google
   Tasks with the local calendar date and a notes line carrying the time and the OpenLoops
   reference. Mark it done in Google Tasks, then Sync Google Tasks in the app; expect the
   card to reconcile.
5. Enable both providers (Include in scans on each), scan, confirm cards from both mailboxes
   and "Signed in · Microsoft 365 + Google" in the title bar. Disable one provider; its cards
   disappear and the other's decisions stay.
Record outcomes in `docs/live-connection.md`.

### 4. Microsoft publisher verification (free)
1. Pick the publisher tenant; add and DNS-verify your custom domain there.
2. Partner Center: enroll in the Microsoft AI Cloud Partner Program (free tier), complete
   business verification (3 to 5 business days), record the Partner One ID of the global
   account.
3. Associate the tenant that owns the OpenLoops registration with that Partner global
   account.
4. On the registration: publisher domain = the custom domain; account type = any
   organizational directory; redirect `http://localhost` under Mobile and desktop; remove
   unused platforms; fill branding, privacy statement, terms, support contact. Delegated
   permissions: `User.Read`, `Mail.Read`, `Mail.Read.Shared`, `Tasks.ReadWrite`,
   `Group.ReadBasic.All`, `Group-Conversation.Read.All`.
5. Branding & properties → Publisher verification → enter the Partner One ID.
6. Add a second owner to the registration.
7. Release builds then set `OPENLOOPS_MS_CLIENT_ID`. Until then the app is BYO-only, as today.

### 5. Second-tenant smoke test (after step 4)
Create a free Entra tenant with default consent settings and a test user. Build with
`OPENLOOPS_MS_CLIENT_ID` set, sign in as that user. Expect the verified-publisher badge on
the consent prompt, and either user consent or the "Need admin approval" page; open the
Admin consent link from the Sources screen as the tenant admin and confirm consent is
recorded (the browser lands on an unreachable localhost page afterwards; that is expected).

### 6. Jev registry values flagged by the #58 investigation (unchanged)
`closure.outcome` accept 1.0 / escalate 0.8 means a choice under 0.8 ends the pair with no
noul fallback and the choice can win only at exactly 1.0; `rules.deadline_kind` accept 1.0
almost never auto-applies; the noul comparison label is a fixed 0.5 cut while bands sit at
0.8 to 0.9; three passing tests sit exactly on new cuts.

## Engineering follow-ups (not done)

- Disabling a provider removes its messages and cards, but a surviving card of the other
  provider may keep `mentions`, `resolution`, `deadline` or `event` anchors that point at
  removed handles. Display staleness only until the next scan.
- "Reload saved settings" can change the enabled provider set without clearing the dropped
  provider's session, cache, or review data.
- With both providers enabled, Scan inboxes prompts both sign-ins back to back; no way to
  choose the order.
- Address-list parsing in `live/google/mime.rs` does not handle `<` inside a quoted display
  name, group syntax, or escaped quotes.
- `ConnectionError::ProviderUnavailable` has no production use (one desktop test references it).
- The Slint scope-summary label is a bare `Text`; at very narrow widths "Inbox / Sent (both
  providers)" widens the layout rather than wrapping.
- `main.rs --check-connection` is Microsoft-only.
- Publisher verification is a prerequisite for an eventual Microsoft 365 App Certification;
  nothing in this work starts that.

## How to work on this (what the sessions learned)

- Branch from `main`; one PR per slice; never stack a PR on an unmerged PR unless the
  merge order is fixed (the #62/#63 mix-up came from a stacked PR merged first).
- Gate per PR, one cargo command at a time on this machine: `cargo fmt --all -- --check`;
  `cargo clippy -p openloops-graph --features live-connection --all-targets -- -D warnings`;
  `cargo clippy -p openloops-desktop --features native-ui,ui-screenshot --all-targets -- -D warnings`;
  `cargo test -p openloops-graph --features live-connection`;
  `cargo test -p openloops-desktop --features native-ui -- --test-threads=1`;
  `git add …; pwsh ./tools/check-public-repo.ps1 -Mode Staged`. Then the checkers, compared
  against the four-failure baseline above.
- Codex's sandbox cannot write Windows Credential Manager (three native-store tests fail
  there; they pass in a normal shell) and cannot launch pwsh 7 from a worktree.
- The public-repo gate rejects any `/users/<x>/` path except `me/` and `@me/`, and quoted
  `"access_token":"…"`-style literals; build JSON fixtures with `json!` and variables.
- Google reports the identity scopes as full `userinfo.*` URLs; normalise them or the cached
  session never matches. Gmail `body.data` may be padded base64url. Google Tasks `due` keeps
  the date exactly as sent: always send the user's local calendar date.
- Review pattern that held: implementer (Codex or Opus) → orchestrator gate → read-only Opus
  review with file:line findings → fix pass → gate → commit. Each review of this work found
  at least one real defect the tests could not see.
