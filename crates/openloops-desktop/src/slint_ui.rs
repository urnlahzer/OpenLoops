//! Slint adapter for the toolkit-free application model.
use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::{
    app_model::{AccountDisplay, AppModel, Outcome, Service, Status},
    review_model::{ReviewState, open_badge_count, scan_strip},
    settings::{MAX_OPENROUTER_PARALLEL, MIN_OPENROUTER_PARALLEL, OllamaPlan, Provider},
};
use openloops_graph::live::{ConnectionConfig, check_connection};
use openloops_inference::{
    ollama::{OllamaCloud, available_models},
    openrouter::{OpenRouter, available_zdr_models},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use zeroize::Zeroizing;

slint::include_modules!();

const ENTRA_URL: &str = "https://entra.microsoft.com/";
const OLLAMA_KEYS_URL: &str = "https://ollama.com/settings/keys";
const OPENROUTER_KEYS_URL: &str = "https://openrouter.ai/settings/keys";

fn joined_status(status: &Status) -> String {
    status.lines.join("\n")
}

/// The label shown for the `ComboBox` row that means "nothing picked yet".
/// `display_model_values` prepends it as row 0 so the widget's own
/// `current-index` never has to represent "no selection" (Slint's
/// `ComboBoxBase.reset-current()` always clamps it to a valid row).
const LOAD_MODELS_PLACEHOLDER: &str = "Load models to choose";

/// Maps the model's raw selection (`-1` for none, matching `selected_index`)
/// to the display index of a `ComboBox` whose row 0 is the placeholder.
fn display_model_index(selected_index: i32) -> i32 {
    selected_index + 1
}

/// The inverse of `display_model_index`. Only the index-mapping unit test
/// below needs this direction; production code never receives a display
/// index back from Slint (row selection comes back as the row's text).
#[cfg(test)]
fn model_index_from_display(display_index: i32) -> i32 {
    display_index - 1
}

/// `model_values` with the placeholder prepended as row 0, for the `ComboBox`.
fn display_model_values(model: &AppModel) -> Vec<String> {
    let mut values = Vec::with_capacity(model.models.len().max(model.zdr_models.len()) + 1);
    values.push(LOAD_MODELS_PLACEHOLDER.to_string());
    values.extend(model_values(model));
    values
}

/// `selected_model_value` falling back to the placeholder when nothing is
/// selected, so the `ComboBox`'s displayed text matches its displayed row.
fn display_model_value(model: &AppModel) -> String {
    let selected = selected_model_value(model);
    if selected.is_empty() {
        LOAD_MODELS_PLACEHOLDER.to_string()
    } else {
        selected
    }
}

fn model_values(model: &AppModel) -> Vec<String> {
    match model.provider {
        Provider::OllamaCloud => model.models.clone(),
        Provider::OpenRouter => model
            .zdr_models
            .iter()
            .map(|choice| {
                if choice.label == choice.id {
                    choice.id.clone()
                } else {
                    format!("{} ({})", choice.label, choice.id)
                }
            })
            .collect(),
    }
}

fn selected_model_value(model: &AppModel) -> String {
    let selected = model.selected_model();
    if selected.is_empty() {
        return String::new();
    }
    match model.provider {
        Provider::OllamaCloud => selected.to_owned(),
        Provider::OpenRouter => model
            .zdr_models
            .iter()
            .find(|choice| choice.id == selected)
            .map_or_else(
                || selected.to_owned(),
                |choice| {
                    if choice.label == choice.id {
                        choice.id.clone()
                    } else {
                        format!("{} ({})", choice.label, choice.id)
                    }
                },
            ),
    }
}

fn selected_index(model: &AppModel) -> i32 {
    let selected = model.selected_model();
    match model.provider {
        Provider::OllamaCloud => model.models.iter().position(|item| item == selected),
        Provider::OpenRouter => model.zdr_models.iter().position(|item| item.id == selected),
    }
    .and_then(|index| i32::try_from(index).ok())
    .unwrap_or(-1)
}

fn commit_parallel(value: &str, current: u16) -> u16 {
    value.parse::<i64>().map_or(current, |value| {
        value
            .clamp(
                i64::from(MIN_OPENROUTER_PARALLEL),
                i64::from(MAX_OPENROUTER_PARALLEL),
            )
            .try_into()
            .expect("clamped parallel value fits u16")
    })
}

fn provider_connected(model: &AppModel) -> bool {
    !model.selected_model().is_empty() && model.model_status.succeeded
}

fn status_with_busy(status: &Status, busy: Option<&str>) -> String {
    let mut lines = status.lines.clone();
    if let Some(busy) = busy {
        lines.push(busy.to_owned());
    }
    lines.join("\n")
}

#[allow(clippy::too_many_lines)]
fn sync(model: &AppModel, window: &AppWindow) {
    // Graph does not expose the signed-in display name through the current
    // connection report yet, so the title bar shows only a connection
    // summary (owner feedback item 3): once the Microsoft connection has
    // succeeded, or once review messages have already loaded (mail is
    // plainly being scanned even if a rescan's own check has not yet
    // re-run), rather than a misleading "Not signed in".
    let account =
        AccountDisplay::connected(model.microsoft.succeeded || !model.review.messages.is_empty());
    let cards = model
        .review
        .analysis
        .as_ref()
        .map(|analysis| model.review.card_contexts(&analysis.items))
        .unwrap_or_default();
    let saved = model.review.decisions.records.len();
    let provider = model.provider.label();
    let selected = model.selected_model();
    let busy = model.pending.is_some();
    let busy_text = busy.then(|| {
        let elapsed = model.started.elapsed();
        format!("{} · {}s", model.progress, elapsed.as_secs())
    });
    window.set_busy(busy);
    let review_scanning = model.scan_progress.is_some();
    // Spec §6: the title-bar chip shows for any running job (§3 describes
    // its Review-scan text; other jobs show their label and elapsed time).
    // The window picks the Review text when `review-scan-chip-text` is set.
    window.set_scanning(busy);
    window.set_busy_chip_text(busy_text.as_deref().unwrap_or_default().into());
    let strip = scan_strip(model.scan_progress.as_deref(), &model.review);
    let (strip_model, scan_chip) = crate::slint_review::scan_strip_view(&strip, model, &cards);
    window.set_review_scan_chip_text(scan_chip.into());
    window.set_account_signed_in(account.signed_in);
    window.set_review_badge(i32::try_from(open_badge_count(&cards)).unwrap_or(i32::MAX));
    window.set_provider_name(match model.provider {
        Provider::OllamaCloud => "Ollama".into(),
        Provider::OpenRouter => "OpenRouter".into(),
    });
    window.set_provider_connected(provider_connected(model));
    window.set_status_left(format!("{saved} saved decisions · Windows Credential Manager").into());
    window.set_status_center(if selected.is_empty() {
        "".into()
    } else {
        format!("Model: {selected} ({provider})").into()
    });
    window.set_status_right("Mail and summaries stay in memory only".into());
    window.set_store_available(model.store.is_some());
    window.set_settings_status(joined_status(&model.settings_status).into());
    window.set_settings_succeeded(model.settings_status.succeeded);
    window.set_client_id(model.client_id.clone().into());
    window.set_groups(model.groups.clone().into());
    window.set_shared_mailboxes(model.shared.clone().into());
    window.set_own_inbox_accessible(
        model
            .microsoft
            .lines
            .first()
            .is_some_and(|line| line == "Personal inbox: access confirmed."),
    );
    let mut microsoft_lines = model
        .microsoft
        .lines
        .iter()
        .map(|line| StatusLine {
            text: line.clone().into(),
            succeeded: line.ends_with("access confirmed."),
        })
        .collect::<Vec<_>>();
    if model.pending_service == Service::Microsoft
        && let Some(busy_text) = &busy_text
    {
        microsoft_lines.push(StatusLine {
            text: busy_text.clone().into(),
            succeeded: false,
        });
    }
    window.set_microsoft_lines(ModelRc::new(VecModel::from(microsoft_lines)));
    window.set_provider_index(match model.provider {
        Provider::OllamaCloud => 0,
        Provider::OpenRouter => 1,
    });
    // X6 / threat-model S6: the Rust-owned key only ever reaches the Slint
    // text property while the Sources screen can show it, and is mirrored on
    // change rather than copied every `sync` tick (this runs up to 4x/sec
    // while a job is busy). `Forget`/`Reload` clear it immediately in their
    // own handlers rather than waiting for the next tick to notice.
    let desired_api_key = if window.get_active_screen() == 1 {
        model.active_key().to_string()
    } else {
        String::new()
    };
    if window.get_api_key().as_str() != desired_api_key {
        window.set_api_key(desired_api_key.into());
    }
    let values = display_model_values(model);
    window.set_models(ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    window.set_model_index(display_model_index(selected_index(model)));
    window.set_model_value(display_model_value(model).into());
    window.set_ollama_plan_index(match model.ollama_plan {
        OllamaPlan::Free => 0,
        OllamaPlan::Pro => 1,
        OllamaPlan::Max => 2,
    });
    window.set_ollama_plan_options(ModelRc::new(VecModel::from(
        [OllamaPlan::Free, OllamaPlan::Pro, OllamaPlan::Max]
            .map(|plan| SharedString::from(plan.label()))
            .to_vec(),
    )));
    if !window.get_parallel_field_focused() {
        window.set_openrouter_parallel(model.openrouter_parallel.to_string().into());
    }
    let model_busy = if model.pending_service == Service::Model {
        busy_text.as_deref()
    } else {
        None
    };
    window.set_model_status(status_with_busy(&model.model_status, model_busy).into());
    window.set_model_succeeded(model.model_status.succeeded);
    window.set_provider_disclosure(model.provider_disclosure().into());
    window.set_key_placeholder(match model.provider {
        Provider::OllamaCloud => "Paste your Ollama API key".into(),
        Provider::OpenRouter => "Paste your OpenRouter API key".into(),
    });
    window.set_load_models_label(match model.provider {
        Provider::OllamaCloud => "Load cloud models".into(),
        Provider::OpenRouter => "Load ZDR models".into(),
    });
    window.set_can_check_microsoft(!model.client_id.trim().is_empty());
    window.set_can_load_models(match model.provider {
        Provider::OllamaCloud => !model.key.is_empty(),
        Provider::OpenRouter => true,
    });
    window.set_can_test_model(!model.selected_model().is_empty() && !model.active_key().is_empty());
    crate::slint_review::sync_review(model, window, &cards, busy, review_scanning, strip_model);
}

fn finish_edit(model: &mut AppModel) {
    model.pending_save = true;
    let _ = model.persist_changes();
}

fn clear_microsoft_after_edit(model: &mut AppModel) {
    model.microsoft = Status::default();
    model.review = ReviewState::default();
    model.review_status = Status::default();
    finish_edit(model);
}

pub(crate) fn refresh(model: &Rc<RefCell<AppModel>>, weak: &slint::Weak<AppWindow>) {
    if let Some(window) = weak.upgrade() {
        sync(&model.borrow(), &window);
    }
}

pub(crate) fn start_timer(timer: &Rc<Timer>) {
    timer.restart();
}

/// Probes the model selected in the saved desktop settings.
///
/// # Errors
///
/// Returns the same user-facing settings and inference errors as the desktop
/// command-line probe.
pub fn probe_saved_model() -> Result<usize, String> {
    let store = crate::settings::production_store()
        .map_err(|_| "Saved settings unavailable".to_string())?
        .ok_or("Saved settings disabled".to_string())?;
    let settings = store
        .load()
        .map_err(|_| "Saved settings could not be loaded".to_string())?
        .ok_or("No saved settings".to_string())?;
    crate::review_model::probe(
        settings.provider,
        settings.active_key().to_string(),
        settings.active_model(),
    )
    .map_err(|error| error.to_string())
}

/// Builds and runs the native Slint window.
///
/// # Errors
///
/// Returns a platform error when Slint cannot create or run the native window.
#[allow(clippy::too_many_lines)]
pub fn run() -> Result<(), slint::PlatformError> {
    #[cfg(feature = "ui-screenshot")]
    let mut initial_model = AppModel::new();
    #[cfg(not(feature = "ui-screenshot"))]
    let initial_model = AppModel::new();
    #[cfg(feature = "ui-screenshot")]
    let preview_review = std::env::args().any(|arg| arg == "--preview-review");
    #[cfg(not(feature = "ui-screenshot"))]
    let preview_review = false;
    // `--preview-review` alone saves a PNG of the fixture and exits (see
    // `save_preview_snapshot`); `--stay` keeps the old behaviour of just
    // showing the window, for the interactive shot2.ps1 capture loop
    // documented in `docs/native-setup.md`. Only bound under `ui-screenshot`:
    // every use site is already `#[cfg]`-gated the same way, so an unused
    // binding in the plain `native-ui` build would otherwise be the only
    // reason to have it there at all.
    #[cfg(feature = "ui-screenshot")]
    let preview_stay = std::env::args().any(|arg| arg == "--stay");
    #[cfg(feature = "ui-screenshot")]
    if preview_review {
        initial_model.review = crate::review_model::layout_fixture();
        // N4: the old egui fixture also seeded a provider and selected model
        // so the status bar's centre text ("Model: ... (...)") was not blank
        // in the screenshot; match that here.
        initial_model.provider = Provider::OllamaCloud;
        initial_model.selected = "Synthetic layout check".into();
    }
    // T7 (brief §5): seeds the Sources screen with OpenRouter selected, a
    // parallel-requests value and a long zero-data-retention model label, so
    // the visual loop's `-Out sources.png` shot exercises the same overlap
    // case the owner's OpenRouter screenshot showed -- independent of
    // `--preview-review`, since a fresh run with no saved settings already
    // lands on Sources (spec §3's "Startup screen" rule).
    #[cfg(feature = "ui-screenshot")]
    if std::env::var("OPENLOOPS_PREVIEW_PROVIDER").as_deref() == Ok("openrouter") {
        initial_model.provider = Provider::OpenRouter;
        initial_model.openrouter_parallel = 32;
        let model_id = "anthropic/claude-sonnet-4.6";
        initial_model.zdr_models = vec![openloops_inference::openrouter::ModelChoice {
            id: model_id.into(),
            label: "Anthropic: Claude Sonnet 4.6 (zero-data-retention endpoint, extended thinking)"
                .into(),
        }];
        initial_model.openrouter_selected = model_id.into();
    }
    // Owner feedback item 1: `OPENLOOPS_PREVIEW_BUSY=1` seeds a fake pending
    // job so the title-bar busy chip renders in a screenshot without a real
    // long-running operation. The channel's `Sender` is leaked (never
    // dropped, never sent to) so `poll`'s `TryRecvError::Disconnected`
    // branch never fires and the chip stays up for the whole capture. Only
    // bound under `ui-screenshot`; a normal run never sets this variable.
    #[cfg(feature = "ui-screenshot")]
    if std::env::var("OPENLOOPS_PREVIEW_BUSY").as_deref() == Ok("1") {
        let (sender, receiver) = std::sync::mpsc::channel::<Outcome>();
        std::mem::forget(sender);
        initial_model.pending = Some(receiver);
        initial_model.pending_service = Service::Microsoft;
        initial_model.progress = "Complete Microsoft sign-in; then scanning recent messages";
        initial_model.started = std::time::Instant::now()
            .checked_sub(Duration::from_secs(82))
            .unwrap_or_else(std::time::Instant::now);
    }
    let model = Rc::new(RefCell::new(initial_model));
    let window = AppWindow::new()?;
    let initial_screen = {
        let model = model.borrow();
        if preview_review {
            0
        } else {
            i32::from(!(model.settings_existed && model.ready_for_review()))
        }
    };
    window.set_active_screen(initial_screen);
    window.set_show_key(false);
    // Preview-fixture only: start the reading pane's "Full scanned
    // conversation" disclosure expanded so a `--preview-review` screenshot
    // shows the message rows, the sent tint, and the nested quoted-history
    // block without a click. `review.slint` binds this once at startup; a
    // real user's own toggle click still drops the binding as usual.
    #[cfg(feature = "ui-screenshot")]
    if preview_review {
        window.set_conversation_expanded_default(true);
    }
    sync(&model.borrow(), &window);

    let timer = Rc::new(Timer::default());
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer_weak = Rc::downgrade(&timer);
        timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let mut model_ref = model.borrow_mut();
            model_ref.poll(|| {});
            let _ = model_ref.persist_changes();
            drop(model_ref);
            refresh(&model, &weak);
            if model.borrow().pending.is_none()
                && let Some(timer) = timer_weak.upgrade()
            {
                timer.stop();
            }
        });
        timer.stop();
    }

    macro_rules! simple_action {
        ($setter:ident, $body:expr) => {{
            let model = Rc::clone(&model);
            let weak = window.as_weak();
            window.$setter(move || {
                $body(&mut model.borrow_mut());
                refresh(&model, &weak);
            });
        }};
    }
    simple_action!(on_save_settings, |model: &mut AppModel| model
        .save_settings());
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_reload_settings(move || {
            let (succeeded, active) = {
                let mut model = model.borrow_mut();
                let existed = model.reload_settings();
                (
                    model.settings_status.succeeded,
                    existed && model.ready_for_review(),
                )
            };
            if let Some(window) = weak.upgrade() {
                window.set_show_key(false);
                // X6: never let a stale key sit in the Slint text property
                // across a reload; `sync` below repopulates it only if the
                // Sources screen is (still) active.
                window.set_api_key("".into());
                if succeeded {
                    window.set_active_screen(i32::from(!active));
                }
                sync(&model.borrow(), &window);
            }
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_forget_settings(move || {
            model.borrow_mut().forget_settings();
            if let Some(window) = weak.upgrade() {
                window.set_show_key(false);
                // X6: the key is gone from the model; clear it from the
                // mirrored property immediately rather than waiting on
                // `sync`'s diff (which would happen to agree here anyway,
                // since the forgotten key is now empty).
                window.set_api_key("".into());
                sync(&model.borrow(), &window);
            }
        });
    }
    window.on_navigate({
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        move |index| {
            if let Some(window) = weak.upgrade() {
                window.set_active_screen(index);
                // X6: clear the mirrored key the moment Sources stops being
                // the active screen, rather than leaving it in the Slint
                // property until the next unrelated `sync` tick notices; a
                // full `sync` (rather than only the key) also repopulates it
                // promptly when navigating back to Sources.
                if index == 1 {
                    sync(&model.borrow(), &window);
                } else {
                    window.set_api_key("".into());
                }
            }
        }
    });

    macro_rules! text_edit {
        ($setter:ident, $field:ident, $model_ref:ident, $clear:block) => {{
            let model = Rc::clone(&model);
            let weak = window.as_weak();
            window.$setter(move |value| {
                let mut $model_ref = model.borrow_mut();
                $model_ref.$field = value.to_string();
                $clear
                drop($model_ref);
                refresh(&model, &weak);
            });
        }};
    }
    text_edit!(on_client_id_edited, client_id, model_ref, {
        model_ref.client_id = model_ref.client_id.chars().take(128).collect();
        clear_microsoft_after_edit(&mut model_ref);
    });
    text_edit!(on_groups_edited, groups, model_ref, {
        clear_microsoft_after_edit(&mut model_ref);
    });
    text_edit!(on_shared_edited, shared, model_ref, {
        clear_microsoft_after_edit(&mut model_ref);
    });

    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_check_microsoft(move || {
            let config = {
                let model = model.borrow();
                ConnectionConfig::new(model.client_id.trim(), Some(&model.shared))
                    .and_then(|config| config.with_groups(Some(&model.groups)))
            };
            match config {
                Ok(config) => {
                    let mut model_ref = model.borrow_mut();
                    model_ref.microsoft = Status::default();
                    model_ref.start(
                        Service::Microsoft,
                        "Complete Microsoft sign-in in your browser",
                        move || Outcome::Microsoft(check_connection(&config)),
                        || {},
                    );
                    start_timer(&timer);
                }
                Err(error) => {
                    model.borrow_mut().microsoft = Status {
                        lines: vec![error.to_string()],
                        succeeded: false,
                    };
                }
            }
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_provider_selected(move |index| {
            let mut model_ref = model.borrow_mut();
            let provider = if index == 1 {
                Provider::OpenRouter
            } else {
                Provider::OllamaCloud
            };
            if provider == model_ref.provider {
                return;
            }
            model_ref.provider = provider;
            model_ref.model_status = Status::default();
            finish_edit(&mut model_ref);
            drop(model_ref);
            // The OpenRouter parallel-requests field only exists while
            // `provider-index == 1` (see sources.slint); switching provider
            // tears it down without ever firing its `changed has-focus`
            // handler, which would otherwise clear this latch. Left set, it
            // would permanently stop `sync` from ever refreshing the field
            // again (see the `!window.get_parallel_field_focused()` guard
            // below), even after the field is recreated.
            if let Some(window) = weak.upgrade() {
                window.set_parallel_field_focused(false);
                // N5: each provider keeps its own key; revealing one
                // provider's key must not leave the other provider's key
                // shown in the clear the moment the switch lands.
                window.set_show_key(false);
            }
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_key_edited(move |value| {
            let mut model_ref = model.borrow_mut();
            // X5: the egui build's key fields enforced `char_limit(4096)`
            // (`setup_ui.rs` at 69235c1, lines 257-259 and 365-367); truncate
            // here the same way `client-id-edited` already truncates to 128.
            let value: Zeroizing<String> = Zeroizing::new(value.chars().take(4096).collect());
            match model_ref.provider {
                Provider::OllamaCloud => {
                    model_ref.key = value;
                    model_ref.models.clear();
                    model_ref.selected.clear();
                }
                Provider::OpenRouter => {
                    model_ref.openrouter_key = value;
                }
            }
            model_ref.trim_keys();
            model_ref.model_status = Status::default();
            finish_edit(&mut model_ref);
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    window.on_open_entra(|| {
        let _ = opener::open(ENTRA_URL);
    });
    {
        let model = Rc::clone(&model);
        window.on_open_key_page(move || {
            let url = match model.borrow().provider {
                Provider::OllamaCloud => OLLAMA_KEYS_URL,
                Provider::OpenRouter => OPENROUTER_KEYS_URL,
            };
            let _ = opener::open(url);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_load_models(move || {
            let mut model_ref = model.borrow_mut();
            model_ref.model_status = Status::default();
            match model_ref.provider {
                Provider::OllamaCloud => {
                    let key = model_ref.key.clone();
                    model_ref.start(
                        Service::Model,
                        "Loading Ollama Cloud models",
                        move || Outcome::Models(available_models(&key)),
                        || {},
                    );
                }
                Provider::OpenRouter => {
                    model_ref.start(
                        Service::Model,
                        "Loading OpenRouter zero-data-retention models",
                        || Outcome::ZdrModels(available_zdr_models()),
                        || {},
                    );
                }
            }
            drop(model_ref);
            start_timer(&timer);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_model_selected(move |value| {
            if value.as_str() == LOAD_MODELS_PLACEHOLDER {
                return;
            }
            let mut model_ref = model.borrow_mut();
            let next = match model_ref.provider {
                Provider::OllamaCloud => Some(value.to_string()),
                Provider::OpenRouter => model_ref.zdr_models.iter().find_map(|choice| {
                    (value == choice.id || value == format!("{} ({})", choice.label, choice.id))
                        .then(|| choice.id.clone())
                }),
            };
            let Some(next) = next else { return };
            if next == model_ref.selected_model() {
                return;
            }
            match model_ref.provider {
                Provider::OllamaCloud => model_ref.selected = next,
                Provider::OpenRouter => model_ref.openrouter_selected = next,
            }
            model_ref.model_status = Status::default();
            finish_edit(&mut model_ref);
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_plan_selected(move |index| {
            let mut model_ref = model.borrow_mut();
            let plan = match index {
                1 => OllamaPlan::Pro,
                2 => OllamaPlan::Max,
                _ => OllamaPlan::Free,
            };
            if plan == model_ref.ollama_plan {
                return;
            }
            model_ref.ollama_plan = plan;
            finish_edit(&mut model_ref);
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_parallel_committed(move |value| {
            {
                let mut model_ref = model.borrow_mut();
                let value = commit_parallel(&value, model_ref.openrouter_parallel);
                if value != model_ref.openrouter_parallel {
                    model_ref.openrouter_parallel = value;
                    finish_edit(&mut model_ref);
                }
            }
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_test_model(move || {
            let mut model_ref = model.borrow_mut();
            let key = model_ref.active_key().clone();
            let selected = model_ref.selected_model().to_owned();
            let provider = model_ref.provider;
            model_ref.model_status = Status::default();
            model_ref.start(
                Service::Model,
                "Testing the selected cloud model (up to 150 seconds per request)",
                move || {
                    Outcome::Generation(match provider {
                        Provider::OllamaCloud => OllamaCloud::connect(key.to_string(), &selected)
                            .and_then(|provider| provider.check_generation()),
                        Provider::OpenRouter => OpenRouter::connect(key.to_string(), &selected)
                            .and_then(|provider| provider.check_generation()),
                    })
                },
                || {},
            );
            drop(model_ref);
            start_timer(&timer);
            refresh(&model, &weak);
        });
    }

    crate::slint_review::register_callbacks(&window, &model, &timer);

    #[cfg(feature = "ui-screenshot")]
    if preview_review && !preview_stay {
        // The window needs at least one real paint before a snapshot means
        // anything; requesting a redraw now and running briefly through the
        // normal event loop (this is exactly what `window.run()` below
        // does) before snapshotting and quitting from a one-shot timer is
        // simpler and more reliable than trying to force a single render
        // pass by hand. On a session with no real interactive desktop
        // attached, `take_snapshot` still returns `Ok` here but the buffer
        // it reads back can come back uniformly blank rather than erroring
        // -- the same underlying limitation as ordinary screen capture
        // returning black in that situation. This has no graceful detection
        // short of inspecting pixel content, so it is not handled specially;
        // an interactive desktop session does not hit it.
        window.window().request_redraw();
        let weak = window.as_weak();
        Timer::single_shot(Duration::from_millis(800), move || {
            if let Some(window) = weak.upgrade() {
                save_preview_snapshot(&window);
            }
            let _ = slint::quit_event_loop();
        });
    }

    window.run()
}

/// Saves `--preview-review`'s fixture window to
/// `<temp dir>/openloops-review-preview.png` via `slint::Window::take_snapshot`,
/// which the femtovg renderer this binary already builds with (`native-ui`'s
/// `renderer-femtovg` feature) implements directly -- no separate software
/// renderer needed. Reports and skips the PNG rather than failing the
/// process when the renderer cannot produce a snapshot; the fixture window
/// is still shown up to that point either way.
#[cfg(feature = "ui-screenshot")]
fn save_preview_snapshot(window: &AppWindow) {
    match window.window().take_snapshot() {
        Ok(buffer) => {
            let path = std::env::temp_dir().join("openloops-review-preview.png");
            match image::RgbaImage::from_raw(
                buffer.width(),
                buffer.height(),
                buffer.as_bytes().to_vec(),
            ) {
                Some(rgba) => match rgba.save(&path) {
                    Ok(()) => println!("Preview snapshot saved to {}", path.display()),
                    Err(error) => {
                        eprintln!(
                            "Preview snapshot: could not save {}: {error}",
                            path.display()
                        );
                    }
                },
                None => eprintln!(
                    "Preview snapshot: the captured buffer did not match its own dimensions"
                ),
            }
        }
        Err(error) => {
            eprintln!(
                "Preview snapshot unavailable ({error}); the fixture was shown but no PNG was saved."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openloops_inference::openrouter::ModelChoice;

    fn model() -> AppModel {
        AppModel::with_store(Ok(None))
    }

    #[test]
    fn parallel_commit_clamps_only_valid_integer_input() {
        assert_eq!(commit_parallel("", 32), 32);
        assert_eq!(commit_parallel("not-a-number", 32), 32);
        assert_eq!(commit_parallel("-5", 32), MIN_OPENROUTER_PARALLEL);
        assert_eq!(commit_parallel("0", 32), MIN_OPENROUTER_PARALLEL);
        assert_eq!(commit_parallel("48", 32), 48);
        assert_eq!(commit_parallel("999", 32), MAX_OPENROUTER_PARALLEL);
    }

    #[test]
    fn openrouter_model_value_matches_its_formatted_list_entry() {
        let mut model = model();
        model.provider = Provider::OpenRouter;
        model.zdr_models = vec![
            ModelChoice {
                id: "vendor/model-a".into(),
                label: "Friendly A".into(),
            },
            ModelChoice {
                id: "vendor/model-b".into(),
                label: "vendor/model-b".into(),
            },
        ];
        model.openrouter_selected = "vendor/model-a".into();
        assert_eq!(selected_index(&model), 0);
        assert_eq!(selected_model_value(&model), "Friendly A (vendor/model-a)");
        assert_eq!(model_values(&model)[0], selected_model_value(&model));
    }

    #[test]
    fn no_selected_model_has_an_explicit_negative_index_and_empty_value() {
        let mut model = model();
        model.models = vec!["available-model".into()];
        assert_eq!(selected_index(&model), -1);
        assert!(selected_model_value(&model).is_empty());
    }

    #[test]
    fn provider_dot_requires_a_success_for_a_selected_model() {
        let mut model = model();
        model.model_status.succeeded = true;
        assert!(!provider_connected(&model));
        model.selected = "selected-model".into();
        assert!(provider_connected(&model));
        model.model_status.succeeded = false;
        assert!(!provider_connected(&model));
    }

    #[test]
    fn display_model_index_shifts_by_one_for_the_placeholder_row() {
        assert_eq!(display_model_index(-1), 0);
        assert_eq!(display_model_index(0), 1);
        assert_eq!(display_model_index(4), 5);
    }

    #[test]
    fn model_index_from_display_is_the_inverse_of_display_model_index() {
        for raw in -1..10 {
            assert_eq!(model_index_from_display(display_model_index(raw)), raw);
        }
    }

    #[test]
    fn display_model_values_prepends_the_placeholder_row() {
        let mut model = model();
        model.models = vec!["a".into(), "b".into()];
        let values = display_model_values(&model);
        assert_eq!(values[0], LOAD_MODELS_PLACEHOLDER);
        assert_eq!(values[1..], model_values(&model)[..]);
    }

    #[test]
    fn display_model_value_falls_back_to_the_placeholder_when_nothing_is_selected() {
        let model = model();
        assert!(selected_model_value(&model).is_empty());
        assert_eq!(display_model_value(&model), LOAD_MODELS_PLACEHOLDER);
    }

    #[test]
    fn display_model_value_matches_the_real_selection_when_one_exists() {
        let mut model = model();
        model.models = vec!["available-model".into()];
        model.selected = "available-model".into();
        assert_eq!(display_model_value(&model), "available-model");
    }
}
