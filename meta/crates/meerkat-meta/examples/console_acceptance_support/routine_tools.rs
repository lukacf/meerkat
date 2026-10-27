//! Actual filesystem tool dispatch for routine-tool presentation acceptance.

use std::path::{Component, Path};
use std::sync::{Arc, Mutex};

use axum::{Json, Router, http::StatusCode, routing::post};
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentToolDispatcher, RunId, ToolCallView, ToolDef, ToolError, ToolResult, types::SessionId,
};
use meerkat_meta::UnifiedRuntime;
use meerkat_meta::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentCustomizer, CustomizerError, DurableAgentSpec,
    LocalExternalToolOverlay,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

pub const NOTES: &str = "  Release notes\nKeep both spaces and this final newline.\n";
pub const LATE_REVIEW: &str = "Late review: all release artifacts match.\n";

#[derive(Default)]
struct Progress {
    entered: bool,
    released: bool,
    completed: bool,
    dropped: bool,
}

// This only observes the real dispatch future. It never creates a tool result.
struct HeldRead<'a> {
    tools: &'a RoutineTools,
    completed: bool,
}

impl HeldRead<'_> {
    fn complete(&mut self) {
        self.completed = true;
        self.tools
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completed = true;
    }
}

impl Drop for HeldRead<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.tools
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .dropped = true;
        }
    }
}

pub struct RoutineTools {
    root: tempfile::TempDir,
    progress: Mutex<Progress>,
    release: Notify,
}

