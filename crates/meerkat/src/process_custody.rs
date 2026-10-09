//! Facade wiring of durable tool process custody (Linux and macOS).
//!
//! Custody itself lives in `meerkat_tools::builtin::shell` (records, spawn
//! gate, recovery). The facade owns where it is applied:
//!
//! - every agent build under a realm runtime root settles the session's
//!   earlier-incarnation tool processes before the agent (and so any new work
//!   for the session) exists, whatever spawners the new build enables;
//! - the session's interrupted-run evidence is handed to the runtime through
//!   the session bindings and, for attachments that precede any agent build,
//!   through the runtime's evidence source, so recovered inputs of
//!   interrupted runs are settled instead of replayed on every surface;
//! - command hooks run in custody through the meerkat-hooks seam;
//! - a realm sweep settles sessions that are never resumed.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "session-store")]
use meerkat_core::tool_process::{
    InterruptedToolEvidence, InterruptedToolEvidenceError, InterruptedToolEvidenceSource,
};
use meerkat_core::types::SessionId;
use meerkat_hooks::{CommandHookCustodyError, CommandHookCustodySpawn, CommandHookProcessCustody};
use meerkat_tools::builtin::shell::{
    PROCESS_CUSTODY_DIR, PreparedCustodySpawn, ProcessCustody, ProcessCustodyError,
    ProcessCustodyScope, ToolProcessSpawner,
};

/// The custody root of a realm runtime root.
pub(crate) fn custody_root(runtime_root: &Path) -> PathBuf {
    runtime_root.join(PROCESS_CUSTODY_DIR)
}

/// Settle the session's earlier-incarnation tool processes and open its
/// custody for this incarnation. A scope this process already holds open is
/// returned as is (settled once per process, not once per caller).
pub(crate) async fn open_session_custody(
    runtime_root: &Path,
    session_id: &SessionId,
) -> Result<Arc<ProcessCustody>, ProcessCustodyError> {
    let (custody, report) = ProcessCustody::recover_and_open(
        &custody_root(runtime_root),
        ProcessCustodyScope::session(session_id),
    )
    .await?;
    for recovered in &report.recovered {
        tracing::warn!(
            %session_id,
            entry_id = %recovered.entry_id,
            prior_incarnation = %recovered.prior_incarnation,
            tool_call_id = ?recovered.tool_call_id,
            run_id = ?recovered.run_id,
            spawner = ?recovered.spawner,
            cessation = ?recovered.cessation,
            "settled a tool process left by a prior host incarnation"
        );
    }
    Ok(custody)
}

/// The runtime's source of interrupted-run evidence: settles a session's
/// earlier-incarnation tool processes when the runtime attaches the session,
/// before anything recovered is served, even when no agent has been built
/// for it yet (for example an RPC on-demand attach). Only the runtime-backed
/// surface installs it, so it exists under the same `session-store` gate.
#[cfg(feature = "session-store")]
pub(crate) struct CustodyEvidenceSource {
    runtime_root: PathBuf,
}

#[cfg(feature = "session-store")]
impl CustodyEvidenceSource {
    pub(crate) fn new(runtime_root: PathBuf) -> Self {
        Self { runtime_root }
    }
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl InterruptedToolEvidenceSource for CustodyEvidenceSource {
    async fn settle_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Arc<dyn InterruptedToolEvidence>>, InterruptedToolEvidenceError> {
        let custody = open_session_custody(&self.runtime_root, session_id)
            .await
            .map_err(|error| InterruptedToolEvidenceError {
                reason: error.to_string(),
            })?;
        Ok(Some(custody as Arc<dyn InterruptedToolEvidence>))
    }
}

/// Settle every session under the realm's custody root once per process, on
/// a background thread.
pub(crate) fn sweep_realm_once(runtime_root: &Path) {
    meerkat_tools::builtin::shell::sweep_process_custody_once(custody_root(runtime_root));
}

/// Command-hook custody backed by the session's process custody.
pub(crate) struct HookProcessCustody {
    custody: Arc<ProcessCustody>,
    confinement: Option<Arc<crate::command_hook_confinement::CommandHookConfinement>>,
}

impl HookProcessCustody {
    pub(crate) fn new(custody: Arc<ProcessCustody>) -> Self {
        Self {
            custody,
            confinement: None,
        }
    }

