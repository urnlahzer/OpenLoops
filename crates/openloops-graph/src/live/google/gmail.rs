use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use chrono::SecondsFormat;
use reqwest::blocking::Client;
use serde_json::Value;
use url::Url;

use super::mime::{decode_rfc2047, header, parse_address_list, select_body};
use super::{GoogleConfig, with_google_session};
use crate::live::review::{
    LoadProgress, MailCache, MailItem, SourceReview, UserIdentity, add_hydrated, cutoff_timestamp,
    fetch_from_origin_with_headers,
};
use crate::live::{ConnectionError, MailProvider};

pub(super) const GMAIL_ORIGIN: &str = "https://gmail.googleapis.com/";
pub(super) const USERINFO_ORIGIN: &str = "https://openidconnect.googleapis.com/";
const LABELS: [(&str, &str); 2] = [("INBOX", "Gmail / Inbox"), ("SENT", "Gmail / Sent")];

fn bounded_text(value: &Value, field: &str, limit: usize) -> Result<String, ConnectionError> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(ConnectionError::ResourceUnavailable)?;
    if text.chars().count() > limit {
        return Err(ConnectionError::ResponseTooLarge);
    }
    Ok(text.to_owned())
}

fn optional_text(
    value: &Value,
    field: &str,
    limit: usize,
) -> Result<Option<String>, ConnectionError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => bounded_text(value, field, limit).map(|text| (!text.is_empty()).then_some(text)),
    }
}

pub(super) fn identity_from_origin(
    http: &Client,
    token: &str,
    origin: &str,
) -> Result<UserIdentity, ConnectionError> {
    let url = Url::parse(origin)
        .and_then(|url| url.join("v1/userinfo"))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let bytes = fetch_from_origin_with_headers(http, token, &url, origin, &[])?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    let sub = bounded_text(&value, "sub", 512)?;
    let email = bounded_text(&value, "email", 512)?;
    if sub.is_empty() || email.is_empty() {
        return Err(ConnectionError::ResourceUnavailable);
    }
    Ok(UserIdentity {
        provider: MailProvider::Google,
        account: format!("google:{sub}"),
        addresses: vec![email.to_ascii_lowercase()],
        display_name: optional_text(&value, "name", 4096)?,
        given_name: optional_text(&value, "given_name", 4096)?,
    })
}

