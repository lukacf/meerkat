//! MCP connection management

use crate::client_service::{ClientServiceSelection, ConnectedClient, McpClientServiceFactory};
use crate::transport::protected::{
    ProtectedMetadata, ProtectedMetadataState, ProtectedStdioTransport,
};
use crate::transport::sse::{SseClientConfig, SseClientTransport};
use crate::transport::streamable_http::{
    RequestDispatch, RequestDisposition, SessionExpiryRecorder,
};
use crate::transport::{
    headers_from_map,
    sse::ReqwestSseClient,
    streamable_http::{OAuthBearer, ReqwestStreamableHttpClient},
};
use crate::{McpError, McpStdioLaunchProfile};
use async_trait::async_trait;
use meerkat_auth_core::{McpAuthMode, McpOAuthError, McpServerIdentity};
use meerkat_core::McpServerConfig;
use meerkat_core::ToolDef;
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use meerkat_core::types::ContentBlock;
use rmcp::model::{CallToolRequest, CallToolRequestParams, CallToolResult, ServerResult};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// Connection to an MCP server
pub struct McpConnection {
    config: McpServerConfig,
    connection_id: crate::McpConnectionId,
    protected_metadata: ProtectedMetadataState,
    service: ConnectedClient,
    /// The stdio server's process, owned until `close` observes its exit.
    stdio_child: Option<StdioChildCustody>,
    /// Set once a Streamable HTTP server drops this connection's session.
    /// Never set for stdio or SSE connections.
    session_expiry: SessionExpiryRecorder,
    /// The OAuth target whose bearer this connection resolves per request.
    oauth_target: Option<McpServerIdentity>,
    /// Full standard discovery, for host replay. Model definitions are a
    /// separate projection and do not carry private UI metadata.
    standard_tools: std::sync::RwLock<Vec<rmcp::model::Tool>>,
}

/// After the process group is killed, how long [`StdioChildCustody::terminate`]
/// waits for the server's stdout to reach EOF. The EOF is the signal that every
/// process holding the pipe has exited; this bound only guards against a
/// process that escaped the group (for example via `setsid`) and still holds
/// it, which would otherwise wedge shutdown.
#[cfg(unix)]
const STDIO_GROUP_EXIT_BACKSTOP: Duration = Duration::from_secs(10);

/// Typed owner of a stdio MCP server's process.
///
/// The router (or a direct connection) holds this from the moment the process
/// is spawned, before the handshake, so no connect future exclusively owns the
/// process. [`Self::terminate`] ends it and returns only once it has exited:
/// on Unix the server runs in its own process group, which is killed as a
/// whole (wrappers such as `sh -c`, `npx` or `uvx` make the real server a
/// grandchild), the direct child is reaped, and EOF on a duplicate of its
/// stdout proves every process holding the pipe has exited. Elsewhere only the
/// direct child is killed and reaped.
#[derive(Clone, Default)]
pub(crate) struct StdioChildCustody {
    slot: Arc<tokio::sync::Mutex<Option<StdioChild>>>,
    #[cfg(test)]
    spawned: Arc<tokio::sync::watch::Sender<Option<u32>>>,
    #[cfg(all(test, unix))]
    reap_gate: Arc<std::sync::Mutex<Option<StdioReapTestGate>>>,
    #[cfg(all(test, unix))]
    direct_kill_requests: Arc<std::sync::atomic::AtomicUsize>,
}

/// Holds the existing reap await pending so cancellation tests do not depend
/// on how quickly the operating system delivers SIGKILL.
#[cfg(all(test, unix))]
struct StdioReapTestGate {
    entered: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

struct StdioChild {
    child: meerkat_sandbox::ProcessChild,
    /// The server's process group, while it may still need killing. Cleared
    /// before the leader is reaped, so a reused id is never signalled.
    #[cfg(unix)]
    process_group: Option<nix::unistd::Pid>,
    #[cfg(unix)]
    stdout_witness: Option<tokio::net::unix::pipe::Receiver>,
}

impl Drop for StdioChild {
    /// Fallback only, for a custody dropped without `terminate`: kill the
    /// whole group without waiting (`kill_on_drop` covers the direct child).
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.process_group.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn confined_stdio_spawn_error(error: std::io::Error) -> McpError {
    match error
        .get_ref()
        .and_then(|source| source.downcast_ref::<meerkat_core::confinement::ConfinementRefusal>())
    {
        Some(refusal) => McpError::Confinement(*refusal),
        None => McpError::Io(error),
    }
}

impl StdioChildCustody {
    /// Spawn the server and take custody of it; returns its stdout and stdin
    /// for the MCP transport.
    async fn spawn(
        &self,
        stdio: &meerkat_core::mcp_config::McpStdioConfig,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(tokio::process::ChildStdout, tokio::process::ChildStdin), McpError> {
        // Acquire custody before spawning, with no await between creating the
        // child and installing its owner.
        let mut slot = self.slot.lock().await;
        // Binding validates the exact launch and retained backend before any
        // process exists. Required isolation has no Command fallback.
        let mut child: meerkat_sandbox::ProcessChild = match profile.prepare(stdio)? {
            Some(prepared) => {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                {
                    prepared
                        .spawn_with_io(meerkat_sandbox::SpawnIo {
                            stdin: meerkat_sandbox::StdioMode::Piped,
                            stdout: meerkat_sandbox::StdioMode::Piped,
                            stderr: meerkat_sandbox::StdioMode::Null,
                        })
                        .map_err(confined_stdio_spawn_error)?
                        .into()
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let _ = prepared;
                    return Err(
                        meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement
                            .into(),
                    );
                }
            }
            None => {
                let mut cmd = Command::new(&stdio.command);
                cmd.args(&stdio.args);
                for (key, value) in &stdio.env {
                    cmd.env(key, value);
                }
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    // Fallback only: the owner path is `terminate`.
                    .kill_on_drop(true);
                #[cfg(unix)]
                cmd.process_group(0);
                cmd.spawn()
                    .map_err(|e| McpError::ConnectionFailed {
                        reason: format!("Failed to spawn process: {e}"),
                    })?
                    .into()
            }
        };
        let (Some(stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
            return Err(McpError::ConnectionFailed {
                reason: "spawned MCP server has no piped stdin/stdout".to_string(),
            });
        };
        #[cfg(unix)]
        let stdout_witness = {
            use std::os::fd::AsFd as _;
            stdout
                .as_fd()
                .try_clone_to_owned()
                .ok()
                .and_then(|fd| tokio::net::unix::pipe::Receiver::from_owned_fd(fd).ok())
        };
        #[cfg(unix)]
        let process_group = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .map(nix::unistd::Pid::from_raw);
        #[cfg(test)]
        self.spawned.send_replace(child.id());
        let replaced = slot.replace(StdioChild {
            child,
            #[cfg(unix)]
            process_group,
            #[cfg(unix)]
            stdout_witness,
        });
        // One custody holds one process; a replaced one is killed by its drop.
        drop(replaced);
        Ok((stdout, stdin))
    }

    /// Kill the server (its whole process group on Unix) and return once it
    /// has exited. `None` when nothing is in custody (never spawned, or
    /// already terminated).
    pub(crate) async fn terminate(&self) -> Option<std::io::Result<std::process::ExitStatus>> {
        // Cancellation releases the lock without dropping the child, so a
        // retained owner can finish reaping it. Concurrent callers wait here.
        let mut slot = self.slot.lock().await;
        let held = slot.as_mut()?;
        #[cfg(unix)]
        if let Some(group) = held.process_group.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            // A failed wait may mean another reaper consumed the status.
            // Issue both signals once, before any reap, so retries cannot
            // signal a reused process id.
            #[cfg(test)]
            self.direct_kill_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = held.child.start_kill();
        }
        // Off Unix, the process handle identifies the direct child.
        #[cfg(not(unix))]
        let _ = held.child.start_kill();
        #[cfg(all(test, unix))]
        {
            let gate = self
                .reap_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                let _ = gate.resume.await;
            }
        }
        let status = match held.child.wait().await {
            Ok(status) => status,
            Err(error) => return Some(Err(error)),
        };
        #[cfg(unix)]
        if let Some(witness) = held.stdout_witness.as_mut() {
            use tokio::io::AsyncReadExt as _;
            let mut discard = [0u8; 4096];
            let all_writers_exited =
                async { while matches!(witness.read(&mut discard).await, Ok(read) if read > 0) {} };
            if tokio::time::timeout(STDIO_GROUP_EXIT_BACKSTOP, all_writers_exited)
                .await
                .is_err()
            {
                tracing::warn!(
                    "a process outside the MCP stdio server's process group still holds its stdout after the group was killed"
                );
            }
        }
        slot.take();
        Some(Ok(status))
    }

