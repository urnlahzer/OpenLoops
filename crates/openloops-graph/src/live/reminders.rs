//! Explicit personal To Do creation. No automatic retry, email sending or shared task writes.
use super::{
    Client, ConnectionConfig, ConnectionError, GRAPH_TIMEOUT_SECONDS, Url, bounded_body,
    request_error, review, with_scopes,
};
use serde_json::{Value, json};

pub struct ReminderRequest {
    pub account: String,
    pub title: String,
    pub at_utc: i64,
    pub marker: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReminderFailure {
    AccountMismatch,
    InvalidDraft,
    DefaultListNotFound,
    Rejected(ConnectionError),
}

impl std::fmt::Display for ReminderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountMismatch => f.write_str(
                "The browser signed into a different account than the one that was scanned. Sign in with the scanned account and try again.",
            ),
            Self::InvalidDraft => f.write_str(
                "The reminder draft is not valid: the title needs 3 to 320 plain characters and the time must be in the future.",
            ),
            Self::DefaultListNotFound => f.write_str(
                "Microsoft To Do did not return a single default Tasks list for this account. Open To Do once so the account's lists exist, then try again.",
            ),
            Self::Rejected(error)
                if matches!(
                    error,
                    ConnectionError::AccessDenied | ConnectionError::Unauthorized
                ) =>
            {
                write!(
                    f,
                    "Microsoft refused the To Do write. The app registration needs the delegated Tasks.ReadWrite permission and the signed-in account must consent to it. {error}"
                )
            }
            Self::Rejected(error) => error.fmt(f),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReminderOutcome {
    /// The task list and task id Graph reported, so a later action (e.g.
    /// marking it complete once the review card is Handled) can address the
    /// same task without re-resolving the default list.
    Created {
        list_id: String,
        task_id: String,
    },
    NotCreated(ReminderFailure),
    Uncertain,
}

/// Outcome of marking an already-created task complete. Mirrors
/// [`ReminderOutcome`]'s three-way shape: `create()`'s caller cannot tell a
/// genuine failure from an unconfirmed one either, so neither can this.
#[derive(Clone, Copy)]
pub enum ReminderCompletionOutcome {
    Completed,
    NotCompleted(ConnectionError),
    Uncertain,
}

/// A Graph task-list or task id: non-empty, not `.`/`..`, and bounded, same
/// as the id check `create()` already applies to the resolved list id.
fn valid_graph_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && id.len() <= 2048
}

fn task_body(request: &ReminderRequest) -> Result<Vec<u8>, ReminderFailure> {
    let now: chrono::DateTime<chrono::Utc> = std::time::SystemTime::now().into();
    if request.account.is_empty()
        || request.title.trim().chars().count() < 3
        || request.title.chars().count() > 320
        || request.title.chars().any(char::is_control)
        || request.at_utc <= now.timestamp()
        || request.marker.len() != 64
        || !request.marker.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ReminderFailure::InvalidDraft);
    }
    let when = chrono::DateTime::from_timestamp(request.at_utc, 0)
        .ok_or(ReminderFailure::InvalidDraft)?
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    // dueDateTime and reminderDateTime are independent concepts in Microsoft
    // To Do -- a due date with no reminder, or vice versa, is a normal task
    // shape. This app only ever collects one date/time from the user (the
    // reminder draft's own "when"), so there is nothing else to derive a due
    // date from; using that same instant for both means the task at least
    // shows up as due on the day it was set to alert, rather than carrying
    // no due date at all.
    serde_json::to_vec(&json!({"title":request.title,"body":{"contentType":"text","content":format!("Created after review in OpenLoops.\nOpenLoops reference: {}",request.marker)},"isReminderOn":true,"reminderDateTime":{"dateTime":when,"timeZone":"UTC"},"dueDateTime":{"dateTime":when,"timeZone":"UTC"}})).map_err(|_|ReminderFailure::InvalidDraft)
}

/// Creates exactly one reviewed task, with an independently authorized session.
/// Caller must durably record the attempt before invoking this function.
#[must_use]
pub fn create(config: &ConnectionConfig, request: &ReminderRequest) -> ReminderOutcome {
    let body = match task_body(request) {
        Ok(b) => b,
        Err(e) => return ReminderOutcome::NotCreated(e),
    };
    reminder_result(with_scopes(config, true, |http, token, _| {
        create_after_sign_in(http, token, &request.account, &body, review::GRAPH_ORIGIN)
    }))
}