/// Loads both recent Gmail sources into memory.
///
/// # Errors
/// Returns fixed, content-free configuration, authorization, transport, or cancellation errors.
pub fn load_recent_with(
    config: &GoogleConfig,
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<Vec<SourceReview>, ConnectionError> {
    load_sources_with(config, cache, progress, None)
}

/// Loads selected recent Gmail sources into memory.
///
/// # Errors
/// Returns fixed, content-free configuration, authorization, transport, or cancellation errors.
pub fn load_sources_with(
    config: &GoogleConfig,
    cache: &MailCache,
    progress: &LoadProgress,
    filter: Option<&BTreeSet<String>>,
) -> Result<Vec<SourceReview>, ConnectionError> {
    if progress.cancel.load(Ordering::Acquire) {
        return Err(ConnectionError::Cancelled);
    }
    with_google_session(config, false, |http, token| {
        load_sources_from_origins(
            http,
            token,
            cache,
            progress,
            filter,
            USERINFO_ORIGIN,
            GMAIL_ORIGIN,
        )
    })
}

fn selected(filter: Option<&BTreeSet<String>>, label: &str) -> bool {
    filter.is_none_or(|filter| {
        filter.contains(label)
            || label
                .rsplit_once(" / ")
                .is_some_and(|(mailbox, _)| filter.contains(mailbox))
    })
}

fn load_sources_from_origins(
    http: &Client,
    token: &str,
    cache: &MailCache,
    progress: &LoadProgress,
    filter: Option<&BTreeSet<String>>,
    userinfo_origin: &str,
    gmail_origin: &str,
) -> Result<Vec<SourceReview>, ConnectionError> {
    progress.listed.store(0, Ordering::Relaxed);
    progress.loaded.store(0, Ordering::Relaxed);
    progress.sources_done.store(0, Ordering::Relaxed);
    progress.sources_total.store(
        LABELS
            .iter()
            .filter(|(_, label)| selected(filter, label))
            .count(),
        Ordering::Relaxed,
    );
    let identity = identity_from_origin(http, token, userinfo_origin)?;
    let mut sources = Vec::new();
    for (remote_label, display_label) in LABELS {
        if !selected(filter, display_label) {
            continue;
        }
        if progress.cancel.load(Ordering::Acquire) {
            return Err(ConnectionError::Cancelled);
        }
        sources.push(load_label(
            http,
            token,
            gmail_origin,
            remote_label,
            display_label,
            &identity,
            cache,
            progress,
        )?);
        progress.sources_done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(sources)
}

fn listing_url(
    origin: &str,
    label: &str,
    page_token: Option<&str>,
) -> Result<Url, ConnectionError> {
    let mut url = Url::parse(origin)
        .and_then(|url| url.join("gmail/v1/users/me/messages"))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    url.query_pairs_mut()
        .append_pair("labelIds", label)
        .append_pair("q", "newer_than:30d")
        .append_pair("maxResults", "100");
    if let Some(token) = page_token {
        url.query_pairs_mut().append_pair("pageToken", token);
    }
    Ok(url)
}

pub(in crate::live) fn list_rows(
    http: &Client,
    token: &str,
    origin: &str,
    label: &str,
) -> Result<(Vec<Value>, bool), ConnectionError> {
    let mut rows = Vec::new();
    let mut page_token = None;
    for _ in 0..10 {
        let url = listing_url(origin, label, page_token.as_deref())?;
        let bytes = fetch_from_origin_with_headers(http, token, &url, origin, &[])?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
        let messages = value
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let remaining = 100usize.saturating_sub(rows.len());
        rows.extend(messages.iter().take(remaining).cloned());
        let next = value
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if rows.len() == 100 {
            return Ok((rows, next.is_some() || messages.len() > remaining));
        }
        let Some(next) = next else {
            return Ok((rows, false));
        };
        page_token = Some(next.to_owned());
    }
    Ok((rows, true))
}

#[expect(
    clippy::too_many_arguments,
    reason = "mailbox loading keeps the request, identity, cache, and progress inputs explicit"
)]
fn load_label(
    http: &Client,
    token: &str,
    origin: &str,
    remote_label: &str,
    display_label: &str,
    identity: &UserIdentity,
    cache: &MailCache,
    progress: &LoadProgress,
) -> Result<SourceReview, ConnectionError> {
    let mut source = SourceReview {
        provider: MailProvider::Google,
        label: display_label.to_owned(),
        messages: vec![],
        errors: vec![],
        message_errors: vec![],
        partial: false,
        failed: false,
    };
    let (rows, partial) = match list_rows(http, token, origin, remote_label) {
        Ok(result) => result,
        Err(ConnectionError::Unauthorized) => return Err(ConnectionError::Unauthorized),
        Err(error) => {
            source.errors.push(error);
            source.failed = true;
            return Ok(source);
        }
    };
    source.partial = partial;
    add_hydrated(
        &mut source,
        &rows,
        remote_label == "SENT",
        false,
        identity,
        cache,
        progress,
        |row| hydrate(http, token, origin, &row),
    )?;
    let cutoff = cutoff_timestamp();
    source.messages.retain(|message| {
        chrono::DateTime::parse_from_rfc3339(&message.received)
            .is_ok_and(|received| received.timestamp() >= cutoff)
    });
    Ok(source)
}

fn message_url(origin: &str, id: &str) -> Result<Url, ConnectionError> {
    if id.is_empty() || id == "." || id == ".." {
        return Err(ConnectionError::ResourceUnavailable);
    }
    let mut url = Url::parse(origin).map_err(|_| ConnectionError::InvalidConfiguration)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?;
        path.pop_if_empty()
            .extend(["gmail", "v1", "users", "me", "messages"])
            .push(id);
    }
    url.query_pairs_mut().append_pair("format", "full");
    Ok(url)
}

