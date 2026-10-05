use std::collections::BTreeSet;

use super::google::GoogleConfig;
use super::reminders::{
    ReminderCompletionOutcome, ReminderOutcome, ReminderRequest, TaskStatusOutcome,
};
use super::review::{LoadProgress, MailCache, SourceReview};
use super::{ConnectionConfig, ConnectionError, ConnectionReport};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MailProvider {
    #[default]
    Microsoft,
    Google,
}

impl MailProvider {
    pub const ALL: [Self; 2] = [Self::Microsoft, Self::Google];

    #[must_use]
    pub const fn service_name(self) -> &'static str {
        match self {
            Self::Microsoft => "Microsoft 365",
            Self::Google => "Google",
        }
    }

    #[must_use]
    pub const fn mail_client_name(self) -> &'static str {
        match self {
            Self::Microsoft => "Outlook",
            Self::Google => "Gmail",
        }
    }

    #[must_use]
    pub const fn tasks_name(self) -> &'static str {
        match self {
            Self::Microsoft => "Microsoft To Do",
            Self::Google => "Google Tasks",
        }
    }

    #[must_use]
    pub const fn tasks_url(self) -> &'static str {
        match self {
            Self::Microsoft => "https://to-do.office.com/tasks/",
            Self::Google => "https://tasks.google.com/",
        }
    }

    #[must_use]
    pub const fn account_prefix(self) -> &'static str {
        match self {
            Self::Microsoft => "",
            Self::Google => "google:",
        }
    }

    #[must_use]
    pub fn is_trusted_message_link(self, url: &str) -> bool {
        const OUTLOOK: [&str; 4] = [
            "https://outlook.office.com/",
            "https://outlook.office365.com/",
            "https://outlook.live.com/",
            "https://outlook.office365.us/",
        ];
        match self {
            Self::Microsoft => OUTLOOK.iter().any(|origin| url.starts_with(origin)),
            Self::Google => url.starts_with("https://mail.google.com/"),
        }
    }

    #[must_use]
    pub const fn slot(self) -> usize {
        match self {
            Self::Microsoft => 0,
            Self::Google => 1,
        }
    }
}

pub enum AccountConfig {
    Microsoft(ConnectionConfig),
    Google(GoogleConfig),
}

#[must_use]
pub fn create_reminder(account: &AccountConfig, request: &ReminderRequest) -> ReminderOutcome {
    if account.provider() != request.provider {
        return ReminderOutcome::NotCreated(super::reminders::ReminderFailure::InvalidDraft);
    }
    match account {
        AccountConfig::Microsoft(config) => super::reminders::create(config, request),
        AccountConfig::Google(config) => super::google::tasks::create(config, request),
    }
}

#[must_use]
pub fn complete_reminder(
    account: &AccountConfig,
    identity: &str,
    list_id: &str,
    task_id: &str,
) -> ReminderCompletionOutcome {
    match account {
        AccountConfig::Microsoft(config) => {
            super::reminders::complete(config, identity, list_id, task_id)
        }
        AccountConfig::Google(config) => {
            super::google::tasks::complete(config, identity, list_id, task_id)
        }
    }
}

#[must_use]
pub fn reminder_status(
    account: &AccountConfig,
    identity: &str,
    list_id: &str,
    task_id: &str,
) -> TaskStatusOutcome {
    match account {
        AccountConfig::Microsoft(config) => {
            super::reminders::check_status(config, identity, list_id, task_id)
        }
        AccountConfig::Google(config) => {
            super::google::tasks::check_status(config, identity, list_id, task_id)
        }
    }
}

impl AccountConfig {
    #[must_use]
    pub const fn provider(&self) -> MailProvider {
        match self {
            Self::Microsoft(_) => MailProvider::Microsoft,
            Self::Google(_) => MailProvider::Google,
        }
    }

    /// Checks the configured provider connection.
    ///
    /// # Errors
    /// Returns a fixed, content-free configuration, authorization, or transport error.
    pub fn check_connection(&self) -> Result<ConnectionReport, ConnectionError> {
        match self {
            Self::Microsoft(config) => super::check_connection(config),
            Self::Google(config) => super::google::check_connection(config),
        }
    }

