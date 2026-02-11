//! CommsRuntime - Full lifecycle manager for agent-to-agent communication.

use super::comms_config::ResolvedCommsConfig;
use crate::agent::types::{CommsContent, CommsMessage};
use crate::{
    InboxSender, InprocRegistry, Keypair, PubKey, Router, TrustedPeers, handle_connection,
};
use async_trait::async_trait;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

fn is_dismiss(msg: &CommsMessage) -> bool {
    matches!(&msg.content, CommsContent::Message { body } if body.trim().eq_ignore_ascii_case("DISMISS"))
}

#[async_trait]
impl CoreCommsRuntime for CommsRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        let messages = self.drain_messages().await;
        if messages.iter().any(is_dismiss) {
            self.dismiss_flag.store(true, Ordering::SeqCst);
        }
        messages
            .iter()
            .filter(|m| !is_dismiss(m))
            .map(|m| m.to_user_message_text())
            .collect()
    }
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }
    fn dismiss_received(&self) -> bool {
        self.dismiss_flag.swap(false, Ordering::SeqCst)
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
}

pub struct CommsRuntime {
    public_key: PubKey,
    router: Arc<Router>,
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    inbox: Arc<Mutex<crate::Inbox>>,
    inbox_notify: Arc<tokio::sync::Notify>,
    config: ResolvedCommsConfig,
    listener_handles: Vec<ListenerHandle>,
    listeners_started: bool,
    keypair: Arc<Keypair>,
    dismiss_flag: AtomicBool,
}

impl CommsRuntime {
    pub async fn new(config: ResolvedCommsConfig) -> Result<Self, CommsRuntimeError> {
        let keypair = Keypair::load_or_generate(&config.identity_dir)
            .await
            .map_err(|e| CommsRuntimeError::IdentityError(e.to_string()))?;
        let public_key = keypair.public_key();
        let trusted_peers = TrustedPeers::load_or_default(&config.trusted_peers_path)
            .map_err(|e| CommsRuntimeError::TrustLoadError(e.to_string()))?;
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
            inbox: Arc::new(Mutex::new(inbox)),
            inbox_notify,
            config: config.clone(),
            listener_handles: Vec::new(),
            listeners_started: false,
            keypair: Arc::new(keypair),
            dismiss_flag: AtomicBool::new(false),
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
            comms_config: comms_config.clone(),
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
            inbox: Arc::new(Mutex::new(inbox)),
            inbox_notify,
            config,
            listener_handles: Vec::new(),
            listeners_started: false,
            keypair: Arc::new(keypair),
            dismiss_flag: AtomicBool::new(false),
        };
        InprocRegistry::global().register(name, runtime.public_key, inbox_sender);
        Ok(runtime)
    }

    pub async fn start_listeners(&mut self) -> Result<(), CommsRuntimeError> {
        if self.listeners_started {
            return Err(CommsRuntimeError::AlreadyStarted);
        }
        if let Some(ref path) = self.config.listen_uds {
            let handle = spawn_uds_listener(
                path,
                self.keypair.clone(),
                self.trusted_peers.clone(),
                self.router.inbox_sender().clone(),
            )
            .await?;
            self.listener_handles.push(handle);
        }
        if let Some(ref addr) = self.config.listen_tcp {
            let handle = spawn_tcp_listener(
                &addr.to_string(),
                self.keypair.clone(),
                self.trusted_peers.clone(),
                self.router.inbox_sender().clone(),
            )
            .await?;
            self.listener_handles.push(handle);
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
    pub fn router_arc(&self) -> Arc<Router> {
        self.router.clone()
    }
    pub fn trusted_peers_shared(&self) -> Arc<RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }
    pub fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
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
