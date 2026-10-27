#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct MockHttpResponse {
    pub status_code: u16,
    pub content_type: String,
    pub body: String,
    pub response_delay: Duration,
}

impl MockHttpResponse {
    pub fn json(body: serde_json::Value) -> Self {
        Self {
            status_code: 200,
            content_type: "application/json".to_string(),
            body: serde_json::to_string(&body).expect("serialize mock response json"),
            response_delay: Duration::from_millis(0),
        }
    }

    #[allow(dead_code)]
    pub fn with_delay(mut self, response_delay: Duration) -> Self {
        self.response_delay = response_delay;
        self
    }
}

#[derive(Debug)]
pub struct MockHttpServer {
    base_url: String,
    captured_requests: Arc<Mutex<Vec<CapturedHttpRequest>>>,
    /// Dropped with the server, which ends the hold of a server from
    /// [`MockHttpServer::start_holding`].
    release: Option<Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

/// Reports when a server from [`MockHttpServer::start_holding`] has captured
/// the request it holds.
#[allow(dead_code)]
#[derive(Debug)]
pub struct MockRequestReceipt {
    received: Receiver<()>,
}

#[allow(dead_code)]
impl MockRequestReceipt {
    /// Blocks until the server has captured its request. Panics if the mock
    /// thread stopped first: it waits 5 s for the connection and 5 s more for
    /// the request.
    pub fn wait(self) {
        self.received
            .recv()
            .expect("the mock HTTP server stopped before it captured a request");
    }
}

impl MockHttpServer {
    pub fn start(responses: Vec<MockHttpResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        listener
            .set_nonblocking(true)
            .expect("set mock listener non-blocking");
        let addr = listener.local_addr().expect("mock listener address");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&captured_requests);
        let join_handle = thread::spawn(move || {
            for response in responses {
                let mut stream = wait_for_connection(&listener, Duration::from_secs(5));
                let request = read_http_request(&mut stream);
                thread_requests
                    .lock()
                    .expect("capture request mutex")
                    .push(request);
                if !response.response_delay.is_zero() {
                    thread::sleep(response.response_delay);
                }
                write_http_response(&mut stream, &response);
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            captured_requests,
            release: None,
            join_handle: Some(join_handle),
        }
    }

    /// A server that captures one request and holds it unanswered, with its
    /// connection open, until the server is dropped; and a receipt that
    /// reports when the request has been captured. A client waiting on the
    /// answer waits for as long as the test keeps the server.
    #[allow(dead_code)]
    pub fn start_holding() -> (Self, MockRequestReceipt) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        listener
            .set_nonblocking(true)
            .expect("set mock listener non-blocking");
        let addr = listener.local_addr().expect("mock listener address");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&captured_requests);
        let (received_tx, received) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let join_handle = thread::spawn(move || {
            let mut stream = wait_for_connection(&listener, Duration::from_secs(5));
            let request = read_http_request(&mut stream);
            thread_requests
                .lock()
                .expect("capture request mutex")
                .push(request);
            // The receipt may already be gone; the request is captured either way.
            let _ = received_tx.send(());
            // Nothing is sent on `release`: dropping the server ends the hold.
            let _ = released.recv();
            drop(stream);
        });

        (
            Self {
                base_url: format!("http://{addr}"),
                captured_requests,
                release: Some(release),
                join_handle: Some(join_handle),
            },
            MockRequestReceipt { received },
        )
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn captured_requests(&self) -> Vec<CapturedHttpRequest> {
        self.captured_requests
            .lock()
            .expect("captured requests mutex")
            .clone()
    }
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn wait_for_connection(listener: &TcpListener, timeout: Duration) -> TcpStream {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => return stream,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for mock HTTP request"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("mock listener accept failed: {err}"),
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> CapturedHttpRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut content_length = None;
    let mut headers_end = None;
    let mut chunk = [0_u8; 4096];

    loop {
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                assert!(Instant::now() < deadline, "timed out reading mock request");
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(err) => panic!("read mock request: {err}"),
        };
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);

        if headers_end.is_none() {
            headers_end = find_headers_end(&bytes);
            if let Some(end) = headers_end {
                let header_text = String::from_utf8_lossy(&bytes[..end]);
                content_length = parse_content_length(&header_text);
            }
        }

        if let Some(end) = headers_end {
            let body_len = content_length.unwrap_or(0);
            if bytes.len() >= end + 4 + body_len {
                break;
            }
        }
    }

    let end = headers_end.expect("request headers delimiter");
    let header_text = String::from_utf8_lossy(&bytes[..end]).to_string();
    let mut header_lines = header_text.lines();
    let request_line = header_lines.next().expect("request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();

    let mut headers = BTreeMap::new();
    for line in header_lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let body_len = content_length.unwrap_or(0);
    let body_start = end + 4;
    let body = if body_len == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes[body_start..body_start + body_len]).to_string()
    };

    CapturedHttpRequest {
        method,
        path,
        headers,
        body,
    }
}

fn parse_content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn find_headers_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_http_response(stream: &mut TcpStream, response: &MockHttpResponse) {
    let reason = match response.status_code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let response_text = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status_code,
        reason,
        response.content_type,
        response.body.len(),
        response.body
    );
    stream
        .write_all(response_text.as_bytes())
        .expect("write mock response");
    stream.flush().expect("flush mock response");
}
