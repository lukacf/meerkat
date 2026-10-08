//! Mechanical adaptation of a retained host requirement to local MCP launches.
//! This is not caller permission, an authorization policy, or process custody.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use meerkat_core::confinement::{ConfinementRefusal, ExecutionConfinement};
use meerkat_core::mcp_config::McpStdioConfig;
use meerkat_sandbox::{
    CompiledConfinement, ConfinementCapabilityReport, PreparedConfinement, ProcessLaunchSpec,
};

/// Immutable host launch configuration for every local server in one router.
///
/// Required isolation is compiled once and retained, including a setup refusal.
/// Add, reload and direct connection attempts cannot replace it with trusted
/// execution. This value carries no caller authority and is not deserializable
/// from an MCP request or server response. Remote transports are not confined
/// by this local-process profile.
#[derive(Clone)]
pub struct McpStdioLaunchProfile {
    binding: Arc<LaunchBinding>,
}

enum LaunchBinding {
    TrustedHost,
    Required {
        compiled: Result<CompiledConfinement, ConfinementRefusal>,
        working_directory: PathBuf,
    },
}

impl std::fmt::Debug for McpStdioLaunchProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.binding.as_ref() {
            LaunchBinding::TrustedHost => formatter.write_str("McpStdioLaunchProfile::TrustedHost"),
            LaunchBinding::Required { .. } => {
                formatter.write_str("McpStdioLaunchProfile::Required([REDACTED])")
            }
        }
    }
}

impl Default for McpStdioLaunchProfile {
    fn default() -> Self {
        Self::trusted_host()
    }
}

impl McpStdioLaunchProfile {
    /// Preserve the legacy host-managed process boundary, including ambient
    /// environment inheritance. This is an explicit absence of OS confinement.
    #[must_use]
    pub fn trusted_host() -> Self {
        Self {
            binding: Arc::new(LaunchBinding::TrustedHost),
        }
    }

    /// Retain the host's complete mechanical requirement and explicit working
    /// directory. No rule is weakened when compilation or path validation fails.
    /// Hosts can inspect [`Self::capabilities`] before admitting a required
    /// deployment; optional unavailable servers instead fail their own launch.
    /// Required stdio launches discard server diagnostics through null stderr.
    /// They neither inherit a host diagnostic descriptor nor retain an unread
    /// diagnostic pipe. MCP protocol stdin and stdout remain piped.
    #[must_use]
    pub fn required(requirement: ExecutionConfinement, working_directory: PathBuf) -> Self {
        let compiled = CompiledConfinement::compile(&requirement).and_then(|compiled| {
            if !working_directory.is_absolute()
                || working_directory
                    .as_os_str()
                    .as_encoded_bytes()
                    .contains(&0)
            {
                return Err(ConfinementRefusal::InvalidLaunch);
            }
            Ok(compiled)
        });
        Self {
            binding: Arc::new(LaunchBinding::Required {
                compiled,
                working_directory,
            }),
        }
    }

    /// Setup support only, not permission or proof of a launched process.
    /// `Ok(None)` identifies explicit trusted-host execution.
    pub fn capabilities(&self) -> Result<Option<&ConfinementCapabilityReport>, ConfinementRefusal> {
        match self.binding.as_ref() {
            LaunchBinding::TrustedHost => Ok(None),
            LaunchBinding::Required { compiled, .. } => compiled
                .as_ref()
                .map(|compiled| Some(compiled.capabilities()))
                .map_err(|error| *error),
        }
    }

    pub(crate) fn prepare(
        &self,
        stdio: &McpStdioConfig,
    ) -> Result<Option<PreparedConfinement>, ConfinementRefusal> {
        match self.binding.as_ref() {
            LaunchBinding::TrustedHost => Ok(None),
            LaunchBinding::Required {
                compiled,
                working_directory,
            } => {
                let compiled = compiled.as_ref().map_err(|error| *error)?;
                let environment: BTreeMap<_, _> = stdio
                    .env
                    .iter()
                    .map(|(key, value)| (key.as_str().into(), value.as_str().into()))
                    .collect();
                let launch = ProcessLaunchSpec::new(
                    stdio.command.as_str().into(),
                    stdio
                        .args
                        .iter()
                        .map(|argument| argument.as_str().into())
                        .collect(),
                    working_directory.clone(),
                    environment,
                )?;
                compiled.bind_launch(launch).map(Some)
            }
        }
    }
}
