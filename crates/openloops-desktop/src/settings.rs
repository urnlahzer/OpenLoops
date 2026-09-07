//! Setup preferences are a single current-user Windows Credential Manager record.
//! No plaintext files, account identifiers in credential names, or diagnostic payloads.
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"OpenLoopsSetup\x01";
const MAX_BYTES: usize = 2560; // Windows CRED_MAX_CREDENTIAL_BLOB_SIZE.

/// The model provider a scan is sent to. One is selected at a time; each
/// keeps its own key and model choice so switching back does not lose them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Provider {
    #[default]
    OllamaCloud,
    OpenRouter,
}

impl Provider {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OllamaCloud => "Ollama Cloud",
            Self::OpenRouter => "OpenRouter",
        }
    }

    const fn tag(self) -> &'static str {
        match self {
            Self::OllamaCloud => "ollama_cloud",
            Self::OpenRouter => "openrouter",
        }
    }

    fn parse(tag: &str) -> Result<Self, SettingsError> {
        match tag {
            "ollama_cloud" => Ok(Self::OllamaCloud),
            "openrouter" => Ok(Self::OpenRouter),
            _ => Err(SettingsError::Invalid),
        }
    }
}

#[derive(Default, Clone)]
pub struct Settings {
    pub client_id: String,
    pub groups: String,
    pub shared: String,
    pub key: Zeroizing<String>,
    pub selected: String,
    pub provider: Provider,
    pub openrouter_key: Zeroizing<String>,
    pub openrouter_selected: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Unavailable,
    Invalid,
    TooLarge,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Windows Credential Manager is unavailable. Your changes have not been saved. Try Save settings again.",
            Self::Invalid => "Saved settings could not be read. Automatic saving is paused to preserve them. Use Reload saved settings to retry, or Save settings to replace them with these inputs.",
            Self::TooLarge => "Settings exceed Windows Credential Manager's 2,560-byte limit. Shorten the inbox list before saving. Previous saved settings are unchanged.",
        })
    }
}

pub trait SettingsStore {
    fn load(&self) -> Result<Option<Settings>, SettingsError>;
    fn save(&self, settings: &Settings) -> Result<(), SettingsError>;
    fn delete(&self) -> Result<(), SettingsError>;
}

impl Settings {
    /// The key for the currently selected provider.
    #[must_use]
    pub fn active_key(&self) -> &Zeroizing<String> {
        match self.provider {
            Provider::OllamaCloud => &self.key,
            Provider::OpenRouter => &self.openrouter_key,
        }
    }

    /// The model label chosen for the currently selected provider.
    #[must_use]
    pub fn active_model(&self) -> &str {
        match self.provider {
            Provider::OllamaCloud => &self.selected,
            Provider::OpenRouter => &self.openrouter_selected,
        }
    }

    fn encode(&self) -> Result<Zeroizing<Vec<u8>>, SettingsError> {
        let fields = [
            self.client_id.as_str(),
            self.groups.as_str(),
            self.shared.as_str(),
            self.key.as_str(),
            self.selected.as_str(),
            self.provider.tag(),
            self.openrouter_key.as_str(),
            self.openrouter_selected.as_str(),
        ];
        let size = fields
            .iter()
            .try_fold(MAGIC.len(), |size, field| {
                size.checked_add(4)?.checked_add(field.len())
            })
            .filter(|size| *size <= MAX_BYTES)
            .ok_or(SettingsError::TooLarge)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(size));
        bytes.extend_from_slice(MAGIC);
        for field in fields {
            let length = u32::try_from(field.len()).map_err(|_| SettingsError::TooLarge)?;
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(field.as_bytes());
        }
        Ok(bytes)
    }

    /// Records written before the provider selection existed end after the
    /// fifth field; they load as Ollama Cloud with no `OpenRouter` inputs.
    fn decode(bytes: &[u8]) -> Result<Self, SettingsError> {
        if bytes.len() > MAX_BYTES {
            return Err(SettingsError::Invalid);
        }
        let mut remaining = bytes.strip_prefix(MAGIC).ok_or(SettingsError::Invalid)?;
        let mut settings = Self {
            client_id: next(&mut remaining)?,
            groups: next(&mut remaining)?,
            shared: next(&mut remaining)?,
            key: Zeroizing::new(next(&mut remaining)?),
            selected: next(&mut remaining)?,
            ..Self::default()
        };
        if !remaining.is_empty() {
            settings.provider = Provider::parse(&next(&mut remaining)?)?;
            settings.openrouter_key = Zeroizing::new(next(&mut remaining)?);
            settings.openrouter_selected = next(&mut remaining)?;
        }
        if !remaining.is_empty() {
            return Err(SettingsError::Invalid);
        }
        Ok(settings)
    }
}

