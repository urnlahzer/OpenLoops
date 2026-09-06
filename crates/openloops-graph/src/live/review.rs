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
        return Err(ConnectionError::ResourceUnavailable);
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
        let next = next_page(&value, original)?;
        let overflow = rows.len() > cap - all.len();
        all.extend(rows.into_iter().take(cap - all.len()));
        if overflow || (all.len() == cap && next.is_some()) {
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
        body: text(body, "content", 131_072)?,
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
        .append_pair("$select", "id,conversationId,subject,body,sender,from,toRecipients,ccRecipients,receivedDateTime,sentDateTime,webLink")
        .append_pair(
            "$orderby",
            if sent { "sentDateTime desc" } else { "receivedDateTime desc" },
        )
        .append_pair("$top", "100");
    Ok(url)
}

fn load_mailbox(http: &Client, token: &str, address: Option<&str>) -> SourceReview {
    load_folder(http, token, address, false)
}

fn load_sent(http: &Client, token: &str, address: Option<&str>) -> SourceReview {
    load_folder(http, token, address, true)
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
    let result = mailbox_url(address, sent).and_then(|url| pages(http, token, &url, 100));
    match result {
        Err(error) => source.errors.push(error),
        Ok((rows, partial)) => {
            source.partial = partial;
            for row in &rows {
                let date_field = if sent {
                    "sentDateTime"
                } else {
                    "receivedDateTime"
                };
                let in_window = text(row, date_field, 64)
                    .ok()
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
                    .is_some_and(|value| value.timestamp() >= cutoff_timestamp());
                if !in_window {
                    continue;
                }
                match item(row, None) {
                    Ok(mut message) => {
                        message.sent = sent;
                        message.team = address.is_some();
                        if sent {
                            message.received =
                                text(row, "sentDateTime", 64).unwrap_or(message.received);
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
        ] {
            assert!(next_page(&serde_json::json!({"@odata.nextLink":next}), &original).is_err());
        }
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
        assert!(item(&value, Some("topic")).is_err());
        assert!(item(&serde_json::json!({}), None).is_err());
    }
}
