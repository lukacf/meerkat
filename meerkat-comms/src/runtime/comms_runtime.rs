//! CommsRuntime - Full lifecycle manager for agent-to-agent communication.

use super::comms_config::ResolvedCommsConfig;
use crate::agent::types::{CommsContent, CommsMessage};
use crate::{
    InboxSender, InprocRegistry, Keypair, PubKey, Router, TrustedPeers, handle_connection,
};
use async_trait::async_trait;
use futures::Stream;
use futures::task::{Context, Poll};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, EventStream, InputStreamMode, PeerDirectoryEntry, PeerDirectorySource, PeerName,
    SendAndStreamError, SendError, SendReceipt, StreamError, StreamScope,
};
use meerkat_core::config::PlainEventSource;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::types::{DrainedMessage, drain_inbox_item};

/// Reservation lifecycle state machine.
///
/// State transitions:
/// - Reserved → Attached (stream consumer attaches)
/// - Reserved → Expired (TTL elapsed without attach)
/// - Attached → Completed (terminal event received)
/// - Attached → ClosedEarly (consumer drops stream before terminal)
/// - Any terminal state → cannot re-attach
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReservationState {
    Reserved,
    Attached,
    Completed,
    Expired,
    ClosedEarly,
}

impl ReservationState {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Expired | Self::ClosedEarly
        )
    }
}

/// Default reservation TTL (time from Reserved to Expired if not attached).
const RESERVATION_TTL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
struct StreamRegistryEntry {
    state: ReservationState,
    _sender: mpsc::Sender<meerkat_core::AgentEvent>,
    receiver: Option<Receiver<meerkat_core::AgentEvent>>,
    created_at: std::time::Instant,
}

impl StreamRegistryEntry {
    fn reserved(
        sender: mpsc::Sender<meerkat_core::AgentEvent>,
        receiver: Receiver<meerkat_core::AgentEvent>,
    ) -> Self {
        Self {
            state: ReservationState::Reserved,
            _sender: sender,
            receiver: Some(receiver),
            created_at: std::time::Instant::now(),
        }
    }

    /// CAS-style state transition. Returns true if transition succeeded.
    fn transition(&mut self, from: ReservationState, to: ReservationState) -> bool {
        if self.state == from {
            self.state = to;
            true
        } else {
            false
        }
    }
}

type InteractionStreamRegistry = Arc<Mutex<HashMap<Uuid, StreamRegistryEntry>>>;

struct InteractionStream {
    id: Uuid,
    receiver: Option<Receiver<meerkat_core::AgentEvent>>,
    registry: InteractionStreamRegistry,
}

impl InteractionStream {
    fn finish(&mut self) {
        if let Some(mut receiver) = self.receiver.take() {
            receiver.close();
        }
        let mut registry = self.registry.lock();
        if let Some(entry) = registry.get_mut(&self.id) {
            // CAS: only transition to ClosedEarly if still Attached.
            // If already Completed (terminal event won the race), leave it.
            if entry.transition(ReservationState::Attached, ReservationState::ClosedEarly) {
                tracing::debug!(interaction_id = %self.id, "stream closed early by consumer");
            }
            // Clean up terminal entries.
            if entry.state.is_terminal() {
                registry.remove(&self.id);
            }
        }
    }
}

impl Stream for InteractionStream {
    type Item = meerkat_core::AgentEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.receiver.as_mut() {
            Some(receiver) => match Pin::new(receiver).poll_recv(cx) {
                Poll::Ready(None) => {
                    this.finish();
                    Poll::Ready(None)
                }
                other => other,
            },
            None => Poll::Ready(None),
        }
    }
}

impl Drop for InteractionStream {
    fn drop(&mut self) {
        self.finish();
    }
}

fn is_dismiss(msg: &CommsMessage) -> bool {
    matches!(&msg.content, CommsContent::Message { body } if body.trim().eq_ignore_ascii_case("DISMISS"))
}

