# Plan: OpenLoops desktop UI redesign (Fluent companion window)

Date: 2026-09-09. Branch: `feat/fluent-ui` from `main`. PR to `main`.

## Sources of truth

- `docs/design/OpenLoops UI Spec.dc.html` — build specification (tokens, frame, screens, states, accessibility, non-goals).
- `docs/design/OpenLoops Companion.dc.html` — interactive reference rendering (exact layout, spacing, colours, copy, sample data with invented names).
- Existing behaviour and copy in `crates/openloops-desktop/src/{setup_ui,review_ui}.rs`; every disclosure string is preserved verbatim.

## Decisions

1. Toolkit: **Slint 1.17.1** (spec recommendation). Native renderer, no webview, accessibility tree, `.slint` markup mirrors the spec's component tree. Rejected Dioxus: a WebView2 process would hold the API key in DOM state and adds a browser engine to the threat model.
2. Architecture: **toolkit-free view models + thin Slint adapter.** All logic now inside `setup_ui.rs`/`review_ui.rs` (job dispatch and `Outcome` polling, settings persistence flows, status text, card context, ordering, labels, summaries, reminder-draft rules) moves to `app_model.rs` and `review_model.rs` with its tests. New logic the spec adds (group/filter/badge/scan-strip state) lives there too, unit-tested. The Slint adapter (`slint_ui.rs`) only maps models to Slint properties and callbacks to model methods.
3. `deadline_view.rs`, `loop_state.rs`, `settings.rs`, `review_scan.rs` and every other crate are unchanged.
4. Threading model unchanged: one pending job (`Option<Receiver<Outcome>>`), background `std::thread`, polled by a `slint::Timer` at 250 ms while busy; the window is disabled while busy except Stop scan.
5. `eframe`/`egui` and the `image`-based empty-window screenshot are removed. `--preview-review` (synthetic fixture) stays under the `ui-screenshot` feature and saves a PNG through `slint::Window::take_snapshot` when that API is available in 1.17; otherwise the flag only shows the fixture.
6. Links open through the `opener` crate (`=0.8.5`), URLs unchanged (Entra, provider key pages, To Do, Outlook `webLink` only when its host is `outlook.office.com`/`outlook.office365.com`).
7. GPU: the executable path stays `target\debug\openloops-ui.exe`, so the existing `GpuPreference=2` registry entry keeps applying.

## Repository changes

