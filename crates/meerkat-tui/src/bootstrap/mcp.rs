//! The MCP servers of the TUI's sessions.
//!
//! The effective set is upstream's one owner of it,
//! [`McpConfig::effective_servers_from_roots`]: the realm config's
//! `[[tools.mcp_servers]]` plus the project `.rkat/mcp.toml` (at the
//! workspace) and the user `~/.rkat/mcp.toml` (at the user-config root),
//! project winning over user by name. It is loaded once at start-up and
//! handed to every session build as its declarative servers
//! (`SessionBuildOptions::mcp_servers`): the factory then builds the
//! session-owned router, connects the servers in the background (the model
//! sees the native `[MCP_PENDING]` notice while they connect) and, with
//! `--wait-for-mcp` (`FactoryAgentBuilder::wait_for_mcp`), waits for them
//! before the first turn.
//!
//! Every MCP tool call passes the approval gate like any other tool: an MCP
//! tool is not a builtin the policy knows, so it reaches the classifier
//! unless a user rule allows or denies it by name.
//!
//! Local stdio servers run as the trusted host (see
//! [`super::confinement`]). The project `mcp.toml` lives in the workspace:
//! it may have come with the repository, and a confined shell can write it
//! on Linux. Its servers therefore start only when the project file is
//! trusted ([`ProjectMcpTrust`]): `--allow-project-mcp` for this run, or the
//! host trust record ([`crate::approval::trust::HostTrustRecord`]) holding
//! the file's digest after `/mcp trust`. Otherwise upstream's trusting
//! loader ([`McpConfig::effective_servers_from_roots_trusting`]) refuses
//! each of its servers (unless a trusted source states the same definition)
//! and a start-up notice lists them.

use std::path::{Path, PathBuf};

use meerkat_core::mcp_config::{McpConfig, McpScope, McpServerSource, project_mcp_path_in};
use meerkat_core::{Config, McpServerConfig};

use super::scope::RuntimeScope;
use crate::approval::trust::{HostTrustRecord, TrustStatus};
use crate::error::TuiError;

/// Whether the project `.rkat/mcp.toml` may start servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMcpTrust {
    /// `--allow-project-mcp`: trusted for this run.
    AllowedByFlag,
    /// Its digest is in the host trust record (`/mcp trust`).
    Recorded,
    /// Not trusted: its servers are refused.
    Untrusted,
}

impl ProjectMcpTrust {
    /// The trust of `scope`'s project file under `record` (`flag`:
    /// `--allow-project-mcp`).
    #[must_use]
    pub fn of(scope: &RuntimeScope, record: &HostTrustRecord, flag: bool) -> Self {
        if flag {
            return Self::AllowedByFlag;
        }
        match record.check(&project_file(scope)) {
            TrustStatus::Trusted => Self::Recorded,
            TrustStatus::Missing | TrustStatus::Untrusted(_) => Self::Untrusted,
        }
    }

    fn trusted(self) -> bool {
        self != Self::Untrusted
    }
}

/// The project MCP config of `scope` (`<workspace>/.rkat/mcp.toml`).
#[must_use]
pub fn project_file(scope: &RuntimeScope) -> PathBuf {
    project_mcp_path_in(&scope.context_root)
}

/// The loaded MCP set and what to tell the user about it.
#[derive(Debug, Default)]
pub struct LoadedMcp {
    /// Servers every session build carries.
    pub servers: Vec<McpServerConfig>,
    /// Start-up notices (project-scoped servers).
    pub notices: Vec<String>,
}