#[async_trait]
impl CoreCommsRuntime for CommsRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        let mut inbox = self.inbox.lock().await;
        let items = inbox.try_drain();
        let trusted = self.trusted_peers.read().await;

        let drained: Vec<DrainedMessage> = items
            .iter()
            .filter_map(|item| drain_inbox_item(item, &trusted))
            .collect();

        // Check for DISMISS in authenticated messages
        for msg in &drained {
            if let DrainedMessage::Authenticated(m) = msg {
                if is_dismiss(m) {
                    self.dismiss_flag.store(true, Ordering::SeqCst);
                }
            }
        }

        drained
            .iter()
            .filter(|m| {
                // Filter out DISMISS messages from output
                !matches!(m, DrainedMessage::Authenticated(m) if is_dismiss(m))
            })
            .map(|m| match m {
                DrainedMessage::Authenticated(msg) => msg.to_user_message_text(),
                DrainedMessage::Plain(msg) => msg.to_user_message_text(),
            })
            .collect()
    }
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }
    fn dismiss_received(&self) -> bool {
        self.dismiss_flag.swap(false, Ordering::SeqCst)
    }

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        Some(self.event_injector())
    }

    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        match scope {
            StreamScope::Session(session_id) => {
                Err(StreamError::NotFound(format!("session {session_id}")))
            }
            StreamScope::Interaction(interaction_id) => {
                let id = interaction_id.0;
                let mut registry = self.interaction_stream_registry.lock();
                let entry = registry
                    .get_mut(&id)
                    .ok_or(StreamError::NotReserved(interaction_id))?;

                match entry.state {
                    ReservationState::Reserved => {
                        // Check TTL
                        if entry.created_at.elapsed() > RESERVATION_TTL {
                            entry.state = ReservationState::Expired;
                            registry.remove(&id);
                            return Err(StreamError::Timeout(format!(
                                "reservation expired for interaction {}",
                                interaction_id.0
                            )));
                        }
                        let receiver = entry.receiver.take().ok_or_else(|| {
                            StreamError::Internal(
                                "interaction stream receiver missing".to_string(),
                            )
                        })?;
                        entry.state = ReservationState::Attached;
                        Ok(Box::pin(InteractionStream {
                            id,
                            receiver: Some(receiver),
                            registry: self.interaction_stream_registry.clone(),
                        }))
                    }
                    ReservationState::Attached => {
                        Err(StreamError::AlreadyAttached(interaction_id))
                    }
                    state if state.is_terminal() => {
                        Err(StreamError::NotReserved(interaction_id))
                    }
                    _ => Err(StreamError::Internal(format!(
                        "unexpected reservation state for {}",
                        interaction_id.0
                    ))),
                }
            }
        }
    }

    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        match cmd {
            CommsCommand::Input {
                body,
                source,
                stream,
                allow_self_session,
                session_id: _,
            } => {
                // Self-input guard: when allow_self_session is false, this runtime's
                // inbox is the target, which is a self-loop. Reject unless explicitly
                // opted in. MCP never exposes this flag.
                if !allow_self_session {
                    return Err(SendError::Validation(
                        "self-session input rejected: set allow_self_session=true to override"
                            .into(),
                    ));
                }
                match stream {
                    InputStreamMode::None => {
                        let injector =
                            CoreCommsRuntime::event_injector(self).ok_or_else(|| {
                                SendError::Unsupported("event injector unavailable".into())
                            })?;
                        injector
                            .inject(body, PlainEventSource::from(source))
                            .map_err(map_event_injector_error)?;
                        Ok(SendReceipt::InputAccepted {
                            interaction_id: meerkat_core::InteractionId(Uuid::new_v4()),
                            stream_reserved: false,
                        })
                    }
                    InputStreamMode::ReserveInteraction => {
                        let interaction_id = Uuid::new_v4();
                        self.register_interaction_stream(interaction_id, body, source)?;
                        Ok(SendReceipt::InputAccepted {
                            interaction_id: meerkat_core::InteractionId(interaction_id),
                            stream_reserved: true,
                        })
                    }
                }
            }
            CommsCommand::PeerMessage { to, body } => self
                .send_peer_command(&to.0, crate::types::MessageKind::Message { body })
                .await
                .map(|_| SendReceipt::PeerMessageSent {
                    envelope_id: Uuid::new_v4(),
                    acked: false,
                }),
            CommsCommand::PeerRequest {
                to,
                intent,
                params,
                stream,
            } => {
                let interaction_id = Uuid::new_v4();
                let stream_reserved = stream == InputStreamMode::ReserveInteraction;

                if stream_reserved {
                    // Register reservation BEFORE sending so stream() can attach.
                    let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(4096);
                    self.subscriber_registry
                        .lock()
                        .insert(interaction_id, sender.clone());
                    self.interaction_stream_registry.lock().insert(
                        interaction_id,
                        StreamRegistryEntry::reserved(sender, receiver),
                    );
                }

                if let Err(e) = self
                    .send_peer_command(
                        &to.0,
                        crate::types::MessageKind::Request { intent, params },
                    )
                    .await
                {
                    if stream_reserved {
                        // Clean up reservation on send failure.
                        self.interaction_stream_registry
                            .lock()
                            .remove(&interaction_id);
                        self.subscriber_registry.lock().remove(&interaction_id);
                    }
                    return Err(e);
                }

                Ok(SendReceipt::PeerRequestSent {
                    envelope_id: Uuid::new_v4(),
                    interaction_id: meerkat_core::InteractionId(interaction_id),
                    stream_reserved,
                })
            }
            CommsCommand::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
            } => {
                let status = match status {
                    meerkat_core::ResponseStatus::Accepted => crate::Status::Accepted,
                    meerkat_core::ResponseStatus::Completed => crate::Status::Completed,
                    meerkat_core::ResponseStatus::Failed => crate::Status::Failed,
                };

                self.send_peer_command(
                    &to.0,
                    crate::types::MessageKind::Response {
                        in_reply_to: in_reply_to.0,
                        status,
                        result,
                    },
                )
                .await
                .map(|_| SendReceipt::PeerResponseSent {
                    envelope_id: Uuid::new_v4(),
                    in_reply_to,
                })
            }
        }
    }

    async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError> {
        match cmd {
            CommsCommand::Input {
                body,
                source,
                stream: InputStreamMode::ReserveInteraction,
                allow_self_session,
                session_id: _,
            } => {
                if !allow_self_session {
                    return Err(SendAndStreamError::Send(SendError::Validation(
                        "self-session input rejected: set allow_self_session=true to override"
                            .into(),
                    )));
                }
                let interaction_id = Uuid::new_v4();
                self.register_interaction_stream(interaction_id, body, source)?;
                let receipt = SendReceipt::InputAccepted {
                    interaction_id: meerkat_core::InteractionId(interaction_id),
                    stream_reserved: true,
                };
                let stream = self
                    .stream(StreamScope::Interaction(meerkat_core::InteractionId(
                        interaction_id,
                    )))
                    .map_err(|error| SendAndStreamError::StreamAttach {
                        receipt: receipt.clone(),
                        error,
                    })?;
                Ok((receipt, stream))
            }
            CommsCommand::PeerRequest {
                to,
                intent,
                params,
                stream: InputStreamMode::ReserveInteraction,
            } => {
                // Reserve a local interaction stream, then send the peer request.
                let interaction_id = Uuid::new_v4();
                let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(4096);
                self.subscriber_registry
                    .lock()
                    .insert(interaction_id, sender.clone());
                self.interaction_stream_registry.lock().insert(
                    interaction_id,
                    StreamRegistryEntry::reserved(sender, receiver),
                );

                if let Err(e) = self
                    .send_peer_command(
                        &to.0,
                        crate::types::MessageKind::Request { intent, params },
                    )
                    .await
                {
                    // Clean up reservation on send failure.
                    self.interaction_stream_registry
                        .lock()
                        .remove(&interaction_id);
                    self.subscriber_registry.lock().remove(&interaction_id);
                    return Err(SendAndStreamError::Send(e));
                }

                let receipt = SendReceipt::PeerRequestSent {
                    envelope_id: Uuid::new_v4(),
                    interaction_id: meerkat_core::InteractionId(interaction_id),
                    stream_reserved: true,
                };
                let event_stream = self
                    .stream(StreamScope::Interaction(meerkat_core::InteractionId(
                        interaction_id,
                    )))
                    .map_err(|error| SendAndStreamError::StreamAttach {
                        receipt: receipt.clone(),
                        error,
                    })?;
                Ok((receipt, event_stream))
            }
            other => {
                let receipt = self.send(other).await?;
                Err(SendAndStreamError::StreamAttach {
                    receipt,
                    error: StreamError::NotFound("command is not streamable".to_string()),
                })
            }
        }
    }

    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        self.resolve_peer_directory().await
    }

    async fn drain_inbox_interactions(&self) -> Vec<meerkat_core::InboxInteraction> {
        let mut inbox = self.inbox.lock().await;
        let items = inbox.try_drain();
        let trusted = self.trusted_peers.read().await;

        let drained: Vec<DrainedMessage> = items
            .iter()
            .filter_map(|item| drain_inbox_item(item, &trusted))
            .collect();

        // Check for DISMISS in authenticated messages
        for msg in &drained {
            if let DrainedMessage::Authenticated(m) = msg {
                if is_dismiss(m) {
                    self.dismiss_flag.store(true, Ordering::SeqCst);
                }
            }
        }

        drained
            .into_iter()
            .filter(|m| !matches!(m, DrainedMessage::Authenticated(m) if is_dismiss(m)))
            .map(|m| match m {
                DrainedMessage::Authenticated(msg) => {
                    let rendered_text = msg.to_user_message_text();
                    let content = match msg.content {
                        CommsContent::Message { body } => {
                            meerkat_core::InteractionContent::Message { body }
                        }
                        CommsContent::Request {
                            request_id: _,
                            intent,
                            params,
                        } => meerkat_core::InteractionContent::Request {
                            intent: intent.to_string(),
                            params,
                        },
                        CommsContent::Response {
                            in_reply_to,
                            status,
                            result,
                        } => {
                            let status = match status {
                                crate::agent::types::CommsStatus::Accepted => {
                                    meerkat_core::ResponseStatus::Accepted
                                }
                                crate::agent::types::CommsStatus::Completed => {
                                    meerkat_core::ResponseStatus::Completed
                                }
                                crate::agent::types::CommsStatus::Failed => {
                                    meerkat_core::ResponseStatus::Failed
                                }
                            };
                            meerkat_core::InteractionContent::Response {
                                in_reply_to: meerkat_core::InteractionId(in_reply_to),
                                status,
                                result,
                            }
                        }
                    };
                    meerkat_core::InboxInteraction {
                        id: meerkat_core::InteractionId(msg.envelope_id),
                        from: msg.from_peer,
                        content,
                        rendered_text,
                    }
                }
                DrainedMessage::Plain(msg) => {
                    let rendered_text = msg.to_user_message_text();
                    meerkat_core::InboxInteraction {
                        id: meerkat_core::InteractionId(
                            msg.interaction_id.unwrap_or_else(uuid::Uuid::new_v4),
                        ),
                        from: format!("event:{}", msg.source),
                        content: meerkat_core::InteractionContent::Message { body: msg.body },
                        rendered_text,
                    }
                }
            })
            .collect()
    }

    fn interaction_subscriber(
        &self,
        id: &meerkat_core::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>> {
        let sender = self.subscriber_registry.lock().remove(&id.0);
        sender.as_ref()?;

        let mut registry = self.interaction_stream_registry.lock();
        match registry.get(&id.0) {
            Some(entry) => {
                // If not yet attached to a stream consumer, clean up.
                if entry.state == ReservationState::Reserved {
                    registry.remove(&id.0);
                }
                sender
            }
            None => sender,
        }
    }

    fn mark_interaction_complete(&self, id: &meerkat_core::InteractionId) {
        self.mark_interaction_complete(id.0);
    }
}

