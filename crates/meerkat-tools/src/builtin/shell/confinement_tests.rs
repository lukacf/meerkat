//! Required-confinement acceptance tests at the real shell entry points.
//!
//! Platform cases use the fixed system executor. Unsupported requirements
//! refuse the operation without an unrestricted fallback.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
    PlatformBaseline,
};
use meerkat_core::{BlobStore, SessionId};
use meerkat_jobs::{
    AttemptClaim, CanonicalArgumentsHash, CheckpointRef, DetachedJobService, DetachedJobStore,
    ExecutionIntentId, InteractionLineageId, JobSpec, JobSubmissionKey, JobTerminalResult,
    RestartClass, RunnerHandleRef, RunnerIdentity, RunnerSpecificationRef, SqliteDetachedJobStore,
    ToolIdentity, WorkerId,
};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use meerkat_store::FsBlobStore;
use serde_json::json;

use super::{
    DurableShellJobRuntime, JobId, JobManager, ShellConfig, ShellConfinement,
    ShellJobDeliveryProjector, ShellTool,
};
use crate::builtin::BuiltinTool;

fn requirement(work: &Path) -> ExecutionConfinement {
    ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.to_owned())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.to_owned())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: false,
    }
    .try_into()
    .unwrap()
}

fn required_config(work: &Path) -> ShellConfig {
    ShellConfig {
        enabled: true,
        shell: "sh".into(),
        shell_path: Some(PathBuf::from("/bin/sh")),
        project_root: work.to_owned(),
        confinement: ShellConfinement::Required {
            requirement: requirement(work),
        },
        ..ShellConfig::default()
    }
}

fn unsupported_config(work: &Path) -> ShellConfig {
    let mut config = required_config(work);
    let mut spec = requirement(work).specification().clone();
    spec.require_descendant_termination = true;
    config.confinement = ShellConfinement::Required {
        requirement: spec.try_into().unwrap(),
    };
    config
}

#[test]
fn required_confinement_config_roundtrip_never_drops_enforcement() {
    let config = required_config(Path::new("/tmp/work"));
    let encoded = serde_json::to_value(&config).unwrap();
    assert_eq!(encoded["confinement"]["mode"], "required");
    let decoded: ShellConfig = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded.confinement, config.confinement);
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);

    let mut legacy = encoded;
    legacy.as_object_mut().unwrap().remove("confinement");
    let decoded: ShellConfig = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.confinement, ShellConfinement::TrustedHost);
}

#[test]
fn required_confinement_malformed_payload_never_defaults_to_trusted() {
    let config = required_config(Path::new("/tmp/work"));
    let good = serde_json::to_value(config).unwrap();
    let mut missing = good.clone();
    missing["confinement"]
        .as_object_mut()
        .unwrap()
        .remove("requirement");
    assert!(serde_json::from_value::<ShellConfig>(missing).is_err());
    let mut null = good.clone();
    null["confinement"]["requirement"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<ShellConfig>(null).is_err());
    let mut unknown = good.clone();
    unknown["confinement"]["fallback"] = json!("trusted_host");
    assert!(serde_json::from_value::<ShellConfig>(unknown).is_err());
    let mut malformed = good;
    malformed["confinement"]["requirement"]["write"] =
        json!({"kind":"paths", "paths":[{"kind":"subtree", "path":"relative"}]});
    assert!(serde_json::from_value::<ShellConfig>(malformed).is_err());
}

