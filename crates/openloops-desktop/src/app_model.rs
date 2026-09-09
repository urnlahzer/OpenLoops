//! Toolkit-free setup/connection state: job start/poll machinery, saved
//! settings, and provider/account pure logic used by the native adapter.
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

use crate::review_model::ReviewState;
use crate::settings::{
    OllamaPlan, Provider, Settings, SettingsError, SettingsStore, max_parallel, production_store,
};
use openloops_graph::live::{ConnectionError, ConnectionReport};
use openloops_inference::ollama::suggested_model;
use openloops_inference::openrouter::ModelChoice;
use openloops_inference::provider::ProviderError;
use zeroize::Zeroizing;

pub(crate) enum Outcome {
    Microsoft(Result<ConnectionReport, ConnectionError>),
    Models(Result<Vec<String>, ProviderError>),
    ZdrModels(Result<Vec<ModelChoice>, ProviderError>),
    Generation(Result<(), ProviderError>),
    #[allow(dead_code)]
    Mail(Result<Vec<openloops_graph::live::review::SourceReview>, ConnectionError>),
    Scan(
        Result<crate::review_model::ScanResult, ProviderError>,
        String,
    ),
    #[allow(dead_code)]
    Reminder([u8; 32], openloops_graph::live::reminders::ReminderOutcome),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Service {
    Microsoft,
    Model,
    Review,
}

#[derive(Default)]
pub(crate) struct Status {
    pub(crate) lines: Vec<String>,
    pub(crate) succeeded: bool,
}

/// Toolkit-free setup/connection/review model used by the native UI adapter.
pub struct AppModel {
    pub client_id: String,
    pub groups: String,
    pub shared: String,
    pub key: Zeroizing<String>,
    pub models: Vec<String>,
    pub selected: String,
    pub provider: Provider,
    pub ollama_plan: OllamaPlan,
    pub openrouter_key: Zeroizing<String>,
    pub zdr_models: Vec<ModelChoice>,
    pub openrouter_selected: String,
    pub openrouter_parallel: u16,
    pub(crate) microsoft: Status,
    pub(crate) model_status: Status,
    pub(crate) pending: Option<Receiver<Outcome>>,
    pub(crate) pending_service: Service,
    pub progress: &'static str,
    pub started: Instant,
    pub store: Option<Box<dyn SettingsStore>>,
    pub(crate) settings_status: Status,
    pub pending_save: bool,
    pub automatic_save: bool,
    pub review: ReviewState,
    pub(crate) review_status: Status,
    pub scan_progress: Option<Arc<crate::review_model::ScanProgress>>,
    /// Whether the most recent [`AppModel::reload_settings`] call (including
    /// the one `with_store` runs at construction) found an existing saved
    /// record. The native adapter reads this immediately after each such call,
    /// together with [`AppModel::ready_for_review`], to reproduce today's
    /// initial-tab decision without the model owning any UI-nav state.
    pub settings_existed: bool,
}

impl AppModel {
    #[must_use]
    pub fn new() -> Self {
        Self::with_store(production_store())
    }

