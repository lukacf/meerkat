//! Configuration loading for the Meerkat CLI.
//!
//! This module handles loading comms configuration from:
//! 1. User config: `~/.rkat/config.toml`
//! 2. Project config: `.rkat/config.toml`
//!
//! Project config overrides user config. CLI flags override both.

use meerkat_core::CoreCommsConfig;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// CLI configuration loaded from TOML files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliConfig {
    /// Comms configuration section.
    #[serde(default)]
    pub comms: Option<CommsConfigSection>,
}

/// Comms section in config.toml.
///
/// Fields map directly to CoreCommsConfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommsConfigSection {
    /// Whether comms is enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Name of this agent.
    #[serde(default)]
    pub name: Option<String>,
    /// Unix domain socket path to listen on.
    #[serde(default)]
    pub listen_uds: Option<PathBuf>,
    /// TCP address to listen on.
    #[serde(default)]
    pub listen_tcp: Option<String>,
    /// Directory where identity keys are stored.
    #[serde(default)]
    pub identity_dir: Option<PathBuf>,
    /// Path to the trusted peers JSON file.
    #[serde(default)]
    pub trusted_peers_path: Option<PathBuf>,
    /// Timeout for ack wait in seconds.
    #[serde(default)]
    pub ack_timeout_secs: Option<u64>,
    /// Maximum message size in bytes.
    #[serde(default)]
    pub max_message_bytes: Option<usize>,
}

impl CommsConfigSection {
    /// Convert to CoreCommsConfig, using defaults for missing fields.
    pub fn to_core_config(&self) -> CoreCommsConfig {
        let mut config = CoreCommsConfig::default();

        if let Some(enabled) = self.enabled {
            config.enabled = enabled;
        }
        if let Some(ref name) = self.name {
            config.name = name.clone();
        }
        if let Some(ref path) = self.listen_uds {
            config.listen_uds = Some(path.clone());
        }
        if let Some(ref addr) = self.listen_tcp {
            if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
                config.listen_tcp = Some(socket_addr);
            }
        }
        if let Some(ref path) = self.identity_dir {
            config.identity_dir = path.clone();
        }
        if let Some(ref path) = self.trusted_peers_path {
            config.trusted_peers_path = path.clone();
        }
        if let Some(timeout) = self.ack_timeout_secs {
            config.ack_timeout_secs = timeout;
        }
        if let Some(size) = self.max_message_bytes {
            config.max_message_bytes = size;
        }

        config
    }
}

/// CLI flag overrides for comms configuration.
#[derive(Debug, Clone, Default)]
pub struct CommsOverrides {
    /// Override agent name.
    pub name: Option<String>,
    /// Override TCP listen address.
    pub listen_tcp: Option<SocketAddr>,
    /// Disable comms entirely.
    pub disabled: bool,
}

impl CommsOverrides {
    /// Apply overrides to a CoreCommsConfig.
    pub fn apply(&self, mut config: CoreCommsConfig) -> CoreCommsConfig {
        if self.disabled {
            config.enabled = false;
            return config;
        }

        if let Some(ref name) = self.name {
            config.name = name.clone();
            // If name is set and comms isn't explicitly disabled, enable it
            if !self.disabled {
                config.enabled = true;
            }
        }

        if let Some(addr) = self.listen_tcp {
            config.listen_tcp = Some(addr);
            // If listen address is set, enable comms
            if !self.disabled {
                config.enabled = true;
            }
        }

        config
    }
}

/// Get the user config file path.
fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".rkat").join("config.toml"))
}

/// Get the project config file path (relative to current directory).
fn project_config_path() -> PathBuf {
    PathBuf::from(".rkat/config.toml")
}

/// Load config from a TOML file, returning None if file doesn't exist.
fn load_config_file(path: &Path) -> Option<CliConfig> {
    if !path.exists() {
        return None;
    }

    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                tracing::warn!("Failed to parse config at {}: {}", path.display(), e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to read config at {}: {}", path.display(), e);
            None
        }
    }
}

