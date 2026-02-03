//! CommsBootstrap - Unified comms setup for all agents.
//!
//! This module provides a uniform way to set up comms for any agent,
//! whether it's a top-level agent or a sub-agent. The key principle is:
//! "An agent is an agent" - no special-casing based on nesting depth.
//!
//! # Usage
//!
//! For a main agent:
//! ```text
//! let bootstrap = CommsBootstrap::from_config(config, base_dir);
//! let prepared = bootstrap.prepare()?;
//! ```
//!
//! For a sub-agent (uses lightweight inproc transport by default):
//! ```text
//! let bootstrap = CommsBootstrap::for_child_inproc(name, parent_context);
//! let prepared = bootstrap.prepare()?;
//! // Parent should trust child using: prepared.advertise
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use meerkat_comms::{PubKey, TrustedPeer, TrustedPeers};
use thiserror::Error;

use crate::comms_config::CoreCommsConfig;
use crate::comms_runtime::CommsRuntime;

/// Errors that can occur during comms bootstrap.
#[derive(Debug, Error)]
pub enum CommsBootstrapError {
    #[error("Failed to create comms runtime: {0}")]
    RuntimeCreation(String),
    #[error("Failed to create directories: {0}")]
    DirectoryCreation(String),
    #[error("Failed to save trusted peers: {0}")]
    TrustedPeersSave(String),
    #[error("Failed to start listeners: {0}")]
    ListenerStart(String),
}

/// Information about a parent agent for establishing trust.
///
/// When spawning a sub-agent, the parent provides this context so the
/// child can trust the parent and the parent can trust the child.
#[derive(Debug, Clone)]
pub struct ParentCommsContext {
    /// Name of the parent agent.
    pub parent_name: String,
    /// Public key of the parent agent.
    pub parent_pubkey: [u8; 32],
    /// Address to reach the parent agent.
    pub parent_addr: String,
    /// Base directory for comms files (shared between parent and children).
    pub comms_base_dir: PathBuf,
}

/// Advertised comms identity for establishing trust.
///
/// After preparing comms, this contains the information needed for
/// another agent to trust and communicate with this one.
#[derive(Debug, Clone)]
pub struct CommsAdvertise {
    /// Name of this agent.
    pub name: String,
    /// Public key of this agent.
    pub pubkey: [u8; 32],
    /// Address to reach this agent.
    pub addr: String,
}

impl CommsAdvertise {
    /// Convert to a TrustedPeer for adding to another agent's trust list.
    pub fn to_trusted_peer(&self) -> TrustedPeer {
        TrustedPeer {
            name: self.name.clone(),
            pubkey: PubKey::new(self.pubkey),
            addr: self.addr.clone(),
        }
    }
}

/// Result of preparing comms.
pub struct PreparedComms {
    /// The comms runtime, ready to use.
    pub runtime: CommsRuntime,
    /// Information to advertise to other agents (for trust establishment).
    /// Only present when preparing for a child agent.
    pub advertise: Option<CommsAdvertise>,
}

/// Mode for comms bootstrap.
pub enum CommsBootstrapMode {
    /// Create comms from a configuration (for main agents).
    Config(CoreCommsConfig),
    /// Use a pre-created runtime (when runtime is created externally).
    Runtime(Arc<tokio::sync::Mutex<Option<CommsRuntime>>>),
    /// Create comms for a child agent with parent trust context (uses UDS/TCP).
    /// This is the legacy mode for backward compatibility.
    Child(ParentCommsContext),
    /// Create comms for a child agent using in-process transport (recommended).
    /// No filesystem or network resources needed - messages delivered via channels.
    Inproc(ParentCommsContext),
    /// Comms disabled (returns None from prepare).
    Disabled,
}

/// Bootstrap configuration for agent comms.
///
/// This struct provides a uniform way to set up comms for any agent.
/// Use the constructor methods to create the appropriate mode:
/// - `from_config()` for main agents
/// - `for_child()` for sub-agents
/// - `disabled()` when comms is not needed
pub struct CommsBootstrap {
    /// Name of this agent.
    pub name: String,
    /// Base directory for resolving paths.
    pub base_dir: PathBuf,
    /// Bootstrap mode.
    mode: CommsBootstrapMode,
}