fn hydrate(
    http: &Client,
    token: &str,
    origin: &str,
    row: &Value,
) -> Result<MailItem, ConnectionError> {
    let id = bounded_text(row, "id", 2048)?;
    let url = message_url(origin, &id)?;
    let bytes =
        fetch_from_origin_with_headers(http, token, &url, origin, &[]).map_err(|error| {
            if error == ConnectionError::ResponseTooLarge {
                ConnectionError::MessageTooLarge
            } else {
                error
            }
        })?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    if value.get("id").and_then(Value::as_str) != Some(id.as_str()) {
        return Err(ConnectionError::ResourceUnavailable);
    }
    item(&value)
}

fn bounded_decoded_header(
    payload: &Value,
    name: &str,
    limit: usize,
) -> Result<String, ConnectionError> {
    let decoded = decode_rfc2047(header(payload, name).unwrap_or_default());
    if decoded.chars().count() > limit {
        Err(ConnectionError::ResponseTooLarge)
    } else {
        Ok(decoded)
    }
}

fn checked_addresses(
    payload: &Value,
    name: &str,
) -> Result<Vec<(Option<String>, String)>, ConnectionError> {
    let addresses = parse_address_list(header(payload, name).unwrap_or_default());
    if addresses.iter().any(|(display_name, address)| {
        display_name
            .as_ref()
            .is_some_and(|name| name.chars().count() > 4096)
            || address.chars().count() > 512
    }) {
        Err(ConnectionError::ResponseTooLarge)
    } else {
        Ok(addresses)
    }
}