    /// How many sources a load with `filter` plans to read; the progress total.
    #[must_use]
    pub fn source_count(&self, filter: Option<&BTreeSet<String>>) -> usize {
        match self {
            Self::Microsoft(config) => super::review::source_count(config, filter),
            Self::Google(_) => super::google::gmail::source_count(filter),
        }
    }

    /// Loads recent sources through the configured provider.
    ///
    /// Call [`LoadProgress::begin`] once per job before this. Loaders never
    /// call it.
    ///
    /// # Errors
    /// Returns a fixed, content-free provider or loading error.
    pub fn load_recent_with(
        &self,
        cache: &MailCache,
        progress: &LoadProgress,
    ) -> Result<Vec<SourceReview>, ConnectionError> {
        self.load_sources_with(cache, progress, None)
    }

    /// Loads selected sources through the configured provider.
    ///
    /// Call [`LoadProgress::begin`] once per job before this. Loaders never
    /// call it.
    ///
    /// # Errors
    /// Returns a fixed, content-free provider or loading error.
    pub fn load_sources_with(
        &self,
        cache: &MailCache,
        progress: &LoadProgress,
        filter: Option<&BTreeSet<String>>,
    ) -> Result<Vec<SourceReview>, ConnectionError> {
        match self {
            Self::Microsoft(config) => {
                super::review::load_sources_with(config, cache, progress, filter)
            }
            Self::Google(config) => {
                super::google::gmail::load_sources_with(config, cache, progress, filter)
            }
        }
    }
}

pub struct ProviderLoad {
    pub provider: MailProvider,
    pub sources: Result<Vec<SourceReview>, ConnectionError>,
}