    #[must_use]
    pub fn with_store(store: Result<Option<Box<dyn SettingsStore>>, SettingsError>) -> Self {
        let mut app = Self {
            client_id: String::new(),
            groups: String::new(),
            shared: String::new(),
            key: Zeroizing::new(String::new()),
            models: vec![],
            selected: String::new(),
            provider: Provider::default(),
            ollama_plan: OllamaPlan::default(),
            openrouter_key: Zeroizing::new(String::new()),
            zdr_models: vec![],
            openrouter_selected: String::new(),
            openrouter_parallel: crate::settings::DEFAULT_OPENROUTER_PARALLEL,
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
            review: ReviewState::default(),
            review_status: Status::default(),
            scan_progress: None,
            settings_existed: false,
        };
        match store {
            Ok(store) => {
                app.store = store;
                app.settings_existed = app.reload_settings();
            }
            Err(error) => {
                app.automatic_save = false;
                app.settings_error(error);
            }
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
        self.ollama_plan = settings.ollama_plan;
        self.openrouter_parallel = settings.openrouter_parallel;
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
        self.microsoft = Status::default();
        self.model_status = Status::default();
        self.pending_save = false;
        self.review = ReviewState::default();
        self.review_status = Status::default();
    }

    /// Reloads settings from the store, if any, applying them the same way
    /// as today. Returns whether a saved record existed (`false` when there
    /// is no store, the load failed, or no record had been saved yet) --
    /// see [`AppModel::settings_existed`] for how the adapter uses this
    /// alongside [`AppModel::ready_for_review`] to decide the initial/
    /// post-reload tab.
    pub fn reload_settings(&mut self) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        match store.load() {
            Ok(settings) => {
                let existed = settings.is_some();
                self.apply_settings(settings.unwrap_or_default());
                self.automatic_save = true;
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
                existed
            }
            Err(error) => {
                self.automatic_save = false;
                self.settings_error(error);
                self.settings_status.lines.push("Automatic saving is paused. Reload to retry, or Save settings to replace the saved copy with these inputs.".into());
                false
            }
        }
    }

    pub fn save_settings(&mut self) {
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
            ollama_plan: self.ollama_plan,
            openrouter_parallel: self.openrouter_parallel,
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

    pub fn persist_changes(&mut self) -> bool {
        if self.pending_save && self.automatic_save {
            self.save_settings();
            true
        } else {
            false
        }
    }

    pub fn forget_settings(&mut self) {
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

    pub(crate) fn start(
        &mut self,
        service: Service,
        label: &'static str,
        work: impl FnOnce() -> Outcome + Send + 'static,
        on_done: impl FnOnce() + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        self.pending_service = service;
        self.progress = label;
        self.started = Instant::now();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            on_done();
        });
    }

    pub fn poll(&mut self, respawn: impl Fn() + Send + Clone + 'static) {
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
                    self.start_scan(respawn.clone());
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
    pub fn trim_keys(&mut self) {
        for key in [&mut self.key, &mut self.openrouter_key] {
            let trimmed = key.trim().to_owned();
            if trimmed.len() != key.len() {
                **key = trimmed;
            }
        }
    }

    /// The key for the selected provider. Each provider keeps its own, so
    /// switching back and forth never sends one provider's key to the other.
    #[must_use]
    pub fn active_key(&self) -> &Zeroizing<String> {
        match self.provider {
            Provider::OllamaCloud => &self.key,
            Provider::OpenRouter => &self.openrouter_key,
        }
    }

    /// How many model requests a scan may keep in flight for the selected
    /// provider; see [`crate::settings::max_parallel`].
    #[must_use]
    pub fn max_parallel(&self) -> usize {
        max_parallel(self.provider, self.ollama_plan, self.openrouter_parallel)
    }

    /// The model chosen for the selected provider.
    #[must_use]
    pub fn selected_model(&self) -> &str {
        match self.provider {
            Provider::OllamaCloud => &self.selected,
            Provider::OpenRouter => &self.openrouter_selected,
        }
    }

    /// How the selected provider is named in every data-transmission
    /// disclosure, including the routing restriction where one applies.
    #[must_use]
    pub fn provider_disclosure(&self) -> &'static str {
        match self.provider {
            Provider::OllamaCloud => "Ollama Cloud",
            Provider::OpenRouter => "OpenRouter, restricted to zero-data-retention endpoints",
        }
    }

    /// Whether the client ID, active key, and selected model are all
    /// non-empty -- the same three checks that decided today's post-reload
    /// active tab. Pure so the adapter can reproduce that decision without
    /// the model owning any UI-nav state.
    #[must_use]
    pub fn ready_for_review(&self) -> bool {
        !self.client_id.is_empty()
            && !self.active_key().is_empty()
            && !self.selected_model().is_empty()
    }