    pub(crate) fn with_confinement(
        mut self,
        confinement: Arc<crate::command_hook_confinement::CommandHookConfinement>,
    ) -> Self {
        self.confinement = Some(confinement);
        self
    }
}

struct HookCustodySpawn {
    prepared: PreparedCustodySpawn,
}

fn hook_custody_error(error: ProcessCustodyError) -> CommandHookCustodyError {
    CommandHookCustodyError {
        reason: error.to_string(),
    }
}

fn hook_spawn_error(error: std::io::Error) -> meerkat_core::HookFailureReason {
    match error
        .get_ref()
        .and_then(|source| source.downcast_ref::<meerkat_core::confinement::ConfinementRefusal>())
    {
        Some(refusal) => meerkat_core::HookFailureReason::ConfinementRefused { refusal: *refusal },
        None => {
            meerkat_core::HookFailureReason::execution_failed("confined command hook spawn failed")
        }
    }
}

#[async_trait::async_trait]
impl CommandHookProcessCustody for HookProcessCustody {
    async fn spawn(
        &self,
        hook_id: &meerkat_core::HookId,
        run_id: Option<&meerkat_core::RunId>,
        command: &meerkat_core::config::CommandRuntimeConfig,
    ) -> Result<meerkat_sandbox::ProcessChild, meerkat_core::HookFailureReason> {
        use meerkat_core::HookFailureReason;
        use meerkat_sandbox::{ProcessChild, SpawnIo, StdioMode};

        let spawner = ToolProcessSpawner::CommandHook {
            hook_id: hook_id.to_string(),
        };
        let (gate, mut child): (PreparedCustodySpawn, ProcessChild) = match &self.confinement {
            Some(confinement) => {
                // All launch data is final before binding. Neither the engine
                // nor the custody gate receives a mutable required command.
                let launch = confinement
                    .prepare(command)
                    .map_err(|refusal| HookFailureReason::ConfinementRefused { refusal })?;
                let gate = self
                    .custody
                    .prepare_gated_spawn(spawner, None, run_id)
                    .await
                    .map_err(|_| {
                        HookFailureReason::execution_failed(
                            "command hook custody preparation failed",
                        )
                    })?;
                let child = gate
                    .spawn_confined_with_io(
                        launch,
                        SpawnIo {
                            stdin: StdioMode::Piped,
                            stdout: StdioMode::Piped,
                            stderr: StdioMode::Piped,
                        },
                    )
                    .map_err(hook_spawn_error)?;
                (gate, child)
            }
            None => {
                // Compatibility only: no host confinement was requested.
                let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
                let (gate, mut builder) = self
                    .custody
                    .prepare_spawn(spawner, None, run_id, OsStr::new(&command.command), &args)
                    .await
                    .map_err(|_| {
                        HookFailureReason::execution_failed(
                            "command hook custody preparation failed",
                        )
                    })?;
                builder
                    .envs(&command.env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);
                let child = builder.spawn().map_err(|_| {
                    HookFailureReason::execution_failed("command hook spawn failed")
                })?;
                (gate, child.into())
            }
        };
        match gate.spawned_process(&child).await {
            Ok(guard) => guard.settle_when_exited(),
            Err(_) => {
                // No target entered: a failed custody commit never releases
                // the gate. Retain the child through termination and reap.
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(HookFailureReason::execution_failed(
                    "command hook custody activation failed",
                ));
            }
        }
        Ok(child)
    }