#[tokio::test]
async fn required_confinement_foreground_unavailable_is_local_feedback_without_execution() {
    let root = tempfile::tempdir().unwrap();
    let tool = ShellTool::new(unsupported_config(root.path()));
    let result = tool.call(json!({"command":"printf ran > forbidden"})).await;
    assert!(
        !root.path().join("forbidden").exists(),
        "refused command ran"
    );
    let error = result.expect_err("required confinement cannot fall back to direct spawn");
    assert!(error.to_string().contains("confinement"));

    // A refusal does not poison shared process custody or disable later tools.
    let mut trusted = tool.config.clone();
    trusted.confinement = ShellConfinement::TrustedHost;
    ShellTool::new(trusted)
        .call(json!({"command":"printf next > permitted"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(root.path().join("permitted")).unwrap(),
        b"next"
    );
}

#[derive(Debug)]
struct DeliveryProjection;

#[async_trait::async_trait]
impl ShellJobDeliveryProjector for DeliveryProjection {
    async fn project_job(&self, _job_id: &str) -> Result<(), String> {
        Ok(())
    }
}

struct JobsFixture {
    _root: tempfile::TempDir,
    work: PathBuf,
    session: SessionId,
    runtime: DurableShellJobRuntime,
    service: DetachedJobService,
    blobs: Arc<dyn BlobStore>,
}

impl JobsFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        // Placement takes an already resolved working root. On macOS the
        // temporary directory spelling can use /var instead of /private/var.
        let work = work.canonicalize().unwrap();
        let session = SessionId::new();
        let store: Arc<dyn DetachedJobStore> =
            Arc::new(SqliteDetachedJobStore::open(root.path().join("jobs.db")).unwrap());
        let blobs: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(root.path().join("blobs")));
        let runtime = DurableShellJobRuntime::new(
            "confinement-tests",
            session.clone(),
            store.clone(),
            blobs.clone(),
            Arc::new(DeliveryProjection),
        )
        .unwrap();
        Self {
            _root: root,
            work,
            session,
            runtime,
            service: DetachedJobService::new(store),
            blobs,
        }
    }

    fn manager(&self, config: ShellConfig) -> Arc<JobManager> {
        Arc::new(
            JobManager::new(config)
                .bind_canonical_async_ops(
                    self.session.clone(),
                    Arc::new(RuntimeOpsLifecycleRegistry::new()),
                )
                .with_durable_job_runtime(self.runtime.clone()),
        )
    }

    async fn completed(&self, id: &meerkat_jobs::JobId) -> meerkat_jobs::JobSnapshot {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let state = self.service.get(id).await.unwrap().unwrap();
                if state.terminal_result.is_some() {
                    break state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded child must finish")
    }

    // Create actual persisted job/attempt/checkpoint state through its owner.
    // The runner specification intentionally contains no host requirements.
    async fn lost_monitor(&self, script: &str) -> meerkat_jobs::JobId {
        let config = ShellConfig::with_project_root(self.work.clone());
        let placement = config
            .execution_placement_for_working_dir_async(&self.work)
            .await
            .unwrap();
        let spec = json!({
            "command": script, "working_dir": self.work, "placement": placement, "timeout_secs": 5,
            "monitor": {"protocol": super::MonitorOutputProtocol::FramedJsonl,
                "limits": super::MonitorProtocolLimits::default(),
                "delivery": meerkat_jobs::JobDeliveryKind::Record}
        });
        let blob = self
            .blobs
            .put_artifact(
                "application/vnd.meerkat.shell-runner+json",
                &spec.to_string(),
            )
            .await
            .unwrap();
        let job = self
            .service
            .submit(
                JobSpec::new(
                    "confinement-tests",
                    self.session.clone(),
                    ExecutionIntentId::from_string("recovered-confinement").unwrap(),
                    InteractionLineageId::from_string("recovered-confinement").unwrap(),
                    ToolIdentity::new("monitor_start", "v1").unwrap(),
                    RunnerIdentity::new("meerkat.monitor_script", "v1").unwrap(),
                    RestartClass::CheckpointResumable,
                    CanonicalArgumentsHash::new(blob.blob_id.to_string()).unwrap(),
                    JobSubmissionKey::new("recovered-confinement").unwrap(),
                )
                .with_runner_specification_ref(
                    RunnerSpecificationRef::new(blob.blob_id.to_string()).unwrap(),
                ),
            )
            .await
            .unwrap();
        let attempt = self
            .service
            .claim_attempt(
                &job.job_id,
                AttemptClaim::new(
                    WorkerId::new("lost-worker").unwrap(),
                    1,
                    10,
                    RunnerHandleRef::new("inproc-monitor:lost").unwrap(),
                ),
            )
            .await
            .unwrap();
        self.service
            .record_checkpoint(
                &job.job_id,
                (&attempt).into(),
                CheckpointRef::new("checkpoint-v1").unwrap(),
                2,
            )
            .await
            .unwrap();
        job.job_id
    }
}

