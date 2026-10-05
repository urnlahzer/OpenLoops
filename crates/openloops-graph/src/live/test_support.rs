use std::io::{Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

pub(super) fn one_shot_server(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4096];
        let _ = stream.read(&mut buffer);
        stream.write_all(&response).unwrap();
        let _ = stream.flush();
    });
    (format!("http://127.0.0.1:{port}/"), handle)
}

pub(super) trait ScriptedResponse {
    fn parts(self) -> (Duration, Vec<u8>);
}

impl ScriptedResponse for Vec<u8> {
    fn parts(self) -> (Duration, Vec<u8>) {
        (Duration::ZERO, self)
    }
}

impl ScriptedResponse for (Duration, Vec<u8>) {
    fn parts(self) -> (Duration, Vec<u8>) {
        self
    }
}

pub(super) fn scripted_server<T: ScriptedResponse + Send + 'static>(
    responses: Vec<T>,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls = Arc::new(AtomicUsize::new(0));
    let server_calls = Arc::clone(&calls);
    let handle = std::thread::spawn(move || {
        for response in responses {
            let (delay, response) = response.parts();
            let (mut stream, _) = listener.accept().unwrap();
            server_calls.fetch_add(1, Ordering::Relaxed);
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            std::thread::sleep(delay);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/"), calls, handle)
}

pub(super) fn concurrent_server(
    requests: usize,
    delay: Duration,
) -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    std::thread::JoinHandle<()>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let server_calls = Arc::clone(&calls);
    let server_max = Arc::clone(&max_in_flight);
    let handle = std::thread::spawn(move || {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let mut handlers = Vec::with_capacity(requests);
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let in_flight = Arc::clone(&in_flight);
            let calls = Arc::clone(&server_calls);
            let max_in_flight = Arc::clone(&server_max);
            handlers.push(std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                calls.fetch_add(1, Ordering::SeqCst);
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(delay);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
                let _ = stream.flush();
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handler in handlers {
            handler.join().unwrap();
        }
    });
    (
        format!("http://127.0.0.1:{port}/"),
        calls,
        max_in_flight,
        handle,
    )
}

pub(super) fn routed_server(
    routes: Vec<(&'static str, Vec<u8>)>,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls = Arc::new(AtomicUsize::new(0));
    let server_calls = Arc::clone(&calls);
    let handle = std::thread::spawn(move || {
        for _ in 0..routes.len() {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap_or(0);
            server_calls.fetch_add(1, Ordering::Relaxed);
            let head = String::from_utf8_lossy(&request[..read]);
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            let response = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map_or_else(
                    || {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    },
                    |(_, response)| response.clone(),
                );
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/"), calls, handle)
}

/// Like [`routed_server`], but reads each request in full (head and
/// `Content-Length` body) and records it as text, so a test can assert the
/// method, path and body that were sent.
pub(super) fn recording_routed_server(
    routes: Vec<(&'static str, Vec<u8>)>,
) -> (
    String,
    Arc<std::sync::Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let handle = std::thread::spawn(move || {
        for _ in 0..routes.len() {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            let body_start = loop {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break None;
                }
                request.extend_from_slice(&chunk[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(end + 4);
                }
            };
            if let Some(start) = body_start {
                let head = String::from_utf8_lossy(&request[..start]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while request.len() < start + length {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let path = text
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            recorded.lock().unwrap().push(text);
            let response = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map_or_else(
                    || {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    },
                    |(_, response)| response.clone(),
                );
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/"), requests, handle)
}

#[test]
fn routed_server_selects_by_path_prefix_in_any_order() {
    let response = |body: &str| {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    };
    let (origin, calls, server) = routed_server(vec![
        ("/first", response("one")),
        ("/second", response("two")),
    ]);
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(format!("{origin}second/item"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
        "two"
    );
    assert_eq!(
        client
            .get(format!("{origin}first/item"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
        "one"
    );
    server.join().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}
