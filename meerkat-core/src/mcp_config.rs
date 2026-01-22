//! MCP server configuration loading and management
//!
//! Provides file-based MCP server configuration with two scopes:
//! - `project`: `.rkat/mcp.toml` (searched upward from cwd) - local, shared in repo
//! - `user`: `~/.rkat/mcp.toml` - global, personal
//!
//! Precedence: project > user (project wins on name collision)

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// MCP configuration containing server definitions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// Transport kind for MCP servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
    Sse,
}

impl McpTransportKind {
    pub fn default_for_http() -> Self {
        McpTransportKind::StreamableHttp
    }
}

/// Stdio transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpStdioConfig {
    /// Command to spawn the server
    pub command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// HTTP transport configuration (streamable HTTP or legacy SSE)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpHttpConfig {
    /// Server URL
    pub url: String,
    /// Extra headers to include on requests
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// HTTP transport selection (default: streamable-http)
    #[serde(default)]
    pub transport: Option<McpHttpTransport>,
}

/// HTTP transport selection for URL-based servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpHttpTransport {
    StreamableHttp,
    Sse,
}

impl Default for McpHttpTransport {
    fn default() -> Self {
        McpHttpTransport::StreamableHttp
    }
}

/// MCP server transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpTransportConfig {
    Stdio(McpStdioConfig),
    Http(McpHttpConfig),
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (must be unique within scope)
    pub name: String,
    /// Transport configuration (stdio or HTTP)
    #[serde(flatten)]
    pub transport: McpTransportConfig,
}

impl McpServerConfig {
    pub fn stdio(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                command: command.into(),
                args,
                env,
            }),
        }
    }

    pub fn streamable_http(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: McpTransportConfig::Http(McpHttpConfig {
                url: url.into(),
                headers,
                transport: None,
            }),
        }
    }

    pub fn sse(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: McpTransportConfig::Http(McpHttpConfig {
                url: url.into(),
                headers,
                transport: Some(McpHttpTransport::Sse),
            }),
        }
    }

    pub fn transport_kind(&self) -> McpTransportKind {
        match &self.transport {
            McpTransportConfig::Stdio(_) => McpTransportKind::Stdio,
            McpTransportConfig::Http(http) => match http.transport.unwrap_or_default() {
                McpHttpTransport::StreamableHttp => McpTransportKind::StreamableHttp,
                McpHttpTransport::Sse => McpTransportKind::Sse,
            },
        }
    }
}

/// Scope for MCP server configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScope {
    /// User-level config: ~/.rkat/mcp.toml
    User,
    /// Project-level config: .rkat/mcp.toml (searched upward)
    Project,
}

/// MCP server with its source scope
#[derive(Debug, Clone)]
pub struct McpServerWithScope {
    pub server: McpServerConfig,
    pub scope: McpScope,
}

/// Errors that can occur during MCP config operations
#[derive(Debug, thiserror::Error)]
pub enum McpConfigError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error in {path}: {message}")]
    Parse { path: String, message: String },
    #[error("Server '{0}' already exists. Remove it first with: rkat mcp remove {0}")]
    ServerExists(String),
    #[error("Server '{0}' not found")]
    ServerNotFound(String),
    #[error("Server '{name}' exists in multiple scopes. Specify --scope: {scopes:?}")]
    AmbiguousServer { name: String, scopes: Vec<McpScope> },
    #[error("Missing environment variable '{var}' referenced in {field}")]
    MissingEnvVar { field: String, var: String },
    #[error("Invalid environment variable reference in {field}: '{value}'")]
    InvalidEnvVarSyntax { field: String, value: String },
}

impl McpConfig {
    /// Load from user + project config, project wins on name collision.
    /// Returns servers in precedence order (project first, then user).
    pub fn load() -> Result<Self, McpConfigError> {
        let user = user_mcp_path();
        let project = find_project_mcp();

        let user_cfg = read_mcp_file(user.as_deref())?;
        let project_cfg = read_mcp_file(project.as_deref())?;

        Ok(merge_project_over_user(user_cfg, project_cfg))
    }