impl RoutineTools {
    pub fn new() -> std::io::Result<Self> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("release-notes.txt"), NOTES)?;
        std::fs::write(root.path().join("late-review.txt"), LATE_REVIEW)?;
        Ok(Self {
            root,
            progress: Mutex::default(),
            release: Notify::new(),
        })
    }

    pub fn control(&self, action: &str) -> Result<Value, String> {
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match action {
            "status" => {}
            "release" => {
                progress.released = true;
                self.release.notify_waiters();
            }
            _ => return Err("routine tool action must be status or release".into()),
        }
        Ok(
            json!({"entered": progress.entered, "released": progress.released,
            "completed": progress.completed, "dropped": progress.dropped}),
        )
    }

    async fn wait_for_release(&self) {
        loop {
            let notified = self.release.notified();
            {
                let mut progress = self
                    .progress
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                progress.entered = true;
                if progress.released {
                    return;
                }
            }
            notified.await;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunAction {
    Status,
    CancelAfterBoundary,
    Interrupt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    identity: String,
    session_id: SessionId,
    run_id: RunId,
    action: RunAction,
}

async fn control_run(runtime: &UnifiedRuntime, request: RunRequest) -> Result<Value, String> {
    if !["router:main", "domain:delivery"].contains(&request.identity.as_str()) {
        return Err("an existing fixture identity is required".into());
    }
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(&request.identity);
    let actual_session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&member)
        .await
        .ok_or("fixture member has no current session")?;
    if actual_session != request.session_id {
        return Err("the requested session is not the current fixture member session".into());
    }
    let machine = runtime
        .mob_runtime()
        .session_service()
        .ok_or("fixture needs the actual session runtime adapter")?
        .acquire_runtime_adapter(None)
        .map_err(|error| format!("acquire the fixture session runtime adapter: {error}"))?
        .ok_or("fixture needs the actual session runtime adapter")?;
    // Both operations compare and admit under the owner's mutation gate. A
    // stale test request cannot cancel a successor run of the same member.
    let accepted = match request.action {
        RunAction::Status => None,
        RunAction::CancelAfterBoundary => Some(
            machine
                .cancel_after_boundary_run_if_current(&actual_session, &request.run_id)
                .await
                .map_err(|error| error.to_string())?,
        ),
        RunAction::Interrupt => Some(
            machine
                .hard_cancel_run_if_current(
                    &actual_session,
                    &request.run_id,
                    "routine acceptance interruption",
                )
                .await
                .map_err(|error| error.to_string())?,
        ),
    };
    let snapshot = machine
        .meerkat_machine_archive_snapshot(&actual_session)
        .await
        .ok_or("runtime snapshot is absent")?;
    Ok(json!({"accepted": accepted, "identity": request.identity,
        "session_id": actual_session, "requested_run_id": request.run_id,
        "current_run_id": snapshot.control.current_run_id, "runtime_phase": snapshot.control.phase,
        "queue": snapshot.queue, "steer_queue": snapshot.steer_queue}))
}

pub fn router(runtime: Arc<UnifiedRuntime>) -> Router {
    Router::new().route(
        "/routine-run",
        post(move |Json(request): Json<RunRequest>| {
            let runtime = runtime.clone();
            async move {
                match control_run(&runtime, request).await {
                    Ok(value) => (StatusCode::OK, Json(value)),
                    Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error": error}))),
                }
            }
        }),
    )
}

pub struct RoutineCustomizer(pub Arc<RoutineTools>);

#[async_trait::async_trait]
impl AgentCustomizer for RoutineCustomizer {
    async fn customize_build(
        &self,
        _: &AgentBuildContext,
        _: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        draft.local_external_tools = LocalExternalToolOverlay::new(self.0.clone());
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for RoutineTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [
            ToolDef { audience: Default::default(), name: "read_file".into(), description: "Read one file from the acceptance workspace.".into(), input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), provenance: None },
            ToolDef { audience: Default::default(), name: "list_files".into(), description: "List the acceptance workspace files.".into(), input_schema: json!({"type":"object","properties":{},"additionalProperties":false}), provenance: None },
        ].into_iter().map(Arc::new).collect::<Vec<_>>().into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let (text, is_error) = match call.name {
            "list_files" => {
                let mut names = std::fs::read_dir(self.root.path())
                    .expect("fixture directory")
                    .map(|entry| {
                        entry
                            .expect("fixture entry")
                            .file_name()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect::<Vec<_>>();
                names.sort();
                (format!("{}\n", names.join("\n")), false)
            }
            "read_file" => {
                let args: Value =
                    serde_json::from_str(call.args.get()).expect("valid fixture arguments");
                let name = args.get("path").and_then(Value::as_str).unwrap_or("");
                let components = Path::new(name).components().collect::<Vec<_>>();
                if components.len() != 1 || !matches!(components[0], Component::Normal(_)) {
                    ("Only one workspace filename is accepted.".to_string(), true)
                } else {
                    let mut held = None;
                    if name == "late-review.txt" {
                        held = Some(HeldRead {
                            tools: self,
                            completed: false,
                        });
                        self.wait_for_release().await;
                    }
                    let result = match std::fs::read_to_string(self.root.path().join(name)) {
                        Ok(text) => (text, false),
                        Err(error) => (format!("Cannot read {name}: {:?}", error.kind()), true),
                    };
                    if let Some(held) = held.as_mut() {
                        held.complete();
                    }
                    result
                }
            }
            _ => return Err(ToolError::not_found(call.name)),
        };
        Ok(ToolResult::new(call.id.to_string(), text, is_error).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_until_entered(tools: &RoutineTools) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while tools.control("status").unwrap()["entered"] != true {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("held filesystem read must enter before its test deadline");
    }

    async fn dispatch(
        tools: &RoutineTools,
        id: &str,
        name: &str,
        args: Value,
    ) -> ToolDispatchOutcome {
        let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        tools
            .dispatch(ToolCallView {
                id,
                name,
                args: &args,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn routine_files_preserve_exact_bytes_and_real_missing_file_error() {
        let tools = RoutineTools::new().unwrap();
        let notes = dispatch(
            &tools,
            "notes",
            "read_file",
            json!({"path":"release-notes.txt"}),
        )
        .await;
        assert_eq!(notes.result.text_content(), NOTES);
        assert!(!notes.result.is_error);
        let missing = dispatch(
            &tools,
            "missing",
            "read_file",
            json!({"path":"missing.txt"}),
        )
        .await;
        assert!(missing.result.is_error);
        assert_eq!(
            missing.result.text_content(),
            "Cannot read missing.txt: NotFound"
        );
        let files = dispatch(&tools, "files", "list_files", json!({})).await;
        assert_eq!(
            files.result.text_content(),
            "late-review.txt\nrelease-notes.txt\n"
        );
    }

    #[tokio::test]
    async fn routine_late_read_has_no_result_until_explicit_release() {
        let tools = Arc::new(RoutineTools::new().unwrap());
        let dispatched = tools.clone();
        let task = tokio::spawn(async move {
            dispatch(
                &dispatched,
                "late",
                "read_file",
                json!({"path":"late-review.txt"}),
            )
            .await
        });
        wait_until_entered(&tools).await;
        assert!(!task.is_finished());
        tools.control("release").unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("released read must finish before its test deadline")
            .unwrap();
        assert_eq!(result.result.text_content(), LATE_REVIEW);
        assert!(!result.result.is_error);
        assert_eq!(tools.control("status").unwrap()["completed"], true);
        assert_eq!(tools.control("status").unwrap()["dropped"], false);
    }

    #[tokio::test]
    async fn routine_dropped_dispatch_has_no_result_or_success_observation() {
        let tools = Arc::new(RoutineTools::new().unwrap());
        let dispatched = tools.clone();
        let task = tokio::spawn(async move {
            dispatch(
                &dispatched,
                "interrupted",
                "read_file",
                json!({"path":"late-review.txt"}),
            )
            .await
        });
        wait_until_entered(&tools).await;
        task.abort();
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("aborted read must stop before its test deadline");
        assert!(joined.unwrap_err().is_cancelled());
        let status = tools.control("status").unwrap();
        assert_eq!(status["dropped"], true);
        assert_eq!(status["completed"], false);
        assert_eq!(status["released"], false);
    }

    #[test]
    fn routine_run_control_requires_exact_typed_run_and_rejects_outcome_injection() {
        let request = json!({"identity":"router:main", "session_id":SessionId::new(),
            "run_id":RunId::new(), "action":"interrupt"});
        assert!(serde_json::from_value::<RunRequest>(request.clone()).is_ok());
        let mut injected = request.clone();
        injected["outcome"] = json!("cancelled");
        assert!(serde_json::from_value::<RunRequest>(injected).is_err());
        let mut ambient = request.clone();
        ambient.as_object_mut().unwrap().remove("run_id");
        assert!(serde_json::from_value::<RunRequest>(ambient).is_err());
        let mut unknown = request;
        unknown["action"] = json!("mark_success");
        assert!(serde_json::from_value::<RunRequest>(unknown).is_err());
    }
}
