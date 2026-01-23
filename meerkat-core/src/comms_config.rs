//! Core comms configuration types for meerkat-core.
//!
//! This module provides portable comms configuration that can be used
//! by all interfaces (CLI, MCP, REST, SDK) to configure agent communication.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Core configuration for agent-to-agent communication.
///
/// This struct defines the comms settings in a portable way. Paths may contain
/// `{name}` placeholders that get interpolated with the agent's name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoreCommsConfig {
    /// Whether comms is enabled.
    pub enabled: bool,
    /// Name of this agent (used in path interpolation and peer identification).
    pub name: String,
    /// Unix domain socket path to listen on (optional).
    /// May contain `{name}` placeholder.
    pub listen_uds: Option<PathBuf>,
    /// TCP address to listen on (optional).
    pub listen_tcp: Option<SocketAddr>,
    /// Directory where identity keys are stored.
    /// May contain `{name}` placeholder.
    pub identity_dir: PathBuf,
    /// Path to the trusted peers JSON file.
    /// May contain `{name}` placeholder.
    pub trusted_peers_path: PathBuf,
    /// Timeout for ack wait in seconds.
    pub ack_timeout_secs: u64,
    /// Maximum message size in bytes.
    pub max_message_bytes: usize,
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
            max_message_bytes: 1_048_576, // 1 MB
        }
    }
}

impl CoreCommsConfig {
    /// Create a new config with the given name.
    ///
    /// This is a convenience method that sets the name and enables comms.
    pub fn with_name(name: &str) -> Self {
        Self {
            enabled: true,
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Interpolate `{name}` placeholders in a path with the agent's name.
    fn interpolate_path(&self, path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();
        let interpolated = path_str.replace("{name}", &self.name);
        PathBuf::from(interpolated)
    }

    /// Resolve all paths to absolute paths relative to a base directory.
    ///
    /// This method:
    /// 1. Interpolates `{name}` placeholders in paths
    /// 2. Makes relative paths absolute relative to `base_dir`
    /// 3. Returns a `ResolvedCommsConfig` with all absolute paths
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
            ack_timeout_secs: self.ack_timeout_secs,
            max_message_bytes: self.max_message_bytes,
        }
    }
}

/// Resolved comms configuration with absolute paths.
///
/// This is the result of calling `CoreCommsConfig::resolve_paths()`.
/// All paths are absolute and ready for use.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCommsConfig {
    /// Whether comms is enabled.
    pub enabled: bool,
    /// Name of this agent.
    pub name: String,
    /// Unix domain socket path to listen on (absolute, optional).
    pub listen_uds: Option<PathBuf>,
    /// TCP address to listen on (optional).
    pub listen_tcp: Option<SocketAddr>,
    /// Directory where identity keys are stored (absolute).
    pub identity_dir: PathBuf,
    /// Path to the trusted peers JSON file (absolute).
    pub trusted_peers_path: PathBuf,
    /// Timeout for ack wait in seconds.
    pub ack_timeout_secs: u64,
    /// Maximum message size in bytes.
    pub max_message_bytes: usize,
}

impl ResolvedCommsConfig {
    /// Convert to a `CommsConfig` for use with the meerkat-comms library.
    ///
    /// This extracts the timeout and message size settings into the format
    /// expected by the comms router.
    pub fn to_comms_config(&self) -> meerkat_comms::CommsConfig {
        meerkat_comms::CommsConfig {
            ack_timeout_secs: self.ack_timeout_secs,
            max_message_bytes: self.max_message_bytes as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_comms_config_struct() {
        // Verify struct has all required fields
        let config = CoreCommsConfig {
            enabled: true,
            name: "test".to_string(),
            listen_uds: Some(PathBuf::from("/tmp/test.sock")),
            listen_tcp: Some("127.0.0.1:4200".parse().unwrap()),
            identity_dir: PathBuf::from("/tmp/identity"),
            trusted_peers_path: PathBuf::from("/tmp/trusted.json"),
            ack_timeout_secs: 10,
            max_message_bytes: 2_000_000,
        };
        assert!(config.enabled);
        assert_eq!(config.name, "test");
        assert_eq!(config.listen_uds, Some(PathBuf::from("/tmp/test.sock")));
        assert_eq!(
            config.listen_tcp,
            Some("127.0.0.1:4200".parse().unwrap())
        );
        assert_eq!(config.identity_dir, PathBuf::from("/tmp/identity"));
        assert_eq!(config.trusted_peers_path, PathBuf::from("/tmp/trusted.json"));
        assert_eq!(config.ack_timeout_secs, 10);
        assert_eq!(config.max_message_bytes, 2_000_000);
    }

    #[test]
    fn test_core_comms_config_defaults() {
        let config = CoreCommsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.name, "meerkat");
        assert_eq!(config.listen_uds, None);
        assert_eq!(config.listen_tcp, None);
        assert_eq!(config.identity_dir, PathBuf::from(".rkat/identity"));
        assert_eq!(
            config.trusted_peers_path,
            PathBuf::from(".rkat/trusted_peers.json")
        );
        assert_eq!(config.ack_timeout_secs, 30);
        assert_eq!(config.max_message_bytes, 1_048_576);
    }

    #[test]
    fn test_core_comms_config_serde() {
        let config = CoreCommsConfig {
            enabled: true,
            name: "alice".to_string(),
            listen_uds: Some(PathBuf::from("/tmp/alice.sock")),
            listen_tcp: Some("0.0.0.0:4200".parse().unwrap()),
            identity_dir: PathBuf::from(".rkat/alice/identity"),
            trusted_peers_path: PathBuf::from(".rkat/alice/trusted.json"),
            ack_timeout_secs: 30,
            max_message_bytes: 500_000,
        };

        // Serialize to JSON
        let json = serde_json::to_string(&config).unwrap();
        // Deserialize back
        let deserialized: CoreCommsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);

        // Also test TOML round-trip
        let toml = toml::to_string(&config).unwrap();
        let from_toml: CoreCommsConfig = toml::from_str(&toml).unwrap();
        assert_eq!(config, from_toml);
    }