Cargo (`crates/openloops-desktop/Cargo.toml`):
- add `slint = { version = "=1.17.1", default-features = false, features = ["std", "compat-1-2", "backend-winit", "renderer-femtovg", "accessibility"], optional = true }` (verify feature names against the crate's `Cargo.toml` for 1.17.1 before pinning), `opener = { version = "=0.8.5", optional = true }`, `[build-dependencies] slint-build = "=1.17.1"`.
- `native-ui` feature: replace `dep:eframe` with `dep:slint`, `dep:opener`. `ui-screenshot` keeps `dep:image`.
- `build.rs`: when `CARGO_FEATURE_NATIVE_UI` is set, `slint_build::compile_with_config("ui/app.slint", CompilerConfiguration::new().with_style("fluent"))`.

Files:
- `crates/openloops-desktop/ui/tokens.slint` — `global Tokens` with every §2 token (colours, font sizes, radii, spacing).
- `crates/openloops-desktop/ui/widgets.slint` — PrimaryButton, SecondaryButton, SubtleButton, OutlineButton, Segmented, Pill, Callout (warning/success/info), Field (single-line, password, multi-line), Disclosure, Avatar, ProgressBar.
- `crates/openloops-desktop/ui/chrome.slint` — TitleBar (48), NavRail (72), StatusBar (26).
- `crates/openloops-desktop/ui/review.slint` — ReviewScreen: CommandBar, ScanStrip (scanning / finished / idle), ListPane (grouped rows, selection, keyboard), ReadingPane (all §4.4 sections).
- `crates/openloops-desktop/ui/sources.slint` — SourcesScreen (§5).
- `crates/openloops-desktop/ui/app.slint` — `export component AppWindow` composing the frame; exported structs for rows, groups, evidence cards, messages, status lines.
- `crates/openloops-desktop/src/app_model.rs`, `src/review_model.rs`, `src/slint_ui.rs`; `src/bin/openloops-ui.rs` rewritten; `setup_ui.rs` and `review_ui.rs` deleted.
- Docs: `docs/native-setup.md` (toolkit, build, preview flag), `docs/adr/ADR-014-desktop-ui-toolkit.md`, `docs/design/README.md` (what the two files are), `docs/inbox-review.md` (UI paragraphs), `docs/threat-model/privacy-and-local-state.md` (one paragraph: native renderer, no webview, key never leaves Rust).

## Tasks (sequential, one commit each, Codex implements, reviewers verify)

### T1 — Extract toolkit-free models (no visual change)
Move out of `setup_ui.rs`/`review_ui.rs` into `app_model.rs`/`review_model.rs`: `Outcome`, `Service`, `Status`, job start/poll, `save_settings`/`reload_settings`/`forget_settings`, `trim_keys`, `active_key`, `microsoft_status`, `start_scan`, provider/plan state; `CardContext`, `card_context`, `card_rank`/`card_order`, `card_hidden`, `is_past_due`, `status_base_label`, `status_label`, `resolution_*_label`, `event_passed_status_label`, `expectations_summary`, `ReminderDraft`, `reminder_time`, `decision_after_*`, `revert_draft_decision`, `open_link` URL check. Add: `ListGroup` (PastDue, Due, NoFixedDeadline, Closed) classification from `DeadlineView` + closed flag, `Filter {All, Mine, Team}`, `open_badge_count`, `ScanStrip` state (Scanning{phase, i, n, m, total, elapsed, pct} | Finished{incomplete, summary, coverage_notes} | Idle), account display ("Not signed in" default; initials from display name). egui files become thin wrappers; all existing tests move and pass; new tests for the added logic. Gate: `cargo test -p openloops-desktop --features native-ui -- --test-threads=1`, clippy, fmt.

### T2 — Slint shell + Sources screen
Cargo/build.rs changes; `tokens.slint`, `widgets.slint`, `chrome.slint`, `sources.slint`, `app.slint` with a Review placeholder; `slint_ui.rs` adapter wiring the Sources screen fully (§5, §6 busy rules, settings command bar and status, provider segmented control, key field with Show key, model dropdown + Load buttons, Test selected model, Microsoft card with status lines, scan-scope card, footnote); startup-screen rule; status bar; nav rail with badge; title bar account. `eframe` removed; `setup_ui.rs`/`review_ui.rs` deleted. ADR-014. Gate: build + tests + clippy + fmt; app launches and Sources works end-to-end against the model (manual check by the orchestrator later).

### T3 — Review screen core
`review.slint`: command bar (§4.1), scan strip (§4.2), list pane (§4.3 groups, rows, selection, ↑/↓, empty state), reading pane top (pills, title, meta grid, uncertainty, decision actions §4.5, action status). Adapter maps `ReviewState` + decisions into row/group models; selection state; Stop scan; Show resolved checkbox; filters. Gate as T2.

### T4 — Reading pane evidence and reminders
Reminder draft panel (§4.6) with quick picks and live "Scheduled instant" line; reminder states (Created / Attempted with marker, links, reconcile buttons); evidence cards per anchor with Outlook deep links; possible-later-completion card with cross-thread badge; "no matching completion" note; full-conversation disclosure with avatar rows, sent tint, nested quoted-history block. Gate as T2.

### T5 — States, accessibility, docs, cleanup
§6 states (worker disconnected text, provider error in the strip, no-usable-expectations sentences, credential-store errors, storage bound); §7 accessibility (accessible-role/label on every control, focus ring, tab order, Enter/Esc); `--preview-review` fixture; docs listed above; remove dead code; full gate set: inference + desktop tests, clippy `-D warnings` (native-ui,ui-screenshot --all-targets), fmt, `check-model-boundary.ps1`, `test-model-boundary.ps1`, `check-public-repo.ps1 -Mode Staged`.

### Final
Codex adversarial review of the whole branch against the spec and reference; fixes; History gate; push; PR to `main`; merge; rebuild `target\debug\openloops-ui.exe`.

## Review checklist per task
- Spec fidelity: every control, label, colour token, size and copy string in the spec/reference sections for the task is present; no invented copy; disclosure strings verbatim from the old code.
- Behaviour parity: each button calls the same model operation as before; busy gating; decision transitions; reminder rules.
- Safety: no mail content or keys in logs/panics; key field is a Slint password input held in Rust; links limited to the allowed hosts.
- Quality: pedantic clippy clean, no `unwrap` on user data, tests for every new pure function, `.slint` files use tokens not literals.
