//! Provider-agnostic model boundary: the fixed error codes, the bounded
//! HTTPS transport every adapter shares, and the one trait a selected
//! model is reached through. Credentials and payloads are session-only and
//! no upstream error text escapes this module.
use std::io::Read;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use zeroize::Zeroizing;

pub(crate) const MAX_RESPONSE: usize = 262_144;
pub(crate) const MAX_REQUEST: usize = 524_288;
const MAX_KEY: usize = 4096;

/// Fixed error codes only; no upstream error text escapes this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    InvalidKey,
    Unauthorized,
    InputUnavailable,
    Timeout,
    Network,
    RateLimited,
    Quota,
    RequestRejected(u16),
    ServerError(u16),
    ModelUnavailable,
    InvalidResponse,
    InputTooLarge,
    ResponseTooLarge,
    InvalidAnalysis,
    InvalidJson,
    InvalidSchema,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "Enter a valid API key for the selected provider.",
            Self::Unauthorized => "The provider rejected the API key or account access.",
            Self::InputUnavailable => "Could not read a model selection from the terminal.",
            Self::Timeout => "The provider did not finish within 60 seconds. Try the model again or select another model.",
            Self::Network => "Could not establish or complete a secure connection to the selected provider. Check connectivity.",
            Self::RateLimited => "The provider returned HTTP 429 (rate limit). Wait before trying again.",
            Self::Quota => "The provider returned HTTP 402. Check your plan or usage balance.",
            Self::RequestRejected(status) => return write!(f, "The provider rejected the request (HTTP {status}). Check that the selected model supports cloud chat."),
            Self::ServerError(status) => return write!(f, "The provider returned a server error (HTTP {status}). Try again later or choose another model."),
            Self::ModelUnavailable => "The selected model is not in the provider's selectable model list.",
            Self::InvalidResponse => "The provider returned an incomplete or unsupported response.",
            Self::InputTooLarge => "The selected message projection exceeds the analysis limits.",
            Self::ResponseTooLarge => "The provider's response exceeds the allowed size.",
            Self::InvalidAnalysis => "The model output did not pass OpenLoops validation.",
            Self::InvalidJson => "The model returned malformed JSON or extra text instead of the requested result.",
            Self::InvalidSchema => "The model returned JSON with missing, duplicate, or unsupported fields.",
        })
    }
}

/// One consented provider, bound to one exact model label.
///
/// Implementors own their own authority, credential, and wire format;
/// callers see only the selected label and a system/user completion.
pub trait ModelClient {
    /// The exact provider-side model label this client is bound to.
    fn model(&self) -> &str;

    /// Sends one system/user pair and returns the assistant's content.
    ///
    /// Calling this explicitly opts into transmitting `user` to the
    /// configured provider.
    /// # Errors
    /// Returns a fixed provider error. No upstream text, header, or body
    /// escapes, and no alternate model or origin is attempted.
    fn complete(&self, system: &str, user: &str) -> Result<Zeroizing<String>, ProviderError>;
}

/// A write-only provider credential: present, bounded, and free of
/// whitespace that could smuggle a second header line.
pub(crate) fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY && !key.chars().any(char::is_whitespace)
}

/// A provider-side model label safe to echo into a request and compare
/// against a response, with no control or terminal-escape bytes.
pub(crate) fn valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

/// The only transport any adapter may build: direct valid-TLS HTTPS, no
/// proxy, no redirect, bounded connect and wall time.
pub(crate) fn https_client() -> Result<Client, ProviderError> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_mins(1))
        .build()
        .map_err(|error| transport_error(&error))
}

/// Reads a success body under `limit` bytes. The caller has already mapped
/// the status, so this never inspects it, and picks the limit its own
/// contract allows: `MAX_RESPONSE` for a completion, a larger adapter-fixed
/// bound for a content-free catalog listing.
pub(crate) fn read_body(
    response: Response,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(ProviderError::ResponseTooLarge);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut
                || error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<reqwest::Error>())
                    .is_some_and(reqwest::Error::is_timeout)
            {
                ProviderError::Timeout
            } else {
                ProviderError::Network
            }
        })?;
    if bytes.len() > limit {
        return Err(ProviderError::ResponseTooLarge);
    }
    Ok(bytes)
}

/// Accept only an entire JSON document, optionally inside one Markdown JSON fence.
/// Never search prose for a plausible object or discard text around a fence.
pub(crate) fn json_document(bytes: &[u8]) -> Result<&[u8], ProviderError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(ProviderError::ResponseTooLarge);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ProviderError::InvalidJson)?
        .trim();
    if let Some(fenced) = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```json\r\n"))
        .or_else(|| text.strip_prefix("```\n"))
    {
        return fenced
            .strip_suffix("```")
            .map(|value| value.trim().as_bytes())
            .ok_or(ProviderError::InvalidJson);
    }
    Ok(text.as_bytes())
}

pub(crate) fn parse_error(error: openloops_contracts::ParseRejection) -> ProviderError {
    use openloops_contracts::ParseRejection;
    match error {
        ParseRejection::ResponseTooLarge => ProviderError::ResponseTooLarge,
        ParseRejection::InvalidUtf8 | ParseRejection::InvalidJson => ProviderError::InvalidJson,
        ParseRejection::InvalidSchema => ProviderError::InvalidSchema,
    }
}

pub(crate) fn transport_error(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Network
    }
}

pub(crate) fn status_error(status: u16) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized,
        402 => ProviderError::Quota,
        429 => ProviderError::RateLimited,
        500..=599 => ProviderError::ServerError(status),
        _ => ProviderError::RequestRejected(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_model_labels_reject_whitespace_and_terminal_escapes() {
        assert!(valid_key("synthetic-key"));
        assert!(!valid_key(""));
        assert!(!valid_key("synthetic key"));
        assert!(!valid_key("synthetic\r\nX-Injected: 1"));
        assert!(!valid_key(&"x".repeat(MAX_KEY + 1)));
        assert!(valid_model_name("vendor/model-1.2:free"));
        assert!(!valid_model_name(""));
        assert!(!valid_model_name("bad\u{1b}[2J"));
        assert!(!valid_model_name(&"x".repeat(257)));
    }

    #[test]
    fn timeout_while_reading_response_body_is_reported_as_timeout() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n")
                .unwrap();
            let _ = hold.recv_timeout(Duration::from_secs(2));
        });
        let response = Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .unwrap();
        let result = read_body(response, MAX_RESPONSE);
        drop(release);
        server.join().unwrap();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
    }

    #[test]
    fn error_text_names_no_provider_upstream_payload_or_endpoint() {
        for error in [
            ProviderError::InvalidKey,
            ProviderError::Unauthorized,
            ProviderError::Network,
            ProviderError::ModelUnavailable,
            ProviderError::InvalidResponse,
        ] {
            let text = error.to_string();
            assert!(!text.contains("http"), "error text must name no endpoint");
        }
        assert!(ProviderError::Timeout.to_string().contains("60 seconds"));
        assert!(
            ProviderError::ServerError(502)
                .to_string()
                .contains("HTTP 502")
        );
    }
}
