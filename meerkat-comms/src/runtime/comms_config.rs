//! Core comms configuration types.

use crate::CommsConfig;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Core configuration for agent-to-agent communication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoreCommsConfig {
    pub enabled: bool,
    pub name: String,
    pub listen_uds: Option<PathBuf>,
    pub listen_tcp: Option<SocketAddr>,
    pub identity_dir: PathBuf,
    pub trusted_peers_path: PathBuf,
    pub ack_timeout_secs: u64,
    pub max_message_bytes: u32,
}

impl Default for CoreCommsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "meerkat".to_string(),
            listen_uds: None,
            listen_tcp: None,
            identity_dir: PathBuf::from(".rkat/identity"),
            trusted_peers_path: PathBuf::from(".rkat/trusted_peers.json"),
            ack_timeout_secs: 30,
            max_message_bytes: 1_048_576,
        }
    }
}

impl CoreCommsConfig {
    pub fn with_name(name: &str) -> Self {
        Self {
            enabled: true,
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn interpolate_path(&self, path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();
        let interpolated = path_str.replace("{name}", &self.name);
        PathBuf::from(interpolated)
    }

    pub fn resolve_paths(&self, base_dir: &Path) -> ResolvedCommsConfig {
        let resolve = |path: &Path| -> PathBuf {
            let interpolated = self.interpolate_path(path);
            if interpolated.is_absolute() {
                interpolated
            } else {
                base_dir.join(interpolated)
            }
        };

        ResolvedCommsConfig {
            enabled: self.enabled,
            name: self.name.clone(),
            listen_uds: self.listen_uds.as_ref().map(|p| resolve(p)),
            listen_tcp: self.listen_tcp,
            identity_dir: resolve(&self.identity_dir),
            trusted_peers_path: resolve(&self.trusted_peers_path),
            comms_config: CommsConfig {
                ack_timeout_secs: self.ack_timeout_secs,
                max_message_bytes: self.max_message_bytes,
            },
        }
    }
}

/// Resolved comms configuration with absolute paths.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCommsConfig {
    pub enabled: bool,
    pub name: String,
    pub listen_uds: Option<PathBuf>,
    pub listen_tcp: Option<SocketAddr>,
    pub identity_dir: PathBuf,
    pub trusted_peers_path: PathBuf,
    pub comms_config: CommsConfig,
}
