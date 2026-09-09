//! Native setup interface with explicit current-user Windows credential storage.
use std::sync::atomic::Ordering;

use crate::app_model::{self, AppModel, Outcome, Service, Status};
use crate::review_model::ReviewState;
use crate::settings::{
    MAX_OPENROUTER_PARALLEL, MIN_OPENROUTER_PARALLEL, OllamaPlan, Provider, SettingsError,
    SettingsStore, production_store,
};
use eframe::egui::{self, Color32, RichText};
use openloops_graph::live::{ConnectionConfig, check_connection};
use openloops_inference::ollama::{OllamaCloud, available_models};
use openloops_inference::openrouter::{OpenRouter, available_zdr_models};

const GREEN: Color32 = Color32::from_rgb(29, 87, 67);
const INK: Color32 = Color32::from_rgb(30, 43, 40);
const MUTED: Color32 = Color32::from_rgb(94, 111, 104);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    Review,
}

pub struct SetupApp {
    model: AppModel,
    reveal_key: bool,
    active_tab: Tab,
}

impl SetupApp {
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_store(ctx, production_store())
    }

    fn with_store(
        ctx: &egui::Context,
        store: Result<Option<Box<dyn SettingsStore>>, SettingsError>,
    ) -> Self {
        ctx.set_visuals(egui::Visuals::light());
        ctx.style_mut_of(egui::Theme::Light, |style| {
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.visuals.override_text_color = Some(INK);
            style.visuals.selection.bg_fill = GREEN;
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        });
        let model = AppModel::with_store(store);
        // Mirrors today's `SetupApp::with_store`: the tab starts at the
        // `ui-screenshot` default, and is only overwritten by the saved-
        // settings-derived choice when `AppModel::with_store`'s internal
        // reload actually ran its "settings loaded" branch (`settings_status
        // .succeeded`) -- a missing store or a failed load leaves the
        // initial default alone, same as before.
        let mut active_tab = if cfg!(feature = "ui-screenshot") {
            Tab::Review
        } else {
            Tab::Connections
        };
        if model.settings_status.succeeded {
            active_tab = if model.settings_existed && model.ready_for_review() {
                Tab::Review
            } else {
                Tab::Connections
            };
        }
        #[cfg_attr(not(feature = "ui-screenshot"), allow(unused_mut))]
        let mut app = Self {
            model,
            reveal_key: false,
            active_tab,
        };
        #[cfg(feature = "ui-screenshot")]
        if std::env::args().any(|arg| arg == "--preview-review") {
            app.model.review = crate::review_model::layout_fixture();
            app.model.selected = "Synthetic layout check".into();
            app.model.provider = Provider::OllamaCloud;
            app.active_tab = Tab::Review;
        }
        app
    }

    fn settings_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(
            self.model.pending.is_none() && self.model.store.is_some(),
            |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Save settings").clicked() {
                        self.model.save_settings();
                    }
                    if ui.button("Reload saved settings").clicked() {
                        let existed = self.model.reload_settings();
                        self.reveal_key = false;
                        if self.model.settings_status.succeeded {
                            self.active_tab = if existed && self.model.ready_for_review() {
                                Tab::Review
                            } else {
                                Tab::Connections
                            };
                        }
                    }
                    if ui.button("Forget saved settings").clicked() {
                        self.model.forget_settings();
                        self.reveal_key = false;
                    }
                });
            },
        );
        status(ui, &self.model.settings_status);
    }

    fn microsoft_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("1  Connect Microsoft");
        ui.label("Your own inbox is included automatically.");
        ui.add_space(4.0);
        ui.label(RichText::new("Application (client) ID").strong());
        let mut changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.model.client_id)
                    .char_limit(128)
                    .desired_width(f32::INFINITY)
                    .hint_text("From your Entra app's Overview"),
            )
            .changed();
        ui.hyperlink_to("Open Microsoft Entra", "https://entra.microsoft.com/");
        ui.label(RichText::new("Outlook Groups").strong());
        ui.label(
            RichText::new(
                "Addresses shown under Groups in Outlook. One per line or separated by commas.",
            )
            .small()
            .color(MUTED),
        );
        changed |= ui
            .add(
                egui::TextEdit::multiline(&mut self.model.groups)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Enter group email addresses"),
            )
            .changed();
        ui.collapsing("Shared mailboxes (optional)", |ui| {
            ui.label("For mailboxes under Shared with me or Open another mailbox.");
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut self.model.shared)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Enter shared mailbox addresses"),
                )
                .changed();
        });
        if changed {
            self.model.microsoft = Status::default();
            self.model.pending_save = true;
            self.model.review = ReviewState::default();
            self.model.review_status = Status::default();
        }
        ui.add_space(6.0);
        if ui
            .add_enabled(
                !self.model.client_id.trim().is_empty(),
                egui::Button::new(RichText::new("Sign in & check inboxes").color(Color32::WHITE))
                    .fill(GREEN),
            )
            .clicked()
        {
            let config =
                ConnectionConfig::new(self.model.client_id.trim(), Some(&self.model.shared))
                    .and_then(|config| config.with_groups(Some(&self.model.groups)));
            match config {
                Ok(config) => {
                    self.model.microsoft = Status::default();
                    let ctx = ui.ctx().clone();
                    self.model.start(
                        Service::Microsoft,
                        "Complete Microsoft sign-in in your browser",
                        move || Outcome::Microsoft(check_connection(&config)),
                        move || ctx.request_repaint(),
                    );
                }
                Err(error) => {
                    self.model.microsoft = Status {
                        lines: vec![error.to_string()],
                        succeeded: false,
                    };
                }
            }
        }
        status(ui, &self.model.microsoft);
    }

    fn model_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("2  Choose your AI model");
        ui.label(RichText::new("Provider").strong());
        let before = self.model.provider;
        ui.horizontal(|ui| {
            for provider in [Provider::OllamaCloud, Provider::OpenRouter] {
                ui.selectable_value(&mut self.model.provider, provider, provider.label());
            }
        });
        if before != self.model.provider {
            self.model.model_status = Status::default();
            self.model.pending_save = true;
        }
        match self.model.provider {
            Provider::OllamaCloud => self.ollama_inputs(ui),
            Provider::OpenRouter => self.openrouter_inputs(ui),
        }
        if ui
            .add_enabled(
                !self.model.selected_model().is_empty() && !self.model.active_key().is_empty(),
                egui::Button::new(RichText::new("Test selected model").color(Color32::WHITE))
                    .fill(GREEN),
            )
            .clicked()
        {
            let key = self.model.active_key().clone();
            let model = self.model.selected_model().to_owned();
            let provider = self.model.provider;
            self.model.model_status = Status::default();
            let ctx = ui.ctx().clone();
            self.model.start(
                Service::Model,
                "Testing the selected cloud model (up to 150 seconds per request)",
                move || {
                    Outcome::Generation(match provider {
                        Provider::OllamaCloud => OllamaCloud::connect(key.to_string(), &model)
                            .and_then(|provider| provider.check_generation()),
                        Provider::OpenRouter => OpenRouter::connect(key.to_string(), &model)
                            .and_then(|provider| provider.check_generation()),
                    })
                },
                move || ctx.request_repaint(),
            );
        }
        status(ui, &self.model.model_status);
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "This connection test sends no email. Scanning inboxes sends recent message text from your configured inboxes to {}.",
                self.model.provider_disclosure()
            ))
            .small()
            .color(MUTED),
        );
    }

    fn ollama_inputs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("API key").strong());
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.model.key)
                    .char_limit(4096)
                    .password(!self.reveal_key)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste your Ollama API key"),
            )
            .changed()
        {
            self.model.trim_keys();
            self.model.models.clear();
            self.model.selected.clear();
            self.model.model_status = Status::default();
            self.model.pending_save = true;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reveal_key, "Show key");
            ui.hyperlink_to("Create an API key", "https://ollama.com/settings/keys");
        });
        self.ollama_plan_selector(ui);
        if ui
            .add_enabled(
                !self.model.key.is_empty(),
                egui::Button::new("Load cloud models"),
            )
            .clicked()
        {
            let key = self.model.key.clone();
            self.model.model_status = Status::default();
            let ctx = ui.ctx().clone();
            self.model.start(
                Service::Model,
                "Loading Ollama Cloud models",
                move || Outcome::Models(available_models(&key)),
                move || ctx.request_repaint(),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Model").strong());
        let before = self.model.selected.clone();
        egui::ComboBox::from_id_salt("ollama-model")
            .selected_text(if self.model.selected.is_empty() {
                "Load models to choose"
            } else {
                &self.model.selected
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for model in &self.model.models {
                    ui.selectable_value(&mut self.model.selected, model.clone(), model);
                }
            });
        if before != self.model.selected {
            self.model.model_status = Status::default();
            self.model.pending_save = true;
        }
    }

    /// Ollama Cloud allots concurrent request slots per plan, so a scan can
    /// only run as many requests at once as the account's plan allows;
    /// beyond that Ollama queues and then rejects them. Defaults to Free
    /// because it is the only plan safe to assume.
    fn ollama_plan_selector(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Ollama plan").strong());
        let before = self.model.ollama_plan;
        ui.horizontal(|ui| {
            for plan in [OllamaPlan::Free, OllamaPlan::Pro, OllamaPlan::Max] {
                ui.selectable_value(&mut self.model.ollama_plan, plan, plan.label());
            }
        });
        ui.label(
            RichText::new("Free 1 \u{b7} Pro 3 \u{b7} Max/Team 10 concurrent requests")
                .small()
                .color(MUTED),
        );
        if before != self.model.ollama_plan {
            self.model.pending_save = true;
        }
    }

    /// `OpenRouter` publishes no concurrency cap for a paid key, so this
    /// ceiling is the user's own. A scan backs off on its own when an
    /// upstream provider answers 429.
    fn openrouter_parallel_input(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Parallel requests (max)").strong());
        let before = self.model.openrouter_parallel;
        ui.add(
            egui::DragValue::new(&mut self.model.openrouter_parallel)
                .speed(1.0)
                .range(MIN_OPENROUTER_PARALLEL..=MAX_OPENROUTER_PARALLEL),
        );
        ui.label(
            RichText::new(
                "How many conversations a scan analyzes at once. OpenRouter publishes no concurrency cap for a paid key; a rate-limited conversation is reported as failed and never resent.",
            )
            .small()
            .color(MUTED),
        );
        if before != self.model.openrouter_parallel {
            self.model.pending_save = true;
        }
    }

    fn openrouter_inputs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("API key").strong());
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.model.openrouter_key)
                    .char_limit(4096)
                    .password(!self.reveal_key)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste your OpenRouter API key"),
            )
            .changed()
        {
            self.model.trim_keys();
            self.model.model_status = Status::default();
            self.model.pending_save = true;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reveal_key, "Show key");
            ui.hyperlink_to("Create an API key", "https://openrouter.ai/settings/keys");
        });
        self.openrouter_parallel_input(ui);
        // The listing is public, so it loads without a key and sends none.
        if ui.button("Load ZDR models").clicked() {
            self.model.model_status = Status::default();
            let ctx = ui.ctx().clone();
            self.model.start(
                Service::Model,
                "Loading OpenRouter zero-data-retention models",
                || Outcome::ZdrModels(available_zdr_models()),
                move || ctx.request_repaint(),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Model").strong());
        let before = self.model.openrouter_selected.clone();
        egui::ComboBox::from_id_salt("openrouter-model")
            .selected_text(if self.model.openrouter_selected.is_empty() {
                "Load models to choose"
            } else {
                &self.model.openrouter_selected
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for model in &self.model.zdr_models {
                    ui.selectable_value(
                        &mut self.model.openrouter_selected,
                        model.id.clone(),
                        if model.label == model.id {
                            model.id.clone()
                        } else {
                            format!("{} ({})", model.label, model.id)
                        },
                    );
                }
            });
        if before != self.model.openrouter_selected {
            self.model.model_status = Status::default();
            self.model.pending_save = true;
        }
        ui.label(
            RichText::new(
                "Only models with a zero-data-retention endpoint are listed, and every request asks OpenRouter to route to those endpoints only. OpenRouter's own retention policy still applies.",
            )
            .small()
            .color(MUTED),
        );
    }

    fn review_card(&mut self, ui: &mut egui::Ui) {
        if self.model.review.analysis.is_none() {
            ui.label("Scan the last 30 days of Inbox and Sent Items, plus configured Groups and shared mailboxes. Each conversation is checked for expectations and later replies.");
            ui.label(format!(
                "Message text and participants go to your chosen model at {}. Attachments are not sent.",
                self.model.provider_disclosure()
            ));
        }
        let disclosure = self.model.provider_disclosure();
        ui.collapsing("Scan scope and data sent to the model provider", |ui| {
            ui.label("The last 30 days of Inbox and Sent Items: up to 100 messages per folder. Groups: up to 20 recent threads and 40 posts per thread, including earlier thread context. At most 10 configured sources. Capped sources and large conversations are reported as incomplete.");
            ui.label(format!("Subjects, current text, quoted history and participants are sent to the selected provider ({disclosure}). Attachments are not sent. Scans run when you click Scan; this preview is not an unattended background service."));
        });
        self.review_scan_buttons(ui);
        ui.add_space(8.0);
        ui.label(format!(
            "Selected model: {} ({})",
            if self.model.selected_model().is_empty() {
                "Choose one in Connections"
            } else {
                self.model.selected_model()
            },
            self.model.provider.label()
        ));
        status(ui, &self.model.review_status);
        self.model.review.show(ui);
        self.dispatch_pending_reminder(ui);
    }

    /// The "Scan inboxes" / "Rescan loaded mail" / "Clear results and mail"
    /// row at the top of the review card.
    fn review_scan_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.model.client_id.trim().is_empty()
                        && !self.model.active_key().is_empty()
                        && !self.model.selected_model().is_empty(),
                    egui::Button::new(RichText::new("Scan inboxes").color(Color32::WHITE))
                        .fill(GREEN),
                )
                .clicked()
            {
                match ConnectionConfig::new(self.model.client_id.trim(), Some(&self.model.shared))
                    .and_then(|config| config.with_groups(Some(&self.model.groups)))
                {
                    Ok(config) => {
                        self.model.review = ReviewState::default();
                        self.model.review_status = Status::default();
                        let ctx = ui.ctx().clone();
                        self.model.start(
                            Service::Review,
                            "Complete Microsoft sign-in; then scanning recent messages",
                            move || {
                                Outcome::Mail(openloops_graph::live::review::load_recent(&config))
                            },
                            move || ctx.request_repaint(),
                        );
                    }
                    Err(error) => {
                        self.model.review_status = Status {
                            lines: vec![error.to_string()],
                            succeeded: false,
                        };
                    }
                }
            }
            if ui
                .add_enabled(
                    !self.model.review.messages.is_empty()
                        && !self.model.active_key().is_empty()
                        && !self.model.selected_model().is_empty(),
                    egui::Button::new("Rescan loaded mail"),
                )
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.model.start_scan(move || ctx.request_repaint());
            }
            if ui.button("Clear results and mail").clicked() {
                self.model.review = ReviewState::default();
                self.model.review_status = Status::default();
            }
        });
    }

    /// Starts creating the Microsoft To Do reminder the review card queued
    /// (via a "Set To Do reminder…" click), if any.
    fn dispatch_pending_reminder(&mut self, ui: &mut egui::Ui) {
        if let Some((key, request)) = self.model.review.pending_reminder.take() {
            match ConnectionConfig::new(self.model.client_id.trim(), None) {
                Ok(config) => {
                    let ctx = ui.ctx().clone();
                    self.model.start(
                        Service::Review,
                        "Sign in to create the reviewed Microsoft To Do reminder",
                        move || {
                            Outcome::Reminder(
                                key,
                                openloops_graph::live::reminders::create(&config, &request),
                            )
                        },
                        move || ctx.request_repaint(),
                    );
                }
                Err(error) => {
                    let mut record = self.model.review.decisions.get(&key);
                    record.reminder = crate::loop_state::Reminder::None;
                    record.updated = crate::loop_state::now();
                    let _ = self.model.review.decisions.update(record);
                    self.model.review.action_status = error.to_string();
                }
            }
        }
    }
}

