//! Native setup interface with explicit current-user Windows credential storage.
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

use crate::review_ui::ReviewState;
use crate::settings::{Provider, Settings, SettingsError, SettingsStore, production_store};
use eframe::egui::{self, Color32, RichText};
use openloops_graph::live::{
    ConnectionConfig, ConnectionError, ConnectionReport, check_connection,
};
use openloops_inference::ollama::{OllamaCloud, available_models, suggested_model};
use openloops_inference::openrouter::{ModelChoice, OpenRouter, available_zdr_models};
use openloops_inference::provider::ProviderError;
use zeroize::Zeroizing;

const GREEN: Color32 = Color32::from_rgb(29, 87, 67);
const INK: Color32 = Color32::from_rgb(30, 43, 40);
const MUTED: Color32 = Color32::from_rgb(94, 111, 104);

enum Outcome {
    Microsoft(Result<ConnectionReport, ConnectionError>),
    Models(Result<Vec<String>, ProviderError>),
    ZdrModels(Result<Vec<ModelChoice>, ProviderError>),
    Generation(Result<(), ProviderError>),
    Mail(Result<Vec<openloops_graph::live::review::SourceReview>, ConnectionError>),
    Scan(Result<crate::review_ui::ScanResult, ProviderError>, String),
    Reminder([u8; 32], openloops_graph::live::reminders::ReminderOutcome),
}

#[derive(Clone, Copy)]
enum Service {
    Microsoft,
    Model,
    Review,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    Review,
}

#[derive(Default)]
struct Status {
    lines: Vec<String>,
    succeeded: bool,
}

