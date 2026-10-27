//! One native attachment call retained beside A's existing source custody.
//! The canonical B link selects it. No readiness or permission is persisted.

use super::*;
use crate::private_host::{ConsoleLoginHandle, ConsoleLoginReservation};
use meerkat_core::mcp_attachment::{
    LiveMcpAttachError, LiveMcpAttachment, LiveMcpAttachmentReceipt, McpAttachmentReadiness,
    McpStdioExit, McpStdioSetupFailure, McpStdioSetupFailureClass, McpStdioSetupPhase,
};
use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentEffect};
use std::{collections::BTreeSet, future::Future, pin::Pin, time::Duration};
use toolkit_store::SqliteStore;

// Bounds process custody without evicting an accepted native call. Completed,
// cancelled and expired canonical B rows release their own entries.
const MAX_ACTIVE_ATTACHMENTS_PER_ORIGINAL: usize = 64;
const OBSERVATION_BUDGET: Duration = Duration::from_millis(250);
type NativeCall = Pin<Box<dyn Future<Output = Result<LiveMcpAttachmentReceipt, LiveMcpAttachError>> + Send>>;

// Only the final successful attachment recheck constructs this value. Its
// private binding prevents another source or grant from borrowing the facts.
// The existing bound Input retains its serialized observation, not this handle.
pub(in crate::private_originals) struct ServiceInputContext {
    original: PrivateOriginalRef,
    grant: PrivateServiceGrantRef,
    observation: ServiceInputObservation,
}

#[derive(Serialize)]
struct ServiceInputObservation {
    schema: &'static str,
    operation: String,
    original: String,
    connection: Id,
    resource: Id,
    allowed_operations: BTreeSet<Id>,
    expires_at_ms: u64,
    configuration_sha256: Revision,
    attachment: &'static str,
    meaning: &'static str,
}

impl ServiceInputContext {
    pub(in crate::private_originals) fn into_content(
        self, original: &PrivateOriginalRef, grant: &PrivateServiceGrantRef,
    ) -> Result<meerkat_core::ContentInput, PrivateOriginalError> {
        if &self.original != original || &self.grant != grant {
            return Err(PrivateOriginalError::Conflict);
        }
        let bytes = canonical_json(&self.observation).map_err(|_| PrivateOriginalError::Conflict)?;
        let text = String::from_utf8(bytes).map_err(|_| PrivateOriginalError::Conflict)?;
        Ok(meerkat_core::ContentInput::Text(text))
    }
}

enum ServiceAttachmentReadiness {
    Pending,
    Ready,
    Failed {
        failure: meerkat_toolkit::setup::ApprovedServiceAttachmentFailure,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
    },
}

// Only this bounded projection may leave the native host. Server identifiers,
// attempt numbers and sensitive stderr stay with the exact native attempt.
fn project_setup_failure(
    failure: McpStdioSetupFailure,
) -> Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation> {
    use meerkat_toolkit::setup::{
        ApprovedServiceAttachmentClass as Class, ApprovedServiceAttachmentExit as Exit,
        ApprovedServiceAttachmentObservation as Observation,
        ApprovedServiceAttachmentPhase as Phase,
    };
    let phase = match failure.phase {
        McpStdioSetupPhase::Launch => Phase::Launch,
        McpStdioSetupPhase::Initialize => Phase::Initialize,
        McpStdioSetupPhase::ToolDiscovery => Phase::ToolDiscovery,
        _ => return None,
    };
    let class = match failure.class {
        McpStdioSetupFailureClass::SpawnFailed => Class::SpawnFailed {},
        McpStdioSetupFailureClass::Closed { exit } => Class::Closed {
            exit: exit.and_then(|exit| match exit {
                McpStdioExit::Code(value) => Some(Exit::Code(value)),
                McpStdioExit::Signal(value) => Some(Exit::Signal(value)),
                _ => None,
            }),
        },
        McpStdioSetupFailureClass::Rejected { .. } => Class::Rejected {},
        McpStdioSetupFailureClass::ProtocolError => Class::ProtocolError {},
        McpStdioSetupFailureClass::TimedOut => Class::TimedOut {},
        McpStdioSetupFailureClass::Unknown => Class::Unknown {},
        _ => Class::Unknown {},
    };
    Some(Observation { phase, class })
}

