//! Session-only Microsoft connection. No token or response is persisted.
//! The organizations authority supports commercial work/school tenants.
//! This module does not enable background processing or reminder writes.

mod callback;
mod groups;
pub mod reminders;
pub mod review;

use std::collections::BTreeSet;
use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use reqwest::blocking::Client;
use url::Url;
const AUTHORIZE: &str = "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/organizations/oauth2/v2.0/token";
const MAX_RESPONSE: u64 = 1024 * 1024;
const TOKEN_TIMEOUT_SECONDS: u64 = 30;
const GRAPH_TIMEOUT_SECONDS: u64 = 90;
static CONNECTING: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

struct Session {
    access_token: Secret,
    expires_at: Instant,
    scopes: BTreeSet<String>,
    shared: SharedScope,
}

#[derive(Clone)]
struct Secret(Vec<u8>);

impl Secret {
    fn new(value: String) -> Self {
        Self(value.into_bytes())
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("secret originated as valid UTF-8")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.iter_mut().for_each(|byte| *byte = 0);
    }
}

/// Closed errors: never contain URLs, identifiers, tokens, or server bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionError {
    InvalidConfiguration,
    AlreadyConnecting,
    BrowserUnavailable,
    CallbackUnavailable,
    SignInTimedOut,
    ConsentDenied,
    Transport,
    Timeout(u64),
    TokenRejected,
    ResponseTooLarge,
    MessageTooLarge,
    AccessDenied,
    Unauthorized,
    MissingSharedScope,
    Throttled,
    ResourceUnavailable,
    GroupNotFound,
    BadRequest,
    NotFound,
    ServerError,
    NextPageRejected,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => {
                "Check the application ID and optional shared mailbox setting."
            }
            Self::AlreadyConnecting => "A connection is already in progress.",
            Self::BrowserUnavailable => "The system browser could not be opened.",
            Self::CallbackUnavailable => "The local sign-in listener is unavailable.",
            Self::SignInTimedOut => "Sign-in timed out. Run the connection command again.",
            Self::ConsentDenied => "Microsoft sign-in was declined or could not be completed.",
            Self::Transport => "Microsoft could not be reached over a secure connection.",
            Self::Timeout(seconds) => {
                return write!(f, "Microsoft did not answer within {seconds} seconds.");
            }
            Self::TokenRejected => {
                "Microsoft rejected the sign-in exchange. Check the registration and redirect URI."
            }
            Self::ResponseTooLarge => "Microsoft returned a response above the allowed size.",
            Self::MessageTooLarge => {
                "A message body exceeded the review size limit and was skipped."
            }
            Self::AccessDenied => "Microsoft Graph returned HTTP 403. Check consent and this signed-in account's access to the selected mailbox or group; an administrator role alone does not grant content access.",
            Self::Unauthorized => "Microsoft Graph returned HTTP 401. Sign in again; if it persists, check the organization's access policies.",
            Self::MissingSharedScope => "The token response did not grant Mail.Read.Shared. Check the app's delegated permissions and consent, then sign in again.",
            Self::Throttled => "Microsoft Graph returned HTTP 429. Wait before retrying the connection check.",
            Self::ResourceUnavailable => "The requested mailbox or resource is unavailable.",
            Self::GroupNotFound => "No unique Microsoft 365 Group matched that primary email address. Check the group's primary address and directory-read consent.",
            Self::BadRequest => "Microsoft Graph rejected the request as malformed (HTTP 400).",
            Self::NotFound => {
                "Microsoft Graph reported the requested resource does not exist (HTTP 404)."
            }
            Self::ServerError => {
                "Microsoft Graph reported a server-side failure (HTTP 5xx). Retry later."
            }
            Self::NextPageRejected => "Microsoft returned a next-page link outside the authorized collection; remaining pages were skipped.",
        })
    }
}

impl std::error::Error for ConnectionError {}

/// Removes the process-only Microsoft access token, if one is present.
pub fn clear_session() {
    *session_store() = None;
}

/// Reports whether a reusable, unexpired Microsoft session is in memory.
#[must_use]
pub fn has_session() -> bool {
    session_store()
        .as_ref()
        .is_some_and(|session| session_is_fresh(session, Instant::now()))
}

fn session_store() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn session_is_fresh(session: &Session, now: Instant) -> bool {
    session
        .expires_at
        .checked_duration_since(now)
        .is_some_and(|remaining| remaining > Duration::from_mins(1))
}