pub struct SetupApp {
    client_id: String,
    groups: String,
    shared: String,
    key: Zeroizing<String>,
    reveal_key: bool,
    models: Vec<String>,
    selected: String,
    provider: Provider,
    openrouter_key: Zeroizing<String>,
    zdr_models: Vec<ModelChoice>,
    openrouter_selected: String,
    microsoft: Status,
    model_status: Status,
    pending: Option<Receiver<Outcome>>,
    pending_service: Service,
    progress: &'static str,
    started: Instant,
    store: Option<Box<dyn SettingsStore>>,
    settings_status: Status,
    pending_save: bool,
    automatic_save: bool,
    active_tab: Tab,
    review: ReviewState,
    review_status: Status,
    scan_progress: Option<Arc<crate::review_ui::ScanProgress>>,
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
        let mut app = Self {
            client_id: String::new(),
            groups: String::new(),
            shared: String::new(),
            key: Zeroizing::new(String::new()),
            reveal_key: false,
            models: vec![],
            selected: String::new(),
            provider: Provider::default(),
            openrouter_key: Zeroizing::new(String::new()),
            zdr_models: vec![],
            openrouter_selected: String::new(),
            microsoft: Status::default(),
            model_status: Status::default(),
            pending: None,
            pending_service: Service::Microsoft,
            progress: "",
            started: Instant::now(),
            store: None,
            settings_status: Status::default(),
            pending_save: false,
            automatic_save: true,
            active_tab: if cfg!(feature = "ui-screenshot") {
                Tab::Review
            } else {
                Tab::Connections
            },
            review: ReviewState::default(),
            review_status: Status::default(),
            scan_progress: None,
        };
        match store {
            Ok(store) => {
                app.store = store;
                app.reload_settings();
            }
            Err(error) => {
                app.automatic_save = false;
                app.settings_error(error);
            }
        }
        #[cfg(feature = "ui-screenshot")]
        if std::env::args().any(|arg| arg == "--preview-review") {
            app.review = crate::review_ui::layout_fixture();
            app.selected = "Synthetic layout check".into();
            app.provider = Provider::OllamaCloud;
            app.active_tab = Tab::Review;
        }
        app
    }

    fn settings_error(&mut self, error: SettingsError) {
        self.settings_status = Status {
            lines: vec![error.to_string()],
            succeeded: false,
        };
    }

    fn apply_settings(&mut self, settings: Settings) {
        self.client_id = settings.client_id;
        self.groups = settings.groups;
        self.shared = settings.shared;
        self.key = settings.key;
        self.selected = settings.selected;
        self.models = if self.selected.is_empty() {
            vec![]
        } else {
            vec![self.selected.clone()]
        };
        self.provider = settings.provider;
        self.openrouter_key = settings.openrouter_key;
        self.openrouter_selected = settings.openrouter_selected;
        self.zdr_models = if self.openrouter_selected.is_empty() {
            vec![]
        } else {
            vec![ModelChoice {
                id: self.openrouter_selected.clone(),
                label: self.openrouter_selected.clone(),
            }]
        };
        self.trim_keys();
        self.reveal_key = false;
        self.microsoft = Status::default();
        self.model_status = Status::default();
        self.pending_save = false;
        self.review = ReviewState::default();
        self.review_status = Status::default();
    }

    fn reload_settings(&mut self) {
        let Some(store) = &self.store else {
            return;
        };
        match store.load() {
            Ok(settings) => {
                let existed = settings.is_some();
                self.apply_settings(settings.unwrap_or_default());
                self.automatic_save = true;
                self.active_tab = if existed
                    && !self.client_id.is_empty()
                    && !self.active_key().is_empty()
                    && !self.selected_model().is_empty()
                {
                    Tab::Review
                } else {
                    Tab::Connections
                };
                self.settings_status = Status {
                    lines: vec![
                        if existed {
                            "Saved settings restored. Microsoft sign-in is still required."
                        } else {
                            "Settings will save automatically on this Windows account."
                        }
                        .into(),
                    ],
                    succeeded: true,
                };
            }
            Err(error) => {
                self.automatic_save = false;
                self.settings_error(error);
                self.settings_status.lines.push("Automatic saving is paused. Reload to retry, or Save settings to replace the saved copy with these inputs.".into());
            }
        }
    }

    fn save_settings(&mut self) {
        self.pending_save = false;
        let Some(store) = &self.store else {
            return;
        };
        let settings = Settings {
            client_id: self.client_id.clone(),
            groups: self.groups.clone(),
            shared: self.shared.clone(),
            key: self.key.clone(),
            selected: self.selected.clone(),
            provider: self.provider,
            openrouter_key: self.openrouter_key.clone(),
            openrouter_selected: self.openrouter_selected.clone(),
        };
        match store.save(&settings) {
            Ok(()) => {
                self.automatic_save = true;
                self.settings_status = Status {
                    lines: vec![
                        "Saved on this Windows account, including your API key and model choice."
                            .into(),
                    ],
                    succeeded: true,
                };
            }
            Err(error) => self.settings_error(error),
        }
    }

    fn persist_changes(&mut self) -> bool {
        if self.pending_save && self.automatic_save {
            self.save_settings();
            true
        } else {
            false
        }
    }

    fn forget_settings(&mut self) {
        let Some(store) = &self.store else {
            return;
        };
        match store.delete() {
            Ok(()) => {
                self.apply_settings(Settings::default());
                self.automatic_save = true;
                self.settings_status = Status {
                    lines: vec!["Saved settings removed and inputs cleared.".into()],
                    succeeded: true,
                };
            }
            Err(_) => {
                self.settings_status = Status { lines: vec!["Could not remove saved settings. They remain in Windows Credential Manager; try again.".into()], succeeded: false };
            }
        }
    }

    fn settings_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(self.pending.is_none() && self.store.is_some(), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save settings").clicked() {
                    self.save_settings();
                }
                if ui.button("Reload saved settings").clicked() {
                    self.reload_settings();
                }
                if ui.button("Forget saved settings").clicked() {
                    self.forget_settings();
                }
            });
        });
        status(ui, &self.settings_status);
    }

    fn start(
        &mut self,
        ctx: &egui::Context,
        service: Service,
        label: &'static str,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        self.pending_service = service;
        self.progress = label;
        self.started = Instant::now();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.pending else {
            return;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                let status = Status {
                    lines: vec!["The operation stopped unexpectedly. Please try again.".into()],
                    succeeded: false,
                };
                match self.pending_service {
                    Service::Microsoft => self.microsoft = status,
                    Service::Model => self.model_status = status,
                    Service::Review => self.review_status = status,
                }
                return;
            }
        };
        self.pending = None;
        match outcome {
            Outcome::Reminder(key, outcome) => {
                self.reminder_outcome(key, outcome);
            }
            Outcome::Models(Ok(models)) => self.loaded_models(models),
            Outcome::ZdrModels(Ok(models)) => self.loaded_zdr_models(models),
            Outcome::Models(Err(error))
            | Outcome::ZdrModels(Err(error))
            | Outcome::Generation(Err(error)) => {
                self.model_status = Status {
                    lines: vec![error.to_string()],
                    succeeded: false,
                };
            }
            Outcome::Generation(Ok(())) => {
                self.model_status = Status {
                    lines: vec![format!(
                        "{} passed the generation check.",
                        self.selected_model()
                    )],
                    succeeded: true,
                };
            }
            Outcome::Microsoft(result) => {
                self.microsoft = microsoft_status(result);
            }
            Outcome::Mail(Ok(sources)) => {
                self.review = ReviewState::loaded(sources);
                if self.review.messages.is_empty() {
                    self.review_status = Status { lines: vec!["No messages could be loaded for analysis. Check scan coverage and errors.".into()], succeeded: false };
                } else {
                    self.start_scan(ctx);
                }
            }
            Outcome::Mail(Err(error)) => {
                self.review_status = Status {
                    lines: vec![error.to_string()],
                    succeeded: false,
                };
            }
            Outcome::Scan(result, model) => {
                self.scan_progress = None;
                match result {
                    Ok(scan) => {
                        self.review.set_scan(scan, model);
                        self.review_status = Status { lines: vec![if self.review.scan_incomplete { "Scan incomplete. Results from completed batches are shown below; check scan coverage and errors." } else { "Scan finished. Review the open loops and their evidence below." }.into()], succeeded: !self.review.scan_incomplete };
                    }
                    Err(error) => {
                        self.review_status = Status {
                            lines: vec![error.to_string()],
                            succeeded: false,
                        }
                    }
                }
            }
        }
    }

    /// Keeps the Ollama Cloud selection only while the freshly loaded list
    /// still offers it; otherwise falls back to the suggested model.
    fn loaded_models(&mut self, models: Vec<String>) {
        if !models.contains(&self.selected) {
            self.selected = suggested_model(&models)
                .and_then(|index| models.get(index))
                .cloned()
                .or_else(|| models.first().cloned())
                .unwrap_or_default();
            self.pending_save = true;
        }
        self.model_status = Status {
            lines: vec![if models.is_empty() {
                "No cloud models were returned. Try loading the list again.".into()
            } else {
                format!(
                    "{} cloud models loaded. Choose one and test it below.",
                    models.len()
                )
            }],
            succeeded: false,
        };
        self.models = models;
    }

    /// Keeps the `OpenRouter` selection only while the model still has a
    /// zero-data-retention endpoint; otherwise falls back to the first one.
    fn loaded_zdr_models(&mut self, models: Vec<ModelChoice>) {
        if !models
            .iter()
            .any(|model| model.id == self.openrouter_selected)
        {
            self.openrouter_selected = models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_default();
            self.pending_save = true;
        }
        self.model_status = Status {
            lines: vec![if models.is_empty() {
                "No zero-data-retention models were returned. Try loading the list again.".into()
            } else {
                format!(
                    "{} zero-data-retention models loaded. Choose one and test it below.",
                    models.len()
                )
            }],
            succeeded: false,
        };
        self.zdr_models = models;
    }

    fn reminder_outcome(
        &mut self,
        key: [u8; 32],
        outcome: openloops_graph::live::reminders::ReminderOutcome,
    ) {
        use crate::loop_state::{Reminder, now};
        use openloops_graph::live::reminders::ReminderOutcome;
        let mut record = self.review.decisions.get(&key);
        let text=match outcome {
            ReminderOutcome::Created=>{record.reminder=Reminder::Created; "Reminder created in your Microsoft To Do Tasks list.".to_owned()},
            ReminderOutcome::NotCreated(error)=>{record.reminder=Reminder::None;format!("No reminder was created: {error} Sign in with the same account used for the scan.")},
            ReminderOutcome::Uncertain=>"Microsoft did not confirm the write. Check To Do before trying again; OpenLoops will not automatically retry.".to_owned(),
        };
        record.updated = now();
        self.review.action_status = match self.review.decisions.update(record) {
            Ok(()) => text,
            Err(error) => format!("{text} {error}"),
        };
    }

    /// Provider keys never contain whitespace. Trimming once, where the key
    /// enters the app, keeps the enabled checks, the Test button, the scan,
    /// and the saved record on exactly the same value.
    fn trim_keys(&mut self) {
        for key in [&mut self.key, &mut self.openrouter_key] {
            let trimmed = key.trim().to_owned();
            if trimmed.len() != key.len() {
                **key = trimmed;
            }
        }
    }

    /// The key for the selected provider. Each provider keeps its own, so
    /// switching back and forth never sends one provider's key to the other.
    fn active_key(&self) -> &Zeroizing<String> {
        match self.provider {
            Provider::OllamaCloud => &self.key,
            Provider::OpenRouter => &self.openrouter_key,
        }
    }

    /// The model chosen for the selected provider.
    fn selected_model(&self) -> &str {
        match self.provider {
            Provider::OllamaCloud => &self.selected,
            Provider::OpenRouter => &self.openrouter_selected,
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let messages = self.review.messages.clone();
        let key = self.active_key().clone();
        let model = self.selected_model().to_owned();
        let provider = self.provider;
        let progress = Arc::new(crate::review_ui::ScanProgress::default());
        progress.total.store(messages.len(), Ordering::Relaxed);
        self.scan_progress = Some(progress.clone());
        self.review.analysis = None;
        self.review.scan_summary.clear();
        self.review.scan_errors.clear();
        self.review_status = Status::default();
        self.start(
            ctx,
            Service::Review,
            match provider {
                Provider::OllamaCloud => "Finding open loops with Ollama Cloud",
                Provider::OpenRouter => "Finding open loops with OpenRouter",
            },
            move || {
                Outcome::Scan(
                    crate::review_ui::scan(provider, key.to_string(), &model, &messages, &progress),
                    model,
                )
            },
        );
    }

    fn microsoft_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("1  Connect Microsoft");
        ui.label("Your own inbox is included automatically.");
        ui.add_space(4.0);
        ui.label(RichText::new("Application (client) ID").strong());
        let mut changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.client_id)
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
                egui::TextEdit::multiline(&mut self.groups)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Enter group email addresses"),
            )
            .changed();
        ui.collapsing("Shared mailboxes (optional)", |ui| {
            ui.label("For mailboxes under Shared with me or Open another mailbox.");
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut self.shared)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Enter shared mailbox addresses"),
                )
                .changed();
        });
        if changed {
            self.microsoft = Status::default();
            self.pending_save = true;
            self.review = ReviewState::default();
            self.review_status = Status::default();
        }
        ui.add_space(6.0);
        if ui
            .add_enabled(
                !self.client_id.trim().is_empty(),
                egui::Button::new(RichText::new("Sign in & check inboxes").color(Color32::WHITE))
                    .fill(GREEN),
            )
            .clicked()
        {
            let config = ConnectionConfig::new(self.client_id.trim(), Some(&self.shared))
                .and_then(|config| config.with_groups(Some(&self.groups)));
            match config {
                Ok(config) => {
                    self.microsoft = Status::default();
                    self.start(
                        ui.ctx(),
                        Service::Microsoft,
                        "Complete Microsoft sign-in in your browser",
                        move || Outcome::Microsoft(check_connection(&config)),
                    );
                }
                Err(error) => {
                    self.microsoft = Status {
                        lines: vec![error.to_string()],
                        succeeded: false,
                    }
                }
            }
        }
        status(ui, &self.microsoft);
    }

    fn model_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("2  Choose your AI model");
        ui.label(RichText::new("Provider").strong());
        let before = self.provider;
        ui.horizontal(|ui| {
            for provider in [Provider::OllamaCloud, Provider::OpenRouter] {
                ui.selectable_value(&mut self.provider, provider, provider.label());
            }
        });
        if before != self.provider {
            self.model_status = Status::default();
            self.pending_save = true;
        }
        match self.provider {
            Provider::OllamaCloud => self.ollama_inputs(ui),
            Provider::OpenRouter => self.openrouter_inputs(ui),
        }
        if ui
            .add_enabled(
                !self.selected_model().is_empty() && !self.active_key().is_empty(),
                egui::Button::new(RichText::new("Test selected model").color(Color32::WHITE))
                    .fill(GREEN),
            )
            .clicked()
        {
            let key = self.active_key().clone();
            let model = self.selected_model().to_owned();
            let provider = self.provider;
            self.model_status = Status::default();
            self.start(
                ui.ctx(),
                Service::Model,
                "Testing the selected cloud model (up to 60 seconds per request)",
                move || {
                    Outcome::Generation(match provider {
                        Provider::OllamaCloud => OllamaCloud::connect(key.to_string(), &model)
                            .and_then(|provider| provider.check_generation()),
                        Provider::OpenRouter => OpenRouter::connect(key.to_string(), &model)
                            .and_then(|provider| provider.check_generation()),
                    })
                },
            );
        }
        status(ui, &self.model_status);
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "This connection test sends no email. Scanning inboxes sends recent message text from your configured inboxes to {}.",
                self.provider_disclosure()
            ))
            .small()
            .color(MUTED),
        );
    }

    /// How the selected provider is named in every data-transmission
    /// disclosure, including the routing restriction where one applies.
    fn provider_disclosure(&self) -> &'static str {
        match self.provider {
            Provider::OllamaCloud => "Ollama Cloud",
            Provider::OpenRouter => "OpenRouter, restricted to zero-data-retention endpoints",
        }
    }

    fn ollama_inputs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("API key").strong());
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.key)
                    .char_limit(4096)
                    .password(!self.reveal_key)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste your Ollama API key"),
            )
            .changed()
        {
            self.trim_keys();
            self.models.clear();
            self.selected.clear();
            self.model_status = Status::default();
            self.pending_save = true;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reveal_key, "Show key");
            ui.hyperlink_to("Create an API key", "https://ollama.com/settings/keys");
        });
        if ui
            .add_enabled(!self.key.is_empty(), egui::Button::new("Load cloud models"))
            .clicked()
        {
            let key = self.key.clone();
            self.model_status = Status::default();
            self.start(
                ui.ctx(),
                Service::Model,
                "Loading Ollama Cloud models",
                move || Outcome::Models(available_models(&key)),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Model").strong());
        let before = self.selected.clone();
        egui::ComboBox::from_id_salt("ollama-model")
            .selected_text(if self.selected.is_empty() {
                "Load models to choose"
            } else {
                &self.selected
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for model in &self.models {
                    ui.selectable_value(&mut self.selected, model.clone(), model);
                }
            });
        if before != self.selected {
            self.model_status = Status::default();
            self.pending_save = true;
        }
    }

    fn openrouter_inputs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("API key").strong());
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.openrouter_key)
                    .char_limit(4096)
                    .password(!self.reveal_key)
                    .desired_width(f32::INFINITY)
                    .hint_text("Paste your OpenRouter API key"),
            )
            .changed()
        {
            self.trim_keys();
            self.model_status = Status::default();
            self.pending_save = true;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reveal_key, "Show key");
            ui.hyperlink_to("Create an API key", "https://openrouter.ai/settings/keys");
        });
        // The listing is public, so it loads without a key and sends none.
        if ui.button("Load ZDR models").clicked() {
            self.model_status = Status::default();
            self.start(
                ui.ctx(),
                Service::Model,
                "Loading OpenRouter zero-data-retention models",
                || Outcome::ZdrModels(available_zdr_models()),
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Model").strong());
        let before = self.openrouter_selected.clone();
        egui::ComboBox::from_id_salt("openrouter-model")
            .selected_text(if self.openrouter_selected.is_empty() {
                "Load models to choose"
            } else {
                &self.openrouter_selected
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for model in &self.zdr_models {
                    ui.selectable_value(
                        &mut self.openrouter_selected,
                        model.id.clone(),
                        if model.label == model.id {
                            model.id.clone()
                        } else {
                            format!("{} ({})", model.label, model.id)
                        },
                    );
                }
            });
        if before != self.openrouter_selected {
            self.model_status = Status::default();
            self.pending_save = true;
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
        if self.review.analysis.is_none() {
            ui.label("Scan the last 30 days of Inbox and Sent Items, plus configured Groups and shared mailboxes. Each conversation is checked for expectations and later replies.");
            ui.label(format!(
                "Message text and participants go to your chosen model at {}. Attachments are not sent.",
                self.provider_disclosure()
            ));
        }
        let disclosure = self.provider_disclosure();
        ui.collapsing("Scan scope and data sent to the model provider", |ui| {
            ui.label("The last 30 days of Inbox and Sent Items: up to 100 messages per folder. Groups: up to 20 recent threads and 40 posts per thread, including earlier thread context. At most 10 configured sources. Capped sources and large conversations are reported as incomplete.");
            ui.label(format!("Subjects, current text, quoted history and participants are sent to the selected provider ({disclosure}). Attachments are not sent. Scans run when you click Scan; this preview is not an unattended background service."));
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.client_id.trim().is_empty()
                        && !self.active_key().is_empty()
                        && !self.selected_model().is_empty(),
                    egui::Button::new(RichText::new("Scan inboxes").color(Color32::WHITE))
                        .fill(GREEN),
                )
                .clicked()
            {
                match ConnectionConfig::new(self.client_id.trim(), Some(&self.shared))
                    .and_then(|config| config.with_groups(Some(&self.groups)))
                {
                    Ok(config) => {
                        self.review = ReviewState::default();
                        self.review_status = Status::default();
                        self.start(
                            ui.ctx(),
                            Service::Review,
                            "Complete Microsoft sign-in; then scanning recent messages",
                            move || {
                                Outcome::Mail(openloops_graph::live::review::load_recent(&config))
                            },
                        );
                    }
                    Err(error) => {
                        self.review_status = Status {
                            lines: vec![error.to_string()],
                            succeeded: false,
                        }
                    }
                }
            }
            if ui
                .add_enabled(
                    !self.review.messages.is_empty()
                        && !self.active_key().is_empty()
                        && !self.selected_model().is_empty(),
                    egui::Button::new("Rescan loaded mail"),
                )
                .clicked()
            {
                self.start_scan(ui.ctx());
            }
            if ui.button("Clear results and mail").clicked() {
                self.review = ReviewState::default();
                self.review_status = Status::default();
            }
        });
        ui.add_space(8.0);
        ui.label(format!(
            "Selected model: {} ({})",
            if self.selected_model().is_empty() {
                "Choose one in Connections"
            } else {
                self.selected_model()
            },
            self.provider.label()
        ));
        status(ui, &self.review_status);
        self.review.show(ui);
        if let Some((key, request)) = self.review.pending_reminder.take() {
            match ConnectionConfig::new(self.client_id.trim(), None) {
                Ok(config) => self.start(
                    ui.ctx(),
                    Service::Review,
                    "Sign in to create the reviewed Microsoft To Do reminder",
                    move || {
                        Outcome::Reminder(
                            key,
                            openloops_graph::live::reminders::create(&config, &request),
                        )
                    },
                ),
                Err(error) => {
                    let mut record = self.review.decisions.get(&key);
                    record.reminder = crate::loop_state::Reminder::None;
                    record.updated = crate::loop_state::now();
                    let _ = self.review.decisions.update(record);
                    self.review.action_status = error.to_string();
                }
            }
        }
    }
}

