use std::collections::BTreeSet;

use super::google::GoogleConfig;
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
            Self::Google(config) => {
                debug_assert!(config.registration_is_present());
                Err(ConnectionError::ProviderUnavailable)
            }
        }
    }

    /// Loads recent sources through the configured provider.
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
                debug_assert!(config.registration_is_present());
                Err(ConnectionError::ProviderUnavailable)
            }
        }
    }
}

pub struct ProviderLoad {
    pub provider: MailProvider,
    pub sources: Result<Vec<SourceReview>, ConnectionError>,
}

pub fn load_all(
    accounts: &[AccountConfig],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Vec<ProviderLoad> {
    accounts
        .iter()
        .map(|account| ProviderLoad {
            provider: account.provider(),
            sources: account.load_recent_with(cache, progress),
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
    fn load_all_keeps_going_after_provider_error() {
        let accounts = [
            AccountConfig::Google(
                GoogleConfig::new("123-fixture.apps.googleusercontent.com", "fixture-secret")
                    .unwrap(),
            ),
            AccountConfig::Google(
                GoogleConfig::new("456-fixture.apps.googleusercontent.com", "fixture-secret")
                    .unwrap(),
            ),
        ];
        let loads = load_all(&accounts, &MailCache::default(), &LoadProgress::default());
        assert_eq!(loads.len(), 2);
        assert!(
            loads
                .iter()
                .all(|load| matches!(load.sources, Err(ConnectionError::ProviderUnavailable)))
        );
    }
}