fn next(remaining: &mut &[u8]) -> Result<String, SettingsError> {
    let (length, tail) = remaining
        .split_at_checked(4)
        .ok_or(SettingsError::Invalid)?;
    let length =
        u32::from_le_bytes(length.try_into().map_err(|_| SettingsError::Invalid)?) as usize;
    let (field, tail) = tail
        .split_at_checked(length)
        .ok_or(SettingsError::Invalid)?;
    *remaining = tail;
    std::str::from_utf8(field)
        .map(str::to_owned)
        .map_err(|_| SettingsError::Invalid)
}

#[cfg(windows)]
struct WindowsStore {
    entry: keyring_core::Entry,
}

#[cfg(windows)]
impl WindowsStore {
    fn new(target: &str) -> Result<Self, SettingsError> {
        use keyring_core::api::CredentialStoreApi;
        let store =
            windows_native_keyring_store::Store::new().map_err(|_| SettingsError::Unavailable)?;
        let modifiers =
            std::collections::HashMap::from([("target", target), ("persistence", "Local")]);
        let entry = store
            .build("OpenLoops", "setup", Some(&modifiers))
            .map_err(|_| SettingsError::Unavailable)?;
        Ok(Self { entry })
    }
}

#[cfg(windows)]
impl SettingsStore for WindowsStore {
    fn load(&self) -> Result<Option<Settings>, SettingsError> {
        match self.entry.get_secret() {
            Ok(bytes) => Settings::decode(&Zeroizing::new(bytes)).map(Some),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(SettingsError::Unavailable),
        }
    }

    fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let encoded = settings.encode()?;
        self.entry
            .set_secret(&encoded)
            .map_err(|_| SettingsError::Unavailable)
    }

    fn delete(&self) -> Result<(), SettingsError> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err(SettingsError::Unavailable),
        }
    }
}

