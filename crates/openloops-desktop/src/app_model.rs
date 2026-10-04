//! Toolkit-free setup/connection state: job start/poll machinery, saved
//! settings, and provider/account pure logic used by the native adapter.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

use crate::claim_view::LoopItem;
use crate::loop_state::Reminder;
use crate::review_model::{CardContext, ConversationKey, ReviewState};
use crate::settings::{
    ExtractionBackend, OllamaPlan, Provider, Settings, SettingsError, SettingsStore, max_parallel,
    production_store,
};
use openloops_graph::live::{
    AccountConfig, ConnectionConfig, ConnectionError, ConnectionReport, MailProvider, ProviderLoad,
    clear_all_sessions, clear_session_for,
    google::GoogleConfig,
    registration,
    review::{LoadProgress, MailCache, SourceReview},
};
use openloops_inference::decision::DecisionCheckReport;
use openloops_inference::ollama::suggested_model;
use openloops_inference::openrouter::ModelChoice;
use openloops_inference::provider::ProviderError;
use zeroize::Zeroizing;

pub(crate) enum Outcome {
    Connection(MailProvider, Result<ConnectionReport, ConnectionError>),
    Models(Result<Vec<String>, ProviderError>),
    ZdrModels(Result<Vec<ModelChoice>, ProviderError>),
    Generation(Result<(), ProviderError>),
    DecisionCheck(Result<DecisionCheckReport, ProviderError>),
    Mail(Vec<ProviderLoad>),
    CheckMail {
        loads: Vec<ProviderLoad>,
    },
    Scan(
        Result<crate::review_model::ScanResult, ProviderError>,
        String,
    ),
    RetryMail {
        loads: Vec<ProviderLoad>,
        source_keys: BTreeSet<(MailProvider, String)>,
        conversations: BTreeSet<ConversationKey>,
        attempted: usize,
    },
    RetryScan {
        result: Result<crate::review_model::ScanResult, ProviderError>,
        conversations: BTreeSet<ConversationKey>,
        attempted: usize,
    },
    CheckScan {
        result: Result<crate::review_model::ScanResult, ProviderError>,
        conversations: BTreeSet<ConversationKey>,
        new_messages: usize,
    },
    Reminder(
        [u8; 32],
        MailProvider,
        openloops_graph::live::reminders::ReminderOutcome,
    ),
    ReminderCompletion(
        [u8; 32],
        openloops_graph::live::reminders::ReminderCompletionOutcome,
    ),
    /// One `check_status` result per still-open, reminder-bearing decision
    /// found at the end of a scan -- see `AppModel::dispatch_reminder_sync`.
    ReminderSync(
        Vec<(
            [u8; 32],
            openloops_graph::live::reminders::TaskStatusOutcome,
        )>,
    ),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Service {
    Mailbox(MailProvider),
    Model,
    Review,
}

#[derive(Default)]
pub(crate) struct Status {
    pub(crate) lines: Vec<String>,
    pub(crate) succeeded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPresence {
    Missing,
    Present,
}

/// The training-export button's two-press confirm state -- an enum rather
/// than a plain `bool` field so it does not count against `AppModel`'s
/// `clippy::struct_excessive_bools` budget.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ExportArmed {
    #[default]
    No,
    Yes,
}

/// Toolkit-free setup/connection/review model used by the native UI adapter.
pub struct AppModel {
    pub client_id: String,
    pub google_client_id: String,
    pub google_client_secret: Zeroizing<String>,
    /// Providers included in scans (saved).
    pub mail_providers: BTreeSet<MailProvider>,
    /// The provider whose fields the Sources card shows. UI-only, never
    /// saved; `None` follows the first enabled provider.
    pub(crate) viewed_mail_provider: Option<MailProvider>,
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
    pub use_decision_model: bool,
    /// P5's gated primary-pass switch. See
    /// [`crate::settings::ExtractionBackend`].
    pub extraction_backend: ExtractionBackend,
    /// The owner-typed destination for a training-data export -- in-memory
    /// only, never part of [`Settings`]/[`AppModel::save_settings`]. See
    /// [`crate::training_export`].
    pub training_export_folder: String,
    pub(crate) training_export_status: Status,
    /// The export button's two-press confirm: `No` until the first click,
    /// which arms it and shows a warning instead of writing; disarmed by
    /// any further folder edit or by the write attempt itself.
    pub(crate) training_export_armed: ExportArmed,
    pub(crate) microsoft: Status,
    pub(crate) google: Status,
    pub(crate) model_status: Status,
    pub(crate) decision_status: Status,
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
    pub load_progress: Option<Arc<LoadProgress>>,
    pub scan_progress: Option<Arc<crate::review_model::ScanProgress>>,
    /// In-memory only and fully replaced after each successful load. One load is already bounded
    /// to at most 100 rows per folder and 10 configured sources, so no separate cache cap is needed.
    pub mail_cache: Arc<MailCache>,
    pub(crate) last_mail_check: Option<i64>,
    /// Whether the most recent [`AppModel::reload_settings`] call (including
    /// the one `with_store` runs at construction) found an existing saved
    /// record. The native adapter reads this immediately after each such call,
    /// together with [`AppModel::ready_for_review`], to reproduce today's
    /// initial-tab decision without the model owning any UI-nav state.
    pub(crate) settings_presence: SettingsPresence,
}

struct ScanJobInputs {
    key: String,
    model: String,
    provider: Provider,
    options: crate::review_model::ScanOptions,
    parallel: usize,
    messages: Vec<crate::review_model::ReviewMessage>,
    progress: Arc<crate::review_model::ScanProgress>,
}

/// Per-provider loads reduced to what a mail outcome handler acts on.
enum LoadSplit {
    /// The single provider's error, or a stop by the user.
    Failed(ConnectionError),
    /// Every one of several providers failed; one prefixed line each.
    AllFailed(Vec<String>),
    /// Sources from every provider that loaded.
    Loaded(Vec<SourceReview>),
}

fn cache_entries(
    sources: &[SourceReview],
) -> impl Iterator<Item = ((String, String), openloops_graph::live::review::MailItem)> + '_ {
    sources
        .iter()
        .flat_map(|source| &source.messages)
        .map(|message| {
            (
                (message.account.clone(), message.id.clone()),
                message.clone(),
            )
        })
}

fn shared_registration_active_for(byo_field: &str, shipped: Option<&str>) -> bool {
    byo_field.trim().is_empty() && shipped.is_some()
}

