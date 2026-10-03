//! Custody-aware spawning shared by every shell spawner (foreground calls,
//! background jobs, monitors).
//!
//! With durable custody bound, the process is reserved before spawn, runs
//! behind the spawn gate until its leader is recorded, and its process-group
//! guard exists before the gate opens. Without custody (unsupported
//! platform, or no realm runtime root) the group is still registered as live
//! so a custody recovery in this process never mistakes it for an earlier
//! incarnation's tool.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use meerkat_sandbox::{CompiledConfinement, ConfinementRefusal, ProcessChild};
use tokio::process::Command;

use super::config::{ShellConfig, ShellConfinement};

use super::custody_types::ToolProcessSpawner;
use super::process_lifecycle::OwnedProcessGroup;

/// Compiled once from the manager's immutable host configuration.
#[derive(Debug, Clone)]
pub(super) enum ConfinementBinding {
    TrustedHost,
    Required(Result<Arc<CompiledConfinement>, ConfinementRefusal>),
}

impl ConfinementBinding {
    pub(super) fn new(config: &ShellConfinement) -> Self {
        match config {
            ShellConfinement::TrustedHost => Self::TrustedHost,
            ShellConfinement::Required { requirement } => {
                Self::Required(CompiledConfinement::compile(requirement).map(Arc::new))
            }
        }
    }
}

/// Durable custody available to a spawner, if any.
#[derive(Debug, Clone, Default)]
pub(super) struct CustodyBinding {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    custody: Option<Arc<super::custody::ProcessCustody>>,
}

impl CustodyBinding {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn new(custody: Option<Arc<super::custody::ProcessCustody>>) -> Self {
        Self { custody }
    }
}

/// Custody held for one spawned process group.
#[derive(Debug, Default)]
pub(super) struct CustodyHold {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    guard: Option<super::custody::CustodyGuard>,
}

impl CustodyHold {
    /// Keep the record until kernel exit notification proves the group exited.
    /// An accepted kill is an execution fence, not an observed exit.
    pub(super) fn retain(self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(guard) = self.guard {
            guard.settle_when_exited();
        }
    }
}

/// What a custody record says about the process it guards.
pub(super) struct SpawnIdentity<'a> {
    pub(super) spawner: ToolProcessSpawner,
    pub(super) tool_call_id: Option<&'a str>,
    pub(super) run_id: Option<&'a meerkat_core::RunId>,
}

/// A process spawned in custody, with its group guard.
pub(super) struct SpawnedInCustody {
    pub(super) child: ProcessChild,
    pub(super) process_group: OwnedProcessGroup,
    pub(super) hold: CustodyHold,
}

/// Resolve the existing shell environment precedence without reading ambient values.
/// Trusted-host spawning still inherits ambient values; required spawning does not.
pub(super) fn environment(config: &ShellConfig, directory: &Path) -> BTreeMap<OsString, OsString> {
    let mut environment =
        BTreeMap::from([(OsString::from("PWD"), directory.as_os_str().to_owned())]);
    environment.extend(
        config
            .env_vars
            .iter()
            .map(|(key, value)| (OsString::from(key.as_str()), OsString::from(value.as_str()))),
    );
    environment
}

/// Exact launch data assembled from the host configuration and this invocation.
pub(super) struct ShellLaunch<'a> {
    pub(super) program: &'a OsStr,
    pub(super) args: &'a [OsString],
    pub(super) directory: &'a Path,
    pub(super) environment: &'a BTreeMap<OsString, OsString>,
}

/// Spawn one exact shell launch and establish its group before releasing custody.
/// Required confinement never passes through a mutable command builder.
pub(super) async fn spawn_in_custody(
    binding: &CustodyBinding,
    identity: SpawnIdentity<'_>,
    confinement: &ConfinementBinding,
    launch: ShellLaunch<'_>,
    make_group: impl FnOnce(&ProcessChild) -> OwnedProcessGroup,
) -> std::io::Result<SpawnedInCustody> {
    let ShellLaunch {
        program,
        args,
        directory,
        environment,
    } = launch;
    match confinement {
        ConfinementBinding::Required(compiled) => {
            let compiled = compiled
                .as_ref()
                .map_err(|error| std::io::Error::other(*error))?;
            let launch = meerkat_sandbox::ProcessLaunchSpec::new(
                program.into(),
                args.to_vec(),
                directory.to_owned(),
                environment.clone(),
            )
            .map_err(std::io::Error::other)?;
            let prepared = compiled
                .bind_launch(launch)
                .map_err(std::io::Error::other)?;
            #[cfg(target_os = "macos")]
            {
                if let Some(custody) = binding.custody.as_ref() {
                    let gate = custody
                        .prepare_gated_spawn(
                            identity.spawner,
                            identity.tool_call_id,
                            identity.run_id,
                        )
                        .await
                        .map_err(std::io::Error::other)?;
                    let child = gate.spawn_confined(prepared)?;
                    return finish_gated_spawn(gate, child, make_group).await;
                }
                let child = prepared.spawn()?.into();
                Ok(finish_ungated_spawn(child, make_group))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (prepared, binding, identity, make_group);
                Err(std::io::Error::other(
                    meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
                ))
            }
        }
        ConfinementBinding::TrustedHost => {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if let Some(custody) = binding.custody.as_ref() {
                let (gate, mut command) = custody
                    .prepare_spawn(
                        identity.spawner,
                        identity.tool_call_id,
                        identity.run_id,
                        program,
                        args,
                    )
                    .await
                    .map_err(std::io::Error::other)?;
                configure_trusted(&mut command, directory, environment);
                let child = command.spawn()?.into();
                return finish_gated_spawn(gate, child, make_group).await;
            }
            let mut command = Command::new(program);
            command.args(args);
            configure_trusted(&mut command, directory, environment);
            let child = command.spawn()?.into();
            Ok(finish_ungated_spawn(child, make_group))
        }
    }
}

fn configure_trusted(
    command: &mut Command,
    directory: &Path,
    environment: &BTreeMap<OsString, OsString>,
) {
    command
        .current_dir(directory)
        .envs(environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn finish_gated_spawn(
    prepared: super::custody::PreparedCustodySpawn,
    mut child: ProcessChild,
    make_group: impl FnOnce(&ProcessChild) -> OwnedProcessGroup,
) -> std::io::Result<SpawnedInCustody> {
    let mut process_group = make_group(&child);
    match prepared.spawned_pid(child.id()).await {
        Ok(guard) => Ok(SpawnedInCustody {
            child,
            process_group,
            hold: CustodyHold { guard: Some(guard) },
        }),
        Err(error) => {
            // The gate stayed closed; reap the prologue without running the command.
            let _ = process_group.terminate(&mut child).await;
            Err(std::io::Error::other(error))
        }
    }
}

fn finish_ungated_spawn(
    child: ProcessChild,
    make_group: impl FnOnce(&ProcessChild) -> OwnedProcessGroup,
) -> SpawnedInCustody {
    let process_group = make_group(&child);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        super::custody::track_owned_process_group(pid);
    }
    SpawnedInCustody {
        child,
        process_group,
        hold: CustodyHold::default(),
    }
}
