//! Opt-in `OpenRouter` adapter restricted to zero-data-retention endpoints.
//! Credentials and payloads are session-only, the model menu is built from
//! the public ZDR endpoint listing, and every completion asks `OpenRouter`
//! to route to ZDR endpoints only.
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::provider::{
    MAX_PARALLEL_REQUESTS, MAX_REQUEST, MAX_RESPONSE, ModelClient, ProviderError, RequestControl,
    https_client, json_document, parse_error, read_body, send_with_control, status_error, valid_key,
    valid_model_name,
};

const AUTHORITY: &str = "https://openrouter.ai";
const CHAT: &str = "https://openrouter.ai/api/v1/chat/completions";
const ZDR_ENDPOINTS: &str = "https://openrouter.ai/api/v1/endpoints/zdr";
/// The account's own key status. Content-free: it names no model and
/// carries no mailbox projection, and its answer is read only for the
/// published request budget below.
const KEY_STATUS: &str = "https://openrouter.ai/api/v1/key";
/// The catalog listing is content-free and much larger than a completion:
/// every ZDR endpoint of every model, several hundred kilobytes today.
const MAX_LISTING: usize = 4_194_304;
const MAX_ENDPOINTS: usize = 8192;
const MAX_LABEL: usize = 128;
/// The key status answer is a single small object; anything larger is not
/// the document this adapter knows how to read.
const MAX_KEY_STATUS: usize = 16_384;
/// Bounds on a published request budget. A budget outside these is not
/// usable pacing information and is treated as absent.
const MAX_BUDGET_REQUESTS: u64 = 100_000;
const MAX_BUDGET_SECONDS: u64 = 3600;

/// One selectable zero-data-retention model: the exact provider-side
/// `id` a request is bound to, and the human `label` shown beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

/// No Debug implementation: the key must never appear in diagnostics.
pub struct OpenRouter {
    client: Client,
    key: Zeroizing<String>,
    model: String,
    /// Caller-chosen dispatch ceiling; see
    /// [`OpenRouter::with_max_parallel`].
    parallel: usize,
    /// The account's published per-interval request budget, read once at
    /// connect time. `None` when `OpenRouter` published none or the answer
    /// could not be read.
    budget: Option<(u32, Duration)>,
}

impl OpenRouter {
    /// Validates the key and confirms the exact model still has a
    /// zero-data-retention endpoint, without sending mail.
    /// # Errors
    /// Returns a fixed provider error. No alternate model or origin is attempted.
    pub fn connect(key: String, model: &str) -> Result<Self, ProviderError> {
        let key = Zeroizing::new(key);
        if !valid_key(&key) {
            return Err(ProviderError::InvalidKey);
        }
        if !valid_model_name(model) {
            return Err(ProviderError::ModelUnavailable);
        }
        let client = https_client()?;
        if !fetch_zdr(&client, ZDR_ENDPOINTS)?
            .iter()
            .any(|choice| choice.id == model)
        {
            return Err(ProviderError::ModelUnavailable);
        }
        let budget = key_budget(&client, &key, KEY_STATUS);
        Ok(Self {
            client,
            key,
            model: model.to_owned(),
            parallel: 1,
            budget,
        })
    }

    /// Records how many requests this client may keep in flight.
    /// `OpenRouter` publishes no concurrency cap for a paid key, so this
    /// is the caller's own ceiling rather than a provider-imposed one;
    /// upstream providers may still answer 429 under load, which the
    /// caller backs off from rather than resending. Defaults to 1.
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
        let content = self.complete(
            "Return only this JSON object: {\"schema_version\":1,\"claims\":[]}",
            "Connection check; there is no message to analyze.",
            None,
        )?;
        let parsed = openloops_contracts::parse_analysis_output(json_document(content.as_bytes())?)
            .map_err(parse_error)?;
        if !parsed.claims.is_empty() {
            return Err(ProviderError::InvalidAnalysis);
        }
        Ok(())
    }

    /// Sends `body` and reads the answer. Records the start instant
    /// immediately before `send()` so [`crate::provider::REQUEST_DEADLINE`]
    /// bounds the whole request -- including the header wait, not just the
    /// body -- rather than only an idle connection; `cancel`, when
    /// supplied, lets a Stop action abort the request in progress, whether
    /// it is still waiting on a response or partway through reading one.
    fn chat_at(
        &self,
        url: &str,
        body: Vec<u8>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Zeroizing<String>, ProviderError> {
        // Only the adapter-fixed CHAT constant reaches here in a real build;
        // the loopback tests substitute their own origin.
        debug_assert!(cfg!(test) || url.starts_with(AUTHORITY));
        let started = Instant::now();
        let control = RequestControl::with_cancel(started, cancel);
        let request = self
            .client
            .post(url)
            .bearer_auth(self.key.as_str())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        let response = send_with_control(request, &control)?;
        parse_chat(&read_response(response, &control)?, &self.model)
    }
}

