//! Host launch configuration adapted to the existing command custody owner.

use std::path::PathBuf;

use meerkat_core::confinement::{ConfinementRefusal, ExecutionConfinement};
use meerkat_core::{CommandRuntimeConfig, HookFailureReason, HookId, RunId};
use meerkat_hooks::{CommandHookCustodyError, CommandHookCustodySpawn, CommandHookProcessCustody};
use meerkat_sandbox::{CompiledConfinement, ProcessChild};

pub(crate) struct CommandHookConfinement {
    compiled: Result<CompiledConfinement, ConfinementRefusal>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    working_directory: PathBuf,
}

impl std::fmt::Debug for CommandHookConfinement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CommandHookConfinement([REDACTED])")
    }
}

impl CommandHookConfinement {
    pub(crate) fn new(requirement: &ExecutionConfinement, working_directory: PathBuf) -> Self {
        let compiled = CompiledConfinement::compile(requirement).and_then(|compiled| {
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
            compiled,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            working_directory,
        }
    }

    pub(crate) fn missing_custody_refusal(&self) -> ConfinementRefusal {
        match &self.compiled {
            Err(refusal) => *refusal,
            Ok(_) => ConfinementRefusal::BackendUnavailable,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn prepare(
        &self,
        command: &CommandRuntimeConfig,
    ) -> Result<meerkat_sandbox::PreparedConfinement, ConfinementRefusal> {
        let compiled = self.compiled.as_ref().map_err(|refusal| *refusal)?;
        let launch = meerkat_sandbox::ProcessLaunchSpec::new(
            command.command.as_str().into(),
            command
                .args
                .iter()
                .map(|argument| argument.as_str().into())
                .collect(),
            self.working_directory.clone(),
            command
                .env
                .iter()
                .map(|(key, value)| (key.as_str().into(), value.as_str().into()))
                .collect(),
        )?;
        compiled.bind_launch(launch)
    }
}

/// Required setup without its native custody owner has no raw spawn route.
pub(crate) struct RefusingCommandHookCustody(pub(crate) ConfinementRefusal);

#[async_trait::async_trait]
impl CommandHookProcessCustody for RefusingCommandHookCustody {
    async fn spawn(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&RunId>,
        _command: &CommandRuntimeConfig,
    ) -> Result<ProcessChild, HookFailureReason> {
        Err(HookFailureReason::ConfinementRefused { refusal: self.0 })
    }

    async fn prepare(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&RunId>,
        _program: &std::ffi::OsStr,
        _args: &[std::ffi::OsString],
    ) -> Result<(Box<dyn CommandHookCustodySpawn>, tokio::process::Command), CommandHookCustodyError>
    {
        Err(CommandHookCustodyError {
            reason: self.0.to_string(),
        })
    }
}