/// Load the effective MCP set for `scope` (see the module docs): the
/// project file's servers only when `project` trusts it. Without the `mcp`
/// feature the set is empty and a configured server is reported.
///
/// # Errors
///
/// [`TuiError::McpConfig`] when an `mcp.toml` cannot be read or parsed, or the
/// realm config and a file define one server name differently.
pub async fn load(
    scope: &RuntimeScope,
    config: &Config,
    project: ProjectMcpTrust,
) -> Result<LoadedMcp, TuiError> {
    let project_path = project_file(scope);
    let untrusted = move |path: &Path| !project.trusted() && path == project_path;
    let effective = McpConfig::effective_servers_from_roots_trusting(
        &config.tools.mcp_servers,
        Some(scope.context_root.as_path()),
        scope.user_config_root.as_deref(),
        &untrusted,
    )
    .await?;
    let servers = effective.admitted;
    let refused: Vec<&str> = effective
        .refused
        .iter()
        .map(|r| r.server.name.as_str())
        .collect();
    let project: Vec<&str> = servers
        .iter()
        .filter(|s| s.source == McpServerSource::File(McpScope::Project))
        .map(|s| s.server.name.as_str())
        .collect();
    let mut notices = Vec::new();
    if !refused.is_empty() {
        notices.push(format!(
            "MCP servers of the project's .rkat/mcp.toml not started (the file is not trusted; /mcp trust or --allow-project-mcp starts them): {}",
            refused.join(", ")
        ));
    }
    if !cfg!(feature = "mcp") {
        if !servers.is_empty() {
            notices.push(format!(
                "{} MCP server(s) configured, but this build has no MCP support (feature `mcp`)",
                servers.len()
            ));
        }
        return Ok(LoadedMcp {
            servers: Vec::new(),
            notices,
        });
    }
    if !project.is_empty() {
        notices.push(format!(
            "MCP servers from the project's .rkat/mcp.toml start with your privileges: {}",
            project.join(", ")
        ));
    }
    Ok(LoadedMcp {
        servers: servers.into_iter().map(|s| s.server).collect(),
        notices,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;

    #[tokio::test]
    async fn project_and_user_files_compose() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let ws = base.join("ws");
        let home = base.join("home");
        std::fs::create_dir_all(ws.join(".rkat")).unwrap();
        std::fs::create_dir_all(home.join(".rkat")).unwrap();
        std::fs::write(
            ws.join(".rkat/mcp.toml"),
            "[[servers]]\nname = \"proj\"\ncommand = \"proj-server\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".rkat/mcp.toml"),
            "[[servers]]\nname = \"usr\"\ncommand = \"user-server\"\n",
        )
        .unwrap();
        let args = crate::cli::Args::try_parse_from([
            "rkat-tui",
            "-w",
            ws.to_str().unwrap(),
            "--user-config-root",
            home.to_str().unwrap(),
            "--config-dir",
            base.join("cfg").to_str().unwrap(),
        ])
        .unwrap();
        let scope = super::super::resolve_scope(&args).unwrap();
        let loaded = load(&scope, &Config::default(), ProjectMcpTrust::AllowedByFlag)
            .await
            .unwrap();
        let mut names: Vec<_> = loaded.servers.iter().map(|s| s.name.clone()).collect();
        names.sort();
        if cfg!(feature = "mcp") {
            assert_eq!(names, ["proj", "usr"]);
            assert_eq!(loaded.notices.len(), 1);
            assert!(
                loaded.notices[0].ends_with(": proj"),
                "{:?}",
                loaded.notices
            );
        } else {
            assert!(names.is_empty());
        }

        // Untrusted: the project's servers are refused and listed; the
        // user's still start.
        let mut record = HostTrustRecord::in_memory(&scope.context_root);
        let trust = ProjectMcpTrust::of(&scope, &record, false);
        assert_eq!(trust, ProjectMcpTrust::Untrusted);
        let loaded = load(&scope, &Config::default(), trust).await.unwrap();
        let names: Vec<_> = loaded.servers.iter().map(|s| s.name.clone()).collect();
        if cfg!(feature = "mcp") {
            assert_eq!(names, ["usr"]);
            assert!(
                loaded
                    .notices
                    .iter()
                    .any(|n| n.contains("not trusted") && n.ends_with(": proj")),
                "{:?}",
                loaded.notices
            );
        }
        // `/mcp trust` records the digest: trusted until the file changes.
        record.record_file(&project_file(&scope)).unwrap();
        assert_eq!(
            ProjectMcpTrust::of(&scope, &record, false),
            ProjectMcpTrust::Recorded
        );
        std::fs::write(ws.join(".rkat/mcp.toml"), "not toml [").unwrap();
        assert_eq!(
            ProjectMcpTrust::of(&scope, &record, false),
            ProjectMcpTrust::Untrusted
        );
        assert!(matches!(
            load(&scope, &Config::default(), ProjectMcpTrust::AllowedByFlag).await,
            Err(TuiError::McpConfig(_))
        ));
    }
}