impl CommsBootstrap {
    /// Create a bootstrap from a comms config (for main agents).
    pub fn from_config(config: CoreCommsConfig, base_dir: PathBuf) -> Self {
        let name = config.name.clone();
        Self {
            name,
            base_dir,
            mode: CommsBootstrapMode::Config(config),
        }
    }

    /// Create a bootstrap for a child agent using UDS (legacy mode).
    ///
    /// This creates filesystem resources (identity, trust files) and UDS listeners.
    /// For most sub-agent use cases, prefer `for_child_inproc()` which is simpler
    /// and doesn't require filesystem or network resources.
    pub fn for_child(name: String, base_dir: PathBuf, parent_context: ParentCommsContext) -> Self {
        Self {
            name,
            base_dir,
            mode: CommsBootstrapMode::Child(parent_context),
        }
    }

    /// Create a bootstrap for a child agent using in-process transport (recommended).
    ///
    /// This is the preferred method for sub-agents. It uses lightweight in-process
    /// channels for communication instead of UDS/TCP sockets:
    /// - No filesystem resources needed (no identity files, no trust files)
    /// - No network listeners (no UDS sockets)
    /// - Messages delivered directly via in-memory channels
    /// - Automatic cleanup when the agent is dropped
    ///
    /// The parent must also use inproc transport or be in the same process
    /// for messages to be delivered.
    pub fn for_child_inproc(name: String, parent_context: ParentCommsContext) -> Self {
        Self {
            name,
            base_dir: PathBuf::new(), // Not used for inproc
            mode: CommsBootstrapMode::Inproc(parent_context),
        }
    }

    /// Create a bootstrap with comms disabled.
    pub fn disabled() -> Self {
        Self {
            name: String::new(),
            base_dir: PathBuf::new(),
            mode: CommsBootstrapMode::Disabled,
        }
    }

    /// Create a bootstrap with a pre-created runtime.
    ///
    /// The runtime is wrapped in Arc<Mutex<Option>> so it can be moved
    /// out during prepare(). This is useful when the runtime is created
    /// externally (e.g., for testing).
    pub fn with_runtime(runtime: CommsRuntime) -> Self {
        Self {
            name: String::new(),
            base_dir: PathBuf::new(),
            mode: CommsBootstrapMode::Runtime(Arc::new(tokio::sync::Mutex::new(Some(runtime)))),
        }
    }

    /// Check if this bootstrap is disabled.
    pub fn is_disabled(&self) -> bool {
        matches!(self.mode, CommsBootstrapMode::Disabled)
    }

    /// Check if this bootstrap uses a config.
    pub fn is_config(&self) -> bool {
        matches!(self.mode, CommsBootstrapMode::Config(_))
    }

    /// Check if this bootstrap is for a child agent (UDS mode).
    pub fn is_child(&self) -> bool {
        matches!(self.mode, CommsBootstrapMode::Child(_))
    }

    /// Check if this bootstrap is for a child agent (inproc mode).
    pub fn is_inproc(&self) -> bool {
        matches!(self.mode, CommsBootstrapMode::Inproc(_))
    }

