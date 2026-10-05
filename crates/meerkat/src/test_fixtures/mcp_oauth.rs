//! Deterministic OAuth-protected MCP server for MCP OAuth canary tests
//! (feature `test-mcp-oauth-fixtures`; never production composition).
//!
//! One loopback HTTP/1.1 listener serves an MCP endpoint that requires the
//! fixture's bearer token, a public MCP endpoint, RFC 9728 protected-resource
//! metadata, RFC 8414 authorization-server metadata, dynamic client
//! registration, an authorize endpoint that redirects straight back to the
//! registered loopback redirect, a token endpoint, and OpenID Connect
//! discovery plus UserInfo. Every secret it issues is a fixed canary, so
//! tests can assert those values never reach agent-visible channels or logs.
//! It is hand-written on tokio so it adds no dependency.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The authorization code the authorize endpoint issues.
pub const CODE_CANARY: &str = "code-canary-7f3a9c";
/// The access token the token endpoint issues.
pub const ACCESS_CANARY: &str = "access-canary-51be04";
/// The refresh token the token endpoint issues.
pub const REFRESH_CANARY: &str = "refresh-canary-c2d811";
/// The ID token the token endpoint issues.
pub const ID_TOKEN_CANARY: &str = "id-token-canary-93af10";
/// The client secret dynamic registration returns (public clients ignore it).
pub const DCR_SECRET_CANARY: &str = "dcr-secret-canary-0e6f";
/// The OIDC subject UserInfo reports.
pub const SUBJECT: &str = "oidc-subject-7";
/// The text the MCP `echo` tool returns.
pub const ECHO_REPLY: &str = "echo-reply-visible-to-agent";

/// Every fixed secret this fixture issues.
pub const ISSUED_SECRET_CANARIES: [&str; 5] = [
    CODE_CANARY,
    ACCESS_CANARY,
    REFRESH_CANARY,
    ID_TOKEN_CANARY,
    DCR_SECRET_CANARY,
];

#[derive(Default)]
struct FixtureState {
    redirect_uri: Mutex<Option<String>>,
    request_paths: Mutex<Vec<String>>,
    fail_token_exchange: AtomicBool,
}

/// A running fixture. The listener lives for the rest of the process.
#[derive(Clone)]
pub struct McpOAuthFixture {
    base: String,
    state: Arc<FixtureState>,
}

impl McpOAuthFixture {
    /// Bind a loopback listener and serve until the process exits.
    pub async fn spawn() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(FixtureState::default());
        let served = Arc::clone(&state);
        let served_base = base.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&served);
                let base = served_base.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, &base, &state).await;
                });
            }
        });
        Ok(Self { base, state })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The OAuth-protected MCP endpoint.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    /// An MCP endpoint that needs no authorization.
    pub fn public_mcp_url(&self) -> String {
        format!("{}/public", self.base)
    }

    /// Paths requested so far, in order.
    pub fn request_paths(&self) -> Vec<String> {
        self.state
            .request_paths
            .lock()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Make the token endpoint refuse authorization-code exchanges.
    pub fn fail_token_exchange(&self, fail: bool) {
        self.state.fail_token_exchange.store(fail, Ordering::SeqCst);
    }
}

/// Follow an authorize URL the way a browser would (redirects followed), so
/// the authorization server's redirect reaches the host's loopback callback.
pub async fn follow_authorize_url(url: &str) -> std::io::Result<()> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(std::io::Error::other)?
        .get(url)
        .send()
        .await
        .map_err(std::io::Error::other)?;
    Ok(())
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    host: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "application/json".to_owned())],
            body: value.to_string().into_bytes(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    base: &str,
    state: &FixtureState,
) -> std::io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    if let Ok(mut paths) = state.request_paths.lock() {
        paths.push(request.path.clone());
    }
    let response = route(&request, base, state);
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        _ => "Not Found",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.shutdown().await
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or("/").to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let url =
        reqwest::Url::parse(&format!("http://fixture{target}")).map_err(std::io::Error::other)?;
    Ok(Some(Request {
        method,
        path: url.path().to_owned(),
        query: url.query_pairs().into_owned().collect(),
        host: headers.get("host").cloned().unwrap_or_default(),
        authorization: headers.get("authorization").cloned(),
        body,
    }))
}

