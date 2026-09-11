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

#[derive(Clone, Copy)]
pub enum ReminderOutcome {
    Created,
    NotCreated(ConnectionError),
    Uncertain,
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
    serde_json::to_vec(&json!({"title":request.title,"body":{"contentType":"text","content":format!("Created after review in OpenLoops.\nOpenLoops reference: {}",request.marker)},"isReminderOn":true,"reminderDateTime":{"dateTime":when,"timeZone":"UTC"}})).map_err(|_|ConnectionError::InvalidConfiguration)
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
        let url = Url::parse(
            "https://graph.microsoft.com/v1.0/me/todo/lists?$select=id,wellknownListName&$top=100",
        )
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
        let id = matches[0]["id"]
            .as_str()
            .filter(|id| !id.is_empty() && *id != "." && *id != ".." && id.len() <= 2048)
            .ok_or(ConnectionError::ResourceUnavailable)?;
        let mut url = Url::parse("https://graph.microsoft.com/v1.0/me/todo/lists/")
            .map_err(|_| ConnectionError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| ConnectionError::InvalidConfiguration)?
            .pop_if_empty()
            .push(id)
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
        if value["id"].as_str().is_none_or(str::is_empty) {
            return Err(ConnectionError::ResourceUnavailable);
        }
        Ok(())
    });
    match result {
        Ok(()) => ReminderOutcome::Created,
        Err(_) if dispatched => ReminderOutcome::Uncertain,
        Err(e) => ReminderOutcome::NotCreated(e),
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
        assert!(value.get("dueDateTime").is_none());
        let mut bad = request;
        bad.at_utc = 1;
        assert!(task_body(&bad).is_err());
    }
}