    /// Load from explicit paths (useful for testing)
    pub fn load_from_paths(
        user_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<Self, McpConfigError> {
        let user_cfg = read_mcp_file(user_path)?;
        let project_cfg = read_mcp_file(project_path)?;
        Ok(merge_project_over_user(user_cfg, project_cfg))
    }

    /// Load servers with their scope information
    pub fn load_with_scopes() -> Result<Vec<McpServerWithScope>, McpConfigError> {
        let user_path = user_mcp_path();
        let project_path = find_project_mcp();

        let user_cfg = read_mcp_file(user_path.as_deref())?;
        let project_cfg = read_mcp_file(project_path.as_deref())?;

        let mut seen: HashSet<String> = HashSet::new();
        let mut result: Vec<McpServerWithScope> = Vec::new();

        // Project first (highest precedence)
        for server in project_cfg.servers {
            if seen.insert(server.name.clone()) {
                result.push(McpServerWithScope {
                    server,
                    scope: McpScope::Project,
                });
            }
        }

        // Then user servers not shadowed by project
        for server in user_cfg.servers {
            if seen.insert(server.name.clone()) {
                result.push(McpServerWithScope {
                    server,
                    scope: McpScope::User,
                });
            }
        }

        Ok(result)
    }

    /// Load from a specific scope only
    pub fn load_scope(scope: McpScope) -> Result<Self, McpConfigError> {
        let path = match scope {
            McpScope::User => user_mcp_path(),
            McpScope::Project => find_project_mcp(),
        };
        read_mcp_file(path.as_deref())
    }

    /// Check if a server exists in a specific scope
    pub fn server_exists(name: &str, scope: McpScope) -> Result<bool, McpConfigError> {
        let config = Self::load_scope(scope)?;
        Ok(config.servers.iter().any(|s| s.name == name))
    }

    /// Find which scopes contain a server with the given name
    pub fn find_server_scopes(name: &str) -> Result<Vec<McpScope>, McpConfigError> {
        let mut scopes = Vec::new();

        if Self::server_exists(name, McpScope::Project)? {
            scopes.push(McpScope::Project);
        }
        if Self::server_exists(name, McpScope::User)? {
            scopes.push(McpScope::User);
        }

        Ok(scopes)
    }
}

fn read_mcp_file(path: Option<&Path>) -> Result<McpConfig, McpConfigError> {
    let Some(path) = path else {
        return Ok(McpConfig::default());
    };
    if !path.exists() {
        return Ok(McpConfig::default());
    }
    let contents =
        fs::read_to_string(path).map_err(|e| McpConfigError::Io(e.to_string()))?;
    let parsed: McpConfig = toml::from_str(&contents).map_err(|e| McpConfigError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    expand_env_in_config(parsed)
}

fn merge_project_over_user(user: McpConfig, project: McpConfig) -> McpConfig {
    let mut seen: HashSet<String> = HashSet::new();
    let mut merged: Vec<McpServerConfig> = Vec::new();

    // Project first (highest precedence, local-first)
    for server in project.servers {
        if seen.insert(server.name.clone()) {
            merged.push(server);
        }
    }

    // Then user servers not shadowed by project
    for server in user.servers {
        if seen.insert(server.name.clone()) {
            merged.push(server);
        }
    }

    McpConfig { servers: merged }
}

fn expand_env_in_config(config: McpConfig) -> Result<McpConfig, McpConfigError> {
    let mut servers = Vec::with_capacity(config.servers.len());
    for server in config.servers {
        servers.push(expand_env_in_server(server)?);
    }
    Ok(McpConfig { servers })
}

fn expand_env_in_server(server: McpServerConfig) -> Result<McpServerConfig, McpConfigError> {
    let transport = match server.transport {
        McpTransportConfig::Stdio(stdio) => {
            let command = expand_env_in_string(&stdio.command, "servers[].command")?;
            let args = stdio
                .args
                .into_iter()
                .map(|arg| expand_env_in_string(&arg, "servers[].args"))
                .collect::<Result<Vec<_>, _>>()?;
            let env = expand_env_in_map(stdio.env, "servers[].env")?;
            McpTransportConfig::Stdio(McpStdioConfig { command, args, env })
        }
        McpTransportConfig::Http(http) => {
            let url = expand_env_in_string(&http.url, "servers[].url")?;
            let headers = expand_env_in_map(http.headers, "servers[].headers")?;
            McpTransportConfig::Http(McpHttpConfig {
                url,
                headers,
                transport: http.transport,
            })
        }
    };
    Ok(McpServerConfig {
        name: server.name,
        transport,
    })
}

fn expand_env_in_map(
    map: HashMap<String, String>,
    field: &str,
) -> Result<HashMap<String, String>, McpConfigError> {
    let mut expanded = HashMap::with_capacity(map.len());
    for (key, value) in map {
        let value = expand_env_in_string(&value, field)?;
        expanded.insert(key, value);
    }
    Ok(expanded)
}

fn expand_env_in_string(value: &str, field: &str) -> Result<String, McpConfigError> {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(McpConfigError::InvalidEnvVarSyntax {
                field: field.to_string(),
                value: value.to_string(),
            });
        };
        let var_name = &after[..end];
        if var_name.is_empty() {
            return Err(McpConfigError::InvalidEnvVarSyntax {
                field: field.to_string(),
                value: value.to_string(),
            });
        }
        let var_value = std::env::var(var_name).map_err(|_| McpConfigError::MissingEnvVar {
            field: field.to_string(),
            var: var_name.to_string(),
        })?;
        output.push_str(&var_value);
        remaining = &after[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

// === Path helpers ===

/// Get the user-level MCP config path: ~/.rkat/mcp.toml
pub fn user_mcp_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".rkat/mcp.toml"))
}