    #[cfg(all(test, unix))]
    fn pause_next_reap(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        *self
            .reap_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(StdioReapTestGate {
            entered,
            resume: resumed,
        });
        (waiting, resume)
    }

    /// The spawned process id, once spawned.
    #[cfg(all(test, unix))]
    #[allow(clippy::expect_used)]
    pub(crate) async fn spawned_pid(&self) -> u32 {
        let mut spawned = self.spawned.subscribe();
        let pid = spawned
            .wait_for(Option::is_some)
            .await
            .expect("custody owns its spawn signal");
        pid.expect("waited for a spawned pid")
    }
}

/// Credential source for OAuth-protected MCP servers.
///
/// A Streamable HTTP connection reads its bearer through
/// `stored_bearer_token` on every request, so the credential owner's current
/// (refreshed) credential is used without a reconnect, and a request without
/// a usable credential is refused before it is sent.
///
/// `interactive_login` is only reached in [`McpAuthMode::Interactive`]. A
/// resolver that completes it commits the credential it returns, so that
/// `stored_bearer_token` yields it from then on. A resolver without a host
/// browser channel returns [`McpOAuthError::HumanAuthorizationRequired`],
/// which the connection reports as the typed
/// [`McpError::AuthorizationRequired`] host status. A host that owns the
/// browser drives `McpOAuthAuthority::login_start`/`login_complete` itself.
#[async_trait]
pub trait McpAuthResolver: Send + Sync {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError>;

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError>;
}

#[async_trait]
impl McpAuthResolver for meerkat_auth_core::McpOAuthAuthority {
    /// Unselected targets (no `oauth_account` or `oauth_account_selection`)
    /// keep their stored-only semantics, so servers that need no OAuth
    /// connect as before.
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        if !target.is_selected() {
            return self.stored_only().stored_bearer_token(target).await;
        }
        self.stored_bearer_token(target).await
    }

    /// The native authority has no browser: human authorization is a host
    /// obligation, reported as typed status instead of opening anything.
    /// Interactive login needs an account selection.
    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        _www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        if !target.is_selected() {
            return Err(McpOAuthError::AccountSelectionRequired);
        }
        Err(McpOAuthError::HumanAuthorizationRequired {
            server_name: target.server_name().to_owned(),
        })
    }
}

impl McpConnection {
    /// Connect to an MCP server and perform the initialize handshake
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        Self::connect_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<Self, McpError> {
        Self::connect_with_services(config, auth_mode, auth_resolver, None).await
    }