impl ModelClient for OpenRouter {
    fn model(&self) -> &str {
        &self.model
    }

    fn max_parallel(&self) -> usize {
        self.parallel
    }

    fn request_budget(&self) -> Option<(u32, Duration)> {
        self.budget
    }

    fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<Zeroizing<String>, ProviderError> {
        self.chat_at(CHAT, request(&self.model, system, user)?, cancel)
    }
}

/// Fetches the selectable zero-data-retention models. The listing is public
/// and content-free: no key is sent and no mailbox content is transmitted.
/// # Errors
/// Returns fixed errors for network failure or a malformed listing.
pub fn available_zdr_models() -> Result<Vec<ModelChoice>, ProviderError> {
    fetch_zdr(&https_client()?, ZDR_ENDPOINTS)
}

fn fetch_zdr(client: &Client, url: &str) -> Result<Vec<ModelChoice>, ProviderError> {
    // Only the adapter-fixed ZDR_ENDPOINTS constant reaches here in a real
    // build; the loopback tests substitute their own origin.
    debug_assert!(cfg!(test) || url.starts_with(AUTHORITY));
    let started = Instant::now();
    let control = RequestControl::new(started);
    let response = send_with_control(
        client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json"),
        &control,
    )?;
    match response.status().as_u16() {
        200 => {}
        status => return Err(status_error(status)),
    }
    zdr_models(&read_body(response, MAX_LISTING, &control)?)
}

/// Reads the account's published per-interval request budget from the
/// content-free key status endpoint. Advisory only: any failure --
/// network, a non-200 status, an unreadable or absent budget -- answers
/// `None`, because a connection that can list ZDR endpoints and complete
/// a chat must not be refused over a pacing hint. The key is sent as a
/// Bearer credential to the same exact authority every other request
/// uses, and the answer is parsed, bounded, and dropped, never persisted.
fn key_budget(client: &Client, key: &str, url: &str) -> Option<(u32, Duration)> {
    // Only the adapter-fixed KEY_STATUS constant reaches here in a real
    // build; the loopback tests substitute their own origin.
    debug_assert!(cfg!(test) || url.starts_with(AUTHORITY));
    let control = RequestControl::new(Instant::now());
    let response = send_with_control(
        client
            .get(url)
            .bearer_auth(key)
            .header(reqwest::header::ACCEPT, "application/json"),
        &control,
    )
    .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    rate_limit(&read_body(response, MAX_KEY_STATUS, &control).ok()?)
}

/// Parses `{"data":{"rate_limit":{"requests":N,"interval":"10s"}}}`.
///
/// Every member is optional and every one is revalidated: a document
/// without a readable, in-range budget answers `None` rather than
/// producing a guess. Parsed strictly (duplicate members reject) because
/// unlike the ZDR listing this is a single small document whose one value
/// is used directly, not a menu the user re-picks from.
fn rate_limit(bytes: &[u8]) -> Option<(u32, Duration)> {
    if bytes.len() > MAX_KEY_STATUS {
        return None;
    }
    let value = openloops_contracts::parse_strict_json(bytes).ok()?;
    let limit = value.get("data")?.get("rate_limit")?.as_object()?;
    let requests = limit.get("requests")?.as_u64()?;
    if requests == 0 || requests > MAX_BUDGET_REQUESTS {
        return None;
    }
    let interval = parse_interval(limit.get("interval")?.as_str()?)?;
    Some((u32::try_from(requests).ok()?, interval))
}