fn reminder_result(result: Result<ReminderOutcome, ConnectionError>) -> ReminderOutcome {
    match result {
        Ok(outcome) => outcome,
        Err(error) => ReminderOutcome::NotCreated(ReminderFailure::Rejected(error)),
    }
}

fn create_after_sign_in(
    http: &Client,
    token: &str,
    scanned_account: &str,
    body: &[u8],
    graph_origin: &str,
) -> Result<ReminderOutcome, ConnectionError> {
    let (signed_in_account, _) = review::identity_from_origin(http, token, graph_origin)?;
    if signed_in_account != scanned_account {
        return Ok(ReminderOutcome::NotCreated(
            ReminderFailure::AccountMismatch,
        ));
    }
    // No $select/$top: Microsoft's own documented example for this
    // endpoint (learn.microsoft.com/graph/api/todo-list-lists) is a bare
    // GET with no query parameters, and its example response already
    // includes both `id` and `wellknownListName` on every list without
    // selecting them. A live HTTP 400 traced to this call when
    // `$select=id,wellknownListName&$top=100` was present; this endpoint's
    // OData query support is documented only as "some" parameters, not
    // confirmed to include either of these two.
    let Ok(url) = Url::parse(graph_origin).and_then(|url| url.join("v1.0/me/todo/lists")) else {
        return Err(ConnectionError::InvalidConfiguration);
    };
    let (lists, partial) = review::pages_from_origin(http, token, &url, 100, graph_origin)?;
    let matches: Vec<_> = lists
        .iter()
        .filter(|value| value["wellknownListName"] == "defaultList")
        .collect();
    if partial || matches.len() != 1 {
        return Ok(ReminderOutcome::NotCreated(
            ReminderFailure::DefaultListNotFound,
        ));
    }
    let Some(id) = matches[0]["id"].as_str().filter(|id| valid_graph_id(id)) else {
        return Ok(ReminderOutcome::NotCreated(
            ReminderFailure::DefaultListNotFound,
        ));
    };
    let Ok(mut url) = Url::parse(graph_origin).and_then(|url| url.join("v1.0/me/todo/lists/"))
    else {
        return Err(ConnectionError::InvalidConfiguration);
    };
    let Ok(mut segments) = url.path_segments_mut() else {
        return Err(ConnectionError::InvalidConfiguration);
    };
    segments.pop_if_empty().push(id).push("tasks");
    drop(segments);

    let Ok(response) = http
        .post(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
    else {
        return Ok(ReminderOutcome::Uncertain);
    };
    if response.status().as_u16() != 201 {
        return Ok(ReminderOutcome::Uncertain);
    }
    let Ok(bytes) = bounded_body(response) else {
        return Ok(ReminderOutcome::Uncertain);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(ReminderOutcome::Uncertain);
    };
    match value["id"]
        .as_str()
        .filter(|task_id| valid_graph_id(task_id))
    {
        Some(task_id) => Ok(ReminderOutcome::Created {
            list_id: id.to_owned(),
            task_id: task_id.to_owned(),
        }),
        None => Ok(ReminderOutcome::Uncertain),
    }
}

/// Marks an already-created task complete. Never called until a card is
/// marked Handled locally, and never reverts that local decision on
/// failure -- see `on_review_decision` in `slint_review.rs`: the task
/// genuinely exists either way, so a failed completion here only means the
/// user may need to complete it manually in Microsoft To Do, not that
/// anything about the saved decision was wrong.
#[must_use]
pub fn complete(
    config: &ConnectionConfig,
    account: &str,
    list_id: &str,
    task_id: &str,
) -> ReminderCompletionOutcome {
    if !valid_graph_id(list_id) || !valid_graph_id(task_id) {
        return ReminderCompletionOutcome::NotCompleted(ConnectionError::InvalidConfiguration);
    }
    let mut dispatched = false;
    let result = with_scopes(config, true, |http, token, _| {
        let (signed_in, _) = review::identity(http, token)?;
        if signed_in != account {
            return Err(ConnectionError::InvalidConfiguration);
        }
        let mut url = Url::parse("https://graph.microsoft.com/v1.0/me/todo/lists/")
            .map_err(|_| ConnectionError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?
            .pop_if_empty()
            .push(list_id)
            .push("tasks")
            .push(task_id);
        dispatched = true;
        let response = http
            .patch(url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&json!({"status": "completed"}))
                    .map_err(|_| ConnectionError::InvalidConfiguration)?,
            )
            .send()
            .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
        match response.status().as_u16() {
            200 => Ok(()),
            404 => Err(ConnectionError::NotFound),
            _ => Err(ConnectionError::ResourceUnavailable),
        }
    });
    match result {
        Ok(()) => ReminderCompletionOutcome::Completed,
        Err(_) if dispatched => ReminderCompletionOutcome::Uncertain,
        Err(e) => ReminderCompletionOutcome::NotCompleted(e),
    }
}

/// Whether a previously created task's `status` currently reads as
/// completed. Read-only: never writes anything back to Graph. A task
/// deleted since creation (404), or any other failure, is `Unknown` rather
/// than `NotCompleted` -- the caller must not treat "couldn't check" the
/// same as "confirmed still open".
pub enum TaskStatusOutcome {
    Completed,
    NotCompleted,
    Unknown(ConnectionError),
}

#[must_use]
pub fn check_status(
    config: &ConnectionConfig,
    account: &str,
    list_id: &str,
    task_id: &str,
) -> TaskStatusOutcome {
    if !valid_graph_id(list_id) || !valid_graph_id(task_id) {
        return TaskStatusOutcome::Unknown(ConnectionError::InvalidConfiguration);
    }
    let result = with_scopes(config, true, |http, token, _| {
        let (signed_in, _) = review::identity(http, token)?;
        if signed_in != account {
            return Err(ConnectionError::InvalidConfiguration);
        }
        let mut url = Url::parse("https://graph.microsoft.com/v1.0/me/todo/lists/")
            .map_err(|_| ConnectionError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?
            .pop_if_empty()
            .push(list_id)
            .push("tasks")
            .push(task_id);
        // No $select: the lists lookup's own HTTP 400 traced to $select
        // combined with $top on that collection endpoint; a single-resource
        // GET is a different, more standard shape, but there's no need to
        // take the same risk twice for one small field on one object.
        let response = http
            .get(url)
            .bearer_auth(token)
            .send()
            .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
        match response.status().as_u16() {
            200 => {
                let value: Value = serde_json::from_slice(&bounded_body(response)?)
                    .map_err(|_| ConnectionError::ResourceUnavailable)?;
                Ok(value["status"] == "completed")
            }
            404 => Err(ConnectionError::NotFound),
            _ => Err(ConnectionError::ResourceUnavailable),
        }
    });
    match result {
        Ok(true) => TaskStatusOutcome::Completed,
        Ok(false) => TaskStatusOutcome::NotCompleted,
        Err(e) => TaskStatusOutcome::Unknown(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn scripted_server(
        responses: Vec<Vec<u8>>,
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let handle = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                server_calls.fetch_add(1, Ordering::Relaxed);
                stream.write_all(&response).unwrap();
                stream.flush().unwrap();
            }
        });
        (format!("http://127.0.0.1:{port}/"), calls, handle)
    }

    fn valid_request() -> ReminderRequest {
        ReminderRequest {
            account: "scanned-account".into(),
            title: "Send the draft".into(),
            at_utc: 4_000_000_000,
            marker: "a".repeat(64),
        }
    }

    fn test_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
    }

    #[test]
    fn reminder_payload_is_exact_reviewed_action_not_model_evidence() {
        let request = valid_request();
        let value: Value = serde_json::from_slice(&task_body(&request).unwrap()).unwrap();
        assert_eq!(value["title"], "Send the draft");
        assert_eq!(value["isReminderOn"], true);
        assert_eq!(value["dueDateTime"], value["reminderDateTime"]);
        let mut bad = request;
        bad.at_utc = 1;
        assert_eq!(task_body(&bad), Err(ReminderFailure::InvalidDraft));
    }

    #[test]
    fn different_signed_in_account_is_not_created_without_a_post() {
        let (origin, calls, server) = scripted_server(vec![response(
            "200 OK",
            r#"{"id":"other-account","mail":"person@example.invalid"}"#,
        )]);
        let request = valid_request();
        let body = task_body(&request).unwrap();

        assert!(matches!(
            reminder_result(create_after_sign_in(
                &test_client(),
                "synthetic-token",
                &request.account,
                &body,
                &origin
            )),
            ReminderOutcome::NotCreated(ReminderFailure::AccountMismatch)
        ));
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn missing_default_list_is_not_created_without_a_post() {
        let (origin, calls, server) = scripted_server(vec![
            response(
                "200 OK",
                r#"{"id":"scanned-account","mail":"person@example.invalid"}"#,
            ),
            response(
                "200 OK",
                r#"{"value":[{"id":"custom-list","wellknownListName":"none"}]}"#,
            ),
        ]);
        let request = valid_request();
        let body = task_body(&request).unwrap();

        assert!(matches!(
            reminder_result(create_after_sign_in(
                &test_client(),
                "synthetic-token",
                &request.account,
                &body,
                &origin
            )),
            ReminderOutcome::NotCreated(ReminderFailure::DefaultListNotFound)
        ));
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn forbidden_list_read_is_rejected_with_todo_permission_guidance() {
        let (origin, calls, server) = scripted_server(vec![
            response(
                "200 OK",
                r#"{"id":"scanned-account","mail":"person@example.invalid"}"#,
            ),
            response("403 Forbidden", ""),
        ]);
        let request = valid_request();
        let body = task_body(&request).unwrap();
        let outcome = reminder_result(create_after_sign_in(
            &test_client(),
            "synthetic-token",
            &request.account,
            &body,
            &origin,
        ));

        let ReminderOutcome::NotCreated(reason) = outcome else {
            panic!("expected a rejected reminder")
        };
        assert_eq!(
            reason,
            ReminderFailure::Rejected(ConnectionError::AccessDenied)
        );
        assert!(
            reason
                .to_string()
                .starts_with("Microsoft refused the To Do write.")
        );
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn post_server_error_is_uncertain_after_dispatch() {
        let (origin, calls, server) = scripted_server(vec![
            response(
                "200 OK",
                r#"{"id":"scanned-account","mail":"person@example.invalid"}"#,
            ),
            response(
                "200 OK",
                r#"{"value":[{"id":"tasks-list","wellknownListName":"defaultList"}]}"#,
            ),
            response("500 Internal Server Error", ""),
        ]);
        let request = valid_request();
        let body = task_body(&request).unwrap();

        assert!(matches!(
            reminder_result(create_after_sign_in(
                &test_client(),
                "synthetic-token",
                &request.account,
                &body,
                &origin
            )),
            ReminderOutcome::Uncertain
        ));
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }
    #[test]
    fn valid_graph_id_rejects_empty_dot_and_oversized_ids() {
        assert!(valid_graph_id("AAMkAGI1AAA="));
        assert!(!valid_graph_id(""));
        assert!(!valid_graph_id("."));
        assert!(!valid_graph_id(".."));
        assert!(!valid_graph_id(&"a".repeat(2049)));
        assert!(valid_graph_id(&"a".repeat(2048)));
    }
    #[test]
    fn complete_rejects_invalid_ids_before_any_request() {
        let config = ConnectionConfig::new("11111111-1111-1111-1111-111111111111", None).unwrap();
        assert!(matches!(
            complete(&config, "acct", "", "task"),
            ReminderCompletionOutcome::NotCompleted(ConnectionError::InvalidConfiguration)
        ));
        assert!(matches!(
            complete(&config, "acct", "list", ".."),
            ReminderCompletionOutcome::NotCompleted(ConnectionError::InvalidConfiguration)
        ));
    }
    #[test]
    fn check_status_rejects_invalid_ids_before_any_request() {
        let config = ConnectionConfig::new("11111111-1111-1111-1111-111111111111", None).unwrap();
        assert!(matches!(
            check_status(&config, "acct", "", "task"),
            TaskStatusOutcome::Unknown(ConnectionError::InvalidConfiguration)
        ));
        assert!(matches!(
            check_status(&config, "acct", "list", ".."),
            TaskStatusOutcome::Unknown(ConnectionError::InvalidConfiguration)
        ));
    }
}