/// Loads every account in order. Progress starts once over all accounts; one
/// account's error never stops the next.
pub fn load_all(
    accounts: &[AccountConfig],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Vec<ProviderLoad> {
    load_each(
        accounts.iter().map(|account| (account, None)),
        cache,
        progress,
    )
}

/// Loads only the given source labels of each account, in order, as one job.
pub fn load_selected(
    jobs: &[(AccountConfig, BTreeSet<String>)],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Vec<ProviderLoad> {
    load_each(
        jobs.iter().map(|(account, labels)| (account, Some(labels))),
        cache,
        progress,
    )
}

fn load_each<'a>(
    jobs: impl Iterator<Item = (&'a AccountConfig, Option<&'a BTreeSet<String>>)> + Clone,
    cache: &MailCache,
    progress: &LoadProgress,
) -> Vec<ProviderLoad> {
    progress.begin(
        jobs.clone()
            .map(|(account, filter)| account.source_count(filter))
            .sum(),
    );
    jobs.map(|(account, filter)| ProviderLoad {
        provider: account.provider(),
        sources: account.load_sources_with(cache, progress, filter),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_slots_and_links_are_fixed() {
        assert_eq!(
            MailProvider::ALL,
            [MailProvider::Microsoft, MailProvider::Google]
        );
        assert_eq!(MailProvider::Microsoft.slot(), 0);
        assert_eq!(MailProvider::Google.slot(), 1);
        assert!(
            MailProvider::Microsoft.is_trusted_message_link("https://outlook.office.com/mail/")
        );
        assert!(MailProvider::Google.is_trusted_message_link("https://mail.google.com/mail/u/0/"));
        assert!(
            !MailProvider::Google.is_trusted_message_link("https://evil.example/mail.google.com/")
        );
        assert!(
            !MailProvider::Google.is_trusted_message_link("https://mail.google.com.evil.example/")
        );
    }

    #[test]
    fn reminder_requests_for_the_other_provider_are_rejected_without_network() {
        use super::super::reminders::{ReminderFailure, ReminderRequest};
        let request = |provider| ReminderRequest {
            provider,
            account: "synthetic-account".into(),
            title: "Review the draft".into(),
            at_utc: 4_000_000_000,
            marker: "a".repeat(64),
        };
        let microsoft =
            || ConnectionConfig::new("11111111-1111-4111-8111-111111111111", None).unwrap();
        let google = || {
            GoogleConfig::new("123-synthetic.apps.googleusercontent.com", "fixture-secret").unwrap()
        };
        let rejected = ReminderOutcome::NotCreated(ReminderFailure::InvalidDraft);
        assert_eq!(
            create_reminder(
                &AccountConfig::Google(google()),
                &request(MailProvider::Microsoft)
            ),
            rejected
        );
        assert_eq!(
            create_reminder(
                &AccountConfig::Microsoft(microsoft()),
                &request(MailProvider::Google)
            ),
            rejected
        );
        assert_eq!(
            super::super::google::tasks::create(&google(), &request(MailProvider::Microsoft)),
            rejected
        );
        assert_eq!(
            super::super::reminders::create(&microsoft(), &request(MailProvider::Google)),
            rejected
        );
    }

    #[test]
    fn load_all_keeps_going_after_provider_error() {
        let accounts = [
            AccountConfig::Google(GoogleConfig::invalid_for_test()),
            AccountConfig::Google(GoogleConfig::invalid_for_test()),
        ];
        let progress = LoadProgress::default();
        let loads = load_all(&accounts, &MailCache::default(), &progress);
        assert_eq!(loads.len(), 2);
        assert!(
            loads
                .iter()
                .all(|load| matches!(load.sources, Err(ConnectionError::InvalidConfiguration)))
        );
    }

    #[test]
    fn source_count_follows_configured_labels_and_filter() {
        let microsoft = AccountConfig::Microsoft(
            ConnectionConfig::new(
                "11111111-1111-4111-8111-111111111111",
                Some("shared@example.invalid"),
            )
            .and_then(|config| config.with_groups(Some("group@example.invalid")))
            .unwrap(),
        );
        assert_eq!(microsoft.source_count(None), 5);
        let filter = BTreeSet::from(["shared@example.invalid".to_owned()]);
        assert_eq!(microsoft.source_count(Some(&filter)), 2);
        let google = AccountConfig::Google(GoogleConfig::invalid_for_test());
        assert_eq!(google.source_count(None), 2);
        let filter = BTreeSet::from(["Gmail / Sent".to_owned()]);
        assert_eq!(google.source_count(Some(&filter)), 1);
    }

    #[test]
    fn load_all_begins_progress_once_with_every_account_total() {
        let accounts = [
            AccountConfig::Google(GoogleConfig::invalid_for_test()),
            AccountConfig::Google(GoogleConfig::invalid_for_test()),
        ];
        let progress = LoadProgress::default();
        progress
            .listed
            .store(9, std::sync::atomic::Ordering::Relaxed);
        progress
            .sources_done
            .store(3, std::sync::atomic::Ordering::Relaxed);
        let _ = load_all(&accounts, &MailCache::default(), &progress);
        assert_eq!(
            progress
                .sources_total
                .load(std::sync::atomic::Ordering::Relaxed),
            4
        );
        assert_eq!(
            progress.listed.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            progress
                .sources_done
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn load_selected_runs_each_account_with_its_own_labels() {
        let jobs = [
            (
                AccountConfig::Google(GoogleConfig::invalid_for_test()),
                BTreeSet::from(["Gmail / Inbox".to_owned()]),
            ),
            (
                AccountConfig::Google(GoogleConfig::invalid_for_test()),
                BTreeSet::from(["Gmail / Inbox".to_owned(), "Gmail / Sent".to_owned()]),
            ),
        ];
        let progress = LoadProgress::default();
        let loads = load_selected(&jobs, &MailCache::default(), &progress);
        assert_eq!(loads.len(), 2);
        assert!(
            loads
                .iter()
                .all(|load| load.provider == MailProvider::Google)
        );
        assert_eq!(
            progress
                .sources_total
                .load(std::sync::atomic::Ordering::Relaxed),
            3
        );
    }
}