    #[test]
    fn test_core_comms_config_with_name() {
        let config = CoreCommsConfig::with_name("bob");
        assert!(config.enabled);
        assert_eq!(config.name, "bob");
        // Other fields should be default
        assert_eq!(config.listen_uds, None);
        assert_eq!(config.listen_tcp, None);
        assert_eq!(config.identity_dir, PathBuf::from(".rkat/identity"));
    }

    #[test]
    fn test_core_comms_config_path_interpolation() {
        let mut config = CoreCommsConfig::default();
        config.name = "alice".to_string();
        config.identity_dir = PathBuf::from(".rkat/{name}/identity");
        config.trusted_peers_path = PathBuf::from(".rkat/{name}/trusted.json");
        config.listen_uds = Some(PathBuf::from("/tmp/meerkat-{name}.sock"));

        let resolved = config.resolve_paths(Path::new("/home/user/project"));

        // Paths should have {name} replaced with "alice"
        assert_eq!(
            resolved.identity_dir,
            PathBuf::from("/home/user/project/.rkat/alice/identity")
        );
        assert_eq!(
            resolved.trusted_peers_path,
            PathBuf::from("/home/user/project/.rkat/alice/trusted.json")
        );
        // UDS path is absolute, so it should just be interpolated
        assert_eq!(
            resolved.listen_uds,
            Some(PathBuf::from("/tmp/meerkat-alice.sock"))
        );
    }

    #[test]
    fn test_core_comms_config_resolve_paths() {
        let config = CoreCommsConfig {
            enabled: true,
            name: "test".to_string(),
            listen_uds: None,
            listen_tcp: Some("127.0.0.1:4200".parse().unwrap()),
            identity_dir: PathBuf::from("data/identity"),
            trusted_peers_path: PathBuf::from("data/peers.json"),
            ack_timeout_secs: 5,
            max_message_bytes: 1_000_000,
        };

        let resolved = config.resolve_paths(Path::new("/app"));

        assert!(resolved.enabled);
        assert_eq!(resolved.name, "test");
        assert_eq!(resolved.listen_uds, None);
        assert_eq!(resolved.listen_tcp, Some("127.0.0.1:4200".parse().unwrap()));
        assert_eq!(resolved.identity_dir, PathBuf::from("/app/data/identity"));
        assert_eq!(
            resolved.trusted_peers_path,
            PathBuf::from("/app/data/peers.json")
        );
        assert_eq!(resolved.ack_timeout_secs, 5);
        assert_eq!(resolved.max_message_bytes, 1_000_000);
    }

    #[test]
    fn test_resolved_comms_config_struct() {
        // Verify ResolvedCommsConfig has all required fields
        let resolved = ResolvedCommsConfig {
            enabled: true,
            name: "test".to_string(),
            listen_uds: Some(PathBuf::from("/tmp/test.sock")),
            listen_tcp: Some("127.0.0.1:4200".parse().unwrap()),
            identity_dir: PathBuf::from("/absolute/path/identity"),
            trusted_peers_path: PathBuf::from("/absolute/path/trusted.json"),
            ack_timeout_secs: 10,
            max_message_bytes: 2_000_000,
        };
        assert!(resolved.enabled);
        assert_eq!(resolved.name, "test");
        assert!(resolved.identity_dir.is_absolute());
        assert!(resolved.trusted_peers_path.is_absolute());
    }

    #[test]
    fn test_resolved_to_comms_config() {
        let resolved = ResolvedCommsConfig {
            enabled: true,
            name: "test".to_string(),
            listen_uds: None,
            listen_tcp: Some("127.0.0.1:4200".parse().unwrap()),
            identity_dir: PathBuf::from("/tmp/identity"),
            trusted_peers_path: PathBuf::from("/tmp/trusted.json"),
            ack_timeout_secs: 30,
            max_message_bytes: 500_000,
        };

        // Convert to CommsConfig
        let comms_config = resolved.to_comms_config();

        // Verify the conversion worked
        assert_eq!(comms_config.ack_timeout_secs, 30);
        assert_eq!(comms_config.max_message_bytes, 500_000);
    }
}