fn map_event_injector_error(error: meerkat_core::event_injector::EventInjectorError) -> SendError {
    match error {
        meerkat_core::event_injector::EventInjectorError::Full => {
            SendError::Validation("input queue full".into())
        }
        meerkat_core::event_injector::EventInjectorError::Closed => SendError::InputClosed,
    }
}

fn map_router_send_error(err: crate::router::SendError) -> SendError {
    match err {
        crate::router::SendError::PeerNotFound(peer) => SendError::PeerNotFound(peer),
        crate::router::SendError::PeerOffline => SendError::PeerOffline,
        crate::router::SendError::Transport(_) | crate::router::SendError::Io(_) => {
            SendError::Internal(err.to_string())
        }
    }
}

fn map_inproc_send_error(err: crate::inproc::InprocSendError) -> SendError {
    match err {
        crate::inproc::InprocSendError::PeerNotFound(peer) => SendError::PeerNotFound(peer),
        crate::inproc::InprocSendError::InboxClosed | crate::inproc::InprocSendError::InboxFull => {
            SendError::PeerOffline
        }
    }
}

#[derive(Debug, Error)]
pub enum CommsRuntimeError {
    #[error("Identity error: {0}")]
    IdentityError(String),
    #[error("Trust load error: {0}")]
    TrustLoadError(String),
    #[error("Listener error: {0}")]
    ListenerError(#[from] std::io::Error),
    #[error("Listeners already started")]
    AlreadyStarted,
    #[error("Unsafe binding: {0}")]
    UnsafeBinding(String),
}

pub struct CommsRuntime {
    public_key: PubKey,
    router: Arc<Router>,
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    inbox: Arc<AsyncMutex<crate::Inbox>>,
    inbox_notify: Arc<tokio::sync::Notify>,
    config: ResolvedCommsConfig,
    listener_handles: Vec<ListenerHandle>,
    listeners_started: bool,
    keypair: Arc<Keypair>,
    dismiss_flag: AtomicBool,
    subscriber_registry: crate::event_injector::SubscriberRegistry,
    interaction_stream_registry: InteractionStreamRegistry,
}

impl CommsRuntime {
    pub async fn new(config: ResolvedCommsConfig) -> Result<Self, CommsRuntimeError> {
        // Always load keypair and trusted peers — outbound routing needs them
        // regardless of auth mode. The auth mode only affects the external
        // event listener, not the signed agent-to-agent path.
        let keypair = Keypair::load_or_generate(&config.identity_dir)
            .await
            .map_err(|e| CommsRuntimeError::IdentityError(e.to_string()))?;
        let trusted_peers = TrustedPeers::load_or_default(&config.trusted_peers_path)
            .map_err(|e| CommsRuntimeError::TrustLoadError(e.to_string()))?;
        let public_key = keypair.public_key();
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let (inbox, inbox_sender) = crate::Inbox::new();
        let inbox_notify = inbox.notify();
        let router = Router::with_shared_peers(
            keypair.clone(),
            trusted_peers.clone(),
            config.comms_config.clone(),
            inbox_sender.clone(),
        );
        let runtime = Self {
            public_key,
            router: Arc::new(router),
            trusted_peers,
            inbox: Arc::new(AsyncMutex::new(inbox)),
            inbox_notify,
            config: config.clone(),
            listener_handles: Vec::new(),
            listeners_started: false,
            keypair: Arc::new(keypair),
            dismiss_flag: AtomicBool::new(false),
            subscriber_registry: crate::event_injector::new_subscriber_registry(),
            interaction_stream_registry: Arc::new(Mutex::new(HashMap::new())),
        };
        InprocRegistry::global().register(config.name, runtime.public_key, inbox_sender);
        Ok(runtime)
    }