/// A published interval such as `"10s"`: decimal digits followed by a
/// single seconds, minutes, or hours unit, bounded by
/// [`MAX_BUDGET_SECONDS`].
fn parse_interval(text: &str) -> Option<Duration> {
    let (digits, unit) = text.split_at_checked(text.len().checked_sub(1)?)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let count: u64 = digits.parse().ok()?;
    let seconds = count.checked_mul(match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    })?;
    (1..=MAX_BUDGET_SECONDS)
        .contains(&seconds)
        .then(|| Duration::from_secs(seconds))
}

/// A displayable model label: present, bounded, and free of the control
/// and escape bytes a terminal or list widget would interpret.
fn valid_label(label: &str) -> bool {
    !label.is_empty() && label.chars().count() <= MAX_LABEL && !label.chars().any(char::is_control)
}

/// Parses the ZDR endpoint listing into one selectable entry per model.
///
/// The listing names every ZDR endpoint of every model, so many rows share
/// a `model_id`; the first healthy row wins and the rest are dropped.
/// Unknown members are ignored, every member read is revalidated, and the
/// raw response is never persisted. A duplicate JSON member would collapse
/// to its last value here rather than being rejected, because the listing
/// is a menu the user chooses from and never an authority: the chosen id is
/// revalidated as a label and every completion carries `provider.zdr` so
/// `OpenRouter` enforces the routing regardless of what the listing said.
///
/// A listing that is not an object with a `data` array, or that exceeds a
/// bound, fails. Individual rows do not: a row this adapter cannot read is
/// dropped, because hundreds of independent endpoints share one response and
/// one unreadable row must not make every model unselectable.
fn zdr_models(bytes: &[u8]) -> Result<Vec<ModelChoice>, ProviderError> {
    if bytes.len() > MAX_LISTING {
        return Err(ProviderError::ResponseTooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    let value: Value = serde_json::from_str(text).map_err(|_| ProviderError::InvalidResponse)?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    if data.len() > MAX_ENDPOINTS {
        return Err(ProviderError::ResponseTooLarge);
    }
    let mut choices: Vec<ModelChoice> = Vec::new();
    for endpoint in data {
        let Some(choice) = healthy_choice(endpoint) else {
            continue;
        };
        if !choices.iter().any(|existing| existing.id == choice.id) {
            choices.push(choice);
        }
    }
    choices.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    Ok(choices)
}

/// One selectable model, or `None` when the row is not a readable healthy
/// endpoint. An absent or null `status` is the healthy case; a present
/// `status` must be an integer, and only zero is healthy.
fn healthy_choice(endpoint: &Value) -> Option<ModelChoice> {
    let endpoint = endpoint.as_object()?;
    let status = match endpoint.get("status") {
        None | Some(Value::Null) => 0,
        Some(status) => status.as_i64()?,
    };
    if status != 0 {
        return None;
    }
    let id = endpoint
        .get("model_id")
        .and_then(Value::as_str)
        .filter(|id| valid_model_name(id))?;
    let label = endpoint
        .get("model_name")
        .and_then(Value::as_str)
        .filter(|label| valid_label(label))?;
    Some(ModelChoice {
        id: id.to_owned(),
        label: label.to_owned(),
    })
}

fn read_response(
    response: Response,
    control: &RequestControl<'_>,
) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    match response.status().as_u16() {
        200 => {}
        401 => return Err(ProviderError::InvalidKey),
        status => return Err(status_error(status)),
    }
    read_body(response, MAX_RESPONSE, control)
}

/// Builds one non-streaming completion pinned to ZDR routing.
///
/// No `response_format` or `structured_outputs` member is sent: strict
/// application validation of the answer stays authoritative, exactly as on
/// the Ollama Cloud path. No tools member is sent either.
fn request(model: &str, system: &str, user: &str) -> Result<Vec<u8>, ProviderError> {
    let body = serde_json::to_vec(&json!({
        "model": model,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
        "stream": false,
        "provider": {"zdr": true}
    }))
    .map_err(|_| ProviderError::InvalidResponse)?;
    if body.len() > MAX_REQUEST {
        return Err(ProviderError::InputTooLarge);
    }
    Ok(body)
}

/// Accepts the exact selected label, or that label followed by a provider
/// variant suffix such as `:free`. A different model whose id merely starts
/// with the same characters, such as `vendor/model-10` for `vendor/model-1`,
/// never matches.
fn model_matches(reported: &str, selected: &str) -> bool {
    reported
        .strip_prefix(selected)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(':'))
}

