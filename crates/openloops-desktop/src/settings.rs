//! Setup preferences are a single current-user Windows Credential Manager record.
//! No plaintext files, account identifiers in credential names, or diagnostic payloads.
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"OpenLoopsSetup\x01";
const MAX_BYTES: usize = 2560; // Windows CRED_MAX_CREDENTIAL_BLOB_SIZE.

#[derive(Default, Clone)]
pub struct Settings {
    pub client_id: String,
    pub groups: String,
    pub shared: String,
    pub key: Zeroizing<String>,
    pub selected: String,
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
    fn encode(&self) -> Result<Zeroizing<Vec<u8>>, SettingsError> {
        let fields = [
            &self.client_id,
            &self.groups,
            &self.shared,
            &*self.key,
            &self.selected,
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

    fn decode(bytes: &[u8]) -> Result<Self, SettingsError> {
        if bytes.len() > MAX_BYTES {
            return Err(SettingsError::Invalid);
        }
        let mut remaining = bytes.strip_prefix(MAGIC).ok_or(SettingsError::Invalid)?;
        let mut next = || -> Result<&str, SettingsError> {
            let (length, tail) = remaining
                .split_at_checked(4)
                .ok_or(SettingsError::Invalid)?;
            let length =
                u32::from_le_bytes(length.try_into().map_err(|_| SettingsError::Invalid)?) as usize;
            let (field, tail) = tail
                .split_at_checked(length)
                .ok_or(SettingsError::Invalid)?;
            remaining = tail;
            std::str::from_utf8(field).map_err(|_| SettingsError::Invalid)
        };
        let settings = Self {
            client_id: next()?.to_owned(),
            groups: next()?.to_owned(),
            shared: next()?.to_owned(),
            key: Zeroizing::new(next()?.to_owned()),
            selected: next()?.to_owned(),
        };
        if !remaining.is_empty() {
            return Err(SettingsError::Invalid);
        }
        Ok(settings)
    }
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
        }
    }

    #[test]
    fn versioned_encoding_rejects_truncation_trailing_data_and_unknown_version() {
        let encoded = synthetic().encode().unwrap();
        for length in 0..encoded.len() {
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