fn effective_google_registration_for(
    byo_client_id: &str,
    byo_client_secret: &str,
    shipped: Option<(&str, &str)>,
) -> Option<(String, Zeroizing<String>)> {
    let client_id = byo_client_id.trim();
    let client_secret = byo_client_secret.trim();
    if !client_id.is_empty() && !client_secret.is_empty() {
        Some((
            client_id.to_owned(),
            Zeroizing::new(client_secret.to_owned()),
        ))
    } else if client_id.is_empty() && client_secret.is_empty() {
        shipped.map(|(client_id, client_secret)| {
            (
                client_id.to_owned(),
                Zeroizing::new(client_secret.to_owned()),
            )
        })
    } else {
        None
    }
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
            google_client_id: String::new(),
            google_client_secret: Zeroizing::new(String::new()),
            mail_providers: BTreeSet::from([MailProvider::Microsoft]),
            viewed_mail_provider: None,
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
            use_decision_model: false,
            extraction_backend: ExtractionBackend::ChatModel,
            training_export_folder: String::new(),
            training_export_status: Status::default(),
            training_export_armed: ExportArmed::No,
            microsoft: Status::default(),
            google: Status::default(),
            model_status: Status::default(),
            decision_status: Status::default(),
            pending: None,
            pending_service: Service::Mailbox(MailProvider::Microsoft),
            progress: "",
            started: Instant::now(),
            store: None,
            settings_status: Status::default(),
            pending_save: false,
            automatic_save: true,
            review: ReviewState::default(),
            review_status: Status::default(),
            load_progress: None,
            scan_progress: None,
            mail_cache: Arc::new(MailCache::default()),
            last_mail_check: None,
            settings_presence: SettingsPresence::Missing,
        };
        match store {
            Ok(store) => {
                app.store = store;
                app.settings_presence = if app.reload_settings() {
                    SettingsPresence::Present
                } else {
                    SettingsPresence::Missing
                };
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
        self.clear_mail_cache();
        self.client_id = settings.client_id;
        self.google_client_id = settings.google_client_id;
        self.google_client_secret = settings.google_client_secret;
        self.mail_providers = settings.mail_providers;
        self.viewed_mail_provider = None;
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
        self.use_decision_model = settings.use_decision_model;
        self.extraction_backend = settings.extraction_backend;
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
        self.google = Status::default();
        self.model_status = Status::default();
        self.decision_status = Status::default();
        self.pending_save = false;
        self.review = ReviewState::default();
        self.review_status = Status::default();
    }

    /// Reloads settings from the store, if any, applying them the same way
    /// as today. Returns whether a saved record existed (`false` when there
    /// is no store, the load failed, or no record had been saved yet) --
    /// see [`AppModel::settings_presence`] for how the adapter uses this
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
                            "Saved settings restored. Sign in again to load mail."
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
            google_client_id: self.google_client_id.clone(),
            google_client_secret: self.google_client_secret.clone(),
            mail_providers: self.mail_providers.clone(),
            groups: self.groups.clone(),
            shared: self.shared.clone(),
            key: self.key.clone(),
            selected: self.selected.clone(),
            provider: self.provider,
            openrouter_key: self.openrouter_key.clone(),
            openrouter_selected: self.openrouter_selected.clone(),
            ollama_plan: self.ollama_plan,
            openrouter_parallel: self.openrouter_parallel,
            use_decision_model: self.use_decision_model,
            extraction_backend: self.extraction_backend,
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
        self.clear_mail_cache();
        clear_all_sessions();
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

    pub(crate) fn clear_mail_cache(&mut self) {
        if let Some(progress) = &self.load_progress {
            progress.cancel.store(true, Ordering::Release);
            self.pending = None;
        }
        self.load_progress = None;
        self.mail_cache = Arc::new(MailCache::default());
        self.last_mail_check = None;
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

    /// The pending job's worker went away without sending an outcome
    /// (a panic, in practice): clear the job and report it on the service
    /// that owned it.
    fn pending_disconnected(&mut self) {
        self.pending = None;
        let status = Status {
            lines: vec!["The operation stopped unexpectedly. Please try again.".into()],
            succeeded: false,
        };
        match self.pending_service {
            Service::Mailbox(provider) => *self.connection_status_mut(provider) = status,
            Service::Model => self.model_status = status,
            Service::Review => {
                self.load_progress = None;
                self.review_status = status;
                self.review.scan_failed = true;
            }
        }
    }

    /// Polls the pending job's channel, if any, applying its outcome.
    /// Returns whether an outcome was actually consumed (a result arrived,
    /// or the worker disconnected) -- `false` on an empty channel (still
    /// running) or when nothing is pending. The native adapter's busy-tick
    /// timer uses this to decide between a full `refresh` (something
    /// changed) and the lightweight `sync_busy` (nothing did).
    #[allow(clippy::too_many_lines)]
    pub fn poll(&mut self, respawn: impl Fn() + Send + Clone + 'static) -> bool {
        let Some(receiver) = &self.pending else {
            return false;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.pending_disconnected();
                return true;
            }
        };
        self.pending = None;
        match outcome {
            Outcome::Reminder(key, provider, outcome) => {
                self.reminder_outcome(key, provider, outcome);
            }
            Outcome::ReminderCompletion(key, outcome) => {
                self.reminder_completion_outcome(key, outcome);
            }
            Outcome::ReminderSync(results) => {
                self.reminder_sync_outcome(results);
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
            Outcome::DecisionCheck(Err(error)) => {
                self.decision_status = Status {
                    lines: vec![error.to_string()],
                    succeeded: false,
                };
            }
            Outcome::DecisionCheck(Ok(report)) => {
                self.decision_status = Status {
                    lines: vec![
                        format!("Decision model check passed in {} ms.", report.latency_ms),
                        if report.zdr_member_supported {
                            "provider.zdr accepted on the decisions endpoint."
                        } else {
                            "provider.zdr not accepted on the decisions endpoint; the model is verified against the zero-data-retention listing before every connection instead."
                        }
                        .into(),
                    ],
                    succeeded: true,
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
            Outcome::Connection(provider, result) => {
                *self.connection_status_mut(provider) = connection_report_status(provider, result);
            }
            Outcome::Mail(loads) => match self.split_loads(loads) {
                LoadSplit::Failed(error) => {
                    self.load_progress = None;
                    self.review.scan_failed = error != ConnectionError::Cancelled;
                    self.review_status = Status {
                        lines: vec![error.to_string()],
                        succeeded: false,
                    };
                }
                LoadSplit::AllFailed(lines) => {
                    self.load_progress = None;
                    self.review.scan_failed = true;
                    self.review_status = Status {
                        lines,
                        succeeded: false,
                    };
                }
                LoadSplit::Loaded(sources) => {
                    self.load_progress = None;
                    self.last_mail_check = Some(chrono::Utc::now().timestamp());
                    self.mail_cache = Arc::new(cache_entries(&sources).collect());
                    self.review = ReviewState::loaded(sources);
                    if self.review.messages.is_empty() {
                        self.review_status = Status { lines: vec!["No messages could be loaded for analysis. Check scan coverage and errors.".into()], succeeded: false };
                    } else {
                        self.start_scan(respawn.clone());
                    }
                }
            },
            Outcome::CheckMail { loads } => {
                self.check_mail_outcome(loads, respawn.clone());
            }
            Outcome::RetryMail {
                loads,
                mut source_keys,
                mut conversations,
                attempted,
            } => {
                self.load_progress = None;
                let loaded: BTreeSet<MailProvider> = loads
                    .iter()
                    .filter(|load| load.sources.is_ok())
                    .map(|load| load.provider)
                    .collect();
                match self.split_loads(loads) {
                    LoadSplit::Failed(ConnectionError::Cancelled) => {
                        self.finish_retry_status(attempted);
                        return true;
                    }
                    LoadSplit::Failed(_) | LoadSplit::AllFailed(_) => {}
                    LoadSplit::Loaded(sources) => {
                        Arc::make_mut(&mut self.mail_cache).extend(cache_entries(&sources));
                        source_keys.retain(|(provider, _)| loaded.contains(provider));
                        conversations.extend(self.review.replace_sources(&source_keys, sources));
                    }
                }
                if conversations.is_empty() {
                    self.finish_retry_status(attempted);
                } else {
                    self.start_retry_scan(conversations, attempted, respawn.clone());
                }
            }
            Outcome::Scan(result, model) => {
                self.scan_progress = None;
                match result {
                    Ok(scan) => {
                        self.review.set_scan(scan, model);
                        self.review_status = Status { lines: vec![if self.review.scan_incomplete { "Scan incomplete. Results from completed batches are shown below; check scan coverage and errors." } else { "Scan finished. Review the open loops and their evidence below." }.into()], succeeded: !self.review.scan_incomplete };
                        self.dispatch_reminder_sync();
                    }
                    Err(error) => {
                        self.review.scan_failed = true;
                        self.review_status = Status {
                            lines: vec![error.to_string()],
                            succeeded: false,
                        }
                    }
                }
            }
            Outcome::RetryScan {
                result,
                conversations,
                attempted,
            } => {
                self.scan_progress = None;
                if let Ok(scan) = result {
                    self.review.merge_scan(scan, &conversations);
                }
                self.finish_retry_status(attempted);
            }
            Outcome::CheckScan {
                result,
                conversations,
                new_messages,
            } => {
                self.scan_progress = None;
                match result {
                    Ok(scan) => {
                        let loops = scan.analysis.items.len();
                        let updates = self.review.merge_scan(scan, &conversations);
                        self.finish_check_status(new_messages, conversations.len(), loops, updates);
                        self.dispatch_reminder_sync();
                    }
                    Err(error) => {
                        self.review.scan_failed = true;
                        self.review_status = Status {
                            lines: vec![error.to_string()],
                            succeeded: false,
                        };
                    }
                }
            }
        }
        true
    }

    /// Splits per-provider loads. One load keeps today's single-provider
    /// handling. With several, each failed provider gets one status line
    /// prefixed with its service name, and the other providers' sources are
    /// kept. A stop by the user cancels the whole job.
    fn split_loads(&mut self, loads: Vec<ProviderLoad>) -> LoadSplit {
        if loads
            .iter()
            .any(|load| matches!(load.sources, Err(ConnectionError::Cancelled)))
        {
            return LoadSplit::Failed(ConnectionError::Cancelled);
        }
        for load in &loads {
            if load.sources.is_ok() {
                let prefix = format!("{}: ", load.provider.service_name());
                self.connection_status_mut(load.provider)
                    .lines
                    .retain(|line| !line.starts_with(&prefix));
            }
        }
        if loads.len() == 1 {
            return match loads.into_iter().next().map(|load| load.sources) {
                Some(Err(error)) => LoadSplit::Failed(error),
                Some(Ok(sources)) => LoadSplit::Loaded(sources),
                None => LoadSplit::Loaded(Vec::new()),
            };
        }
        let mut sources = Vec::new();
        let mut lines = Vec::new();
        let mut any_loaded = loads.is_empty();
        for load in loads {
            match load.sources {
                Ok(loaded) => {
                    any_loaded = true;
                    sources.extend(loaded);
                }
                Err(error) => {
                    let line = format!("{}: {error}", load.provider.service_name());
                    *self.connection_status_mut(load.provider) = Status {
                        lines: vec![line.clone()],
                        succeeded: false,
                    };
                    lines.push(line);
                }
            }
        }
        if any_loaded {
            LoadSplit::Loaded(sources)
        } else {
            LoadSplit::AllFailed(lines)
        }
    }

    fn check_mail_outcome(
        &mut self,
        loads: Vec<ProviderLoad>,
        respawn: impl FnOnce() + Send + 'static,
    ) {
        self.load_progress = None;
        let previous_check = self.last_mail_check;
        self.last_mail_check = Some(chrono::Utc::now().timestamp());
        match self.split_loads(loads) {
            LoadSplit::Loaded(sources) => {
                Arc::make_mut(&mut self.mail_cache).extend(cache_entries(&sources));
                let known: BTreeSet<String> = self
                    .review
                    .messages
                    .iter()
                    .map(|message| message.input.handle.clone())
                    .collect();
                let changed = self.review.append_sources(sources);
                let new_messages: BTreeSet<String> = self
                    .review
                    .messages
                    .iter()
                    .map(|message| message.input.handle.clone())
                    .filter(|handle| !known.contains(handle))
                    .collect();
                if changed.is_empty() {
                    let local = chrono::DateTime::from_timestamp(
                        previous_check.unwrap_or_else(|| chrono::Utc::now().timestamp()),
                        0,
                    )
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local);
                    self.review_status = Status {
                        lines: vec![format!("No new mail since {}.", local.format("%H:%M"))],
                        succeeded: true,
                    };
                } else {
                    let prior = self.review.prior_open_items(&changed);
                    self.start_check_scan(changed, new_messages, prior, respawn);
                }
            }
            LoadSplit::Failed(ConnectionError::Cancelled) => {
                self.review_status = Status {
                    lines: vec!["Check stopped; no new mail was added.".into()],
                    succeeded: false,
                };
            }
            LoadSplit::Failed(error) => {
                self.review_status = Status {
                    lines: vec![error.to_string()],
                    succeeded: false,
                };
            }
            LoadSplit::AllFailed(lines) => {
                self.review_status = Status {
                    lines,
                    succeeded: false,
                };
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
        provider: MailProvider,
        outcome: openloops_graph::live::reminders::ReminderOutcome,
    ) {
        use crate::loop_state::{Reminder, now};
        use openloops_graph::live::reminders::ReminderOutcome;
        let mut record = self.review.decisions.get(&key);
        let (text, outcome_succeeded)=match outcome {
            ReminderOutcome::Created{provider,list_id,task_id}=>{record.reminder=Reminder::Created{provider,list_id,task_id}; (match provider { MailProvider::Microsoft => "Reminder created in your Microsoft To Do Tasks list.".to_owned(), MailProvider::Google => "Reminder created in your Google Tasks default list.".to_owned() }, true)},
            ReminderOutcome::NotCreated(reason)=>{record.reminder=Reminder::None;(format!("No reminder was created: {}", reason.describe(provider)), false)},
            ReminderOutcome::Uncertain=>(match provider { MailProvider::Microsoft => "Microsoft did not confirm the write. Check To Do before trying again; OpenLoops will not automatically retry.".to_owned(), MailProvider::Google => "Google did not confirm the write. Check Google Tasks before trying again; OpenLoops will not automatically retry.".to_owned() }, false),
        };
        record.updated = now();
        let saved = self.review.decisions.update(record);
        self.review.action_status_succeeded = outcome_succeeded && saved.is_ok();
        self.review.action_status = match saved {
            Ok(()) => text,
            Err(error) => format!("{text} {error}"),
        };
    }

    /// Reports whether the already-Handled card's linked task was also
    /// marked complete. Never reverts the local `Done` decision or the
    /// `Reminder::Created` record on failure -- see `reminders::complete()`'s
    /// doc comment -- so this only ever updates the status line, distinct
    /// from `reminder_outcome`, which can revert a reminder to `None`.
    fn reminder_completion_outcome(
        &mut self,
        key: [u8; 32],
        outcome: openloops_graph::live::reminders::ReminderCompletionOutcome,
    ) {
        use openloops_graph::live::reminders::ReminderCompletionOutcome;
        let provider = match self.review.decisions.get(&key).reminder {
            Reminder::Created { provider, .. } | Reminder::Completed { provider, .. } => provider,
            Reminder::None | Reminder::Attempted => MailProvider::Microsoft,
        };
        self.review.action_status = match outcome {
            ReminderCompletionOutcome::Completed => {
                format!(
                    "Decision saved on this Windows account. The linked {} task was also marked complete.",
                    provider.tasks_name()
                )
            }
            ReminderCompletionOutcome::NotCompleted(error) => {
                format!(
                    "Decision saved on this Windows account. The linked {} task was not marked complete: {error}",
                    provider.tasks_name()
                )
            }
            ReminderCompletionOutcome::Uncertain => match provider {
                MailProvider::Microsoft => {
                    "Decision saved on this Windows account. Microsoft did not confirm the To Do task was marked complete; check it there.".to_owned()
                }
                MailProvider::Google => {
                    "Decision saved on this Windows account. Google did not confirm the Google Tasks task was marked complete; check it there.".to_owned()
                }
            },
        };
        self.review.action_status_succeeded =
            matches!(outcome, ReminderCompletionOutcome::Completed);
    }

    /// Applies the reverse direction of `reminder_completion_outcome`: a
    /// task the user completed directly in Microsoft To Do or Google Tasks,
    /// discovered by
    /// `dispatch_reminder_sync`, marks its loop Handled in `OpenLoops`.
    /// `NotCompleted`/`Unknown` results change nothing -- a task that's
    /// still open, or one `OpenLoops` simply couldn't check this time, is not
    /// evidence of anything; `Reminder::Completed` (rather than leaving it
    /// `Created`) is what lets `status_pill_hint` say this loop was closed
    /// by the linked task specifically, not by the user clicking Handled.
    fn reminder_sync_outcome(
        &mut self,
        results: Vec<(
            [u8; 32],
            openloops_graph::live::reminders::TaskStatusOutcome,
        )>,
    ) {
        use crate::loop_state::{Decision, Reminder};
        use openloops_graph::live::reminders::TaskStatusOutcome;
        let mut completed = 0usize;
        let mut providers = BTreeSet::new();
        for (key, outcome) in results {
            if !matches!(outcome, TaskStatusOutcome::Completed) {
                continue;
            }
            let mut record = self.review.decisions.get(&key);
            let Reminder::Created {
                provider,
                list_id,
                task_id,
            } = record.reminder.clone()
            else {
                continue;
            };
            record.decision = Decision::Done;
            record.reminder = Reminder::Completed {
                provider,
                list_id,
                task_id,
            };
            record.updated = crate::loop_state::now();
            if self.review.decisions.update(record).is_ok() {
                completed += 1;
                providers.insert(provider);
            }
        }
        if completed > 0 {
            let linked = match (providers.len(), providers.first()) {
                (1, Some(provider)) => format!("linked {} task", provider.tasks_name()),
                _ => "linked task".to_owned(),
            };
            self.review.scan_errors.push(format!(
                "{completed} loop(s) marked handled because their {linked} was already completed."
            ));
        }
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
            Provider::OpenRouter
                if self.use_decision_model
                    && self.extraction_backend == ExtractionBackend::DecisionModel =>
            {
                "OpenRouter, restricted to zero-data-retention endpoints, including the Jev decision model for closure checks, triage, and finding open loops"
            }
            Provider::OpenRouter if self.use_decision_model => {
                "OpenRouter, restricted to zero-data-retention endpoints, including the Jev decision model"
            }
            Provider::OpenRouter => "OpenRouter, restricted to zero-data-retention endpoints",
        }
    }

    #[must_use]
    pub fn can_check_decision_model(&self) -> bool {
        self.provider == Provider::OpenRouter
            && !self.openrouter_key.is_empty()
            && self.use_decision_model
    }

    /// Whether the "Find loops with" segmented control may be used: P5's
    /// switch is gated on the decision model toggle, same rule as "Check
    /// decision model".
    #[must_use]
    pub fn can_select_extraction_backend(&self) -> bool {
        self.provider == Provider::OpenRouter && self.use_decision_model
    }

    /// Whether "Export training data" may be pressed: a scan result is
    /// loaded and the owner has typed a destination folder.
    #[must_use]
    pub fn can_export_training(&self) -> bool {
        self.review.analysis.is_some() && !self.training_export_folder.trim().is_empty()
    }

    /// Whether the client ID, active key, and selected model are all
    /// non-empty -- the same three checks that decided today's post-reload
    /// active tab. Pure so the adapter can reproduce that decision without
    /// the model owning any UI-nav state.
    #[must_use]
    pub fn ready_for_review(&self) -> bool {
        self.enabled_mail_providers()
            .into_iter()
            .any(|provider| self.registration_present(provider))
            && !self.active_key().is_empty()
            && !self.selected_model().is_empty()
    }

    fn registration_present(&self, provider: MailProvider) -> bool {
        match provider {
            MailProvider::Microsoft => self.effective_microsoft_client_id().is_some(),
            MailProvider::Google => self.effective_google_registration().is_some(),
        }
    }

    /// The providers included in scans, in [`MailProvider::ALL`] order.
    #[must_use]
    pub fn enabled_mail_providers(&self) -> Vec<MailProvider> {
        MailProvider::ALL
            .into_iter()
            .filter(|provider| self.mail_providers.contains(provider))
            .collect()
    }

    /// Includes or excludes `provider` from scans and saves the choice.
    /// Disabling clears that provider's session, status and mail only.
    /// Refuses to disable the last enabled provider; returns whether the
    /// change was applied.
    pub fn set_provider_enabled(&mut self, provider: MailProvider, enabled: bool) -> bool {
        if enabled {
            self.settings_status = Status::default();
            if self.mail_providers.insert(provider) {
                self.pending_save = true;
                let _ = self.persist_changes();
            }
            return true;
        }
        if !self.mail_providers.contains(&provider) {
            return true;
        }
        if self.mail_providers.len() == 1 {
            self.settings_status = Status {
                lines: vec!["Keep at least one mail provider enabled.".into()],
                succeeded: false,
            };
            return false;
        }
        self.settings_status = Status::default();
        self.mail_providers.remove(&provider);
        clear_session_for(provider);
        *self.connection_status_mut(provider) = Status::default();
        self.review.remove_provider(provider);
        Arc::make_mut(&mut self.mail_cache).retain(|_, message| message.provider != provider);
        self.pending_save = true;
        let _ = self.persist_changes();
        true
    }

    /// The provider whose fields the Sources card shows: the user's pick,
    /// else the first enabled provider. Sources card only; the Review pane
    /// uses [`AppModel::primary_mail_provider`].
    #[must_use]
    pub fn active_mail_provider(&self) -> MailProvider {
        self.viewed_mail_provider
            .unwrap_or_else(|| self.primary_mail_provider())
    }

    /// The first enabled provider. The Review pane's default when no card
    /// names a provider.
    #[must_use]
    pub fn primary_mail_provider(&self) -> MailProvider {
        self.enabled_mail_providers()
            .first()
            .copied()
            .unwrap_or(MailProvider::Microsoft)
    }

    /// Shows `provider`'s fields on the Sources card. Changes no enablement.
    pub fn set_active_mail_provider(&mut self, provider: MailProvider) {
        self.viewed_mail_provider = Some(provider);
    }

    /// One configuration per enabled provider. A provider with a missing or
    /// invalid registration is skipped and gets a status line, unless no
    /// provider is usable.
    ///
    /// # Errors
    /// Returns the first provider's error when no enabled provider is usable.
    pub fn enabled_accounts(&mut self) -> Result<Vec<AccountConfig>, ConnectionError> {
        let mut accounts = Vec::new();
        let mut failures = Vec::new();
        for provider in self.enabled_mail_providers() {
            match self.account_config(provider) {
                Ok(account) => accounts.push(account),
                Err(error) => failures.push((provider, error)),
            }
        }
        if accounts.is_empty() {
            return Err(failures
                .into_iter()
                .next()
                .map_or(ConnectionError::InvalidConfiguration, |(_, error)| error));
        }
        for (provider, error) in failures {
            *self.connection_status_mut(provider) = Status {
                lines: vec![format!("{}: {error}", provider.service_name())],
                succeeded: false,
            };
        }
        Ok(accounts)
    }

    /// The retry job for each provider with failed sources: its
    /// configuration and the labels to load again. A provider with a missing
    /// or invalid registration is skipped and gets a status line, unless no
    /// provider is usable.
    ///
    /// # Errors
    /// Returns the first provider's configuration error when no provider is
    /// usable.
    pub fn retry_jobs(
        &mut self,
        failed_sources: &BTreeSet<(MailProvider, String)>,
    ) -> Result<Vec<(AccountConfig, BTreeSet<String>)>, ConnectionError> {
        let mut grouped = BTreeMap::<MailProvider, BTreeSet<String>>::new();
        for (provider, label) in failed_sources {
            grouped.entry(*provider).or_default().insert(label.clone());
        }
        let mut jobs = Vec::new();
        let mut failures = Vec::new();
        for (provider, labels) in grouped {
            match self.account_config(provider) {
                Ok(account) => jobs.push((account, labels)),
                Err(error) => failures.push((provider, error)),
            }
        }
        if jobs.is_empty() && !failures.is_empty() {
            return Err(failures
                .into_iter()
                .next()
                .map_or(ConnectionError::InvalidConfiguration, |(_, error)| error));
        }
        for (provider, error) in failures {
            *self.connection_status_mut(provider) = Status {
                lines: vec![format!("{}: {error}", provider.service_name())],
                succeeded: false,
            };
        }
        Ok(jobs)
    }

    #[must_use]
    pub fn effective_microsoft_client_id(&self) -> Option<String> {
        registration::effective_client_id(
            &self.client_id,
            registration::microsoft().map(|value| value.client_id),
        )
    }

    #[must_use]
    pub fn shared_registration_active(&self) -> bool {
        shared_registration_active_for(
            &self.client_id,
            registration::microsoft().map(|value| value.client_id),
        )
    }

    #[must_use]
    pub fn effective_google_registration(&self) -> Option<(String, Zeroizing<String>)> {
        effective_google_registration_for(
            &self.google_client_id,
            &self.google_client_secret,
            registration::google().map(|value| (value.client_id, value.client_secret)),
        )
    }

    /// Builds the selected provider's validated runtime configuration.
    ///
    /// # Errors
    /// Returns [`ConnectionError::InvalidConfiguration`] when required
    /// registration values are missing or malformed.
    pub fn account_config(&self, provider: MailProvider) -> Result<AccountConfig, ConnectionError> {
        match provider {
            MailProvider::Microsoft => {
                let client_id = self
                    .effective_microsoft_client_id()
                    .ok_or(ConnectionError::InvalidConfiguration)?;
                ConnectionConfig::new(&client_id, Some(&self.shared))
                    .and_then(|config| config.with_groups(Some(&self.groups)))
                    .map(AccountConfig::Microsoft)
            }
            MailProvider::Google => {
                let (client_id, client_secret) = self
                    .effective_google_registration()
                    .ok_or(ConnectionError::InvalidConfiguration)?;
                GoogleConfig::new(&client_id, &client_secret).map(AccountConfig::Google)
            }
        }
    }

    /// Builds the configuration for a reminder write or check. Microsoft
    /// reminders use the personal To Do list only, so the configuration has
    /// no shared mailbox and no groups (those inputs are for mail loading).
    ///
    /// # Errors
    /// Returns [`ConnectionError::InvalidConfiguration`] when required
    /// registration values are missing or malformed.
    pub fn reminder_account_config(
        &self,
        provider: MailProvider,
    ) -> Result<AccountConfig, ConnectionError> {
        match provider {
            MailProvider::Microsoft => {
                let client_id = self
                    .effective_microsoft_client_id()
                    .ok_or(ConnectionError::InvalidConfiguration)?;
                ConnectionConfig::new(&client_id, None).map(AccountConfig::Microsoft)
            }
            MailProvider::Google => self.account_config(MailProvider::Google),
        }
    }

    #[must_use]
    pub fn admin_consent_url(&self) -> Option<String> {
        self.effective_microsoft_client_id()
            .map(|client_id| registration::microsoft_admin_consent_url(&client_id))
    }

    #[must_use]
    pub(crate) const fn connection_status(&self, provider: MailProvider) -> &Status {
        match provider {
            MailProvider::Microsoft => &self.microsoft,
            MailProvider::Google => &self.google,
        }
    }

    pub(crate) fn connection_status_mut(&mut self, provider: MailProvider) -> &mut Status {
        match provider {
            MailProvider::Microsoft => &mut self.microsoft,
            MailProvider::Google => &mut self.google,
        }
    }

    pub(crate) fn start_scan(&mut self, on_done: impl FnOnce() + Send + 'static) {
        let inputs = self.scan_job_inputs();
        inputs
            .progress
            .total
            .store(inputs.messages.len(), Ordering::Relaxed);
        self.review.analysis = None;
        self.review.scan_summary.clear();
        self.review.scan_errors.clear();
        self.review_status = Status::default();
        self.start(
            Service::Review,
            match inputs.provider {
                Provider::OllamaCloud => "Finding open loops with Ollama Cloud",
                Provider::OpenRouter => "Finding open loops with OpenRouter",
            },
            move || {
                Outcome::Scan(
                    crate::review_model::scan(
                        inputs.options,
                        inputs.key,
                        &inputs.model,
                        inputs.parallel,
                        &inputs.messages,
                        &inputs.progress,
                        crate::review_model::ScanScope::Full,
                    ),
                    inputs.model,
                )
            },
            on_done,
        );
    }

    /// After a scan, checks whether any still-open, reminder-bearing
    /// decision's linked Microsoft To Do or Google Tasks task was completed outside
    /// `OpenLoops` -- the reverse direction of `on_review_decision`'s
    /// complete-on-Handled hook (`slint_review.rs`). Runs after
    /// `Outcome::Scan` rather than before the mail download, because
    /// matching a saved decision back to an account needs the freshly
    /// loaded messages: `Decisions` never stores the account itself, only
    /// folds it into the opaque fingerprint (see `loop_state.rs`'s module
    /// doc), so `card_context()`'s live `source_message` lookup is the only
    /// way to recover it. A no-op when nothing is eligible: only a
    /// `Mine`/`Watching`/`Review` decision with a `Reminder::Created`
    /// record carrying real (non-empty) ids has anything left to learn from
    /// its provider.
    pub(crate) fn dispatch_reminder_sync(&mut self) {
        let jobs = self.reminder_sync_jobs();
        if jobs.is_empty() {
            return;
        }
        let providers = jobs
            .iter()
            .map(|(config, _)| config.provider())
            .collect::<BTreeSet<_>>();
        self.start(
            Service::Review,
            reminder_sync_job_label(&providers),
            move || {
                let results = jobs
                    .into_iter()
                    .flat_map(|(config, checks)| {
                        checks
                            .into_iter()
                            .map(move |(account, key, list_id, task_id)| {
                                (
                                    key,
                                    openloops_graph::live::provider::reminder_status(
                                        &config, &account, &list_id, &task_id,
                                    ),
                                )
                            })
                    })
                    .collect();
                Outcome::ReminderSync(results)
            },
            || {},
        );
    }

    pub(crate) fn start_retry_scan(
        &mut self,
        conversations: BTreeSet<ConversationKey>,
        attempted: usize,
        on_done: impl FnOnce() + Send + 'static,
    ) {
        let inputs = self.scan_job_inputs();
        self.review_status = Status::default();
        self.start(
            Service::Review,
            match inputs.provider {
                Provider::OllamaCloud => "Finding open loops with Ollama Cloud",
                Provider::OpenRouter => "Finding open loops with OpenRouter",
            },
            move || Outcome::RetryScan {
                result: crate::review_model::scan(
                    inputs.options,
                    inputs.key,
                    &inputs.model,
                    inputs.parallel,
                    &inputs.messages,
                    &inputs.progress,
                    crate::review_model::ScanScope::Conversations(&conversations),
                ),
                conversations,
                attempted,
            },
            on_done,
        );
    }

    fn scan_job_inputs(&mut self) -> ScanJobInputs {
        let progress = Arc::new(crate::review_model::ScanProgress::default());
        self.scan_progress = Some(progress.clone());
        ScanJobInputs {
            key: self.active_key().to_string(),
            model: self.selected_model().to_owned(),
            provider: self.provider,
            options: crate::review_model::ScanOptions {
                provider: self.provider,
                use_decision_model: self.use_decision_model,
                extraction_backend: self.extraction_backend,
            },
            parallel: self.max_parallel(),
            messages: self.review.messages.clone(),
            progress,
        }
    }

    pub(crate) fn start_check_scan(
        &mut self,
        conversations: BTreeSet<ConversationKey>,
        new_messages: BTreeSet<String>,
        prior_items: Vec<LoopItem>,
        on_done: impl FnOnce() + Send + 'static,
    ) {
        let inputs = self.scan_job_inputs();
        let new_message_count = new_messages.len();
        self.review_status = Status::default();
        self.start(
            Service::Review,
            "Checking for new mail",
            move || Outcome::CheckScan {
                result: crate::review_model::scan(
                    inputs.options,
                    inputs.key,
                    &inputs.model,
                    inputs.parallel,
                    &inputs.messages,
                    &inputs.progress,
                    crate::review_model::ScanScope::Incremental(
                        crate::review_model::IncrementalScope {
                            conversations: &conversations,
                            new_messages: &new_messages,
                            prior_items,
                        },
                    ),
                ),
                conversations,
                new_messages: new_message_count,
            },
            on_done,
        );
    }

    fn finish_retry_status(&mut self, attempted: usize) {
        let remaining = self.review.retryable_failures();
        self.review_status = Status {
            lines: vec![format!("Retried {attempted}; {remaining} still failing.")],
            succeeded: remaining == 0,
        };
    }

    fn finish_check_status(
        &mut self,
        new_messages: usize,
        conversations: usize,
        loops: usize,
        updates: usize,
    ) {
        let mut line = format!(
            "{new_messages} new messages in {conversations} conversations; {loops} loops added, {updates} updates suggested."
        );
        if self.review.scan_summary.ends_with("; stopped by you") {
            line.push_str("; stopped by you");
        }
        self.review_status = Status {
            lines: vec![line],
            succeeded: !self.review.scan_incomplete,
        };
    }

    /// The reminder status checks `dispatch_reminder_sync` would run, grouped
    /// by provider (Microsoft first), each with its reminder configuration.
    /// A provider whose configuration is missing or invalid is left out.
    pub(crate) fn reminder_sync_jobs(&self) -> Vec<(AccountConfig, Vec<ReminderSyncCheck>)> {
        let Some(analysis) = &self.review.analysis else {
            return Vec::new();
        };
        let items = &analysis.items;
        let cards = self.review.card_contexts(items);
        let mut grouped = BTreeMap::<MailProvider, Vec<_>>::new();
        for (provider, account, key, list_id, task_id) in
            self.review.reminder_sync_checks(items, &cards)
        {
            grouped
                .entry(provider)
                .or_default()
                .push((account, key, list_id, task_id));
        }
        grouped
            .into_iter()
            .filter_map(|(provider, checks)| {
                self.reminder_account_config(provider)
                    .ok()
                    .map(|config| (config, checks))
            })
            .collect()
    }

    /// The providers of the reminder records a sync-button click would check.
    #[must_use]
    pub(crate) fn reminder_sync_providers(
        &self,
        cards: &[Option<CardContext>],
    ) -> BTreeSet<MailProvider> {
        self.review
            .analysis
            .as_ref()
            .map_or_else(BTreeSet::new, |analysis| {
                self.review
                    .reminder_sync_checks(&analysis.items, cards)
                    .into_iter()
                    .map(|(provider, ..)| provider)
                    .collect()
            })
    }

    /// How many still-open, reminder-bearing decisions a sync-button click
    /// would check right now, given `cards` the caller already computed
    /// (typically the same `card_contexts` result a `sync`/`sync_review`
    /// pass already built) -- see `ReviewState::reminder_sync_checks`.
    /// Drives the Review toolbar button's enabled state and its "Checking N
    /// ... task(s)..." status line. Never calls `card_contexts` itself.
    #[must_use]
    pub(crate) fn reminder_sync_eligible_count(&self, cards: &[Option<CardContext>]) -> usize {
        self.review.analysis.as_ref().map_or(0, |analysis| {
            self.review
                .reminder_sync_checks(&analysis.items, cards)
                .len()
        })
    }
}

impl Default for AppModel {
    fn default() -> Self {
        Self::new()
    }
}

/// One reminder status check: account, decision key, list id, task id.
pub(crate) type ReminderSyncCheck = (String, [u8; 32], String, String);

/// The progress label for a reminder sync over `providers`.
pub(crate) fn reminder_sync_job_label(providers: &BTreeSet<MailProvider>) -> &'static str {
    match (
        providers.contains(&MailProvider::Microsoft),
        providers.contains(&MailProvider::Google),
    ) {
        (true, true) => "Checking reminders for completed tasks",
        (false, true) => "Checking Google Tasks for completed reminders",
        _ => "Checking Microsoft To Do for completed reminders",
    }
}

pub(crate) fn connection_report_status(
    provider: MailProvider,
    result: Result<ConnectionReport, ConnectionError>,
) -> Status {
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
            if provider == MailProvider::Microsoft {
                for (kind, results) in [
                    ("Group inbox", report.group_inbox_results),
                    ("Shared mailbox", report.shared_inbox_results),
                ] {
                    for (index, result) in results.into_iter().enumerate() {
                        match result {
                            Ok(()) => {
                                lines.push(format!("{kind} {}: access confirmed.", index + 1));
                            }
                            Err(error) => {
                                lines.push(format!("{kind} {}: {error}", index + 1));
                                succeeded = false;
                            }
                        }
                    }
                }
            }
            Status { lines, succeeded }
        }
    }
}

