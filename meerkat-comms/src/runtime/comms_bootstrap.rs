//! CommsBootstrap - Unified comms setup for all agents.

use super::comms_config::CoreCommsConfig;
use super::comms_runtime::CommsRuntime;
use crate::{PubKey, TrustedPeer};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommsBootstrapError {
    #[error("Runtime error: {0}")]
    RuntimeError(String),
}

pub enum CommsBootstrapMode {
    #[cfg(not(target_arch = "wasm32"))]
    Standalone,
    ChildInproc,
}

pub struct CommsAdvertise {
    pub name: String,
    pub pubkey: [u8; 32],
    pub addr: String,
}

pub struct CommsBootstrap {
    config: CoreCommsConfig,
    #[cfg(not(target_arch = "wasm32"))]
    base_dir: PathBuf,
    mode: CommsBootstrapMode,
    parent_context: Option<ParentCommsContext>,
}

impl CommsBootstrap {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_config(config: CoreCommsConfig, base_dir: PathBuf) -> Self {
        Self {
            config,
            base_dir,
            mode: CommsBootstrapMode::Standalone,
            parent_context: None,
        }
    }

    pub fn for_child_inproc(name: String, parent_context: ParentCommsContext) -> Self {
        let mut config = CoreCommsConfig::with_name(&name);
        config.enabled = true;
        config.inproc_namespace.clone_from(&parent_context.inproc_namespace);
        Self {
            config,
            #[cfg(not(target_arch = "wasm32"))]
            base_dir: parent_context.comms_base_dir.clone(),
            mode: CommsBootstrapMode::ChildInproc,
            parent_context: Some(parent_context),
        }
    }

    pub async fn prepare(self) -> Result<Option<PreparedComms>, CommsBootstrapError> {
        match self.mode {
            #[cfg(not(target_arch = "wasm32"))]
            CommsBootstrapMode::Standalone => {
                let resolved = self.config.resolve_paths(&self.base_dir);
                let runtime = CommsRuntime::new(resolved)
                    .await
                    .map_err(|e| CommsBootstrapError::RuntimeError(e.to_string()))?;
                Ok(Some(PreparedComms {
                    runtime,
                    advertise: None,
                }))
            }
            CommsBootstrapMode::ChildInproc => {
                let parent = self.parent_context.ok_or_else(|| {
                    CommsBootstrapError::RuntimeError(
                        "ChildInproc mode requires parent_context".to_string(),
                    )
                })?;
                let runtime = CommsRuntime::inproc_only_scoped(
                    &self.config.name,
                    self.config.inproc_namespace.clone(),
                )
                .map_err(|e| CommsBootstrapError::RuntimeError(e.to_string()))?;

                let parent_peer = TrustedPeer {
                    name: parent.parent_name,
                    pubkey: PubKey::new(parent.parent_pubkey),
                    addr: parent.parent_addr,
                    meta: crate::PeerMeta::default(),
                };
                runtime
                    .trusted_peers_shared()
                    .write()
                    .await
                    .upsert(parent_peer);

                let advertise = CommsAdvertise {
                    name: self.config.name,
                    pubkey: *runtime.public_key().as_bytes(),
                    addr: format!("inproc://{}", runtime.public_key().to_peer_id()),
                };

                Ok(Some(PreparedComms {
                    runtime,
                    advertise: Some(advertise),
                }))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParentCommsContext {
    pub parent_name: String,
    pub parent_pubkey: [u8; 32],
    pub parent_addr: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub comms_base_dir: PathBuf,
    pub inproc_namespace: Option<String>,
}

pub struct PreparedComms {
    pub runtime: CommsRuntime,
    pub advertise: Option<CommsAdvertise>,
}

pub fn create_child_comms_config(name: &str) -> CoreCommsConfig {
    CoreCommsConfig::with_name(name)
}