    pub fn inproc_only(name: &str) -> Result<Self, CommsRuntimeError> {
        let keypair = Keypair::generate();
        let public_key = keypair.public_key();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let (inbox, inbox_sender) = crate::Inbox::new();
        let inbox_notify = inbox.notify();
        let comms_config = crate::CommsConfig::default();
        let config = ResolvedCommsConfig {
            enabled: true,
            name: name.to_string(),
            identity_dir: std::path::PathBuf::new(),
            trusted_peers_path: std::path::PathBuf::new(),
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            comms_config: comms_config.clone(),
            auth: meerkat_core::CommsAuthMode::Open,
            allow_external_unauthenticated: false,
        };
        let router = Router::with_shared_peers(
            keypair.clone(),
            trusted_peers.clone(),
            comms_config,
            inbox_sender.clone(),
        );
        let runtime = Self {
            public_key,
            router: Arc::new(router),
            trusted_peers,
            inbox: Arc::new(AsyncMutex::new(inbox)),
            inbox_notify,
            config,
            listener_handles: Vec::new(),
            listeners_started: false,
            keypair: Arc::new(keypair),
            dismiss_flag: AtomicBool::new(false),
            subscriber_registry: crate::event_injector::new_subscriber_registry(),
            interaction_stream_registry: Arc::new(Mutex::new(HashMap::new())),
        };
        InprocRegistry::global().register(name, runtime.public_key, inbox_sender);
        Ok(runtime)
    }

    pub async fn start_listeners(&mut self) -> Result<(), CommsRuntimeError> {
        if self.listeners_started {
            return Err(CommsRuntimeError::AlreadyStarted);
        }

        let inbox_sender = self.router.inbox_sender().clone();
        let max_line_length = self.config.comms_config.max_message_bytes as usize;

        // === Signed (Ed25519) listeners — ALWAYS run for agent-to-agent comms ===
        #[cfg(unix)]
        if let Some(ref path) = self.config.listen_uds {
            let handle = spawn_uds_listener(
                path,
                self.keypair.clone(),
                self.trusted_peers.clone(),
                inbox_sender.clone(),
            )
            .await?;
            self.listener_handles.push(handle);
        }

        if let Some(ref addr) = self.config.listen_tcp {
            let handle = spawn_tcp_listener(
                &addr.to_string(),
                self.keypair.clone(),
                self.trusted_peers.clone(),
                inbox_sender.clone(),
            )
            .await?;
            self.listener_handles.push(handle);
        }

        // === Plain event listeners — run ADDITIONALLY when auth=Open ===
        if self.config.auth == meerkat_core::CommsAuthMode::Open {
            // Enforce loopback-only on plain TCP listener unless explicitly overridden
            if let Some(addr) = &self.config.event_listen_tcp {
                if !addr.ip().is_loopback() && !self.config.allow_external_unauthenticated {
                    return Err(CommsRuntimeError::UnsafeBinding(
                        "Plain event listener on non-loopback address is a prompt injection \
                         vector; set allow_external_unauthenticated=true to override"
                            .to_string(),
                    ));
                }
            }

            if let Some(ref addr) = self.config.event_listen_tcp {
                let handle = spawn_plain_tcp_listener(
                    &addr.to_string(),
                    inbox_sender.clone(),
                    max_line_length,
                )
                .await?;
                self.listener_handles.push(handle);
            }

            #[cfg(unix)]
            if let Some(ref path) = self.config.event_listen_uds {
                let handle =
                    spawn_plain_uds_listener(path, inbox_sender.clone(), max_line_length).await?;
                self.listener_handles.push(handle);
            }
        }

        self.listeners_started = true;
        Ok(())
    }

    pub fn public_key(&self) -> PubKey {
        self.public_key
    }
    pub fn router(&self) -> &Router {
        &self.router
    }
    /// Canonical peer resolver: trusted first, inproc second.
    ///
    /// Both `send()` and `peers()` use this same resolution policy.
    async fn resolve_peer_directory(&self) -> Vec<PeerDirectoryEntry> {
        let sendable_kinds = vec![
            "peer_message".to_string(),
            "peer_request".to_string(),
            "peer_response".to_string(),
        ];
        let inproc_peers = InprocRegistry::global().peers();
        let inproc_names: std::collections::HashSet<String> =
            inproc_peers.iter().map(|(n, _)| n.clone()).collect();

        let mut peers: std::collections::HashMap<String, PeerDirectoryEntry> =
            std::collections::HashMap::new();

        {
            let trusted = self.trusted_peers.read().await;
            for peer in &trusted.peers {
                if peer.name == self.config.name {
                    continue;
                }
                let source = if inproc_names.contains(&peer.name) {
                    PeerDirectorySource::TrustedAndInproc
                } else {
                    PeerDirectorySource::Trusted
                };
                peers.insert(
                    peer.name.clone(),
                    PeerDirectoryEntry {
                        name: PeerName(peer.name.clone()),
                        peer_id: peer.pubkey.to_peer_id(),
                        address: peer.addr.clone(),
                        source,
                        sendable_kinds: sendable_kinds.clone(),
                        capabilities: serde_json::json!({}),
                    },
                );
            }
        }

        for (name, pubkey) in &inproc_peers {
            if *name == self.config.name {
                continue;
            }
            peers
                .entry(name.clone())
                .or_insert_with(|| PeerDirectoryEntry {
                    name: PeerName(name.clone()),
                    peer_id: pubkey.to_peer_id(),
                    address: format!("inproc://{}", name),
                    source: PeerDirectorySource::Inproc,
                    sendable_kinds: sendable_kinds.clone(),
                    capabilities: serde_json::json!({}),
                });
        }

        peers.into_values().collect()
    }

    /// Canonical send resolver: trusted first, inproc fallback.
    async fn send_peer_command(
        &self,
        peer_name: &str,
        kind: crate::types::MessageKind,
    ) -> Result<(), SendError> {
        let primary = self.router.send(peer_name, kind.clone()).await;
        match primary {
            Ok(()) => {
                tracing::debug!(peer = peer_name, route = "trusted", "peer command sent");
                Ok(())
            }
            Err(crate::router::SendError::PeerNotFound(_)) => {
                tracing::debug!(
                    peer = peer_name,
                    route = "inproc",
                    "trusted peer not found, trying inproc fallback"
                );
                InprocRegistry::global()
                    .send(&self.keypair, peer_name, kind)
                    .map_err(map_inproc_send_error)
            }
            Err(err) => Err(map_router_send_error(err)),
        }
    }