fn session_covers(session: &Session, required: &BTreeSet<String>, now: Instant) -> bool {
    session_is_fresh(session, now) && session.scopes.is_superset(required)
}

/// Values are supplied at runtime and deliberately have no Debug implementation.
pub struct ConnectionConfig {
    client_id: String,
    shared_mailboxes: Vec<String>,
    group_inboxes: Vec<String>,
}

impl ConnectionConfig {
    /// Validates runtime configuration without echoing supplied values.
    /// # Errors
    /// Returns a fixed configuration error for malformed inputs.
    pub fn new(client_id: &str, shared_mailbox: Option<&str>) -> Result<Self, ConnectionError> {
        if !valid_application_id(client_id) {
            return Err(ConnectionError::InvalidConfiguration);
        }
        let mut shared_mailboxes = Vec::<String>::new();
        for value in shared_mailbox
            .unwrap_or_default()
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if value.len() > 254
                || value.matches('@').count() != 1
                || value.starts_with('@')
                || value.ends_with('@')
                || value.chars().any(|c| c.is_control() || c.is_whitespace())
                || value.contains(['/', '\\', '?', '#'])
            {
                return Err(ConnectionError::InvalidConfiguration);
            }
            if !shared_mailboxes
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(value))
            {
                shared_mailboxes.push(value.to_owned());
            }
        }
        Ok(Self {
            client_id: client_id.to_owned(),
            shared_mailboxes,
            group_inboxes: Vec::new(),
        })
    }

    /// Adds explicitly selected Microsoft 365 Groups, using primary email addresses.
    /// # Errors
    /// Returns a sanitized configuration error for malformed addresses.
    pub fn with_groups(mut self, addresses: Option<&str>) -> Result<Self, ConnectionError> {
        self.group_inboxes = Self::new(&self.client_id, addresses)?.shared_mailboxes;
        Ok(self)
    }
}

fn valid_application_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        && value.bytes().any(|b| b.is_ascii_hexdigit() && b != b'0')
}

struct ConnectionGuard;
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        CONNECTING.store(false, Ordering::Release);
    }
}

/// Contains only success indicators, never account identifiers or message data.
#[derive(Debug, PartialEq, Eq)]
pub struct ConnectionReport {
    pub own_inbox_accessible: bool,
    pub shared_inbox_results: Vec<Result<(), ConnectionError>>,
    pub shared_scope: SharedScope,
    pub group_inbox_results: Vec<Result<(), ConnectionError>>,
}

/// Only a fixed permission classification is exposed, never token contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedScope {
    Granted,
    Missing,
    NotReported,
}

fn shared_scope(scopes: Option<&Vec<Scope>>) -> SharedScope {
    match scopes {
        None => SharedScope::NotReported,
        Some(scopes)
            if scopes.iter().any(|scope| {
                let value = scope
                    .as_str()
                    .strip_prefix("https://graph.microsoft.com/")
                    .unwrap_or(scope.as_str());
                matches!(value, "Mail.Read.Shared" | "Mail.ReadWrite.Shared")
            }) =>
        {
            SharedScope::Granted
        }
        Some(_) => SharedScope::Missing,
    }
}

/// Performs browser PKCE sign-in and bounded, read-only inbox access checks.
/// No refresh token, To Do permission, mail mutation, or model transmission is requested.
/// # Errors
/// Returns sanitized errors; upstream error text is never forwarded.
pub fn check_connection(config: &ConnectionConfig) -> Result<ConnectionReport, ConnectionError> {
    with_session(config, |http, token, scopes| {
        check_inbox(http, token, None)?;
        let shared_inbox_results = check_shared_inboxes(&config.shared_mailboxes, |mailbox| {
            if scopes == SharedScope::Missing {
                Err(ConnectionError::MissingSharedScope)
            } else {
                check_inbox(http, token, Some(mailbox))
            }
        });
        let group_inbox_results = check_shared_inboxes(&config.group_inboxes, |address| {
            groups::check(http, token, address)
        });
        Ok(ConnectionReport {
            own_inbox_accessible: true,
            shared_inbox_results,
            shared_scope: scopes,
            group_inbox_results,
        })
    })
}

fn with_session<T>(
    config: &ConnectionConfig,
    work: impl FnMut(&Client, &str, SharedScope) -> Result<T, ConnectionError>,
) -> Result<T, ConnectionError> {
    with_scopes(config, false, work)
}

