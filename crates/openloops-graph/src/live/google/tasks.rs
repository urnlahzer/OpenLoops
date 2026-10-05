//! Explicit Google Tasks reminder writes. Google Tasks keeps only the date
//! part of `due` exactly as sent and drops the time, so `due` carries the
//! user's LOCAL calendar date of the chosen time (as midnight `Z`), never the
//! UTC date. The reviewed local time, with its UTC offset, is retained in
//! `notes` and does not create an alert.

use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use url::Url;

use super::{GoogleConfig, gmail, with_google_session};
use crate::live::reminders::{
    ReminderCompletionOutcome, ReminderFailure, ReminderOutcome, ReminderRequest,
    TaskStatusOutcome, format_local_reminder_time, valid_remote_id, validate_request,
};
use crate::live::review::fetch_from_origin_with_headers;
use crate::live::{
    ConnectionError, GRAPH_TIMEOUT_SECONDS, MailProvider, bounded_body, request_error,
};

pub(super) const TASKS_ORIGIN: &str = "https://tasks.googleapis.com/";

fn task_body(request: &ReminderRequest) -> Result<Vec<u8>, ReminderFailure> {
    validate_request(request)?;
    let due = chrono::DateTime::from_timestamp(request.at_utc, 0)
        .ok_or(ReminderFailure::InvalidDraft)?
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%dT00:00:00.000Z")
        .to_string();
    let local = format_local_reminder_time(request.at_utc).ok_or(ReminderFailure::InvalidDraft)?;
    serde_json::to_vec(&json!({
        "title": request.title,
        "status": "needsAction",
        "due": due,
        "notes": format!(
            "Created after review in OpenLoops.\nReminder time: {local}\nOpenLoops reference: {}",
            request.marker
        )
    }))
    .map_err(|_| ReminderFailure::InvalidDraft)
}

fn url_with_segments(origin: &str, base: &str, segments: &[&str]) -> Result<Url, ConnectionError> {
    let mut url = Url::parse(origin)
        .and_then(|url| url.join(base))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let mut path = url
        .path_segments_mut()
        .map_err(|()| ConnectionError::InvalidConfiguration)?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    drop(path);
    Ok(url)
}

fn write_from_origin(
    http: &Client,
    token: &str,
    method: Method,
    url: Url,
    expected_origin: &str,
    body: &[u8],
) -> Result<(u16, Vec<u8>), ConnectionError> {
    let expected =
        Url::parse(expected_origin).map_err(|_| ConnectionError::InvalidConfiguration)?;
    if url.origin() != expected.origin() {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let response = http
        .request(method, url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(GRAPH_TIMEOUT_SECONDS))
        .body(body.to_vec())
        .send()
        .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
    let status = response.status().as_u16();
    let bytes = bounded_body(response)?;
    Ok((status, bytes))
}

fn reminder_result(result: Result<ReminderOutcome, ConnectionError>) -> ReminderOutcome {
    match result {
        Ok(outcome) => outcome,
        Err(error) => ReminderOutcome::NotCreated(ReminderFailure::Rejected(error)),
    }
}

fn create_from_origins(
    http: &Client,
    token: &str,
    request: &ReminderRequest,
    body: &[u8],
    userinfo_origin: &str,
    tasks_origin: &str,
) -> Result<ReminderOutcome, ConnectionError> {
    let identity = gmail::identity_from_origin(http, token, userinfo_origin)?;
    if identity.account != request.account {
        return Ok(ReminderOutcome::NotCreated(
            ReminderFailure::AccountMismatch,
        ));
    }
    let mut lists_url = Url::parse(tasks_origin)
        .and_then(|url| url.join("tasks/v1/users/@me/lists"))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    lists_url.query_pairs_mut().append_pair("maxResults", "100");
    let bytes = fetch_from_origin_with_headers(http, token, &lists_url, tasks_origin, &[])?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    if !value.is_object() {
        return Err(ConnectionError::ResourceUnavailable);
    }
    // Google omits empty repeated fields: no `items` key is an empty list.
    let items = match value.get("items") {
        None => &[][..],
        Some(items) => items
            .as_array()
            .ok_or(ConnectionError::ResourceUnavailable)?
            .as_slice(),
    };
    if items.len() > 100 {
        return Err(ConnectionError::ResponseTooLarge);
    }
    let Some(list_id) = items
        .first()
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .filter(|id| valid_remote_id(id))
    else {
        return Ok(ReminderOutcome::NotCreated(
            ReminderFailure::DefaultListNotFound,
        ));
    };
    let url = url_with_segments(tasks_origin, "tasks/v1/lists/", &[list_id, "tasks"])?;
    let response = write_from_origin(http, token, Method::POST, url, tasks_origin, body);
    let Ok((status, bytes)) = response else {
        return Ok(ReminderOutcome::Uncertain);
    };
    if status == 401 {
        crate::live::clear_session_for(MailProvider::Google);
        return Ok(ReminderOutcome::Uncertain);
    }
    if status != 200 {
        return Ok(ReminderOutcome::Uncertain);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(ReminderOutcome::Uncertain);
    };
    match value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_remote_id(id))
    {
        Some(task_id) => Ok(ReminderOutcome::Created {
            provider: MailProvider::Google,
            list_id: list_id.to_owned(),
            task_id: task_id.to_owned(),
        }),
        None => Ok(ReminderOutcome::Uncertain),
    }
}