    async fn prepare(
        &self,
        hook_id: &meerkat_core::HookId,
        run_id: Option<&meerkat_core::RunId>,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<(Box<dyn CommandHookCustodySpawn>, tokio::process::Command), CommandHookCustodyError>
    {
        if self.confinement.is_some() {
            return Err(CommandHookCustodyError {
                reason: "required command hooks use the immutable spawn boundary".into(),
            });
        }
        let (prepared, command) = self
            .custody
            .prepare_spawn(
                ToolProcessSpawner::CommandHook {
                    hook_id: hook_id.to_string(),
                },
                None,
                run_id,
                program,
                args,
            )
            .await
            .map_err(hook_custody_error)?;
        Ok((Box::new(HookCustodySpawn { prepared }), command))
    }
}

#[async_trait::async_trait]
impl CommandHookCustodySpawn for HookCustodySpawn {
    async fn spawned(
        self: Box<Self>,
        child: &tokio::process::Child,
    ) -> Result<(), CommandHookCustodyError> {
        let guard = self
            .prepared
            .spawned(child)
            .await
            .map_err(hook_custody_error)?;
        // A hook's background members may outlive it; custody holds the
        // group until kernel exit notification proves it gone.
        guard.settle_when_exited();
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::HookId;
    use meerkat_core::{HookAdapterConfig, HookEntryConfig, HookRuntimeKind, HooksConfig};
    use meerkat_core::{HookEngine, HookInvocation, HookPoint};

    #[test]
    fn confined_hook_spawn_preserves_only_typed_confinement_refusals() {
        use meerkat_core::HookFailureReason;
        use meerkat_core::confinement::ConfinementRefusal;

        for refusal in [
            ConfinementRefusal::InvalidRequirement,
            ConfinementRefusal::InvalidLaunch,
            ConfinementRefusal::UnsupportedRequirement,
            ConfinementRefusal::BackendUnavailable,
            ConfinementRefusal::PreparationFailed,
        ] {
            assert_eq!(
                hook_spawn_error(std::io::Error::other(refusal)),
                HookFailureReason::ConfinementRefused { refusal }
            );
        }
        for error in [
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            std::io::Error::other(ConfinementRefusal::BackendUnavailable.to_string()),
            std::io::Error::other("private command /host/private"),
        ] {
            assert_eq!(
                hook_spawn_error(error),
                HookFailureReason::execution_failed("confined command hook spawn failed")
            );
        }
    }

    fn record_count(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .and_then(|extension| extension.to_str())
                            == Some("json")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn command_hooks_run_in_durable_custody() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let custody = open_session_custody(temp.path(), &session_id)
            .await
            .unwrap();
        let scope_dir = custody_root(temp.path()).join(session_id.to_string());
        let listing = temp.path().join("listing");
        let config = HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("custody-hook"),
                point: HookPoint::PreToolExecution,
                runtime: HookAdapterConfig::from_kind_and_value(
                    HookRuntimeKind::Command,
                    Some(serde_json::json!({
                        "command": "sh",
                        "args": [
                            "-c",
                            format!(
                                "ls '{}' > '{}'; cat >/dev/null; printf '{{}}'",
                                scope_dir.display(),
                                listing.display()
                            )
                        ],
                        "env": {}
                    })),
                )
                .unwrap_or_default(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = meerkat_hooks::DefaultHookEngine::new(config)
            .with_command_process_custody(Arc::new(HookProcessCustody::new(custody)));

        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: session_id.clone(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(
            report.outcomes[0].failure_reason.is_none(),
            "{:?}",
            report.outcomes[0].failure_reason
        );
        let recorded = std::fs::read_to_string(&listing).unwrap();
        assert!(
            recorded.contains(".json"),
            "the hook ran with a custody record: {recorded:?}"
        );
        // The hook's group has exited; custody settles its record once the
        // exit is observed.
        let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while record_count(&scope_dir) > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(settled.is_ok(), "the hook's custody record is settled");
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn required_command_hook_enters_only_with_its_durable_custody_record() {
        use meerkat_core::confinement::{
            ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess, PlatformBaseline,
        };
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let session_id = SessionId::new();
        let custody = open_session_custody(&root, &session_id).await.unwrap();
        let scope_dir = custody_root(&root).join(session_id.to_string());
        let requirement = ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
        .try_into()
        .unwrap();
        let adapter = HookProcessCustody::new(custody).with_confinement(Arc::new(
            crate::command_hook_confinement::CommandHookConfinement::new(&requirement, work),
        ));
        let hook_id = HookId::new("required-custody-hook");
        let command = meerkat_core::config::CommandRuntimeConfig {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'entered\n'; IFS= read -r finish; test \"$finish\" = finish".into(),
            ],
            env: Default::default(),
        };
        let mut child = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            adapter.spawn(&hook_id, None, &command),
        )
        .await
        .unwrap()
        .unwrap();
        let mut stdout = BufReader::new(child.take_stdout().unwrap());
        let mut stdin = child.take_stdin().unwrap();
        let mut entered = String::new();
        let ready = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stdout.read_line(&mut entered),
        )
        .await;
        let records = std::fs::read_dir(&scope_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .collect::<Vec<_>>();
        // Release and reap before asserting captured state. No sleep decides
        // whether target entry preceded the durable custody publication.
        stdin.write_all(b"finish\n").await.unwrap();
        drop(stdin);
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while record_count(&scope_dir) > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(ready.is_ok_and(|result| result.is_ok()));
        assert_eq!(entered, "entered\n");
        assert!(status.success());
        assert_eq!(records.len(), 1);
        let record: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        assert_eq!(
            record["spawner"],
            serde_json::to_value(ToolProcessSpawner::CommandHook {
                hook_id: hook_id.to_string(),
            })
            .unwrap()
        );
        assert_eq!(record["scope"], session_id.to_string());
        assert_eq!(record["phase"], "spawned");
        assert!(
            settled.is_ok(),
            "actual child exit must settle the custody record"
        );
    }
}