fn with_scopes<T>(
    config: &ConnectionConfig,
    reminders: bool,
    mut work: impl FnMut(&Client, &str, SharedScope) -> Result<T, ConnectionError>,
) -> Result<T, ConnectionError> {
    CONNECTING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| ConnectionError::AlreadyConnecting)?;
    let _guard = ConnectionGuard;
    let required = required_scopes(config, reminders);
    let http = graph_client()?;
    run_with_session(
        required,
        |scopes| authorize(config, scopes),
        |token, shared| work(&http, token, shared),
    )
}

fn run_with_session<T>(
    required: BTreeSet<String>,
    mut authorize_session: impl FnMut(BTreeSet<String>) -> Result<Session, ConnectionError>,
    mut work: impl FnMut(&str, SharedScope) -> Result<T, ConnectionError>,
) -> Result<T, ConnectionError> {
    let mut prior_scopes = BTreeSet::new();
    let cached = {
        let mut stored = session_store();
        if let Some(session) = stored.as_ref() {
            prior_scopes.clone_from(&session.scopes);
        }
        if stored
            .as_ref()
            .is_some_and(|session| session_covers(session, &required, Instant::now()))
        {
            stored
                .as_ref()
                .map(|session| (session.access_token.clone(), session.shared))
        } else {
            *stored = None;
            None
        }
    };
    if let Some((token, shared)) = cached {
        match work(token.as_str(), shared) {
            Err(ConnectionError::Unauthorized) => clear_session(),
            result => return result,
        }
    }
    prior_scopes.extend(required);
    let session = authorize_session(prior_scopes)?;
    let token = session.access_token.clone();
    let shared = session.shared;
    *session_store() = Some(session);
    let result = work(token.as_str(), shared);
    if matches!(result, Err(ConnectionError::Unauthorized)) {
        clear_session();
    }
    result
}

fn required_scopes(config: &ConnectionConfig, reminders: bool) -> BTreeSet<String> {
    let mut scopes = BTreeSet::from(["https://graph.microsoft.com/User.Read".to_owned()]);
    let mail_scope = if reminders {
        "Tasks.ReadWrite"
    } else if config.shared_mailboxes.is_empty() {
        "Mail.Read"
    } else {
        "Mail.Read.Shared"
    };
    scopes.insert(format!("https://graph.microsoft.com/{mail_scope}"));
    if !reminders && !config.group_inboxes.is_empty() {
        for scope in ["Group.ReadBasic.All", "Group-Conversation.Read.All"] {
            scopes.insert(format!("https://graph.microsoft.com/{scope}"));
        }
    }
    scopes
}

fn graph_client() -> Result<Client, ConnectionError> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(GRAPH_TIMEOUT_SECONDS))
        .build()
        .map_err(|_| ConnectionError::Transport)
}

fn authorize(
    config: &ConnectionConfig,
    requested_scopes: BTreeSet<String>,
) -> Result<Session, ConnectionError> {
    let listener = callback::Listener::bind()?;
    let client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(AUTHORIZE.to_owned())
                .map_err(|_| ConnectionError::InvalidConfiguration)?,
        )
        .set_token_uri(
            TokenUrl::new(TOKEN.to_owned()).map_err(|_| ConnectionError::InvalidConfiguration)?,
        )
        .set_redirect_uri(
            RedirectUrl::new(listener.redirect_uri())
                .map_err(|_| ConnectionError::InvalidConfiguration)?,
        );
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorization_request = client.authorize_url(CsrfToken::new_random);
    for scope in &requested_scopes {
        authorization_request = authorization_request.add_scope(Scope::new(scope.clone()));
    }
    let authorization_request = authorization_request.set_pkce_challenge(challenge);
    let (authorization, state) = authorization_request
        .add_extra_param("response_mode", "query")
        .add_extra_param("prompt", "select_account")
        .url();
    let token_http = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(TOKEN_TIMEOUT_SECONDS))
        .build()
        .map_err(|_| ConnectionError::Transport)?;
    webbrowser::open(authorization.as_str()).map_err(|_| ConnectionError::BrowserUnavailable)?;
    let code = listener.wait(&state, Duration::from_mins(5))?;
    let exchange = |request: oauth2::HttpRequest| -> Result<oauth2::HttpResponse, ConnectionError> {
        // OAuth constructs the request, but the transport independently confines its destination.
        if request.uri() != TOKEN {
            return Err(ConnectionError::InvalidConfiguration);
        }
        let response = token_http
            .post(TOKEN)
            .headers(request.headers().clone())
            .body(request.body().clone())
            .send()
            .map_err(|error| request_error(&error, TOKEN_TIMEOUT_SECONDS))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(ConnectionError::TokenRejected);
        }
        let headers = response.headers().clone();
        let body = bounded_body_with_timeout(response, TOKEN_TIMEOUT_SECONDS)?;
        let mut result = oauth2::HttpResponse::new(body);
        *result.status_mut() = status;
        *result.headers_mut() = headers;
        Ok(result)
    };
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request(&exchange)
        .map_err(|error| match error {
            oauth2::RequestTokenError::Request(error) => error,
            _ => ConnectionError::TokenRejected,
        })?;
    if token.token_type() != &oauth2::basic::BasicTokenType::Bearer {
        return Err(ConnectionError::TokenRejected);
    }
    let expires_in = token.expires_in().ok_or(ConnectionError::TokenRejected)?;
    let scopes = token.scopes().map_or(requested_scopes, |granted| {
        granted
            .iter()
            .map(|scope| {
                let value = scope.as_str();
                if value.starts_with("https://graph.microsoft.com/") {
                    value.to_owned()
                } else {
                    format!("https://graph.microsoft.com/{value}")
                }
            })
            .collect()
    });
    Ok(Session {
        access_token: Secret::new(token.access_token().secret().to_owned()),
        expires_at: Instant::now()
            .checked_add(expires_in)
            .ok_or(ConnectionError::TokenRejected)?,
        scopes,
        shared: shared_scope(token.scopes()),
    })
}