    pub(crate) fn start_scan(&mut self, on_done: impl FnOnce() + Send + 'static) {
        let messages = self.review.messages.clone();
        let key = self.active_key().clone();
        let model = self.selected_model().to_owned();
        let provider = self.provider;
        let parallel = self.max_parallel();
        let progress = Arc::new(crate::review_model::ScanProgress::default());
        progress.total.store(messages.len(), Ordering::Relaxed);
        self.scan_progress = Some(progress.clone());
        self.review.analysis = None;
        self.review.scan_summary.clear();
        self.review.scan_errors.clear();
        self.review_status = Status::default();
        self.start(
            Service::Review,
            match provider {
                Provider::OllamaCloud => "Finding open loops with Ollama Cloud",
                Provider::OpenRouter => "Finding open loops with OpenRouter",
            },
            move || {
                Outcome::Scan(
                    crate::review_model::scan(
                        provider,
                        key.to_string(),
                        &model,
                        parallel,
                        &messages,
                        &progress,
                    ),
                    model,
                )
            },
            on_done,
        );
    }
}

impl Default for AppModel {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn microsoft_status(result: Result<ConnectionReport, ConnectionError>) -> Status {
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

/// Returns the throttled busy indicator text for the native UI.
///
/// The spinner requests continuous repaints, which can produce black frames on
/// hybrid-GPU systems. This indicator advances only on the busy view's 250 ms
/// repaint schedule.
pub(crate) fn busy_indicator(elapsed_millis: u128) -> &'static str {
    match (elapsed_millis / 250) % 3 {
        0 => "·",
        1 => "··",
        2 => "···",
        _ => unreachable!(),
    }
}

/// Whether `url` is an Outlook web link `OpenLoops` is willing to draw a
/// hyperlink to: exactly the two accepted host prefixes, requiring the
/// trailing slash (so a bare host with nothing after it does not match) and
/// `https`. Guards against a host-spoofing attempt such as
/// `https://example.invalid/outlook.office.com/`, where the accepted text
/// appears but not as the scheme+host prefix.
#[must_use]
#[allow(dead_code)]
pub fn is_outlook_link(url: &str) -> bool {
    url.starts_with("https://outlook.office.com/")
        || url.starts_with("https://outlook.office365.com/")
}

/// How the signed-in Microsoft account is shown in the title bar (spec
/// §4.1). Name comes from the Graph identity resolved at sign-in; before
/// sign-in it shows "Not signed in".
///
/// No Graph identity is resolved yet, so the title bar uses the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountDisplay {
    pub name: String,
    pub initials: String,
    pub signed_in: bool,
}

impl Default for AccountDisplay {
    fn default() -> Self {
        Self {
            name: "Not signed in".into(),
            initials: String::new(),
            signed_in: false,
        }
    }
}

impl AccountDisplay {
    /// Builds the signed-in display for `display_name`: the initials are the
    /// uppercased first character of each of up to the first two
    /// whitespace-separated words (e.g. "Alex Rivera" -> "AR", "Alex" ->
    /// "A", "" -> "").
    #[allow(dead_code)]
    #[must_use]
    pub fn signed_in(display_name: &str) -> Self {
        let initials = display_name
            .split_whitespace()
            .take(2)
            .filter_map(|word| word.chars().next())
            .flat_map(char::to_uppercase)
            .collect();
        Self {
            name: display_name.to_string(),
            initials,
            signed_in: true,
        }
    }
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
    fn busy_indicator_advances_every_250_milliseconds_and_wraps() {
        assert_eq!(busy_indicator(0), "·");
        assert_eq!(busy_indicator(249), "·");
        assert_eq!(busy_indicator(250), "··");
        assert_eq!(busy_indicator(499), "··");
        assert_eq!(busy_indicator(500), "···");
        assert_eq!(busy_indicator(749), "···");
        assert_eq!(busy_indicator(750), "·");
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
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
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
        let mut app = AppModel::new();
        app.selected = "first-model".into();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::Models(Ok(vec![
                "deepseek-v4-flash:0731".into(),
                "other-model".into(),
            ])))
            .unwrap();
        app.poll(|| {});
        assert_eq!(app.selected, "deepseek-v4-flash:0731");
        assert!(!app.model_status.succeeded);
        assert!(app.pending.is_none());