fn status(ui: &mut egui::Ui, status: &Status) {
    if status.lines.is_empty() {
        return;
    }
    ui.add_space(6.0);
    let color = if status.succeeded {
        GREEN
    } else {
        Color32::from_rgb(132, 72, 31)
    };
    for line in &status.lines {
        ui.label(RichText::new(line).color(color));
    }
}

impl eframe::App for SetupApp {
    #[cfg(feature = "ui-screenshot")]
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        // Render normal enabled styling, but accept no mouse, clipboard, or key input.
        input
            .events
            .retain(|event| matches!(event, egui::Event::Screenshot { .. }));
        input.dropped_files.clear();
        input.hovered_files.clear();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "ui-screenshot")]
        capture_empty_window(ui.ctx());
        let ctx = ui.ctx().clone();
        self.model.poll(move || ctx.request_repaint());
        let busy = self.model.pending.is_some();
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("OPENLOOPS").size(15.0).strong().color(GREEN));
                ui.heading(RichText::new("Who is waiting on you?").size(26.0));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.active_tab, Tab::Review, "Review inboxes");
                    ui.selectable_value(&mut self.active_tab, Tab::Connections, "Connections");
                });
                ui.add_space(16.0);
                ui.add_enabled_ui(!busy, |ui| {
                    if self.active_tab == Tab::Review { self.review_card(ui); } else {
                    ui.columns(2, |columns| {
                        egui::Frame::group(columns[0].style()).inner_margin(20.0).show(&mut columns[0], |ui| self.microsoft_card(ui));
                        egui::Frame::group(columns[1].style()).inner_margin(20.0).show(&mut columns[1], |ui| self.model_card(ui));
                    });
                    }
                });
                ui.add_space(18.0);
                if busy {
                    let elapsed = self.model.started.elapsed();
                    ui.horizontal(|ui| {
                        ui.label(app_model::busy_indicator(elapsed.as_millis()));
                        ui.label(format!("{} · {}s", self.model.progress, elapsed.as_secs()));
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    if let Some(progress) = &self.model.scan_progress {
                        ui.label(format!("{} / {} messages processed", progress.processed.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)));
                        let snapshot = progress.snapshot();
                        let started = snapshot.request_started_unix;
                        if started != 0 {
                            let elapsed = (chrono::Utc::now().timestamp() - started).max(0);
                            let label = if progress.closure_phase.load(Ordering::Relaxed) { "Closure check" } else { "Conversation" };
                            let in_flight = snapshot.in_flight;
                            let done = snapshot.conversation_index.saturating_sub(in_flight);
                            ui.label(format!(
                                "{label} {done} of {} done · {in_flight} in flight · {elapsed}s on the oldest request",
                                progress.conversation_total.load(Ordering::Relaxed),
                            ));
                        }
                        if progress.cancel.load(Ordering::Relaxed) { ui.label("Stopping; the current request is dropped within a second."); }
                        else if ui.button("Stop scan").clicked() { progress.cancel.store(true, Ordering::Relaxed); }
                    }
                }
                ui.separator();
                ui.label(RichText::new("Saved connection settings").strong());
                self.settings_controls(ui);
                ui.label("Settings and abstract loop decisions save in Windows Credential Manager. Mail and descriptions remain in memory; scan to reconstruct them after restart. Reviewed reminders live in Microsoft To Do.");
                ui.label(RichText::new("This is the Windows companion setup window. Outlook integration is a separate step.").small().color(MUTED));
            });
        });
        if self.model.persist_changes() {
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(feature = "ui-screenshot")]
fn capture_empty_window(ctx: &egui::Context) {
    let captured = ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(image) = captured {
        let path = std::env::temp_dir().join("openloops-setup-empty.png");
        let width = u32::try_from(image.width()).expect("window width fits u32");
        let height = u32::try_from(image.height()).expect("window height fits u32");
        image::save_buffer(path, image.as_raw(), width, height, image::ColorType::Rgba8)
            .expect("write empty setup screenshot outside repository");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    } else if ctx.cumulative_pass_nr() >= 4 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::sync::mpsc;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    use zeroize::Zeroizing;

    #[derive(Clone, Default)]
    struct MemoryStore {
        saved: Rc<RefCell<Option<Settings>>>,
        fail_read: Rc<Cell<bool>>,
        fail_write: Rc<Cell<bool>>,
    }
    impl SettingsStore for MemoryStore {
        fn load(&self) -> Result<Option<Settings>, SettingsError> {
            if self.fail_read.get() {
                Err(SettingsError::Invalid)
            } else {
                Ok(self.saved.borrow().clone())
            }
        }
        fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
            if self.fail_write.get() {
                return Err(SettingsError::Unavailable);
            }
            *self.saved.borrow_mut() = Some(settings.clone());
            Ok(())
        }
        fn delete(&self) -> Result<(), SettingsError> {
            *self.saved.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn app_restores_settings_without_authentication_or_revealing_key_and_forgets_them() {
        let memory = MemoryStore::default();
        let mut app = SetupApp::with_store(
            &egui::Context::default(),
            Ok(Some(Box::new(memory.clone()))),
        );
        app.model.client_id = "00000000-0000-0000-0000-000000000000".into();
        app.model.groups = "one@example.invalid\ntwo@example.invalid\nthree@example.invalid".into();
        app.model.shared = "shared@example.invalid".into();
        app.model.key = Zeroizing::new("synthetic-key".into());
        app.model.selected = "deepseek-v4-flash:0731".into();
        app.model.openrouter_key = Zeroizing::new("synthetic-openrouter-key".into());
        app.model.openrouter_selected = "vendor/model-1".into();
        app.model.ollama_plan = OllamaPlan::Pro;
        app.model.openrouter_parallel = 48;
        app.reveal_key = true;
        app.model.pending_save = true;
        assert!(app.model.persist_changes());
        drop(app);
        let mut reopened = SetupApp::with_store(
            &egui::Context::default(),
            Ok(Some(Box::new(memory.clone()))),
        );
        assert_eq!(reopened.model.groups.lines().count(), 3);
        assert_eq!(reopened.model.shared, "shared@example.invalid");
        assert_eq!(&*reopened.model.key, "synthetic-key");
        assert_eq!(reopened.model.selected, "deepseek-v4-flash:0731");
        assert_eq!(reopened.model.provider, Provider::OllamaCloud);
        assert_eq!(&*reopened.model.openrouter_key, "synthetic-openrouter-key");
        assert_eq!(reopened.model.openrouter_selected, "vendor/model-1");
        assert_eq!(reopened.model.ollama_plan, OllamaPlan::Pro);
        assert_eq!(reopened.model.openrouter_parallel, 48);
        assert!(!reopened.reveal_key);
        assert!(!reopened.model.microsoft.succeeded);
        assert!(reopened.model.pending.is_none());
        reopened.model.forget_settings();
        assert!(memory.saved.borrow().is_none());
        assert!(reopened.model.key.is_empty());
        assert!(reopened.model.openrouter_key.is_empty());
        assert!(reopened.model.groups.is_empty());
        assert!(!reopened.model.persist_changes());
    }

    /// Guards against a future `poll`/`start` signature change silently
    /// losing the "notify the caller when a result lands" contract this
    /// screen depends on for its egui repaint hook.
    #[test]
    fn start_and_poll_still_round_trip_through_a_plain_callback() {
        let mut app = SetupApp::with_store(&egui::Context::default(), Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.model.pending = Some(receiver);
        sender.send(Outcome::Generation(Ok(()))).unwrap();
        app.model.poll(|| {});
        assert!(app.model.model_status.succeeded);
    }
}
