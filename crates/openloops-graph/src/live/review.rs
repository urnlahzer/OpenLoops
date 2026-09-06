//! Explicit, bounded, read-only review. No following server-provided URLs or storing mail.
use super::{
    Client, ConnectionConfig, ConnectionError, SharedScope, Url, bounded_body, classify_status,
    groups, inbox_url, with_session,
};
use serde_json::Value;

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
}

pub struct SourceReview {
    pub label: String,
    pub messages: Vec<MailItem>,
    pub errors: Vec<ConnectionError>,
    pub partial: bool,
}

/// Browser sign-in, then personal/shared Inbox messages and selected Group thread posts.
/// Only invoke after the user chooses to load message content for review.
/// # Errors
/// Returns fixed configuration/authentication errors. Source-specific failures remain visible.
pub fn load_recent(config: &ConnectionConfig) -> Result<Vec<SourceReview>, ConnectionError> {
    if 1 + config.group_inboxes.len() + config.shared_mailboxes.len() > 10 {
        return Err(ConnectionError::InvalidConfiguration);
    }
    with_session(config, |http, token, scope| {
        let (account, addresses) = identity(http, token)?;
        let mut sources = vec![load_mailbox(http, token, None)];
        sources.push(load_sent(http, token, None));
        for address in &config.group_inboxes {
            sources.push(load_group(http, token, address));
        }
        for address in &config.shared_mailboxes {
            if scope == SharedScope::Missing {
                sources.push(SourceReview {
                    label: address.clone(),
                    messages: vec![],
                    errors: vec![ConnectionError::MissingSharedScope],
                    partial: false,
                });
            } else {
                sources.push(load_mailbox(http, token, Some(address)));
                sources.push(load_sent(http, token, Some(address)));
            }
        }
        for source in &mut sources {
            for message in &mut source.messages {
                message.account.clone_from(&account);
                message.own_addresses.clone_from(&addresses);
            }
        }
        Ok(sources)
    })
}

pub(super) fn fetch(http: &Client, token: &str, url: Url) -> Result<Vec<u8>, ConnectionError> {
    if url.scheme() != "https"
        || url.host_str() != Some("graph.microsoft.com")
        || url.port().is_some()
    {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let response = http
        .get(url)
        .bearer_auth(token)
        .header(
            "Prefer",
            "outlook.body-content-type=\"html\", IdType=\"ImmutableId\"",
        )
        .send()
        .map_err(|_| ConnectionError::Transport)?;
    if response.status().as_u16() != 200 {
        return Err(classify_status(response.status().as_u16()));
    }
    bounded_body(response)
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
    let url = Url::parse("https://graph.microsoft.com/v1.0/me?$select=id,mail,userPrincipalName")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let value: Value = serde_json::from_slice(&fetch(http, token, url)?)
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
    let mut url = original.clone();
    let mut all = vec![];
    for _ in 0..10 {
        let bytes = fetch(http, token, url)?;
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
/// `mailbox_url`. Never follows a server-provided URL; always the fixed Graph origin.
fn message_url(address: Option<&str>, id: &str) -> Result<Url, ConnectionError> {
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
    }
    url.query_pairs_mut().append_pair("$select", "body");
    Ok(url)
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
    let bytes = fetch(http, token, message_url(address, &id)?).map_err(|error| match error {
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
    item(&row, None)
}

fn load_mailbox(http: &Client, token: &str, address: Option<&str>) -> SourceReview {
    load_folder(http, token, address, false)
}

fn load_sent(http: &Client, token: &str, address: Option<&str>) -> SourceReview {
    load_folder(http, token, address, true)
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
        let bytes = match fetch(http, token, current.clone()) {
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

fn load_folder(http: &Client, token: &str, address: Option<&str>, sent: bool) -> SourceReview {
    let mut source = SourceReview {
        label: format!(
            "{} / {}",
            address.unwrap_or("Personal mailbox"),
            if sent { "Sent Items" } else { "Inbox" }
        ),
        messages: vec![],
        errors: vec![],
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
            return source;
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
    for row in collected {
        let sent_time = if sent {
            text(&row, "sentDateTime", 64).ok()
        } else {
            None
        };
        match hydrate(http, token, address, row) {
            Ok(mut message) => {
                message.sent = sent;
                message.team = address.is_some();
                if let Some(sent_time) = sent_time {
                    message.received = sent_time;
                }
                if message.id.is_empty() || message.conversation.is_empty() {
                    source.errors.push(ConnectionError::ResourceUnavailable);
                } else {
                    source.messages.push(message);
                }
            }
            Err(error) => source.errors.push(error),
        }
    }
    source
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
        partial: false,
    };
    let result = (|| {
        let id = groups::resolve_id(&fetch(http, token, groups::lookup_url(address)?)?)?;
        let (threads, partial) = page(&fetch(http, token, group_url(&id, None)?)?)?;
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
                let mut messages = posts
                    .iter()
                    .map(|post| item(post, Some(&topic)))
                    .collect::<Result<Vec<_>, _>>()?;
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