/// Merge two comms configs, with `overlay` taking precedence.
fn merge_comms_section(
    base: Option<CommsConfigSection>,
    overlay: Option<CommsConfigSection>,
) -> Option<CommsConfigSection> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (Some(b), Some(o)) => Some(CommsConfigSection {
            enabled: o.enabled.or(b.enabled),
            name: o.name.or(b.name),
            listen_uds: o.listen_uds.or(b.listen_uds),
            listen_tcp: o.listen_tcp.or(b.listen_tcp),
            identity_dir: o.identity_dir.or(b.identity_dir),
            trusted_peers_path: o.trusted_peers_path.or(b.trusted_peers_path),
            ack_timeout_secs: o.ack_timeout_secs.or(b.ack_timeout_secs),
            max_message_bytes: o.max_message_bytes.or(b.max_message_bytes),
        }),
    }
}

/// Load comms configuration from all sources.
///
/// Priority (highest to lowest):
/// 1. CLI flag overrides
/// 2. Project config (.rkat/config.toml)
/// 3. User config (~/.rkat/config.toml)
/// 4. Defaults
///
/// Returns (config, base_dir) where base_dir is the directory to resolve
/// relative paths from (project root if project config exists, else cwd).
pub fn load_comms_config(overrides: &CommsOverrides) -> (Option<CoreCommsConfig>, PathBuf) {
    // Load user config
    let user_config = user_config_path().and_then(|p| load_config_file(&p));

    // Load project config
    let project_path = project_config_path();
    let project_config = load_config_file(&project_path);

    // Determine base directory for path resolution
    let base_dir = if project_path.exists() {
        // If project config exists, use project root (parent of .rkat)
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    // Merge comms sections (project overrides user)
    let merged_section = merge_comms_section(
        user_config.and_then(|c| c.comms),
        project_config.and_then(|c| c.comms),
    );

    // Convert to CoreCommsConfig if section exists
    let config = merged_section.map(|s| {
        let core = s.to_core_config();
        // Apply CLI overrides
        overrides.apply(core)
    });

    // If no config section but overrides enable comms, create default config with overrides
    let config = config.or_else(|| {
        if overrides.name.is_some() || overrides.listen_tcp.is_some() {
            Some(overrides.apply(CoreCommsConfig::default()))
        } else {
            None
        }
    });

    (config, base_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cli_config_has_comms() {
        // Verify CliConfig has comms field
        let config = CliConfig::default();
        assert!(config.comms.is_none());

        let config_with_comms = CliConfig {
            comms: Some(CommsConfigSection {
                enabled: Some(true),
                name: Some("test".to_string()),
                listen_uds: None,
                listen_tcp: None,
                identity_dir: None,
                trusted_peers_path: None,
                ack_timeout_secs: None,
                max_message_bytes: None,
            }),
        };
        assert!(config_with_comms.comms.is_some());
    }

    #[test]
    fn test_cli_loads_comms_from_toml() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join(".rkat");
        std::fs::create_dir_all(&config_dir).unwrap();

        let config_path = config_dir.join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[comms]
enabled = true
name = "alice"
listen_tcp = "127.0.0.1:4200"
ack_timeout_secs = 60
"#,
        )
        .unwrap();

        let config: CliConfig =
            toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();

        assert!(config.comms.is_some());
        let comms = config.comms.unwrap();
        assert_eq!(comms.enabled, Some(true));
        assert_eq!(comms.name, Some("alice".to_string()));
        assert_eq!(comms.listen_tcp, Some("127.0.0.1:4200".to_string()));
        assert_eq!(comms.ack_timeout_secs, Some(60));
    }

    #[test]
    fn test_cli_project_config_overrides() {
        // Test that project config overrides user config
        let user_section = CommsConfigSection {
            enabled: Some(true),
            name: Some("user-agent".to_string()),
            listen_uds: None,
            listen_tcp: Some("127.0.0.1:4200".to_string()),
            identity_dir: None,
            trusted_peers_path: None,
            ack_timeout_secs: Some(30),
            max_message_bytes: None,
        };

        let project_section = CommsConfigSection {
            enabled: None,                           // Inherit from user
            name: Some("project-agent".to_string()), // Override
            listen_uds: None,
            listen_tcp: None,                                    // Inherit from user
            identity_dir: Some(PathBuf::from(".rkat/identity")), // Add new
            trusted_peers_path: None,
            ack_timeout_secs: None, // Inherit from user
            max_message_bytes: None,
        };

        let merged = merge_comms_section(Some(user_section), Some(project_section)).unwrap();

        assert_eq!(merged.enabled, Some(true)); // Inherited
        assert_eq!(merged.name, Some("project-agent".to_string())); // Overridden
        assert_eq!(merged.listen_tcp, Some("127.0.0.1:4200".to_string())); // Inherited
        assert_eq!(merged.identity_dir, Some(PathBuf::from(".rkat/identity"))); // Added
        assert_eq!(merged.ack_timeout_secs, Some(30)); // Inherited
    }

    #[test]
    fn test_cli_comms_name_flag() {
        let overrides = CommsOverrides {
            name: Some("cli-agent".to_string()),
            listen_tcp: None,
            disabled: false,
        };

        let config = CoreCommsConfig::default();
        let result = overrides.apply(config);

        assert!(result.enabled); // Enabled because name was set
        assert_eq!(result.name, "cli-agent");
    }

    #[test]
    fn test_cli_comms_listen_tcp_flag() {
        let overrides = CommsOverrides {
            name: None,
            listen_tcp: Some("0.0.0.0:5000".parse().unwrap()),
            disabled: false,
        };

        let config = CoreCommsConfig::default();
        let result = overrides.apply(config);

        assert!(result.enabled); // Enabled because listen_tcp was set
        assert_eq!(result.listen_tcp, Some("0.0.0.0:5000".parse().unwrap()));
    }

    #[test]
    fn test_cli_no_comms_flag() {
        let overrides = CommsOverrides {
            name: Some("agent".to_string()),
            listen_tcp: Some("0.0.0.0:5000".parse().unwrap()),
            disabled: true, // --no-comms flag
        };

        let config = CoreCommsConfig {
            enabled: true,
            ..Default::default()
        };
        let result = overrides.apply(config);

        assert!(!result.enabled); // Disabled by flag
    }

    #[test]
    fn test_cli_resolves_comms_paths() {
        // Test that to_core_config properly converts paths
        let section = CommsConfigSection {
            enabled: Some(true),
            name: Some("test".to_string()),
            listen_uds: Some(PathBuf::from("/tmp/test.sock")),
            listen_tcp: Some("127.0.0.1:4200".to_string()),
            identity_dir: Some(PathBuf::from(".rkat/identity")),
            trusted_peers_path: Some(PathBuf::from(".rkat/peers.json")),
            ack_timeout_secs: Some(30),
            max_message_bytes: Some(500_000),
        };

        let config = section.to_core_config();

        assert!(config.enabled);
        assert_eq!(config.name, "test");
        assert_eq!(config.listen_uds, Some(PathBuf::from("/tmp/test.sock")));
        assert_eq!(config.listen_tcp, Some("127.0.0.1:4200".parse().unwrap()));
        assert_eq!(config.identity_dir, PathBuf::from(".rkat/identity"));
        assert_eq!(config.trusted_peers_path, PathBuf::from(".rkat/peers.json"));
        assert_eq!(config.ack_timeout_secs, 30);
        assert_eq!(config.max_message_bytes, 500_000);
    }

    #[test]
    fn test_comms_config_section_serde() {
        let toml_str = r#"
enabled = true
name = "bob"
listen_uds = "/tmp/bob.sock"
listen_tcp = "0.0.0.0:4200"
identity_dir = ".rkat/bob/identity"
trusted_peers_path = ".rkat/bob/peers.json"
ack_timeout_secs = 45
max_message_bytes = 2000000
"#;

        let section: CommsConfigSection = toml::from_str(toml_str).unwrap();
        assert_eq!(section.enabled, Some(true));
        assert_eq!(section.name, Some("bob".to_string()));
        assert_eq!(section.listen_uds, Some(PathBuf::from("/tmp/bob.sock")));
        assert_eq!(section.listen_tcp, Some("0.0.0.0:4200".to_string()));
        assert_eq!(section.ack_timeout_secs, Some(45));
        assert_eq!(section.max_message_bytes, Some(2000000));
    }
}
