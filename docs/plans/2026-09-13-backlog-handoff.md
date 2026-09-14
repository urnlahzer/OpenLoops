# Backlog handoff: after the Slint redesign (September 2026)

State on 2026-09-13: `main` = b869be9. PR #20 (Slint UI redesign, 13 commits) and PR #21 (event-passed closure rules) are merged; `main` passes fmt, clippy, 273 desktop tests and 128 graph tests. The owner runs `target\release\openloops-ui.exe` (release profile, about 17.6 MB); the debug build is for development only.

This document hands over the five open items in priority order. Each item records what was observed, what is already known about the cause, where the code is, the proposed change, the tests that prove it, and the gates that must pass. Items 1 and 2 need the owner's input where marked.

## How to work in this repo (learned the hard way)

- Branch from `main` for each item; PR to `main`; ruleset requires the `scan-history` check.
- `cargo` is not on PATH in tool shells: `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`. Use `--offline`; every crate is cached. Feature flags: desktop `--features native-ui` (`ui-screenshot` adds the preview fixture), graph `--features live-connection`. Desktop tests need `-- --test-threads=1` (two tests share the Windows credential store).
- Build the release binary alone with `-j 2`; the LTO link gets killed on this machine under memory pressure. Delete `target\debug\incremental` when disk is low (it reached 15 GB).
- Rendering checks: the Slint window can be captured even on a locked desktop with `PrintWindow(PW_RENDERFULLCONTENT)` from a DPI-aware process (the display is 150 %); the session scratchpad script `shot2.ps1` does this. `--preview-review --stay` (under `ui-screenshot`) loads `review_model::layout_fixture()`; `OPENLOOPS_PREVIEW_PROVIDER=openrouter`, `OPENLOOPS_PREVIEW_CONNECTED=1` and `OPENLOOPS_PREVIEW_BUSY=1` seed other states. When the desktop is unlocked, real clicks can be injected with `SetCursorPos` + `mouse_event`; that is how the one-click fix was verified. Codex's sandbox cannot capture renders; a layout change must be rendered by the orchestrator before it is accepted.
- Slint rules that cost review rounds: layout stretch works only when the layout has no `alignment`; `visible` does not affect layout; explicit `x`/`y` children are excluded from layout info; assigning a property at runtime drops its binding; a wrapping text must be the shared `WrappedText` (a Rectangle whose own height is its label's wrapped height) and must not be used as a `HorizontalLayout` child because it fixes its width to 100 %; a `FocusScope` stacked over a `TouchArea` needs `focus-on-click: false` or it swallows the first click.
- Three repo gates fail on `main` and are not caused by any recent change (item 5).
- Codex CLI has hit its usage limit repeatedly; the owner's instruction is to use Codex for execution and to ask before falling back to a Claude subagent.

## 1. Retry only what failed

Observed (2026-09-10): after a scan, the coverage panel showed "Personal mailbox / Inbox: Microsoft could not be reached over a secure connection" with 0 messages, Sent Items and Groups loaded, and two conversations "The provider did not answer in time". The only actions are "Scan inboxes" (reload every source and rescan everything) and "Rescan loaded mail" (rescan every loaded conversation). Neither retries only the failed parts. Since then, the transport wording and one automatic retry landed in PR #20 (`crates/openloops-graph/src/live/review.rs`, `fetch_from_origin_with_policy`), and the scan summary now counts conversations (`ScanResult::conversation_count`, `analyzed_conversations`, `failed_conversations`, `not_started_conversations` in `crates/openloops-desktop/src/review_scan.rs`); the failure records are still free text (`ScanResult::failures: Vec<String>`, built by `failure_line`).

Scope:
1. Structured failures. Add `failed_conversations_detail: Vec<ConversationFailure { conversation: String, subject_short: String, reason: FailureReason { Timeout, RateLimited, Transport, Quota, Provider(String), Panicked, NotStarted } }>` to `ScanResult` (keep `failures` for display), and `SourceReview::failed` on the graph side (`crates/openloops-graph/src/live/review.rs`, `SourceReview` already has `errors` and `message_errors`).
2. Subset scan and merge. `review_scan::scan` takes an optional conversation filter; `ReviewState::merge_scan(result)` (in `crates/openloops-desktop/src/review_model.rs`) replaces the items of retried conversations by conversation id, keeps the others, preserves decisions by fingerprint, recomputes `scan_summary`, coverage notes and `scan_incomplete`, and re-runs `close_passed_events` over the merged analysis (it is idempotent).
3. Source retry. `load_recent` takes a source filter; messages from retried sources are appended to `review.messages` (dedupe by message handle) and their conversations are scanned through (2).
4. UI. A "Retry failed (N)" button in the coverage panel (`crates/openloops-desktop/ui/review.slint`, adapter `src/slint_review.rs`), enabled when N > 0 and not busy, running source retries first (sign-in may be required) then conversation retries as one `Service::Review` job with the normal strip progress. Status after: "Retried N; M still failing." (new copy; the plan's "no invented copy" rule was for the redesign, this is a feature).
5. Rate limits. The model contract forbids re-sending content after a 429; `RateLimited` conversations are retried only after the rate window, or excluded with a note. Timeouts, transport failures, 5xx and not-started conversations retry freely. Quota (402) means the provider account is exhausted; do not retry until the owner acts.
6. Tests: merge semantics (replace, keep, decisions preserved), filter correctness, coverage recomputation, button enable rules, no message content in failure records.

Gates: full desktop and graph test sets, clippy for both crates, `tools/check-model-boundary.ps1`, `tools/check-privacy-boundary.ps1`, public-repo gate.

## 2. Microsoft To Do reminder "does not work"

Observed (2026-09-10): the owner created reminders and found no tasks in To Do. The code path is intact: `crates/openloops-graph/src/live/reminders.rs::create` resolves the account identity, requires it to match the scanned account, finds the list with `wellknownListName == "defaultList"`, posts one task and reports `Created` only on HTTP 201; `AppModel::reminder_outcome` writes the outcome into the action status.

What is not known: what the card showed afterwards. The three candidates map to three status texts:
- "No reminder was created: …" with an authorization error: the app registration lacks the delegated `Tasks.ReadWrite` permission, or the browser signed into a different account than the one scanned (identity mismatch).
- "Microsoft did not confirm the write. Check To Do before trying again…": the POST was sent but no 201 came back.
- "Reminder created in your Microsoft To Do Tasks list.": the task exists in the default "Tasks" list; check that list, not a custom one.

Since PR #20 the reminder shares the in-process session (`live::SESSION`); the first reminder adds `Tasks.ReadWrite` through one extra sign-in, later ones reuse it.

Next step: ask the owner for the exact status text and whether the second browser prompt appeared, then fix the identified cause. If it is the app registration, the fix is in Entra, not in code; document the required delegated permissions in `docs/native-setup.md` with a screenshot-free checklist.

## 3. Owner and waiting-party attribution for recap-sourced loops

Observed (2026-09-10): an action item taken from a Fathom meeting summary showed "Waiting on this: <the owner's own address>", because the summary's sender is a service and the model had no real counterparty. Attribution comes from the model output (`crates/openloops-inference/src/expectations.rs`: `owner`, `waiting_party`) and is projected by `review_scan::waiting_party_address` and the "Not established" placeholder.

Proposed deterministic post-processing (no prompt change, so the model contract stays untouched): when the evidence message is a recap artifact (`review_scan::is_meeting_recap_artifact`, added by PR #21) and the reported waiting party resolves to one of the owner's own addresses or to the service sender, replace it with "Not established" and mark the owner as "You (suggested)" unless the action names a counterparty. Tests with a Fathom-shaped message. Open question for the owner: whether action items in a recap should default to "Responsible: You" (they are usually the owner's own commitments).

## 4. Pinned-shortcut icon

Observed: the taskbar showed a generic icon after the toolkit change. PR #20 set the window icon at runtime (`ui/app.slint`, `icon: @image-url("assets/openloops-256.png")`, an original open-loop mark), which fixes the running window and taskbar button. A pinned shortcut reads the icon resource embedded in the exe, which the crate does not have. Adding one needs a build-time resource step (`winresource` or `embed-resource`); neither crate is in the offline cache, and the desktop crate's dependencies are governed by the build-skeleton and authentication manifests, so this is an owner decision plus an online `cargo fetch`. Alternative with no dependency: ship an `.ico` next to the exe and document changing the shortcut's icon by hand.

## 5. Repo gates failing on `main`

`tools/check-reminder-adapter-boundary.ps1` (P0-REMINDER-INVENTORY-001), `tools/check-authentication-boundary.ps1` (P0-AUTH-DEPENDENCIES-001) and `tools/check-synchronization-boundary.ps1` (P0-SYNC-CROSS-CONTRACT-001) fail on a clean `main` under PowerShell 7.6.5. Each compares a SHA-256 of the manifest JSON re-serialised with `ConvertTo-Json -Depth 100 -Compress` against a hash literal in the script (lines 35, 50 and 23 respectively). The manifests were not changed by recent PRs. The likely cause is a change in `ConvertTo-Json` output between PowerShell versions; verify by hashing under the PowerShell version that produced the literals, then either update the literals with an owner-approved commit or make the canonicalisation version-independent (for example, hash the file bytes after normalising line endings, as `check-public-repo.ps1` does). These are governance checks; treat the change as an owner decision.

## Known constraints to keep

- Disclosure and status strings are preserved verbatim from the pre-redesign code (spec §8); labels follow the design reference.
- The desktop crate has a package-level `unsafe_code = "allow"` solely for Slint's generated module; a unit test fails if any hand-written source contains the token. The workspace root still forbids unsafe code.
- No token persistence: the Graph session lives in process memory only (ADR-002 amendment in PR #20).
- Recap and transcription services (Fathom, Otter, Fireflies, Read.ai, tl;dv, Grain, Avoma, Gong, Chorus, notetaker bots on Zoom, Teams and Meet) are records of past meetings: their action items become loops; their meetings never close loops (PR #21).
