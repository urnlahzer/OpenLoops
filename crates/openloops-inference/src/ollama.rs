//! Opt-in Ollama Cloud adapter. Credentials and payloads are session-only.
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::provider::{
    MAX_PARALLEL_REQUESTS, MAX_REQUEST, ModelClient, REQUEST_DEADLINE, RequestControl,
    https_client, json_document, parse_error, read_body, send_with_control, status_error,
    valid_key, valid_model_name,
};

pub use crate::provider::ProviderError;

/// The governed `analysis-output-v1` path lives in [`crate::analysis`]
/// because it is provider-neutral; `OllamaCloud::analyze` and
/// `OllamaCloud::analyze_for_review` are thin delegates defined there.
pub use crate::analysis;

pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const TAGS: &str = "https://ollama.com/api/tags";
const CHAT: &str = "https://ollama.com/api/chat";

/// No Debug implementation: the key must never appear in diagnostics.
pub struct OllamaCloud {
    client: Client,
    key: Zeroizing<String>,
    model: String,
    /// Concurrent request slots the account's plan allots; see
    /// [`OllamaCloud::with_max_parallel`].
    parallel: usize,
}

impl OllamaCloud {
    /// Validates the key and confirms the exact model is available without sending mail.
    /// # Errors
    /// Returns a fixed provider error. No alternate model or origin is attempted.
    pub fn connect(key: String, model: &str) -> Result<Self, ProviderError> {
        let key = Zeroizing::new(key);
        if !valid_key(&key) {
            return Err(ProviderError::InvalidKey);
        }
        let client = https_client()?;
        if !valid_model_name(model) {
            return Err(ProviderError::ModelUnavailable);
        }
        let provider = Self {
            client,
            key,
            model: model.to_owned(),
            parallel: 1,
        };
        let started = Instant::now();
        let control = RequestControl::new(started);
        let response = send_with_control(
            provider.client.get(TAGS).bearer_auth(provider.key.as_str()),
            &control,
        )?;
        if !model_names(&read_response(response, &control)?)?
            .iter()
            .any(|name| name == model)
        {
            return Err(ProviderError::ModelUnavailable);
        }
        Ok(provider)
    }

    /// Records how many requests this client may keep in flight, which on
    /// Ollama Cloud is fixed by the account's plan (Free 1, Pro 3,
    /// Max/Team 10 concurrent requests). Requests past the plan's slots
    /// are queued server-side and rejected once that queue fills, so the
    /// caller must not exceed the number it sets here. Defaults to 1 --
    /// the Free plan, and the only value safe to assume without asking.
    /// Clamped to 1..=[`MAX_PARALLEL_REQUESTS`].
    #[must_use]
    pub fn with_max_parallel(mut self, parallel: usize) -> Self {
        self.parallel = parallel.clamp(1, MAX_PARALLEL_REQUESTS);
        self
    }

    /// Exercises generation with a fixed content-free request. No mailbox is accessed.
    /// # Errors
    /// Returns a fixed error when generation or strict JSON validation fails.
    pub fn check_generation(&self) -> Result<(), ProviderError> {
        let body = request(
            &self.model,
            "Return only this JSON object: {\"schema_version\":1,\"claims\":[]}",
            "Connection check; there is no message to analyze.",
        )?;
        let content = self.chat(body, None, REQUEST_DEADLINE)?;
        let parsed = openloops_contracts::parse_analysis_output(json_document(content.as_bytes())?)
            .map_err(parse_error)?;
        if !parsed.claims.is_empty() {
            return Err(ProviderError::InvalidAnalysis);
        }
        Ok(())
    }

    /// Sends `body` and reads the answer. Records the start instant
    /// immediately before `send()` so the supplied `deadline` bounds the
    /// whole request -- including the header wait, not just the body --
    /// rather than just an idle connection; `cancel`, when
    /// supplied, lets a Stop action abort the request in progress, whether
    /// it is still waiting on a response or partway through reading one.
    fn chat(
        &self,
        body: Vec<u8>,
        cancel: Option<&AtomicBool>,
        deadline: Duration,
    ) -> Result<Zeroizing<String>, ProviderError> {
        let started = Instant::now();
        let control = RequestControl::with_cancel_and_deadline(started, cancel, deadline);
        let request = self
            .client
            .post(CHAT)
            .bearer_auth(self.key.as_str())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        let response = send_with_control(request, &control)?;
        parse_chat(&read_response(response, &control)?, &self.model)
    }
}

impl ModelClient for OllamaCloud {
    fn model(&self) -> &str {
        &self.model
    }

    fn max_parallel(&self) -> usize {
        self.parallel
    }

    fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: Option<&AtomicBool>,
        deadline: Duration,
    ) -> Result<Zeroizing<String>, ProviderError> {
        self.chat(request(&self.model, system, user)?, cancel, deadline)
    }
}

fn read_response(
    response: Response,
    control: &RequestControl<'_>,
) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    match response.status().as_u16() {
        200 => {}
        status => return Err(status_error(status)),
    }
    read_body(response, crate::provider::MAX_RESPONSE, control)
}

