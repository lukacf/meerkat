//! Custody-aware spawning shared by every shell spawner (foreground calls,
//! background jobs, monitors).
//!
//! With durable custody bound, the process is reserved before spawn, runs
//! behind the spawn gate until its leader is recorded, and its process-group
//! guard exists before the gate opens. Without custody (unsupported
//! platform, or no realm runtime root) the group is still registered as live
//! so a custody recovery in this process never mistakes it for an earlier
//! incarnation's tool.

use std::ffi::{OsStr, OsString};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Arc;

use tokio::process::{Child, Command};

use super::custody_types::ToolProcessSpawner;
use super::process_lifecycle::OwnedProcessGroup;

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
    /// The owner proved the whole group exited: remove the record.
    pub(super) async fn settle(self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(guard) = self.guard {
            guard.settle().await;
        }
    }

    /// Containment is not proven: keep the record until kernel exit
    /// notification proves the group exited.
    pub(super) fn retain(self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(guard) = self.guard {
            guard.settle_when_exited();
        }
    }
}

/// A process spawned in custody, with its group guard.
pub(super) struct SpawnedInCustody {
    pub(super) child: Child,
    pub(super) process_group: OwnedProcessGroup,
    pub(super) hold: CustodyHold,
}

/// Spawn `program args...` in a fresh process group under `binding`.
///
/// `configure` sets the working directory, environment and stdio, and must
/// keep the process in its own group; `make_group` builds the group guard,
/// which exists before the command may run. A custody failure is reported
/// as an I/O error before the command ever runs.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(unused_variables)
)]
pub(super) async fn spawn_in_custody(
    binding: &CustodyBinding,
    spawner: ToolProcessSpawner,
    tool_call_id: Option<&str>,
    run_id: Option<&meerkat_core::RunId>,
    program: &OsStr,
    args: &[OsString],
    configure: impl FnOnce(&mut Command),
    make_group: impl FnOnce(&Child) -> OwnedProcessGroup,
) -> std::io::Result<SpawnedInCustody> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(custody) = binding.custody.as_ref() {
        let (prepared, mut command) = custody
            .prepare_spawn(spawner, tool_call_id, run_id, program, args)
            .await
            .map_err(std::io::Error::other)?;
        configure(&mut command);
        // A spawn failure drops the preparation, which removes the
        // reservation: nothing was released.
        let mut child = command.spawn()?;
        let mut process_group = make_group(&child);
        return match prepared.spawned(&child).await {
            Ok(guard) => Ok(SpawnedInCustody {
                child,
                process_group,
                hold: CustodyHold { guard: Some(guard) },
            }),
            Err(error) => {
                // The gate stayed closed, so the command never ran; reap the
                // gated prologue.
                let _ = process_group.terminate(&mut child).await;
                Err(std::io::Error::other(error))
            }
        };
    }
    let mut command = Command::new(program);
    command.args(args);
    configure(&mut command);
    let child = command.spawn()?;
    let process_group = make_group(&child);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        super::custody::track_owned_process_group(pid);
    }
    Ok(SpawnedInCustody {
        child,
        process_group,
        hold: CustodyHold::default(),
    })
}