pub(super) fn request_error(error: &reqwest::Error, seconds: u64) -> ConnectionError {
    if error.is_timeout() {
        ConnectionError::Timeout(seconds)
    } else {
        ConnectionError::Transport
    }
}

fn check_shared_inboxes(
    mailboxes: &[String],
    mut check: impl FnMut(&str) -> Result<(), ConnectionError>,
) -> Vec<Result<(), ConnectionError>> {
    mailboxes.iter().map(|mailbox| check(mailbox)).collect()
}

fn inbox_url(mailbox: Option<&str>) -> Result<Url, ConnectionError> {
    let mut url = Url::parse("https://graph.microsoft.com/v1.0/")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?;
        path.pop_if_empty();
        match mailbox {
            None => {
                path.push("me");
            }
            Some(value) => {
                path.push("users").push(value);
            }
        }
        path.extend(["mailFolders", "inbox", "messages"]);
    }
    url.query_pairs_mut()
        .append_pair("$top", "1")
        .append_pair("$select", "id");
    Ok(url)
}

fn check_inbox(http: &Client, token: &str, mailbox: Option<&str>) -> Result<(), ConnectionError> {
    let response = http
        .get(inbox_url(mailbox)?)
        .bearer_auth(token)
        .send()
        .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
    match response.status().as_u16() {
        200 => {
            let _ = bounded_body(response)?;
            Ok(())
        }
        status => Err(classify_status(status)),
    }
}

fn classify_status(status: u16) -> ConnectionError {
    match status {
        400 => ConnectionError::BadRequest,
        401 => ConnectionError::Unauthorized,
        403 => ConnectionError::AccessDenied,
        404 => ConnectionError::NotFound,
        429 => ConnectionError::Throttled,
        500..=599 => ConnectionError::ServerError,
        _ => ConnectionError::ResourceUnavailable,
    }
}

fn bounded_body(response: reqwest::blocking::Response) -> Result<Vec<u8>, ConnectionError> {
    bounded_body_with_timeout(response, GRAPH_TIMEOUT_SECONDS)
}

