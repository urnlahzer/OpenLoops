//! Explicit personal To Do creation. No automatic retry, email sending or shared task writes.
use super::{
    ConnectionConfig, ConnectionError, GRAPH_TIMEOUT_SECONDS, Url, bounded_body, request_error,
    review, with_scopes,
};
use serde_json::{Value, json};

pub struct ReminderRequest {
    pub account: String,
    pub title: String,
    pub at_utc: i64,
    pub marker: String,
}

pub enum ReminderOutcome {
    /// The task list and task id Graph reported, so a later action (e.g.
    /// marking it complete once the review card is Handled) can address the
    /// same task without re-resolving the default list.
    Created {
        list_id: String,
        task_id: String,
    },
    NotCreated(ConnectionError),
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

fn task_body(request: &ReminderRequest) -> Result<Vec<u8>, ConnectionError> {
    let now: chrono::DateTime<chrono::Utc> = std::time::SystemTime::now().into();
    if request.account.is_empty()
        || request.title.trim().len() < 3
        || request.title.len() > 320
        || request.title.chars().any(char::is_control)
        || request.at_utc <= now.timestamp()
        || request.marker.len() != 64
        || !request.marker.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let when = chrono::DateTime::from_timestamp(request.at_utc, 0)
        .ok_or(ConnectionError::InvalidConfiguration)?
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    // dueDateTime and reminderDateTime are independent concepts in Microsoft
    // To Do -- a due date with no reminder, or vice versa, is a normal task
    // shape. This app only ever collects one date/time from the user (the
    // reminder draft's own "when"), so there is nothing else to derive a due
    // date from; using that same instant for both means the task at least
    // shows up as due on the day it was set to alert, rather than carrying
    // no due date at all.
    serde_json::to_vec(&json!({"title":request.title,"body":{"contentType":"text","content":format!("Created after review in OpenLoops.\nOpenLoops reference: {}",request.marker)},"isReminderOn":true,"reminderDateTime":{"dateTime":when,"timeZone":"UTC"},"dueDateTime":{"dateTime":when,"timeZone":"UTC"}})).map_err(|_|ConnectionError::InvalidConfiguration)
}

/// Creates exactly one reviewed task, with an independently authorized session.
/// Caller must durably record the attempt before invoking this function.
#[must_use]
pub fn create(config: &ConnectionConfig, request: &ReminderRequest) -> ReminderOutcome {
    let body = match task_body(request) {
        Ok(b) => b,
        Err(e) => return ReminderOutcome::NotCreated(e),
    };
    let mut dispatched = false;
    let result = with_scopes(config, true, |http, token, _| {
        let (account, _) = review::identity(http, token)?;
        if account != request.account {
            return Err(ConnectionError::InvalidConfiguration);
        }
        // No $select/$top: Microsoft's own documented example for this
        // endpoint (learn.microsoft.com/graph/api/todo-list-lists) is a bare
        // GET with no query parameters, and its example response already
        // includes both `id` and `wellknownListName` on every list without
        // selecting them. A live 400 (HTTP Graph rejected the request as
        // malformed) traced to this call when `$select=id,wellknownListName
        // &$top=100` was present; this endpoint's OData query support is
        // documented only as "some" parameters, not confirmed to include
        // either of these two.
        let url = Url::parse("https://graph.microsoft.com/v1.0/me/todo/lists")
            .map_err(|_| ConnectionError::InvalidConfiguration)?;
        let (lists, partial) = review::pages(http, token, &url, 100)?;
        if partial {
            return Err(ConnectionError::ResponseTooLarge);
        }
        let matches: Vec<_> = lists
            .iter()
            .filter(|v| v["wellknownListName"] == "defaultList")
            .collect();
        if matches.len() != 1 {
            return Err(ConnectionError::ResourceUnavailable);
        }
        let list_id = matches[0]["id"]
            .as_str()
            .filter(|id| valid_graph_id(id))
            .ok_or(ConnectionError::ResourceUnavailable)?;
        let mut url = Url::parse("https://graph.microsoft.com/v1.0/me/todo/lists/")
            .map_err(|_| ConnectionError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?
            .pop_if_empty()
            .push(list_id)
            .push("tasks");
        dispatched = true;
        let response = http
            .post(url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
        if response.status().as_u16() != 201 {
            return Err(ConnectionError::ResourceUnavailable);
        }
        let value: Value = serde_json::from_slice(&bounded_body(response)?)
            .map_err(|_| ConnectionError::ResourceUnavailable)?;
        let task_id = value["id"]
            .as_str()
            .filter(|id| valid_graph_id(id))
            .ok_or(ConnectionError::ResourceUnavailable)?;
        Ok((list_id.to_owned(), task_id.to_owned()))
    });
    match result {
        Ok((list_id, task_id)) => ReminderOutcome::Created { list_id, task_id },
        Err(_) if dispatched => ReminderOutcome::Uncertain,
        Err(e) => ReminderOutcome::NotCreated(e),
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
    #[test]
    fn reminder_payload_is_exact_reviewed_action_not_model_evidence() {
        let request = ReminderRequest {
            account: "synthetic".into(),
            title: "Send the draft".into(),
            at_utc: 4_000_000_000,
            marker: "a".repeat(64),
        };
        let value: Value = serde_json::from_slice(&task_body(&request).unwrap()).unwrap();
        assert_eq!(value["title"], "Send the draft");
        assert_eq!(value["isReminderOn"], true);
        assert_eq!(value["dueDateTime"], value["reminderDateTime"]);
        let mut bad = request;
        bad.at_utc = 1;
        assert!(task_body(&bad).is_err());
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