fn item(value: &Value) -> Result<MailItem, ConnectionError> {
    let payload = value
        .get("payload")
        .ok_or(ConnectionError::ResourceUnavailable)?;
    let id = bounded_text(value, "id", 2048)?;
    let conversation = bounded_text(value, "threadId", 2048)?;
    let milliseconds = value
        .get("internalDate")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(ConnectionError::ResourceUnavailable)?;
    let received = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds)
        .ok_or(ConnectionError::ResourceUnavailable)?
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let (body, body_is_html) = select_body(payload)?.ok_or(ConnectionError::ResourceUnavailable)?;
    let sender_rows = checked_addresses(payload, "From")?;
    let (sender_name, sender_address) = sender_rows.into_iter().next().unwrap_or_default();
    let sender = sender_name.map_or_else(
        || sender_address.clone(),
        |name| format!("{name} <{sender_address}>"),
    );
    let addresses = |name| -> Result<Vec<String>, ConnectionError> {
        Ok(checked_addresses(payload, name)?
            .into_iter()
            .map(|(_, address)| address)
            .collect())
    };
    let labels = value
        .get("labelIds")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    Ok(MailItem {
        provider: MailProvider::Google,
        subject: bounded_decoded_header(payload, "Subject", 8192)?,
        body,
        body_is_html,
        sender,
        received,
        id: id.clone(),
        conversation,
        sender_address,
        to: addresses("To")?,
        cc: addresses("Cc")?,
        sent: labels.iter().any(|label| label.as_str() == Some("SENT")),
        team: false,
        web_link: format!("https://mail.google.com/mail/u/0/#all/{id}"),
        event: None,
        ..MailItem::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::base64url_encode;
    use crate::live::test_support::routed_server;

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn identity_body() -> String {
        serde_json::json!({"sub":"synthetic-sub","email":"user@example.invalid"}).to_string()
    }

    fn message_body(id: &str, thread: &str, encoded_body: &str, date: &str) -> String {
        serde_json::json!({
            "id":id, "threadId":thread, "internalDate":date, "labelIds":["INBOX"],
            "payload":{"mimeType":"text/plain","headers":[
                {"name":"Subject","value":"Synthetic subject"},
                {"name":"From","value":"sender@example.invalid"}
            ],"body":{"data":encoded_body}}
        })
        .to_string()
    }

    #[test]
    fn identity_is_bounded_and_normalized() {
        let body = serde_json::json!({"sub":"synthetic-sub","email":"USER@example.invalid","name":"Synthetic User","given_name":"Synthetic"}).to_string();
        let (origin, _, server) = routed_server(vec![("/v1/userinfo", response("200 OK", &body))]);
        let http = Client::builder().no_proxy().build().unwrap();
        let identity = identity_from_origin(&http, "synthetic-token", &origin).unwrap();
        assert_eq!(identity.account, "google:synthetic-sub");
        assert_eq!(identity.addresses, ["user@example.invalid"]);
        server.join().unwrap();
    }

    #[test]
    fn identity_rejects_empty_or_missing_subject() {
        for body in [
            serde_json::json!({"sub":"","email":"user@example.invalid"}).to_string(),
            serde_json::json!({"email":"user@example.invalid"}).to_string(),
        ] {
            let (origin, _, server) =
                routed_server(vec![("/v1/userinfo", response("200 OK", &body))]);
            let http = Client::builder().no_proxy().build().unwrap();
            assert!(matches!(
                identity_from_origin(&http, "synthetic-token", &origin),
                Err(ConnectionError::ResourceUnavailable)
            ));
            server.join().unwrap();
        }
    }

    #[test]
    fn item_maps_gmail_shape_and_mime() {
        let body = base64url_encode(b"<p>Synthetic body</p>");
        let value = serde_json::json!({
            "id":"synthetic-id", "threadId":"synthetic-thread", "internalDate":"1893456000000", "labelIds":["SENT"],
            "payload":{"mimeType":"text/html","headers":[
                {"name":"Subject","value":"=?UTF-8?B?U3ludGhldGljIOKckw==?="},
                {"name":"From","value":"Synthetic Sender <SENDER@example.invalid>"},
                {"name":"To","value":"TO@example.invalid"}, {"name":"Cc","value":"CC@example.invalid"}
            ],"body":{"data":body}}
        });
        let message = item(&value).unwrap();
        assert_eq!(message.subject, "Synthetic \u{2713}");
        assert_eq!(message.conversation, "synthetic-thread");
        assert_eq!(message.received, "2030-01-01T00:00:00Z");
        assert_eq!(message.sender, "Synthetic Sender <sender@example.invalid>");
        assert_eq!(message.sender_address, "sender@example.invalid");
        assert_eq!(message.cc, ["cc@example.invalid"]);
        assert_eq!(
            message.web_link,
            "https://mail.google.com/mail/u/0/#all/synthetic-id"
        );
        assert!(message.sent);
        assert_eq!(message.to, ["to@example.invalid"]);
    }

    #[test]
    fn item_bounds_subject_sender_name_and_addresses() {
        let fixture = |subject: String, from: String| {
            serde_json::json!({
                "id":"synthetic-id", "threadId":"synthetic-thread",
                "internalDate":"1893456000000",
                "payload":{"mimeType":"text/plain","headers":[
                    {"name":"Subject","value":subject},
                    {"name":"From","value":from}
                ],"body":{"data":"Zm9v"}}
            })
        };
        for value in [
            fixture("x".repeat(8193), "sender@example.invalid".into()),
            fixture(
                "Synthetic".into(),
                format!("{} <sender@example.invalid>", "x".repeat(4097)),
            ),
            fixture(
                "Synthetic".into(),
                format!("Synthetic <{}@example.invalid>", "x".repeat(500)),
            ),
        ] {
            assert!(matches!(
                item(&value),
                Err(ConnectionError::ResponseTooLarge)
            ));
        }
    }

    #[test]
    fn malformed_body_is_a_message_error_only() {
        let identity =
            serde_json::json!({"sub":"synthetic-sub","email":"user@example.invalid"}).to_string();
        let listing =
            serde_json::json!({"messages":[{"id":"synthetic-id","threadId":"synthetic-thread"}]})
                .to_string();
        let message = serde_json::json!({"id":"synthetic-id","threadId":"synthetic-thread","internalDate":"1893456000000","payload":{"mimeType":"text/plain","body":{"data":"!"}}}).to_string();
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity)),
            ("/gmail/v1/users/me/messages?", response("200 OK", &listing)),
            (
                "/gmail/v1/users/me/messages/synthetic-id",
                response("200 OK", &message),
            ),
        ]);
        let http = Client::builder().no_proxy().build().unwrap();
        let filter = BTreeSet::from(["Gmail / Inbox".to_owned()]);
        let sources = load_sources_from_origins(
            &http,
            "synthetic-token",
            &MailCache::default(),
            &LoadProgress::default(),
            Some(&filter),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert!(!sources[0].failed);
        assert_eq!(
            sources[0].message_errors,
            [ConnectionError::ResourceUnavailable]
        );
    }

    #[test]
    fn routed_loads_both_labels_and_keeps_message_failures_local() {
        let inbox = serde_json::json!({"messages":[
            {"id":"synthetic-html","threadId":"thread-html"},
            {"id":"synthetic-plain","threadId":"thread-plain"}
        ]})
        .to_string();
        let sent = serde_json::json!({"messages":[
            {"id":"synthetic-sent","threadId":"thread-sent"},
            {"id":"synthetic-large","threadId":"thread-large"}
        ]})
        .to_string();
        let html = serde_json::json!({
            "id":"synthetic-html","threadId":"thread-html","internalDate":"1893456000000","labelIds":["INBOX"],
            "payload":{"mimeType":"multipart/alternative","headers":[
                {"name":"Subject","value":"Synthetic HTML"},
                {"name":"From","value":"Synthetic Sender <sender@example.invalid>"}
            ],"parts":[
                {"mimeType":"text/plain","body":{"data":base64url_encode(b"Synthetic plain")}},
                {"mimeType":"text/html","body":{"data":base64url_encode(b"<p>Synthetic HTML</p>")}}
            ]}
        }).to_string();
        let plain = message_body(
            "synthetic-plain",
            "thread-plain",
            &base64url_encode(b"Synthetic plain"),
            "1893456000000",
        );
        let sent_message = serde_json::json!({
            "id":"synthetic-sent","threadId":"thread-sent","internalDate":"1893456000000","labelIds":["SENT"],
            "payload":{"mimeType":"text/plain","headers":[
                {"name":"Subject","value":"Synthetic sent"},
                {"name":"From","value":"sender@example.invalid"}
            ],"body":{"data":base64url_encode(b"Synthetic sent body")}}
        }).to_string();
        let oversized = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            super::super::super::MAX_RESPONSE + 1
        )
        .into_bytes();
        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity_body())),
            (
                "/gmail/v1/users/me/messages?labelIds=INBOX",
                response("200 OK", &inbox),
            ),
            (
                "/gmail/v1/users/me/messages?labelIds=SENT",
                response("200 OK", &sent),
            ),
            (
                "/gmail/v1/users/me/messages/synthetic-html",
                response("200 OK", &html),
            ),
            (
                "/gmail/v1/users/me/messages/synthetic-plain",
                response("200 OK", &plain),
            ),
            (
                "/gmail/v1/users/me/messages/synthetic-sent",
                response("200 OK", &sent_message),
            ),
            ("/gmail/v1/users/me/messages/synthetic-large", oversized),
        ]);
        let http = Client::builder().no_proxy().build().unwrap();
        let sources = load_sources_from_origins(
            &http,
            "synthetic-token",
            &MailCache::default(),
            &LoadProgress::default(),
            None,
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].messages.len(), 2);
        assert_eq!(sources[0].messages[0].body, "<p>Synthetic HTML</p>");
        assert!(sources[0].messages[0].body_is_html);
        assert_eq!(sources[0].messages[1].body, "Synthetic plain");
        assert!(!sources[0].failed);
        assert_eq!(
            sources[1].message_errors,
            [ConnectionError::MessageTooLarge]
        );
        assert_eq!(sources[1].messages.len(), 1);
        assert!(sources[1].messages[0].sent);
        assert!(!sources[1].failed);
    }

    #[test]
    fn paging_to_the_hundred_row_cap_is_partial() {
        let first = (0..60)
            .map(|index| serde_json::json!({"id":format!("synthetic-a-{index}"),"threadId":format!("thread-a-{index}")}))
            .collect::<Vec<_>>();
        let second = (0..50)
            .map(|index| serde_json::json!({"id":format!("synthetic-b-{index}"),"threadId":format!("thread-b-{index}")}))
            .collect::<Vec<_>>();
        let page_one =
            serde_json::json!({"messages":first,"nextPageToken":"synthetic-next"}).to_string();
        let page_two =
            serde_json::json!({"messages":second,"nextPageToken":"synthetic-more"}).to_string();
        let specific = "/gmail/v1/users/me/messages?labelIds=INBOX&q=newer_than%3A30d&maxResults=100&pageToken=synthetic-next";
        let (origin, _, server) = routed_server(vec![
            (specific, response("200 OK", &page_two)),
            (
                "/gmail/v1/users/me/messages?labelIds=INBOX",
                response("200 OK", &page_one),
            ),
        ]);
        let http = Client::builder().no_proxy().build().unwrap();
        let (rows, partial) = list_rows(&http, "synthetic-token", &origin, "INBOX").unwrap();
        server.join().unwrap();
        assert_eq!(rows.len(), 100);
        assert!(partial);
    }

    #[test]
    fn paging_stops_at_ten_pages_and_is_partial() {
        let page = serde_json::json!({
            "messages":[{"id":"synthetic-id","threadId":"synthetic-thread"}],
            "nextPageToken":"synthetic-next"
        })
        .to_string();
        let routes = (0..10)
            .map(|_| {
                (
                    "/gmail/v1/users/me/messages?labelIds=INBOX",
                    response("200 OK", &page),
                )
            })
            .collect();
        let (origin, calls, server) = routed_server(routes);
        let http = Client::builder().no_proxy().build().unwrap();
        let (rows, partial) = list_rows(&http, "synthetic-token", &origin, "INBOX").unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 10);
        assert_eq!(rows.len(), 10);
        assert!(partial);
    }

    #[test]
    fn hydrate_rejects_a_different_response_id() {
        let body = message_body(
            "different-id",
            "synthetic-thread",
            &base64url_encode(b"Synthetic body"),
            "1893456000000",
        );
        let (origin, _, server) = routed_server(vec![(
            "/gmail/v1/users/me/messages/requested-id",
            response("200 OK", &body),
        )]);
        let http = Client::builder().no_proxy().build().unwrap();
        let row = serde_json::json!({"id":"requested-id"});
        assert!(matches!(
            hydrate(&http, "synthetic-token", &origin, &row),
            Err(ConnectionError::ResourceUnavailable)
        ));
        server.join().unwrap();
    }

    #[test]
    fn cache_hit_skips_fetch_and_cutoff_drops_old_message() {
        let listing = serde_json::json!({"messages":[
            {"id":"synthetic-cached","threadId":"thread-cached"}
        ]})
        .to_string();
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity_body())),
            (
                "/gmail/v1/users/me/messages?labelIds=INBOX",
                response("200 OK", &listing),
            ),
        ]);
        let mut cache = MailCache::default();
        cache.insert(
            ("google:synthetic-sub".into(), "synthetic-cached".into()),
            MailItem {
                provider: MailProvider::Google,
                id: "synthetic-cached".into(),
                conversation: "thread-cached".into(),
                received: "2030-01-01T00:00:00Z".into(),
                body: "Synthetic cached body".into(),
                ..MailItem::default()
            },
        );
        let http = Client::builder().no_proxy().build().unwrap();
        let filter = BTreeSet::from(["Gmail / Inbox".to_owned()]);
        let sources = load_sources_from_origins(
            &http,
            "synthetic-token",
            &cache,
            &LoadProgress::default(),
            Some(&filter),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(sources[0].messages[0].body, "Synthetic cached body");

        let old_listing = serde_json::json!({"messages":[
            {"id":"synthetic-old","threadId":"thread-old"}
        ]})
        .to_string();
        let old = message_body(
            "synthetic-old",
            "thread-old",
            &base64url_encode(b"Synthetic old"),
            "946684800000",
        );
        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", response("200 OK", &identity_body())),
            (
                "/gmail/v1/users/me/messages?labelIds=INBOX",
                response("200 OK", &old_listing),
            ),
            (
                "/gmail/v1/users/me/messages/synthetic-old",
                response("200 OK", &old),
            ),
        ]);
        let sources = load_sources_from_origins(
            &http,
            "synthetic-token",
            &MailCache::default(),
            &LoadProgress::default(),
            Some(&filter),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert!(sources[0].messages.is_empty());
        assert!(sources[0].message_errors.is_empty());
    }
}