        app.selected = "other-model".into();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender.send(Outcome::Generation(Ok(()))).unwrap();
        app.poll(|| {});
        assert!(app.model_status.succeeded);
        assert!(app.model_status.lines[0].contains("other-model"));
    }

    #[test]
    fn keys_are_trimmed_once_so_testing_and_scanning_use_the_same_value() {
        let mut app = AppModel::new();
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
        let mut app = AppModel::new();
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
        app.poll(|| {});
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
        let mut app = AppModel::new();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Microsoft;
        drop(sender);
        app.poll(|| {});
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

    #[test]
    fn is_outlook_link_accepts_only_the_two_prefixes() {
        assert!(is_outlook_link("https://outlook.office.com/mail/deeplink"));
        assert!(is_outlook_link(
            "https://outlook.office365.com/mail/deeplink"
        ));
        assert!(!is_outlook_link("https://outlook.office.com"));
        assert!(!is_outlook_link(
            "https://example.invalid/outlook.office.com/"
        ));
        assert!(!is_outlook_link("http://outlook.office.com/mail/deeplink"));
    }

    #[test]
    fn account_display_default_says_not_signed_in() {
        let display = AccountDisplay::default();
        assert_eq!(display.name, "Not signed in");
        assert_eq!(display.initials, "");
        assert!(!display.signed_in);
    }

    #[test]
    fn account_display_signed_in_derives_initials() {
        let display = AccountDisplay::signed_in("Alex Rivera");
        assert_eq!(display.name, "Alex Rivera");
        assert_eq!(display.initials, "AR");
        assert!(display.signed_in);

        let one_word = AccountDisplay::signed_in("Alex");
        assert_eq!(one_word.initials, "A");
        assert!(one_word.signed_in);

        let empty = AccountDisplay::signed_in("");
        assert_eq!(empty.initials, "");
        assert!(empty.signed_in);

        let extra_whitespace = AccountDisplay::signed_in("  Alex   Rivera  ");
        assert_eq!(extra_whitespace.name, "  Alex   Rivera  ");
        assert_eq!(extra_whitespace.initials, "AR");
        assert!(extra_whitespace.signed_in);
    }

    #[test]
    fn app_restores_settings_without_authentication_or_revealing_key_and_forgets_them() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        app.client_id = "00000000-0000-0000-0000-000000000000".into();
        app.groups = "one@example.invalid\ntwo@example.invalid\nthree@example.invalid".into();
        app.shared = "shared@example.invalid".into();
        app.key = Zeroizing::new("synthetic-key".into());
        app.selected = "deepseek-v4-flash:0731".into();
        app.openrouter_key = Zeroizing::new("synthetic-openrouter-key".into());
        app.openrouter_selected = "vendor/model-1".into();
        app.ollama_plan = OllamaPlan::Pro;
        app.openrouter_parallel = 48;
        app.pending_save = true;
        assert!(app.persist_changes());
        drop(app);

        let mut reopened = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        assert_eq!(reopened.groups.lines().count(), 3);
        assert_eq!(reopened.shared, "shared@example.invalid");
        assert_eq!(&*reopened.key, "synthetic-key");
        assert_eq!(reopened.selected, "deepseek-v4-flash:0731");
        assert_eq!(reopened.provider, Provider::OllamaCloud);
        assert_eq!(&*reopened.openrouter_key, "synthetic-openrouter-key");
        assert_eq!(reopened.openrouter_selected, "vendor/model-1");
        assert_eq!(reopened.ollama_plan, OllamaPlan::Pro);
        assert_eq!(reopened.openrouter_parallel, 48);
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
    fn start_and_poll_still_round_trip_through_a_plain_callback() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender.send(Outcome::Generation(Ok(()))).unwrap();
        app.poll(|| {});
        assert!(app.model_status.succeeded);
    }
}