fn microsoft_status(result: Result<ConnectionReport, ConnectionError>) -> Status {
    match result {
        Err(error) => Status {
            lines: vec![error.to_string()],
            succeeded: false,
        },
        Ok(report) => {
            let mut lines = vec![if report.own_inbox_accessible {
                "Personal inbox: access confirmed.".into()
            } else {
                "Personal inbox: access not confirmed.".into()
            }];
            let mut succeeded = report.own_inbox_accessible;
            for (kind, results) in [
                ("Group inbox", report.group_inbox_results),
                ("Shared mailbox", report.shared_inbox_results),
            ] {
                for (index, result) in results.into_iter().enumerate() {
                    match result {
                        Ok(()) => lines.push(format!("{kind} {}: access confirmed.", index + 1)),
                        Err(error) => {
                            lines.push(format!("{kind} {}: {error}", index + 1));
                            succeeded = false;
                        }
                    }
                }
            }
            Status { lines, succeeded }
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
        self.poll(ui.ctx());
        let busy = self.pending.is_some();
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
                    ui.horizontal(|ui| { ui.spinner(); ui.label(format!("{} · {}s", self.progress, self.started.elapsed().as_secs())); });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    if let Some(progress) = &self.scan_progress {
                        ui.label(format!("{} / {} messages processed", progress.processed.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)));
                        let started = progress.request_started_unix.load(Ordering::Relaxed);
                        if started != 0 {
                            let elapsed = (chrono::Utc::now().timestamp() - started).max(0);
                            ui.label(format!(
                                "Conversation {} of {} · {elapsed}s on this request",
                                progress.conversation_index.load(Ordering::Relaxed),
                                progress.conversation_total.load(Ordering::Relaxed),
                            ));
                        }
                        if progress.cancel.load(Ordering::Relaxed) { ui.label("Stopping; the current request is abandoned within a second."); }
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
        if self.persist_changes() {
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
    use openloops_graph::live::SharedScope;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

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
        app.client_id = "00000000-0000-0000-0000-000000000000".into();
        app.groups = "one@example.invalid\ntwo@example.invalid\nthree@example.invalid".into();
        app.shared = "shared@example.invalid".into();
        app.key = Zeroizing::new("synthetic-key".into());
        app.selected = "deepseek-v4-flash:0731".into();
        app.openrouter_key = Zeroizing::new("synthetic-openrouter-key".into());
        app.openrouter_selected = "vendor/model-1".into();
        app.reveal_key = true;
        app.pending_save = true;
        assert!(app.persist_changes());
        drop(app);
        let mut reopened = SetupApp::with_store(
            &egui::Context::default(),
            Ok(Some(Box::new(memory.clone()))),
        );
        assert_eq!(reopened.groups.lines().count(), 3);
        assert_eq!(reopened.shared, "shared@example.invalid");
        assert_eq!(&*reopened.key, "synthetic-key");
        assert_eq!(reopened.selected, "deepseek-v4-flash:0731");
        assert_eq!(reopened.provider, Provider::OllamaCloud);
        assert_eq!(&*reopened.openrouter_key, "synthetic-openrouter-key");
        assert_eq!(reopened.openrouter_selected, "vendor/model-1");
        assert!(!reopened.reveal_key);
        assert!(!reopened.microsoft.succeeded);
        assert!(reopened.pending.is_none());
        reopened.forget_settings();
        assert!(memory.saved.borrow().is_none());
        assert!(reopened.key.is_empty());
        assert!(reopened.openrouter_key.is_empty());
        assert!(reopened.groups.is_empty());
        assert!(!reopened.persist_changes());
    }

    #[test]
    fn failed_load_and_failed_save_preserve_existing_settings() {
        let memory = MemoryStore::default();
        memory
            .save(&Settings {
                selected: "saved-model".into(),
                ..Settings::default()
            })
            .unwrap();
        memory.fail_read.set(true);
        let mut app = SetupApp::with_store(
            &egui::Context::default(),
            Ok(Some(Box::new(memory.clone()))),
        );
        app.selected = "replacement-model".into();
        app.pending_save = true;
        assert!(!app.persist_changes());
        assert!(!app.settings_status.succeeded);
        assert_eq!(
            memory.saved.borrow().as_ref().unwrap().selected,
            "saved-model"
        );
        memory.fail_read.set(false);
        app.reload_settings();
        app.selected = "new-model".into();
        memory.fail_write.set(true);
        app.pending_save = true;
        app.persist_changes();
        assert!(!app.settings_status.succeeded);
        assert_eq!(
            memory.saved.borrow().as_ref().unwrap().selected,
            "saved-model"
        );
        memory.fail_write.set(false);
        app.save_settings();
        assert!(app.settings_status.succeeded);
        assert_eq!(
            memory.saved.borrow().as_ref().unwrap().selected,
            "new-model"
        );
    }

    #[test]
    fn switching_model_invalidates_previous_success_and_binds_next_result() {
        let mut app = SetupApp::new(&egui::Context::default());
        app.selected = "first-model".into();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::Models(Ok(vec![
                "deepseek-v4-flash:0731".into(),
                "other-model".into(),
            ])))
            .unwrap();
        app.poll(&egui::Context::default());
        assert_eq!(app.selected, "deepseek-v4-flash:0731");
        assert!(!app.model_status.succeeded);
        assert!(app.pending.is_none());

        app.selected = "other-model".into();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender.send(Outcome::Generation(Ok(()))).unwrap();
        app.poll(&egui::Context::default());
        assert!(app.model_status.succeeded);
        assert!(app.model_status.lines[0].contains("other-model"));
    }

    #[test]
    fn keys_are_trimmed_once_so_testing_and_scanning_use_the_same_value() {
        let mut app = SetupApp::new(&egui::Context::default());
        app.key = Zeroizing::new("  synthetic-ollama-key\n".into());
        app.openrouter_key = Zeroizing::new("\tsynthetic-openrouter-key ".into());
        app.trim_keys();
        assert_eq!(&*app.key, "synthetic-ollama-key");
        assert_eq!(&*app.openrouter_key, "synthetic-openrouter-key");
        assert_eq!(&**app.active_key(), "synthetic-ollama-key");
        app.provider = Provider::OpenRouter;
        assert_eq!(&**app.active_key(), "synthetic-openrouter-key");
    }

    #[test]
    fn each_provider_keeps_its_own_key_and_model_selection() {
        let mut app = SetupApp::new(&egui::Context::default());
        app.key = Zeroizing::new("synthetic-ollama-key".into());
        app.selected = "deepseek-v4-flash:0731".into();
        app.openrouter_key = Zeroizing::new("synthetic-openrouter-key".into());
        app.provider = Provider::OpenRouter;
        assert_eq!(&**app.active_key(), "synthetic-openrouter-key");
        assert_eq!(app.selected_model(), "");

        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::ZdrModels(Ok(vec![
                ModelChoice {
                    id: "vendor/model-1".into(),
                    label: "Vendor: Model 1".into(),
                },
                ModelChoice {
                    id: "vendor/model-2".into(),
                    label: "Vendor: Model 2".into(),
                },
            ])))
            .unwrap();
        app.poll(&egui::Context::default());
        assert_eq!(app.selected_model(), "vendor/model-1");
        assert!(app.model_status.lines[0].contains("zero-data-retention"));

        app.provider = Provider::OllamaCloud;
        assert_eq!(&**app.active_key(), "synthetic-ollama-key");
        assert_eq!(app.selected_model(), "deepseek-v4-flash:0731");
        app.provider = Provider::OpenRouter;
        assert_eq!(app.selected_model(), "vendor/model-1");
    }

    #[test]
    fn disconnected_worker_reports_failure_for_correct_service() {
        let mut app = SetupApp::new(&egui::Context::default());
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Microsoft;
        drop(sender);
        app.poll(&egui::Context::default());
        assert!(app.pending.is_none());
        assert!(!app.microsoft.lines.is_empty());
        assert!(app.model_status.lines.is_empty());
    }

    #[test]
    fn group_failure_is_visible_and_does_not_mark_microsoft_ready() {
        let status = microsoft_status(Ok(ConnectionReport {
            own_inbox_accessible: true,
            shared_scope: SharedScope::NotReported,
            shared_inbox_results: vec![],
            group_inbox_results: vec![Ok(()), Err(ConnectionError::AccessDenied), Ok(())],
        }));
        assert!(!status.succeeded);
        assert_eq!(status.lines.len(), 4);
        assert!(status.lines[2].contains("HTTP 403"));
    }
}