/// Creates one reviewed task in the signed-in account's first task list.
#[must_use]
pub fn create(config: &GoogleConfig, request: &ReminderRequest) -> ReminderOutcome {
    if request.provider != MailProvider::Google {
        return ReminderOutcome::NotCreated(ReminderFailure::InvalidDraft);
    }
    let body = match task_body(request) {
        Ok(body) => body,
        Err(error) => return ReminderOutcome::NotCreated(error),
    };
    reminder_result(with_google_session(config, true, |http, token| {
        create_from_origins(
            http,
            token,
            request,
            &body,
            gmail::USERINFO_ORIGIN,
            TASKS_ORIGIN,
        )
    }))
}

fn complete_from_origins(
    http: &Client,
    token: &str,
    account: &str,
    list_id: &str,
    task_id: &str,
    origins: (&str, &str),
    dispatched: &mut bool,
) -> Result<(), ConnectionError> {
    let (userinfo_origin, tasks_origin) = origins;
    if !valid_remote_id(list_id) || !valid_remote_id(task_id) {
        return Err(ConnectionError::InvalidConfiguration);
    }
    if gmail::identity_from_origin(http, token, userinfo_origin)?.account != account {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let url = url_with_segments(
        tasks_origin,
        "tasks/v1/lists/",
        &[list_id, "tasks", task_id],
    )?;
    let body = serde_json::to_vec(&json!({"status": "completed"}))
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    *dispatched = true;
    let (status, _) = write_from_origin(http, token, Method::PATCH, url, tasks_origin, &body)?;
    // Mapped like the Microsoft `complete`: a 401 is cleared here and never
    // surfaces as `Unauthorized`, so the session runner cannot re-send the PATCH.
    match status {
        200 => Ok(()),
        404 => Err(ConnectionError::NotFound),
        401 => {
            crate::live::clear_session_for(MailProvider::Google);
            Err(ConnectionError::ResourceUnavailable)
        }
        _ => Err(ConnectionError::ResourceUnavailable),
    }
}

/// Marks an existing Google task completed.
#[must_use]
pub fn complete(
    config: &GoogleConfig,
    account: &str,
    list_id: &str,
    task_id: &str,
) -> ReminderCompletionOutcome {
    if !valid_remote_id(list_id) || !valid_remote_id(task_id) {
        return ReminderCompletionOutcome::NotCompleted(ConnectionError::InvalidConfiguration);
    }
    let mut dispatched = false;
    let result = with_google_session(config, true, |http, token| {
        complete_from_origins(
            http,
            token,
            account,
            list_id,
            task_id,
            (gmail::USERINFO_ORIGIN, TASKS_ORIGIN),
            &mut dispatched,
        )
    });
    match result {
        Ok(()) => ReminderCompletionOutcome::Completed,
        Err(_) if dispatched => ReminderCompletionOutcome::Uncertain,
        Err(error) => ReminderCompletionOutcome::NotCompleted(error),
    }
}

fn status_from_origins(
    http: &Client,
    token: &str,
    account: &str,
    list_id: &str,
    task_id: &str,
    userinfo_origin: &str,
    tasks_origin: &str,
) -> Result<bool, ConnectionError> {
    if !valid_remote_id(list_id) || !valid_remote_id(task_id) {
        return Err(ConnectionError::InvalidConfiguration);
    }
    if gmail::identity_from_origin(http, token, userinfo_origin)?.account != account {
        return Err(ConnectionError::InvalidConfiguration);
    }
    let url = url_with_segments(
        tasks_origin,
        "tasks/v1/lists/",
        &[list_id, "tasks", task_id],
    )?;
    let bytes = fetch_from_origin_with_headers(http, token, &url, tasks_origin, &[])?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    Ok(value.get("status").and_then(Value::as_str) == Some("completed"))
}

/// Checks whether an existing Google task is completed.
#[must_use]
pub fn check_status(
    config: &GoogleConfig,
    account: &str,
    list_id: &str,
    task_id: &str,
) -> TaskStatusOutcome {
    if !valid_remote_id(list_id) || !valid_remote_id(task_id) {
        return TaskStatusOutcome::Unknown(ConnectionError::InvalidConfiguration);
    }
    let result = with_google_session(config, true, |http, token| {
        status_from_origins(
            http,
            token,
            account,
            list_id,
            task_id,
            gmail::USERINFO_ORIGIN,
            TASKS_ORIGIN,
        )
    });
    match result {
        Ok(true) => TaskStatusOutcome::Completed,
        Ok(false) => TaskStatusOutcome::NotCompleted,
        Err(error) => TaskStatusOutcome::Unknown(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::test_support::{recording_routed_server, routed_server};
    use std::sync::atomic::Ordering;

    fn response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn request() -> ReminderRequest {
        ReminderRequest {
            provider: MailProvider::Google,
            account: "google:synthetic-sub".into(),
            title: "Review the draft".into(),
            at_utc: 4_000_000_000,
            marker: "a".repeat(64),
        }
    }

    fn identity() -> Vec<u8> {
        response(
            "200 OK",
            r#"{"sub":"synthetic-sub","email":"user@example.invalid"}"#,
        )
    }

    fn client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }

    fn local_due(at_utc: i64) -> String {
        chrono::DateTime::from_timestamp(at_utc, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%dT00:00:00.000Z")
            .to_string()
    }

    #[test]
    fn due_is_the_local_calendar_date_of_the_chosen_time() {
        use chrono::{Offset, TimeZone};
        // A local time near midnight on the side where the UTC date differs
        // from the local date (late evening west of UTC, early morning east).
        let probe = chrono::Local
            .with_ymd_and_hms(2096, 10, 2, 12, 0, 0)
            .single()
            .unwrap();
        let hour = if probe.offset().fix().local_minus_utc() < 0 {
            23
        } else {
            0
        };
        let local = chrono::Local
            .with_ymd_and_hms(2096, 10, 2, hour, 30, 0)
            .single()
            .unwrap();
        let mut request = request();
        request.at_utc = local.timestamp();
        let value: Value = serde_json::from_slice(&task_body(&request).unwrap()).unwrap();
        assert_eq!(value["due"], "2096-10-02T00:00:00.000Z");
    }

    #[test]
    fn body_has_date_only_due_and_local_time_in_notes() {
        let request = request();
        let value: Value = serde_json::from_slice(&task_body(&request).unwrap()).unwrap();
        assert_eq!(value["title"], "Review the draft");
        assert_eq!(value["status"], "needsAction");
        assert_eq!(value["due"], local_due(request.at_utc));
        assert_eq!(
            value["notes"],
            format!(
                "Created after review in OpenLoops.\nReminder time: {}\nOpenLoops reference: {}",
                format_local_reminder_time(request.at_utc).unwrap(),
                "a".repeat(64)
            )
        );
        assert_eq!(value.as_object().unwrap().len(), 4);
    }

    #[test]
    fn account_mismatch_stops_after_userinfo() {
        let (origin, calls, server) = routed_server(vec![(
            "/v1/userinfo",
            response(
                "200 OK",
                r#"{"sub":"different-sub","email":"user@example.invalid"}"#,
            ),
        )]);
        let request = request();
        let body = task_body(&request).unwrap();
        let outcome = create_from_origins(
            &client(),
            "synthetic-token",
            &request,
            &body,
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            outcome,
            ReminderOutcome::NotCreated(ReminderFailure::AccountMismatch)
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_or_incomplete_list_set_has_no_default() {
        for lists in [
            r#"{"items":[]}"#,
            r#"{"items":[],"nextPageToken":"more"}"#,
            r#"{"kind":"tasks#taskLists"}"#,
        ] {
            let (origin, calls, server) = routed_server(vec![
                ("/v1/userinfo", identity()),
                ("/tasks/v1/users/@me/lists", response("200 OK", lists)),
            ]);
            let request = request();
            let outcome = create_from_origins(
                &client(),
                "synthetic-token",
                &request,
                &task_body(&request).unwrap(),
                &origin,
                &origin,
            )
            .unwrap();
            server.join().unwrap();
            assert_eq!(
                outcome,
                ReminderOutcome::NotCreated(ReminderFailure::DefaultListNotFound)
            );
            assert_eq!(calls.load(Ordering::Relaxed), 2);
        }
    }

    #[test]
    fn successful_create_keeps_concrete_list_and_task_ids() {
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/users/@me/lists",
                response("200 OK", r#"{"items":[{"id":"list-id"}]}"#),
            ),
            (
                "/tasks/v1/lists/list-id/tasks",
                response("200 OK", r#"{"id":"task-id"}"#),
            ),
        ]);
        let request = request();
        let outcome = create_from_origins(
            &client(),
            "synthetic-token",
            &request,
            &task_body(&request).unwrap(),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            outcome,
            ReminderOutcome::Created {
                provider: MailProvider::Google,
                list_id: "list-id".into(),
                task_id: "task-id".into(),
            }
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn success_without_task_id_is_uncertain() {
        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/users/@me/lists",
                response("200 OK", r#"{"items":[{"id":"list-id"}]}"#),
            ),
            ("/tasks/v1/lists/list-id/tasks", response("200 OK", "{}")),
        ]);
        let request = request();
        let outcome = create_from_origins(
            &client(),
            "synthetic-token",
            &request,
            &task_body(&request).unwrap(),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(outcome, ReminderOutcome::Uncertain);
    }

    #[test]
    fn post_unauthorized_clears_only_the_google_session() {
        let _serial = crate::live::SESSION_TEST.lock().unwrap();
        crate::live::clear_all_sessions();
        let session = || crate::live::Session {
            access_token: crate::live::Secret::new("synthetic-token".to_owned()),
            expires_at: std::time::Instant::now() + std::time::Duration::from_mins(2),
            scopes: std::collections::BTreeSet::from(["scope".to_owned()]),
            shared: crate::live::SharedScope::NotReported,
        };
        crate::live::session_store()[MailProvider::Microsoft.slot()] = Some(session());
        crate::live::session_store()[MailProvider::Google.slot()] = Some(session());
        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/users/@me/lists",
                response("200 OK", r#"{"items":[{"id":"list-id"}]}"#),
            ),
            (
                "/tasks/v1/lists/list-id/tasks",
                response("401 Unauthorized", ""),
            ),
        ]);
        let request = request();
        let outcome = create_from_origins(
            &client(),
            "synthetic-token",
            &request,
            &task_body(&request).unwrap(),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(outcome, ReminderOutcome::Uncertain);
        assert!(crate::live::has_session_for(MailProvider::Microsoft));
        assert!(!crate::live::has_session_for(MailProvider::Google));
        crate::live::clear_all_sessions();
    }

    fn create_with_post_response(post: Vec<u8>) -> ReminderOutcome {
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/users/@me/lists",
                response("200 OK", r#"{"items":[{"id":"list-id"}]}"#),
            ),
            ("/tasks/v1/lists/list-id/tasks", post),
        ]);
        let request = request();
        let outcome = create_from_origins(
            &client(),
            "synthetic-token",
            &request,
            &task_body(&request).unwrap(),
            &origin,
            &origin,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        outcome
    }

    #[test]
    fn post_non_success_status_is_uncertain() {
        for status in [
            "201 Created",
            "400 Bad Request",
            "500 Internal Server Error",
        ] {
            assert_eq!(
                create_with_post_response(response(status, r#"{"id":"task-id"}"#)),
                ReminderOutcome::Uncertain
            );
        }
    }

    #[test]
    fn post_transport_failure_after_dispatch_is_uncertain() {
        // The server accepts the POST and closes without any response bytes.
        assert_eq!(
            create_with_post_response(Vec::new()),
            ReminderOutcome::Uncertain
        );
    }

    #[test]
    fn patch_sends_exact_completed_body_to_the_concrete_task_path() {
        let (origin, requests, server) = recording_routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/lists/list-id/tasks/task-id",
                response("200 OK", r#"{"status":"completed"}"#),
            ),
        ]);
        let mut dispatched = false;
        complete_from_origins(
            &client(),
            "synthetic-token",
            "google:synthetic-sub",
            "list-id",
            "task-id",
            (&origin, &origin),
            &mut dispatched,
        )
        .unwrap();
        server.join().unwrap();
        assert!(dispatched);
        let requests = requests.lock().unwrap();
        let patch = &requests[1];
        assert!(patch.starts_with("PATCH /tasks/v1/lists/list-id/tasks/task-id HTTP/1.1\r\n"));
        let body = &patch[patch.find("\r\n\r\n").unwrap() + 4..];
        assert_eq!(body, r#"{"status":"completed"}"#);
    }

    #[test]
    fn patch_status_mapping_matches_microsoft_complete() {
        for (status, expected) in [
            ("404 Not Found", ConnectionError::NotFound),
            ("401 Unauthorized", ConnectionError::ResourceUnavailable),
            ("403 Forbidden", ConnectionError::ResourceUnavailable),
            (
                "500 Internal Server Error",
                ConnectionError::ResourceUnavailable,
            ),
        ] {
            let (origin, _, server) = routed_server(vec![
                ("/v1/userinfo", identity()),
                (
                    "/tasks/v1/lists/list-id/tasks/task-id",
                    response(status, ""),
                ),
            ]);
            let mut dispatched = false;
            let result = complete_from_origins(
                &client(),
                "synthetic-token",
                "google:synthetic-sub",
                "list-id",
                "task-id",
                (&origin, &origin),
                &mut dispatched,
            );
            server.join().unwrap();
            assert_eq!(result, Err(expected));
            assert!(dispatched);
        }
    }

    #[test]
    fn patch_unauthorized_clears_google_session_and_never_patches_again() {
        let _serial = crate::live::SESSION_TEST.lock().unwrap();
        crate::live::clear_all_sessions();
        let scopes = std::collections::BTreeSet::from(["scope".to_owned()]);
        let session = || crate::live::Session {
            access_token: crate::live::Secret::new("synthetic-token".to_owned()),
            expires_at: std::time::Instant::now() + std::time::Duration::from_mins(2),
            scopes: scopes.clone(),
            shared: crate::live::SharedScope::NotReported,
        };
        crate::live::session_store()[MailProvider::Microsoft.slot()] = Some(session());
        crate::live::session_store()[MailProvider::Google.slot()] = Some(session());
        let (origin, requests, server) = recording_routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/lists/list-id/tasks/task-id",
                response("401 Unauthorized", ""),
            ),
        ]);
        let http = client();
        let mut authorizations = 0;
        let mut dispatched = false;
        let result = crate::live::run_with_session_for(
            MailProvider::Google,
            scopes.clone(),
            |_| {
                authorizations += 1;
                Ok(session())
            },
            |token, _| {
                complete_from_origins(
                    &http,
                    token,
                    "google:synthetic-sub",
                    "list-id",
                    "task-id",
                    (&origin, &origin),
                    &mut dispatched,
                )
            },
        );
        server.join().unwrap();
        assert!(result.is_err());
        assert_eq!(authorizations, 0);
        let patches = requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("PATCH "))
            .count();
        assert_eq!(patches, 1);
        assert!(crate::live::has_session_for(MailProvider::Microsoft));
        assert!(!crate::live::has_session_for(MailProvider::Google));
        crate::live::clear_all_sessions();
    }

    #[test]
    fn patch_and_get_use_the_concrete_task_path() {
        let (origin, calls, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/lists/list-id/tasks/task-id",
                response("200 OK", r#"{"status":"completed"}"#),
            ),
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/lists/list-id/tasks/task-id",
                response("200 OK", r#"{"status":"completed"}"#),
            ),
        ]);
        let http = client();
        let mut dispatched = false;
        complete_from_origins(
            &http,
            "synthetic-token",
            "google:synthetic-sub",
            "list-id",
            "task-id",
            (&origin, &origin),
            &mut dispatched,
        )
        .unwrap();
        assert!(dispatched);
        assert!(
            status_from_origins(
                &http,
                "synthetic-token",
                "google:synthetic-sub",
                "list-id",
                "task-id",
                &origin,
                &origin,
            )
            .unwrap()
        );
        server.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn status_not_found_is_preserved() {
        let (origin, _, server) = routed_server(vec![
            ("/v1/userinfo", identity()),
            (
                "/tasks/v1/lists/list-id/tasks/missing-id",
                response("404 Not Found", ""),
            ),
        ]);
        let result = status_from_origins(
            &client(),
            "synthetic-token",
            "google:synthetic-sub",
            "list-id",
            "missing-id",
            &origin,
            &origin,
        );
        server.join().unwrap();
        assert_eq!(result, Err(ConnectionError::NotFound));
    }
}