// Screenshot builds and UI unit tests must never read or modify real saved inputs.
pub fn production_store() -> Result<Option<Box<dyn SettingsStore>>, SettingsError> {
    if cfg!(any(test, feature = "ui-screenshot")) {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        Ok(Some(Box::new(WindowsStore::new("OpenLoops/Setup/v1")?)))
    }
    #[cfg(not(windows))]
    {
        Err(SettingsError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic() -> Settings {
        Settings {
            client_id: "00000000-0000-0000-0000-000000000000".into(),
            groups: "hello@example.invalid\ninvestors@example.invalid\ncareers@example.invalid"
                .into(),
            shared: "shared@example.invalid".into(),
            key: Zeroizing::new("synthetic-key".into()),
            selected: "deepseek-v4-flash:0731".into(),
            provider: Provider::OpenRouter,
            openrouter_key: Zeroizing::new("synthetic-openrouter-key".into()),
            openrouter_selected: "vendor/model-1".into(),
        }
    }

    /// The five-field layout written before the provider selection existed.
    fn legacy_record() -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for field in [
            "00000000-0000-0000-0000-000000000000",
            "hello@example.invalid",
            "shared@example.invalid",
            "synthetic-key",
            "deepseek-v4-flash:0731",
        ] {
            bytes.extend_from_slice(&u32::try_from(field.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(field.as_bytes());
        }
        bytes
    }

    #[test]
    fn versioned_encoding_rejects_truncation_trailing_data_and_unknown_version() {
        let encoded = synthetic().encode().unwrap();
        let settings = synthetic();
        let legacy_end = MAGIC.len()
            + [
                settings.client_id.as_str(),
                settings.groups.as_str(),
                settings.shared.as_str(),
                settings.key.as_str(),
                settings.selected.as_str(),
            ]
            .iter()
            .map(|field| 4 + field.len())
            .sum::<usize>();
        for length in 0..encoded.len() {
            if length == legacy_end {
                // A record truncated exactly at the pre-provider boundary is
                // by construction identical to one written before the
                // provider selection existed, and loads as that.
                let truncated = Settings::decode(&encoded[..length]).unwrap();
                assert_eq!(truncated.provider, Provider::OllamaCloud);
                assert!(truncated.openrouter_key.is_empty());
                continue;
            }
            assert!(Settings::decode(&encoded[..length]).is_err());
        }
        let mut bad = encoded.to_vec();
        bad.push(0);
        assert!(Settings::decode(&bad).is_err());
        bad = encoded.to_vec();
        bad[MAGIC.len() - 1] = 2;
        assert!(Settings::decode(&bad).is_err());
        let decoded = Settings::decode(&encoded).unwrap();
        assert_eq!(decoded.groups, synthetic().groups);
        assert_eq!(&*decoded.key, "synthetic-key");
        assert_eq!(decoded.provider, Provider::OpenRouter);
        assert_eq!(&*decoded.openrouter_key, "synthetic-openrouter-key");
        assert_eq!(decoded.openrouter_selected, "vendor/model-1");
    }

    #[test]
    fn records_written_before_the_provider_selection_still_load() {
        let decoded = Settings::decode(&legacy_record()).unwrap();
        assert_eq!(decoded.selected, "deepseek-v4-flash:0731");
        assert_eq!(&*decoded.key, "synthetic-key");
        assert_eq!(decoded.provider, Provider::OllamaCloud);
        assert!(decoded.openrouter_key.is_empty());
        assert!(decoded.openrouter_selected.is_empty());
        // Re-saving upgrades the record in place without changing the inputs.
        let upgraded = Settings::decode(&decoded.encode().unwrap()).unwrap();
        assert_eq!(upgraded.provider, Provider::OllamaCloud);
        assert_eq!(upgraded.selected, decoded.selected);
        assert_eq!(&*upgraded.key, "synthetic-key");
    }

    #[test]
    fn an_unknown_or_truncated_provider_tag_is_rejected() {
        let mut unknown = legacy_record();
        for field in ["synthetic_provider", "", ""] {
            unknown.extend_from_slice(&u32::try_from(field.len()).unwrap().to_le_bytes());
            unknown.extend_from_slice(field.as_bytes());
        }
        assert_eq!(
            Settings::decode(&unknown).err(),
            Some(SettingsError::Invalid)
        );
        let mut short = legacy_record();
        short.extend_from_slice(&u32::try_from("openrouter".len()).unwrap().to_le_bytes());
        short.extend_from_slice(b"openrouter");
        assert_eq!(Settings::decode(&short).err(), Some(SettingsError::Invalid));
    }

    #[test]
    fn size_is_bounded_before_serializing_private_values() {
        let mut settings = synthetic();
        settings.groups = "x".repeat(MAX_BYTES);
        assert!(matches!(settings.encode(), Err(SettingsError::TooLarge)));
        assert!(Settings::decode(&vec![0; MAX_BYTES + 1]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_credential_survives_reopen_update_and_delete() {
        struct Cleanup(WindowsStore);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete();
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let target = format!("OpenLoops/Test/{}-{unique}", std::process::id());
        let store = WindowsStore::new(&target).unwrap();
        let cleanup = Cleanup(store);
        assert!(cleanup.0.load().unwrap().is_none());
        cleanup.0.save(&synthetic()).unwrap();
        // A fresh process proves that this is durable OS storage, not an in-process cache.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "settings::tests::read_synthetic_settings_in_new_process",
                "--ignored",
            ])
            .env("OPENLOOPS_TEST_CREDENTIAL", &target)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "fresh-process settings check failed"
        );
        assert!(
            String::from_utf8_lossy(&child.stdout).contains("1 passed"),
            "fresh-process check must run its test"
        );
        let reopened = WindowsStore::new(&target).unwrap();
        let mut loaded = reopened.load().unwrap().unwrap();
        assert_eq!(loaded.client_id, synthetic().client_id);
        assert_eq!(loaded.groups, synthetic().groups);
        assert_eq!(loaded.shared, synthetic().shared);
        assert_eq!(&*loaded.key, "synthetic-key");
        assert_eq!(loaded.selected, synthetic().selected);
        assert_eq!(
            reopened
                .entry
                .get_attributes()
                .unwrap()
                .get("persistence")
                .unwrap(),
            "Local"
        );
        loaded.selected = "other-model".into();
        reopened.save(&loaded).unwrap();
        assert_eq!(cleanup.0.load().unwrap().unwrap().selected, "other-model");
        loaded.groups = "x".repeat(MAX_BYTES);
        assert_eq!(reopened.save(&loaded), Err(SettingsError::TooLarge));
        assert_eq!(cleanup.0.load().unwrap().unwrap().selected, "other-model");
        reopened.delete().unwrap();
        assert!(cleanup.0.load().unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "parent test provides an isolated synthetic credential"]
    fn read_synthetic_settings_in_new_process() {
        let target = std::env::var("OPENLOOPS_TEST_CREDENTIAL").unwrap();
        assert!(target.starts_with("OpenLoops/Test/"));
        let loaded = WindowsStore::new(&target).unwrap().load().unwrap().unwrap();
        assert_eq!(loaded.client_id, synthetic().client_id);
        assert_eq!(loaded.groups, synthetic().groups);
        assert_eq!(loaded.shared, synthetic().shared);
        assert_eq!(&*loaded.key, "synthetic-key");
        assert_eq!(loaded.selected, synthetic().selected);
    }
}