#[tokio::test]
async fn required_confinement_rejects_mismatched_manager_and_public_config_changes() {
    let fixture = JobsFixture::new();
    let required = required_config(&fixture.work);
    let mut trusted = required.clone();
    trusted.confinement = ShellConfinement::TrustedHost;
    let mut tool = ShellTool::with_job_manager(required.clone(), fixture.manager(trusted.clone()));
    for background in [false, true] {
        let error = tool
            .call(json!({"command":"printf ran > mismatched", "background":background}))
            .await
            .expect_err("a separate trusted manager must not weaken Required");
        assert_eq!(
            error.to_string(),
            format!(
                "Execution failed: {}",
                meerkat_core::confinement::ConfinementRefusal::InvalidRequirement
            )
        );
        assert!(!fixture.work.join("mismatched").exists());
    }
    tool.config = trusted;
    tool.call(json!({"command":"printf next > matching"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(fixture.work.join("matching")).unwrap(),
        b"next"
    );

    let mut tool = ShellTool::new(required);
    tool.config.confinement = ShellConfinement::TrustedHost;
    let error = tool
        .call(json!({"command":"printf ran > changed-profile"}))
        .await
        .expect_err("public config mutation must not select the cached Required profile");
    assert_eq!(
        error.to_string(),
        format!(
            "Execution failed: {}",
            meerkat_core::confinement::ConfinementRefusal::InvalidRequirement
        )
    );
    assert!(!fixture.work.join("changed-profile").exists());
}

#[tokio::test]
async fn required_confinement_background_unavailable_is_local_feedback_without_execution() {
    let fixture = JobsFixture::new();
    let config = unsupported_config(&fixture.work);
    let tool = ShellTool::with_job_manager(config.clone(), fixture.manager(config));
    let result = tool
        .call(json!({"command":"printf ran > forbidden", "background":true}))
        .await;
    assert!(
        result.is_err(),
        "background launch silently bypassed required confinement"
    );
    assert!(!fixture.work.join("forbidden").exists());
}

#[tokio::test]
async fn required_confinement_recovered_monitor_uses_current_host_not_old_job_metadata() {
    let fixture = JobsFixture::new();
    let id = fixture
        .lost_monitor("printf ran > forbidden; printf '%s\\n' '{\"type\":\"complete\"}'")
        .await;
    let manager = fixture.manager(unsupported_config(&fixture.work));
    let result = manager.get_status(&JobId::from_string(id.as_str())).await;
    assert!(
        result.is_err(),
        "recovery silently spawned with old unconfined job metadata"
    );
    assert!(!fixture.work.join("forbidden").exists());
    let state = fixture.completed(&id).await;
    assert!(matches!(
        state.terminal_result,
        Some(JobTerminalResult::Failed { .. })
    ));
}

#[cfg(all(target_os = "macos", feature = "integration-real-tests"))]
mod platform {
    use super::*;

    fn configured(work: &Path) -> ShellConfig {
        // This is an observed parent environment canary, never mutated by tests.
        assert!(
            std::env::var_os("HOME").is_some(),
            "parent HOME canary required"
        );
        let mut config = required_config(work);
        config
            .env_vars
            .insert("EXPLICIT_VALUE".into(), "explicit".into());
        config
            .env_vars
            .insert("PATH".into(), "/usr/bin:/bin".into());
        config
    }

    // Record each observation before asserting it in the parent. An inherited
    // variable must produce an explicit confinement failure, not a missing-file
    // error caused by exiting the test command before its markers are written.
    const RECORD_ENVIRONMENT: &str = concat!(
        "printf '%s' \"${HOME+x}\" > ambient-home; ",
        "printf '%s' \"$EXPLICIT_VALUE\" > environment-value; ",
        "printf '%s' \"$PATH\" > environment-path; ",
    );

    fn assert_environment(work: &Path) {
        assert_eq!(
            std::fs::read(work.join("ambient-home")).unwrap(),
            b"",
            "required launch inherited the parent's HOME"
        );
        assert_eq!(
            std::fs::read(work.join("environment-value")).unwrap(),
            b"explicit"
        );
        assert_eq!(
            std::fs::read(work.join("environment-path")).unwrap(),
            b"/usr/bin:/bin"
        );
    }

    fn assert_success(output: crate::builtin::ToolOutput) {
        let crate::builtin::ToolOutput::JsonRenderedAsText { value, .. } = output else {
            panic!("shell output");
        };
        assert_eq!(
            value["exit_code"],
            json!(0),
            "positive-control command failed: {value}"
        );
    }

    #[tokio::test]
    async fn required_confinement_foreground_denied_syscall_then_next_command_succeeds() {
        let fixture = JobsFixture::new();
        let outside = fixture._root.path().join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        let tool = ShellTool::new(configured(&fixture.work));
        assert_success(
            tool.call(json!({"command":format!("{RECORD_ENVIRONMENT}printf first > first")}))
                .await
                .unwrap(),
        );
        assert_eq!(std::fs::read(fixture.work.join("first")).unwrap(), b"first");
        // Relative path avoids shell quoting or accidental fixture path escape.
        let denied = tool
            .call(json!({"command":"printf changed > ../outside"}))
            .await
            .unwrap();
        assert_success(
            tool.call(json!({"command":"printf next > next"}))
                .await
                .unwrap(),
        );
        assert_eq!(std::fs::read(fixture.work.join("next")).unwrap(), b"next");
        assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
        let crate::builtin::ToolOutput::JsonRenderedAsText { value, .. } = denied else {
            panic!("shell output");
        };
        assert_ne!(
            value["exit_code"],
            json!(0),
            "actual forbidden syscall must fail"
        );
        assert_environment(&fixture.work);
    }

    #[tokio::test]
    async fn required_confinement_background_has_exact_environment_and_denied_write() {
        let fixture = JobsFixture::new();
        let outside = fixture._root.path().join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        let manager = fixture.manager(configured(&fixture.work));
        let command = format!(
            "{RECORD_ENVIRONMENT}if printf changed > ../outside; then printf allowed > outside-write; else printf denied > outside-write; fi; printf good > background-good"
        );
        let id = manager
            .spawn_job_for_call(&command, None, 5, "confined-background")
            .await
            .unwrap();
        let state = fixture
            .completed(&meerkat_jobs::JobId::new(id.to_string()).unwrap())
            .await;
        assert!(matches!(
            state.terminal_result,
            Some(JobTerminalResult::Succeeded { .. })
        ));
        assert_eq!(
            std::fs::read(fixture.work.join("background-good")).unwrap(),
            b"good"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
        assert_eq!(
            std::fs::read(fixture.work.join("outside-write")).unwrap(),
            b"denied"
        );
        assert_environment(&fixture.work);
    }

    #[tokio::test]
    async fn required_confinement_bound_custody_records_target_and_settles_denied_write() {
        use super::super::{ProcessCustody, ProcessCustodyScope};

        let fixture = JobsFixture::new();
        let outside = fixture._root.path().join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        let custody_root = fixture._root.path().join("custody");
        let (custody, recovered) = ProcessCustody::recover_and_open(
            &custody_root,
            ProcessCustodyScope::session(&fixture.session),
        )
        .await
        .unwrap();
        assert!(recovered.recovered.is_empty());
        // An empty recovered scope is created lazily by its first reservation.
        // The read grant needs an existing canonical fixture directory now.
        let scope_dir = custody_root.join(custody.scope().as_str());
        std::fs::create_dir_all(&scope_dir).unwrap();
        let scope_dir = scope_dir.canonicalize().unwrap();

        let mut config = configured(&fixture.work);
        // The target's first action reads its persisted custody record. Grant
        // only this fixture's scope so the outside write remains forbidden.
        let mut spec = requirement(&fixture.work).specification().clone();
        spec.read = FilesystemAccess::Paths(vec![
            PathAccess::Subtree(fixture.work.clone()),
            PathAccess::Subtree(scope_dir.clone()),
        ]);
        config.confinement = ShellConfinement::Required {
            requirement: spec.try_into().unwrap(),
        };
        config.env_vars.insert(
            "CUSTODY_RECORD_DIR".into(),
            scope_dir.to_str().unwrap().into(),
        );
        let manager = fixture.manager(config);
        manager.bind_process_custody(custody).unwrap();

        let command = format!(
            "{}{RECORD_ENVIRONMENT}{}",
            concat!(
                "cat \"$CUSTODY_RECORD_DIR\"/*.json > custody-record || exit 90; ",
                "printf '%s' \"$$\" > custody-pid; ",
            ),
            concat!(
                "if printf changed > ../outside; then printf allowed > outside-write; ",
                "else printf denied > outside-write; fi; ",
                "printf good > custody-good",
            ),
        );
        let id = manager
            .spawn_job_for_call(&command, None, 5, "confined-custody")
            .await
            .unwrap();
        let state = fixture
            .completed(&meerkat_jobs::JobId::new(id.to_string()).unwrap())
            .await;
        assert_eq!(state.attempt_count, 1);
        assert!(matches!(
            state.terminal_result,
            Some(JobTerminalResult::Succeeded { .. })
        ));

        let recorded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.work.join("custody-record")).unwrap())
                .unwrap();
        let target_pid: i32 = std::fs::read_to_string(fixture.work.join("custody-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(recorded["phase"], "spawned");
        assert_eq!(recorded["leader"]["pid"], target_pid);
        assert!(recorded["leader"]["start"].is_object());
        assert_eq!(recorded["scope"], fixture.session.to_string());
        assert_eq!(recorded["tool_call_id"], "confined-custody");
        assert_eq!(recorded["spawner"]["kind"], "background_job");
        assert_eq!(recorded["spawner"]["job_id"], id.to_string());
        assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
        assert_eq!(
            std::fs::read(fixture.work.join("outside-write")).unwrap(),
            b"denied"
        );
        assert_eq!(
            std::fs::read(fixture.work.join("custody-good")).unwrap(),
            b"good"
        );
        assert_environment(&fixture.work);

        // A denied syscall must leave this same custody-bound runner usable.
        let sibling = manager
            .spawn_job_for_call(
                "printf next > custody-sibling",
                None,
                5,
                "confined-custody-next",
            )
            .await
            .unwrap();
        let state = fixture
            .completed(&meerkat_jobs::JobId::new(sibling.to_string()).unwrap())
            .await;
        assert!(matches!(
            state.terminal_result,
            Some(JobTerminalResult::Succeeded { .. })
        ));
        assert_eq!(
            std::fs::read(fixture.work.join("custody-sibling")).unwrap(),
            b"next"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let records = std::fs::read_dir(&scope_dir)
                    .unwrap()
                    .map(Result::unwrap)
                    .filter(|entry| {
                        entry.path().extension().and_then(|e| e.to_str()) == Some("json")
                    })
                    .count();
                if records == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both confined job custody records settle after containment");
    }

    #[tokio::test]
    async fn required_confinement_recovered_monitor_keeps_private_variables_without_ambient_env() {
        let fixture = JobsFixture::new();
        let outside = fixture._root.path().join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        let command = format!(
            "{RECORD_ENVIRONMENT}{}",
            concat!(
                "printf '%s' \"$MEERKAT_MONITOR_SUBMISSION_KEY\" > monitor-submission; ",
                "printf '%s' \"$MEERKAT_MONITOR_CHECKPOINT\" > monitor-checkpoint; ",
                "if printf changed > ../outside; then printf allowed > outside-write; else printf denied > outside-write; fi; ",
                "printf good > recovered-good; printf '%s\\n' '{\"type\":\"complete\"}'",
            )
        );
        let id = fixture.lost_monitor(&command).await;
        let manager = fixture.manager(configured(&fixture.work));
        manager
            .get_status(&JobId::from_string(id.as_str()))
            .await
            .unwrap();
        let state = fixture.completed(&id).await;
        assert_eq!(state.attempt_count, 2);
        assert!(matches!(
            state.terminal_result,
            Some(JobTerminalResult::Succeeded { .. })
        ));
        assert_eq!(
            std::fs::read(fixture.work.join("recovered-good")).unwrap(),
            b"good"
        );
        assert_eq!(
            std::fs::read(fixture.work.join("monitor-submission")).unwrap(),
            b"recovered-confinement"
        );
        assert_eq!(
            std::fs::read(fixture.work.join("monitor-checkpoint")).unwrap(),
            b"checkpoint-v1"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
        assert_eq!(
            std::fs::read(fixture.work.join("outside-write")).unwrap(),
            b"denied"
        );
        assert_environment(&fixture.work);
    }
}