/// Whether `url` is a message link from any supported mail provider.
/// Keep this UI gate aligned with `provider.rs::is_trusted_message_link`.
#[must_use]
pub fn is_trusted_message_link(url: &str) -> bool {
    MailProvider::ALL
        .iter()
        .any(|provider| provider.is_trusted_message_link(url))
}

/// How connected mail providers are shown in the title bar.
///
/// Owner feedback item 3: the Graph layer exposes no display name yet, so
/// there is no avatar or account name to show -- only a stable-order,
/// per-provider connection summary once each provider's connection has
/// succeeded or its review messages have already loaded, and nothing otherwise
/// (never a misleading "Not signed in" while mail is actually being
/// scanned). [`AccountDisplay::connected`] builds that display.
/// [`AccountDisplay::signed_in`] is kept for when the Graph layer exposes a
/// real display name.
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
    /// Builds a stable-order summary for exactly the connected providers.
    #[must_use]
    pub fn connected(providers: &[MailProvider]) -> Self {
        let services = MailProvider::ALL
            .iter()
            .filter(|provider| providers.contains(provider))
            .map(|provider| provider.service_name())
            .collect::<Vec<_>>()
            .join(" + ");
        Self {
            name: if services.is_empty() {
                String::new()
            } else {
                format!("Signed in · {services}")
            },
            initials: String::new(),
            signed_in: !providers.is_empty(),
        }
    }

    /// Builds the signed-in display for `display_name`: the initials are the
    /// uppercased first character of each of up to the first two
    /// whitespace-separated words (e.g. "Alex Rivera" -> "AR", "Alex" ->
    /// "A", "" -> "").
    ///
    /// Not wired into the native UI yet -- kept ready for when the Graph
    /// layer exposes a real display name. Production code has no caller
    /// until that wiring lands, so this would otherwise trip `dead_code`
    /// outside `#[cfg(test)]` builds.
    #[must_use]
    #[allow(dead_code)]
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
    use openloops_graph::live::{SharedScope, review::MailItem};
    use openloops_inference::decision::DecisionCheckReport;
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

    fn check_source(messages: Vec<MailItem>) -> SourceReview {
        SourceReview {
            provider: openloops_graph::live::MailProvider::Microsoft,
            label: "Inbox".into(),
            messages,
            errors: vec![],
            message_errors: vec![],
            partial: false,
            failed: false,
        }
    }

    fn check_scan_result(items: Vec<LoopItem>) -> crate::review_model::ScanResult {
        crate::review_model::ScanResult {
            analysis: crate::claim_view::LoopItems {
                items,
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            prior_updates: vec![],
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: std::collections::BTreeMap::default(),
            conversation_notes_by_id: std::collections::BTreeMap::default(),
            conversation_rejection_reasons: std::collections::BTreeMap::default(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        }
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
    fn decision_check_ok_and_err_set_decision_status() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::DecisionCheck(Ok(DecisionCheckReport {
                zdr_member_supported: false,
                latency_ms: 17,
            })))
            .unwrap();
        app.poll(|| {});
        assert!(app.decision_status.succeeded);
        assert_eq!(app.decision_status.lines.len(), 2);
        assert!(app.decision_status.lines[0].contains("17 ms"));
        assert!(app.decision_status.lines[1].contains("not accepted"));

        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::DecisionCheck(Err(ProviderError::InvalidResponse)))
            .unwrap();
        app.poll(|| {});
        assert!(!app.decision_status.succeeded);
        assert_eq!(
            app.decision_status.lines,
            [ProviderError::InvalidResponse.to_string()]
        );
    }

    #[test]
    fn provider_disclosure_names_the_decision_model_only_when_enabled() {
        let mut app = AppModel::with_store(Ok(None));
        app.provider = Provider::OpenRouter;
        assert!(!app.provider_disclosure().contains("Jev"));
        app.use_decision_model = true;
        assert!(app.provider_disclosure().contains("Jev decision model"));
        app.provider = Provider::OllamaCloud;
        assert!(!app.provider_disclosure().contains("Jev"));
    }

    #[test]
    fn provider_disclosure_names_finding_open_loops_only_when_the_extraction_switch_is_on() {
        let mut app = AppModel::with_store(Ok(None));
        app.provider = Provider::OpenRouter;
        app.use_decision_model = true;
        assert!(!app.provider_disclosure().contains("finding open loops"));
        app.extraction_backend = ExtractionBackend::DecisionModel;
        assert!(app.provider_disclosure().contains("finding open loops"));
        // Turning the decision model itself off must remove the mention
        // even though the extraction switch is still set to DecisionModel.
        app.use_decision_model = false;
        assert!(!app.provider_disclosure().contains("finding open loops"));
    }

    #[test]
    fn extraction_backend_is_selectable_only_with_openrouter_and_the_decision_model_on() {
        let mut app = AppModel::with_store(Ok(None));
        assert!(!app.can_select_extraction_backend());
        app.provider = Provider::OpenRouter;
        assert!(!app.can_select_extraction_backend());
        app.use_decision_model = true;
        assert!(app.can_select_extraction_backend());
        app.provider = Provider::OllamaCloud;
        assert!(!app.can_select_extraction_backend());
    }

    #[test]
    fn apply_settings_round_trips_extraction_backend() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        app.extraction_backend = ExtractionBackend::DecisionModel;
        app.save_settings();
        let reopened = AppModel::with_store(Ok(Some(Box::new(memory))));
        assert_eq!(
            reopened.extraction_backend,
            ExtractionBackend::DecisionModel
        );
    }

    #[test]
    fn apply_settings_round_trips_use_decision_model() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        app.use_decision_model = true;
        app.save_settings();
        let reopened = AppModel::with_store(Ok(Some(Box::new(memory))));
        assert!(reopened.use_decision_model);
    }

    #[test]
    fn apply_settings_round_trips_google_mail_settings() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        app.google_client_id = "123-fixture.apps.googleusercontent.com".into();
        app.google_client_secret = Zeroizing::new("fixture-secret".into());
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft, MailProvider::Google]);
        app.save_settings();

        let reopened = AppModel::with_store(Ok(Some(Box::new(memory))));
        assert_eq!(reopened.google_client_id, app.google_client_id);
        assert_eq!(reopened.google_client_secret, app.google_client_secret);
        assert_eq!(reopened.mail_providers, app.mail_providers);
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
        app.pending_service = Service::Mailbox(MailProvider::Microsoft);
        drop(sender);
        app.poll(|| {});
        assert!(app.pending.is_none());
        assert!(!app.microsoft.lines.is_empty());
        assert!(app.model_status.lines.is_empty());
    }

    #[test]
    fn disconnected_google_mailbox_worker_reports_on_google_status() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Mailbox(MailProvider::Google);
        drop(sender);

        assert!(app.poll(|| {}));
        assert!(app.microsoft.lines.is_empty());
        assert_eq!(
            app.google.lines,
            ["The operation stopped unexpectedly. Please try again."]
        );
    }

    // Regression guard for the perf fix: the native adapter's busy-tick
    // timer decides between a full `refresh` and the lightweight
    // `sync_busy` based on this return value, so it must be exact --
    // `false` on nothing pending or an empty channel (job still running),
    // `true` the moment an outcome (success or a disconnected worker) is
    // actually consumed.
    #[test]
    fn poll_reports_whether_it_consumed_an_outcome() {
        let mut app = AppModel::new();
        assert!(!app.poll(|| {}), "nothing pending");

        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Model;
        assert!(!app.poll(|| {}), "channel still empty");
        assert!(app.pending.is_some());

        sender.send(Outcome::Generation(Ok(()))).unwrap();
        assert!(app.poll(|| {}), "a result arrived");
        assert!(app.pending.is_none());

        let (sender, receiver) = mpsc::channel::<Outcome>();
        app.pending = Some(receiver);
        drop(sender);
        assert!(app.poll(|| {}), "the worker disconnected");
        assert!(app.pending.is_none());
    }

    #[test]
    fn connection_outcome_updates_only_the_named_provider_status() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::Connection(
                MailProvider::Google,
                Err(ConnectionError::ProviderUnavailable),
            ))
            .unwrap();

        assert!(app.poll(|| {}));
        assert!(!app.connection_status(MailProvider::Google).lines.is_empty());
        assert!(
            app.connection_status(MailProvider::Microsoft)
                .lines
                .is_empty()
        );
    }

    #[test]
    fn microsoft_registration_helpers_prefer_the_typed_id() {
        let mut app = AppModel::with_store(Ok(None));
        app.client_id = "00000000-0000-4000-8000-000000000000".into();
        assert_eq!(
            app.effective_microsoft_client_id().as_deref(),
            Some("00000000-0000-4000-8000-000000000000")
        );
        assert!(!app.shared_registration_active());
        assert!(
            app.admin_consent_url()
                .is_some_and(|url| url.contains("client_id=00000000-0000-4000-8000-000000000000"))
        );
    }

    const SYNTHETIC_MS_ID: &str = "00000000-0000-4000-8000-000000000000";
    const SYNTHETIC_GOOGLE_ID: &str = "123-synthetic.apps.googleusercontent.com";

    fn mail_item(provider: MailProvider, account: &str, id: &str) -> MailItem {
        MailItem {
            provider,
            id: id.into(),
            account: account.into(),
            conversation: format!("thread-{id}"),
            received: "not-a-timestamp".into(),
            ..MailItem::default()
        }
    }

    fn provider_source(
        provider: MailProvider,
        label: &str,
        messages: Vec<MailItem>,
    ) -> SourceReview {
        SourceReview {
            provider,
            label: label.into(),
            messages,
            errors: vec![],
            message_errors: vec![],
            partial: false,
            failed: false,
        }
    }

    fn load(
        provider: MailProvider,
        sources: Result<Vec<SourceReview>, ConnectionError>,
    ) -> openloops_graph::live::ProviderLoad {
        openloops_graph::live::ProviderLoad { provider, sources }
    }

    #[test]
    fn enabled_providers_follow_all_order_and_persist() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        assert_eq!(app.enabled_mail_providers(), [MailProvider::Microsoft]);
        assert!(app.set_provider_enabled(MailProvider::Google, true));
        assert_eq!(
            app.enabled_mail_providers(),
            [MailProvider::Microsoft, MailProvider::Google]
        );
        assert_eq!(
            memory.saved.borrow().as_ref().unwrap().mail_providers,
            BTreeSet::from([MailProvider::Microsoft, MailProvider::Google])
        );
        assert!(app.set_provider_enabled(MailProvider::Microsoft, false));
        assert_eq!(app.enabled_mail_providers(), [MailProvider::Google]);
        assert_eq!(
            memory.saved.borrow().as_ref().unwrap().mail_providers,
            BTreeSet::from([MailProvider::Google])
        );
    }

    #[test]
    fn disabling_the_last_enabled_provider_is_refused() {
        let mut app = AppModel::with_store(Ok(None));
        assert!(!app.set_provider_enabled(MailProvider::Microsoft, false));
        assert_eq!(app.enabled_mail_providers(), [MailProvider::Microsoft]);
        assert_eq!(
            app.settings_status.lines,
            ["Keep at least one mail provider enabled."]
        );
        assert!(!app.settings_status.succeeded);

        // A later successful toggle clears the refusal line.
        assert!(app.set_provider_enabled(MailProvider::Google, true));
        assert!(app.settings_status.lines.is_empty());
        app.settings_status = Status {
            lines: vec!["Keep at least one mail provider enabled.".into()],
            succeeded: false,
        };
        assert!(app.set_provider_enabled(MailProvider::Google, false));
        assert!(app.settings_status.lines.is_empty());
    }

    #[test]
    fn disabling_a_provider_removes_only_its_mail_and_status() {
        let mut app = AppModel::with_store(Ok(None));
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft, MailProvider::Google]);
        app.microsoft.lines = vec!["Personal inbox: access confirmed.".into()];
        app.google.lines = vec!["Personal inbox: access confirmed.".into()];
        app.review = crate::review_model::layout_fixture();
        let microsoft_messages = app.review.messages.len();
        Arc::make_mut(&mut app.mail_cache).extend([
            (
                ("synthetic".into(), "synthetic-ms".into()),
                mail_item(MailProvider::Microsoft, "synthetic", "synthetic-ms"),
            ),
            (
                ("google:synthetic-sub".into(), "synthetic-g".into()),
                mail_item(MailProvider::Google, "google:synthetic-sub", "synthetic-g"),
            ),
        ]);

        assert!(app.set_provider_enabled(MailProvider::Google, false));

        assert!(app.google.lines.is_empty());
        assert_eq!(app.microsoft.lines, ["Personal inbox: access confirmed."]);
        assert_eq!(app.review.messages.len(), microsoft_messages);
        assert!(app.review.analysis.is_some());
        assert_eq!(app.mail_cache.len(), 1);
        assert!(
            app.mail_cache
                .contains_key(&("synthetic".into(), "synthetic-ms".into()))
        );

        assert!(app.set_provider_enabled(MailProvider::Google, true));
        assert_eq!(app.microsoft.lines, ["Personal inbox: access confirmed."]);
        assert_eq!(app.review.messages.len(), microsoft_messages);
    }

    #[test]
    fn viewed_mail_provider_is_a_ui_selection_only() {
        let memory = MemoryStore::default();
        let mut app = AppModel::with_store(Ok(Some(Box::new(memory.clone()))));
        app.review = crate::review_model::layout_fixture();
        assert_eq!(app.active_mail_provider(), MailProvider::Microsoft);
        app.mail_providers = BTreeSet::from([MailProvider::Google]);
        assert_eq!(app.active_mail_provider(), MailProvider::Google);
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft]);

        app.set_active_mail_provider(MailProvider::Google);

        assert_eq!(app.active_mail_provider(), MailProvider::Google);
        assert_eq!(app.enabled_mail_providers(), [MailProvider::Microsoft]);
        assert!(app.review.analysis.is_some());
        assert!(memory.saved.borrow().is_none());
    }

    #[test]
    fn enabled_accounts_skip_a_provider_without_registration() {
        let mut app = AppModel::with_store(Ok(None));
        if registration::google().is_some() {
            return;
        }
        app.client_id = SYNTHETIC_MS_ID.into();
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft, MailProvider::Google]);
        let accounts = app.enabled_accounts().unwrap();
        assert_eq!(
            accounts
                .iter()
                .map(AccountConfig::provider)
                .collect::<Vec<_>>(),
            [MailProvider::Microsoft]
        );
        assert_eq!(
            app.google.lines,
            [format!("Google: {}", ConnectionError::InvalidConfiguration)]
        );
        assert!(app.microsoft.lines.is_empty());

        app.google_client_id = SYNTHETIC_GOOGLE_ID.into();
        app.google_client_secret = Zeroizing::new("synthetic-client-parameter".into());
        assert_eq!(app.enabled_accounts().unwrap().len(), 2);
    }

    #[test]
    fn enabled_accounts_without_any_registration_fail_without_status_lines() {
        let mut app = AppModel::with_store(Ok(None));
        if registration::microsoft().is_some() {
            return;
        }
        assert!(matches!(
            app.enabled_accounts(),
            Err(ConnectionError::InvalidConfiguration)
        ));
        assert!(app.microsoft.lines.is_empty());
    }

    #[test]
    fn mail_outcome_keeps_the_ok_provider_and_records_the_failed_one() {
        let mut app = AppModel::with_store(Ok(None));
        app.load_progress = Some(Arc::new(LoadProgress::default()));
        app.microsoft.lines = vec!["Personal inbox: access confirmed.".into()];
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Review;
        sender
            .send(Outcome::Mail(vec![
                load(
                    MailProvider::Microsoft,
                    Ok(vec![provider_source(
                        MailProvider::Microsoft,
                        "Personal mailbox / Inbox",
                        vec![mail_item(
                            MailProvider::Microsoft,
                            "synthetic",
                            "synthetic-a",
                        )],
                    )]),
                ),
                load(
                    MailProvider::Google,
                    Err(ConnectionError::ProviderUnavailable),
                ),
            ]))
            .unwrap();
        assert!(app.poll(|| {}));
        assert!(app.load_progress.is_none());
        assert!(
            app.mail_cache
                .contains_key(&("synthetic".into(), "synthetic-a".into()))
        );
        assert!(
            app.review
                .notices
                .iter()
                .any(|note| note.starts_with("Personal mailbox / Inbox"))
        );
        assert_eq!(
            app.google.lines,
            [format!("Google: {}", ConnectionError::ProviderUnavailable)]
        );
        assert!(!app.google.succeeded);
        assert_eq!(app.microsoft.lines, ["Personal inbox: access confirmed."]);
    }

    #[test]
    fn single_provider_mail_error_reports_on_the_review_status_as_before() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Review;
        sender
            .send(Outcome::Mail(vec![load(
                MailProvider::Microsoft,
                Err(ConnectionError::ProviderUnavailable),
            )]))
            .unwrap();
        assert!(app.poll(|| {}));
        assert_eq!(
            app.review_status.lines,
            [ConnectionError::ProviderUnavailable.to_string()]
        );
        assert!(app.review.scan_failed);
        assert!(app.microsoft.lines.is_empty());
    }

    #[test]
    fn both_providers_failing_report_each_prefixed_line() {
        let mut app = AppModel::with_store(Ok(None));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Review;
        sender
            .send(Outcome::Mail(vec![
                load(MailProvider::Microsoft, Err(ConnectionError::AccessDenied)),
                load(
                    MailProvider::Google,
                    Err(ConnectionError::ProviderUnavailable),
                ),
            ]))
            .unwrap();
        assert!(app.poll(|| {}));
        assert_eq!(
            app.review_status.lines,
            [
                format!("Microsoft 365: {}", ConnectionError::AccessDenied),
                format!("Google: {}", ConnectionError::ProviderUnavailable),
            ]
        );
        assert!(app.review.scan_failed);
    }

    #[test]
    fn check_mail_with_mixed_loads_keeps_ok_mail_and_records_the_error() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        app.last_mail_check = Some(0);
        let known = MailItem {
            id: "synthetic-0".into(),
            account: "synthetic".into(),
            conversation: "synthetic-thread".into(),
            subject: "Quarterly planning".into(),
            body: "Synthetic cached body.".into(),
            received: "2026-09-06T12:00:00Z".into(),
            ..MailItem::default()
        };
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::CheckMail {
                loads: vec![
                    load(MailProvider::Microsoft, Ok(vec![check_source(vec![known])])),
                    load(MailProvider::Google, Err(ConnectionError::Unauthorized)),
                ],
            })
            .unwrap();
        assert!(app.poll(|| {}));
        assert!(
            app.mail_cache
                .contains_key(&("synthetic".into(), "synthetic-0".into()))
        );
        assert!(app.review_status.lines[0].starts_with("No new mail since "));
        assert_eq!(
            app.google.lines,
            [format!("Google: {}", ConnectionError::Unauthorized)]
        );
    }

    #[test]
    fn a_successful_load_removes_that_providers_earlier_load_error() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        app.last_mail_check = Some(0);
        let known = MailItem {
            id: "synthetic-0".into(),
            account: "synthetic".into(),
            conversation: "synthetic-thread".into(),
            subject: "Quarterly planning".into(),
            body: "Synthetic cached body.".into(),
            received: "2026-09-06T12:00:00Z".into(),
            ..MailItem::default()
        };
        for google in [Err(ConnectionError::Unauthorized), Ok(vec![])] {
            let (sender, receiver) = mpsc::channel();
            app.pending = Some(receiver);
            sender
                .send(Outcome::CheckMail {
                    loads: vec![
                        load(
                            MailProvider::Microsoft,
                            Ok(vec![check_source(vec![known.clone()])]),
                        ),
                        load(MailProvider::Google, google),
                    ],
                })
                .unwrap();
            assert!(app.poll(|| {}));
        }
        assert!(
            !app.google
                .lines
                .iter()
                .any(|line| line.starts_with("Google: "))
        );
    }

    #[test]
    fn retry_jobs_group_failed_labels_by_provider() {
        let mut app = AppModel::with_store(Ok(None));
        app.client_id = SYNTHETIC_MS_ID.into();
        app.google_client_id = SYNTHETIC_GOOGLE_ID.into();
        app.google_client_secret = Zeroizing::new("synthetic-client-parameter".into());
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft, MailProvider::Google]);
        let failed = BTreeSet::from([
            (
                MailProvider::Microsoft,
                "Personal mailbox / Inbox".to_owned(),
            ),
            (MailProvider::Google, "Gmail / Sent".to_owned()),
            (MailProvider::Google, "Gmail / Inbox".to_owned()),
        ]);
        let jobs = app.retry_jobs(&failed).unwrap();
        assert_eq!(
            jobs.iter()
                .map(|(account, labels)| (account.provider(), labels.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    MailProvider::Microsoft,
                    BTreeSet::from(["Personal mailbox / Inbox".to_owned()])
                ),
                (
                    MailProvider::Google,
                    BTreeSet::from(["Gmail / Inbox".to_owned(), "Gmail / Sent".to_owned()])
                ),
            ]
        );
    }

    #[test]
    fn retry_jobs_skip_a_provider_without_registration() {
        let mut app = AppModel::with_store(Ok(None));
        if registration::google().is_some() {
            return;
        }
        app.client_id = SYNTHETIC_MS_ID.into();
        app.mail_providers = BTreeSet::from([MailProvider::Microsoft, MailProvider::Google]);
        let failed = BTreeSet::from([
            (
                MailProvider::Microsoft,
                "Personal mailbox / Inbox".to_owned(),
            ),
            (MailProvider::Google, "Gmail / Inbox".to_owned()),
        ]);
        let jobs = app.retry_jobs(&failed).unwrap();
        assert_eq!(
            jobs.iter()
                .map(|(account, _)| account.provider())
                .collect::<Vec<_>>(),
            [MailProvider::Microsoft]
        );
        assert_eq!(
            app.google.lines,
            [format!("Google: {}", ConnectionError::InvalidConfiguration)]
        );
        assert!(app.microsoft.lines.is_empty());

        app.client_id.clear();
        if registration::microsoft().is_none() {
            assert!(matches!(
                app.retry_jobs(&failed),
                Err(ConnectionError::InvalidConfiguration)
            ));
        }
    }

    #[test]
    fn retry_mail_routes_replacements_by_provider_label() {
        let mut app = AppModel::with_store(Ok(None));
        app.review.failed_sources = BTreeSet::from([
            (
                MailProvider::Microsoft,
                "Personal mailbox / Inbox".to_owned(),
            ),
            (MailProvider::Google, "Gmail / Inbox".to_owned()),
        ]);
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::RetryMail {
                loads: vec![
                    load(
                        MailProvider::Microsoft,
                        Ok(vec![provider_source(
                            MailProvider::Microsoft,
                            "Personal mailbox / Inbox",
                            vec![],
                        )]),
                    ),
                    load(
                        MailProvider::Google,
                        Err(ConnectionError::ProviderUnavailable),
                    ),
                ],
                source_keys: app.review.failed_sources.clone(),
                conversations: BTreeSet::new(),
                attempted: 2,
            })
            .unwrap();
        assert!(app.poll(|| {}));
        assert_eq!(
            app.review.failed_sources,
            BTreeSet::from([(MailProvider::Google, "Gmail / Inbox".to_owned())])
        );
        assert_eq!(app.review_status.lines, ["Retried 2; 1 still failing."]);
        assert_eq!(
            app.google.lines,
            [format!("Google: {}", ConnectionError::ProviderUnavailable)]
        );
    }

    #[test]
    fn effective_google_registration_precedence_is_pure() {
        let shipped = Some((
            "123-shipped.apps.googleusercontent.com",
            "synthetic-shipped-parameter",
        ));
        let byo = effective_google_registration_for(
            " 456-byo.apps.googleusercontent.com ",
            " synthetic-byo-parameter ",
            shipped,
        )
        .unwrap();
        assert_eq!(byo.0, "456-byo.apps.googleusercontent.com");
        assert_eq!(&*byo.1, "synthetic-byo-parameter");

        let shared = effective_google_registration_for("", " ", shipped).unwrap();
        assert_eq!(shared.0, "123-shipped.apps.googleusercontent.com");
        assert_eq!(&*shared.1, "synthetic-shipped-parameter");
        assert!(effective_google_registration_for("", "", None).is_none());
        assert!(
            effective_google_registration_for("456-byo.apps.googleusercontent.com", "", shipped)
                .is_none()
        );
        assert!(effective_google_registration_for("", "synthetic", shipped).is_none());
    }

    #[test]
    fn google_account_config_requires_both_valid_synthetic_fields() {
        let mut app = AppModel::with_store(Ok(None));
        app.google_client_id = "123-synthetic.apps.googleusercontent.com".into();
        assert!(matches!(
            app.account_config(MailProvider::Google),
            Err(ConnectionError::InvalidConfiguration)
        ));
        app.google_client_secret = Zeroizing::new("synthetic-client-parameter".into());
        assert!(matches!(
            app.account_config(MailProvider::Google),
            Ok(AccountConfig::Google(_))
        ));
    }

    #[test]
    fn shared_registration_is_active_only_for_an_empty_byo_field_when_shipped() {
        assert!(shared_registration_active_for("", Some("synthetic")));
        assert!(shared_registration_active_for("  \t", Some("synthetic")));
        assert!(!shared_registration_active_for(
            "synthetic-byo",
            Some("synthetic")
        ));
        assert!(!shared_registration_active_for("", None));
    }

    #[test]
    fn empty_byo_without_a_shipped_registration_has_no_microsoft_registration() {
        if registration::microsoft().is_some() {
            return;
        }
        let app = AppModel::with_store(Ok(None));
        assert!(app.client_id.is_empty());
        assert!(app.effective_microsoft_client_id().is_none());
        assert!(app.admin_consent_url().is_none());
        assert!(!app.shared_registration_active());
    }

    #[test]
    fn mail_outcome_populates_the_session_cache_and_clears_load_progress() {
        use openloops_graph::live::review::{LoadProgress, MailItem, SourceReview};

        let mut app = AppModel::with_store(Ok(None));
        app.load_progress = Some(Arc::new(LoadProgress::default()));
        let sources = vec![SourceReview {
            provider: openloops_graph::live::MailProvider::Microsoft,
            label: "Personal mailbox / Inbox".into(),
            messages: vec![
                MailItem {
                    id: "synthetic-message-a".into(),
                    account: "synthetic-account".into(),
                    conversation: "synthetic-conversation-a".into(),
                    received: "not-a-timestamp".into(),
                    ..MailItem::default()
                },
                MailItem {
                    id: "synthetic-message-b".into(),
                    account: "synthetic-account".into(),
                    conversation: "synthetic-conversation-b".into(),
                    received: "not-a-timestamp".into(),
                    ..MailItem::default()
                },
            ],
            errors: vec![],
            message_errors: vec![],
            partial: false,
            failed: false,
        }];
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Review;
        sender
            .send(Outcome::Mail(vec![load(
                MailProvider::Microsoft,
                Ok(sources),
            )]))
            .unwrap();
        assert!(app.poll(|| {}));
        assert!(app.load_progress.is_none());
        assert_eq!(app.mail_cache.len(), 2);
        assert!(
            app.mail_cache
                .contains_key(&("synthetic-account".into(), "synthetic-message-a".into()))
        );
        assert!(
            app.mail_cache
                .contains_key(&("synthetic-account".into(), "synthetic-message-b".into()))
        );
    }

    #[test]
    fn cancelled_mail_outcome_is_not_a_scan_failure() {
        use openloops_graph::live::review::LoadProgress;

        let mut app = AppModel::with_store(Ok(None));
        app.load_progress = Some(Arc::new(LoadProgress::default()));
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_service = Service::Review;
        sender
            .send(Outcome::Mail(vec![load(
                MailProvider::Microsoft,
                Err(ConnectionError::Cancelled),
            )]))
            .unwrap();
        assert!(app.poll(|| {}));
        assert!(app.load_progress.is_none());
        assert!(!app.review.scan_failed);
        assert!(!app.review_status.succeeded);
        assert_eq!(
            app.review_status.lines,
            ["Download stopped before the scan began."]
        );
    }

    #[test]
    fn check_mail_with_no_new_sources_reports_no_new_mail_since_last_check() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        app.last_mail_check = Some(0);
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::CheckMail {
                loads: vec![load(MailProvider::Microsoft, Ok(Vec::new()))],
            })
            .unwrap();
        assert!(app.poll(|| {}));
        let expected = chrono::DateTime::from_timestamp(0, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M");
        assert_eq!(
            app.review_status.lines,
            [format!("No new mail since {expected}.")]
        );
        assert!(app.review_status.succeeded);
    }

    #[test]
    fn check_mail_cancelled_leaves_review_untouched() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        let handles: Vec<_> = app
            .review
            .messages
            .iter()
            .map(|message| message.input.handle.clone())
            .collect();
        let item_count = app.review.analysis.as_ref().unwrap().items.len();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::CheckMail {
                loads: vec![load(
                    MailProvider::Microsoft,
                    Err(ConnectionError::Cancelled),
                )],
            })
            .unwrap();
        assert!(app.poll(|| {}));
        assert_eq!(
            app.review
                .messages
                .iter()
                .map(|message| message.input.handle.clone())
                .collect::<Vec<_>>(),
            handles
        );
        assert_eq!(
            app.review.analysis.as_ref().unwrap().items.len(),
            item_count
        );
        assert_eq!(
            app.review_status.lines,
            ["Check stopped; no new mail was added."]
        );
    }

    #[test]
    fn check_scan_outcome_merges_and_formats_counts() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        app.review.source_failures = 0;
        let mut item = app.review.analysis.as_ref().unwrap().items[2].clone();
        item.action = "Synthetic replacement action".into();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::CheckScan {
                result: Ok(check_scan_result(vec![item])),
                conversations: BTreeSet::from([("synthetic".into(), "synthetic-thread-3".into())]),
                new_messages: 2,
            })
            .unwrap();
        assert!(app.poll(|| {}));
        assert_eq!(
            app.review_status.lines,
            ["2 new messages in 1 conversations; 1 loops added, 0 updates suggested."]
        );
        assert!(app.review_status.succeeded);
    }

    #[test]
    fn check_mail_extends_cache_without_replacing_it() {
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        Arc::make_mut(&mut app.mail_cache).insert(
            ("existing-account".into(), "existing-id".into()),
            MailItem::default(),
        );
        let known = MailItem {
            id: "synthetic-0".into(),
            account: "synthetic".into(),
            conversation: "synthetic-thread".into(),
            subject: "Quarterly planning".into(),
            body: "Synthetic cached body.".into(),
            received: "2026-09-06T12:00:00Z".into(),
            ..MailItem::default()
        };
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::CheckMail {
                loads: vec![load(
                    MailProvider::Microsoft,
                    Ok(vec![check_source(vec![known])]),
                )],
            })
            .unwrap();
        assert!(app.poll(|| {}));
        assert!(
            app.mail_cache
                .contains_key(&("existing-account".into(), "existing-id".into()))
        );
        assert!(
            app.mail_cache
                .contains_key(&("synthetic".into(), "synthetic-0".into()))
        );
    }

    #[test]
    fn retry_completion_uses_review_status_without_touching_action_status() {
        let mut app = AppModel::with_store(Ok(None));
        app.review.action_status = "Synthetic decision status".into();
        app.review.action_status_succeeded = false;
        app.review.failed_conversations_detail = vec![crate::review_model::ConversationFailure {
            account: "synthetic-account".into(),
            conversation: "synthetic-timeout".into(),
            subject_short: "Synthetic subject".into(),
            reason: crate::review_model::FailureReason::Timeout,
        }];

        app.finish_retry_status(3);

        assert_eq!(app.review.action_status, "Synthetic decision status");
        assert!(!app.review.action_status_succeeded);
        assert_eq!(app.review_status.lines, ["Retried 3; 1 still failing."]);
        assert!(!app.review_status.succeeded);

        app.review.failed_conversations_detail.clear();
        app.finish_retry_status(1);
        assert_eq!(app.review.action_status, "Synthetic decision status");
        assert_eq!(app.review_status.lines, ["Retried 1; 0 still failing."]);
        assert!(app.review_status.succeeded);
    }

    #[test]
    fn group_failure_is_visible_and_does_not_mark_microsoft_ready() {
        let status = connection_report_status(
            MailProvider::Microsoft,
            Ok(ConnectionReport {
                own_inbox_accessible: true,
                shared_scope: SharedScope::NotReported,
                shared_inbox_results: vec![],
                group_inbox_results: vec![Ok(()), Err(ConnectionError::AccessDenied), Ok(())],
            }),
        );
        assert!(!status.succeeded);
        assert_eq!(status.lines.len(), 4);
        assert!(status.lines[2].contains("HTTP 403"));
    }

    #[test]
    fn google_connection_report_has_only_the_personal_inbox_line() {
        let status = connection_report_status(
            MailProvider::Google,
            Ok(ConnectionReport {
                own_inbox_accessible: true,
                shared_scope: SharedScope::Granted,
                shared_inbox_results: vec![Err(ConnectionError::AccessDenied)],
                group_inbox_results: vec![Err(ConnectionError::AccessDenied)],
            }),
        );
        assert!(status.succeeded);
        assert_eq!(status.lines, ["Personal inbox: access confirmed."]);
    }

    #[test]
    fn trusted_message_links_accept_only_allowlisted_https_hosts_with_paths() {
        assert!(is_trusted_message_link(
            "https://outlook.office.com/mail/deeplink"
        ));
        assert!(is_trusted_message_link(
            "https://outlook.office365.com/mail/deeplink"
        ));
        assert!(is_trusted_message_link(
            "https://outlook.live.com/mail/deeplink"
        ));
        assert!(is_trusted_message_link(
            "https://outlook.office365.us/mail/deeplink"
        ));
        assert!(!is_trusted_message_link("https://outlook.office.com"));
        assert!(!is_trusted_message_link("https://outlook.live.com"));
        assert!(!is_trusted_message_link(
            "https://example.invalid/outlook.office.com/"
        ));
        assert!(!is_trusted_message_link(
            "https://outlook.office.com.evil.invalid/mail/deeplink"
        ));
        assert!(!is_trusted_message_link(
            "http://outlook.office.com/mail/deeplink"
        ));
        assert!(!is_trusted_message_link(
            "http://outlook.live.com/mail/deeplink"
        ));
        assert!(!is_trusted_message_link(
            "https://outlook.office.com:443/mail/deeplink"
        ));
        assert!(!is_trusted_message_link(
            "https://user@outlook.office.com/mail/deeplink"
        ));
        assert!(is_trusted_message_link(
            "https://mail.google.com/mail/u/0/#inbox/synthetic"
        ));
        assert!(!is_trusted_message_link(
            "https://evil.example/mail.google.com/"
        ));
    }

    #[test]
    fn account_display_default_says_not_signed_in() {
        let display = AccountDisplay::default();
        assert_eq!(display.name, "Not signed in");
        assert_eq!(display.initials, "");
        assert!(!display.signed_in);
    }

    #[test]
    fn account_display_lists_connected_providers_in_stable_order() {
        for (providers, expected) in [
            (vec![MailProvider::Microsoft], "Signed in · Microsoft 365"),
            (vec![MailProvider::Google], "Signed in · Google"),
            (
                vec![MailProvider::Google, MailProvider::Microsoft],
                "Signed in · Microsoft 365 + Google",
            ),
        ] {
            let connected = AccountDisplay::connected(&providers);
            assert_eq!(connected.name, expected);
            assert_eq!(connected.initials, "");
            assert!(connected.signed_in);
        }

        let not_connected = AccountDisplay::connected(&[]);
        assert_eq!(not_connected.name, "");
        assert_eq!(not_connected.initials, "");
        assert!(!not_connected.signed_in);
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
        reopened.mail_cache = Arc::new(std::collections::HashMap::from([(
            ("synthetic-account".into(), "synthetic-message".into()),
            openloops_graph::live::review::MailItem::default(),
        )]));
        reopened.forget_settings();
        assert!(memory.saved.borrow().is_none());
        assert!(reopened.key.is_empty());
        assert!(reopened.openrouter_key.is_empty());
        assert!(reopened.groups.is_empty());
        assert!(reopened.mail_cache.is_empty());
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

    #[test]
    fn reminder_sync_marks_completed_tasks_handled_and_leaves_others_alone() {
        use crate::loop_state::{Decision, Record, Reminder, now};
        use openloops_graph::live::reminders::TaskStatusOutcome;
        let mut app = AppModel::new();
        let completed_key = [1; 32];
        let open_key = [2; 32];
        let unknown_key = [3; 32];
        for (key, decision) in [
            (completed_key, Decision::Mine),
            (open_key, Decision::Watching),
            (unknown_key, Decision::Review),
        ] {
            app.review
                .decisions
                .update(Record {
                    key,
                    decision,
                    reminder: Reminder::Created {
                        provider: MailProvider::Microsoft,
                        list_id: "list".into(),
                        task_id: "task".into(),
                    },
                    updated: now(),
                })
                .unwrap();
        }
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        sender
            .send(Outcome::ReminderSync(vec![
                (completed_key, TaskStatusOutcome::Completed),
                (open_key, TaskStatusOutcome::NotCompleted),
                (
                    unknown_key,
                    TaskStatusOutcome::Unknown(ConnectionError::Timeout(1)),
                ),
            ]))
            .unwrap();
        app.poll(|| {});

        let completed = app.review.decisions.get(&completed_key);
        assert_eq!(completed.decision, Decision::Done);
        assert!(matches!(completed.reminder, Reminder::Completed { .. }));

        let open = app.review.decisions.get(&open_key);
        assert_eq!(open.decision, Decision::Watching);
        assert!(matches!(open.reminder, Reminder::Created { .. }));

        let unknown = app.review.decisions.get(&unknown_key);
        assert_eq!(unknown.decision, Decision::Review);
        assert!(matches!(unknown.reminder, Reminder::Created { .. }));

        assert!(
            app.review
                .scan_errors
                .iter()
                .any(|line| line.contains("1 loop(s) marked handled")),
            "{:?}",
            app.review.scan_errors
        );
    }

    #[test]
    fn dispatch_reminder_sync_is_a_noop_with_no_eligible_reminders() {
        let mut app = AppModel::new();
        // No analysis at all: nothing to check.
        app.dispatch_reminder_sync();
        assert!(app.pending.is_none());
    }

    fn set_fixture_reminder(app: &mut AppModel, card: usize, provider: MailProvider) {
        use crate::loop_state::{Decision, Record, now};
        let items = app.review.analysis.as_ref().unwrap().items.clone();
        let cards = app.review.card_contexts(&items);
        let key = cards[card].as_ref().unwrap().record.key;
        app.review
            .decisions
            .update(Record {
                key,
                decision: Decision::Mine,
                reminder: Reminder::Created {
                    provider,
                    list_id: format!("synthetic-list-{card}"),
                    task_id: format!("synthetic-task-{card}"),
                },
                updated: now(),
            })
            .unwrap();
    }

    fn reminder_registrations(app: &mut AppModel) {
        app.client_id = "00000000-0000-4000-8000-000000000000".into();
        app.google_client_id = "123-synthetic.apps.googleusercontent.com".into();
        app.google_client_secret = Zeroizing::new("synthetic-client-parameter".into());
    }

    // The planner is tested rather than `dispatch_reminder_sync` itself:
    // starting the job would run a real browser sign-in on a worker thread.
    #[test]
    fn google_reminder_with_valid_google_config_plans_one_sync_job() {
        let mut app = AppModel::with_store(Ok(None));
        reminder_registrations(&mut app);
        app.review = crate::review_model::layout_fixture();
        set_fixture_reminder(&mut app, 0, MailProvider::Google);

        let jobs = app.reminder_sync_jobs();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0.provider(), MailProvider::Google);
        assert_eq!(jobs[0].1.len(), 1);
        assert_eq!(
            reminder_sync_job_label(&BTreeSet::from([MailProvider::Google])),
            "Checking Google Tasks for completed reminders"
        );
    }

    #[test]
    fn microsoft_and_google_reminders_plan_two_sync_groups() {
        let mut app = AppModel::with_store(Ok(None));
        reminder_registrations(&mut app);
        app.review = crate::review_model::layout_fixture();
        set_fixture_reminder(&mut app, 0, MailProvider::Microsoft);
        set_fixture_reminder(&mut app, 1, MailProvider::Google);

        let jobs = app.reminder_sync_jobs();

        let providers = jobs
            .iter()
            .map(|(config, checks)| (config.provider(), checks.len()))
            .collect::<Vec<_>>();
        assert_eq!(
            providers,
            vec![(MailProvider::Microsoft, 1), (MailProvider::Google, 1)]
        );
    }

    #[test]
    fn reminder_sync_job_label_names_the_providers_checked() {
        assert_eq!(
            reminder_sync_job_label(&BTreeSet::from([MailProvider::Microsoft])),
            "Checking Microsoft To Do for completed reminders"
        );
        assert_eq!(
            reminder_sync_job_label(&BTreeSet::from([MailProvider::Google])),
            "Checking Google Tasks for completed reminders"
        );
        assert_eq!(
            reminder_sync_job_label(&BTreeSet::from([
                MailProvider::Microsoft,
                MailProvider::Google
            ])),
            "Checking reminders for completed tasks"
        );
    }

    #[test]
    fn microsoft_reminder_config_ignores_mail_loading_shared_and_group_inputs() {
        let mut app = AppModel::with_store(Ok(None));
        reminder_registrations(&mut app);
        app.shared = "not a mailbox".into();
        app.groups = "not a group".into();
        assert!(app.account_config(MailProvider::Microsoft).is_err());
        assert!(app.reminder_account_config(MailProvider::Microsoft).is_ok());
        assert!(app.reminder_account_config(MailProvider::Google).is_ok());
    }

    #[test]
    fn reminder_completion_copy_is_per_provider() {
        use openloops_graph::live::reminders::ReminderCompletionOutcome;
        let mut app = AppModel::with_store(Ok(None));
        app.review = crate::review_model::layout_fixture();
        set_fixture_reminder(&mut app, 0, MailProvider::Microsoft);
        set_fixture_reminder(&mut app, 1, MailProvider::Google);
        let items = app.review.analysis.as_ref().unwrap().items.clone();
        let cards = app.review.card_contexts(&items);
        let microsoft = cards[0].as_ref().unwrap().record.key;
        let google = cards[1].as_ref().unwrap().record.key;
        let cases = [
            (
                microsoft,
                ReminderCompletionOutcome::Completed,
                "Decision saved on this Windows account. The linked Microsoft To Do task was also marked complete.".to_owned(),
            ),
            (
                microsoft,
                ReminderCompletionOutcome::NotCompleted(ConnectionError::NotFound),
                format!(
                    "Decision saved on this Windows account. The linked Microsoft To Do task was not marked complete: {}",
                    ConnectionError::NotFound
                ),
            ),
            (
                microsoft,
                ReminderCompletionOutcome::Uncertain,
                "Decision saved on this Windows account. Microsoft did not confirm the To Do task was marked complete; check it there.".to_owned(),
            ),
            (
                google,
                ReminderCompletionOutcome::Completed,
                "Decision saved on this Windows account. The linked Google Tasks task was also marked complete.".to_owned(),
            ),
            (
                google,
                ReminderCompletionOutcome::NotCompleted(ConnectionError::NotFound),
                format!(
                    "Decision saved on this Windows account. The linked Google Tasks task was not marked complete: {}",
                    ConnectionError::NotFound
                ),
            ),
            (
                google,
                ReminderCompletionOutcome::Uncertain,
                "Decision saved on this Windows account. Google did not confirm the Google Tasks task was marked complete; check it there.".to_owned(),
            ),
        ];
        for (key, outcome, expected) in cases {
            app.reminder_completion_outcome(key, outcome);
            assert_eq!(app.review.action_status, expected);
        }
    }

    #[test]
    fn reminder_create_success_and_uncertain_copy_is_per_provider() {
        use openloops_graph::live::reminders::ReminderOutcome;
        let mut app = AppModel::with_store(Ok(None));
        app.reminder_outcome(
            [9_u8; 32],
            MailProvider::Microsoft,
            ReminderOutcome::Created {
                provider: MailProvider::Microsoft,
                list_id: "synthetic-list".into(),
                task_id: "synthetic-task".into(),
            },
        );
        assert_eq!(
            app.review.action_status,
            "Reminder created in your Microsoft To Do Tasks list."
        );
        app.reminder_outcome(
            [10_u8; 32],
            MailProvider::Microsoft,
            ReminderOutcome::Uncertain,
        );
        assert_eq!(
            app.review.action_status,
            "Microsoft did not confirm the write. Check To Do before trying again; OpenLoops will not automatically retry."
        );
        app.reminder_outcome(
            [11_u8; 32],
            MailProvider::Google,
            ReminderOutcome::Uncertain,
        );
        assert_eq!(
            app.review.action_status,
            "Google did not confirm the write. Check Google Tasks before trying again; OpenLoops will not automatically retry."
        );
    }

    #[test]
    fn reminder_sync_outcome_copy_is_per_provider() {
        use openloops_graph::live::reminders::TaskStatusOutcome;
        for (providers, expected) in [
            (
                vec![MailProvider::Microsoft],
                "1 loop(s) marked handled because their linked Microsoft To Do task was already completed.",
            ),
            (
                vec![MailProvider::Google],
                "1 loop(s) marked handled because their linked Google Tasks task was already completed.",
            ),
            (
                vec![MailProvider::Microsoft, MailProvider::Google],
                "2 loop(s) marked handled because their linked task was already completed.",
            ),
        ] {
            let mut app = AppModel::with_store(Ok(None));
            app.review = crate::review_model::layout_fixture();
            let items = app.review.analysis.as_ref().unwrap().items.clone();
            let mut results = Vec::new();
            for (card, provider) in providers.iter().enumerate() {
                set_fixture_reminder(&mut app, card, *provider);
                let cards = app.review.card_contexts(&items);
                results.push((
                    cards[card].as_ref().unwrap().record.key,
                    TaskStatusOutcome::Completed,
                ));
            }
            app.reminder_sync_outcome(results);
            assert_eq!(app.review.scan_errors.last().unwrap(), expected);
        }
    }

    #[test]
    fn reminder_outcome_maps_each_reason_to_exact_status_text() {
        use openloops_graph::live::reminders::{ReminderFailure, ReminderOutcome};

        let cases = [
            (
                ReminderFailure::AccountMismatch,
                "No reminder was created: The browser signed into a different account than the one that was scanned. Sign in with the scanned account and try again.",
            ),
            (
                ReminderFailure::InvalidDraft,
                "No reminder was created: The reminder draft is not valid: the title needs 3 to 320 plain characters and the time must be in the future.",
            ),
            (
                ReminderFailure::DefaultListNotFound,
                "No reminder was created: Microsoft To Do did not return a single default Tasks list for this account. Open To Do once so the account's lists exist, then try again.",
            ),
            (
                ReminderFailure::Rejected(ConnectionError::Transport),
                "No reminder was created: The mail service could not be reached over a secure connection.",
            ),
            (
                ReminderFailure::Rejected(ConnectionError::AccessDenied),
                "No reminder was created: Microsoft refused the To Do write. The app registration needs the delegated Tasks.ReadWrite permission and the signed-in account must consent to it. The mail service returned HTTP 403. Check consent and this signed-in account's access to the selected mailbox or group; an administrator role alone does not grant content access.",
            ),
            (
                ReminderFailure::Rejected(ConnectionError::Unauthorized),
                "No reminder was created: Microsoft refused the To Do write. The app registration needs the delegated Tasks.ReadWrite permission and the signed-in account must consent to it. The mail service returned HTTP 401. Sign in again; if it persists, check the organization's access policies.",
            ),
        ];

        for (index, (reason, expected)) in cases.into_iter().enumerate() {
            let mut app = AppModel::with_store(Ok(None));
            let mut key = [0_u8; 32];
            key[0] = u8::try_from(index).unwrap();
            app.reminder_outcome(
                key,
                MailProvider::Microsoft,
                ReminderOutcome::NotCreated(reason),
            );
            assert_eq!(app.review.action_status, expected);
            assert!(!app.review.action_status_succeeded);
        }
    }

    #[test]
    fn google_reminder_outcomes_use_google_copy_and_preserve_provider() {
        use openloops_graph::live::reminders::{ReminderFailure, ReminderOutcome};
        let mut app = AppModel::with_store(Ok(None));
        let created_key = [7_u8; 32];
        app.reminder_outcome(
            created_key,
            MailProvider::Google,
            ReminderOutcome::Created {
                provider: MailProvider::Google,
                list_id: "synthetic-list".into(),
                task_id: "synthetic-task".into(),
            },
        );
        assert_eq!(
            app.review.action_status,
            "Reminder created in your Google Tasks default list."
        );
        assert!(matches!(
            app.review.decisions.get(&created_key).reminder,
            Reminder::Created {
                provider: MailProvider::Google,
                ..
            }
        ));

        app.reminder_outcome(
            [8_u8; 32],
            MailProvider::Google,
            ReminderOutcome::NotCreated(ReminderFailure::DefaultListNotFound),
        );
        assert_eq!(
            app.review.action_status,
            "No reminder was created: Google Tasks did not return a default list for this account. Open Google Tasks once, then try again."
        );
    }

    #[test]
    fn reminder_sync_eligible_count_matches_what_dispatch_would_check() {
        use crate::loop_state::{Decision, Record, Reminder, now};
        let mut app = AppModel::new();
        // No analysis: the count is zero regardless of `cards`.
        assert_eq!(app.reminder_sync_eligible_count(&[]), 0);

        app.review = crate::review_model::layout_fixture();
        let items = app.review.analysis.as_ref().unwrap().items.clone();
        let cards = app.review.card_contexts(&items);
        // The fixture already saves card 0 as `Mine` with a `Created`
        // reminder carrying real ids -- exactly what `dispatch_reminder_sync`
        // would check. Card 2 is `Watching`/`Attempted` (no confirmed task
        // id) and card 1 has no saved decision at all, so neither counts.
        assert_eq!(app.reminder_sync_eligible_count(&cards), 1);

        let key = cards[1].as_ref().unwrap().record.key;
        app.review
            .decisions
            .update(Record {
                key,
                decision: Decision::Watching,
                reminder: Reminder::Created {
                    provider: MailProvider::Microsoft,
                    list_id: "list-2".into(),
                    task_id: "task-2".into(),
                },
                updated: now(),
            })
            .unwrap();
        let cards = app.review.card_contexts(&items);
        assert_eq!(app.reminder_sync_eligible_count(&cards), 2);
    }
}
