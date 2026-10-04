//! Google OAuth configuration and read-only Gmail connection support.

pub mod gmail;
mod mime;

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use reqwest::blocking::Client;
use url::Url;

use super::{
    ConnectionError, ConnectionReport, MailProvider, OAuthEndpoints, Secret, SharedScope,
    authorize_with, graph_client, run_with_session_for,
};
use crate::live::review::fetch_from_origin_with_headers;

const IDENTITY: [&str; 3] = ["openid", "email", "profile"];
const GMAIL_READ: &str = "https://www.googleapis.com/auth/gmail.readonly";
const TASKS: &str = "https://www.googleapis.com/auth/tasks";

fn google_scope(scope: &str) -> String {
    match scope {
        "https://www.googleapis.com/auth/userinfo.email" => "email".to_owned(),
        "https://www.googleapis.com/auth/userinfo.profile" => "profile".to_owned(),
        _ => scope.to_owned(),
    }
}

pub(super) fn google_endpoints(config: &GoogleConfig) -> OAuthEndpoints {
    OAuthEndpoints {
        authorize: "https://accounts.google.com/o/oauth2/v2/auth",
        token: "https://oauth2.googleapis.com/token",
        redirect_host: super::callback::RedirectHost::LoopbackIp,
        client_secret: Some(config.client_secret().clone()),
        extra_params: &[
            ("access_type", "online"),
            ("prompt", "select_account"),
            ("include_granted_scopes", "true"),
        ],
        normalize_scope: google_scope,
    }
}

pub(super) fn required_scopes(reminders: bool) -> BTreeSet<String> {
    let mut scopes: BTreeSet<String> = IDENTITY.map(str::to_owned).into_iter().collect();
    scopes.insert(if reminders { TASKS } else { GMAIL_READ }.to_owned());
    scopes
}

/// Runtime Google OAuth registration values. Deliberately has no `Debug` implementation.
pub struct GoogleConfig {
    client_id: String,
    client_secret: Secret,
}

pub(super) fn with_google_session<T>(
    config: &GoogleConfig,
    reminders: bool,
    mut work: impl FnMut(&Client, &str) -> Result<T, ConnectionError>,
) -> Result<T, ConnectionError> {
    if !valid_client_id(config.client_id()) || !valid_client_secret(config.client_secret().as_str())
    {
        return Err(ConnectionError::InvalidConfiguration);
    }
    super::CONNECTING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| ConnectionError::AlreadyConnecting)?;
    let _guard = super::ConnectionGuard;
    let http = graph_client()?;
    let endpoints = google_endpoints(config);
    run_with_session_for(
        MailProvider::Google,
        required_scopes(reminders),
        |scopes| authorize_with(&endpoints, config.client_id(), scopes),
        |token, _| work(&http, token),
    )
}

fn check_from_origins(
    http: &Client,
    token: &str,
    userinfo_origin: &str,
    gmail_origin: &str,
) -> Result<ConnectionReport, ConnectionError> {
    let _ = gmail::identity_from_origin(http, token, userinfo_origin)?;
    let url = Url::parse(gmail_origin)
        .and_then(|url| url.join("gmail/v1/users/me/profile"))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let _ = fetch_from_origin_with_headers(http, token, &url, gmail_origin, &[])?;
    Ok(ConnectionReport {
        own_inbox_accessible: true,
        shared_inbox_results: vec![],
        shared_scope: SharedScope::NotReported,
        group_inbox_results: vec![],
    })
}

/// Checks OIDC identity and read-only Gmail profile access.
///
/// # Errors
/// Returns only fixed, content-free configuration, authorization, or transport errors.
pub fn check_connection(config: &GoogleConfig) -> Result<ConnectionReport, ConnectionError> {
    with_google_session(config, false, |http, token| {
        check_from_origins(http, token, gmail::USERINFO_ORIGIN, gmail::GMAIL_ORIGIN)
    })
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

    pub(super) fn client_id(&self) -> &str {
        &self.client_id
    }

    pub(super) fn client_secret(&self) -> &Secret {
        &self.client_secret
    }

    #[cfg(test)]
    pub(super) fn invalid_for_test() -> Self {
        Self {
            client_id: String::new(),
            client_secret: Secret::new(String::new()),
        }
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
    use crate::live::test_support::routed_server;
    use std::time::{Duration, Instant};

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

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

    #[test]
    fn scopes_and_endpoints_are_provider_specific() {
        let mail = required_scopes(false);
        assert!(IDENTITY.iter().all(|scope| mail.contains(*scope)));
        assert!(mail.contains(GMAIL_READ));
        assert!(!mail.contains(TASKS));
        let tasks = required_scopes(true);
        assert!(tasks.contains(TASKS));
        assert!(!tasks.contains(GMAIL_READ));

        let config = GoogleConfig::new(
            "123-synthetic.apps.googleusercontent.com",
            "synthetic-client-parameter",
        )
        .unwrap();
        let endpoints = google_endpoints(&config);
        assert_eq!(
            endpoints.authorize,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(endpoints.token, "https://oauth2.googleapis.com/token");
        assert_eq!(
            endpoints.extra_params,
            [
                ("access_type", "online"),
                ("prompt", "select_account"),
                ("include_granted_scopes", "true")
            ]
        );
        assert!(matches!(
            endpoints.redirect_host,
            super::super::callback::RedirectHost::LoopbackIp
        ));
        assert_eq!(
            endpoints.client_secret.unwrap().as_str(),
            "synthetic-client-parameter"
        );
    }

    #[test]
    fn google_granted_identity_scope_aliases_cover_required_scopes() {
        let scopes = [
            "openid",
            "https://www.googleapis.com/auth/userinfo.email",
            "https://www.googleapis.com/auth/userinfo.profile",
            GMAIL_READ,
        ]
        .map(google_scope)
        .into_iter()
        .collect();
        let now = Instant::now();
        let session = super::super::Session {
            access_token: Secret::new("synthetic-token".to_owned()),
            expires_at: now + Duration::from_mins(2),
            scopes,
            shared: SharedScope::NotReported,
        };
        assert!(super::super::session_covers(
            &session,
            &required_scopes(false),
            now
        ));
    }

    #[test]
    fn routed_connection_check_succeeds_and_maps_unauthorized() {
        let identity = serde_json::json!({
            "sub":"synthetic-sub",
            "email":"user@example.invalid"
        })
        .to_string();
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity)),
            ("/gmail/v1/users/me/profile", response("200 OK", "{}")),
        ]);
        let http = Client::builder().no_proxy().build().unwrap();
        let report = check_from_origins(&http, "synthetic-token", &origin, &origin).unwrap();
        server.join().unwrap();
        assert!(report.own_inbox_accessible);
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity)),
            (
                "/gmail/v1/users/me/profile",
                response("401 Unauthorized", ""),
            ),
        ]);
        let result = check_from_origins(&http, "synthetic-token", &origin, &origin);
        server.join().unwrap();
        assert_eq!(result, Err(ConnectionError::Unauthorized));
    }
}