/// Accepts exactly one completed assistant choice from the selected model.
fn parse_chat(bytes: &[u8], selected: &str) -> Result<Zeroizing<String>, ProviderError> {
    let value = openloops_contracts::parse_strict_json(bytes)
        .map_err(|_| ProviderError::InvalidResponse)?;
    if value.get("error").is_some()
        || !value
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|reported| model_matches(reported, selected))
    {
        return Err(ProviderError::InvalidResponse);
    }
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let [choice] = choices.as_slice() else {
        return Err(ProviderError::InvalidResponse);
    };
    let message = choice
        .get("message")
        .ok_or(ProviderError::InvalidResponse)?;
    if choice
        .get("finish_reason")
        .is_some_and(|reason| reason.as_str() != Some("stop"))
        || message.get("role").and_then(Value::as_str) != Some("assistant")
        || message
            .get("tool_calls")
            .is_some_and(|calls| calls.as_array().is_none_or(|calls| !calls.is_empty()))
        || message
            .get("images")
            .is_some_and(|images| images.as_array().is_none_or(|images| !images.is_empty()))
        || message
            .get("refusal")
            .is_some_and(|refusal| !refusal.is_null())
        || message.get("audio").is_some_and(|audio| !audio.is_null())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::Receiver;

    const MODEL: &str = "vendor/model-1";

    fn listing(entries: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "data": entries })).unwrap()
    }

    fn endpoint(id: &str, label: &str, status: i64) -> Value {
        json!({
            "name": format!("Provider | {id}"),
            "model_id": id,
            "model_name": label,
            "context_length": 8192,
            "pricing": {"prompt": "0.0000014"},
            "provider_name": "Provider",
            "supported_parameters": ["response_format", "structured_outputs"],
            "status": status
        })
    }

    /// One loopback exchange: captures the exact request bytes and replies
    /// with `response`. No TLS and no real origin is involved.
    fn loopback(response: Vec<u8>) -> (String, Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while let Ok(read) = socket.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = body_offset(&request)
                    && request.len() >= end + declared_length(&request[..end])
                {
                    break;
                }
            }
            let _ = socket.write_all(&response);
            let _ = socket.flush();
            let _ = sender.send(request);
        });
        (format!("http://{address}/"), receiver)
    }

    fn body_offset(request: &[u8]) -> Option<usize> {
        request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|start| start + 4)
    }

    fn declared_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0)
    }

    fn http(status: u16, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn synthetic_provider() -> OpenRouter {
        OpenRouter {
            client: Client::builder().no_proxy().build().unwrap(),
            key: Zeroizing::new("synthetic-key".into()),
            model: MODEL.to_owned(),
            parallel: 1,
            budget: None,
        }
    }

    fn answer(content: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": "gen-synthetic",
            "model": MODEL,
            "choices": [{"index": 0, "finish_reason": "stop",
                "message": {"role": "assistant", "content": content, "refusal": null}}]
        }))
        .unwrap()
    }

    #[test]
    fn listing_keeps_one_healthy_entry_per_model_sorted_by_label() {
        let bytes = listing(&json!([
            endpoint("vendor/zeta", "Vendor: Zeta", 0),
            endpoint("vendor/alpha", "Vendor: Alpha", 0),
            endpoint("vendor/alpha", "Vendor: Alpha", 0),
            endpoint("vendor/degraded", "Vendor: Degraded", -2),
            endpoint("vendor/offline", "Vendor: Offline", -5),
        ]));
        assert_eq!(
            zdr_models(&bytes).unwrap(),
            [
                ModelChoice {
                    id: "vendor/alpha".into(),
                    label: "Vendor: Alpha".into()
                },
                ModelChoice {
                    id: "vendor/zeta".into(),
                    label: "Vendor: Zeta".into()
                },
            ]
        );
        assert_eq!(zdr_models(&listing(&json!([]))).unwrap(), []);
    }

    #[test]
    fn a_listing_that_is_not_a_bounded_data_array_is_rejected() {
        for bytes in [
            br#"{"models":[]}"#.to_vec(),
            br#"{"data":{}}"#.to_vec(),
            br#"["vendor/alpha"]"#.to_vec(),
            b"synthetic-private-canary".to_vec(),
        ] {
            assert_eq!(zdr_models(&bytes), Err(ProviderError::InvalidResponse));
        }
        assert!(
            !ProviderError::InvalidResponse
                .to_string()
                .contains("canary")
        );
        assert_eq!(
            zdr_models(&vec![b'x'; MAX_LISTING + 1]),
            Err(ProviderError::ResponseTooLarge)
        );
    }

    #[test]
    fn unreadable_rows_are_skipped_without_losing_the_healthy_models() {
        let bytes = listing(&json!([
            "vendor/not-an-object",
            endpoint("vendor/ alpha", "Vendor: Space", 0),
            endpoint("vendor/escape", "Vendor: \u{1b}[2JEscape", 0),
            endpoint("vendor/blank", "", 0),
            {"model_id": "vendor/no-name", "status": 0},
            {"model_name": "Vendor: No Id", "status": 0},
            {"model_id": "vendor/text-status", "model_name": "Vendor: Text", "status": "0"},
            endpoint("vendor/good", "Vendor: Good", 0),
            {"model_id": "vendor/absent", "model_name": "Vendor: Absent Status"},
            {"model_id": "vendor/null", "model_name": "Vendor: Null Status", "status": null},
            endpoint("vendor/degraded", "Vendor: Degraded", -2),
        ]));
        assert_eq!(
            zdr_models(&bytes).unwrap(),
            [
                ModelChoice {
                    id: "vendor/absent".into(),
                    label: "Vendor: Absent Status".into()
                },
                ModelChoice {
                    id: "vendor/good".into(),
                    label: "Vendor: Good".into()
                },
                ModelChoice {
                    id: "vendor/null".into(),
                    label: "Vendor: Null Status".into()
                },
            ]
        );
    }

    #[test]
    fn a_variant_suffix_matches_the_selected_model_but_a_longer_id_does_not() {
        assert!(CHAT.starts_with(AUTHORITY) && ZDR_ENDPOINTS.starts_with(AUTHORITY));
        for (reported, matches) in [
            ("vendor/model-1", true),
            ("vendor/model-1:free", true),
            ("vendor/model-1:extended", true),
            ("vendor/model-10", false),
            ("vendor/model-1x", false),
            ("vendor/other", false),
            ("", false),
        ] {
            assert_eq!(model_matches(reported, MODEL), matches, "{reported}");
            let mut value: Value = serde_json::from_slice(&answer("{}")).unwrap();
            value["model"] = json!(reported);
            assert_eq!(
                parse_chat(&serde_json::to_vec(&value).unwrap(), MODEL).is_ok(),
                matches,
                "{reported}"
            );
        }
    }

    #[test]
    fn requests_pin_zdr_routing_and_send_no_structured_output_or_tools() {
        let (url, captured) = loopback(http(200, &answer("{}")));
        let provider = synthetic_provider();
        let content = provider
            .chat_at(&url, request(MODEL, "policy", "data").unwrap(), None)
            .unwrap();
        assert_eq!(&*content, "{}");
        let raw = captured.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = body_offset(&raw).unwrap();
        let sent: Value = serde_json::from_slice(&raw[start..]).unwrap();
        assert_eq!(sent["provider"]["zdr"], true);
        assert_eq!(sent["stream"], false);
        assert_eq!(sent["model"], MODEL);
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(sent["messages"][1]["content"], "data");
        assert_eq!(sent.as_object().unwrap().len(), 4);
        for absent in ["response_format", "structured_outputs", "tools", "n"] {
            assert!(sent.get(absent).is_none(), "{absent} must not be sent");
        }
        let headers = String::from_utf8_lossy(&raw[..start]).to_lowercase();
        assert!(raw.starts_with(b"POST "));
        assert!(headers.contains("authorization: bearer synthetic-key"));
        assert!(request(MODEL, "policy", &"x".repeat(MAX_REQUEST)).is_err());
    }

    #[test]
    fn rejected_credentials_and_oversized_answers_report_fixed_codes() {
        let (url, _captured) = loopback(http(401, br#"{"error":{"message":"no"}}"#));
        assert_eq!(
            synthetic_provider().chat_at(&url, request(MODEL, "policy", "data").unwrap(), None),
            Err(ProviderError::InvalidKey)
        );
        let (url, _captured) = loopback(http(200, &vec![b'x'; MAX_RESPONSE + 1]));
        assert_eq!(
            synthetic_provider().chat_at(&url, request(MODEL, "policy", "data").unwrap(), None),
            Err(ProviderError::ResponseTooLarge)
        );
        for (status, expected) in [
            (402, ProviderError::Quota),
            (429, ProviderError::RateLimited),
            (403, ProviderError::Unauthorized),
            (502, ProviderError::ServerError(502)),
        ] {
            let (url, _captured) = loopback(http(status, b"{}"));
            assert_eq!(
                synthetic_provider().chat_at(&url, request(MODEL, "policy", "data").unwrap(), None),
                Err(expected)
            );
        }
    }

    #[test]
    fn only_one_completed_assistant_choice_from_the_selected_model_is_accepted() {
        assert_eq!(&*parse_chat(&answer("{}"), MODEL).unwrap(), "{}");
        assert_eq!(
            parse_chat(&answer("{}"), "vendor/other"),
            Err(ProviderError::InvalidResponse)
        );
        let mut rejected = Vec::new();
        for (member, replacement) in [
            ("tool_calls", json!([{"function": {"name": "send_mail"}}])),
            ("images", json!(["unexpected"])),
            ("audio", json!({"data": "unexpected"})),
            ("refusal", json!("no")),
            ("content", json!("")),
            ("role", json!("system")),
        ] {
            let mut value: Value = serde_json::from_slice(&answer("{}")).unwrap();
            value["choices"][0]["message"][member] = replacement;
            rejected.push(value);
        }
        let mut truncated: Value = serde_json::from_slice(&answer("{}")).unwrap();
        truncated["choices"][0]["finish_reason"] = json!("length");
        rejected.push(truncated);
        let mut two: Value = serde_json::from_slice(&answer("{}")).unwrap();
        two["choices"] = json!([two["choices"][0].clone(), two["choices"][0].clone()]);
        rejected.push(two);
        let mut failed: Value = serde_json::from_slice(&answer("{}")).unwrap();
        failed["error"] = json!({"message": "no"});
        rejected.push(failed);
        for value in rejected {
            assert_eq!(
                parse_chat(&serde_json::to_vec(&value).unwrap(), MODEL),
                Err(ProviderError::InvalidResponse)
            );
        }
        assert_eq!(
            parse_chat(
                br#"{"model":"vendor/model-1","choices":[],"choices":[{"message":{"role":"assistant","content":"{}"}}]}"#,
                MODEL
            ),
            Err(ProviderError::InvalidResponse)
        );
    }

    #[test]
    fn a_published_request_budget_is_read_from_the_key_status_endpoint() {
        let body = serde_json::to_vec(&json!({
            "data": {
                "label": "synthetic key",
                "usage": 0,
                "is_free_tier": false,
                "rate_limit": {"requests": 20, "interval": "10s"}
            }
        }))
        .unwrap();
        let (url, requests) = loopback(http(200, &body));
        let client = Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            key_budget(&client, "synthetic-key", &url),
            Some((20, Duration::from_secs(10)))
        );
        let sent = String::from_utf8_lossy(&requests.recv().unwrap()).to_lowercase();
        assert!(sent.starts_with("get "), "key status is a GET: {sent}");
        assert!(sent.contains("authorization: bearer synthetic-key"));
    }

    #[test]
    fn a_key_status_without_a_readable_budget_reports_no_budget() {
        for body in [
            json!({"data": {"label": "synthetic key", "usage": 0}}),
            json!({"data": {"rate_limit": {"interval": "10s"}}}),
            json!({"data": {"rate_limit": {"requests": 20}}}),
            json!({"data": {"rate_limit": {"requests": 0, "interval": "10s"}}}),
            json!({"data": {"rate_limit": {"requests": 20, "interval": "0s"}}}),
            json!({"data": {"rate_limit": {"requests": 20, "interval": "10"}}}),
            json!({"data": {"rate_limit": {"requests": 20, "interval": "ss"}}}),
            json!({"data": {"rate_limit": {"requests": 20, "interval": "10d"}}}),
            json!({"data": {"rate_limit": {"requests": 20, "interval": "2h"}}}),
            json!({"data": {"rate_limit": {"requests": -1, "interval": "10s"}}}),
            json!({"data": {"rate_limit": {"requests": 200_000, "interval": "10s"}}}),
            json!({"data": {"rate_limit": "20/10s"}}),
            json!({"data": []}),
            json!({"rate_limit": {"requests": 20, "interval": "10s"}}),
        ] {
            assert_eq!(
                rate_limit(&serde_json::to_vec(&body).unwrap()),
                None,
                "unreadable budget must not produce a guess: {body}"
            );
        }
        // Duplicate members reject rather than collapsing to the last value.
        assert_eq!(
            rate_limit(
                br#"{"data":{"rate_limit":{"requests":1,"requests":900,"interval":"10s"}}}"#
            ),
            None
        );
        assert_eq!(rate_limit(b"synthetic-private-canary"), None);
        assert_eq!(rate_limit(&vec![b'x'; MAX_KEY_STATUS + 1]), None);
        assert_eq!(
            rate_limit(br#"{"data":{"rate_limit":{"requests":600,"interval":"1m"}}}"#),
            Some((600, Duration::from_mins(1)))
        );
        assert_eq!(
            rate_limit(br#"{"data":{"rate_limit":{"requests":1,"interval":"1h"}}}"#),
            Some((1, Duration::from_hours(1)))
        );
    }

    #[test]
    fn a_key_status_that_fails_never_fails_the_connection() {
        let client = Client::builder().no_proxy().build().unwrap();
        let (url, _requests) = loopback(http(401, b"{}"));
        assert_eq!(key_budget(&client, "synthetic-key", &url), None);
        let (url, _requests) = loopback(http(500, b"{}"));
        assert_eq!(key_budget(&client, "synthetic-key", &url), None);
        // A closed port: no answer at all is still only a missing budget.
        assert_eq!(
            key_budget(&client, "synthetic-key", "http://127.0.0.1:1/"),
            None
        );
    }

    #[test]
    fn the_parallel_ceiling_is_clamped_and_defaults_to_one() {
        let provider = synthetic_provider();
        assert_eq!(provider.max_parallel(), 1);
        assert_eq!(provider.request_budget(), None);
        assert_eq!(synthetic_provider().with_max_parallel(0).max_parallel(), 1);
        assert_eq!(synthetic_provider().with_max_parallel(32).max_parallel(), 32);
        assert_eq!(
            synthetic_provider()
                .with_max_parallel(usize::MAX)
                .max_parallel(),
            MAX_PARALLEL_REQUESTS
        );
    }

    #[test]
    fn invalid_keys_and_model_labels_never_reach_the_network() {
        for key in ["", "synthetic key", "synthetic\r\nX-Injected: 1"] {
            assert_eq!(
                OpenRouter::connect(key.to_owned(), MODEL).err(),
                Some(ProviderError::InvalidKey)
            );
        }
        assert_eq!(
            OpenRouter::connect("synthetic-key".into(), "bad\u{1b}[2J").err(),
            Some(ProviderError::ModelUnavailable)
        );
    }
}