    /// Connect with an optional host client-service factory. The first profile
    /// forwards form elicitation only; authentication remains with the resolver.
    /// Each physical attempt receives a fresh service for this exact config.
    pub async fn connect_with_services(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
    ) -> Result<Self, McpError> {
        Self::connect_with_services_and_stdio_profile(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await
    }

    /// Connect with a retained host profile for local process isolation.
    /// Remote HTTP transports are outside this local-process boundary.
    pub async fn connect_with_stdio_profile(
        config: &McpServerConfig,
        profile: &McpStdioLaunchProfile,
    ) -> Result<Self, McpError> {
        Self::connect_with_services_and_stdio_profile(
            config,
            McpAuthMode::Stored,
            None,
            None,
            profile,
        )
        .await
    }

    /// Connect with the same explicit host service and local-launch owners.
    pub async fn connect_with_services_and_stdio_profile(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<Self, McpError> {
        Self::connect_with_custody(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            None,
            profile,
        )
        .await
    }

    /// [`Self::connect_with_services`] with the custody a stdio server's
    /// process is deposited into before the handshake (a fresh one when
    /// `None`). On failure the process is terminated before returning.
    pub(crate) async fn connect_with_custody(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<Self, McpError> {
        if matches!(config.transport, McpTransportConfig::Http(_)) {
            let target = McpServerIdentity::from_config(config)
                .map_err(mcp_auth_error_to_connection_failed)?;
            if target.is_selected() && auth_resolver.is_none() {
                return Err(McpError::OAuthAccountRejected(
                    McpOAuthError::UnsupportedAccountSelection,
                ));
            }
        }
        let connection_id = crate::McpConnectionId::allocate()?;
        let protected_metadata = ProtectedMetadataState::default();
        // Refusal precedes process spawn, SSE startup and HTTP transport effects.
        let client = ClientServiceSelection::select(config, client_factory.as_deref())?;
        let mut stdio_child = None;
        let service = match &config.transport {
            McpTransportConfig::Stdio(stdio) => {
                // We own the process; rmcp only gets its stdout/stdin.
                let custody = stdio_custody.unwrap_or_default();
                let (stdout, stdin) = custody.spawn(stdio, profile).await?;
                match client
                    .serve(ProtectedStdioTransport::new(
                        stdout,
                        stdin,
                        protected_metadata.clone(),
                    ))
                    .await
                {
                    Ok(service) => {
                        stdio_child = Some(custody);
                        service
                    }
                    Err(e) => {
                        custody.terminate().await;
                        return Err(McpError::ConnectionFailed {
                            reason: format!("Failed to establish MCP connection: {e}"),
                        });
                    }
                }
            }
            McpTransportConfig::Http(http) => {
                let headers = headers_from_map(&http.headers)
                    .map_err(|e| McpError::ConnectionFailed { reason: e })?;
                match http.transport.unwrap_or_default() {
                    McpHttpTransport::StreamableHttp => {
                        return Self::connect_streamable_http(
                            config,
                            headers,
                            &http.url,
                            auth_mode,
                            auth_resolver,
                            client_factory,
                            client,
                        )
                        .await;
                    }
                    McpHttpTransport::Sse => {
                        let http_client = ReqwestSseClient::new(headers)
                            .with_protected_metadata(protected_metadata.clone());
                        let transport = SseClientTransport::start_with_client(
                            http_client,
                            SseClientConfig {
                                sse_endpoint: http.url.clone().into(),
                                use_message_endpoint: None,
                            },
                        )
                        .await
                        .map_err(|e| McpError::ConnectionFailed {
                            reason: format!("Failed to establish SSE connection: {e}"),
                        })?;
                        client
                            .serve(transport)
                            .await
                            .map_err(|e| McpError::ConnectionFailed {
                                reason: format!("Failed to establish MCP connection: {e}"),
                            })?
                    }
                }
            }
        };

        Ok(Self {
            config: config.clone(),
            connection_id,
            protected_metadata,
            service,
            stdio_child,
            session_expiry: Default::default(),
            oauth_target: None,
            standard_tools: Default::default(),
        })
    }

    async fn connect_streamable_http(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        client: ClientServiceSelection,
    ) -> Result<Self, McpError> {
        let has_static_authorization = headers
            .keys()
            .any(|name| name.as_str().eq_ignore_ascii_case("authorization"));
        if has_static_authorization {
            return Self::connect_streamable_http_once(config, headers, url, None, None, client)
                .await
                .map_err(StreamableConnectError::into_mcp_error);
        }
        let target =
            McpServerIdentity::from_config(config).map_err(mcp_auth_error_to_connection_failed)?;
        // The connection resolves its bearer per request through the resolver;
        // this connect-time read only decides the path before any transport.
        let bearer = auth_resolver
            .as_ref()
            .map(|resolver| OAuthBearer::new(Arc::clone(resolver), target.clone()));
        let mut has_stored_token = false;
        let mut force_interactive_reauth = false;
        if let Some(resolver) = auth_resolver.as_deref() {
            match resolver.stored_bearer_token(&target).await {
                Ok(None) if target.is_selected() => {
                    if matches!(auth_mode, McpAuthMode::Interactive) {
                        force_interactive_reauth = true;
                    } else {
                        return Err(McpError::OAuthAccountRejected(
                            McpOAuthError::MissingStoredToken {
                                server_name: config.name.clone(),
                            },
                        ));
                    }
                }
                Ok(token) => has_stored_token = token.is_some(),
                Err(McpOAuthError::ReauthRequired { .. })
                    if matches!(auth_mode, McpAuthMode::Interactive) =>
                {
                    force_interactive_reauth = true;
                }
                Err(error) => return Err(mcp_auth_error_to_connection_failed(error)),
            }
        }
        if force_interactive_reauth {
            let Some(resolver) = auth_resolver else {
                return Err(mcp_auth_error_to_connection_failed(
                    McpOAuthError::ReauthRequired {
                        server_name: config.name.clone(),
                    },
                ));
            };
            resolver
                .interactive_login(&target, None)
                .await
                .map_err(|error| mcp_interactive_error(&target, error))?;
            return Self::connect_streamable_http_once(config, headers, url, bearer, None, client)
                .await
                .map_err(StreamableConnectError::into_mcp_error);
        }
        let first = Self::connect_streamable_http_once(
            config,
            headers.clone(),
            url,
            bearer.clone(),
            Some(crate::transport::streamable_http::AuthChallengeRecorder::default()),
            client,
        )
        .await;
        match first {
            Ok(connection) => Ok(connection),
            Err(err) if auth_failure_suggests_oauth(&err) => {
                let challenge = err.auth_challenge();
                let Some(resolver) = auth_resolver else {
                    return Err(StreamableConnectError::into_mcp_error(err));
                };
                match (has_stored_token, auth_mode) {
                    (true, McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::ReauthRequired {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (false, McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::MissingStoredToken {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (_, McpAuthMode::Interactive) => {
                        resolver
                            .interactive_login(&target, challenge.as_deref())
                            .await
                            .map_err(|error| mcp_interactive_error(&target, error))?;
                    }
                }
                // The first attempt consumed its service. A retry selects a
                // fresh owner for the same config before touching transport.
                let retry_client =
                    ClientServiceSelection::select(config, client_factory.as_deref())?;
                Self::connect_streamable_http_once(config, headers, url, bearer, None, retry_client)
                    .await
                    .map_err(StreamableConnectError::into_mcp_error)
            }
            Err(err) => Err(err.into_mcp_error()),
        }
    }

    async fn connect_streamable_http_once(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        bearer: Option<OAuthBearer>,
        recorder: Option<crate::transport::streamable_http::AuthChallengeRecorder>,
        client: ClientServiceSelection,
    ) -> Result<Self, StreamableConnectError> {
        let connection_id =
            crate::McpConnectionId::allocate().map_err(|_| StreamableConnectError {
                reason: "MCP connection generation unavailable".into(),
                auth: Default::default(),
            })?;
        let protected_metadata = ProtectedMetadataState::default();
        let session_expiry = SessionExpiryRecorder::default();
        let recorder = recorder.unwrap_or_default();
        let mut http_client =
            ReqwestStreamableHttpClient::new_with_auth_challenge(headers, recorder.clone())
                .with_protected_metadata(protected_metadata.clone())
                .with_session_expiry(session_expiry.clone());
        let oauth_target = bearer.as_ref().map(|bearer| bearer.target().clone());
        if let Some(bearer) = bearer {
            http_client = http_client.with_oauth_bearer(bearer);
        }
        // Never re-initialize and re-send a request after a session expiry
        // (rmcp defaults to one transparent replay): a call's effect may
        // already have happened, so its outcome is reported as uncertain.
        let transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .reinit_on_expired_session(false);
        let transport = StreamableHttpClientTransport::with_client(http_client, transport_config);
        let service = client
            .serve(transport)
            .await
            .map_err(|error| StreamableConnectError {
                reason: format!("Failed to establish MCP connection: {error}"),
                auth: recorder.take(),
            })?;
        Ok(Self {
            config: config.clone(),
            connection_id,
            protected_metadata,
            service,
            stdio_child: None,
            session_expiry,
            oauth_target,
            standard_tools: Default::default(),
        })
    }

    /// Default connection timeout in seconds.
    pub const DEFAULT_CONNECT_TIMEOUT_SECS: u32 = 10;

    /// Most `tools/list` pages one tool discovery follows. A server that still
    /// reports a next page after this many is refused with
    /// [`McpError::ToolDiscoveryLimitExceeded`] rather than followed forever.
    pub const MAX_TOOL_DISCOVERY_PAGES: usize = 100;

    /// Most tools one tool discovery accepts across all pages. A server that
    /// lists more is refused with [`McpError::ToolDiscoveryLimitExceeded`].
    pub const MAX_DISCOVERED_TOOLS: usize = 10_000;

    /// Connect to an MCP server, perform handshake, and enumerate tools in a
    /// single timeout-bounded operation.
    ///
    /// This is the preferred entry point for all add/reload paths. The timeout
    /// covers connect + initialize handshake + list_tools as a single budget.
    pub async fn connect_and_enumerate(
        config: &McpServerConfig,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_and_enumerate_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services(config, auth_mode, auth_resolver, None).await
    }

    /// Connect and enumerate under the existing timeout, with an exact-attempt
    /// host service factory. Native staged and synchronous router paths use this.
    pub async fn connect_and_enumerate_with_services(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services_and_stdio_profile(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await
    }

    /// Connect and enumerate under the normal deadline with a retained host
    /// launch profile. Required isolation never falls back to trusted execution.
    pub async fn connect_and_enumerate_with_stdio_profile(
        config: &McpServerConfig,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        Self::connect_and_enumerate_with_services_and_stdio_profile(
            config,
            McpAuthMode::Stored,
            None,
            None,
            profile,
        )
        .await
    }

    /// Connect and enumerate with explicit host services and local isolation.
    pub async fn connect_and_enumerate_with_services_and_stdio_profile(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let stdio_custody = matches!(config.transport, McpTransportConfig::Stdio(_))
            .then(StdioChildCustody::default);
        Self::connect_and_enumerate_with_custody(
            config,
            auth_mode,
            auth_resolver,
            client_factory,
            stdio_custody,
            profile,
        )
        .await
    }

    /// [`Self::connect_and_enumerate_with_services`] with a caller-held custody
    /// for a stdio server's process, so the caller can also terminate it while
    /// the attempt is still in flight. On failure or timeout the process has
    /// exited when this returns.
    pub(crate) async fn connect_and_enumerate_with_custody(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
        client_factory: Option<Arc<dyn McpClientServiceFactory>>,
        stdio_custody: Option<StdioChildCustody>,
        profile: &McpStdioLaunchProfile,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let timeout_secs = config
            .connect_timeout_secs
            .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS);
        let mut timeout = Duration::from_secs(timeout_secs as u64);
        if matches!(auth_mode, McpAuthMode::Interactive) {
            timeout += meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT;
        }

        let server_name = config.name.clone();
        let attempt = tokio::time::timeout(timeout, async {
            let conn = Self::connect_with_custody(
                config,
                auth_mode,
                auth_resolver,
                client_factory,
                stdio_custody.clone(),
                profile,
            )
            .await?;
            let tools = conn
                .list_tools(&server_name)
                .await?
                .into_iter()
                .map(Arc::new)
                .collect::<Vec<_>>();
            Ok((conn, tools))
        })
        .await
        .map_err(|_| McpError::ConnectionFailed {
            reason: format!(
                "Timed out connecting to '{}' ({timeout_secs}s)",
                config.name
            ),
        })
        .flatten();
        if attempt.is_err()
            && let Some(custody) = &stdio_custody
        {
            // The attempt future (and any connection it held) is gone; the
            // process it spawned is not until its owner observes the exit.
            custody.terminate().await;
        }
        attempt
    }

    /// Transfer this exact connected owner into the protocol wrapper without
    /// another handshake or host-service selection.
    pub fn into_protocol(self) -> crate::McpProtocol {
        crate::McpProtocol::from_connection(
            self.service,
            self.stdio_child,
            self.config.name,
            self.protected_metadata,
            self.session_expiry,
        )
    }

    /// Get the config used to create this connection.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Physical generation of this exact connection.
    pub fn connection_id(&self) -> crate::McpConnectionId {
        self.connection_id
    }

    pub(crate) fn supports_mcp_apps(&self) -> bool {
        self.service.supports_mcp_apps()
    }

    /// Get server info
    pub fn server_info(&self) -> Option<Arc<rmcp::model::ServerInfo>> {
        self.service.peer_info()
    }

    /// List available tools
    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<ToolDef>, McpError> {
        let standard =
            crate::protocol::list_all_standard_tools_with(&self.service, server_name, |error| {
                self.authorization_required(error)
                    .unwrap_or_else(|| crate::protocol::list_tools_failed(error))
            })
            .await?;
        let projected = standard
            .iter()
            .map(|tool| crate::apps::project_tool(tool, server_name))
            .collect::<Result<Vec<_>, _>>()?;
        *self
            .standard_tools
            .write()
            .map_err(|_| McpError::ProtocolError {
                message: "MCP discovery owner is unavailable".into(),
            })? = standard;
        Ok(projected)
    }

    /// An ambiguous duplicate cannot select UI metadata for a result.
    pub(crate) fn standard_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        let tools = self.standard_tools.read().ok()?;
        let mut matches = tools.iter().filter(|tool| tool.name == name);
        let first = matches.next()?;
        matches.all(|tool| tool == first).then(|| first.clone())
    }

    /// The typed host status for a request that this OAuth connection's
    /// server refused with a `401`, or that was refused before dispatch for
    /// want of a usable credential. Such a request is never replayed.
    fn authorization_required(&self, error: &rmcp::ServiceError) -> Option<McpError> {
        authorization_required(self.oauth_target.as_ref(), error)
    }

    /// Call a tool, returning multimodal content blocks.
    ///
    /// MCP servers can return text, image, and other content types. Text and
    /// image content are captured as their typed [`ContentBlock`] variants;
    /// resource, audio, and resource-link content the agent loop does not model
    /// are preserved verbatim as [`ContentBlock::Structured`] rather than
    /// silently dropped. Optional `structuredContent` is appended as one
    /// additional Structured block containing that JSON value; a text block
    /// whose content parses to the same JSON value is its serialization and
    /// is not repeated.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let result = self.call_tool_result(name, args, None).await?;
        crate::protocol::convert_tool_result(result, name)
    }

    pub(crate) async fn call_tool_result(
        &self,
        name: &str,
        args: &Value,
        metadata: Option<serde_json::Map<String, Value>>,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_result_entering(name, args, metadata, || Ok(()))
            .await
    }

    pub(crate) async fn read_resource_entering(
        &self,
        uri: &str,
        metadata: Option<serde_json::Map<String, Value>>,
        enter: impl FnOnce() -> Result<(), McpError>,
    ) -> Result<rmcp::model::ReadResourceResult, McpError> {
        if self.session_expiry.expired() {
            return Err(session_dead(&self.config.name));
        }
        let dispatch = RequestDispatch::default();
        let mut request =
            rmcp::model::ReadResourceRequest::new(rmcp::model::ReadResourceRequestParams::new(uri));
        request.extensions.insert(dispatch.clone());
        if let Some(metadata) = metadata {
            self.protected_metadata.register(&metadata)?;
            request.extensions.insert(ProtectedMetadata(metadata));
        }
        let request = request.into();
        enter()?;
        let result = self.service.send_request(request).await.map_err(|error| {
            let disposition = dispatch.disposition();
            if matches!(disposition, Some(RequestDisposition::Sent) | None)
                && let Some(refused) = self.authorization_required(&error)
            {
                return refused;
            }
            tool_call_failure(&self.config.name, "resources/read", disposition, &error)
        })?;
        match result {
            ServerResult::ReadResourceResult(result) => {
                if serde_json::to_vec(&result)
                    .map_err(|_| McpError::Serialization("invalid resource response".into()))?
                    .len()
                    > 8 * 1024 * 1024
                {
                    return Err(McpError::ProtocolError {
                        message: "MCP App resource exceeds host limit".into(),
                    });
                }
                Ok(result)
            }
            _ => Err(McpError::ProtocolError {
                message: "unexpected MCP resources/read response".into(),
            }),
        }
    }

    /// Like [`Self::call_tool_result`], running `enter` after every local
    /// request preparation step (parameters, protected metadata registration)
    /// and immediately before the local transport handoff. A refusal from
    /// `enter` sends nothing; nothing that could still fail locally runs after
    /// it. The reviewed boundary is this local handoff: it makes no claim about
    /// when or whether the remote server performs its effect.
    pub(crate) async fn call_tool_result_entering(
        &self,
        name: &str,
        args: &Value,
        metadata: Option<serde_json::Map<String, Value>>,
        enter: impl FnOnce() -> Result<(), McpError>,
    ) -> Result<CallToolResult, McpError> {
        call_tool_on(
            &self.service,
            &self.config.name,
            &self.protected_metadata,
            &self.session_expiry,
            self.oauth_target.as_ref(),
            name,
            args,
            metadata,
            enter,
        )
        .await
    }

    /// Call a tool, returning only the text content as a concatenated string.
    ///
    /// This projects every block returned by [`call_tool`](Self::call_tool)
    /// to text, including derived JSON for Structured blocks and media labels.
    pub async fn call_tool_text(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let blocks = self.call_tool(name, args).await?;
        Ok(meerkat_core::types::text_content(&blocks))
    }

    /// Close the connection. A stdio server's process is killed once its
    /// stdin is closed (its whole process group on Unix) and has exited when
    /// this returns.
    pub async fn close(self) -> Result<(), McpError> {
        close_connected(self.service, self.stdio_child).await
    }
}

/// The one `tools/call` path of a connected service, shared by
/// [`McpConnection`] and the [`crate::McpProtocol`] it converts into.
///
/// A known-dead session refuses the call before it is queued. Otherwise the
/// call carries its own [`RequestDispatch`] witness through the transport,
/// and a failure is typed from that witness alone: sent and then answered
/// `404` for its session is uncertain ([`McpError::SessionExpired`]);
/// redirected, as shown by the request's own returned response, and then
/// failed is uncertain ([`McpError::RedirectedOutcomeUncertain`]); refused
/// at transport entry is affirmatively unsent
/// ([`McpError::ServerUnavailable`]); anything else, including no recorded
/// disposition or a transport failure with no response (which can hide a
/// followed redirect), keeps the ordinary [`McpError::ToolCallFailed`].
/// That ordinary failure does not prove the call had no effect. The session
/// is never re-initialized and nothing is re-sent; a followed same-origin
/// redirect is itself more than one physical request.
// The connection's per-call owners (protected metadata, session expiry and
// OAuth target) are passed separately so both callers share one path.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_tool_on(
    service: &rmcp::service::Peer<rmcp::RoleClient>,
    server: &str,
    protected_metadata: &ProtectedMetadataState,
    session_expiry: &SessionExpiryRecorder,
    oauth_target: Option<&McpServerIdentity>,
    name: &str,
    args: &Value,
    metadata: Option<serde_json::Map<String, Value>>,
    enter: impl FnOnce() -> Result<(), McpError>,
) -> Result<CallToolResult, McpError> {
    let params = match args.as_object().cloned() {
        Some(arguments) => CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
        None => CallToolRequestParams::new(name.to_string()),
    };
    // A dropped session is never resumed under another session.
    if session_expiry.expired() {
        return Err(session_dead(server));
    }
    let dispatch = RequestDispatch::default();
    let mut request = CallToolRequest::new(params);
    request.extensions.insert(dispatch.clone());
    if let Some(metadata) = metadata {
        protected_metadata.register(&metadata)?;
        request.extensions.insert(ProtectedMetadata(metadata));
    }
    let request = request.into();
    // Reviewed boundary: the local transport handoff. The single consuming
    // native entry step runs with no local preparation left that could fail
    // before it.
    enter()?;
    let result = service.send_request(request).await.map_err(|error| {
        let disposition = dispatch.disposition();
        // An OAuth refusal (a 401, or no usable credential before
        // dispatch) is typed unless the request's own disposition already
        // makes its outcome uncertain or proves it unsent.
        if matches!(disposition, Some(RequestDisposition::Sent) | None)
            && let Some(refused) = authorization_required(oauth_target, &error)
        {
            return refused;
        }
        tool_call_failure(server, name, disposition, &error)
    })?;
    match result {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(McpError::ProtocolError {
            message: "unexpected MCP tools/call response".into(),
        }),
    }
}

/// The typed host status for a request that an OAuth connection's server
/// refused with a `401`, or that was refused before dispatch for want of a
/// usable credential. Such a request is never replayed.
fn authorization_required(
    target: Option<&McpServerIdentity>,
    error: &rmcp::ServiceError,
) -> Option<McpError> {
    let target = target?;
    let rmcp::ServiceError::TransportSend(transport) = error else {
        return None;
    };
    matches!(
        transport
            .error
            .downcast_ref::<StreamableHttpError<reqwest::Error>>(),
        Some(StreamableHttpError::AuthRequired(_))
    )
    .then(|| McpError::AuthorizationRequired {
        target: Box::new(target.clone()),
    })
}

fn session_dead(server: &str) -> McpError {
    McpError::ServerUnavailable {
        server: server.to_owned(),
        state: "session expired; reconnect required".into(),
    }
}

/// The typed failure of one `tools/call`, from its own disposition.
fn tool_call_failure(
    server: &str,
    tool: &str,
    disposition: Option<RequestDisposition>,
    error: &dyn std::fmt::Display,
) -> McpError {
    match disposition {
        // Answered 404 for its own session; not re-sent, and the session is
        // not re-initialized.
        Some(RequestDisposition::SentSessionExpired) => McpError::SessionExpired {
            server: server.to_owned(),
            tool: tool.to_owned(),
        },
        // Nothing was sent: the session was already known dead.
        Some(RequestDisposition::RefusedExpired) => session_dead(server),
        // Redirected (followed or stopped) and then failed: some hop may
        // have taken effect.
        Some(RequestDisposition::SentRedirected) => McpError::RedirectedOutcomeUncertain {
            server: server.to_owned(),
            tool: tool.to_owned(),
            reason: error.to_string(),
        },
        Some(RequestDisposition::Sent) | None => McpError::ToolCallFailed {
            tool: tool.to_owned(),
            reason: error.to_string(),
        },
    }
}

/// Cancel the client service (closing a stdio server's stdin), then terminate
/// the server's process and observe its exit. The process is terminated even
/// when the cancel fails.
pub(crate) async fn close_connected(
    service: ConnectedClient,
    stdio_child: Option<StdioChildCustody>,
) -> Result<(), McpError> {
    let cancelled = service.cancel().await;
    if let Some(custody) = stdio_child
        && let Some(Err(error)) = custody.terminate().await
    {
        return Err(McpError::Io(error));
    }
    cancelled.map_err(|e| McpError::ConnectionFailed {
        reason: format!("Failed to close connection: {e:?}"),
    })?;
    Ok(())
}

struct StreamableConnectError {
    reason: String,
    /// Typed auth-failure record captured at the transport boundary: a parsed
    /// `www-authenticate` challenge and/or the `401`/`403` status. Both are
    /// `None` for non-auth failures.
    auth: crate::transport::streamable_http::AuthChallengeState,
}

impl StreamableConnectError {
    fn auth_challenge(&self) -> Option<String> {
        self.auth.challenge().map(String::from)
    }

    fn into_mcp_error(self) -> McpError {
        McpError::ConnectionFailed {
            reason: self.reason,
        }
    }
}

fn auth_failure_suggests_oauth(error: &StreamableConnectError) -> bool {
    error.auth.challenge().is_some() || error.auth.status().is_some()
}

fn mcp_interactive_error(target: &McpServerIdentity, error: McpOAuthError) -> McpError {
    match error {
        McpOAuthError::HumanAuthorizationRequired { .. } => McpError::AuthorizationRequired {
            target: Box::new(target.clone()),
        },
        other => mcp_auth_error_to_connection_failed(other),
    }
}

fn mcp_auth_error_to_connection_failed(error: McpOAuthError) -> McpError {
    match error {
        McpOAuthError::InvalidAccountSelection
        | McpOAuthError::AccountSelectionRequired
        | McpOAuthError::UnsupportedAccountSelection
        | McpOAuthError::Verification(
            meerkat_auth_core::connector_oauth::ConnectorOAuthRefusal::AccountMismatch,
        ) => McpError::OAuthAccountRejected(error),
        other => McpError::ConnectionFailed {
            reason: other.to_string(),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod tests {
    use super::*;
    use crate::protocol::{extract_content_blocks, tool_error_reason};

    /// A Streamable HTTP server whose background GET answer is configurable,
    /// and which counts `tools/call` POSTs.
    mod get_ordering {
        use super::*;
        use std::sync::Arc;

        pub(super) struct Server {
            pub(super) get_status: StatusCode,
            pub(super) tool_posts: AtomicUsize,
            pub(super) gets: tokio::sync::Notify,
        }

        async fn post_handler(
            State(server): State<Arc<Server>>,
            body: String,
        ) -> axum::response::Response {
            let message: Value = serde_json::from_str(&body).unwrap();
            let id = message.get("id").cloned();
            let reply = |result: Value| {
                (
                    StatusCode::OK,
                    [
                        ("content-type", "application/json"),
                        ("mcp-session-id", "s-1"),
                    ],
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                )
                    .into_response()
            };
            match message["method"].as_str().unwrap_or_default() {
                "initialize" => reply(serde_json::json!({
                    "protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "get-ordering", "version": "1"},
                })),
                _ if id.is_none() => StatusCode::ACCEPTED.into_response(),
                "tools/call" => {
                    server.tool_posts.fetch_add(1, Ordering::SeqCst);
                    reply(serde_json::json!({"content": [{"type": "text", "text": "done"}]}))
                }
                _ => reply(serde_json::json!({})),
            }
        }

        async fn get_handler(State(server): State<Arc<Server>>) -> StatusCode {
            server.gets.notify_one();
            server.get_status
        }

        pub(super) async fn start(get_status: StatusCode) -> (Arc<Server>, String) {
            let server = Arc::new(Server {
                get_status,
                tool_posts: AtomicUsize::new(0),
                gets: tokio::sync::Notify::new(),
            });
            let app = Router::new()
                .route(
                    "/mcp",
                    post(post_handler)
                        .get(get_handler)
                        .delete(|| async { StatusCode::OK }),
                )
                .with_state(Arc::clone(&server));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/mcp", listener.local_addr().unwrap());
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            (server, url)
        }
    }

    /// A session the server invalidated before its first GET: the GET 404
    /// expires it, and a later tool call is refused unsent.
    #[tokio::test]
    async fn a_session_invalidated_before_its_first_get_refuses_calls_unsent() {
        let (server, url) = get_ordering::start(StatusCode::NOT_FOUND).await;
        let config = McpServerConfig::streamable_http("get-ordering", url, HashMap::new());
        let connection = McpConnection::connect(&config).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.session_expiry.recorded(),
        )
        .await
        .expect("the background GET 404 is recorded");
        let refused = connection
            .call_tool("effect", &serde_json::json!({}))
            .await
            .expect_err("an expired session refuses the call");
        assert!(
            matches!(refused, McpError::ServerUnavailable { .. }),
            "{refused:?}"
        );
        assert_eq!(server.tool_posts.load(Ordering::SeqCst), 0);
        let _ = connection.close().await;
    }

    /// Paired control: a healthy POST-only server answering the GET with 405
    /// (the spec's "no SSE stream") stays usable.
    #[tokio::test]
    async fn a_post_only_server_answering_get_with_405_stays_usable() {
        let (server, url) = get_ordering::start(StatusCode::METHOD_NOT_ALLOWED).await;
        let config = McpServerConfig::streamable_http("get-ordering", url, HashMap::new());
        let connection = McpConnection::connect(&config).await.unwrap();
        // Wait for the client's own classification of the GET, not for the
        // server's answer: only then is "no expiry" a settled fact.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.session_expiry.stream_unsupported_classified(),
        )
        .await
        .expect("the background GET 405 is classified");
        connection
            .call_tool("effect", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(server.tool_posts.load(Ordering::SeqCst), 1);
        assert!(!connection.session_expiry.expired());
        let _ = connection.close().await;
    }

    /// A call's failure is typed from its own disposition only: there is no
    /// connection-wide input, so a queued call that failed before sending is
    /// never labelled uncertain because another call expired the session.
    #[test]
    fn call_failures_are_typed_from_their_own_request_disposition() {
        let failure = |disposition| tool_call_failure("srv", "effect", disposition, &"cause");
        assert!(matches!(
            failure(Some(RequestDisposition::SentSessionExpired)),
            McpError::SessionExpired { ref server, ref tool } if server == "srv" && tool == "effect"
        ));
        assert!(matches!(
            failure(Some(RequestDisposition::RefusedExpired)),
            McpError::ServerUnavailable { .. }
        ));
        // No final disposition recorded (a pre-send failure such as the frame
        // bound, or an unfinished send): ordinary failure, never typed as
        // uncertain from connection state.
        assert!(matches!(failure(None), McpError::ToolCallFailed { .. }));
        assert!(matches!(
            failure(Some(RequestDisposition::Sent)),
            McpError::ToolCallFailed { .. }
        ));
        assert!(matches!(
            failure(Some(RequestDisposition::SentRedirected)),
            McpError::RedirectedOutcomeUncertain { ref tool, .. } if tool == "effect"
        ));
    }
    use async_trait::async_trait;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use rmcp::model::Content;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn confined_stdio_spawn_preserves_exact_refusals_and_original_io() {
        use meerkat_core::confinement::ConfinementRefusal;

        for refusal in [
            ConfinementRefusal::InvalidRequirement,
            ConfinementRefusal::InvalidLaunch,
            ConfinementRefusal::UnsupportedRequirement,
            ConfinementRefusal::BackendUnavailable,
            ConfinementRefusal::PreparationFailed,
        ] {
            assert!(matches!(
                confined_stdio_spawn_error(std::io::Error::other(refusal)),
                McpError::Confinement(actual) if actual == refusal,
            ));
        }
        for error in [
            std::io::Error::from_raw_os_error(nix::libc::EPIPE),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "ordinary IO"),
            std::io::Error::other(ConfinementRefusal::BackendUnavailable.to_string()),
        ] {
            let kind = error.kind();
            let raw = error.raw_os_error();
            let message = error.to_string();
            let retained = match confined_stdio_spawn_error(error) {
                McpError::Io(error) => Some(error),
                _ => None,
            }
            .expect("ordinary IO must remain IO, including refusal text lookalikes");
            assert_eq!(retained.kind(), kind);
            assert_eq!(retained.raw_os_error(), raw);
            assert_eq!(retained.to_string(), message);
        }
    }

    /// Test that content block extraction works correctly with multiple text items
    #[test]
    fn test_extract_content_blocks_multiple_text() {
        let contents = vec![
            Content::text("Line 1"),
            Content::text("Line 2"),
            Content::text("Line 3"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Line 1".to_string()
            }
        );
        assert_eq!(
            blocks[1],
            ContentBlock::Text {
                text: "Line 2".to_string()
            }
        );
        assert_eq!(
            blocks[2],
            ContentBlock::Text {
                text: "Line 3".to_string()
            }
        );
    }

    /// Test that content block extraction works with single text item
    #[test]
    fn test_extract_content_blocks_single_text() {
        let contents = vec![Content::text("Only line")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Only line".to_string()
            }
        );
    }

    /// Test that content block extraction returns empty vec for empty input
    #[test]
    fn test_extract_content_blocks_empty() {
        let contents: Vec<Content> = Vec::new();
        let blocks = extract_content_blocks(contents);
        assert!(blocks.is_empty());
    }

    /// Test that image content from MCP is captured as ContentBlock::Image
    #[test]
    fn mcp_call_tool_captures_image() {
        let contents = vec![Content::image("aW1hZ2VkYXRh", "image/png")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aW1hZ2VkYXRh".into(),
            }
        );
    }

    /// Test that mixed text and image content is preserved in order
    #[test]
    fn mcp_call_tool_mixed_text_and_image() {
        let contents = vec![
            Content::text("description of the image"),
            Content::image("cG5nZGF0YQ==", "image/png"),
            Content::text("additional context"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert!(
            matches!(&blocks[0], ContentBlock::Text { text } if text == "description of the image")
        );
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { media_type, data, .. }
                if media_type == "image/png"
                    && matches!(data, meerkat_core::ImageData::Inline { data } if data == "cG5nZGF0YQ==")
        ));
        assert!(matches!(&blocks[2], ContentBlock::Text { text } if text == "additional context"));
    }

    /// Test that text-only responses still work as before
    #[test]
    fn mcp_call_tool_text_only_compat() {
        let contents = vec![Content::text("Hello, MCP!")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Hello, MCP!".to_string()
            }
        );

        // Verify text_content projection matches legacy behavior
        let text = meerkat_core::types::text_content(&blocks);
        assert_eq!(text, "Hello, MCP!");
    }

    /// Regression: an unmodeled content variant (embedded resource) must be
    /// preserved as `Structured` JSON, never silently dropped (no `_ => None`
    /// launder). The data the server returned survives the conversion.
    #[test]
    fn mcp_call_tool_preserves_unmodeled_variant() {
        let contents = vec![
            Content::text("before"),
            Content::embedded_text("file:///doc.txt", "resource body"),
        ];
        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 2, "no content variant may be dropped");
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "before"));
        match &blocks[1] {
            ContentBlock::Structured { data } => {
                let rendered = data.get();
                assert!(
                    rendered.contains("resource body"),
                    "structured passthrough must preserve the resource body verbatim, got: {rendered}"
                );
            }
            other => panic!("expected Structured passthrough, got {other:?}"),
        }
    }

    /// Regression: when a tool errors, the typed reason must carry the
    /// server-authored content detail, not a fixed `"Tool returned error"`
    /// string that launders the cause away.
    #[test]
    fn mcp_tool_error_reason_carries_server_detail() {
        let content = vec![Content::text("disk quota exceeded")];
        let reason = tool_error_reason(&extract_content_blocks(content));
        assert_eq!(reason, "disk quota exceeded");
    }

    /// An errored result with no content still produces a non-empty, honest
    /// reason rather than an empty string.
    #[test]
    fn mcp_tool_error_reason_handles_empty_content() {
        let reason = tool_error_reason(&[]);
        assert_eq!(reason, "tool returned error with no content");
    }

    /// RCT: Verify MCP initialize handshake works
    #[tokio::test]
    async fn test_mcp_initialize_handshake() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        // Connect (includes initialize handshake)
        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect to MCP server");

        // Verify we got server info back
        let info = conn.server_info();
        assert!(info.is_some(), "Server should return info after initialize");
        let info = info.unwrap();
        assert_eq!(info.server_info.name, "mcp-test-server");

        // Clean up
        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/list schema parsing
    #[tokio::test]
    async fn test_mcp_tools_list_schema_parse() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // List tools
        let tools = conn
            .list_tools("test-server")
            .await
            .expect("Failed to list tools");

        // Verify we got the expected tools
        assert!(!tools.is_empty(), "Should have at least one tool");

        // Find the echo tool
        let echo_tool = tools.iter().find(|t| t.name == "echo");
        assert!(echo_tool.is_some(), "Should have echo tool");
        let echo_tool = echo_tool.unwrap();
        assert!(
            !echo_tool.description.is_empty(),
            "Echo tool should have description"
        );

        // Verify schema has expected structure
        let schema = &echo_tool.input_schema;
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "Schema should be object type"
        );
        assert!(
            schema.get("properties").is_some(),
            "Schema should have properties"
        );

        // Find the add tool and verify its schema
        let add_tool = tools.iter().find(|t| t.name == "add");
        assert!(add_tool.is_some(), "Should have add tool");
        let add_schema = &add_tool.unwrap().input_schema;
        let props = add_schema.get("properties").unwrap();
        assert!(
            props.get("a").is_some(),
            "Add tool should have 'a' property"
        );
        assert!(
            props.get("b").is_some(),
            "Add tool should have 'b' property"
        );

        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/call round-trip (returns Vec<ContentBlock>)
    #[tokio::test]
    async fn test_mcp_tools_call_round_trip() {
        let server_path = mcp_test_server::fixture_binary();

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // Test echo tool -- returns Vec<ContentBlock>
        let blocks = conn
            .call_tool("echo", &serde_json::json!({"message": "Hello, MCP!"}))
            .await
            .expect("Echo call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "Hello, MCP!");

        // Test add tool
        let blocks = conn
            .call_tool("add", &serde_json::json!({"a": 5, "b": 3}))
            .await
            .expect("Add call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "8");

        // Test fail tool returns error
        let result = conn
            .call_tool("fail", &serde_json::json!({"message": "Expected error"}))
            .await;
        assert!(result.is_err(), "Fail tool should return error");

        conn.close().await.expect("Failed to close connection");
    }

    pub(crate) struct HttpMcpTestState {
        accepted_token: &'static str,
        seen_authorizations: Mutex<Vec<Option<String>>>,
    }

    pub(crate) async fn spawn_http_mcp_server(
        accepted_token: &'static str,
    ) -> (String, Arc<HttpMcpTestState>) {
        let state = Arc::new(HttpMcpTestState {
            accepted_token,
            seen_authorizations: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp", post(http_mcp_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, state)
    }

    async fn http_mcp_handler(
        State(state): State<Arc<HttpMcpTestState>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> impl IntoResponse {
        let authorization = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        state
            .seen_authorizations
            .lock()
            .unwrap()
            .push(authorization.clone());

        if authorization.as_deref() != Some(&format!("Bearer {}", state.accepted_token)) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    http::header::WWW_AUTHENTICATE,
                    "Bearer resource_metadata=\"/.well-known/oauth-protected-resource/mcp\"",
                )],
            )
                .into_response();
        }

        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let response = match request.get("method").and_then(|method| method.as_str()) {
            Some("initialize") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "http-mcp-test-server",
                        "version": "0.1.0"
                    }
                }
            }),
            Some("tools/list") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo input",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": { "type": "string" }
                            }
                        }
                    }]
                }
            }),
            method => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported method {method:?}")
                }
            }),
        };
        (StatusCode::OK, Json(response)).into_response()
    }

    /// Models the credential owner: a completed interactive login commits
    /// its token, which every later per-request read returns.
    pub(crate) struct FakeMcpAuthResolver {
        stored_token: Mutex<Option<String>>,
        stored_reauth_required: AtomicBool,
        interactive_token: String,
        interactive_delay: Option<Duration>,
        interactive_calls: AtomicUsize,
        challenges: Mutex<Vec<Option<String>>>,
        human_authorization_required: bool,
    }

    impl FakeMcpAuthResolver {
        pub(crate) fn new(stored_token: Option<&str>, interactive_token: &str) -> Self {
            Self {
                stored_token: Mutex::new(stored_token.map(ToOwned::to_owned)),
                stored_reauth_required: AtomicBool::new(false),
                interactive_token: interactive_token.to_owned(),
                interactive_delay: None,
                interactive_calls: AtomicUsize::new(0),
                challenges: Mutex::new(Vec::new()),
                human_authorization_required: false,
            }
        }

        fn with_interactive_delay(mut self, delay: Duration) -> Self {
            self.interactive_delay = Some(delay);
            self
        }

        fn with_stored_reauth_required(self) -> Self {
            self.stored_reauth_required.store(true, Ordering::SeqCst);
            self
        }

        pub(crate) fn with_human_authorization_required(mut self) -> Self {
            self.human_authorization_required = true;
            self
        }
    }

    #[async_trait]
    impl McpAuthResolver for FakeMcpAuthResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpServerIdentity,
        ) -> Result<Option<String>, McpOAuthError> {
            if self.stored_reauth_required.load(Ordering::SeqCst) {
                return Err(McpOAuthError::ReauthRequired {
                    server_name: target.server_name().to_string(),
                });
            }
            Ok(self.stored_token.lock().unwrap().clone())
        }

        async fn interactive_login(
            &self,
            _target: &McpServerIdentity,
            www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            if let Some(delay) = self.interactive_delay {
                tokio::time::sleep(delay).await;
            }
            self.interactive_calls.fetch_add(1, Ordering::SeqCst);
            self.challenges
                .lock()
                .unwrap()
                .push(www_authenticate.map(ToOwned::to_owned));
            if self.human_authorization_required {
                return Err(McpOAuthError::HumanAuthorizationRequired {
                    server_name: _target.server_name().to_owned(),
                });
            }
            *self.stored_token.lock().unwrap() = Some(self.interactive_token.clone());
            self.stored_reauth_required.store(false, Ordering::SeqCst);
            Ok(self.interactive_token.clone())
        }
    }

    #[tokio::test]
    async fn mcp_oauth_human_authorization_is_typed_with_target_and_carries_no_secret() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver =
            Arc::new(FakeMcpAuthResolver::new(None, "unused").with_human_authorization_required());

        let error = match McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        {
            Ok(_) => panic!("unauthorized server must not connect"),
            Err(error) => error,
        };
        let McpError::AuthorizationRequired { target } = &error else {
            panic!("expected typed authorization-required, got {error:?}");
        };
        assert_eq!(**target, McpServerIdentity::from_config(&config).unwrap());
        let rendered = format!("{error} {error:?}");
        for secret in ["authorize", "state=", "code=", "resource_metadata"] {
            assert!(!rendered.contains(secret), "{secret:?} leaked: {rendered}");
        }
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .all(Option::is_none),
            "no credential may be sent without completed host authorization"
        );
    }

    #[tokio::test]
    async fn mcp_oauth_stored_token_is_injected_for_streamable_http() {
        let (url, state) = spawn_http_mcp_server("stored-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(Some("stored-token"), "unused"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Stored,
            Some(resolver),
        )
        .await
        .expect("stored token should connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer stored-token")),
            "streamable HTTP requests should include the stored bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_retries_streamable_http_connect() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(None, "interactive-token"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive login should retry and connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "interactive login should receive the WWW-Authenticate challenge"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter().any(Option::is_none),
            "first connect attempt should be unauthenticated"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "retry should include the interactive bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_gets_browser_timeout_budget() {
        let (url, _state) = spawn_http_mcp_server("interactive-token").await;
        let mut config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        config.connect_timeout_secs = Some(1);
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token")
                .with_interactive_delay(Duration::from_millis(1200)),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver),
        )
        .await
        .expect("interactive login should not be cut off by the normal connect timeout");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_stored_reauth_required() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token").with_stored_reauth_required(),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when stored credentials require it");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should connect with the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_server_rejected_stored_token() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(
            Some("stale-stored-token"),
            "interactive-token",
        ));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when the server rejects a stored token");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "server-rejected stored tokens should pass the auth challenge into reauth"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer stale-stored-token")),
            "first connect attempt should use the stored token"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should use the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[cfg(unix)]
    async fn custody_test_child() -> (
        StdioChildCustody,
        (tokio::process::ChildStdout, tokio::process::ChildStdin),
    ) {
        let custody = StdioChildCustody::default();
        let pipes = custody
            .spawn(
                &meerkat_core::mcp_config::McpStdioConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), "exec sleep 60".to_string()],
                    env: HashMap::new(),
                },
                &McpStdioLaunchProfile::trusted_host(),
            )
            .await
            .expect("spawn real stdio child");
        (custody, pipes)
    }

    #[cfg(unix)]
    pub(crate) fn assert_child_reaped(pid: u32) {
        assert_eq!(
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("valid child pid")),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
            ),
            Err(nix::errno::Errno::ECHILD),
            "custody must already have reaped its direct child"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_cancelled_termination_can_be_reaped_by_the_retained_owner() {
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let first = tokio::spawn({
            let custody = custody.clone();
            async move { custody.terminate().await }
        });
        waiting.await.expect("termination reached its reap await");
        assert_eq!(
            custody
                .direct_kill_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the first waiter requests termination before reaping"
        );
        first.abort();
        assert!(
            first
                .await
                .expect_err("first waiter was aborted")
                .is_cancelled()
        );
        drop(resume);

        custody
            .terminate()
            .await
            .expect("cancellation must leave the child in retained custody")
            .expect("retained owner reaps the child");
        assert_child_reaped(pid);
        assert_eq!(
            custody
                .direct_kill_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "reaping retries must not signal the process id again"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_concurrent_termination_waits_for_the_reap_owner() {
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let first = tokio::spawn({
            let custody = custody.clone();
            async move { custody.terminate().await }
        });
        waiting.await.expect("termination reached its reap await");
        let mut second = Box::pin(custody.terminate());
        let second_waited = futures::poll!(second.as_mut()).is_pending();

        resume.send(()).expect("release the first reap");
        first
            .await
            .expect("first waiter joins")
            .expect("first waiter owns the child")
            .expect("first waiter reaps the child");
        if second_waited {
            assert!(second.await.is_none(), "the first waiter completed custody");
        }
        assert_child_reaped(pid);
        assert!(
            second_waited,
            "a concurrent close must wait for the owned reap"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_custody_close_reports_a_reap_failure() {
        use rmcp::ServiceExt as _;

        let (client_io, server_io) = tokio::io::duplex(8192);
        let (client, server) = tokio::join!(
            ClientServiceSelection::Default.serve(client_io),
            mcp_test_server::FormTestServer::default().serve(server_io),
        );
        let client = client.expect("initialize client");
        let server = server.expect("initialize server");
        let (custody, _pipes) = custody_test_child().await;
        let pid = custody.spawned_pid().await;
        let (waiting, resume) = custody.pause_next_reap();
        let close = tokio::spawn(close_connected(client, Some(custody)));
        waiting.await.expect("close reached its reap await");

        // Fault injection: another process owner consumes the status before
        // Child::wait can reap it. The close must preserve that OS error.
        tokio::task::spawn_blocking(move || {
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("valid child pid")),
                None,
            )
        })
        .await
        .expect("external waiter joins")
        .expect("external waiter consumes the exit status");
        resume.send(()).expect("release the failed reap");
        let result = close.await.expect("close task joins");
        server.cancel().await.expect("server task joins");
        assert!(
            matches!(result, Err(McpError::Io(ref error)) if error.raw_os_error() == Some(nix::errno::Errno::ECHILD as i32)),
            "close must return the typed reap error, got {result:?}"
        );
    }

    /// `close` owns the stdio server's exit: the server is killed with its
    /// whole process group right after its stdin closes, even when it keeps
    /// running past EOF, and has exited when `close` returns. (Fails-old:
    /// rmcp gave the server up to 3 s after EOF, then killed only the direct
    /// child, so a wrapped server's grandchild outlived `close`.)
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn close_returns_after_the_stdio_servers_process_group_exits() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let mut report = PidReport::new("close-group");
        let config = McpServerConfig::stdio(
            "eof-ignoring",
            "/bin/sh",
            sh_mcp_server_args(Some(report.path())),
            HashMap::new(),
        );
        let (conn, tools) = McpConnection::connect_and_enumerate(&config)
            .await
            .expect("fixture server completes the handshake");
        assert!(tools.is_empty());
        let pids = report.pids().await;
        let (wrapper, server) = (pids[0], pids[1]);

        conn.close().await.expect("close");

        assert_child_reaped(wrapper);
        assert!(
            process_exited(server),
            "stdio server's grandchild {server} outlived close"
        );
    }

    /// A failed handshake terminates the spawned server before the error is
    /// returned. (Fails-old: the transport drop only scheduled the kill.)
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_handshake_returns_after_the_stdio_server_exits() {
        // Closes its stdout (the handshake fails on EOF) but keeps running.
        let config = McpServerConfig::stdio(
            "closes-stdout",
            "/bin/sh",
            vec!["-c".to_string(), "exec >&-; exec sleep 60".to_string()],
            HashMap::new(),
        );
        let custody = StdioChildCustody::default();

        let result = McpConnection::connect_and_enumerate_with_custody(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            Some(custody.clone()),
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await;

        assert!(result.is_err(), "a server without stdout cannot connect");
        let pid = custody.spawned_pid().await;
        assert_child_reaped(pid);
        assert!(
            custody.terminate().await.is_none(),
            "the failed attempt already terminated and reaped the server"
        );
    }

    // --- Per-request OAuth bearer on established connections -----------------

    /// An OAuth-protected MCP server whose accepted bearer the test rotates,
    /// recording the `Authorization` header of every `tools/call`.
    struct RotatingMcpState {
        accepted: Mutex<String>,
        tool_call_authorizations: Mutex<Vec<Option<String>>>,
    }

    impl RotatingMcpState {
        fn accept(&self, token: &str) {
            *self.accepted.lock().unwrap() = token.to_owned();
        }

        fn tool_calls(&self) -> Vec<Option<String>> {
            self.tool_call_authorizations.lock().unwrap().clone()
        }
    }

    async fn spawn_rotating_mcp_server(accepted: &str) -> (String, Arc<RotatingMcpState>) {
        let state = Arc::new(RotatingMcpState {
            accepted: Mutex::new(accepted.to_owned()),
            tool_call_authorizations: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp", post(rotating_mcp_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, state)
    }

    async fn rotating_mcp_handler(
        State(state): State<Arc<RotatingMcpState>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> axum::response::Response {
        let authorization = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let method = request.get("method").and_then(Value::as_str);
        if method == Some("tools/call") {
            state
                .tool_call_authorizations
                .lock()
                .unwrap()
                .push(authorization.clone());
        }
        let accepted = format!("Bearer {}", state.accepted.lock().unwrap());
        if authorization.as_deref() != Some(accepted.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    http::header::WWW_AUTHENTICATE,
                    "Bearer error=\"invalid_token\", resource_metadata=\"/.well-known/oauth-protected-resource/mcp\"",
                )],
            )
                .into_response();
        }
        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match method {
            Some("initialize") => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rotating-mcp-test-server", "version": "0.1.0" }
            }),
            Some("tools/list") => serde_json::json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo input",
                    "inputSchema": { "type": "object", "properties": {} }
                }]
            }),
            Some("tools/call") => serde_json::json!({
                "content": [{ "type": "text", "text": "echoed" }]
            }),
            other => {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("unsupported {other:?}") }
                    })),
                )
                    .into_response();
            }
        };
        (
            StatusCode::OK,
            Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        )
            .into_response()
    }

    /// The native credential owner as seen through the resolver: the stored
    /// credential changes when the owner refreshes or loses it.
    struct StoredCredentialResolver {
        stored: Mutex<Result<Option<String>, ()>>,
    }

    impl StoredCredentialResolver {
        fn new(token: &str) -> Self {
            Self {
                stored: Mutex::new(Ok(Some(token.to_owned()))),
            }
        }

        fn store(&self, token: &str) {
            *self.stored.lock().unwrap() = Ok(Some(token.to_owned()));
        }

        fn require_reauth(&self) {
            *self.stored.lock().unwrap() = Err(());
        }
    }

    #[async_trait]
    impl McpAuthResolver for StoredCredentialResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpServerIdentity,
        ) -> Result<Option<String>, McpOAuthError> {
            self.stored
                .lock()
                .unwrap()
                .clone()
                .map_err(|()| McpOAuthError::ReauthRequired {
                    server_name: target.server_name().to_owned(),
                })
        }

        async fn interactive_login(
            &self,
            target: &McpServerIdentity,
            _www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            Err(McpOAuthError::HumanAuthorizationRequired {
                server_name: target.server_name().to_owned(),
            })
        }
    }

    async fn connect_rotating(
        token: &str,
    ) -> (
        McpServerConfig,
        McpConnection,
        Arc<RotatingMcpState>,
        Arc<StoredCredentialResolver>,
    ) {
        let (url, state) = spawn_rotating_mcp_server(token).await;
        let config = McpServerConfig::streamable_http("rotating", url, HashMap::new());
        let resolver = Arc::new(StoredCredentialResolver::new(token));
        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("the stored credential connects");
        assert!(tools.iter().any(|tool| tool.name == "echo"));
        (config, conn, state, resolver)
    }

    /// G5: an established connection reads the bearer from the credential
    /// owner on every request, so a refreshed credential is used without a
    /// reconnect. (Fails-old: the connect-time token stayed on the transport.)
    #[tokio::test]
    async fn oauth_connection_resolves_the_bearer_per_request() {
        let (_config, conn, state, resolver) = connect_rotating("token-a").await;
        state.accept("token-b");
        resolver.store("token-b");

        let blocks = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect("the refreshed credential is used");

        assert_eq!(meerkat_core::types::text_content(&blocks), "echoed");
        assert_eq!(
            state.tool_calls(),
            vec![Some("Bearer token-b".to_owned())],
            "the call carries the owner's current credential"
        );
        conn.close().await.expect("close");
    }

    /// G5: a 401 on an established connection is the typed host status, and
    /// the refused call is not replayed. (Fails-old: an untyped tool failure.)
    #[tokio::test]
    async fn oauth_call_refused_with_401_is_typed_and_not_replayed() {
        let (config, conn, state, _resolver) = connect_rotating("token-a").await;
        state.accept("token-after-revocation");

        let error = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect_err("the provider revoked the credential");

        let McpError::AuthorizationRequired { target } = &error else {
            panic!("expected typed authorization-required, got {error:?}");
        };
        assert_eq!(**target, McpServerIdentity::from_config(&config).unwrap());
        assert_eq!(
            state.tool_calls(),
            vec![Some("Bearer token-a".to_owned())],
            "exactly one dispatch: the refused call is never replayed"
        );
        conn.close().await.expect("close");
    }

    /// G5: when the owner has no usable credential (reauthentication
    /// required), the call fails typed before anything is sent.
    /// (Fails-old: the stale connect-time token was still sent.)
    #[tokio::test]
    async fn oauth_call_without_a_usable_credential_fails_typed_before_dispatch() {
        let (_config, conn, state, resolver) = connect_rotating("token-a").await;
        resolver.require_reauth();

        let error = conn
            .call_tool("echo", &serde_json::json!({}))
            .await
            .expect_err("no usable credential");

        assert!(
            matches!(error, McpError::AuthorizationRequired { .. }),
            "got {error:?}"
        );
        assert!(
            state.tool_calls().is_empty(),
            "nothing is dispatched without a usable credential"
        );
        conn.close().await.expect("close");
    }
}

#[cfg(test)]
#[path = "pagination_tests.rs"]
mod pagination_tests;

#[cfg(test)]
#[path = "structured_result_tests.rs"]
mod structured_result_tests;

#[cfg(test)]
#[path = "process_confinement_tests.rs"]
mod process_confinement_tests;