// Fixed stage/outcome labels only. These observations establish neither
// native entry nor settlement, and never authorize retry or readiness.
fn attachment_diagnostic(stage: &'static str, outcome: &'static str) {
    emit_attachment_diagnostic(stage, outcome, None);
}

pub(in crate::private_originals) fn attachment_preparation_diagnostic<T>(
    stage: &'static str,
    result: Option<&Result<T, PrivateOriginalError>>,
) {
    let (outcome, error_class) = match result {
        None => ("entered", None),
        Some(Ok(_)) => ("ok", None),
        Some(Err(error)) => ("error", Some(match error {
            PrivateOriginalError::Unavailable => "unavailable",
            PrivateOriginalError::Conflict => "conflict",
            PrivateOriginalError::Denied => "denied",
        })),
    };
    emit_attachment_diagnostic(stage, outcome, error_class);
}

fn emit_attachment_diagnostic(
    stage: &'static str, outcome: &'static str, error_class: Option<&'static str>,
) {
    use std::io::Write as _;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let diagnostic = serde_json::json!({
        "event": "toolkit_service_attachment",
        "stage": stage,
        "outcome": outcome,
        "error_class": error_class,
        "monotonic_elapsed_ms": elapsed_ms,
    });
    let _ = writeln!(std::io::stderr().lock(), "{diagnostic}");
}

enum NativeCallState {
    RetiredBeforePoll,
    Waiting { future: NativeCall, polled: bool, reservation: Option<ConsoleLoginReservation> },
    Settled(Result<LiveMcpAttachmentReceipt, LiveMcpAttachError>),
}

/// This owns the reply receiver, not the member/router or an alternative
/// lifecycle. Native retains its entered B/R command independently. Keeping
/// the future prevents caller cancellation from losing that command's attempt.
pub(in crate::private_originals) struct HeldServiceAttachment {
    reference: PrivateOriginalRef,
    task: ApprovedTaskMember,
    control: Arc<McpAttachmentControlRequest>,
    operations: BTreeSet<Id>,
    console_login: Option<ConsoleLoginHandle>,
    call: tokio::sync::Mutex<NativeCallState>,
}

impl HeldServiceAttachment {
    fn new(
        reference: PrivateOriginalRef,
        task: ApprovedTaskMember,
        control: Arc<McpAttachmentControlRequest>,
        operations: BTreeSet<Id>,
        future: NativeCall,
        console_login: Option<ConsoleLoginHandle>,
    ) -> Self {
        Self { reference, task, control, operations, console_login,
            call: tokio::sync::Mutex::new(NativeCallState::Waiting {
                future, polled: false, reservation: None,
            }) }
    }

    async fn receipt(
        &self,
        may_start: impl FnOnce() -> Result<(), PrivateOriginalError>,
    ) -> Result<Result<LiveMcpAttachmentReceipt, LiveMcpAttachError>, PrivateOriginalError> {
        let mut state = self.call.lock().await;
        match &mut *state {
            NativeCallState::RetiredBeforePoll => Err(PrivateOriginalError::Denied),
            NativeCallState::Settled(result) => Ok(result.clone()),
            NativeCallState::Waiting { future, polled, reservation } => {
                if !*polled {
                    // No await separates this last currentness check from the
                    // first poll. Cleanup never invokes this start path.
                    may_start()?;
                    // The source selected this exact A login during preparation.
                    // Retirement races only this short reserve, never a relookup.
                    *reservation = self.console_login.as_ref().map(|login| login.reserve())
                        .transpose().map_err(|_| PrivateOriginalError::Denied)?;
                    *polled = true;
                }
                let result = future.as_mut().await;
                settle_login(reservation.take(), &result);
                *state = NativeCallState::Settled(result.clone());
                Ok(result)
            }
        }
    }

