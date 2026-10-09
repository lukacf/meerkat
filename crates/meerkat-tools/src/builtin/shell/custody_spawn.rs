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

/// The single native entry step for a reviewed shell call, run after every
/// local launch preparation step (confinement binding, custody reservation)
/// and immediately before the process spawn. `None` for spawns that are not
/// a new tool-call effect (recovery, host-only helpers).
pub(crate) type EntryHook<'a> =
    Option<&'a (dyn Fn() -> Result<(), meerkat_core::ToolError> + Send + Sync)>;

/// A native entry refusal carried through the io error channel so the shell
/// tool surface can recover the exact typed tool error.
#[derive(Debug)]
pub(crate) struct ReviewedEntryRefused(pub(crate) meerkat_core::ToolError);

impl std::fmt::Display for ReviewedEntryRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for ReviewedEntryRefused {}

pub(super) fn enter_physical(entry: EntryHook<'_>) -> std::io::Result<()> {
    if let Some(enter) = entry {
        enter().map_err(|refusal| std::io::Error::other(ReviewedEntryRefused(refusal)))?;
    }
    Ok(())
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
///
/// Stdin is always `/dev/null`: a tool's child never shares the host's
/// stdin, which in a stdio JSON-RPC host is the protocol transport.
/// Trusted launches set null stdin below; confined launches use null stdin
/// through `SpawnIo::default`, including their custody-gated prologue.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(unused_variables)
)]
pub(super) async fn spawn_in_custody(
    binding: &CustodyBinding,
    identity: SpawnIdentity<'_>,
    confinement: &ConfinementBinding,
    launch: ShellLaunch<'_>,
    entry: EntryHook<'_>,
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
            #[cfg(any(target_os = "linux", target_os = "macos"))]
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
                    return finish_gated_spawn(gate, child, entry, make_group).await;
                }
                enter_physical(entry)?;
                let child = prepared.spawn()?.into();
                Ok(finish_ungated_spawn(child, make_group))
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (prepared, binding, identity, entry, make_group);
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
                return finish_gated_spawn(gate, child, entry, make_group).await;
            }
            let mut command = Command::new(program);
            command.args(args);
            configure_trusted(&mut command, directory, environment);
            enter_physical(entry)?;
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
        .stdin(Stdio::null())
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
    entry: EntryHook<'_>,
    make_group: impl FnOnce(&ProcessChild) -> OwnedProcessGroup,
) -> std::io::Result<SpawnedInCustody> {
    let mut process_group = make_group(&child);
    // The native entry runs after custody recorded the spawned leader and
    // immediately before the gate release that lets the command run.
    match prepared
        .spawned_pid_entering(child.id(), || enter_physical(entry))
        .await
    {
        Ok(guard) => Ok(SpawnedInCustody {
            child,
            process_group,
            hold: CustodyHold { guard: Some(guard) },
        }),
        Err(failure) => {
            // The gate stayed closed; reap the prologue without running the command.
            let _ = process_group.terminate(&mut child).await;
            Err(match failure {
                super::custody::GatedSpawnFailure::Custody(error) => std::io::Error::other(error),
                super::custody::GatedSpawnFailure::Entry(error) => error,
            })
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