fn form(body: &[u8]) -> HashMap<String, String> {
    reqwest::Url::parse(&format!(
        "http://fixture/?{}",
        String::from_utf8_lossy(body)
    ))
    .map(|url| url.query_pairs().into_owned().collect())
    .unwrap_or_default()
}

fn route(request: &Request, base: &str, state: &FixtureState) -> Response {
    let origin = if request.host.is_empty() {
        base.to_owned()
    } else {
        format!("http://{}", request.host)
    };
    let bearer = request
        .authorization
        .as_deref()
        .and_then(|value| value.strip_prefix("Bearer "));
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/mcp") => {
            if bearer != Some(ACCESS_CANARY) {
                let mut response = Response::empty(401);
                response.headers.push((
                    "WWW-Authenticate",
                    r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#
                        .to_owned(),
                ));
                return response;
            }
            mcp_reply(&request.body)
        }
        ("POST", "/public") => mcp_reply(&request.body),
        ("GET", "/.well-known/oauth-protected-resource/mcp") => Response::json(
            200,
            &json!({
                "resource": format!("{origin}/mcp"),
                "authorization_servers": [origin],
            }),
        ),
        ("GET", "/.well-known/oauth-authorization-server") => Response::json(
            200,
            &json!({
                "issuer": origin,
                "code_challenge_methods_supported": ["S256"],
                "authorization_endpoint": "/authorize",
                "token_endpoint": "/token",
                "registration_endpoint": "/register",
            }),
        ),
        ("GET", "/.well-known/openid-configuration") => Response::json(
            200,
            &json!({
                "issuer": origin,
                "userinfo_endpoint": format!("{origin}/userinfo"),
            }),
        ),
        ("POST", "/register") => {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            if let (Some(redirect), Ok(mut slot)) =
                (body["redirect_uris"][0].as_str(), state.redirect_uri.lock())
            {
                *slot = Some(redirect.to_owned());
            }
            Response::json(
                200,
                &json!({
                    "client_id": "client-123",
                    "client_secret": DCR_SECRET_CANARY,
                    "token_endpoint_auth_method": "none",
                }),
            )
        }
        ("GET", "/authorize") => {
            let redirect = state
                .redirect_uri
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_default();
            let mut location = match reqwest::Url::parse(&redirect) {
                Ok(location) => location,
                Err(_) => return Response::empty(400),
            };
            location
                .query_pairs_mut()
                .append_pair("code", CODE_CANARY)
                .append_pair(
                    "state",
                    request.query.get("state").map(String::as_str).unwrap_or(""),
                );
            let mut response = Response::empty(307);
            response.headers.push(("Location", location.to_string()));
            response
        }
        ("POST", "/token") => {
            let body = form(&request.body);
            if state.fail_token_exchange.load(Ordering::SeqCst)
                || body.get("code").map(String::as_str) != Some(CODE_CANARY)
            {
                return Response::json(400, &json!({ "error": "invalid_grant" }));
            }
            Response::json(
                200,
                &json!({
                    "access_token": ACCESS_CANARY,
                    "refresh_token": REFRESH_CANARY,
                    "id_token": ID_TOKEN_CANARY,
                    "expires_in": 3600,
                    "scope": "openid",
                    "token_type": "Bearer",
                }),
            )
        }
        ("GET", "/userinfo") => {
            if bearer != Some(ACCESS_CANARY) {
                return Response::empty(401);
            }
            Response::json(200, &json!({ "sub": SUBJECT }))
        }
        _ => Response::empty(404),
    }
}

fn mcp_reply(body: &[u8]) -> Response {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let Some(id) = request.get("id").cloned() else {
        return Response::empty(202);
    };
    let result = match request["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "canary-mcp", "version": "0.1.0" },
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "echo",
                "description": "Echo input",
                "inputSchema": { "type": "object", "properties": {} },
            }]
        }),
        Some("tools/call") => json!({
            "content": [{ "type": "text", "text": ECHO_REPLY }],
        }),
        other => {
            return Response::json(
                200,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("unsupported {other:?}") },
                }),
            );
        }
    };
    Response::json(
        200,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
}