    /// Prepare the comms runtime.
    ///
    /// This method handles all the setup based on the bootstrap mode:
    /// - Config: resolves paths, creates runtime, starts listeners
    /// - Runtime: returns the pre-created runtime
    /// - Child: creates trust files, generates child config, starts listeners
    /// - Disabled: returns None
    ///
    /// For Child mode, returns advertise info that the parent should use
    /// to add this child to its trust list.
    pub async fn prepare(self) -> Result<Option<PreparedComms>, CommsBootstrapError> {
        match self.mode {
            CommsBootstrapMode::Disabled => Ok(None),

            CommsBootstrapMode::Runtime(runtime_holder) => {
                let mut guard = runtime_holder.lock().await;
                let runtime = guard.take().ok_or_else(|| {
                    CommsBootstrapError::RuntimeCreation("Runtime already taken".to_string())
                })?;
                Ok(Some(PreparedComms {
                    runtime,
                    advertise: None,
                }))
            }

            CommsBootstrapMode::Config(config) => {
                if !config.enabled {
                    return Ok(None);
                }

                let resolved = config.resolve_paths(&self.base_dir);
                let mut runtime = CommsRuntime::new(resolved)
                    .await
                    .map_err(|e| CommsBootstrapError::RuntimeCreation(e.to_string()))?;

                // Start listeners
                runtime
                    .start_listeners()
                    .await
                    .map_err(|e| CommsBootstrapError::ListenerStart(e.to_string()))?;

                tracing::info!(
                    "Comms enabled for agent '{}' (peer ID: {})",
                    self.name,
                    runtime.public_key().to_peer_id()
                );

                Ok(Some(PreparedComms {
                    runtime,
                    advertise: None,
                }))
            }

            CommsBootstrapMode::Child(parent_context) => {
                // Create child comms config
                let child_config = create_child_comms_config(&self.name, &self.base_dir);
                let resolved = child_config.resolve_paths(&self.base_dir);

                // Create trusted peers (trusting the parent)
                let trusted_peers = create_child_trusted_peers(&parent_context);

                // Ensure directories exist
                tokio::fs::create_dir_all(&resolved.identity_dir)
                    .await
                    .map_err(|e| CommsBootstrapError::DirectoryCreation(e.to_string()))?;

                if let Some(parent) = resolved.trusted_peers_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| CommsBootstrapError::DirectoryCreation(e.to_string()))?;
                }

                // Save trusted peers
                let trusted_peers_path = resolved.trusted_peers_path.clone();
                trusted_peers
                    .save(&trusted_peers_path)
                    .await
                    .map_err(|e| CommsBootstrapError::TrustedPeersSave(e.to_string()))?;

                // Create runtime with the keypair that will be loaded/generated
                let resolved_for_runtime = resolved.clone();
                let mut runtime = CommsRuntime::new(resolved_for_runtime)
                    .await
                    .map_err(|e| CommsBootstrapError::RuntimeCreation(e.to_string()))?;

                // Get child's public key and address for advertising
                let child_pubkey = *runtime.public_key().as_bytes();
                let child_addr = resolved
                    .listen_uds
                    .as_ref()
                    .map(|p| format!("uds://{}", p.display()))
                    .or_else(|| resolved.listen_tcp.map(|a| format!("tcp://{}", a)))
                    .unwrap_or_else(|| {
                        format!(
                            "uds://{}",
                            self.base_dir.join(format!("{}.sock", self.name)).display()
                        )
                    });

                // Start listeners
                runtime
                    .start_listeners()
                    .await
                    .map_err(|e| CommsBootstrapError::ListenerStart(e.to_string()))?;

                tracing::info!(
                    "Comms enabled for child agent '{}' (peer ID: {})",
                    self.name,
                    runtime.public_key().to_peer_id()
                );

                let advertise = CommsAdvertise {
                    name: self.name.clone(),
                    pubkey: child_pubkey,
                    addr: child_addr,
                };

                Ok(Some(PreparedComms {
                    runtime,
                    advertise: Some(advertise),
                }))
            }

            CommsBootstrapMode::Inproc(parent_context) => {
                // Create inproc-only runtime (no network listeners).
                // This may still perform blocking work (e.g., identity/key material setup),
                // so do it on the blocking pool.
                let name = self.name.clone();
                let runtime = tokio::task::spawn_blocking(move || CommsRuntime::inproc_only(name))
                    .await
                    .map_err(|e| CommsBootstrapError::RuntimeCreation(e.to_string()))?
                    .map_err(|e| CommsBootstrapError::RuntimeCreation(e.to_string()))?;

                // Get child's public key and inproc address
                let child_pubkey = *runtime.public_key().as_bytes();
                let child_addr = runtime.inproc_addr();

                // Add parent to this child's trusted peers so we can send to parent
                let parent_peer = TrustedPeer {
                    name: parent_context.parent_name.clone(),
                    pubkey: PubKey::new(parent_context.parent_pubkey),
                    addr: parent_context.parent_addr.clone(),
                };
                runtime.add_trusted_peer(parent_peer).await;

                tracing::info!(
                    "Inproc comms enabled for child agent '{}' (peer ID: {})",
                    self.name,
                    runtime.public_key().to_peer_id()
                );

                let advertise = CommsAdvertise {
                    name: self.name.clone(),
                    pubkey: child_pubkey,
                    addr: child_addr,
                };

                Ok(Some(PreparedComms {
                    runtime,
                    advertise: Some(advertise),
                }))
            }
        }
    }
}

