//! Google provider configuration. Network support is added in a later phase.

use super::{ConnectionError, Secret};

/// Runtime Google OAuth registration values. Deliberately has no `Debug` implementation.
pub struct GoogleConfig {
    client_id: String,
    client_secret: Secret,
}

impl GoogleConfig {
    /// Validates Google desktop OAuth registration values without echoing them.
    ///
    /// # Errors
    /// Returns a fixed configuration error for malformed values.
    pub fn new(client_id: &str, client_secret: &str) -> Result<Self, ConnectionError> {
        if !valid_client_id(client_id) || !valid_client_secret(client_secret) {
            return Err(ConnectionError::InvalidConfiguration);
        }
        Ok(Self {
            client_id: client_id.to_owned(),
            client_secret: Secret::new(client_secret.to_owned()),
        })
    }

    pub(super) fn registration_is_present(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.as_str().is_empty()
    }
}

pub(super) fn valid_client_id(value: &str) -> bool {
    let Some(stem) = value.strip_suffix(".apps.googleusercontent.com") else {
        return false;
    };
    let Some((digits, suffix)) = stem.split_once('-') else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && !suffix.is_empty()
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

pub(super) fn valid_client_secret(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_google_registration_without_exposing_values() {
        assert!(
            GoogleConfig::new("123-synthetic.apps.googleusercontent.com", "fixture-secret").is_ok()
        );
        for client_id in [
            "",
            "synthetic.apps.googleusercontent.com",
            "123-.apps.googleusercontent.com",
            "123-UPPER.apps.googleusercontent.com",
            "123-synthetic.example.invalid",
        ] {
            assert!(GoogleConfig::new(client_id, "fixture-secret").is_err());
        }
        assert!(GoogleConfig::new("123-synthetic.apps.googleusercontent.com", "").is_err());
        assert!(
            GoogleConfig::new("123-synthetic.apps.googleusercontent.com", "bad\nvalue").is_err()
        );
    }
}