/// Prefer the exact default label, or the dated version the provider lists.
#[must_use]
pub fn suggested_model(models: &[String]) -> Option<usize> {
    models
        .iter()
        .position(|model| model == DEFAULT_MODEL)
        .or_else(|| {
            models
                .iter()
                .position(|model| model.starts_with("deepseek-v4-flash:"))
        })
}

fn model_names(bytes: &[u8]) -> Result<Vec<String>, ProviderError> {
    let value = openloops_contracts::parse_strict_json(bytes)
        .map_err(|_| ProviderError::InvalidResponse)?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut names = Vec::new();
    for model in models {
        let name = model
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| model.get("model").and_then(Value::as_str))
            .filter(|name| valid_model_name(name))
            .ok_or(ProviderError::InvalidResponse)?;
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Fetches selectable cloud model names without sending mailbox content.
/// # Errors
/// Returns fixed errors for authentication, network, or malformed model listing.
pub fn available_models(key: &str) -> Result<Vec<String>, ProviderError> {
    if !valid_key(key) {
        return Err(ProviderError::InvalidKey);
    }
    let started = Instant::now();
    let control = RequestControl::new(started);
    let response = send_with_control(https_client()?.get(TAGS).bearer_auth(key), &control)?;
    model_names(&read_response(response, &control)?)
}

fn parse_chat(bytes: &[u8], selected: &str) -> Result<Zeroizing<String>, ProviderError> {
    let value = openloops_contracts::parse_strict_json(bytes)
        .map_err(|_| ProviderError::InvalidResponse)?;
    let message = value.get("message").ok_or(ProviderError::InvalidResponse)?;
    if value.get("done").and_then(Value::as_bool) != Some(true)
        || value.get("model").and_then(Value::as_str) != Some(selected)
        || value
            .get("done_reason")
            .is_some_and(|reason| reason.as_str() != Some("stop"))
        || value.get("error").is_some()
        || message.get("role").and_then(Value::as_str) != Some("assistant")
        || message
            .get("tool_calls")
            .is_some_and(|calls| calls.as_array().is_none_or(|calls| !calls.is_empty()))
        || message
            .get("images")
            .is_some_and(|images| images.as_array().is_none_or(|images| !images.is_empty()))
        || message.get("audio").is_some()
    {
        return Err(ProviderError::InvalidResponse);
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .ok_or(ProviderError::InvalidResponse)?;
    Ok(Zeroizing::new(content.to_owned()))
}

fn request(model: &str, system: &str, user: &str) -> Result<Vec<u8>, ProviderError> {
    let body = serde_json::to_vec(&json!({"model": model,"messages":[{"role":"system","content":system},{"role":"user","content":user}],"stream":false}))
        .map_err(|_| ProviderError::InvalidResponse)?;
    if body.len() > MAX_REQUEST {
        return Err(ProviderError::InputTooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_provider() -> OllamaCloud {
        OllamaCloud {
            client: Client::builder().no_proxy().build().unwrap(),
            key: Zeroizing::new("synthetic-key".into()),
            model: DEFAULT_MODEL.to_owned(),
            parallel: 1,
        }
    }

    #[test]
    fn the_plan_slot_count_is_clamped_and_defaults_to_the_free_plan() {
        // Free is the only plan safe to assume: one concurrent request.
        assert_eq!(synthetic_provider().max_parallel(), 1);
        for (slots, expected) in [(0, 1), (1, 1), (3, 3), (10, 10)] {
            assert_eq!(
                synthetic_provider().with_max_parallel(slots).max_parallel(),
                expected
            );
        }
        assert_eq!(
            synthetic_provider()
                .with_max_parallel(usize::MAX)
                .max_parallel(),
            MAX_PARALLEL_REQUESTS
        );
    }

    #[test]
    fn markdown_wrapping_does_not_relax_json_or_schema_validation() {
        let value = br#"{"schema_version":1,"claims":[]}"#;
        for prefix in ["```json\n", "```json\r\n", "```\n"] {
            let fenced = format!(" {prefix}{}\n``` ", std::str::from_utf8(value).unwrap());
            let document = json_document(fenced.as_bytes()).unwrap();
            assert_eq!(document, value);
            assert!(openloops_contracts::parse_analysis_output(document).is_ok());
        }
        for invalid in [
            "Here is JSON: {\"schema_version\":1,\"claims\":[]}",
            "```json\n{\"schema_version\":1,\"claims\":[]}\n``` trailing instructions",
            "```json\n{\"schema_version\":1,\"claims\":[],\"claims\":[]}\n```",
            "```json\n{\"schema_version\":1,\"claims\":[],\"other\":true}\n```",
        ] {
            assert!(
                json_document(invalid.as_bytes())
                    .and_then(|bytes| openloops_contracts::parse_analysis_output(bytes)
                        .map_err(parse_error))
                    .is_err()
            );
        }
    }

    #[test]
    fn failures_report_status_without_server_payloads() {
        for (status, expected) in [
            (401, ProviderError::Unauthorized),
            (403, ProviderError::Unauthorized),
            (402, ProviderError::Quota),
            (429, ProviderError::RateLimited),
            (400, ProviderError::RequestRejected(400)),
            (404, ProviderError::RequestRejected(404)),
            (502, ProviderError::ServerError(502)),
            (503, ProviderError::ServerError(503)),
        ] {
            assert_eq!(status_error(status), expected);
        }
        assert!(
            ProviderError::ServerError(502)
                .to_string()
                .contains("HTTP 502")
        );
        assert!(
            ProviderError::Timeout
                .to_string()
                .contains("did not answer in time")
        );
    }

    #[test]
    fn dated_deepseek_is_suggested_and_exact_label_is_preserved() {
        let models = vec!["another-model".into(), "deepseek-v4-flash:0731".into()];
        assert_eq!(suggested_model(&models), Some(1));
        assert_eq!(models[1], "deepseek-v4-flash:0731");
        assert_eq!(suggested_model(&["another-model".into()]), None);
        assert_eq!(suggested_model(&[]), None);
    }

    #[test]
    fn invalid_provider_json_never_exposes_payload_in_errors() {
        let invalid = b"synthetic-private-canary";
        assert_eq!(
            parse_chat(invalid, DEFAULT_MODEL).err(),
            Some(ProviderError::InvalidResponse)
        );
        assert_eq!(
            model_names(invalid).err(),
            Some(ProviderError::InvalidResponse)
        );
        assert!(
            !ProviderError::InvalidResponse
                .to_string()
                .contains("canary")
        );
    }
    #[test]
    fn duplicate_envelope_fields_cannot_override_tool_calls_or_completion() {
        let bytes = br#"{"model":"deepseek-v4-flash","done":false,"done":true,"message":{"role":"assistant","content":"{}","tool_calls":[{}],"tool_calls":[]}}"#;
        assert!(parse_chat(bytes, DEFAULT_MODEL).is_err());
        assert!(model_names(br#"{"models":[],"models":[{"name":"deepseek-v4-flash"}]}"#).is_err());
    }

    #[test]
    fn malformed_completion_reason_and_multimodal_output_are_rejected() {
        for extra in [
            json!({"done_reason":42}),
            json!({"done_reason":null}),
            json!({"error":"failure"}),
        ] {
            let mut value = json!({"model":DEFAULT_MODEL,"done":true,"message":{"role":"assistant","content":"{}"}});
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(parse_chat(&serde_json::to_vec(&value).unwrap(), DEFAULT_MODEL).is_err());
        }
        let value = json!({"model":DEFAULT_MODEL,"done":true,"message":{"role":"assistant","content":"{}","images":["unexpected"]}});
        assert!(parse_chat(&serde_json::to_vec(&value).unwrap(), DEFAULT_MODEL).is_err());
    }
    #[test]
    fn model_listing_supports_switching_and_rejects_terminal_injection() {
        assert_eq!(
            model_names(br#"{"models":[{"name":"deepseek-v4-flash"},{"name":"other:cloud"}]}"#)
                .unwrap(),
            ["deepseek-v4-flash", "other:cloud"]
        );
        assert!(model_names(br#"{"models":[{"name":"bad\u001b[2J"}]}"#).is_err());
    }
    #[test]
    fn incomplete_tool_and_wrong_model_outputs_are_rejected() {
        let valid = json!({"model":DEFAULT_MODEL,"done":true,"done_reason":"stop","message":{"role":"assistant","content":"{}"}});
        assert!(parse_chat(&serde_json::to_vec(&valid).unwrap(), DEFAULT_MODEL).is_ok());
        for (field, replacement) in [
            ("done", json!(false)),
            ("model", json!("other")),
            ("done_reason", json!("length")),
        ] {
            let mut bad = valid.clone();
            bad[field] = replacement;
            assert!(parse_chat(&serde_json::to_vec(&bad).unwrap(), DEFAULT_MODEL).is_err());
        }
        let mut bad = valid;
        bad["message"]["tool_calls"] = json!([{"function":{"name":"send_mail"}}]);
        assert!(parse_chat(&serde_json::to_vec(&bad).unwrap(), DEFAULT_MODEL).is_err());
    }
    #[test]
    fn request_has_no_tools_and_bounds_input() {
        let value: Value =
            serde_json::from_slice(&request(DEFAULT_MODEL, "policy", "data").unwrap()).unwrap();
        assert_eq!(value["stream"], false);
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert!(request(DEFAULT_MODEL, "policy", &"x".repeat(MAX_REQUEST)).is_err());
    }

    #[test]
    fn switching_models_changes_request_and_response_binding() {
        for model in [DEFAULT_MODEL, "another-model:version"] {
            let request: Value =
                serde_json::from_slice(&request(model, "policy", "data").unwrap()).unwrap();
            assert_eq!(request["model"], model);
            let response = serde_json::to_vec(
                &json!({"model":model,"done":true,"message":{"role":"assistant","content":"{}"}}),
            )
            .unwrap();
            assert!(parse_chat(&response, model).is_ok());
            assert!(parse_chat(&response, "unselected-model").is_err());
        }
    }
}
