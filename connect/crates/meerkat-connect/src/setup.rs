//! Compose the selected setup server through native MCP and native tool policy.
//!
//! The resource preparer owns artifact verification and command construction.
//! This module accepts only its one-server handoff, never agent-selected commands.

use meerkat_core::{
    McpServerConfig,
    mcp_config::McpStdioConfig,
    ops::ToolAccessPolicy,
    tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME},
};
use serde::Deserialize;

use crate::{
    ConnectError, SessionOptions,
    paired_host_tool::{PAIRED_HOST_FRONTEND_READ_TOOLS, PairedHostFrontend},
};

#[cfg(feature = "console")]
mod mobkit_helper;
#[cfg(feature = "console")]
pub use mobkit_helper::{PreparedSetupHelper, SetupHelperTools};

const CONFIG_LIMIT: usize = 256 * 1024;
pub(crate) const SERVER_NAME: &str = "toolkit-setup";

// These are the existing adapter's operation names, not catalog/profile choices.
// Preparation and Stop are mutations of local setup work, not read-only calls.
pub(crate) const PREPARATION_TOOLS: &[&str] = &[
    "setup_state",
    "setup_jobs",
    "setup_job",
    "setup_catalog",
    "setup_catalog_entry",
    "setup_catalog_route",
    "setup_prepare",
    "setup_retry_prepare",
    "setup_help",
    "setup_check",
    "setup_stop",
    "setup_reconcile",
    "setup_recovery_manifest",
    "setup_guide",
    "setup_inspect_package",
    "setup_inspect_schema",
];