    /// Mark an interaction stream as completed (terminal event received).
    ///
    /// Uses CAS to ensure exactly-once cleanup: if the consumer already closed
    /// the stream (ClosedEarly), this is a no-op.
    pub fn mark_interaction_complete(&self, interaction_id: Uuid) {
        let mut registry = self.interaction_stream_registry.lock();
        let mut should_remove = false;
        if let Some(entry) = registry.get_mut(&interaction_id) {
            if entry.transition(ReservationState::Attached, ReservationState::Completed) {
                tracing::debug!(
                    interaction_id = %interaction_id,
                    "interaction stream completed by terminal event"
                );
                should_remove = true;
            }
        }
        if should_remove {
            self.subscriber_registry.lock().remove(&interaction_id);
            registry.remove(&interaction_id);
        }
    }

    /// Reap expired reservations that were never attached within the TTL.
    pub fn reap_expired_reservations(&self) {
        let mut registry = self.interaction_stream_registry.lock();
        registry.retain(|id, entry| {
            if entry.state == ReservationState::Reserved
                && entry.created_at.elapsed() > RESERVATION_TTL
            {
                tracing::debug!(interaction_id = %id, "reservation expired (TTL)");
                entry.state = ReservationState::Expired;
                false
            } else {
                true
            }
        });
    }

    pub fn router_arc(&self) -> Arc<Router> {
        self.router.clone()
    }
    pub fn trusted_peers_shared(&self) -> Arc<RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }
    pub fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }

    /// Get a subscribable event injector for this runtime's inbox.
    ///
    /// Surfaces use this to push external events and optionally subscribe to
    /// interaction-scoped streaming responses.
    pub fn event_injector(&self) -> Arc<dyn meerkat_core::SubscribableInjector> {
        Arc::new(crate::CommsEventInjector::new(
            self.router.inbox_sender().clone(),
            self.subscriber_registry.clone(),
        ))
    }

    fn register_interaction_stream(
        &self,
        interaction_id: Uuid,
        body: String,
        source: meerkat_core::InputSource,
    ) -> Result<(), SendError> {
        let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(4096);
        self.subscriber_registry
            .lock()
            .insert(interaction_id, sender.clone());
        self.interaction_stream_registry.lock().insert(
            interaction_id,
            StreamRegistryEntry::reserved(sender, receiver),
        );

        if let Err(error) = self
            .router
            .inbox_sender()
            .send(crate::types::InboxItem::PlainEvent {
                body,
                source: PlainEventSource::from(source),
                interaction_id: Some(interaction_id),
            })
        {
            self.interaction_stream_registry
                .lock()
                .remove(&interaction_id);
            self.subscriber_registry.lock().remove(&interaction_id);
            return Err(match error {
                crate::inbox::InboxError::Full => SendError::Validation("input queue full".into()),
                crate::inbox::InboxError::Closed => SendError::InputClosed,
            });
        }

        Ok(())
    }

    pub async fn drain_messages(&self) -> Vec<CommsMessage> {
        let mut inbox = self.inbox.lock().await;
        let items = inbox.try_drain();
        let trusted = self.trusted_peers.read().await;
        items
            .into_iter()
            .filter_map(|item| CommsMessage::from_inbox_item(&item, &trusted))
            .collect()
    }

    pub async fn recv_message(&self) -> Option<CommsMessage> {
        loop {
            {
                let mut inbox = self.inbox.lock().await;
                let items = inbox.try_drain();
                if !items.is_empty() {
                    let trusted = self.trusted_peers.read().await;
                    if let Some(msg) = items
                        .into_iter()
                        .find_map(|item| CommsMessage::from_inbox_item(&item, &trusted))
                    {
                        return Some(msg);
                    }
                }
            }
            self.inbox_notify.notified().await;
        }
    }

    pub fn shutdown(&mut self) {
        for handle in self.listener_handles.drain(..) {
            handle.abort();
        }
        self.listeners_started = false;
    }
}

impl Drop for CommsRuntime {
    fn drop(&mut self) {
        self.shutdown();
        InprocRegistry::global().unregister(&self.public_key);
    }
}

pub struct ListenerHandle {
    handle: JoinHandle<()>,
}
impl ListenerHandle {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

#[cfg(unix)]
async fn spawn_uds_listener(
    path: &Path,
    keypair: Arc<Keypair>,
    trusted: Arc<RwLock<TrustedPeers>>,
    inbox_sender: InboxSender,
) -> Result<ListenerHandle, std::io::Error> {
    use tokio::net::UnixListener;
    let path = path.to_path_buf();
    if let Err(err) = tokio::fs::remove_file(&path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(&path)?;
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (keypair, trusted, inbox_sender) =
                (keypair.clone(), trusted.clone(), inbox_sender.clone());
            tokio::spawn(async move {
                let trusted_snapshot = trusted.read().await.clone();
                let _ = handle_connection(stream, &keypair, &trusted_snapshot, &inbox_sender).await;
            });
        }
    });
    Ok(ListenerHandle { handle })
}

async fn spawn_tcp_listener(
    addr: &str,
    keypair: Arc<Keypair>,
    trusted: Arc<RwLock<TrustedPeers>>,
    inbox_sender: InboxSender,
) -> Result<ListenerHandle, std::io::Error> {
    let listener = TcpListener::bind(addr).await?;
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (keypair, trusted, inbox_sender) =
                (keypair.clone(), trusted.clone(), inbox_sender.clone());
            tokio::spawn(async move {
                let trusted_snapshot = trusted.read().await.clone();
                let _ = handle_connection(stream, &keypair, &trusted_snapshot, &inbox_sender).await;
            });
        }
    });
    Ok(ListenerHandle { handle })
}

/// Max concurrent connections for the plain listener (DoS protection).
const PLAIN_LISTENER_MAX_CONCURRENT: usize = 64;

async fn spawn_plain_tcp_listener(
    addr: &str,
    inbox_sender: InboxSender,
    max_line_length: usize,
) -> Result<ListenerHandle, std::io::Error> {
    let listener = TcpListener::bind(addr).await?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PLAIN_LISTENER_MAX_CONCURRENT));
    let handle = tokio::spawn(async move {
        while let Ok((stream, _peer)) = listener.accept().await {
            let sender = inbox_sender.clone();
            let sem = semaphore.clone();
            tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return, // Semaphore closed
                };
                crate::plain_listener::handle_plain_connection(
                    stream,
                    sender,
                    max_line_length,
                    meerkat_core::PlainEventSource::Tcp,
                )
                .await;
            });
        }
    });
    Ok(ListenerHandle { handle })
}

