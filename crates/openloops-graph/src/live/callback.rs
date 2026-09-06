//! Bounded localhost callback with state validation before a success response.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use oauth2::CsrfToken;

use super::ConnectionError;

const MAX_HEAD: usize = 16 * 1024;

pub(super) struct Listener {
    socket: TcpListener,
    port: u16,
}

impl Listener {
    pub(super) fn bind() -> Result<Self, ConnectionError> {
        let socket = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| ConnectionError::CallbackUnavailable)?;
        socket
            .set_nonblocking(true)
            .map_err(|_| ConnectionError::CallbackUnavailable)?;
        let port = socket
            .local_addr()
            .map_err(|_| ConnectionError::CallbackUnavailable)?
            .port();
        Ok(Self { socket, port })
    }

    pub(super) fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }

    pub(super) fn wait(
        self,
        state: &CsrfToken,
        timeout: Duration,
    ) -> Result<String, ConnectionError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.socket.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let request_deadline = deadline.min(Instant::now() + Duration::from_secs(3));
                    let outcome = read_head(&mut stream, request_deadline)
                        .and_then(|head| parse_head(&head, self.port, state));
                    respond(&mut stream, outcome.is_ok());
                    match outcome {
                        Ok(code) => return Ok(code),
                        Err(ConnectionError::ConsentDenied) => {
                            return Err(ConnectionError::ConsentDenied);
                        }
                        // An unsolicited request cannot consume the pending sign-in.
                        Err(_) => {}
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return Err(ConnectionError::CallbackUnavailable),
            }
        }
        Err(ConnectionError::SignInTimedOut)
    }
}

fn read_head(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, ConnectionError> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || head.len() >= MAX_HEAD {
            return Err(ConnectionError::CallbackUnavailable);
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| ConnectionError::CallbackUnavailable)?;
        let read = stream
            .read(&mut chunk)
            .map_err(|_| ConnectionError::CallbackUnavailable)?;
        if read == 0 {
            return Err(ConnectionError::CallbackUnavailable);
        }
        head.extend_from_slice(&chunk[..read]);
        if head.len() > MAX_HEAD {
            return Err(ConnectionError::CallbackUnavailable);
        }
        if head.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            return Ok(head);
        }
    }
}

fn parse_head(head: &[u8], port: u16, expected: &CsrfToken) -> Result<String, ConnectionError> {
    let reject = ConnectionError::CallbackUnavailable;
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut request = httparse::Request::new(&mut headers);
    if head.len() > MAX_HEAD
        || !request.parse(head).map_err(|_| reject)?.is_complete()
        || request.method != Some("GET")
        || request.version != Some(1)
    {
        return Err(reject);
    }
    let expected_host = format!("localhost:{port}");
    let hosts: Vec<_> = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("host"))
        .collect();
    if hosts.len() != 1
        || !hosts[0]
            .value
            .eq_ignore_ascii_case(expected_host.as_bytes())
    {
        return Err(reject);
    }
    let target = request.path.ok_or(reject)?;
    if target.contains('#') || !target.starts_with("/?") {
        return Err(reject);
    }
    let query = &target[2..];
    // url's form decoder is deliberately forgiving; reject malformed escapes first.
    for (index, byte) in query.bytes().enumerate() {
        if byte == b'%'
            && !query
                .as_bytes()
                .get(index + 1..index + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(reject);
        }
    }
    let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
    if pairs.len() > 32 || pairs.is_empty() {
        return Err(reject);
    }
    let mut seen = std::collections::HashSet::new();
    for (key, value) in &pairs {
        if key.is_empty() || !seen.insert(key.as_ref()) || value.contains('\u{fffd}') {
            return Err(reject);
        }
    }
    let field = |name| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_ref())
    };
    let supplied = field("state")
        .filter(|value| !value.is_empty())
        .ok_or(reject)?;
    if CsrfToken::new(supplied.to_owned()) != *expected {
        return Err(reject);
    }
    match (field("code"), field("error")) {
        (Some(code), None) if !code.is_empty() => Ok(code.to_owned()),
        (None, Some(error)) if !error.is_empty() => Err(ConnectionError::ConsentDenied),
        _ => Err(reject),
    }
}

fn respond(stream: &mut TcpStream, accepted: bool) {
    let (status, body) = if accepted {
        (
            "200 OK",
            "Sign-in received. Return to OpenLoops to see the connection result.",
        )
    } else {
        (
            "400 Bad Request",
            "This callback was not accepted. Return to OpenLoops.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nPragma: no-cache\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .is_ok()
    {
        let _ = stream.write_all(response.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(target: &str, host: &str) -> Vec<u8> {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()
    }

    #[test]
    fn strict_callback_rejects_injected_malformed_and_duplicate_parameters() {
        let state = CsrfToken::new("synthetic-state".into());
        for target in [
            "/?code=x&state=wrong",
            "/?code=x&state=synthetic-state&state=synthetic-state",
            "/?code=x&state=synthetic-state&%73tate=synthetic-state",
            "/?code=x&error=denied&state=synthetic-state",
            "/?code=&state=synthetic-state",
            "/?code=%ZZ&state=synthetic-state",
            "/callback?code=x&state=synthetic-state",
            "http://localhost:1234/?code=x&state=synthetic-state",
            "/?code=x&state=synthetic-state#fragment",
        ] {
            assert!(parse_head(&request(target, "localhost:1234"), 1234, &state).is_err());
        }
        for host in [
            "localhost:9999",
            "evil.invalid",
            "127.0.0.1:1234",
            "localhost:1234\r\nHost: localhost:1234",
        ] {
            assert!(
                parse_head(
                    &request("/?code=x&state=synthetic-state", host),
                    1234,
                    &state
                )
                .is_err()
            );
        }
        assert_eq!(
            parse_head(
                &request(
                    "/?code=x&state=synthetic-state&session_state=ignored",
                    "localhost:1234"
                ),
                1234,
                &state
            ),
            Ok("x".into())
        );
    }

    #[test]
    fn error_callback_requires_matching_state() {
        let state = CsrfToken::new("synthetic-state".into());
        assert_eq!(
            parse_head(
                &request(
                    "/?error=access_denied&state=synthetic-state",
                    "localhost:1234"
                ),
                1234,
                &state
            ),
            Err(ConnectionError::ConsentDenied)
        );
        assert_eq!(
            parse_head(
                &request("/?error=access_denied&state=wrong", "localhost:1234"),
                1234,
                &state
            ),
            Err(ConnectionError::CallbackUnavailable)
        );
    }

    #[test]
    fn listener_times_out_without_any_connection() {
        let listener = Listener::bind().unwrap();
        assert_eq!(
            listener.wait(
                &CsrfToken::new("synthetic".into()),
                Duration::from_millis(30)
            ),
            Err(ConnectionError::SignInTimedOut)
        );
    }

    #[test]
    fn invalid_request_does_not_consume_real_pending_callback() {
        let listener = Listener::bind().unwrap();
        let port = listener.port;
        let worker = std::thread::spawn(move || {
            listener.wait(
                &CsrfToken::new("synthetic-state".into()),
                Duration::from_secs(5),
            )
        });
        for (state, expected_status) in
            [("wrong", "400 Bad Request"), ("synthetic-state", "200 OK")]
        {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .write_all(&request(
                    &format!("/?code=synthetic-code&state={state}"),
                    &format!("localhost:{port}"),
                ))
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.contains(expected_status));
            assert!(response.contains("Cache-Control: no-store"));
            assert!(!response.contains("synthetic-code"));
        }
        assert_eq!(worker.join().unwrap(), Ok("synthetic-code".into()));
    }
}