/// Get the user-level MCP config directory: ~/.rkat/
pub fn user_mcp_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".rkat"))
}

/// Find project-level MCP config in cwd only: ./.rkat/mcp.toml
/// Does NOT walk up the directory tree for security reasons.
pub fn find_project_mcp() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_mcp_in(&cwd)
}

/// Find project-level MCP config in a specific directory only.
/// Does NOT walk up the directory tree for security reasons.
pub fn find_project_mcp_in(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(".rkat/mcp.toml");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Get the project MCP config path for the current directory (creates path even if doesn't exist)
pub fn project_mcp_path() -> Option<PathBuf> {
    std::env::current_dir().ok().map(|cwd| cwd.join(".rkat/mcp.toml"))
}

/// Get the project MCP config directory for the current directory
pub fn project_mcp_dir() -> Option<PathBuf> {
    std::env::current_dir().ok().map(|cwd| cwd.join(".rkat"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl std::fmt::Display for McpScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpScope::User => write!(f, "user"),
            McpScope::Project => write!(f, "project"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_empty_config_loads() {
        let config = McpConfig::load_from_paths(None, None).unwrap();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_parse_mcp_toml() {
        let toml = r#"
[[servers]]
name = "test-server"
command = "npx"
args = ["-y", "@test/mcp-server"]
env = { API_KEY = "secret" }

[[servers]]
name = "remote-server"
url = "https://example.com/mcp"
headers = { Authorization = "Bearer token" }
"#;
        let config: McpConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].name, "test-server");
        match &config.servers[0].transport {
            McpTransportConfig::Stdio(stdio) => {
                assert_eq!(stdio.command, "npx");
                assert_eq!(stdio.args, vec!["-y", "@test/mcp-server"]);
                assert_eq!(stdio.env.get("API_KEY"), Some(&"secret".to_string()));
            }
            _ => panic!("Expected stdio transport"),
        }
        assert_eq!(config.servers[1].name, "remote-server");
        match &config.servers[1].transport {
            McpTransportConfig::Http(http) => {
                assert_eq!(http.url, "https://example.com/mcp");
                assert_eq!(
                    http.headers.get("Authorization"),
                    Some(&"Bearer token".to_string())
                );
            }
            _ => panic!("Expected http transport"),
        }
    }

    #[test]
    fn test_merge_project_over_user() {
        let user = McpConfig {
            servers: vec![
                McpServerConfig::stdio("shared", "user-cmd", vec![], HashMap::new()),
                McpServerConfig::stdio("user-only", "user-only-cmd", vec![], HashMap::new()),
            ],
        };

        let project = McpConfig {
            servers: vec![
                McpServerConfig::stdio("shared", "project-cmd", vec![], HashMap::new()),
                McpServerConfig::stdio("project-only", "project-only-cmd", vec![], HashMap::new()),
            ],
        };

        let merged = merge_project_over_user(user, project);

        // Project servers first, then user-only
        assert_eq!(merged.servers.len(), 3);
        assert_eq!(merged.servers[0].name, "shared");
        match &merged.servers[0].transport {
            McpTransportConfig::Stdio(stdio) => {
                assert_eq!(stdio.command, "project-cmd"); // Project wins
            }
            _ => panic!("Expected stdio transport"),
        }
        assert_eq!(merged.servers[1].name, "project-only");
        assert_eq!(merged.servers[2].name, "user-only");
    }

    #[test]
    fn test_load_from_files() {
        let temp = TempDir::new().unwrap();

        let user_dir = temp.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        let user_file = user_dir.join("mcp.toml");
        fs::write(&user_file, r#"
[[servers]]
name = "user-server"
command = "user-cmd"
"#).unwrap();

        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        let project_file = project_dir.join("mcp.toml");
        fs::write(&project_file, r#"
[[servers]]
name = "project-server"
command = "project-cmd"
"#).unwrap();

        let config = McpConfig::load_from_paths(
            Some(&user_file),
            Some(&project_file),
        ).unwrap();

        assert_eq!(config.servers.len(), 2);
        // Project first
        assert_eq!(config.servers[0].name, "project-server");
        assert_eq!(config.servers[1].name, "user-server");
    }

    #[test]
    fn test_find_project_mcp_does_not_walk_up_tree() {
        let temp = TempDir::new().unwrap();

        // Create parent/.rkat/mcp.toml (should NOT be found)
        let parent_config = temp.path().join(".rkat");
        fs::create_dir_all(&parent_config).unwrap();
        fs::write(parent_config.join("mcp.toml"), r#"
[[servers]]
name = "parent-server"
command = "should-not-load"
"#).unwrap();

        // Create parent/child/ directory (no .rkat here)
        let child_dir = temp.path().join("child");
        fs::create_dir_all(&child_dir).unwrap();

        // Looking from child/ should NOT find parent/.rkat/mcp.toml
        let result = find_project_mcp_in(&child_dir);
        assert!(result.is_none(), "Should not find config in parent directory");

        // But looking from parent/ should find it
        let result = find_project_mcp_in(temp.path());
        assert!(result.is_some(), "Should find config in current directory");
    }

    #[test]
    fn test_find_project_mcp_finds_config_in_current_dir() {
        let temp = TempDir::new().unwrap();

        // Create .rkat/mcp.toml in the directory
        let meerkat_dir = temp.path().join(".rkat");
        fs::create_dir_all(&meerkat_dir).unwrap();
        let config_path = meerkat_dir.join("mcp.toml");
        fs::write(&config_path, r#"
[[servers]]
name = "local-server"
command = "echo"
"#).unwrap();

        let result = find_project_mcp_in(temp.path());
        assert_eq!(result, Some(config_path));
    }

    #[test]
    fn test_http_transport_defaults_to_streamable() {
        let toml = r#"
[[servers]]
name = "remote"
url = "https://mcp.example.com/mcp"
"#;
        let config: McpConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].transport_kind(), McpTransportKind::StreamableHttp);
    }

    #[test]
    fn test_http_transport_sse() {
        let toml = r#"
[[servers]]
name = "legacy"
url = "https://old.example.com/sse"
transport = "sse"
"#;
        let config: McpConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].transport_kind(), McpTransportKind::Sse);
    }

    #[test]
    fn test_rejects_conflicting_transport_fields() {
        let toml = r#"
[[servers]]
name = "invalid"
command = "cmd"
url = "https://example.com/mcp"
"#;
        let parsed: Result<McpConfig, _> = toml::from_str(toml);
        assert!(parsed.is_err(), "Config with command + url should fail");
    }

    #[test]
    fn test_env_expansion_in_config() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("mcp.toml");
        unsafe {
            std::env::set_var("RKAT_TEST_API_KEY", "secret");
        }
        fs::write(
            &config_path,
            r#"
[[servers]]
name = "remote"
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${RKAT_TEST_API_KEY}" }
"#,
        )
        .unwrap();

        let config = McpConfig::load_from_paths(Some(&config_path), None).unwrap();
        let server = &config.servers[0];
        match &server.transport {
            McpTransportConfig::Http(http) => {
                assert_eq!(
                    http.headers.get("Authorization"),
                    Some(&"Bearer secret".to_string())
                );
            }
            _ => panic!("Expected http transport"),
        }
        unsafe {
            std::env::remove_var("RKAT_TEST_API_KEY");
        }
    }
}