fn bounded_body_with_timeout(
    response: reqwest::blocking::Response,
    timeout_seconds: u64,
) -> Result<Vec<u8>, ConnectionError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE)
    {
        return Err(ConnectionError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut body)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                ConnectionError::Timeout(timeout_seconds)
            } else {
                ConnectionError::Transport
            }
        })?;
    if body.len() as u64 > MAX_RESPONSE {
        return Err(ConnectionError::ResponseTooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    const APP: &str = "11111111-1111-4111-8111-111111111111";
    static SESSION_TEST: Mutex<()> = Mutex::new(());

    fn synthetic_session(scopes: BTreeSet<String>, lifetime: Duration) -> Session {
        Session {
            access_token: Secret::new("synthetic-token".to_owned()),
            expires_at: Instant::now() + lifetime,
            scopes,
            shared: SharedScope::NotReported,
        }
    }

    #[test]
    fn required_scope_sets_preserve_incremental_consent() {
        let mail = ConnectionConfig::new(APP, None)
            .unwrap()
            .with_groups(Some("group@example.invalid"))
            .unwrap();
        let mail_scopes = required_scopes(&mail, false);
        assert!(mail_scopes.contains("https://graph.microsoft.com/User.Read"));
        assert!(mail_scopes.contains("https://graph.microsoft.com/Mail.Read"));
        assert!(mail_scopes.contains("https://graph.microsoft.com/Group.ReadBasic.All"));
        assert!(mail_scopes.contains("https://graph.microsoft.com/Group-Conversation.Read.All"));
        assert!(!mail_scopes.contains("https://graph.microsoft.com/Tasks.ReadWrite"));

        let reminder_scopes = required_scopes(&mail, true);
        assert!(reminder_scopes.contains("https://graph.microsoft.com/User.Read"));
        assert!(reminder_scopes.contains("https://graph.microsoft.com/Tasks.ReadWrite"));
        assert!(!reminder_scopes.contains("https://graph.microsoft.com/Mail.Read"));
        assert!(!reminder_scopes.contains("https://graph.microsoft.com/Group.ReadBasic.All"));
    }

    #[test]
    fn session_uses_a_strict_sixty_second_expiry_margin() {
        let scopes = BTreeSet::from(["scope".to_owned()]);
        let now = Instant::now();
        let at_margin = Session {
            access_token: Secret::new("synthetic-token".to_owned()),
            expires_at: now + Duration::from_mins(1),
            scopes: scopes.clone(),
            shared: SharedScope::NotReported,
        };
        let beyond_margin = Session {
            expires_at: now + Duration::from_secs(61),
            ..synthetic_session(scopes.clone(), Duration::from_secs(1))
        };
        assert!(!session_covers(&at_margin, &scopes, now));
        assert!(session_covers(&beyond_margin, &scopes, now));
    }

    #[test]
    fn clear_session_removes_the_process_session() {
        let _serial = SESSION_TEST.lock().unwrap();
        *session_store() = Some(synthetic_session(
            BTreeSet::from(["scope".to_owned()]),
            Duration::from_mins(2),
        ));
        assert!(has_session());
        clear_session();
        assert!(!has_session());
    }

    #[test]
    fn cached_unauthorized_clears_and_reauthorizes_exactly_once() {
        let _serial = SESSION_TEST.lock().unwrap();
        clear_session();
        let required = BTreeSet::from(["scope".to_owned()]);
        *session_store() = Some(synthetic_session(required.clone(), Duration::from_mins(2)));
        let mut authorizations = 0;
        let mut work_calls = 0;
        let result = run_with_session(
            required.clone(),
            |requested| {
                authorizations += 1;
                assert_eq!(requested, required);
                let mut session = synthetic_session(requested, Duration::from_mins(2));
                session.access_token = Secret::new("replacement-token".to_owned());
                Ok(session)
            },
            |token, _| {
                work_calls += 1;
                if token == "synthetic-token" {
                    Err(ConnectionError::Unauthorized)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(authorizations, 1);
        assert_eq!(work_calls, 2);
        clear_session();
    }

    #[test]
    fn insufficient_session_scope_authorizes_the_union() {
        let _serial = SESSION_TEST.lock().unwrap();
        clear_session();
        let existing = BTreeSet::from(["Mail.Read".to_owned()]);
        let required = BTreeSet::from(["Tasks.ReadWrite".to_owned()]);
        *session_store() = Some(synthetic_session(existing.clone(), Duration::from_mins(2)));
        let result = run_with_session(
            required.clone(),
            |requested| {
                assert_eq!(
                    requested,
                    existing.union(&required).cloned().collect::<BTreeSet<_>>()
                );
                Ok(synthetic_session(requested, Duration::from_mins(2)))
            },
            |_, _| Ok(()),
        );
        assert_eq!(result, Ok(()));
        clear_session();
    }

    #[test]
    fn granted_scope_diagnostics_distinguish_missing_from_unreported() {
        assert_eq!(shared_scope(None), SharedScope::NotReported);
        assert_eq!(
            shared_scope(Some(&vec![Scope::new("Mail.Read".into())])),
            SharedScope::Missing
        );
        for value in [
            "Mail.Read.Shared",
            "https://graph.microsoft.com/Mail.Read.Shared",
            "Mail.ReadWrite.Shared",
        ] {
            assert_eq!(
                shared_scope(Some(&vec![Scope::new(value.into())])),
                SharedScope::Granted
            );
        }
        assert_eq!(
            shared_scope(Some(&vec![Scope::new("Mail.Read.Shared.extra".into())])),
            SharedScope::Missing
        );
    }

    #[test]
    fn authentication_authorization_and_throttling_have_distinct_diagnostics() {
        assert_eq!(classify_status(401), ConnectionError::Unauthorized);
        assert_eq!(classify_status(403), ConnectionError::AccessDenied);
        assert_eq!(classify_status(429), ConnectionError::Throttled);
    }

    #[test]
    fn bad_request_not_found_and_server_error_have_distinct_diagnostics() {
        assert_eq!(classify_status(400), ConnectionError::BadRequest);
        assert_eq!(classify_status(404), ConnectionError::NotFound);
        assert_eq!(classify_status(500), ConnectionError::ServerError);
        assert_eq!(classify_status(503), ConnectionError::ServerError);
    }

    #[test]
    fn an_unmapped_status_falls_back_to_resource_unavailable() {
        assert_eq!(classify_status(418), ConnectionError::ResourceUnavailable);
    }

    #[test]
    fn new_status_class_variants_have_the_expected_display_text() {
        assert_eq!(
            ConnectionError::BadRequest.to_string(),
            "Microsoft Graph rejected the request as malformed (HTTP 400)."
        );
        assert_eq!(
            ConnectionError::NotFound.to_string(),
            "Microsoft Graph reported the requested resource does not exist (HTTP 404)."
        );
        assert_eq!(
            ConnectionError::ServerError.to_string(),
            "Microsoft Graph reported a server-side failure (HTTP 5xx). Retry later."
        );
    }

    #[test]
    fn configuration_rejects_injection_and_placeholders() {
        for value in [
            "",
            "your-client-id",
            "00000000-0000-0000-0000-000000000000",
            "../organizations",
        ] {
            assert!(ConnectionConfig::new(value, None).is_err());
        }
        for mailbox in [
            "person",
            "team@example.invalid/other",
            "team@example.invalid\r\nHeader: value",
        ] {
            assert!(ConnectionConfig::new(APP, Some(mailbox)).is_err());
        }
        assert!(ConnectionConfig::new(APP, Some("team@example.invalid")).is_ok());
    }

    #[test]
    fn multiple_inboxes_preserve_order_and_remove_duplicates() {
        let config = ConnectionConfig::new(APP, Some("one@example.invalid, two@example.invalid; three@example.invalid\nONE@example.invalid")).unwrap();
        assert_eq!(
            config.shared_mailboxes,
            [
                "one@example.invalid",
                "two@example.invalid",
                "three@example.invalid"
            ]
        );
        assert!(
            ConnectionConfig::new(APP, Some(" ; , "))
                .unwrap()
                .shared_mailboxes
                .is_empty()
        );
        assert!(ConnectionConfig::new(APP, Some("one@example.invalid, malformed")).is_err());
    }

    #[test]
    fn denied_shared_inbox_does_not_skip_remaining_inboxes() {
        let config = ConnectionConfig::new(
            APP,
            Some("one@example.invalid,two@example.invalid,three@example.invalid"),
        )
        .unwrap();
        let mut visited = Vec::new();
        let results = check_shared_inboxes(&config.shared_mailboxes, |mailbox| {
            visited.push(mailbox.to_owned());
            if visited.len() == 2 {
                Err(ConnectionError::AccessDenied)
            } else {
                Ok(())
            }
        });
        assert_eq!(visited, config.shared_mailboxes);
        assert_eq!(
            results,
            [Ok(()), Err(ConnectionError::AccessDenied), Ok(())]
        );
    }

    #[test]
    fn mailbox_is_one_encoded_path_segment_and_cannot_change_origin() {
        let url = inbox_url(Some("team@example.invalid")).unwrap();
        assert_eq!(
            url.origin().ascii_serialization(),
            "https://graph.microsoft.com"
        );
        assert_eq!(
            url.path(),
            "/v1.0/users/team@example.invalid/mailFolders/inbox/messages"
        );
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("$top".into(), "1".into()), ("$select".into(), "id".into())]
        );
        assert_eq!(
            inbox_url(None).unwrap().path(),
            "/v1.0/me/mailFolders/inbox/messages"
        );
    }
}