    async fn join_if_polled(&self) {
        let mut state = self.call.lock().await;
        match &mut *state {
            NativeCallState::Waiting { future, polled: true, reservation } => {
                let result = future.as_mut().await;
                settle_login(reservation.take(), &result);
                *state = NativeCallState::Settled(result);
            }
            NativeCallState::Waiting { polled: false, .. } => {
                *state = NativeCallState::RetiredBeforePoll;
            }
            _ => (),
        }
    }
}

// A receipt is a known attachment decision, not readiness or physical cleanup.
// Busy explicitly proves unentered work. Other errors are kept conservative;
// this owner does not invent a stronger effect outcome from their detail text.
fn settle_login(
    reservation: Option<ConsoleLoginReservation>,
    result: &Result<LiveMcpAttachmentReceipt, LiveMcpAttachError>,
) {
    if let Some(reservation) = reservation {
        if matches!(result, Ok(_) | Err(LiveMcpAttachError::Busy)) {
            reservation.settled();
        } else {
            reservation.uncertain();
        }
    }
}

impl PrivateOriginals {
    fn attachment_parent(
        &self, task: &ApprovedTaskMember,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| held.reference == task.parent).cloned()
            .ok_or(PrivateOriginalError::Unavailable)
    }

    fn held_service_attachment(
        &self, reference: &PrivateOriginalRef, task: &ApprovedTaskMember,
    ) -> Result<Option<Arc<HeldServiceAttachment>>, PrivateOriginalError> {
        let parent = self.attachment_parent(task)?;
        let entries = parent.service_attachments.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let held = entries.iter().find(|held| held.reference == *reference && held.task.grant == task.grant).cloned();
        if held.as_ref().is_some_and(|held| held.task != *task) {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(held)
    }

    fn attachment_current(&self, held: &HeldServiceAttachment) -> Result<(), PrivateOriginalError> {
        self.service_input_host_current()?;
        let now = self.owner.clock().now().map_err(|_| PrivateOriginalError::Unavailable)?.unix_ms;
        self.current_service_mcp_control(&held.control,
            &held.control.effect_binding(McpAttachmentEffect::Apply), now, false)?;
        Ok(())
    }

    async fn attachment_current_after_read(
        &self, held: &HeldServiceAttachment,
    ) -> Result<(), PrivateOriginalError> {
        self.observe_service_mcp_attachment(&held.reference, &held.control,
            &held.control.effect_binding(McpAttachmentEffect::Apply)).await?;
        self.attachment_current(held)
    }

    /// Preparation reads only the canonical reviewed installation and existing
    /// native owners. Concurrent callers select one keeper before any call is
    /// polled. A cold process cannot reconstruct the missing retained A owner.
    async fn prepare_service_attachment(
        self: &Arc<Self>, store: &Arc<SqliteStore>, reference: &PrivateOriginalRef,
        task: &ApprovedTaskMember,
    ) -> Result<Arc<HeldServiceAttachment>, PrivateOriginalError> {
        if let Some(held) = self.held_service_attachment(reference, task)? { return Ok(held); }
        // Read the canonical B link and retain its actual parent A handle now.
        // No scope string can reconstruct or replace this custody after waits.
        attachment_diagnostic("prepare_console_login", "entered");
        let console_login = self.console_login_for_approved_source(reference).await;
        attachment_diagnostic("prepare_console_login", if console_login.is_ok() { "ok" } else { "error" });
        let console_login = console_login?;
        attachment_diagnostic("prepare_installation", "entered");
        let control = self.prepare_service_mcp_installation_member(store, reference, &task.grant).await;
        attachment_diagnostic("prepare_installation", if control.is_ok() { "ok" } else { "error" });
        let control = control?;
        let binding = control.effect_binding(McpAttachmentEffect::Apply);
        let now = self.owner.clock().now().map_err(|_| PrivateOriginalError::Unavailable)?.unix_ms;
        let current = self.current_service_mcp_control(&control, &binding, now, false)?;
        let parent = self.attachment_parent(task)?;
        if parent.original.association() != &current.parent {
            return Err(PrivateOriginalError::Conflict);
        }
        let member = task.original_task.member.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let route = self.route(&reference.route)?;
        if route.grant_target.as_str() != member.member {
            return Err(PrivateOriginalError::Denied);
        }
        let runtime = self.bound_identity_runtime()?;
        attachment_diagnostic("resolve_identity", "entered");
        let identity = runtime.owned_identity_for_member_alias(&member.member).await;
        attachment_diagnostic("resolve_identity", if identity.is_some() { "some" } else { "none" });
        let identity = identity.ok_or(PrivateOriginalError::Unavailable)?;
        attachment_diagnostic("observe_after_identity", "entered");
        let observed = self.observe_service_mcp_attachment(reference, &control, &binding).await;
        attachment_diagnostic("observe_after_identity", if observed.is_ok() { "ok" } else { "error" });
        observed?;
        let machine = self.machine()?;
        attachment_diagnostic("registration_witness", "entered");
        let registration = machine.current_session_registration_witness(&task.original_task.origin_session_id).await;
        attachment_diagnostic("registration_witness", if registration.is_some() { "some" } else { "none" });
        let registration = registration.ok_or(PrivateOriginalError::Unavailable)?;
        attachment_diagnostic("observe_after_registration", "entered");
        let observed = self.observe_service_mcp_attachment(reference, &control, &binding).await;
        attachment_diagnostic("observe_after_registration", if observed.is_ok() { "ok" } else { "error" });
        observed?;
        attachment_diagnostic("prepare_authority", "entered");
        let authority = machine.prepare_mcp_attachment(&registration,
            &current.parent.candidate().target, control.clone()).await;
        attachment_diagnostic("prepare_authority", if authority.is_ok() { "ok" } else { "error" });
        let authority = authority.map_err(|_| PrivateOriginalError::Unavailable)?;
        attachment_diagnostic("observe_after_authority", "entered");
        let observed = self.observe_service_mcp_attachment(reference, &control, &binding).await;
        attachment_diagnostic("observe_after_authority", if observed.is_ok() { "ok" } else { "error" });
        observed?;
        let server = control.server().clone();
        let held = Arc::new(HeldServiceAttachment::new(reference.clone(), task.clone(), control,
            current.operations, Box::pin(async move {
                attachment_diagnostic("native_attach", "entered");
                let result = runtime.attach_live_mcp_server(&identity, LiveMcpAttachment {
                    control: authority, server, persist: None,
                }).await;
                attachment_diagnostic("native_attach", match &result {
                    Ok(_) => "ok",
                    Err(LiveMcpAttachError::Busy) => "busy",
                    Err(_) => "error",
                });
                result
            }), console_login));
        self.attachment_current(&held)?;
        let mut entries = parent.service_attachments.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(previous) = entries.iter().find(|entry| entry.reference == *reference && entry.task.grant == task.grant) {
            return if previous.task == *task { Ok(previous.clone()) }
                else { Err(PrivateOriginalError::Conflict) };
        }
        if entries.len() >= MAX_ACTIVE_ATTACHMENTS_PER_ORIGINAL {
            return Err(PrivateOriginalError::Unavailable);
        }
        entries.push(held.clone());
        Ok(held)
    }

    fn remove_service_attachment(&self, held: &Arc<HeldServiceAttachment>) {
        if let Ok(parent) = self.attachment_parent(&held.task) {
            parent.service_attachments.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|entry| !Arc::ptr_eq(entry, held));
        }
    }

    /// One bounded readiness observation of the exact accepted attempt. An
    /// observation timeout cancels only this read, never native connection work
    /// or its retained call. It cannot authorize a replacement attachment.
    async fn service_attachment_ready(
        &self, held: &Arc<HeldServiceAttachment>, receipt: &LiveMcpAttachmentReceipt,
    ) -> Result<ServiceAttachmentReadiness, PrivateOriginalError> {
        if !receipt.outcome.staged || receipt.outcome.persisted
            || receipt.attempt.server() != held.control.server().name
            || held.operations.is_empty()
        { return Err(PrivateOriginalError::Conflict); }
        let deadline = tokio::time::Instant::now() + OBSERVATION_BUDGET;
        let mut generation = None;
        for operation in &held.operations {
            self.attachment_current_after_read(held).await?;
            if tokio::time::Instant::now() >= deadline {
                attachment_diagnostic("readiness_observation", "budget_elapsed_before_wait");
                return Ok(ServiceAttachmentReadiness::Pending);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let observed = tokio::time::timeout_at(deadline,
                receipt.attempt.wait(operation.as_str(), remaining)).await;
            let within_deadline = tokio::time::Instant::now() <= deadline;
            // The canonical source/task/grant owner is reopened after every
            // native await. A cached AccessView or ready bit is insufficient.
            self.attachment_current_after_read(held).await?;
            // Project only the result already observed, after the same currentness
            // fence. Failure detail and server/tool descriptors remain private.
            attachment_diagnostic("native_readiness", match &observed {
                Ok(McpAttachmentReadiness::Pending) => "pending",
                Ok(McpAttachmentReadiness::Usable { .. }) => "usable",
                Ok(McpAttachmentReadiness::ToolNotRouted { .. }) => "tool_not_routed",
                Ok(McpAttachmentReadiness::Rejected { .. }) => "rejected",
                Ok(McpAttachmentReadiness::Failed { .. }) => "failed",
                Ok(McpAttachmentReadiness::Superseded) => "superseded",
                Ok(McpAttachmentReadiness::Retired) => "retired",
                Ok(McpAttachmentReadiness::NotCurrent) => "not_current",
                Err(_) => "observation_timeout",
            });
            if !within_deadline {
                attachment_diagnostic("readiness_observation", "budget_elapsed_after_wait");
                return Ok(ServiceAttachmentReadiness::Pending);
            }
            let connection_generation = match observed {
                Ok(McpAttachmentReadiness::Usable { connection_generation }) => connection_generation,
                Ok(
                    terminal @ (McpAttachmentReadiness::Failed { .. }
                    | McpAttachmentReadiness::Rejected { .. }),
                ) => {
                    let failure = match terminal {
                        McpAttachmentReadiness::Failed { .. } => {
                            meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Failed
                        }
                        _ => meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Rejected,
                    };
                    // Metadata is optional and shares the existing readiness
                    // deadline. A slow read cannot erase a timely failure.
                    let observation = if tokio::time::Instant::now() < deadline {
                        let metadata =
                            tokio::time::timeout_at(deadline, receipt.attempt.setup_failure())
                                .await;
                        let metadata_in_time = tokio::time::Instant::now() <= deadline;
                        self.attachment_current_after_read(held).await?;
                        if metadata_in_time {
                            metadata.ok().flatten().and_then(project_setup_failure)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    return Ok(ServiceAttachmentReadiness::Failed {
                        failure,
                        observation,
                    });
                }
                _ => return Ok(ServiceAttachmentReadiness::Pending),
            };
            if generation.is_some_and(|prior| prior != connection_generation) {
                attachment_diagnostic("readiness_generation", "mismatch");
                return Ok(ServiceAttachmentReadiness::Pending);
            }
            generation = Some(connection_generation);
        }
        Ok(ServiceAttachmentReadiness::Ready)
    }

    async fn ensure_service_attachment_member(
        self: &Arc<Self>, store: &Arc<SqliteStore>, reference: &PrivateOriginalRef,
        task: &ApprovedTaskMember,
    ) -> Result<bool, PrivateOriginalError> {
        attachment_diagnostic("prepare", "entered");
        let held = self.prepare_service_attachment(store, reference, task).await;
        attachment_diagnostic("prepare", if held.is_ok() { "ok" } else { "error" });
        let held = held?;
        // An already polled native call must settle even if authority closed
        // meanwhile. An unpolled call is checked immediately before first poll.
        attachment_diagnostic("receipt", "entered");
        let result = held.receipt(|| self.attachment_current(&held)).await;
        attachment_diagnostic("receipt", if result.is_ok() { "returned" } else { "error" });
        let result = result?;
        if matches!(&result, Err(LiveMcpAttachError::Busy)) {
            // Native Busy explicitly proves no attachment was admitted.
            self.remove_service_attachment(&held);
            return Ok(false);
        }
        let receipt = result.map_err(|_| PrivateOriginalError::Unavailable)?;
        attachment_diagnostic("current_after_attach", "entered");
        let current = self.attachment_current_after_read(&held).await;
        attachment_diagnostic("current_after_attach", if current.is_ok() { "ok" } else { "error" });
        current?;
        attachment_diagnostic("readiness", "entered");
        let ready = self.service_attachment_ready(&held, &receipt).await;
        attachment_diagnostic("readiness", match &ready {
            Ok(ServiceAttachmentReadiness::Ready) => "ready",
            Ok(ServiceAttachmentReadiness::Pending) => "not_ready",
            Ok(ServiceAttachmentReadiness::Failed { .. }) => "failed",
            Err(_) => "error",
        });
        self.settle_service_attachment_observation(&held, ready?).await
    }

    /// Repeat the same native observation after the final setup/content read.
    /// Only a ready, current attempt yields host context for the existing B
    /// submission. The observation creates no authority or durable ready bit.
    async fn recheck_service_attachment_member(
        &self, reference: &PrivateOriginalRef, task: &ApprovedTaskMember,
    ) -> Result<Option<ServiceInputContext>, PrivateOriginalError> {
        let held = self.held_service_attachment(reference, task)?.ok_or(PrivateOriginalError::Unavailable)?;
        let receipt = held.receipt(|| Err(PrivateOriginalError::Denied)).await?
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        attachment_diagnostic("recheck_readiness", "entered");
        let ready = self.service_attachment_ready(&held, &receipt).await;
        attachment_diagnostic("recheck_readiness", match &ready {
            Ok(ServiceAttachmentReadiness::Ready) => "ready",
            Ok(ServiceAttachmentReadiness::Pending) => "not_ready",
            Ok(ServiceAttachmentReadiness::Failed { .. }) => "failed",
            Err(_) => "error",
        });
        if !self.settle_service_attachment_observation(&held, ready?).await? {
            return Ok(None);
        }
        // The same canonical control has just passed native readiness and
        // post-await currentness. Project only allowlisted nonsecret facts;
        // never serialize the server, launch arguments or environment.
        let now = self.owner.clock().now().map_err(|_| PrivateOriginalError::Unavailable)?.unix_ms;
        let current = self.current_service_mcp_control(&held.control,
            &held.control.effect_binding(McpAttachmentEffect::Apply), now, false)?;
        Ok(Some(ServiceInputContext {
            original: reference.clone(),
            grant: task.grant.clone(),
            observation: ServiceInputObservation {
                schema: "toolkit.approved-service-context.v1",
                operation: task.acquisition.id.clone(),
                original: reference.original.as_str().to_owned(),
                connection: current.connection,
                resource: current.resource,
                allowed_operations: current.operations,
                expires_at_ms: current.expires_at_ms,
                configuration_sha256: current.configuration_revision,
                attachment: "observed_usable_before_input_admission",
                meaning: "The host completed the approved integration attachment checks for this continuation. This context accompanies the unchanged reviewed user instruction. Native authorization is still checked on each use. This observation does not guarantee future readiness, extend permission, establish filesystem isolation or complete the user's task.",
            },
        }))
    }

    async fn settle_service_attachment_observation(
        &self, held: &Arc<HeldServiceAttachment>, observed: ServiceAttachmentReadiness,
    ) -> Result<bool, PrivateOriginalError> {
        match observed {
            ServiceAttachmentReadiness::Ready => Ok(true),
            ServiceAttachmentReadiness::Pending => Ok(false),
            ServiceAttachmentReadiness::Failed {
                failure,
                observation,
            } => {
                // Native failure is not non-entry evidence. Reopen the exact
                // source/task/grant, then let its existing cancellation writer
                // fence B and prove no immutable binding/admission was published.
                self.attachment_current_after_read(held).await?;
                self.reject_failed_service_attachment_member(
                    &held.reference,
                    &held.task.grant,
                    failure,
                    observation,
                )
                .await?;
                self.retire_service_attachment_member(&held.reference, &held.task)
                    .await;
                Ok(false)
            }
        }
    }

    async fn retire_service_attachment_member(
        &self, reference: &PrivateOriginalRef, task: &ApprovedTaskMember,
    ) {
        if let Ok(Some(held)) = self.held_service_attachment(reference, task) {
            held.join_if_polled().await;
            self.remove_service_attachment(&held);
        }
    }

    pub(super) async fn ensure_service_attachment(
        self: &Arc<Self>, store: &Arc<SqliteStore>, reference: &PrivateOriginalRef, task: &ApprovedTaskLink,
    ) -> Result<bool, PrivateOriginalError> {
        for member in task.attachments() {
            if !self.ensure_service_attachment_member(store, reference, &member).await? { return Ok(false); }
        }
        Ok(true)
    }

    pub(super) async fn recheck_service_attachments(
        &self, reference: &PrivateOriginalRef, task: &ApprovedTaskLink,
    ) -> Result<Option<Vec<ServiceInputContext>>, PrivateOriginalError> {
        let mut contexts = Vec::new();
        for member in task.attachments() {
            let Some(context) = self.recheck_service_attachment_member(reference, &member).await? else { return Ok(None); };
            contexts.push(context);
        }
        // Every await may invalidate an earlier member. Reopen all exact controls
        // once more; the final native binder independently validates the set.
        for member in task.attachments() {
            let held = self.held_service_attachment(reference, &member)?.ok_or(PrivateOriginalError::Unavailable)?;
            self.attachment_current_after_read(&held).await?;
        }
        Ok(Some(contexts))
    }

    #[cfg(test)]
    pub(super) async fn recheck_service_attachment(
        &self, reference: &PrivateOriginalRef, task: &ApprovedTaskLink,
    ) -> Result<Option<ServiceInputContext>, PrivateOriginalError> {
        task.singleton_grant()?;
        self.recheck_service_attachment_member(reference, &task.attachments()[0]).await
    }

    pub(super) async fn retire_service_attachment(&self, reference: &PrivateOriginalRef, task: &ApprovedTaskLink) {
        for member in task.attachments() { self.retire_service_attachment_member(reference, &member).await; }
    }

    pub(super) async fn prune_service_attachments(&self) -> Result<(), PrivateOriginalError> {
        let now = self.owner.clock().now().map_err(|_| PrivateOriginalError::Unavailable)?.unix_ms;
        let parents = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?.clone();
        for parent in parents {
            let entries = parent.service_attachments.lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?.clone();
            let association = parent.original.association();
            let cancelled = matches!(self.owner.original_disposition(
                &association.candidate().ingress_namespace, &association.candidate().original_work),
                Ok(OriginalDisposition::OperationCancelled { .. }));
            for held in entries {
                if cancelled || now >= held.task.before_ms {
                    held.join_if_polled().await;
                    self.remove_service_attachment(&held);
                }
            }
        }
        Ok(())
    }

    /// Shutdown joins only previously polled calls. Never-polled preparations
    /// cannot start an attachment during cleanup. Native router cleanup remains
    /// native-owned; a reply or dropped observation proves no physical cleanup.
    pub(crate) async fn join_service_attachments(&self) {
        let parents = self.retained.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        for parent in parents {
            let entries = parent.service_attachments.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            for held in entries {
                held.join_if_polled().await;
                self.remove_service_attachment(&held);
            }
        }
    }
}

#[cfg(test)]
#[path = "attachment_caller_tests.rs"]
mod tests;