#[cfg(unix)]
async fn spawn_plain_uds_listener(
    path: &std::path::Path,
    inbox_sender: InboxSender,
    max_line_length: usize,
) -> Result<ListenerHandle, std::io::Error> {
    use tokio::net::UnixListener;
    let path = path.to_path_buf();
    if let Err(err) = tokio::fs::remove_file(&path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(&path)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PLAIN_LISTENER_MAX_CONCURRENT));
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let sender = inbox_sender.clone();
            let sem = semaphore.clone();
            tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                crate::plain_listener::handle_plain_connection(
                    stream,
                    sender,
                    max_line_length,
                    meerkat_core::PlainEventSource::Uds,
                )
                .await;
            });
        }
    });
    Ok(ListenerHandle { handle })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::event_injector::CommsEventInjector;
    use crate::identity::Signature;
    use crate::types::{Envelope, InboxItem, MessageKind, Status};
    use futures::StreamExt;
    use meerkat_core::SubscribableInjector;
    use meerkat_core::{
        SendError,
        comms::{
            InputSource, InputStreamMode, PeerDirectorySource, PeerName, StreamError, StreamScope,
        },
        interaction::InteractionId,
        types::SessionId,
    };
    use uuid::Uuid;
    use tokio::time::{timeout, Duration};

    fn clear_inproc_registry() {
        InprocRegistry::global().clear();
    }

    fn test_runtime_config(name: &str, tmp: &tempfile::TempDir) -> ResolvedCommsConfig {
        ResolvedCommsConfig {
            enabled: true,
            name: name.to_string(),
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            allow_external_unauthenticated: false,
        }
    }

    fn signed_envelope(from: &Keypair, to: PubKey, kind: MessageKind) -> Envelope {
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: from.public_key(),
            to,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(from);
        envelope
    }

    /// Regression: auth=Open must always load keypair and trusted peers from disk
    /// so that outbound routing still works.
    #[tokio::test]
    async fn test_auth_open_loads_keypair_and_peers() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-agent".to_string(),
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            allow_external_unauthenticated: false,
        };

        let runtime = CommsRuntime::new(config).await.unwrap();
        assert_ne!(runtime.public_key().as_bytes(), &[0u8; 32]);
    }

    /// Regression: signed listener must ALWAYS start, regardless of auth mode.
    #[tokio::test]
    #[ignore] // integration-real: binds TCP port
    async fn test_signed_listener_starts_in_open_mode() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-signed".to_string(),
            listen_uds: None,
            listen_tcp: Some("127.0.0.1:0".parse().unwrap()),
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            allow_external_unauthenticated: false,
        };

        let mut runtime = CommsRuntime::new(config).await.unwrap();
        runtime.start_listeners().await.unwrap();
        assert!(runtime.listeners_started);
        runtime.shutdown();
    }

    /// Regression: non-loopback plain event listener must be rejected.
    #[tokio::test]
    async fn test_plain_listener_rejects_non_loopback() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-reject".to_string(),
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: Some("0.0.0.0:4201".parse().unwrap()),
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            allow_external_unauthenticated: false,
        };

        let mut runtime = CommsRuntime::new(config).await.unwrap();
        let result = runtime.start_listeners().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("prompt injection"));
    }

    #[tokio::test]
    async fn test_drain_inbox_interactions_converts_all_authenticated_content_variants() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("variants", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();

        let sender = Keypair::generate();
        {
            let mut trusted = runtime.trusted_peers.write().await;
            trusted.upsert(crate::TrustedPeer {
                name: "sender".to_string(),
                pubkey: sender.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            });
        }

        let request_id = Uuid::new_v4();
        let reply_to = Uuid::new_v4();

        let msg = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let req = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 19}),
            },
        );
        let mut req = req;
        req.id = request_id;
        req.sign(&sender);
        let resp = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Response {
                in_reply_to: reply_to,
                status: Status::Completed,
                result: serde_json::json!({"ok": true}),
            },
        );

        runtime
            .router
            .inbox_sender()
            .send(InboxItem::External { envelope: msg })
            .unwrap();
        runtime
            .router
            .inbox_sender()
            .send(InboxItem::External { envelope: req })
            .unwrap();
        runtime
            .router
            .inbox_sender()
            .send(InboxItem::External { envelope: resp })
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 3);

        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Message { body } if body == "hello"
            )
        }));
        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Request { intent, params }
                    if intent == "review" && params["pr"] == 19
            )
        }));
        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Response { in_reply_to, status, result }
                    if in_reply_to.0 == reply_to
                        && *status == meerkat_core::ResponseStatus::Completed
                        && result["ok"] == true
            )
        }));
    }

    #[tokio::test]
    async fn test_subscription_correlation_e2e_one_shot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("subscription", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();

        let injector = CommsEventInjector::new(
            runtime.router.inbox_sender().clone(),
            runtime.subscriber_registry.clone(),
        );
        let sub = injector
            .inject_with_subscription("tracked".to_string(), meerkat_core::PlainEventSource::Rpc)
            .unwrap();
        let tracked_id = sub.id;

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id, tracked_id);

        let first = CoreCommsRuntime::interaction_subscriber(&runtime, &tracked_id);
        assert!(first.is_some(), "subscriber should be found");
        let second = CoreCommsRuntime::interaction_subscriber(&runtime, &tracked_id);
        assert!(second.is_none(), "subscriber should be one-shot");
    }

    #[tokio::test]
    async fn test_plain_event_interaction_id_is_preserved_in_drain_inbox_interactions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("plain-id", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();

        let interaction_id = Uuid::new_v4();
        runtime
            .router
            .inbox_sender()
            .send(InboxItem::PlainEvent {
                body: "evt".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                interaction_id: Some(interaction_id),
            })
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id.0, interaction_id);
    }

    #[test]
    fn test_interaction_subscriber_correlation_miss_returns_none() {
        let runtime = CommsRuntime::inproc_only("corr-miss").unwrap();
        let random = meerkat_core::InteractionId(Uuid::new_v4());
        let sender = CoreCommsRuntime::interaction_subscriber(&runtime, &random);
        assert!(sender.is_none());
    }

    #[tokio::test]
    async fn test_core_send_input_no_reservation() {
        clear_inproc_registry();
        let runtime = CommsRuntime::inproc_only("input-no-reservation").unwrap();

        let cmd = CommsCommand::Input {
            session_id: SessionId::new(),
            body: "standalone test input".to_string(),
            source: InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: true,
        };

        let receipt = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(receipt.is_ok(), "send should succeed: {receipt:?}");

        match receipt.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id: _,
                stream_reserved,
            } => assert!(!stream_reserved),
            other => panic!("Expected InputAccepted, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body } if body == "standalone test input"
        ));
    }

    #[tokio::test]
    async fn test_core_send_input_reserves_stream() {
        clear_inproc_registry();
        let runtime = CommsRuntime::inproc_only("input-with-reserve").unwrap();

        let cmd = CommsCommand::Input {
            session_id: SessionId::new(),
            body: "streaming input".to_string(),
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let (interaction_id, reserved) = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => (interaction_id, stream_reserved),
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        assert!(reserved);

        let sender = runtime
            .take_interaction_stream_sender(&interaction_id)
            .expect("reserved interaction should be registered");
        drop(sender);

        assert!(
            runtime
                .take_interaction_stream_sender(&interaction_id)
                .is_none(),
            "interaction registration must be one-shot"
        );
    }

    #[tokio::test]
    async fn test_core_stream_attachment_duplicate_attach_fails() {
        clear_inproc_registry();
        let runtime = CommsRuntime::inproc_only("stream-duplicate-attach").unwrap();

        let cmd = CommsCommand::Input {
            session_id: SessionId::new(),
            body: "duplicate stream test".to_string(),
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let interaction_id = match CoreCommsRuntime::send(&runtime, cmd).await.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let stream =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id.clone()))
                .expect("first attach should succeed");

        let dup =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id.clone()));
        assert!(matches!(dup, Err(StreamError::AlreadyAttached(_))));

        drop(stream);

        let after_drop =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id));
        assert!(matches!(after_drop, Err(StreamError::NotReserved(_))));
    }

    #[tokio::test]
    async fn test_core_stream_not_reserved_before_send() {
        clear_inproc_registry();
        let runtime = CommsRuntime::inproc_only("stream-before-send").unwrap();
        let random = InteractionId(Uuid::new_v4());

        let missing = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(random));
        assert!(matches!(missing, Err(StreamError::NotReserved(_))));

        let cmd = CommsCommand::Input {
            session_id: SessionId::new(),
            body: "send before stream attach".to_string(),
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let interaction_id = match CoreCommsRuntime::send(&runtime, cmd).await.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let _stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id))
            .expect("should attach after send");
    }

    #[tokio::test]
    async fn test_core_send_and_stream_input_returns_stream_and_receipt() {
        clear_inproc_registry();
        let runtime = CommsRuntime::inproc_only("send-and-stream").unwrap();

        let cmd = CommsCommand::Input {
            session_id: SessionId::new(),
            body: "stream-first".to_string(),
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let (receipt, _stream) = CoreCommsRuntime::send_and_stream(&runtime, cmd)
            .await
            .unwrap();

        let interaction_id = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let dup =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id.clone()));
        assert!(matches!(dup, Err(StreamError::AlreadyAttached(_))));

        assert!(
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id)).is_err()
        );
    }

    #[tokio::test]
    async fn test_core_send_peer_message_unknown_peer_fails() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("sender-{suffix}")).unwrap();

        let cmd = CommsCommand::PeerMessage {
            to: PeerName("missing-peer".to_string()),
            body: "hello".to_string(),
        };

        let result = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(matches!(
            result,
            Err(SendError::PeerNotFound(peer)) if peer == "missing-peer"
        ));
    }

    #[tokio::test]
    async fn test_core_send_peer_message_success_and_drain() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-{suffix}");
        let receiver_name = format!("receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();

        {
            let mut trusted = sender.trusted_peers.write().await;
            trusted.upsert(crate::TrustedPeer {
                name: receiver_name.clone(),
                pubkey: receiver.public_key(),
                addr: format!("inproc://{receiver_name}"),
            });
        }

        {
            let mut trusted = receiver.trusted_peers.write().await;
            trusted.upsert(crate::TrustedPeer {
                name: sender_name.clone(),
                pubkey: sender.public_key(),
                addr: format!("inproc://{sender_name}"),
            });
        }

        let cmd = CommsCommand::PeerMessage {
            to: PeerName(receiver_name),
            body: "greeting".to_string(),
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        match receipt {
            Ok(SendReceipt::PeerMessageSent {
                envelope_id: _,
                acked,
            }) => assert!(!acked),
            other => panic!("Expected peer message receipt, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body } if body == "greeting"
        ));
    }

    #[tokio::test]
    async fn test_core_send_peer_message_success_via_inproc_without_trusted_entry() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-ipc-{suffix}");
        let receiver_name = format!("receiver-ipc-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();
        {
            let mut trusted = receiver.trusted_peers.write().await;
            trusted.upsert(crate::TrustedPeer {
                name: sender_name.clone(),
                pubkey: sender.public_key(),
                addr: format!("inproc://{sender_name}"),
            });
        }

        let cmd = CommsCommand::PeerMessage {
            to: PeerName(receiver_name),
            body: "inproc-only hello".to_string(),
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        match receipt {
            Ok(SendReceipt::PeerMessageSent { .. }) => {}
            other => panic!("Expected peer message receipt, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body } if body == "inproc-only hello"
        ));
    }

    #[tokio::test]
    async fn test_core_peers_includes_inproc_and_trusted_without_self() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("trusted-mixed-{suffix}");
        let runtime_name = format!("runtime-mixed-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();

        {
            let mut trusted = runtime.trusted_peers.write().await;
            trusted.upsert(crate::TrustedPeer {
                name: peer_name.clone(),
                pubkey: peer.public_key(),
                addr: format!("inproc://{peer_name}"),
            });
        }

        let peers = CoreCommsRuntime::peers(&runtime).await;
        let names: Vec<_> = peers.iter().map(|entry| entry.name.0.clone()).collect();
        assert!(names.iter().any(|name| name == &peer_name));
        assert!(!names.iter().any(|name| name == &runtime_name));

        let matched: Vec<_> = peers
            .iter()
            .filter(|entry| entry.name.0 == peer_name)
            .collect();
        assert_eq!(matched.len(), 1);

        let peer = matched[0];
        // Peer exists in both registries → TrustedAndInproc.
        // In parallel test execution another test may clear the global inproc
        // registry, so Trusted alone is also accepted.
        assert!(
            peer.source == PeerDirectorySource::TrustedAndInproc
                || peer.source == PeerDirectorySource::Trusted,
            "expected TrustedAndInproc or Trusted, got {:?}",
            peer.source
        );
        assert_eq!(peer.address, format!("inproc://{peer_name}"));
        assert_eq!(peer.sendable_kinds.len(), 3);
        assert!(peer.sendable_kinds.contains(&"peer_message".to_string()));
        assert!(peer.sendable_kinds.contains(&"peer_request".to_string()));
        assert!(peer.sendable_kinds.contains(&"peer_response".to_string()));
    }

    /// Truthfulness invariant: every peer/kind in peers() must be sendable.
    #[tokio::test]
    async fn test_m3_truthfulness_invariant_all_advertised_peers_are_sendable() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("truth-peer-{suffix}");
        let runtime_name = format!("truth-runtime-{suffix}");

        let _peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();

        let all_peers = CoreCommsRuntime::peers(&runtime).await;
        // Only test peers we set up (avoid stale global registry entries)
        let peers: Vec<_> = all_peers
            .iter()
            .filter(|e| e.name.0.contains(&suffix))
            .collect();
        assert!(
            !peers.is_empty(),
            "should have at least the inproc peer visible"
        );

        for entry in &peers {
            for kind in &entry.sendable_kinds {
                let cmd = match kind.as_str() {
                    "peer_message" => CommsCommand::PeerMessage {
                        to: entry.name.clone(),
                        body: "truthfulness test".to_string(),
                    },
                    "peer_request" => CommsCommand::PeerRequest {
                        to: entry.name.clone(),
                        intent: "test".to_string(),
                        params: serde_json::json!({}),
                        stream: InputStreamMode::None,
                    },
                    "peer_response" => CommsCommand::PeerResponse {
                        to: entry.name.clone(),
                        in_reply_to: meerkat_core::InteractionId(Uuid::new_v4()),
                        status: meerkat_core::ResponseStatus::Completed,
                        result: serde_json::json!({}),
                    },
                    _ => continue,
                };
                let result = CoreCommsRuntime::send(&runtime, cmd).await;
                assert!(
                    !matches!(result, Err(SendError::PeerNotFound(_))),
                    "peer '{}' advertised kind '{}' but send failed with PeerNotFound",
                    entry.name.0,
                    kind
                );
            }
        }
    }

    // --- M4: Reservation FSM + concurrency tests ---

    #[tokio::test]
    async fn test_m4_duplicate_close_is_safe() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("dup-close-{suffix}")).unwrap();
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        let stream = CoreCommsRuntime::stream(
            &runtime,
            StreamScope::Interaction(iid.clone()),
        )
        .unwrap();
        // Drop stream → ClosedEarly
        drop(stream);

        // Second attach after close → NotReserved (entry cleaned up)
        let result = CoreCommsRuntime::stream(
            &runtime,
            StreamScope::Interaction(iid.clone()),
        );
        assert!(matches!(result, Err(StreamError::NotReserved(_))));
    }

    #[tokio::test]
    async fn test_m4_mark_interaction_complete_cleans_up() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("complete-{suffix}")).unwrap();
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        let mut stream = CoreCommsRuntime::stream(
            &runtime,
            StreamScope::Interaction(iid.clone()),
        )
        .unwrap();

        // Simulate terminal event
        runtime.mark_interaction_complete(iid.0);

        let terminal = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("stream should terminate promptly after completion");
        assert!(terminal.is_none());

        // Registry entry should be cleaned up
        let registry = runtime.interaction_stream_registry.lock();
        assert!(
            !registry.contains_key(&iid.0),
            "completed entry should be cleaned from registry"
        );
    }

    #[tokio::test]
    async fn test_m4_reap_expired_reservations() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("reap-{suffix}")).unwrap();

        // Manually insert an entry with an old timestamp
        let id = Uuid::new_v4();
        let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(16);
        {
            let mut entry = StreamRegistryEntry::reserved(sender, receiver);
            entry.created_at =
                std::time::Instant::now() - std::time::Duration::from_secs(60);
            runtime.interaction_stream_registry.lock().insert(id, entry);
        }

        runtime.reap_expired_reservations();

        let registry = runtime.interaction_stream_registry.lock();
        assert!(
            !registry.contains_key(&id),
            "expired reservation should have been reaped"
        );
    }

    // --- M5: send_and_stream partial-failure tests ---

    #[tokio::test]
    async fn test_m5_send_and_stream_non_streamable_returns_stream_attach() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("m5-peer-{suffix}");
        let runtime_name = format!("m5-runtime-{suffix}");

        let _peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();

        let cmd = CommsCommand::PeerMessage {
            to: PeerName(peer_name),
            body: "not streamable".to_string(),
        };
        let result = CoreCommsRuntime::send_and_stream(&runtime, cmd).await;
        match result {
            Err(SendAndStreamError::StreamAttach { receipt, error }) => {
                assert!(
                    matches!(receipt, SendReceipt::PeerMessageSent { .. }),
                    "receipt should be PeerMessageSent"
                );
                assert!(
                    matches!(error, StreamError::NotFound(_)),
                    "error should be NotFound for non-streamable command"
                );
            }
            Err(e) => panic!("expected StreamAttach error, got send error: {e}"),
            Ok(_) => panic!("expected StreamAttach error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_m5_send_and_stream_input_none_is_not_streamable() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("m5-none-{suffix}")).unwrap();
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            session_id: session_id.clone(),
            body: "no stream".to_string(),
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: true,
        };
        let result = CoreCommsRuntime::send_and_stream(&runtime, cmd).await;
        match result {
            Err(SendAndStreamError::StreamAttach { receipt, error }) => {
                assert!(
                    matches!(receipt, SendReceipt::InputAccepted { stream_reserved: false, .. }),
                    "receipt should indicate no stream reserved"
                );
                assert!(matches!(error, StreamError::NotFound(_)));
            }
            Err(e) => panic!("expected StreamAttach error, got send error: {e}"),
            Ok(_) => panic!("expected StreamAttach error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_m4_attach_after_completed_returns_not_reserved() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("post-comp-{suffix}")).unwrap();
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        // Attach
        let _stream = CoreCommsRuntime::stream(
            &runtime,
            StreamScope::Interaction(iid.clone()),
        )
        .unwrap();

        // Complete
        runtime.mark_interaction_complete(iid.0);

        // Try to attach again → NotReserved
        let result = CoreCommsRuntime::stream(
            &runtime,
            StreamScope::Interaction(iid.clone()),
        );
        assert!(
            matches!(result, Err(StreamError::NotReserved(_))),
            "attach after completed should return NotReserved"
        );
    }

    #[tokio::test]
    async fn test_allow_self_session_guard_rejects_default() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("self-guard-{suffix}")).unwrap();
        let cmd = CommsCommand::Input {
            session_id: meerkat_core::SessionId::new(),
            body: "blocked".to_string(),
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: false,
        };
        let result = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(
            matches!(result, Err(SendError::Validation(_))),
            "allow_self_session=false should reject self-input"
        );
    }
}