/// Create comms configuration for a child agent.
fn create_child_comms_config(child_name: &str, base_dir: &std::path::Path) -> CoreCommsConfig {
    CoreCommsConfig {
        enabled: true,
        name: child_name.to_string(),
        // Use UDS for local communication (more efficient than TCP)
        listen_uds: Some(base_dir.join(format!("{}.sock", child_name))),
        listen_tcp: None,
        identity_dir: base_dir.join(format!("{}/identity", child_name)),
        trusted_peers_path: base_dir.join(format!("{}/trusted_peers.json", child_name)),
        ack_timeout_secs: 30,
        max_message_bytes: 1_048_576,
    }
}

/// Create a TrustedPeers list that trusts the parent.
fn create_child_trusted_peers(parent_context: &ParentCommsContext) -> TrustedPeers {
    TrustedPeers {
        peers: vec![TrustedPeer {
            name: parent_context.parent_name.clone(),
            pubkey: PubKey::new(parent_context.parent_pubkey),
            addr: parent_context.parent_addr.clone(),
        }],
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_comms_bootstrap_disabled() {
        let bootstrap = CommsBootstrap::disabled();
        assert!(bootstrap.is_disabled());
    }

    #[test]
    fn test_comms_bootstrap_from_config() {
        let config = CoreCommsConfig::with_name("test-agent");
        let bootstrap = CommsBootstrap::from_config(config, PathBuf::from("/tmp"));
        assert_eq!(bootstrap.name, "test-agent");
        assert!(bootstrap.is_config());
    }

    #[test]
    fn test_comms_bootstrap_for_child() {
        let parent_context = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "uds:///tmp/parent.sock".to_string(),
            comms_base_dir: PathBuf::from("/tmp/comms"),
        };
        let bootstrap = CommsBootstrap::for_child(
            "child".to_string(),
            PathBuf::from("/tmp/comms"),
            parent_context,
        );
        assert_eq!(bootstrap.name, "child");
        assert!(bootstrap.is_child());
    }

    #[tokio::test]
    async fn test_prepare_disabled_returns_none() {
        let bootstrap = CommsBootstrap::disabled();
        let result = bootstrap.prepare().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_prepare_config_disabled_returns_none() {
        let mut config = CoreCommsConfig::with_name("test");
        config.enabled = false;
        let bootstrap = CommsBootstrap::from_config(config, PathBuf::from("/tmp"));
        let result = bootstrap.prepare().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_prepare_config_creates_runtime() {
        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.listen_uds = None; // No listeners to avoid port conflicts
        config.listen_tcp = None;

        let bootstrap = CommsBootstrap::from_config(config, tmp.path().to_path_buf());
        let result = bootstrap.prepare().await;

        assert!(result.is_ok());
        let prepared = result.unwrap();
        assert!(prepared.is_some());
        let prepared = prepared.unwrap();
        assert!(prepared.advertise.is_none()); // Config mode doesn't advertise
    }

    #[tokio::test]
    async fn test_prepare_child_creates_runtime_and_advertise() {
        let tmp = TempDir::new().unwrap();
        let parent_context = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "uds:///tmp/parent.sock".to_string(),
            comms_base_dir: tmp.path().to_path_buf(),
        };

        let bootstrap = CommsBootstrap::for_child(
            "child".to_string(),
            tmp.path().to_path_buf(),
            parent_context,
        );
        let result = bootstrap.prepare().await;

        assert!(result.is_ok());
        let prepared = result.unwrap();
        assert!(prepared.is_some());
        let prepared = prepared.unwrap();

        // Child mode should have advertise info
        assert!(prepared.advertise.is_some());
        let advertise = prepared.advertise.unwrap();
        assert_eq!(advertise.name, "child");
        assert_eq!(advertise.pubkey.len(), 32);
    }

    #[test]
    fn test_comms_advertise_to_trusted_peer() {
        let advertise = CommsAdvertise {
            name: "test".to_string(),
            pubkey: [1u8; 32],
            addr: "uds:///tmp/test.sock".to_string(),
        };

        let peer = advertise.to_trusted_peer();
        assert_eq!(peer.name, "test");
        assert_eq!(*peer.pubkey.as_bytes(), [1u8; 32]);
        assert_eq!(peer.addr, "uds:///tmp/test.sock");
    }

    #[test]
    fn test_comms_bootstrap_error_display() {
        let err = CommsBootstrapError::RuntimeCreation("test".to_string());
        assert!(err.to_string().contains("runtime"));

        let err = CommsBootstrapError::DirectoryCreation("test".to_string());
        assert!(err.to_string().contains("directories"));

        let err = CommsBootstrapError::TrustedPeersSave("test".to_string());
        assert!(err.to_string().contains("trusted peers"));

        let err = CommsBootstrapError::ListenerStart("test".to_string());
        assert!(err.to_string().contains("listeners"));
    }

    #[test]
    fn test_comms_bootstrap_for_child_inproc() {
        use meerkat_comms::InprocRegistry;
        InprocRegistry::global().clear();

        let parent_context = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "inproc://parent".to_string(),
            comms_base_dir: PathBuf::from("/unused"),
        };

        let bootstrap =
            CommsBootstrap::for_child_inproc("inproc-child".to_string(), parent_context);
        assert_eq!(bootstrap.name, "inproc-child");
        assert!(bootstrap.is_inproc());
        assert!(!bootstrap.is_child());

        InprocRegistry::global().clear();
    }

    #[tokio::test]
    async fn test_prepare_inproc_creates_runtime_and_advertise() {
        use meerkat_comms::InprocRegistry;
        InprocRegistry::global().clear();

        let parent_context = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "inproc://parent".to_string(),
            comms_base_dir: PathBuf::from("/unused"),
        };

        let bootstrap =
            CommsBootstrap::for_child_inproc("inproc-child".to_string(), parent_context);
        let result = bootstrap.prepare().await;

        assert!(result.is_ok());
        let prepared = result.unwrap();
        assert!(prepared.is_some());
        let prepared = prepared.unwrap();

        // Inproc mode should have advertise info
        assert!(prepared.advertise.is_some());
        let advertise = prepared.advertise.unwrap();
        assert_eq!(advertise.name, "inproc-child");
        assert_eq!(advertise.pubkey.len(), 32);
        assert_eq!(advertise.addr, "inproc://inproc-child");

        // Child should be registered in global registry
        assert!(InprocRegistry::global().contains_name("inproc-child"));

        InprocRegistry::global().clear();
    }

    #[tokio::test]
    async fn test_inproc_child_can_reach_parent() {
        use meerkat_comms::InprocRegistry;
        InprocRegistry::global().clear();

        // Set up parent first
        let parent_keypair = meerkat_comms::Keypair::generate();
        let parent_pubkey = *parent_keypair.public_key().as_bytes();
        let (_, parent_sender) = meerkat_comms::Inbox::new();
        InprocRegistry::global().register("parent", parent_keypair.public_key(), parent_sender);

        // Create child with parent context
        let parent_context = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey,
            parent_addr: "inproc://parent".to_string(),
            comms_base_dir: PathBuf::from("/unused"),
        };

        let bootstrap = CommsBootstrap::for_child_inproc("child".to_string(), parent_context);
        let result = bootstrap.prepare().await;
        assert!(result.is_ok());

        let prepared = result.unwrap().unwrap();

        // Child should have parent as trusted peer
        let trusted = prepared.runtime.trusted_peers_shared();
        let guard = trusted.read().await;
        assert!(guard.get_by_name("parent").is_some());

        InprocRegistry::global().clear();
    }
}
