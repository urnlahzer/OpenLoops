//! Explicit, bounded, read-only review. No following server-provided URLs or storing mail.
use super::{
    Client, ConnectionConfig, ConnectionError, GRAPH_TIMEOUT_SECONDS, SharedScope, Url,
    bounded_body, classify_status, groups, inbox_url, request_error, with_session,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

/// Microsoft Graph documents a limit of four concurrent Outlook requests per mailbox.
const OUTLOOK_REQUEST_WORKERS: usize = 4;

/// Private message content; deliberately no Debug implementation.
#[derive(Clone, Default)]
pub struct MailItem {
    pub subject: String,
    pub body: String,
    pub body_is_html: bool,
    pub sender: String,
    pub received: String,
    pub id: String,
    pub conversation: String,
    pub account: String,
    pub own_addresses: Vec<String>,
    pub sender_address: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub sent: bool,
    pub team: bool,
    pub web_link: String,
    pub event: Option<MailEvent>,
}

#[derive(Clone)]
pub struct MailEvent {
    pub start: String,
    pub end: String,
    pub out_of_date: bool,
}

pub struct SourceReview {
    pub label: String,
    pub messages: Vec<MailItem>,
    pub errors: Vec<ConnectionError>,
    pub message_errors: Vec<ConnectionError>,
    pub partial: bool,
}

/// Process-local progress and cancellation state for the mail download preceding a scan.
#[derive(Default)]
pub struct LoadProgress {
    pub listed: AtomicUsize,
    pub loaded: AtomicUsize,
    pub sources_done: AtomicUsize,
    pub sources_total: AtomicUsize,
    pub cancel: AtomicBool,
}

/// Downloaded messages retained only for the current process session.
pub type MailCache = HashMap<(String, String), MailItem>;

/// Browser sign-in, then personal/shared Inbox messages and selected Group thread posts.
/// Only invoke after the user chooses to load message content for review.
/// # Errors
/// Returns fixed configuration/authentication errors. Source-specific failures remain visible.
pub fn load_recent(config: &ConnectionConfig) -> Result<Vec<SourceReview>, ConnectionError> {
    load_recent_with(config, &MailCache::default(), &LoadProgress::default())
}

/// Browser sign-in, then a cancellable, cache-aware download of recent mail.
///
/// # Errors
/// Returns fixed configuration/authentication errors, or [`ConnectionError::Cancelled`] when the
/// caller stops the download. Source-specific failures remain visible on their source.
pub fn load_recent_with(
    config: &ConnectionConfig,
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<Vec<SourceReview>, ConnectionError> {
    if 1 + config.group_inboxes.len() + config.shared_mailboxes.len() > 10 {
        return Err(ConnectionError::InvalidConfiguration);
    }
    if progress.cancel.load(Ordering::Acquire) {
        return Err(ConnectionError::Cancelled);
    }
    progress.listed.store(0, Ordering::Relaxed);
    progress.loaded.store(0, Ordering::Relaxed);
    progress.sources_done.store(0, Ordering::Relaxed);
    progress.sources_total.store(
        2 + config.group_inboxes.len() + config.shared_mailboxes.len() * 2,
        Ordering::Relaxed,
    );
    with_session(config, |http, token, scope| {
        progress.listed.store(0, Ordering::Relaxed);
        progress.loaded.store(0, Ordering::Relaxed);
        progress.sources_done.store(0, Ordering::Relaxed);
        let shared_sources = if scope == SharedScope::Missing {
            config.shared_mailboxes.len()
        } else {
            config.shared_mailboxes.len() * 2
        };
        progress.sources_total.store(
            2 + config.group_inboxes.len() + shared_sources,
            Ordering::Relaxed,
        );
        ensure_loading(progress)?;
        let (account, addresses) = identity(http, token)?;
        ensure_loading(progress)?;
        let mut sources = vec![load_mailbox(
            http, token, None, &account, &addresses, cache, progress,
        )?];
        progress.sources_done.fetch_add(1, Ordering::Relaxed);
        ensure_loading(progress)?;
        sources.push(load_sent(
            http, token, None, &account, &addresses, cache, progress,
        )?);
        progress.sources_done.fetch_add(1, Ordering::Relaxed);
        for address in &config.group_inboxes {
            ensure_loading(progress)?;
            let mut source = load_group(http, token, address);
            stamp_group_messages(&mut source, &account, &addresses);
            progress
                .listed
                .fetch_add(source.messages.len(), Ordering::Relaxed);
            progress
                .loaded
                .fetch_add(source.messages.len(), Ordering::Relaxed);
            sources.push(source);
            progress.sources_done.fetch_add(1, Ordering::Relaxed);
        }
        for address in &config.shared_mailboxes {
            ensure_loading(progress)?;
            if scope == SharedScope::Missing {
                sources.push(SourceReview {
                    label: address.clone(),
                    messages: vec![],
                    errors: vec![ConnectionError::MissingSharedScope],
                    message_errors: vec![],
                    partial: false,
                });
                progress.sources_done.fetch_add(1, Ordering::Relaxed);
            } else {
                sources.push(load_mailbox(
                    http,
                    token,
                    Some(address),
                    &account,
                    &addresses,
                    cache,
                    progress,
                )?);
                progress.sources_done.fetch_add(1, Ordering::Relaxed);
                ensure_loading(progress)?;
                sources.push(load_sent(
                    http,
                    token,
                    Some(address),
                    &account,
                    &addresses,
                    cache,
                    progress,
                )?);
                progress.sources_done.fetch_add(1, Ordering::Relaxed);
            }
        }
        ensure_loading(progress)?;
        Ok(sources)
    })
}

fn ensure_loading(progress: &LoadProgress) -> Result<(), ConnectionError> {
    if progress.cancel.load(Ordering::Acquire) {
        Err(ConnectionError::Cancelled)
    } else {
        Ok(())
    }
}

fn stamp_group_messages(source: &mut SourceReview, account: &str, addresses: &[String]) {
    for message in &mut source.messages {
        account.clone_into(&mut message.account);
        message.own_addresses = addresses.to_vec();
    }
}

pub(super) fn fetch(http: &Client, token: &str, url: &Url) -> Result<Vec<u8>, ConnectionError> {
    fetch_from_origin(http, token, url, GRAPH_ORIGIN)
}

/// Fixed production origin every real request is checked against; the only
/// caller that ever passes anything else is the `#[cfg(test)]` mock-HTTP
/// harness below, which points a real loopback listener's own origin at
/// `fetch_from_origin` directly -- production code always goes through
/// [`fetch`], which hardcodes this constant.
pub(super) const GRAPH_ORIGIN: &str = "https://graph.microsoft.com/";

/// Read-only bounded fetch, checked against `expected_origin` (scheme, host,
/// and port together, via [`Url::origin`]) rather than an inline literal, so
/// the exact same request/response handling this function performs -- status
/// classification and the response-size bound -- is exercisable end to end
/// against a real, local mock server in tests without weakening the fixed
/// production check: [`fetch`] always supplies [`GRAPH_ORIGIN`].
pub(super) fn fetch_from_origin(
    http: &Client,
    token: &str,
    url: &Url,
    expected_origin: &str,
) -> Result<Vec<u8>, ConnectionError> {
    fetch_from_origin_with_policy(
        http,
        token,
        url,
        expected_origin,
        Duration::from_secs(GRAPH_TIMEOUT_SECONDS),
        Duration::from_secs(2),
    )
}

fn fetch_from_origin_with_policy(
    http: &Client,
    token: &str,
    url: &Url,
    expected_origin: &str,
    timeout: Duration,
    retry_delay: Duration,
) -> Result<Vec<u8>, ConnectionError> {
    let expected =
        Url::parse(expected_origin).map_err(|_| ConnectionError::InvalidConfiguration)?;
    if url.origin() != expected.origin() {
        return Err(ConnectionError::InvalidConfiguration);
    }
    for attempt in 0..2 {
        let response = http
            .get(url.clone())
            .bearer_auth(token)
            .header(
                "Prefer",
                "outlook.body-content-type=\"html\", IdType=\"ImmutableId\"",
            )
            .timeout(timeout)
            .send();
        let (result, retryable) = match response {
            Ok(response) if response.status().as_u16() == 200 => {
                let result = bounded_body(response);
                let retryable = matches!(
                    result,
                    Err(ConnectionError::Timeout(_) | ConnectionError::Transport)
                );
                (result, retryable)
            }
            Ok(response) => {
                let status = response.status().as_u16();
                (Err(classify_status(status)), matches!(status, 503 | 504))
            }
            Err(error) => {
                let error = request_error(&error, timeout.as_secs());
                let retryable = matches!(
                    error,
                    ConnectionError::Timeout(_) | ConnectionError::Transport
                );
                (Err(error), retryable)
            }
        };
        if attempt == 0 && retryable {
            std::thread::sleep(retry_delay);
        } else {
            return result;
        }
    }
    unreachable!("the retry loop always returns on its final attempt")
}

fn page(bytes: &[u8]) -> Result<(Vec<Value>, bool), ConnectionError> {
    let mut value: Value =
        serde_json::from_slice(bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    let partial = value.get("@odata.nextLink").is_some();
    let rows = value
        .get_mut("value")
        .and_then(Value::as_array_mut)
        .ok_or(ConnectionError::ResourceUnavailable)?;
    if rows.len() > 100 {
        return Err(ConnectionError::ResponseTooLarge);
    }
    Ok((std::mem::take(rows), partial))
}

pub(super) fn identity(
    http: &Client,
    token: &str,
) -> Result<(String, Vec<String>), ConnectionError> {
    identity_from_origin(http, token, GRAPH_ORIGIN)
}

pub(super) fn identity_from_origin(
    http: &Client,
    token: &str,
    origin: &str,
) -> Result<(String, Vec<String>), ConnectionError> {
    let url = Url::parse(origin)
        .and_then(|url| url.join("v1.0/me?$select=id,mail,userPrincipalName"))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let value: Value = serde_json::from_slice(&fetch_from_origin(http, token, &url, origin)?)
        .map_err(|_| ConnectionError::ResourceUnavailable)?;
    let id = text(&value, "id", 512)?;
    let addresses: Vec<_> = ["mail", "userPrincipalName"]
        .iter()
        .filter_map(|field| text(&value, field, 512).ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    if id.is_empty() || addresses.is_empty() {
        return Err(ConnectionError::ResourceUnavailable);
    }
    Ok((id, addresses))
}

fn cutoff() -> String {
    let now: chrono::DateTime<chrono::Utc> = std::time::SystemTime::now().into();
    (now - chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn cutoff_timestamp() -> i64 {
    chrono::DateTime::parse_from_rfc3339(&cutoff()).map_or(0, |value| value.timestamp())
}

fn recipients(value: &Value, field: &str) -> Result<Vec<String>, ConnectionError> {
    let Some(rows) = value.get(field).and_then(Value::as_array) else {
        return Ok(vec![]);
    };
    if rows.len() > 200 {
        return Err(ConnectionError::ResponseTooLarge);
    }
    rows.iter()
        .map(|row| {
            let email = row
                .get("emailAddress")
                .ok_or(ConnectionError::ResourceUnavailable)?;
            text(email, "address", 512).map(|s| s.to_lowercase())
        })
        .collect()
}

// Follow only the same collection at the fixed Graph origin; never follow an
// arbitrary URL from content, a model, or a different resource in the response.
fn next_page(value: &Value, original: &Url) -> Result<Option<Url>, ConnectionError> {
    let Some(next) = value.get("@odata.nextLink") else {
        return Ok(None);
    };
    let url = Url::parse(next.as_str().ok_or(ConnectionError::ResourceUnavailable)?)
        .map_err(|_| ConnectionError::ResourceUnavailable)?;
    if url.origin() != original.origin()
        || url.path() != original.path()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ConnectionError::NextPageRejected);
    }
    Ok(Some(url))
}

pub(super) fn pages(
    http: &Client,
    token: &str,
    original: &Url,
    cap: usize,
) -> Result<(Vec<Value>, bool), ConnectionError> {
    pages_from_origin(http, token, original, cap, GRAPH_ORIGIN)
}

pub(super) fn pages_from_origin(
    http: &Client,
    token: &str,
    original: &Url,
    cap: usize,
    origin: &str,
) -> Result<(Vec<Value>, bool), ConnectionError> {
    let mut url = original.clone();
    let mut all = vec![];
    for _ in 0..10 {
        let bytes = fetch_from_origin(http, token, &url, origin)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
        let (rows, _) = page(&bytes)?;
        let overflow = rows.len() > cap - all.len();
        all.extend(rows.into_iter().take(cap - all.len()));
        if overflow {
            return Ok((all, true));
        }
        // `all` already holds every row fetched so far, including this page's,
        // so a rejected next-link below only truncates pagination -- it cannot
        // lose rows. We deliberately fold a next_page rejection into
        // `partial: true` here instead of propagating ConnectionError::NextPageRejected
        // through `?`: surfacing it would require widening this function's
        // return type and touching `load_group`'s call site for no real benefit,
        // since the caller already treats "stopped early" the same way
        // regardless of cause.
        let Ok(next) = next_page(&value, original) else {
            return Ok((all, true));
        };
        if all.len() == cap && next.is_some() {
            return Ok((all, true));
        }
        let Some(next) = next else {
            return Ok((all, false));
        };
        url = next;
    }
    Ok((all, true))
}

fn text(value: &Value, field: &str, limit: usize) -> Result<String, ConnectionError> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(ConnectionError::ResourceUnavailable)?;
    if text.len() > limit {
        return Err(ConnectionError::ResponseTooLarge);
    }
    Ok(text.to_owned())
}

fn item(value: &Value, topic: Option<&str>) -> Result<MailItem, ConnectionError> {
    let body = value
        .get("body")
        .ok_or(ConnectionError::ResourceUnavailable)?;
    let kind = text(body, "contentType", 16)?;
    if !kind.eq_ignore_ascii_case("html") && !kind.eq_ignore_ascii_case("text") {
        return Err(ConnectionError::ResourceUnavailable);
    }
    let sender = value
        .get("sender")
        .filter(|value| !value.is_null())
        .or_else(|| value.get("from"))
        .and_then(|value| value.get("emailAddress"));
    let sender_address = sender
        .and_then(|v| v.get("address"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let sender = match sender {
        Some(value) => {
            let name = text(value, "name", 4096).unwrap_or_default();
            let address = text(value, "address", 512)?;
            if name.is_empty() {
                address
            } else {
                format!("{name} <{address}>")
            }
        }
        None => String::new(),
    };
    Ok(MailItem {
        subject: match topic {
            Some(topic) => topic.into(),
            None => text(value, "subject", 8192)?,
        },
        body: text(body, "content", 131_072).map_err(|error| match error {
            ConnectionError::ResponseTooLarge => ConnectionError::MessageTooLarge,
            other => other,
        })?,
        body_is_html: kind.eq_ignore_ascii_case("html"),
        sender,
        received: text(value, "receivedDateTime", 64)?,
        id: text(value, "id", 2048).unwrap_or_default(),
        conversation: text(value, "conversationId", 2048).unwrap_or_default(),
        sender_address,
        to: recipients(value, "toRecipients")?,
        cc: recipients(value, "ccRecipients")?,
        web_link: text(value, "webLink", 8192).unwrap_or_default(),
        ..MailItem::default()
    })
}

fn mailbox_url(address: Option<&str>, sent: bool) -> Result<Url, ConnectionError> {
    let mut url = inbox_url(address)?;
    if sent {
        url.set_path(
            &url.path()
                .replace("/mailFolders/inbox/", "/mailFolders/sentitems/"),
        );
    }
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("$select", "id,conversationId,subject,sender,from,toRecipients,ccRecipients,receivedDateTime,sentDateTime,webLink")
        .append_pair(
            "$orderby",
            if sent { "sentDateTime desc" } else { "receivedDateTime desc" },
        )
        .append_pair("$top", "100");
    Ok(url)
}

/// Bounded, read-only body fetch for a single message already discovered via
/// `mailbox_url`. Never follows a server-provided URL; always the fixed Graph
/// origin. `cast` optionally appends an `OData` cast segment (e.g.
/// `microsoft.graph.eventMessage`) after the message ID, the documented way
/// to reach type-specific properties on a `message` resource.
fn message_url_with_select(
    address: Option<&str>,
    id: &str,
    cast: Option<&str>,
    select: &str,
) -> Result<Url, ConnectionError> {
    if id.is_empty() || id == "." || id == ".." {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let mut url = Url::parse("https://graph.microsoft.com/v1.0/")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?;
        path.pop_if_empty();
        match address {
            None => {
                path.push("me");
            }
            Some(value) => {
                path.push("users").push(value);
            }
        }
        path.push("messages").push(id);
        if let Some(cast) = cast {
            path.push(cast);
        }
    }
    url.query_pairs_mut().append_pair("$select", select);
    Ok(url)
}

fn message_url(address: Option<&str>, id: &str) -> Result<Url, ConnectionError> {
    message_url_with_select(address, id, None, "body")
}

/// The `OData` cast path for the bounded meeting-metadata extra fetch:
/// `/messages/{id}/microsoft.graph.eventMessage?$select=...` (personal), or,
/// for a shared mailbox like `shared@example.invalid`,
/// `/users/shared@example.invalid/messages/{id}/microsoft.graph.eventMessage?$select=...`.
fn event_url(address: Option<&str>, id: &str) -> Result<Url, ConnectionError> {
    message_url_with_select(
        address,
        id,
        Some("microsoft.graph.eventMessage"),
        "meetingMessageType,startDateTime,endDateTime,isOutOfDate",
    )
}

fn is_event_message(value: &Value) -> bool {
    matches!(
        value.get("@odata.type").and_then(Value::as_str),
        Some("#microsoft.graph.eventMessageRequest" | "#microsoft.graph.eventMessage")
    )
}

fn utc_event_time(value: &Value, field: &str) -> Option<String> {
    let value = value.get(field)?;
    if value.get("timeZone")?.as_str()? != "UTC" {
        return None;
    }
    // Graph's own dateTime strings never carry a trailing zone designator
    // (that is what the separate timeZone field is for), but guard the
    // append anyway rather than assume the server never sends one.
    let raw = value.get("dateTime")?.as_str()?.trim_end_matches('Z');
    let parsed = chrono::DateTime::parse_from_rfc3339(&format!("{raw}Z")).ok()?;
    Some(
        parsed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// `meetingMessageType` is used only to confirm the extra fetch actually
/// returned meeting metadata (rather than, say, a truncated or unexpected
/// payload); the value itself is not needed downstream, so it is not stored.
fn mail_event(value: &Value) -> Option<MailEvent> {
    value.get("meetingMessageType")?.as_str()?;
    Some(MailEvent {
        start: utc_event_time(value, "startDateTime")?,
        end: utc_event_time(value, "endDateTime")?,
        out_of_date: value.get("isOutOfDate")?.as_bool()?,
    })
}

/// Splits newest-first `rows` into the in-window prefix (timestamp >= `cutoff`,
/// capped at `cap`) and reports whether the walk reached a row older than the
/// cutoff. Rows with a missing or unparseable date are skipped (not counted as
/// in-window); the caller is told how many were skipped so it can record errors.
fn window(
    rows: Vec<Value>,
    date_field: &str,
    cutoff: i64,
    cap: usize,
) -> (Vec<Value>, bool, usize) {
    let mut kept = Vec::new();
    let mut reached_cutoff = false;
    let mut skipped = 0usize;
    for row in rows {
        if kept.len() >= cap {
            break;
        }
        match text(&row, date_field, 64)
            .ok()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        {
            Some(parsed) => {
                if parsed.timestamp() < cutoff {
                    reached_cutoff = true;
                    break;
                }
                kept.push(row);
            }
            None => skipped += 1,
        }
    }
    (kept, reached_cutoff, skipped)
}

/// Fetches the body for one listing row and folds it into `row`, then projects
/// the result through `item`. The body request is independently bounded by
/// `bounded_body` and by the content-length limit inside `item`.
fn hydrate(
    http: &Client,
    token: &str,
    address: Option<&str>,
    mut row: Value,
) -> Result<MailItem, ConnectionError> {
    let id = text(&row, "id", 2048)?;
    let bytes = fetch(http, token, &message_url(address, &id)?).map_err(|error| match error {
        ConnectionError::ResponseTooLarge => ConnectionError::MessageTooLarge,
        other => other,
    })?;
    let body_value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    let body = body_value
        .get("body")
        .cloned()
        .ok_or(ConnectionError::ResourceUnavailable)?;
    row.as_object_mut()
        .ok_or(ConnectionError::ResourceUnavailable)?
        .insert("body".to_string(), body);
    let mut result = item(&row, None)?;
    if is_event_message(&body_value)
        && let Ok(url) = event_url(address, &id)
    {
        result.event = fetch_event(http, token, &url, GRAPH_ORIGIN);
    }
    Ok(result)
}

/// The bounded, best-effort meeting-metadata extra fetch: any failure --
/// transport, a non-200 status (a message that is no longer a meeting
/// request, or one Graph otherwise rejects the cast on), malformed JSON, or
/// a payload missing one of the four selected fields -- yields `None`
/// rather than failing the whole message. `expected_origin` is threaded
/// through to [`fetch_from_origin`] purely for testability; every real
/// caller passes [`GRAPH_ORIGIN`].
fn fetch_event(http: &Client, token: &str, url: &Url, expected_origin: &str) -> Option<MailEvent> {
    let bytes = fetch_from_origin(http, token, url, expected_origin).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    mail_event(&value)
}

fn load_mailbox(
    http: &Client,
    token: &str,
    address: Option<&str>,
    account: &str,
    own_addresses: &[String],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<SourceReview, ConnectionError> {
    load_folder(
        http,
        token,
        address,
        false,
        account,
        own_addresses,
        cache,
        progress,
    )
}

fn load_sent(
    http: &Client,
    token: &str,
    address: Option<&str>,
    account: &str,
    own_addresses: &[String],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<SourceReview, ConnectionError> {
    load_folder(
        http,
        token,
        address,
        true,
        account,
        own_addresses,
        cache,
        progress,
    )
}

// Walks the newest-first listing page by page, stopping as soon as a row older
// than `cutoff` is seen (or the cap / the 10-page hard limit is hit). Keeps
// every listing response small: no message bodies are ever requested in bulk.
// `source.partial` semantics: false once the walk reaches the cutoff or the
// collection ends with fewer than `cap` in-window rows taken; true once `cap`
// in-window rows were taken (more could exist), the 10-page limit was hit
// before reaching the cutoff, or a fetched page's next-page link was rejected
// (that page's rows are kept; more mail may exist beyond the rejected link).
fn windowed_rows(
    http: &Client,
    token: &str,
    original: &Url,
    date_field: &str,
    cutoff: i64,
    cap: usize,
    errors: &mut Vec<ConnectionError>,
) -> (Vec<Value>, bool) {
    let mut current = original.clone();
    let mut collected: Vec<Value> = Vec::new();
    let mut reached_cutoff = false;
    let mut page_limit_hit = true;
    let mut next_page_rejected = false;
    for _ in 0..10 {
        let bytes = match fetch(http, token, &current) {
            Ok(bytes) => bytes,
            Err(error) => {
                errors.push(error);
                page_limit_hit = false;
                break;
            }
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            errors.push(ConnectionError::ResourceUnavailable);
            page_limit_hit = false;
            break;
        };
        let rows = match page(&bytes) {
            Ok((rows, _)) => rows,
            Err(error) => {
                errors.push(error);
                page_limit_hit = false;
                break;
            }
        };
        // Fold this page's rows into `collected` before asking for the next
        // link: a rejected/malformed next-link must not throw away a page
        // that was already fetched and parsed successfully.
        let remaining = cap - collected.len();
        let (in_window, page_reached_cutoff, skipped) = window(rows, date_field, cutoff, remaining);
        for _ in 0..skipped {
            errors.push(ConnectionError::ResourceUnavailable);
        }
        collected.extend(in_window);
        if page_reached_cutoff {
            reached_cutoff = true;
            page_limit_hit = false;
            break;
        }
        if collected.len() >= cap {
            page_limit_hit = false;
            break;
        }
        let next = match next_page(&value, original) {
            Ok(next) => next,
            Err(error) => {
                // This page's rows are already in `collected`. Only
                // pagination stops here; more mail may exist beyond the
                // rejected link, so `partial` must stay true.
                errors.push(error);
                page_limit_hit = false;
                next_page_rejected = true;
                break;
            }
        };
        let Some(next_url) = next else {
            page_limit_hit = false;
            break;
        };
        current = next_url;
    }
    let partial = if collected.len() >= cap {
        true
    } else if reached_cutoff {
        false
    } else {
        page_limit_hit || next_page_rejected
    };
    (collected, partial)
}

#[allow(clippy::too_many_arguments)]
fn load_folder(
    http: &Client,
    token: &str,
    address: Option<&str>,
    sent: bool,
    account: &str,
    own_addresses: &[String],
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<SourceReview, ConnectionError> {
    let mut source = SourceReview {
        label: format!(
            "{} / {}",
            address.unwrap_or("Personal mailbox"),
            if sent { "Sent Items" } else { "Inbox" }
        ),
        messages: vec![],
        errors: vec![],
        message_errors: vec![],
        partial: false,
    };
    let date_field = if sent {
        "sentDateTime"
    } else {
        "receivedDateTime"
    };
    let original = match mailbox_url(address, sent) {
        Ok(url) => url,
        Err(error) => {
            source.errors.push(error);
            return Ok(source);
        }
    };
    let (collected, partial) = windowed_rows(
        http,
        token,
        &original,
        date_field,
        cutoff_timestamp(),
        100,
        &mut source.errors,
    );
    source.partial = partial;
    add_hydrated(
        &mut source,
        &collected,
        sent,
        address.is_some(),
        account,
        own_addresses,
        cache,
        progress,
        |row| hydrate(http, token, address, row),
    )?;
    Ok(source)
}

#[allow(clippy::too_many_arguments)]
fn add_hydrated(
    source: &mut SourceReview,
    collected: &[Value],
    sent: bool,
    team: bool,
    account: &str,
    own_addresses: &[String],
    cache: &MailCache,
    progress: &LoadProgress,
    hydrate_row: impl Fn(Value) -> Result<MailItem, ConnectionError> + Sync,
) -> Result<(), ConnectionError> {
    progress
        .listed
        .fetch_add(collected.len(), Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..OUTLOOK_REQUEST_WORKERS {
            let sender = sender.clone();
            let hydrate_row = &hydrate_row;
            let next = &next;
            scope.spawn(move || {
                loop {
                    if progress.cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(row) = collected.get(index).cloned() else {
                        break;
                    };
                    if progress.cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let sent_time = sent.then(|| text(&row, "sentDateTime", 64).ok()).flatten();
                    let cached = text(&row, "id", 2048).ok().and_then(|id| {
                        cache
                            .get(&(account.to_owned(), id))
                            .cloned()
                            .map(|message| (message, true))
                    });
                    let result =
                        cached.map_or_else(|| hydrate_row(row).map(|message| (message, false)), Ok);
                    if result.is_ok() {
                        progress.loaded.fetch_add(1, Ordering::Relaxed);
                    }
                    if sender.send((index, sent_time, result)).is_err() {
                        break;
                    }
                }
            });
        }
    });
    drop(sender);
    let mut results = receiver.into_iter().collect::<Vec<_>>();
    results.sort_unstable_by_key(|(index, _, _)| *index);
    for (_, sent_time, result) in results {
        match result {
            Ok((mut message, cached)) => {
                if !cached {
                    account.clone_into(&mut message.account);
                    message.own_addresses = own_addresses.to_vec();
                    message.sent = sent;
                    message.team = team;
                    if let Some(sent_time) = sent_time {
                        message.received = sent_time;
                    }
                }
                if message.id.is_empty() || message.conversation.is_empty() {
                    source.errors.push(ConnectionError::ResourceUnavailable);
                } else {
                    source.messages.push(message);
                }
            }
            Err(error) => source.message_errors.push(error),
        }
    }
    ensure_loading(progress)
}

fn group_url(id: &str, thread: Option<&str>) -> Result<Url, ConnectionError> {
    let mut url = Url::parse("https://graph.microsoft.com/v1.0/groups/")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?;
        path.pop_if_empty().push(id).push("threads");
        if let Some(thread) = thread {
            path.push(thread).push("posts");
        }
    }
    if thread.is_some() {
        // This endpoint documents $select/$expand, not $top/$orderby. Bound locally.
        url.query_pairs_mut()
            .append_pair("$select", "id,body,sender,from,receivedDateTime");
    } else {
        url.query_pairs_mut()
            .append_pair("$select", "id,topic,lastDeliveredDateTime")
            .append_pair("$orderby", "lastDeliveredDateTime desc")
            .append_pair("$top", "20");
    }
    Ok(url)
}

fn load_group(http: &Client, token: &str, address: &str) -> SourceReview {
    let mut source = SourceReview {
        label: format!("Group: {address}"),
        messages: vec![],
        errors: vec![],
        message_errors: vec![],
        partial: false,
    };
    let result = (|| {
        let id = groups::resolve_id(&fetch(http, token, &groups::lookup_url(address)?)?)?;
        let (threads, partial) = page(&fetch(http, token, &group_url(&id, None)?)?)?;
        source.partial = partial || threads.len() > 20;
        for thread in threads.iter().take(20) {
            let result = (|| {
                let thread_id = text(thread, "id", 2048)?;
                if thread_id.is_empty() || thread_id == "." || thread_id == ".." {
                    return Err(ConnectionError::ResourceUnavailable);
                }
                let topic = text(thread, "topic", 8192)?;
                let delivered = text(thread, "lastDeliveredDateTime", 64)?;
                if delivered < cutoff() {
                    return Ok(());
                }
                let (posts, partial) = pages(http, token, &group_url(&id, Some(&thread_id))?, 40)?;
                source.partial |= partial;
                let mut messages = Vec::new();
                for post in &posts {
                    match item(post, Some(&topic)) {
                        Ok(message) => messages.push(message),
                        Err(error) => source.message_errors.push(error),
                    }
                }
                messages.sort_by(|a, b| b.received.cmp(&a.received));
                for mut message in messages {
                    message.conversation = format!("group:{id}:{thread_id}");
                    message.team = true;
                    if message.id.is_empty() {
                        return Err(ConnectionError::ResourceUnavailable);
                    }
                    source.messages.push(message);
                }
                Ok(())
            })();
            if let Err(error) = result {
                source.errors.push(error);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        source.errors.push(error);
    }
    source
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_paths_are_read_only_bounded_and_encode_identifiers() {
        let url = mailbox_url(Some("synthetic@example.invalid"), false).unwrap();
        assert_eq!(url.host_str(), Some("graph.microsoft.com"));
        assert!(url.query_pairs().any(|(k, v)| k == "$top" && v == "100"));
        let url = group_url("synthetic-group", Some("thread/id?query")).unwrap();
        assert!(url.path().contains("thread%2Fid%3Fquery/posts"));
        assert!(
            !url.query_pairs()
                .any(|(k, _)| k == "$top" || k == "$orderby")
        );
        let (_, partial) =
            page(br#"{"value":[],"@odata.nextLink":"https://example.invalid/private"}"#).unwrap();
        assert!(partial);
    }
    #[test]
    fn pagination_cannot_escape_the_authorized_collection() {
        let original = mailbox_url(None, false).unwrap();
        for next in [
            "http://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages",
            "https://example.invalid/v1.0/me/mailFolders/inbox/messages",
            "https://graph.microsoft.com/v1.0/me/contacts",
            "https://user@graph.microsoft.com/v1.0/me/mailFolders/inbox/messages",
            "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages#frag",
        ] {
            assert!(matches!(
                next_page(&serde_json::json!({"@odata.nextLink":next}), &original),
                Err(ConnectionError::NextPageRejected)
            ));
        }
        // A non-string nextLink is a different failure mode (malformed payload,
        // not an out-of-collection redirect) and keeps the generic diagnostic.
        assert!(matches!(
            next_page(&serde_json::json!({"@odata.nextLink": 12345}), &original),
            Err(ConnectionError::ResourceUnavailable)
        ));
        // An unparsable URL string is likewise a malformed payload, not a
        // rejected-but-well-formed link.
        assert!(matches!(
            next_page(
                &serde_json::json!({"@odata.nextLink": "not a url"}),
                &original
            ),
            Err(ConnectionError::ResourceUnavailable)
        ));
        let mut next = original.clone();
        next.query_pairs_mut().append_pair("$skip", "25");
        assert!(
            next_page(
                &serde_json::json!({"@odata.nextLink":next.as_str()}),
                &original
            )
            .unwrap()
            .is_some()
        );
        let sent = mailbox_url(None, true).unwrap();
        assert!(sent.path().contains("/sentitems/messages"));
        assert!(!sent.query_pairs().any(|(k, _)| k == "$filter"));
    }
    #[test]
    fn mail_projection_preserves_conversation_and_recipient_context() {
        let value = serde_json::json!({"id":"synthetic-message","conversationId":"synthetic-conversation","subject":"Budget","body":{"contentType":"text","content":"Please send the draft."},"sender":{"emailAddress":{"name":"Alex","address":"alex@example.invalid"}},"toRecipients":[{"emailAddress":{"address":"user@example.invalid"}}],"ccRecipients":[{"emailAddress":{"address":"observer@example.invalid"}}],"receivedDateTime":"2026-09-01T12:00:00Z"});
        let m = item(&value, None).unwrap();
        assert_eq!(m.id, "synthetic-message");
        assert_eq!(m.conversation, "synthetic-conversation");
        assert_eq!(m.to, ["user@example.invalid"]);
        assert_eq!(m.cc, ["observer@example.invalid"]);
    }
    #[test]
    fn group_posts_use_thread_topic_and_reject_missing_or_oversized_bodies() {
        let mut value = serde_json::json!({"body":{"contentType":"html","content":"<p>Synthetic request</p>"},"from":{"emailAddress":{"name":"Synthetic","address":"sender@example.invalid"}},"receivedDateTime":"2026-01-01T12:00:00Z"});
        let message = item(&value, Some("Synthetic topic")).unwrap();
        assert_eq!(message.subject, "Synthetic topic");
        assert!(message.body_is_html);
        value["body"]["content"] = Value::String("x".repeat(131_073));
        assert!(matches!(
            item(&value, Some("topic")),
            Err(ConnectionError::MessageTooLarge)
        ));
        assert!(item(&serde_json::json!({}), None).is_err());
    }
    #[test]
    fn message_too_large_has_a_distinct_message() {
        assert_eq!(
            ConnectionError::MessageTooLarge.to_string(),
            "A message body exceeded the review size limit and was skipped."
        );
    }
    #[test]
    fn next_page_rejected_has_a_distinct_message() {
        assert_eq!(
            ConnectionError::NextPageRejected.to_string(),
            "Microsoft returned a next-page link outside the authorized collection; remaining pages were skipped."
        );
    }
    #[test]
    fn mailbox_url_excludes_body_and_orders_by_folder_date_field() {
        let inbox = mailbox_url(None, false).unwrap();
        let select = inbox
            .query_pairs()
            .find(|(k, _)| k == "$select")
            .unwrap()
            .1
            .into_owned();
        assert!(!select.split(',').any(|field| field == "body"));
        assert!(!inbox.query_pairs().any(|(k, _)| k == "$filter"));
        assert!(inbox.query_pairs().any(|(k, v)| k == "$top" && v == "100"));
        assert!(
            inbox
                .query_pairs()
                .any(|(k, v)| k == "$orderby" && v == "receivedDateTime desc")
        );
        let sent = mailbox_url(None, true).unwrap();
        let select = sent
            .query_pairs()
            .find(|(k, _)| k == "$select")
            .unwrap()
            .1
            .into_owned();
        assert!(!select.split(',').any(|field| field == "body"));
        assert!(!sent.query_pairs().any(|(k, _)| k == "$filter"));
        assert!(sent.query_pairs().any(|(k, v)| k == "$top" && v == "100"));
        assert!(
            sent.query_pairs()
                .any(|(k, v)| k == "$orderby" && v == "sentDateTime desc")
        );
    }
    #[test]
    fn message_url_scopes_and_encodes_the_identifier() {
        let personal = message_url(None, "synthetic-id").unwrap();
        assert_eq!(personal.path(), "/v1.0/me/messages/synthetic-id");
        assert!(
            personal
                .query_pairs()
                .any(|(k, v)| k == "$select" && v == "body")
        );
        let shared = message_url(Some("shared@example.invalid"), "synthetic-id").unwrap();
        assert_eq!(
            shared.path(),
            "/v1.0/users/shared@example.invalid/messages/synthetic-id"
        );
        let encoded = message_url(None, "a/b?c").unwrap();
        assert!(encoded.path().contains("a%2Fb%3Fc"));
        for id in ["", ".", ".."] {
            assert!(message_url(None, id).is_err());
        }
    }
    #[test]
    fn event_url_selects_only_meeting_metadata_via_the_odata_cast_path() {
        let personal = event_url(None, "synthetic-id").unwrap();
        assert_eq!(
            personal.path(),
            "/v1.0/me/messages/synthetic-id/microsoft.graph.eventMessage"
        );
        assert!(personal.query_pairs().any(|(key, value)| {
            key == "$select" && value == "meetingMessageType,startDateTime,endDateTime,isOutOfDate"
        }));
        let shared = event_url(Some("shared@example.invalid"), "a/b").unwrap();
        assert_eq!(
            shared.path(),
            "/v1.0/users/shared@example.invalid/messages/a%2Fb/microsoft.graph.eventMessage"
        );
    }

    #[test]
    fn meeting_metadata_parses_only_utc_event_times() {
        let meeting = serde_json::json!({
            "@odata.type": "#microsoft.graph.eventMessageRequest",
            "meetingMessageType": "meetingRequest",
            "startDateTime": {"dateTime": "2026-08-21T18:30:00.0000000", "timeZone": "UTC"},
            "endDateTime": {"dateTime": "2026-08-21T19:30:00.0000000", "timeZone": "UTC"},
            "isOutOfDate": false
        });
        assert!(is_event_message(&meeting));
        let event = mail_event(&meeting).unwrap();
        assert_eq!(event.start, "2026-08-21T18:30:00Z");
        assert_eq!(event.end, "2026-08-21T19:30:00Z");
        assert!(!event.out_of_date);

        let mut non_utc = meeting;
        non_utc["startDateTime"]["timeZone"] = serde_json::json!("Pacific Standard Time");
        assert!(mail_event(&non_utc).is_none());
    }

    #[test]
    fn a_trailing_z_on_the_graph_datetime_string_is_not_doubled() {
        let meeting = serde_json::json!({
            "meetingMessageType": "meetingRequest",
            "startDateTime": {"dateTime": "2026-08-21T18:30:00Z", "timeZone": "UTC"},
            "endDateTime": {"dateTime": "2026-08-21T19:30:00.0000000", "timeZone": "UTC"},
            "isOutOfDate": false
        });
        let event = mail_event(&meeting).unwrap();
        assert_eq!(event.start, "2026-08-21T18:30:00Z");
    }

    #[test]
    fn plain_message_has_no_event() {
        let body = serde_json::json!({
            "@odata.type": "#microsoft.graph.message",
            "body": {"contentType": "text", "content": "Synthetic request"}
        });
        assert!(!is_event_message(&body));
    }

    /// Spins a real, one-shot loopback HTTP server (no TLS -- `fetch_event`
    /// is exercised through [`fetch_from_origin`]'s testable seam, which
    /// checks the request's origin against a caller-supplied one instead of
    /// [`fetch`]'s hardcoded [`GRAPH_ORIGIN`]) that reads one request and
    /// writes back `response` verbatim, then closes. Returns the server's
    /// own origin (for use as both the request's base and the `expected_origin`
    /// argument) and a handle the caller joins once the round trip is done.
    fn one_shot_server(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            stream.write_all(&response).unwrap();
            let _ = stream.flush();
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

    fn scripted_server(
        responses: Vec<(Duration, Vec<u8>)>,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        use std::sync::{Arc, atomic::AtomicUsize};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let handle = std::thread::spawn(move || {
            for (delay, response) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                server_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                std::thread::sleep(delay);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/"), calls, handle)
    }

    fn concurrent_server(
        requests: usize,
        delay: Duration,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let server_max = Arc::clone(&max_in_flight);
        let handle = std::thread::spawn(move || {
            let in_flight = Arc::new(AtomicUsize::new(0));
            let mut handlers = Vec::with_capacity(requests);
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let in_flight = Arc::clone(&in_flight);
                let calls = Arc::clone(&server_calls);
                let max_in_flight = Arc::clone(&server_max);
                handlers.push(std::thread::spawn(move || {
                    let mut buffer = [0u8; 4096];
                    let _ = stream.read(&mut buffer);
                    calls.fetch_add(1, Ordering::SeqCst);
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    );
                    let _ = stream.flush();
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        (
            format!("http://127.0.0.1:{port}/"),
            calls,
            max_in_flight,
            handle,
        )
    }

    #[test]
    fn timeout_has_distinct_wording_after_one_retry() {
        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (origin, calls, server) = scripted_server(vec![
            (Duration::from_millis(1100), response.clone()),
            (Duration::from_millis(1100), response),
        ]);
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!("{origin}v1.0/me/messages")).unwrap();
        let result = fetch_from_origin_with_policy(
            &http,
            "synthetic-token",
            &url,
            &origin,
            Duration::from_secs(1),
            Duration::ZERO,
        );
        assert_eq!(result, Err(ConnectionError::Timeout(1)));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Microsoft did not answer within 1 seconds."
        );
        server.join().unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn service_unavailable_retries_once_then_succeeds() {
        let (origin, calls, server) = scripted_server(vec![
            (
                Duration::ZERO,
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            ),
            (
                Duration::ZERO,
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec(),
            ),
        ]);
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!("{origin}v1.0/me/messages")).unwrap();
        assert_eq!(
            fetch_from_origin_with_policy(
                &http,
                "synthetic-token",
                &url,
                &origin,
                Duration::from_secs(1),
                Duration::ZERO,
            ),
            Ok(b"{}".to_vec())
        );
        server.join().unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn throttling_is_not_retried() {
        let (origin, calls, server) = scripted_server(vec![(
            Duration::ZERO,
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        )]);
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!("{origin}v1.0/me/messages")).unwrap();
        assert_eq!(
            fetch_from_origin_with_policy(
                &http,
                "synthetic-token",
                &url,
                &origin,
                Duration::from_secs(1),
                Duration::ZERO,
            ),
            Err(ConnectionError::Throttled)
        );
        server.join().unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn one_failed_body_fetch_does_not_drop_the_source() {
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let rows = vec![
            serde_json::json!({"id":"failed"}),
            serde_json::json!({"id":"loaded"}),
        ];
        add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &[],
            &MailCache::default(),
            &LoadProgress::default(),
            |row| {
                if row["id"] == "failed" {
                    Err(ConnectionError::Timeout(GRAPH_TIMEOUT_SECONDS))
                } else {
                    Ok(MailItem {
                        id: "loaded".into(),
                        conversation: "synthetic-conversation".into(),
                        ..MailItem::default()
                    })
                }
            },
        )
        .unwrap();
        assert_eq!(source.messages.len(), 1);
        assert_eq!(source.message_errors, [ConnectionError::Timeout(90)]);
        assert!(source.errors.is_empty());
    }

    fn synthetic_rows(count: usize) -> Vec<Value> {
        (0..count)
            .map(|index| {
                serde_json::json!({
                    "id": format!("synthetic-message-{index}"),
                    "conversationId": format!("synthetic-conversation-{index}"),
                    "receivedDateTime": "2026-09-01T12:00:00Z"
                })
            })
            .collect()
    }

    fn hydrated_row(row: &Value) -> MailItem {
        MailItem {
            id: row["id"].as_str().unwrap().into(),
            conversation: row["conversationId"].as_str().unwrap().into(),
            received: row["receivedDateTime"].as_str().unwrap().into(),
            ..MailItem::default()
        }
    }

    #[test]
    fn concurrent_hydration_preserves_listing_order() {
        let rows = synthetic_rows(8);
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let progress = LoadProgress::default();
        add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &["user@example.invalid".into()],
            &MailCache::default(),
            &progress,
            |row| {
                let index = row["id"]
                    .as_str()
                    .unwrap()
                    .rsplit('-')
                    .next()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                std::thread::sleep(Duration::from_millis((8 - index) * 5));
                Ok(hydrated_row(&row))
            },
        )
        .unwrap();
        assert_eq!(
            source
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            (0..8)
                .map(|index| format!("synthetic-message-{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn load_progress_reaches_the_discovered_row_count() {
        let rows = synthetic_rows(5);
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let progress = LoadProgress::default();
        add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &[],
            &MailCache::default(),
            &progress,
            |row| Ok(hydrated_row(&row)),
        )
        .unwrap();
        assert_eq!(progress.listed.load(Ordering::Relaxed), 5);
        assert_eq!(progress.loaded.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn load_recent_with_an_existing_cancel_returns_cancelled_without_sign_in() {
        let config = ConnectionConfig::new("11111111-1111-1111-1111-111111111111", None).unwrap();
        let progress = LoadProgress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            load_recent_with(&config, &MailCache::default(), &progress),
            Err(ConnectionError::Cancelled)
        ));
    }

    #[test]
    fn concurrent_hydration_uses_at_most_four_requests() {
        let rows = synthetic_rows(8);
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let progress = LoadProgress::default();
        let (origin, calls, max_in_flight, server) =
            concurrent_server(8, Duration::from_millis(100));
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &[],
            &MailCache::default(),
            &progress,
            |row| {
                let url = Url::parse(&format!("{origin}messages/{}", row["id"].as_str().unwrap()))
                    .unwrap();
                fetch_from_origin(&http, "synthetic-token", &url, &origin)?;
                Ok(hydrated_row(&row))
            },
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 8);
        let maximum = max_in_flight.load(Ordering::SeqCst);
        assert!(
            maximum > 1,
            "expected concurrent requests, observed {maximum}"
        );
        assert!(maximum <= 4, "observed {maximum} requests in flight");
    }

    #[test]
    fn cached_hydration_makes_no_request_and_returns_the_item_unchanged() {
        let rows = synthetic_rows(1);
        let cached = MailItem {
            subject: "Cached subject".into(),
            body: "Cached synthetic body".into(),
            body_is_html: true,
            sender: "Synthetic Sender <sender@example.invalid>".into(),
            received: "2026-08-31T18:00:00Z".into(),
            id: "synthetic-message-0".into(),
            conversation: "cached-conversation".into(),
            account: "synthetic-account".into(),
            own_addresses: vec!["cached@example.invalid".into()],
            sender_address: "sender@example.invalid".into(),
            to: vec!["recipient@example.invalid".into()],
            cc: vec!["observer@example.invalid".into()],
            sent: true,
            team: true,
            web_link: "https://outlook.office.com/mail/synthetic".into(),
            event: Some(MailEvent {
                start: "2026-09-01T18:00:00Z".into(),
                end: "2026-09-01T19:00:00Z".into(),
                out_of_date: false,
            }),
        };
        let mut cache = MailCache::default();
        cache.insert(
            ("synthetic-account".into(), "synthetic-message-0".into()),
            cached.clone(),
        );
        let (origin, calls, _, server) = concurrent_server(0, Duration::ZERO);
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let progress = LoadProgress::default();
        add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &[],
            &cache,
            &progress,
            |row| {
                let url = Url::parse(&format!("{origin}messages/{}", row["id"].as_str().unwrap()))
                    .unwrap();
                fetch_from_origin(&http, "synthetic-token", &url, &origin)?;
                Ok(hydrated_row(&row))
            },
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let returned = &source.messages[0];
        assert_eq!(returned.subject, cached.subject);
        assert_eq!(returned.body, cached.body);
        assert_eq!(returned.body_is_html, cached.body_is_html);
        assert_eq!(returned.sender, cached.sender);
        assert_eq!(returned.received, cached.received);
        assert_eq!(returned.id, cached.id);
        assert_eq!(returned.conversation, cached.conversation);
        assert_eq!(returned.account, cached.account);
        assert_eq!(returned.own_addresses, cached.own_addresses);
        assert_eq!(returned.sender_address, cached.sender_address);
        assert_eq!(returned.to, cached.to);
        assert_eq!(returned.cc, cached.cc);
        assert_eq!(returned.sent, cached.sent);
        assert_eq!(returned.team, cached.team);
        assert_eq!(returned.web_link, cached.web_link);
        let returned_event = returned.event.as_ref().unwrap();
        let cached_event = cached.event.as_ref().unwrap();
        assert_eq!(returned_event.start, cached_event.start);
        assert_eq!(returned_event.end, cached_event.end);
        assert_eq!(returned_event.out_of_date, cached_event.out_of_date);
        assert_eq!(progress.loaded.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancelling_hydration_stops_dispatch_after_in_flight_rows() {
        use std::sync::atomic::AtomicUsize;

        let rows = synthetic_rows(20);
        let mut source = SourceReview {
            label: "Personal mailbox / Inbox".into(),
            messages: vec![],
            errors: vec![],
            message_errors: vec![],
            partial: false,
        };
        let progress = LoadProgress::default();
        let requests = AtomicUsize::new(0);
        let result = add_hydrated(
            &mut source,
            &rows,
            false,
            false,
            "synthetic-account",
            &[],
            &MailCache::default(),
            &progress,
            |row| {
                requests.fetch_add(1, Ordering::SeqCst);
                progress.cancel.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
                Ok(hydrated_row(&row))
            },
        );
        assert_eq!(result, Err(ConnectionError::Cancelled));
        let request_count = requests.load(Ordering::SeqCst);
        assert!((1..=4).contains(&request_count));
    }

    /// The status classification a 400 actually produces, not just that
    /// [`fetch_event`]'s best-effort `Option` collapses it to `None` --
    /// exercised through [`fetch_from_origin`] directly so the specific
    /// [`ConnectionError`] variant is visible to the assertion.
    #[test]
    fn a_400_classifies_as_bad_request() {
        let (origin, server) = one_shot_server(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        );
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!(
            "{origin}v1.0/me/messages/synthetic-id/microsoft.graph.eventMessage"
        ))
        .unwrap();
        assert_eq!(
            fetch_from_origin(&http, "synthetic-token", &url, &origin),
            Err(ConnectionError::BadRequest)
        );
        server.join().unwrap();
    }

    /// A response whose `Content-Length` header alone exceeds the bound is
    /// rejected before any body is read.
    #[test]
    fn a_large_content_length_classifies_as_response_too_large() {
        let (origin, server) = one_shot_server(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                super::super::MAX_RESPONSE + 1
            )
            .into_bytes(),
        );
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!(
            "{origin}v1.0/me/messages/synthetic-id/microsoft.graph.eventMessage"
        ))
        .unwrap();
        assert_eq!(
            fetch_from_origin(&http, "synthetic-token", &url, &origin),
            Err(ConnectionError::ResponseTooLarge)
        );
        server.join().unwrap();
    }

    #[test]
    fn extra_fetch_populates_the_event_from_a_200_with_the_four_fields() {
        let payload = serde_json::json!({
            "meetingMessageType": "meetingRequest",
            "startDateTime": {"dateTime": "2026-08-21T18:30:00.0000000", "timeZone": "UTC"},
            "endDateTime": {"dateTime": "2026-08-21T19:30:00.0000000", "timeZone": "UTC"},
            "isOutOfDate": false
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        let (origin, server) = one_shot_server(response.into_bytes());
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let url = Url::parse(&format!(
            "{origin}v1.0/me/messages/synthetic-id/microsoft.graph.eventMessage"
        ))
        .unwrap();
        let event = fetch_event(&http, "synthetic-token", &url, &origin).unwrap();
        assert_eq!(event.start, "2026-08-21T18:30:00Z");
        assert_eq!(event.end, "2026-08-21T19:30:00Z");
        assert!(!event.out_of_date);
        server.join().unwrap();
    }
    #[test]
    fn window_walks_newest_first_rows_until_the_cutoff() {
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let rows = vec![
            serde_json::json!({"receivedDateTime": "2026-01-05T00:00:00Z", "id": "a"}),
            serde_json::json!({"id": "missing-date"}),
            serde_json::json!({"receivedDateTime": "2026-01-01T00:00:00Z", "id": "b"}),
            serde_json::json!({"receivedDateTime": "2025-12-31T00:00:00Z", "id": "c"}),
            serde_json::json!({"receivedDateTime": "2025-12-01T00:00:00Z", "id": "d"}),
        ];
        let (kept, reached_cutoff, skipped) = window(rows.clone(), "receivedDateTime", cutoff, 100);
        assert_eq!(
            kept.iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(reached_cutoff);
        assert_eq!(skipped, 1);

        let (kept, reached_cutoff, _) = window(rows.clone(), "receivedDateTime", cutoff, 1);
        assert_eq!(kept.len(), 1);
        assert!(!reached_cutoff);

        let (kept, reached_cutoff, skipped) = window(vec![], "receivedDateTime", cutoff, 100);
        assert!(kept.is_empty());
        assert!(!reached_cutoff);
        assert_eq!(skipped, 0);
    }
}