// Host-selected service preparation may research and retain inert Python/npm
// artifacts. These do not widen legacy preparation-only or frontend-only sessions.
pub(crate) const ACQUISITION_TOOLS: &[&str] = &[
    "setup_acquire_python",
    "setup_search_node",
    "setup_acquire_node",
    "setup_acquire_node_prebuilt",
    "setup_inspect_acquired",
    "setup_recipes",
    "setup_recipe",
    "setup_remember_remote",
    "setup_export_recipe",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    #[serde(rename = "mcpServers")]
    servers: SetupServer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupServer {
    #[serde(rename = "toolkit-setup")]
    setup: McpStdioConfig,
}

/// The one setup MCP command emitted by the verified product resource preparer.
/// Parsing is structural validation, not authority to execute arbitrary bytes.
pub struct PreparedSetupConnection {
    server: McpServerConfig,
}

impl PreparedSetupConnection {
    pub(crate) fn into_server(self) -> McpServerConfig {
        self.server
    }

    /// The caller must obtain these bytes from the selected resource preparer.
    /// Unknown/duplicate fields, extra servers and non-stdio transports refuse.
    pub fn from_preparer_output(bytes: &[u8]) -> Result<Self, ConnectError> {
        if bytes.len() > CONFIG_LIMIT {
            return Err(ConnectError::InvalidInput(
                "Prepared setup configuration exceeds the supported limit".into(),
            ));
        }
        let configuration: Configuration = serde_json::from_slice(bytes)
            .map_err(|e| ConnectError::native("read prepared setup connection", e))?;
        let server = configuration.servers.setup;
        // The accepted preparer emits an absolute executable and environment
        // clearing argv. It never supplies native credential environment values.
        if !std::path::Path::new(&server.command).is_absolute()
            || server.command.contains('\0')
            || server.args.iter().any(|argument| argument.contains('\0'))
            || !server.env.is_empty()
        {
            return Err(ConnectError::InvalidInput(
                "Prepared setup connection has unsupported execution inputs".into(),
            ));
        }
        Ok(Self {
            server: McpServerConfig::stdio(SERVER_NAME, server.command, server.args, server.env),
        })
    }

    /// Enable catalog, preparation, configuration checks and local job controls.
    /// Installation stays refused until the supported install and lifecycle
    /// composition qualifies. Caller ceilings remain in the native conjunction.
    pub fn preparation_session(
        self,
        options: SessionOptions,
    ) -> Result<SessionOptions, ConnectError> {
        self.compose(options, None)
    }

    /// Compose the same preparation tools plus exactly this captured PairedHost
    /// inventory/candidate binding. Caller restrictions are conjoined, never replaced.
    pub fn preparation_session_with_paired_host(
        self,
        options: SessionOptions,
        paired_host: &PairedHostFrontend,
    ) -> Result<SessionOptions, ConnectError> {
        self.compose(options, Some(paired_host))
    }

    /// One original setup session with the selected stdio preparation server
    /// and this exact local PairedHost binding. Only PairedHost effects receive
    /// per-call consent; service installation remains outside the tool ceiling.
    pub fn interactive_session_with_paired_host(
        self,
        options: SessionOptions,
        paired_host: &PairedHostFrontend,
    ) -> Result<SessionOptions, ConnectError> {
        let mut allowed = paired_host.allowed_tools();
        allowed.extend(PREPARATION_TOOLS.iter().copied());
        allowed.extend([TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME]);
        let mut options = compose_paired_host_session(options, paired_host, allowed)?;
        options.build.mcp_servers.push(self.server);
        Ok(options)
    }

    fn compose(
        self,
        mut options: SessionOptions,
        paired_host: Option<&PairedHostFrontend>,
    ) -> Result<SessionOptions, ConnectError> {
        if !options.build.mcp_servers.is_empty() || options.build.external_tools.is_some() {
            return Err(ConnectError::InvalidInput(
                "The packaged setup session accepts only its selected setup server".into(),
            ));
        }
        let allowed = PREPARATION_TOOLS
            .iter()
            .copied()
            .chain([TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME])
            .chain(
                paired_host
                    .into_iter()
                    .flat_map(|_| PAIRED_HOST_FRONTEND_READ_TOOLS.iter().copied()),
            )
            .collect();
        let ceiling = ToolAccessPolicy::AllowList(allowed);
        let policy = match options.build.tool_access_policy.take() {
            Some(existing) => existing
                .conjoin(ceiling)
                .map_err(|e| ConnectError::native("compose setup tool policy", e))?,
            None => ceiling,
        };
        options.build.tool_access_policy = Some(policy);
        options.build.mcp_servers.push(self.server);
        if let Some(paired_host) = paired_host {
            options.build.external_tools = Some(paired_host.dispatcher());
        }
        Ok(options)
    }
}

/// Compose the captured host directly, without locating or launching service
/// integration resources. Optional research uses the same ordinary admission.
pub fn paired_host_frontend_session(
    options: SessionOptions,
    paired_host: &PairedHostFrontend,
) -> Result<SessionOptions, ConnectError> {
    compose_paired_host_session(options, paired_host, paired_host.allowed_tools())
}

pub(crate) fn lazy_setup_session(
    options: SessionOptions,
    lazy: &crate::service_preparation::LazySetup,
) -> Result<SessionOptions, ConnectError> {
    let mut options =
        compose_paired_host_session(options, lazy.paired_host(), lazy.allowed_tools())?;
    options.build.external_tools = Some(lazy.dispatcher());
    Ok(options)
}

pub(crate) fn compose_paired_host_session<S: Into<String>>(
    mut options: SessionOptions,
    paired_host: &PairedHostFrontend,
    allowed: Vec<S>,
) -> Result<SessionOptions, ConnectError> {
    if !options.build.mcp_servers.is_empty()
        || options.build.external_tools.is_some()
        || options.build.tool_dispatch_admission.is_some()
        || options.build.resume_session.is_some()
        || options.build.recoverable_tool_defs.is_some()
        || options.build.fork_source.is_some()
        || options.build.mob_member_binding.is_some()
        || options.build.mob_tools.is_some()
        || options.build.schedule_tools.is_some()
        || options.build.workgraph_tools.is_some()
        || options.build.session_comms_runtime_override.is_some()
        || options.build.comms_name.is_some()
        || options.build.keep_alive
    {
        return Err(ConnectError::InvalidInput(
            "Direct PairedHost setup requires a fresh local session without other execution surfaces"
                .into(),
        ));
    }
    let ceiling = ToolAccessPolicy::AllowList(
        allowed
            .into_iter()
            .map(|name| -> String { name.into() })
            .collect(),
    );
    options.build.tool_access_policy = Some(match options.build.tool_access_policy.take() {
        Some(existing) => existing
            .conjoin(ceiling)
            .map_err(|e| ConnectError::native("compose PairedHost tool policy", e))?,
        None => ceiling,
    });
    use meerkat_core::ToolCategoryOverride::Disable;
    options.build.override_builtins = Disable;
    options.build.override_shell = Disable;
    options.build.override_comms = Disable;
    options.build.override_memory = Disable;
    options.build.override_schedule = Disable;
    options.build.override_workgraph = Disable;
    options.build.override_mob = Disable;
    options.build.override_image_generation = Disable;
    options.build.override_web_search = Disable;
    options.build.external_tools = Some(paired_host.dispatcher());
    Ok(options)
}

pub(crate) fn is_direct_paired_host_composition(options: &SessionOptions) -> bool {
    options.build.mcp_servers.is_empty() && has_closed_paired_host_execution(options)
}

pub(crate) fn is_interactive_paired_host_composition(options: &SessionOptions) -> bool {
    let servers = &options.build.mcp_servers;
    servers.len() == 1
        && servers[0].name == SERVER_NAME
        && matches!(
            servers[0].transport,
            meerkat_core::mcp_config::McpTransportConfig::Stdio(_)
        )
        && has_closed_paired_host_execution(options)
}

fn has_closed_paired_host_execution(options: &SessionOptions) -> bool {
    use meerkat_core::ToolCategoryOverride::Disable;
    let b = &options.build;
    b.resume_session.is_none()
        && b.recoverable_tool_defs.is_none()
        && b.fork_source.is_none()
        && b.mob_member_binding.is_none()
        && b.mob_tools.is_none()
        && b.schedule_tools.is_none()
        && b.workgraph_tools.is_none()
        && b.session_comms_runtime_override.is_none()
        && b.comms_name.is_none()
        && !b.keep_alive
        && [
            b.override_builtins,
            b.override_shell,
            b.override_comms,
            b.override_memory,
            b.override_schedule,
            b.override_workgraph,
            b.override_mob,
            b.override_image_generation,
            b.override_web_search,
        ]
        .iter()
        .all(|value| *value == Disable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};

    fn connection() -> PreparedSetupConnection {
        PreparedSetupConnection::from_preparer_output(
            br#"{"mcpServers":{"toolkit-setup":{"command":"/usr/bin/env","args":["-i","/product/python","-I","/product/setup.py"]}}}"#,
        )
        .expect("selected preparer output")
    }

    #[test]
    fn refuses_ambiguous_or_extra_execution_inputs() {
        for bytes in [
            br#"{"mcpServers":{"toolkit-setup":{"command":"/ok"},"extra":{"command":"/bad"}}}"#
                .as_slice(),
            br#"{"mcpServers":{"toolkit-setup":{"command":"/ok","command":"/bad"}}}"#,
            br#"{"mcpServers":{"toolkit-setup":{"url":"https://example.com"}}}"#,
            br#"{"mcpServers":{"toolkit-setup":{"command":"python"}}}"#,
            br#"{"mcpServers":{"toolkit-setup":{"command":"/ok","env":{"TOKEN":"secret"}}}}"#,
            br#"{"mcpServers":{"toolkit-setup":{"command":"/ok","tool_names":{}}}}"#,
        ] {
            assert!(PreparedSetupConnection::from_preparer_output(bytes).is_err());
        }
    }

    #[test]
    fn native_policy_preserves_caller_denial_and_refuses_installation() {
        let mut options = SessionOptions::default();
        options.build.tool_access_policy = Some(ToolAccessPolicy::DenyList(
            ["setup_prepare"].into_iter().collect(),
        ));
        let options = connection()
            .preparation_session(options)
            .expect("composition");
        let policy = ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap())
            .expect("native resolved policy");
        assert!(policy.permits("setup_help"));
        assert!(policy.permits(TOOL_CATALOG_LOAD_NAME));
        for name in [
            "setup_prepare",
            "setup_install",
            "setup_selected_install",
            "setup_compose_start",
            "unexpected_tool",
        ] {
            assert!(!policy.permits(name), "{name} must stay refused");
        }
    }

    #[test]
    fn native_read_only_ceiling_still_refuses_preparation_and_stop() {
        let mut options = SessionOptions::default();
        options.build.tool_access_policy = Some(ToolAccessPolicy::ReadOnly);
        let options = connection()
            .preparation_session(options)
            .expect("composition");
        let policy = ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap())
            .expect("native resolved policy");
        assert!(policy.permits_call("setup_catalog", ToolMutationClass::ReadOnly));
        assert!(!policy.permits_call("setup_prepare", ToolMutationClass::Unknown));
        assert!(!policy.permits_call("setup_stop", ToolMutationClass::Unknown));
    }

    #[test]
    fn outside_catalog_metadata_inspection_preserves_native_caller_ceiling() {
        for (caller, permitted) in [
            (None, true),
            (Some(ToolAccessPolicy::ReadOnly), true),
            (
                Some(ToolAccessPolicy::DenyList(
                    ["setup_inspect_package", "setup_inspect_schema"]
                        .into_iter()
                        .collect(),
                )),
                false,
            ),
        ] {
            let mut options = SessionOptions::default();
            options.build.tool_access_policy = caller;
            let options = connection().preparation_session(options).unwrap();
            let policy =
                ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap();
            assert_eq!(
                policy.permits_call("setup_inspect_package", ToolMutationClass::ReadOnly),
                permitted
            );
            assert_eq!(
                policy.permits_call("setup_inspect_schema", ToolMutationClass::ReadOnly),
                permitted
            );
            for name in [
                "setup_acquire_python",
                "setup_acquire_node",
                "setup_acquire_node_prebuilt",
                "setup_selected_check",
                "setup_selected_install",
                "setup_remember_remote",
                "setup_export_recipe",
            ] {
                assert!(!policy.permits(name), "{name} must stay refused");
            }
        }
    }

    #[test]
    fn extra_server_is_not_silently_adopted() {
        let mut options = SessionOptions::default();
        options.build.mcp_servers.push(McpServerConfig::stdio(
            "other",
            "/unselected",
            Vec::new(),
            Default::default(),
        ));
        assert!(connection().preparation_session(options).is_err());
    }
}
