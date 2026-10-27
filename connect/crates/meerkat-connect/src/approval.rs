//! Local terminal consent for a single live invocation.
//!
//! ApprovalService owns every durable decision. The state below only retains
//! custody of a currently awaited dispatch; it cannot be restored or replayed.
//! The direct PairedHost path permits exact retained review submissions and
//! nonsecret secret-intent creation/cancellation. PairedHost owns secret entry.
//! Packaged preparation keeps its existing authorization. Only the separately
//! composed service path can approve one captured package installation; account
//! effects remain denied here. An access review may return a native decision
//! record to its authenticated owner; only that owner can issue access.
//! The presenter retains the terminal or host actor;
//! this gate does not cover descendants, jobs, rebuilds, schedules or bridges.

pub mod installation_policy;
use installation_policy::{InstallationPolicySource, InstalledInstallationPolicy, PolicyEntry};

use std::{
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalId, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
    ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStatus,
    LiveBridgeEffectKind, LiveBridgeEffectOutcome, SessionId, ToolDispatchAdmission, ToolError,
    agent::ToolDispatchContext,
    mcp_config::McpTransportConfig,
    tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME},
    types::ToolCallView,
};
use meerkat_store::FileApprovalStore;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    ConnectError, SessionOptions,
    paired_host_tool::{
        HeldAction as HeldPairedHostAction, PAIRED_HOST_FRONTEND_CONSENT_TOOLS,
        PAIRED_HOST_FRONTEND_READ_TOOLS, PairedHostFrontend, ReadbackTarget as PairedHostReadback,
    },
    setup::PREPARATION_TOOLS,
    terminal::{TerminalInput, review_text},
};

enum HeldAction {
    PairedHost(HeldPairedHostAction),
    Forge(crate::forge::HeldForgeAction),
    Service(crate::service_install::HeldInstall),
}
enum ReadbackTarget {
    PairedHost(PairedHostReadback),
    Forge(String),
    Service(crate::service_install::Readback),
}
impl HeldAction {
    fn readback_target(&self) -> Option<ReadbackTarget> {
        match self {
            Self::PairedHost(held) => held.readback_target().map(ReadbackTarget::PairedHost),
            Self::Forge(held) => Some(ReadbackTarget::Forge(held.change_id().into())),
            Self::Service(held) => Some(ReadbackTarget::Service(held.readback())),
        }
    }
}

const DECISION_LIFETIME: Duration = Duration::from_secs(120);
/// A protected review cannot outlive its canonical task. Capture the
/// monotonic instant before wall time so the waiter never gets extra time.
/// Standalone reviews retain their existing lifetime, including zero.
fn dispatch_review_deadline(
    lifetime: Duration,
    task_deadline_ms: Option<u64>,
    monotonic: Instant,
    wall: SystemTime,
) -> Option<(Instant, SystemTime)> {
    let mut expires = wall.checked_add(lifetime)?;
    if let Some(task_deadline_ms) = task_deadline_ms {
        let task_deadline =
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(task_deadline_ms))?;
        if task_deadline <= wall {
            return None;
        }
        expires = expires.min(task_deadline);
    }
    let remaining = expires.duration_since(wall).ok()?;
    Some((monotonic.checked_add(remaining)?, expires))
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn deadline_recovery_review_bounds_both_clocks_and_preserves_shorter_review() {
        let monotonic = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_millis(1_000);
        for (task_deadline_ms, expected) in [(61_000, 60), (181_000, 120)] {
            let (wait, shown) = dispatch_review_deadline(
                Duration::from_secs(120),
                Some(task_deadline_ms),
                monotonic,
                wall,
            )
            .unwrap();
            assert_eq!(
                wait.duration_since(monotonic),
                Duration::from_secs(expected)
            );
            assert_eq!(
                shown.duration_since(wall).unwrap(),
                Duration::from_secs(expected)
            );
            assert!(shown <= SystemTime::UNIX_EPOCH + Duration::from_millis(task_deadline_ms));
        }
    }

    #[test]
    fn deadline_recovery_review_refuses_due_task_without_changing_standalone_lifetime() {
        let monotonic = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_millis(1_000);
        for task_deadline_ms in [0, 999, 1_000] {
            assert!(
                dispatch_review_deadline(
                    DECISION_LIFETIME,
                    Some(task_deadline_ms),
                    monotonic,
                    wall
                )
                .is_none()
            );
        }
        for lifetime in [Duration::ZERO, DECISION_LIFETIME] {
            let (wait, shown) = dispatch_review_deadline(lifetime, None, monotonic, wall).unwrap();
            assert_eq!(wait.duration_since(monotonic), lifetime);
            assert_eq!(shown.duration_since(wall).unwrap(), lifetime);
        }
    }
}

const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
// A complete approved member policy includes preserved grants, not just the
// agent's bounded tool arguments. Keep the full access review independently bounded.
const MAX_ACCESS_REVIEW_BYTES: usize = 1_048_576;

fn invalid(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.into())
}

/// Invocation-only observation, never a durable approval status or a receipt
/// proving that an integration was installed or a business operation succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Custody {
    AwaitingDecision,
    DecisionRecorded(ApprovalDecision),
    DecisionReturned,
    RetiredBeforeAdmission,
    SpentUncertain,
    Settled(LiveBridgeEffectOutcome),
}

enum AttemptInput {
    Dispatch {
        call_id: String,
        name: String,
        arguments: String,
    },
    AccessReview(Box<ApprovalRequest>),
}

struct ServiceReview {
    task: toolkit_mobkit::private_originals::InstalledServiceTask,
    decision_current:
        Option<crate::installed_service_review::account_request::console::DecisionCheck>,
}
struct Attempt {
    approval_id: ApprovalId,
    input: AttemptInput,
    // Equality-only disclosure binding, captured from the actual dispatch.
    review_work: Option<meerkat_core::retained_work::RetainedWorkIdentity>,
    review_scope: Option<meerkat_core::exact_operation::OperationExecutionScope>,
    dispatch_error: Option<meerkat_core::ToolDispatchTerminalErrorKind>,
    deadline: Instant,
    custody: Custody,
    held: Option<HeldAction>,
    service: Option<ServiceReview>,
    service_entry: Option<toolkit_mobkit::private_host::ConsoleLoginReservation>,
    policy_entry: Option<PolicyEntry>,
    readback: Option<ReadbackTarget>,
    retired: watch::Sender<bool>,
}

struct State {
    session: Option<SessionId>,
    closed: bool,
    attempt: Option<Attempt>,
}

struct Shared {
    service: ApprovalService,
    installation_policy: Option<InstallationPolicySource>,
    // Fixed by the trusted presenter constructor, never supplied in an answer.
    actor: ApprovalPrincipalId,
    actor_expires_at: Option<SystemTime>,
    state: Mutex<State>,
    prompts: mpsc::Sender<ApprovalPrompt>,
    // Wake idle presentation on retirement; State.closed remains authoritative.
    retirement: watch::Sender<()>,
    store_path: PathBuf,
    lifetime: Duration,
    // The selected concrete composition fixes eligible effect names. A model,
    // CLI argument or saved approval cannot widen this set.
    consent_tools: &'static [&'static str],
}

impl Shared {
    fn state(&self) -> Result<MutexGuard<'_, State>, ConnectError> {
        self.state.lock().map_err(|_| {
            ConnectError::native(
                "lock local approval custody",
                io::Error::other("Local approval custody is unavailable"),
            )
        })
    }

    fn live_state(&self) -> Result<MutexGuard<'_, State>, ConnectError> {
        let state = self.state()?;
        if self
            .actor_expires_at
            .is_some_and(|expiry| SystemTime::now() >= expiry)
        {
            return Err(invalid("The retained approval presenter login expired"));
        }
        Ok(state)
    }

    fn retire_attempt(&self, id: &ApprovalId) {
        let mut policy = None;
        if let Ok(mut state) = self.state.lock()
            && let Some(attempt) = state.attempt.as_mut()
            && &attempt.approval_id == id
            && matches!(
                attempt.custody,
                Custody::AwaitingDecision | Custody::DecisionRecorded(_)
            )
        {
            attempt.custody = Custody::RetiredBeforeAdmission;
            policy = attempt.policy_entry.take();
            attempt.retired.send_replace(true);
        }
        drop(policy);
        // There is no native cancel-decision API. A pending native record stays
        // pending until its native expiry; retiring this waiter is not a human
        // denial and must not rewrite the decision owner with invented status.
    }

    fn retire_checked(&self) -> Result<(), ConnectError> {
        let mut policy = None;
        {
            let mut state = self.state()?;
            state.closed = true;
            if let Some(attempt) = state.attempt.as_mut() {
                if matches!(
                    attempt.custody,
                    Custody::AwaitingDecision | Custody::DecisionRecorded(_)
                ) {
                    attempt.custody = Custody::RetiredBeforeAdmission;
                    policy = attempt.policy_entry.take();
                }
                // After admission, missing settlement remains SpentUncertain.
                attempt.retired.send_replace(true);
            }
        }
        drop(policy);
        self.retirement.send_replace(());
        Ok(())
    }

    fn retire(&self) {
        let _ = self.retire_checked();
    }
}

/// One exclusively owned fresh native decision store and dispatch gate.
/// No reopen or saved-decision admission API is provided.
pub struct LocalApprovals {
    shared: Arc<Shared>,
    service_tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
}

/// The sole local terminal decision endpoint. It is deliberately not cloneable.
pub struct ApprovalPresenter {
    shared: Arc<Shared>,
    prompts: mpsc::Receiver<ApprovalPrompt>,
    // Presentation scheduling only. A mismatched live prompt remains with its
    // sole presenter and keeps its exact native response/approval identity.
    pending: Option<ApprovalPrompt>,
}

impl LocalApprovals {
    // Exercise the existing per-request deadline after the exact prompt is
    // acknowledged. This is not owner retirement or operation-grant revocation.
    #[cfg(test)]
    pub(crate) fn fixture_expire_waiting_review(
        &self,
        id: &ApprovalId,
    ) -> Result<(), ConnectError> {
        let mut state = self.shared.live_state()?;
        if state.closed {
            return Err(invalid("fixture approval owner closed"));
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| {
                &attempt.approval_id == id && attempt.custody == Custody::AwaitingDecision
            })
            .ok_or_else(|| invalid("fixture requires this exact waiting approval"))?;
        attempt.deadline = Instant::now();
        Ok(())
    }

    /// `parent` is the product's already reserved private setup artifact directory.
    /// This creates another exclusive directory and retains native audit records.
    pub fn create(parent: &Path) -> Result<(Self, ApprovalPresenter), ConnectError> {
        Self::create_with(parent, DECISION_LIFETIME, &[])
    }

    /// Local terminal decisions for the bounded direct PairedHost composition.
    /// No remote decision or saved approval is accepted by this invocation.
    /// Attribution of this installed presenter only. The actual verified Human
    /// and task/login checks remain with the caller and native decision owner.
    pub(crate) fn create_console_setup(
        parent: &Path,
    ) -> Result<(Self, ApprovalPresenter), ConnectError> {
        let actor = ApprovalPrincipalId::new("connect-console-presenter")
            .map_err(|error| ConnectError::native("identify Console presenter", error))?;
        Self::create_for_actor(parent, DECISION_LIFETIME, &[], actor, None)
    }

    pub fn create_paired_host(parent: &Path) -> Result<(Self, ApprovalPresenter), ConnectError> {
        Self::create_with(
            parent,
            DECISION_LIFETIME,
            PAIRED_HOST_FRONTEND_CONSENT_TOOLS,
        )
    }

    fn create_with(
        parent: &Path,
        lifetime: Duration,
        consent_tools: &'static [&'static str],
    ) -> Result<(Self, ApprovalPresenter), ConnectError> {
        let actor = ApprovalPrincipalId::new("connect-local-terminal")
            .map_err(|e| ConnectError::native("identify local decision presenter", e))?;
        Self::create_for_actor(parent, lifetime, consent_tools, actor, None)
    }

    /// Private-pipe construction after validation of the retained host login.
    /// This records attribution only; the native policy still owns permission.
    pub(crate) fn create_browser(
        parent: &Path,
        actor: ApprovalPrincipalId,
        expires_at: SystemTime,
    ) -> Result<(Self, ApprovalPresenter), ConnectError> {
        Self::create_for_actor(
            parent,
            DECISION_LIFETIME,
            PAIRED_HOST_FRONTEND_CONSENT_TOOLS,
            actor,
            Some(expires_at),
        )
    }

    fn create_for_actor(
        parent: &Path,
        lifetime: Duration,
        consent_tools: &'static [&'static str],
        actor: ApprovalPrincipalId,
        actor_expires_at: Option<SystemTime>,
    ) -> Result<(Self, ApprovalPresenter), ConnectError> {
        let (approvals, prompts) = Self::create_owner(
            parent,
            lifetime,
            consent_tools,
            actor,
            actor_expires_at,
            None,
        )?;
        let presenter = ApprovalPresenter {
            shared: approvals.shared.clone(),
            prompts,
            pending: None,
        };
        Ok((approvals, presenter))
    }

    /// Trusted application composition for software only. No human presenter
    /// is created, and account/service review still requires its own owner.
    pub fn create_for_installation_policy(
        parent: &Path,
        policy: InstalledInstallationPolicy,
    ) -> Result<Self, ConnectError> {
        let actor = policy
            .actor()
            .map_err(|_| invalid("The installed provisioning policy is unavailable"))?;
        let (approvals, prompts) = Self::create_owner(
            parent,
            DECISION_LIFETIME,
            &[],
            actor,
            None,
            Some(InstallationPolicySource::Installed(policy)),
        )?;
        drop(prompts);
        Ok(approvals)
    }

    /// The factory can be prepared before final adoption. The pending source
    /// refuses use until its actual installed owner is supplied once. Optional
    /// presentation is the same real terminal/Console owner, not policy authority.
    #[cfg(all(unix, feature = "console"))]
    pub(crate) fn create_for_selected_policy(
        parent: &Path,
        selected: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
        console: Option<bool>,
    ) -> Result<(Self, Option<ApprovalPresenter>), ConnectError> {
        let actor =
            match console {
                Some(true) => ApprovalPrincipalId::new("connect-console-presenter"),
                Some(false) => ApprovalPrincipalId::new("connect-local-terminal"),
                None => ApprovalPrincipalId::new(serde_json::to_string(selected.owner()).map_err(
                    |error| ConnectError::native("identify selected application", error),
                )?),
            }
            .map_err(|error| ConnectError::native("identify selected decision owner", error))?;
        let (approvals, prompts) = Self::create_owner(
            parent,
            DECISION_LIFETIME,
            &[],
            actor,
            None,
            Some(InstallationPolicySource::Selected(selected)),
        )?;
        let presenter = console.map(|_| ApprovalPresenter {
            shared: approvals.shared.clone(),
            prompts,
            pending: None,
        });
        Ok((approvals, presenter))
    }

    fn create_owner(
        parent: &Path,
        lifetime: Duration,
        consent_tools: &'static [&'static str],
        actor: ApprovalPrincipalId,
        actor_expires_at: Option<SystemTime>,
        installation_policy: Option<InstallationPolicySource>,
    ) -> Result<(Self, mpsc::Receiver<ApprovalPrompt>), ConnectError> {
        if !parent.is_absolute()
            || !std::fs::symlink_metadata(parent)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(invalid(
                "Approval artifacts require an existing absolute directory",
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("consent-")
            .tempdir_in(parent)
            .map_err(|e| ConnectError::native("reserve invocation approval artifacts", e))?
            .keep();
        let store_path = directory.join("native-decisions.json");
        let store = FileApprovalStore::open(&store_path)
            .map_err(|e| ConnectError::native("open invocation approval store", e))?;
        let service = ApprovalService::with_store(Arc::new(store))
            .map_err(|e| ConnectError::native("open native approval service", e))?;
        let (tx, rx) = mpsc::channel(1);
        let (retirement, _) = watch::channel(());
        let shared = Arc::new(Shared {
            service,
            installation_policy,
            actor,
            actor_expires_at,
            store_path,
            lifetime,
            consent_tools,
            state: Mutex::new(State {
                session: None,
                closed: false,
                attempt: None,
            }),
            prompts: tx,
            retirement,
        });
        Ok((
            Self {
                shared: shared.clone(),
                service_tasks: None,
            },
            rx,
        ))
    }

    pub(crate) fn with_service_tasks(
        mut self,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
    ) -> Self {
        self.service_tasks = tasks;
        self
    }

    pub fn store_path(&self) -> &Path {
        &self.shared.store_path
    }

    /// Present an exact access review supplied by the authenticated host owner.
    /// The owner retains the verified account, operation, grantee and lifetime
    /// selection privately and must match this record before native issuance.
    /// An approval body or this returned record is not native permission.
    ///
    /// This uses the same terminal/console presenter and one live attempt as
    /// software review. A host may review software and access together when it
    /// holds both exact selections; this method does not release an installed
    /// tool call. Existing authorized access needs no request to this method.
    /// The owner may retain the record while settling/closing a standalone
    /// setup session, then finalize through the native administrative owner.
    pub async fn review_access(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalRecord, ConnectError> {
        self.review_access_cancellable(request, std::future::pending(), |_| {})
            .await
    }

    /// Cancellation belongs to this exact waiter. The existing PendingGuard
    /// retires its approval ID; neither the presenter nor other attempts close.
    pub(crate) async fn review_access_cancellable(
        &self,
        request: ApprovalRequest,
        cancelled: impl std::future::Future<Output = ()>,
        created: impl FnOnce(&ApprovalId),
    ) -> Result<ApprovalRecord, ConnectError> {
        tokio::pin!(cancelled);
        // Earlier account/status work may have been joined after caller loss.
        // Do not create a fresh approval record for that retired request.
        if futures_util::FutureExt::now_or_never(cancelled.as_mut()).is_some() {
            return Err(ConnectError::ReviewCancelled);
        }
        let now = SystemTime::now();
        let remaining = request
            .expires_at
            .map(SystemTime::from)
            .and_then(|expiry| expiry.duration_since(now).ok())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| invalid("The access review requires a future decision deadline"))?;
        if request.allowed_decisions
            != [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect()
            || request.proposed_action.summary.trim().is_empty()
            || request.proposed_action.body.is_none()
            || serde_json::to_vec(&request)
                .map_err(|error| ConnectError::native("encode exact access review", error))?
                .len()
                > MAX_ACCESS_REVIEW_BYTES
        {
            return Err(invalid(
                "The access review must retain its exact bounded proposal and approve/deny choices",
            ));
        }
        let deadline = Instant::now()
            .checked_add(remaining.min(self.shared.lifetime))
            .ok_or_else(|| invalid("The access review deadline is invalid"))?;
        let (response, receive) = oneshot::channel();
        let (retired, retirement) = watch::channel(false);
        let record = {
            let mut state = self.shared.live_state()?;
            if state.closed
                || !matches!(&request.owner,
                ApprovalOwnerRef::Session { session_id } if state.session.as_ref() == Some(session_id))
                || state.attempt.as_ref().is_some_and(|attempt| {
                    matches!(
                        attempt.custody,
                        Custody::AwaitingDecision
                            | Custody::DecisionRecorded(_)
                            | Custody::SpentUncertain
                    )
                })
            {
                return Err(invalid(
                    "The access review has no available matching setup invocation",
                ));
            }
            let record = self
                .shared
                .service
                .request(request.clone())
                .map_err(|error| ConnectError::native("retain native access review", error))?;
            state.attempt = Some(Attempt {
                approval_id: record.approval_id.clone(),
                input: AttemptInput::AccessReview(Box::new(request)),
                review_work: None,
                review_scope: None,
                dispatch_error: None,
                deadline,
                custody: Custody::AwaitingDecision,
                held: None,
                service: None,
                service_entry: None,
                policy_entry: None,
                readback: None,
                retired,
            });
            record
        };
        let guard = PendingGuard {
            shared: self.shared.clone(),
            id: record.approval_id.clone(),
        };
        self.shared
            .prompts
            .try_send(ApprovalPrompt {
                record,
                presentation: ApprovalPresentation::AccessReview,
                service_task: None,
                shared: self.shared.clone(),
                response: Some(response),
                retired: retirement,
            })
            .map_err(|_| invalid("The access review presenter is unavailable"))?;
        created(&guard.id);
        let decision = tokio::select! { biased;
            _ = cancelled.as_mut() => return Err(ConnectError::ReviewCancelled),
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receive) => result
                .map_err(|_| invalid("The access review expired without a returned decision"))?
                .map_err(|_| invalid("The access review ended without a returned decision"))?,
        };
        let mut state = self.shared.live_state()?;
        if state.closed {
            return Err(invalid("The access review invocation has ended"));
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| attempt.approval_id == guard.id)
            .ok_or_else(|| invalid("The access review no longer has its retained invocation"))?;
        let AttemptInput::AccessReview(request) = &attempt.input else {
            return Err(invalid("The retained approval is not this access review"));
        };
        let native = self
            .shared
            .service
            .get(&guard.id)
            .map_err(|error| ConnectError::native("read native access decision", error))?;
        let retained = ApprovalRequest {
            requester: native.requester.clone(),
            owner: native.owner.clone(),
            resource: native.resource.clone(),
            proposed_action: native.proposed_action.clone(),
            risk: native.risk,
            request_body: native.request_body.clone(),
            allowed_decisions: native.allowed_decisions.clone(),
            expires_at: native.expires_at,
            metadata: native.metadata.clone(),
            request_provenance: native.request_provenance.clone(),
        };
        let expected_status = match decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Deny => ApprovalStatus::Denied,
        };
        if Instant::now() >= attempt.deadline
            || retained != **request
            || native.status != expected_status
            || native.decision.as_ref().is_none_or(|record| {
                record.decision != decision || record.actor != self.shared.actor
            })
            || attempt.custody != Custody::DecisionRecorded(decision)
        {
            return Err(invalid(
                "The native access decision does not match the retained review",
            ));
        }
        // Only the human decision was returned. There was no grant mutation,
        // package dispatch, attachment or business operation to call committed.
        attempt.custody = Custody::DecisionReturned;
        attempt.retired.send_replace(true);
        Ok(native)
    }

    pub(crate) async fn await_service_dispatch(
        &self,
        owner: &crate::service_install::CheckedInstalls,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<SoftwareReviewOutcome, ToolError> {
        let refused = || ToolError::access_denied(call.name);
        let session = context.origin_session_id().ok_or_else(refused)?;
        self.check_session(session).map_err(|_| refused())?;
        if !crate::service_install::INSTALL_TOOLS.contains(&call.name)
            || meerkat_core::AgentToolDispatcher::live_bridge_effect_kind(owner, call.name)
                != LiveBridgeEffectKind::ExternalIo
            || call.id.len() > 512
            || call.args.get().len() > MAX_ARGUMENT_BYTES
        {
            return Err(refused());
        }
        // The held factory has no installed policy before final adoption.
        // Refuse that state before an unavailable task source can obscure it.
        if let Some(source) = &self.shared.installation_policy {
            source
                .resolve()
                .map_err(|error| installation_policy::tool_refusal(call.name, error))?;
        }
        let held = owner.retain_action(call, session)?;
        let mut reviewed = held.review();
        reviewed["tool"] = call.name.into();
        reviewed["call_id"] = call.id.into();
        // The installed source selects the same canonical waiting task without
        // requiring service access authority. Its currentness is re-opened after
        // the decision, including on the application-policy branch.
        let task = match &self.service_tasks {
            Some(tasks) => tasks
                .prepare_software(call, context, plan)
                .await
                .map_err(|_| refused())?,
            None => None,
        };
        if self.shared.installation_policy.is_some()
            && self.service_tasks.is_some()
            && task.is_none()
        {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            ));
        }
        let policy_entry = match &self.shared.installation_policy {
            Some(source) => {
                let refreshed = context.check_tool_authorization(call, plan)?;
                let policy = source
                    .resolve()
                    .map_err(|error| installation_policy::tool_refusal(call.name, error))?;
                let entry = match policy {
                    Some(policy) => policy
                        .reserve(
                            held.realm(),
                            call,
                            refreshed.as_ref().unwrap_or(context),
                            &reviewed,
                        )
                        .map_err(|error| installation_policy::tool_refusal(call.name, error))?,
                    None => None,
                };
                // Only an actual human-required decision selects this same
                // invocation's real presenter. Denial/staleness never retries it.
                if entry.is_none() && self.shared.prompts.is_closed() {
                    return Ok(SoftwareReviewOutcome::RequiresHuman);
                }
                entry
            }
            None => None,
        };
        self.check_session(session).map_err(|_| refused())?;
        self.shared
            .review_dispatch(
                call,
                Some(context),
                Some(HeldAction::Service(held)),
                reviewed,
                task,
                policy_entry,
                |held| match held {
                    HeldAction::Service(held) => owner.arm_action(held),
                    _ => Err(refused()),
                },
            )
            .await?;
        Ok(SoftwareReviewOutcome::Approved)
    }

    pub(crate) fn service_dispatch_guard(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ServiceDispatchGuard, ToolError> {
        let refused = || ToolError::access_denied(call.name);
        let state = self.shared.state().map_err(|_| refused())?;
        if context.origin_session_id() != state.session.as_ref() {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_ref()
            .filter(|attempt| {
                attempt.custody == Custody::SpentUncertain
                    && matches!(&attempt.input, AttemptInput::Dispatch { call_id, name, arguments }
                if call_id == call.id && name == call.name && arguments == call.args.get())
            })
            .ok_or_else(refused)?;
        Ok(ServiceDispatchGuard {
            shared: self.shared.clone(),
            id: attempt.approval_id.clone(),
        })
    }

    pub(crate) fn check_service_entry(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<(), ToolError> {
        let refused = || ToolError::access_denied(call.name);
        // Service access holds this phase while checking approvals/session
        // currentness. Never take the phase lock while holding Shared.state.
        if let Some(source) = &self.shared.installation_policy
            && let Some(policy) = source
                .resolve()
                .map_err(|error| installation_policy::tool_refusal(call.name, error))?
        {
            policy
                .check_entry()
                .map_err(|error| installation_policy::tool_refusal(call.name, error))?;
        }
        let state = self.shared.live_state().map_err(|_| refused())?;
        if state.closed || context.origin_session_id() != state.session.as_ref() {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_ref()
            .filter(|attempt| {
                attempt.custody == Custody::SpentUncertain
                    && matches!(&attempt.input, AttemptInput::Dispatch { call_id, name, arguments }
                if call_id == call.id && name == call.name && arguments == call.args.get())
            })
            .ok_or_else(refused)?;
        if let Some(entry) = &attempt.policy_entry {
            entry
                .check_current()
                .map_err(|error| installation_policy::tool_refusal(call.name, error))?;
        }
        if let Some(service) = &attempt.service
            && service.task.console_login().is_some()
            && (attempt.service_entry.is_none()
                || (attempt.policy_entry.is_none()
                    && service
                        .decision_current
                        .as_ref()
                        .is_none_or(|current| !current())))
        {
            return Err(refused());
        }
        Ok(())
    }

    pub(crate) fn settle_service_result(
        &self,
        owner: &crate::service_install::CheckedInstalls,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        outcome: LiveBridgeEffectOutcome,
        result: Result<meerkat_core::ops::ToolDispatchOutcome, ToolError>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let dispatch_error = result
            .as_ref()
            .err()
            .map(|error| meerkat_core::ToolDispatchTerminalErrorKind::from(error.primary_error()));
        let failure = self
            .shared
            .settle_service_dispatch(owner, call, context, outcome, dispatch_error)
            .err()
            .map(|error| meerkat_core::ToolDispatchSettlementFailure {
                admission_source: meerkat_core::ToolDispatchAdmissionSource::ConfiguredGate,
                effect_kind: LiveBridgeEffectKind::ExternalIo,
                physical_outcome: outcome,
                failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::from(
                    error.primary_error(),
                ),
            });
        match result {
            Ok(mut result) => {
                result.result.settlement_failures.extend(failure);
                Ok(result)
            }
            Err(error) => Err(error.with_settlement_failures(failure.into_iter().collect())),
        }
    }

    /// Observe only this live owner's latest retained software attempt. This
    /// never searches approval history, renews permission or recommends retry.
    pub(crate) async fn software_review_observation(
        &self,
        owner: &crate::service_install::CheckedInstalls,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<serde_json::Value, ToolError> {
        let refused = || ToolError::access_denied(call.name);
        if call.name != "setup_state" {
            return Err(refused());
        }
        let session = context.origin_session_id().ok_or_else(refused)?;
        self.check_session(session).map_err(|_| refused())?;
        let (id, task) = {
            let state = self.shared.live_state().map_err(|_| refused())?;
            if state.closed || state.session.as_ref() != Some(session) {
                return Err(refused());
            }
            let Some(attempt) = state
                .attempt
                .as_ref()
                .filter(|attempt| matches!(attempt.readback, Some(ReadbackTarget::Service(_))))
            else {
                return Ok(serde_json::json!({
                    "scope":"latest_retained_software_attempt_only", "review":null,
                }));
            };
            if attempt.review_work.as_ref() != context.retained_work()
                || attempt.review_scope.as_ref()
                    != context
                        .work_authorization()
                        .map(|authorization| authorization.execution_scope())
            {
                return Err(refused());
            }
            (
                attempt.approval_id.clone(),
                attempt.service.as_ref().map(|service| service.task.clone()),
            )
        };
        if let Some(task) = task {
            task.check_setup_current().await.map_err(|_| refused())?;
        }
        context.check_tool_authorization(call, plan)?;
        let state = self.shared.live_state().map_err(|_| refused())?;
        if state.closed || state.session.as_ref() != Some(session) {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_ref()
            .filter(|attempt| {
                attempt.approval_id == id
                    && attempt.review_work.as_ref() == context.retained_work()
                    && attempt.review_scope.as_ref()
                        == context
                            .work_authorization()
                            .map(|authorization| authorization.execution_scope())
            })
            .ok_or_else(refused)?;
        let (
            AttemptInput::Dispatch {
                call_id,
                name,
                arguments,
            },
            Some(ReadbackTarget::Service(target)),
        ) = (&attempt.input, &attempt.readback)
        else {
            return Err(refused());
        };
        let arguments =
            serde_json::value::RawValue::from_string(arguments.clone()).map_err(|_| refused())?;
        let reviewed_call = ToolCallView {
            id: call_id,
            name,
            args: &arguments,
        };
        let native = self.shared.service.get(&id).map_err(|_| refused())?;
        let mut review = owner.software_review_binding(target, reviewed_call, session, &native)?;
        review["approval_id"] = serde_json::json!(id);
        review["approval_status"] = serde_json::json!(native.status);
        review["decision"] =
            serde_json::json!(native.decision.as_ref().map(|decision| decision.decision));
        review["dispatch_custody"] = serde_json::json!(match attempt.custody {
            Custody::AwaitingDecision => "awaiting_decision",
            Custody::DecisionRecorded(_) => "decision_recorded",
            Custody::DecisionReturned => "decision_returned",
            Custody::RetiredBeforeAdmission => "retired_before_admission",
            Custody::SpentUncertain => "spent_uncertain",
            Custody::Settled(_) => "settled",
        });
        review["effect_outcome"] = match attempt.custody {
            Custody::Settled(LiveBridgeEffectOutcome::Committed) => serde_json::json!("committed"),
            Custody::Settled(LiveBridgeEffectOutcome::Failed) => serde_json::json!("failed"),
            Custody::Settled(LiveBridgeEffectOutcome::Unknown) => serde_json::json!("unknown"),
            _ => serde_json::Value::Null,
        };
        review["dispatch_error_kind"] = serde_json::json!(attempt.dispatch_error);
        Ok(serde_json::json!({"scope":"latest_retained_software_attempt_only", "review":review}))
    }

    /// Add the process-local gate without replacing any caller authority.
    /// This accepts only the existing packaged stdio preparation composition.
    pub fn attach_preparation(&self, options: &mut SessionOptions) -> Result<(), ConnectError> {
        self.attach(options, None)
    }

    /// Permit the concrete captured-host inventory/candidate binding already composed by
    /// PreparedSetupConnection. This does not enable consent-requiring effects.
    pub fn attach_paired_host_preparation(
        &self,
        options: &mut SessionOptions,
        paired_host: &PairedHostFrontend,
    ) -> Result<(), ConnectError> {
        self.attach(options, Some(paired_host))
    }

    fn attach(
        &self,
        options: &mut SessionOptions,
        paired_host: Option<&PairedHostFrontend>,
    ) -> Result<(), ConnectError> {
        let expected_external = match (paired_host, &options.build.external_tools) {
            (None, None) => true,
            (Some(binding), Some(dispatcher)) => binding.matches(dispatcher),
            _ => false,
        };
        if !self.shared.consent_tools.is_empty()
            || options.build.tool_dispatch_admission.is_some()
            || options.build.resume_session.is_some()
            || !expected_external
            || options.build.tool_access_policy.is_none()
            || options.build.mcp_servers.len() != 1
            || options.build.mcp_servers[0].name != "toolkit-setup"
            || !matches!(
                options.build.mcp_servers[0].transport,
                McpTransportConfig::Stdio(_)
            )
        {
            return Err(invalid(
                "Local approvals require the fresh packaged stdio setup session",
            ));
        }
        if meerkat_core::ToolExecutionPolicy::resolve(
            options
                .build
                .tool_access_policy
                .clone()
                .ok_or_else(|| invalid("Setup tool policy is missing"))?,
        )
        .map_err(|e| ConnectError::native("validate local approval tool ceiling", e))?
        .is_unrestricted()
        {
            return Err(invalid(
                "The local approval composition requires a restricted native tool ceiling",
            ));
        }
        options.build.tool_dispatch_admission = Some(Arc::new(Admission(
            self.shared.clone(),
            paired_host.is_some(),
            None,
            None,
            None,
            None,
        )));
        Ok(())
    }

    /// Attach only to this captured direct host's fresh session. The policy
    /// remains native and conjoined with the caller's ceiling.
    pub fn attach_paired_host_direct(
        &self,
        options: &mut SessionOptions,
        paired_host: &PairedHostFrontend,
    ) -> Result<(), ConnectError> {
        self.attach_paired_host_consent(options, paired_host, false)
    }

    /// Attach the same native per-effect approval gate to the exact combined
    /// selected stdio preparation and local PairedHost composition.
    pub fn attach_paired_host_interactive(
        &self,
        options: &mut SessionOptions,
        paired_host: &PairedHostFrontend,
    ) -> Result<(), ConnectError> {
        self.attach_paired_host_consent(options, paired_host, true)
    }

    fn attach_paired_host_consent(
        &self,
        options: &mut SessionOptions,
        paired_host: &PairedHostFrontend,
        with_preparation: bool,
    ) -> Result<(), ConnectError> {
        let expected_composition = if with_preparation {
            crate::setup::is_interactive_paired_host_composition(options)
        } else {
            crate::setup::is_direct_paired_host_composition(options)
        };
        if self.shared.consent_tools != PAIRED_HOST_FRONTEND_CONSENT_TOOLS
            || options.build.tool_dispatch_admission.is_some()
            || options.build.resume_session.is_some()
            || !options
                .build
                .external_tools
                .as_ref()
                .is_some_and(|tools| paired_host.matches(tools))
            || !expected_composition
        {
            return Err(invalid(
                "Local PairedHost consent requires its exact fresh local host composition",
            ));
        }
        let mut allowed = paired_host.allowed_tools();
        if with_preparation {
            allowed.extend(PREPARATION_TOOLS.iter().copied());
            allowed.extend([TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME]);
        }
        let ceiling = meerkat_core::ops::ToolAccessPolicy::AllowList(allowed.into_iter().collect());
        options.build.tool_access_policy = Some(
            options
                .build
                .tool_access_policy
                .take()
                .ok_or_else(|| invalid("Local PairedHost policy is missing"))?
                .conjoin(ceiling)
                .map_err(|e| ConnectError::native("conjoin PairedHost consent ceiling", e))?,
        );
        options.build.tool_dispatch_admission = Some(Arc::new(Admission(
            self.shared.clone(),
            with_preparation,
            Some(paired_host.clone()),
            None,
            None,
            None,
        )));
        Ok(())
    }

    /// Attach the existing local decision owner to this exact lazy composition.
    /// The separately selected service composition permits an exact checked
    /// install decision; neither handoff nor decision grants MCP management.
    pub fn attach_lazy_setup(
        &self,
        options: &mut SessionOptions,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<(), ConnectError> {
        if self.shared.consent_tools != PAIRED_HOST_FRONTEND_CONSENT_TOOLS
            || options.build.tool_dispatch_admission.is_some()
            || !crate::setup::is_direct_paired_host_composition(options)
            || !options
                .build
                .external_tools
                .as_ref()
                .is_some_and(|tools| lazy.matches(tools))
        {
            return Err(invalid(
                "Local lazy setup consent requires its exact fresh composition",
            ));
        }
        let ceiling = meerkat_core::ops::ToolAccessPolicy::AllowList(
            lazy.allowed_tools().into_iter().collect(),
        );
        options.build.tool_access_policy = Some(
            options
                .build
                .tool_access_policy
                .take()
                .ok_or_else(|| invalid("Local lazy setup policy is missing"))?
                .conjoin(ceiling)
                .map_err(|error| {
                    ConnectError::native("conjoin lazy setup consent ceiling", error)
                })?,
        );
        options.build.tool_dispatch_admission = Some(self.native_setup_admission(lazy)?);
        Ok(())
    }

    /// Forward this same private approval owner after native construction has
    /// supplied its actual adapter and checked-install owner. No grant or
    /// session admission is created by this handoff.
    pub(crate) fn native_setup_admission(
        &self,
        lazy: &crate::service_preparation::LazySetup,
    ) -> Result<Arc<dyn ToolDispatchAdmission>, ConnectError> {
        if self.shared.consent_tools != PAIRED_HOST_FRONTEND_CONSENT_TOOLS {
            return Err(invalid("The selected setup approval owner is incompatible"));
        }
        Ok(Arc::new(Admission(
            self.shared.clone(),
            true,
            Some(lazy.paired_host().clone()),
            Some(lazy.owner()),
            Some(lazy.forge().clone()),
            lazy.installs(),
        )))
    }

    /// Only effects with a retained owner reference permit uncertain readback.
    /// An unconfirmed create has no such reference and cannot recover an ID.
    /// This does not restore its decision or permit another effect.
    pub fn permits_uncertain_status_readback(&self) -> bool {
        self.shared.state.lock().is_ok_and(|state| {
            state.attempt.as_ref().is_some_and(|attempt| {
                attempt.custody == Custody::SpentUncertain
                    && attempt
                        .readback
                        .as_ref()
                        .is_some_and(|target| match target {
                            ReadbackTarget::Service(target) => target.available(),
                            _ => true,
                        })
            })
        })
    }

    /// Bind once to this invocation's exact native session ID before submitting
    /// input. A protected pre-open review may bind the preassigned ID before
    /// materialization; this attribution grants no execution authority.
    pub fn bind_session(&self, session: &SessionId) -> Result<(), ConnectError> {
        let mut state = self.shared.live_state()?;
        if state.closed || state.session.is_some() {
            return Err(invalid("The local approval invocation cannot be rebound"));
        }
        state.session = Some(session.clone());
        Ok(())
    }

    /// Check this same private presenter owner, without creating or rebinding
    /// a session. This is review eligibility, not native operation authority.
    pub(crate) fn check_session(&self, session: &SessionId) -> Result<(), ConnectError> {
        let state = self.shared.live_state()?;
        if state.closed || state.session.as_ref() != Some(session) {
            return Err(invalid(
                "The private review owner does not cover this live session",
            ));
        }
        Ok(())
    }

    /// Stop eligibility before cancelling/closing native execution. This does
    /// not claim that any already admitted physical effect failed to happen.
    pub fn retire(&self) {
        self.shared.retire();
    }

    pub(crate) fn retire_checked(&self) -> Result<(), ConnectError> {
        self.shared.retire_checked()
    }

    pub fn has_uncertain_dispatch(&self) -> bool {
        self.shared.state.lock().map_or(true, |state| {
            state
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.custody == Custody::SpentUncertain)
        })
    }
}

impl Drop for LocalApprovals {
    fn drop(&mut self) {
        self.retire();
    }
}

impl ApprovalPresenter {
    pub async fn next(&mut self) -> Option<ApprovalPrompt> {
        let mut retirement = self.shared.retirement.subscribe();
        loop {
            // The notification only wakes this read of the existing owner.
            if self.shared.state().map_or(true, |state| state.closed) {
                return None;
            }
            if let Some(prompt) = self.pending.take()
                && prompt
                    .response
                    .as_ref()
                    .is_some_and(|response| !response.is_closed())
            {
                return Some(prompt);
            }
            tokio::select! {
                biased;
                _ = retirement.changed() => {}
                prompt = self.prompts.recv() => match prompt {
                    // Only this exact obsolete sender is discarded. A live
                    // sibling prompt is never consumed to cancel another one.
                    Some(prompt) if prompt.response.as_ref().is_none_or(oneshot::Sender::is_closed) => {},
                    prompt => return prompt,
                },
            }
        }
    }
}

impl ApprovalPresenter {
    pub(crate) async fn next_for_review(
        &mut self,
        id: &ApprovalId,
    ) -> Result<Option<ApprovalPrompt>, ConnectError> {
        match self.next().await {
            Some(prompt) if &prompt.record.approval_id != id => {
                self.pending = Some(prompt);
                Err(invalid("The retained prompt belongs to another request"))
            }
            prompt => Ok(prompt),
        }
    }
}

impl Drop for ApprovalPresenter {
    fn drop(&mut self) {
        self.shared.retire();
    }
}

enum ApprovalPresentation {
    ToolCall,
    AccessReview,
    SuppliedSetup,
    ControllerSetup(String),
}

/// Exact native request held by a live admission future. These nonsecret fields
/// are supplied by the bounded host tool contract, not a model's approval claim.
pub struct ApprovalPrompt {
    record: ApprovalRecord,
    presentation: ApprovalPresentation,
    service_task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
    shared: Arc<Shared>,
    response: Option<oneshot::Sender<ApprovalDecision>>,
    retired: watch::Receiver<bool>,
}

impl ApprovalPrompt {
    pub fn record(&self) -> &ApprovalRecord {
        &self.record
    }

    /// A local terminal decision. Remote actors and saved IDs cannot use this
    /// endpoint; only the currently retained prompt owns its one-shot response.
    pub fn decide(&mut self, decision: ApprovalDecision) -> Result<(), ConnectError> {
        self.decide_current(decision, None)
    }
    pub(crate) fn console_task(
        &self,
    ) -> Option<&toolkit_mobkit::private_originals::InstalledServiceTask> {
        self.service_task
            .as_ref()
            .filter(|task| task.console_login().is_some())
    }
    pub(crate) fn decide_console(
        &mut self,
        decision: ApprovalDecision,
        current: crate::installed_service_review::account_request::console::DecisionCheck,
    ) -> Result<(), ConnectError> {
        if self.console_task().is_none() {
            return Err(invalid("The software prompt has no Console task"));
        }
        self.decide_current(decision, Some(current))
    }
    fn decide_current(
        &mut self,
        decision: ApprovalDecision,
        current: Option<crate::installed_service_review::account_request::console::DecisionCheck>,
    ) -> Result<(), ConnectError> {
        let mut state = self.shared.live_state()?;
        if state.closed {
            return Err(invalid("This local approval invocation has ended"));
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| attempt.approval_id == self.record.approval_id)
            .ok_or_else(|| invalid("This approval no longer has a live invocation"))?;
        if attempt.custody != Custody::AwaitingDecision
            || Instant::now() >= attempt.deadline
            || self
                .response
                .as_ref()
                .is_none_or(oneshot::Sender::is_closed)
        {
            return Err(invalid(
                "This approval is no longer eligible; no dispatch was released",
            ));
        }
        let login = attempt
            .service
            .as_ref()
            .and_then(|service| service.task.console_login());
        if login.is_some() && current.as_ref().is_none_or(|current| !current()) {
            return Err(invalid("The exact Console decision is unavailable"));
        }
        let reservation = login
            .map(|login| login.reserve())
            .transpose()
            .map_err(|_| invalid("The Console login refused approval entry"))?;
        if current.as_ref().is_some_and(|current| !current()) {
            if let Some(reservation) = reservation {
                reservation.settled();
            }
            return Err(invalid("The Console decision changed before entry"));
        }
        self.shared
            .service
            .decide(
                &attempt.approval_id,
                decision,
                self.shared.actor.clone(),
                None,
                None,
            )
            .map_err(|e| ConnectError::native("record native approval decision", e))?;
        if let Some(reservation) = reservation {
            reservation.settled();
        }
        if let Some(service) = &mut attempt.service {
            service.decision_current = current;
        }
        attempt.custody = Custody::DecisionRecorded(decision);
        self.response
            .take()
            .ok_or_else(|| invalid("Approval response was already used"))?
            .send(decision)
            .map_err(|_| {
                invalid("Native decision retained, but the invocation ended; do not replay it")
            })?;
        Ok(())
    }

    pub(crate) async fn closed(&mut self) {
        while !*self.retired.borrow_and_update() {
            if self.retired.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Lives on the actual install dispatch future, not its presentation. A
/// dropped future cannot keep the retirement drain open forever. The existing
/// attempt remains SpentUncertain and retains readback and replay exclusion.
pub(crate) enum SoftwareReviewOutcome {
    Approved,
    RequiresHuman,
}

pub(crate) struct ServiceDispatchGuard {
    shared: Arc<Shared>,
    id: ApprovalId,
}
impl Drop for ServiceDispatchGuard {
    fn drop(&mut self) {
        // Poison still denies future admission, but must not retain a policy
        // mutation reservation after the physical future has disappeared.
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut policy = None;
        if let Some(attempt) = state
            .attempt
            .as_mut()
            .filter(|attempt| attempt.approval_id == self.id)
        {
            policy = attempt.policy_entry.take();
            if attempt.custody == Custody::SpentUncertain
                && let Some(entry) = attempt.service_entry.take()
            {
                entry.uncertain();
            }
        }
        drop(state);
        drop(policy);
    }
}

struct PendingGuard {
    shared: Arc<Shared>,
    id: ApprovalId,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.shared.retire_attempt(&self.id);
    }
}

impl Shared {
    // One native decision/custody path shared by the standalone admission and
    // the selected checked-install owner. The callback borrows its actual
    // effect owner; Shared never retains a CheckedInstalls Arc back-edge.
    async fn review_dispatch(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        held: Option<HeldAction>,
        reviewed: serde_json::Value,
        task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
        policy_entry: Option<PolicyEntry>,
        arm: impl FnOnce(HeldAction) -> Result<(), ToolError> + Send,
    ) -> Result<(), ToolError> {
        let refused = || ToolError::access_denied(call.name);
        let (response, receive) = oneshot::channel();
        let (retired, retirement) = watch::channel(false);
        let (deadline, expires) = dispatch_review_deadline(
            self.lifetime,
            task.as_ref().map(|task| task.deadline_ms()),
            Instant::now(),
            SystemTime::now(),
        )
        .ok_or_else(refused)?;
        let policy_provenance = policy_entry
            .as_ref()
            .map(|entry| entry.provenance().clone());
        let policy_decided = policy_entry.is_some();
        let retain = |policy_entry: Option<PolicyEntry>| -> Result<ApprovalRecord, ToolError> {
            let mut state = self.live_state().map_err(|_| refused())?;
            if state.closed
                || state.attempt.as_ref().is_some_and(|attempt| {
                    matches!(
                        attempt.custody,
                        Custody::AwaitingDecision
                            | Custody::DecisionRecorded(_)
                            | Custody::SpentUncertain
                    )
                })
            {
                return Err(refused());
            }
            let record = self.service.request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("connect-setup-agent").map_err(|_| refused())?,
                owner: ApprovalOwnerRef::Session { session_id: state.session.clone().ok_or_else(refused)? },
                resource: ApprovalResourceRef { kind: ApprovalResourceKind::ToolCall,
                    id: ApprovalResourceId::new(call.name) },
                proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::ToolCall,
                    summary: if held.is_some() && reviewed["kind"] == "new_extension" {
                        "Request host-owned build and review of this pinned extension package. The receipt confirms the request was recorded; installation, activation and readiness require later host evidence.".into()
                    } else if held.is_some() && reviewed["kind"] == "secret_intent_create" {
                        "Create the reviewed PairedHost secret-entry intent. This supersedes older pending intents for the same slot and discards their pending values; it does not enter or confirm a secret.".into()
                    } else if held.is_some() && reviewed["kind"] == "secret_intent_cancel" {
                        "Cancel the reviewed PairedHost secret-entry intent and discard its pending value. This does not revoke or roll back a committed secret.".into()
                    } else if matches!(held, Some(HeldAction::Forge(_))) {
                        "Commit the exact reviewed Forge frontend configuration. This may trigger the application's existing activation or reconciliation and process messages already queued from the configured person; it does not provision credentials or prove activation or delivery.".into()
                    } else if matches!(held, Some(HeldAction::Service(_))) {
                        "Ask the selected package owner to install this exact checked software configuration once. The returned job and receipt are software-only observations; this does not authenticate an account, attach usable tools, grant access or complete the original task.".into()
                    } else if held.is_some() {
                        "Submit the reviewed change to PairedHost for operator review. This does not activate it.".into()
                    } else { format!("Run {} once in this setup invocation", call.name) },
                    body: Some(reviewed) },
                risk: ApprovalRisk::Medium, request_body: None,
                allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
                expires_at: Some(expires.into()), metadata: Default::default(),
                request_provenance: policy_provenance,
            }).map_err(|_| refused())?;
            if let Some(entry) = &policy_entry {
                let actor = entry.actor().map_err(|_| refused())?;
                self.service
                    .decide(
                        &record.approval_id,
                        ApprovalDecision::Approve,
                        actor,
                        None,
                        None,
                    )
                    .map_err(|_| refused())?;
            }
            state.attempt = Some(Attempt {
                approval_id: record.approval_id.clone(),
                input: AttemptInput::Dispatch {
                    call_id: call.id.into(),
                    name: call.name.into(),
                    arguments: call.args.get().into(),
                },
                review_work: context
                    .and_then(ToolDispatchContext::retained_work)
                    .cloned(),
                review_scope: context
                    .and_then(ToolDispatchContext::work_authorization)
                    .map(|authorization| authorization.execution_scope().clone()),
                dispatch_error: None,
                deadline,
                custody: if policy_decided {
                    Custody::DecisionRecorded(ApprovalDecision::Approve)
                } else {
                    Custody::AwaitingDecision
                },
                readback: held.as_ref().and_then(HeldAction::readback_target),
                service: task.clone().map(|task| ServiceReview {
                    task,
                    decision_current: None,
                }),
                service_entry: None,
                policy_entry,
                held,
                retired,
            });
            Ok(record)
        };
        let record = match policy_entry {
            // The file fence spans the same native request/decision and exact
            // attempt retention. Its phase observation precedes Shared.state.
            Some(entry) => entry
                .record(|entry| retain(Some(entry)))
                .map_err(|error| installation_policy::tool_refusal(call.name, error))??,
            None => retain(None)?,
        };
        let guard = PendingGuard {
            shared: self.clone(),
            id: record.approval_id.clone(),
        };
        let decision = if policy_decided {
            ApprovalDecision::Approve
        } else {
            self.prompts
                .try_send(ApprovalPrompt {
                    record,
                    presentation: ApprovalPresentation::ToolCall,
                    service_task: task.clone(),
                    shared: self.clone(),
                    response: Some(response),
                    retired: retirement,
                })
                .map_err(|_| refused())?;
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receive)
                .await
                .map_err(|_| refused())?
                .map_err(|_| refused())?
        };
        if let Some(task) = &task {
            task.check_setup_current().await.map_err(|_| refused())?;
        }
        let mut state = self.live_state().map_err(|_| refused())?;
        if state.closed {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| attempt.approval_id == guard.id)
            .ok_or_else(refused)?;
        let native = self.service.get(&guard.id).map_err(|_| refused())?;
        if Instant::now() >= attempt.deadline
            || decision != ApprovalDecision::Approve
            || native.status != ApprovalStatus::Approved
            || attempt.custody != Custody::DecisionRecorded(ApprovalDecision::Approve)
        {
            return Err(refused());
        }
        // Mark spent before releasing the one retained native dispatch. Until
        // an exact outcome arrives, including after future drop, it is uncertain.
        let reservation = if let Some(service) = &attempt.service {
            if service.task.console_login().is_some()
                && attempt.policy_entry.is_none()
                && service
                    .decision_current
                    .as_ref()
                    .is_none_or(|current| !current())
            {
                return Err(refused());
            }
            service
                .task
                .console_login()
                .map(|login| login.reserve())
                .transpose()
                .map_err(|_| refused())?
        } else {
            None
        };
        if let Some(held) = attempt.held.take()
            && let Err(error) = arm(held)
        {
            if let Some(reservation) = reservation {
                reservation.settled();
            }
            return Err(error);
        }
        attempt.service_entry = reservation;
        attempt.custody = Custody::SpentUncertain;
        attempt.retired.send_replace(true);
        drop(state);
        drop(guard);
        Ok(())
    }

    fn settle_service_dispatch(
        &self,
        owner: &crate::service_install::CheckedInstalls,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        outcome: LiveBridgeEffectOutcome,
        dispatch_error: Option<meerkat_core::ToolDispatchTerminalErrorKind>,
    ) -> Result<(), ToolError> {
        let refused = || ToolError::access_denied(call.name);
        // Settlement preserves a known result even after presenter retirement.
        // It cannot create a new decision or restore admission eligibility.
        let mut state = self.state().map_err(|_| refused())?;
        if context.origin_session_id() != state.session.as_ref() {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| {
                attempt.custody == Custody::SpentUncertain
                    && matches!(&attempt.input,
                    AttemptInput::Dispatch { call_id, name, arguments }
                    if call_id == call.id && name == call.name && arguments == call.args.get())
            })
            .ok_or_else(refused)?;
        attempt.dispatch_error = dispatch_error;
        let policy = attempt.policy_entry.take();
        if let Some(reservation) = attempt.service_entry.take() {
            if outcome == LiveBridgeEffectOutcome::Unknown {
                reservation.uncertain();
            } else {
                reservation.settled();
            }
        }
        if outcome != LiveBridgeEffectOutcome::Unknown {
            attempt.custody = Custody::Settled(outcome);
            // A poisoned cleanup lock does not replace the known inner result.
            // The poisoned owner cannot arm or enter another installation.
            let _ = owner.discard_unused_arm(call);
        }
        drop(state);
        drop(policy);
        Ok(())
    }
}

struct Admission(
    Arc<Shared>,
    bool,
    Option<PairedHostFrontend>,
    Option<Arc<crate::service_preparation::ServicePreparationOwner>>,
    Option<crate::forge::ForgeFrontend>,
    Option<Arc<crate::service_install::CheckedInstalls>>,
);

#[cfg(test)]
#[path = "service_install_tests.rs"]
mod service_install_tests;

fn ordinary_preparation(name: &str) -> bool {
    PREPARATION_TOOLS.contains(&name)
        || [TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME].contains(&name)
}

impl Admission {
    fn ordinary_call(&self, name: &str) -> bool {
        if crate::setup::ACQUISITION_TOOLS.contains(&name)
            && self
                .3
                .as_ref()
                .is_some_and(|owner| owner.allows_acquisition())
        {
            return true;
        }
        if self.5.is_some() && name == crate::service_install::SELECTED_CHECK {
            return true;
        }
        if crate::terminal_connector::TOOLS.contains(&name)
            && self
                .3
                .as_ref()
                .is_some_and(|owner| owner.allows_terminal_connectors())
        {
            return true;
        }
        if self.3.is_some()
            && (name == crate::service_preparation::TOOLKIT_PREPARE_SERVICES
                || (crate::forge::FORGE_TOOLS.contains(&name)
                    && name != crate::forge::FORGE_SUBMIT)
                || name == crate::public_document::READ_PUBLIC_DOCUMENT
                || crate::recipe_discovery::RECIPE_TOOLS.contains(&name))
        {
            return true;
        }
        if let Some(paired_host) = &self.2 {
            paired_host.read_tool(name)
                || paired_host.selection_tool(name)
                || (self.1 && ordinary_preparation(name))
        } else {
            ordinary_preparation(name)
                || (self.1 && PAIRED_HOST_FRONTEND_READ_TOOLS.contains(&name))
        }
    }
}

#[async_trait]
impl ToolDispatchAdmission for Admission {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        effect_kind: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        let refused = || ToolError::access_denied(call.name);
        {
            let state = self.0.live_state().map_err(|_| refused())?;
            if state.closed
                || state.session.is_none()
                || context.and_then(ToolDispatchContext::origin_session_id)
                    != state.session.as_ref()
                || context.is_some_and(|context| context.live_bridge_admission().is_some())
            {
                return Err(refused());
            }
            if let Some(attempt) = state.attempt.as_ref()
                && attempt.custody == Custody::SpentUncertain
                && !(attempt
                    .readback
                    .as_ref()
                    .is_some_and(|target| match target {
                        ReadbackTarget::PairedHost(target) => target.permits(call),
                        ReadbackTarget::Forge(key) => self
                            .4
                            .as_ref()
                            .is_some_and(|forge| forge.permits_readback(key, call)),
                        ReadbackTarget::Service(target) => self
                            .5
                            .as_ref()
                            .is_some_and(|owner| owner.permits_readback(target, call)),
                    }))
            {
                return Err(refused());
            }
        }
        // A native connection can finish after its activation waiter expired.
        // Its catalog/readiness cannot restore this local invocation's authority.
        if (ordinary_preparation(call.name)
            || crate::terminal_connector::TOOLS.contains(&call.name)
            || crate::setup::ACQUISITION_TOOLS.contains(&call.name)
            || self.5.is_some()
                && (call.name == crate::service_install::SELECTED_CHECK
                    || crate::service_install::INSTALL_TOOLS.contains(&call.name)))
            && self.3.as_ref().is_some_and(|owner| !owner.accepted())
        {
            return Err(refused());
        }
        // These operations retain their existing authorization. Passing them
        // does not consent to their descendants, retries or recovery actions.
        if self.ordinary_call(call.name) {
            return Ok(());
        }
        let expected_effect = if self.2.is_some()
            && PAIRED_HOST_FRONTEND_CONSENT_TOOLS.contains(&call.name)
            || self.4.is_some() && call.name == crate::forge::FORGE_SUBMIT
            || self.5.is_some() && crate::service_install::INSTALL_TOOLS.contains(&call.name)
        {
            LiveBridgeEffectKind::ExternalIo
        } else {
            LiveBridgeEffectKind::ToolDispatch
        };
        if !(self.0.consent_tools.contains(&call.name)
            || self.4.is_some() && call.name == crate::forge::FORGE_SUBMIT
            || self.5.is_some() && crate::service_install::INSTALL_TOOLS.contains(&call.name))
            || effect_kind != expected_effect
            || call.id.len() > 512
            || call.args.get().len() > MAX_ARGUMENT_BYTES
        {
            return Err(refused());
        }
        let held = match &self.2 {
            Some(paired_host) if PAIRED_HOST_FRONTEND_CONSENT_TOOLS.contains(&call.name) => {
                Some(HeldAction::PairedHost(paired_host.retain_action(call)?))
            }
            _ if call.name == crate::forge::FORGE_SUBMIT => Some(HeldAction::Forge(
                self.4.as_ref().ok_or_else(refused)?.retain_action(call)?,
            )),
            _ if crate::service_install::INSTALL_TOOLS.contains(&call.name) => {
                Some(HeldAction::Service(
                    self.5.as_ref().ok_or_else(refused)?.retain_action(
                        call,
                        context
                            .and_then(ToolDispatchContext::origin_session_id)
                            .ok_or_else(refused)?,
                    )?,
                ))
            }
            _ => None,
        };
        let mut reviewed = match (&self.2, &held) {
            (Some(paired_host), Some(HeldAction::PairedHost(held))) => {
                paired_host.approval_body(held)?
            }
            (_, Some(HeldAction::Forge(held))) => held.review(),
            (_, Some(HeldAction::Service(held))) => held.review(),
            _ => {
                serde_json::json!({"arguments":call.parse_args::<serde_json::Value>().map_err(|_| refused())?})
            }
        };
        reviewed["tool"] = call.name.into();
        reviewed["call_id"] = call.id.into();
        self.0
            .review_dispatch(
                call,
                context,
                held,
                reviewed,
                None,
                None,
                |held| match held {
                    HeldAction::PairedHost(held) => {
                        self.2.as_ref().ok_or_else(refused)?.arm_action(held)
                    }
                    HeldAction::Forge(held) => {
                        self.4.as_ref().ok_or_else(refused)?.arm_action(held)
                    }
                    HeldAction::Service(held) => {
                        self.5.as_ref().ok_or_else(refused)?.arm_action(held)
                    }
                },
            )
            .await
    }

    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _effect_kind: LiveBridgeEffectKind,
        outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        if self.ordinary_call(call.name) {
            return Ok(());
        }
        if crate::service_install::INSTALL_TOOLS.contains(&call.name) {
            return self.0.settle_service_dispatch(
                self.5
                    .as_ref()
                    .ok_or_else(|| ToolError::access_denied(call.name))?,
                call,
                context.ok_or_else(|| ToolError::access_denied(call.name))?,
                outcome,
                None,
            );
        }
        let refused = || ToolError::access_denied(call.name);
        let mut state = self.0.state().map_err(|_| refused())?;
        if context.and_then(ToolDispatchContext::origin_session_id) != state.session.as_ref() {
            return Err(refused());
        }
        let attempt = state
            .attempt
            .as_mut()
            .filter(|attempt| {
                attempt.custody == Custody::SpentUncertain
                    && matches!(&attempt.input, AttemptInput::Dispatch { call_id, name, arguments }
                        if call_id == call.id && name == call.name && arguments == call.args.get())
            })
            .ok_or_else(refused)?;
        if outcome != LiveBridgeEffectOutcome::Unknown {
            if call.name == crate::forge::FORGE_SUBMIT {
                self.4
                    .as_ref()
                    .ok_or_else(refused)?
                    .discard_unused_arm(call)?;
            } else if let Some(paired_host) = &self.2 {
                paired_host.discard_unused_arm(call)?;
            }
            attempt.custody = Custody::Settled(outcome);
        }
        Ok(())
    }
}

/// Use the presenter's existing stdin reader. Decisions are never conversation
/// messages. The fresh native ID prevents buffered text approving a later call.
/// Returns false for EOF, /quit or interruption; the owner must cancel and join.
pub async fn present_approval(
    approval: ApprovalPrompt,
    lines: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    match present_approval_outcome(approval, lines, output, interrupted).await? {
        ApprovalPromptOutcome::Continue => Ok(true),
        ApprovalPromptOutcome::Stopped => Ok(false),
        ApprovalPromptOutcome::Refused(error) => Err(error),
    }
}

/// Only the supplied-helper call path chooses this compact projection. The
/// native request, decision endpoint and complete technical review stay intact.
pub(crate) async fn present_supplied_setup_approval(
    mut approval: ApprovalPrompt,
    lines: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    if matches!(&approval.presentation, ApprovalPresentation::AccessReview) {
        approval.presentation = ApprovalPresentation::SuppliedSetup;
    }
    present_approval(approval, lines, output, interrupted).await
}

/// The startup caller derives this display from its typed requests. Bind it to
/// the complete retained body; a different prompt or unsupported shape stays full.
/// This changes neither the native request nor its decision endpoint.
pub(crate) async fn present_controller_setup_approval(
    mut approval: ApprovalPrompt,
    expected_body: &serde_json::Value,
    summary: Option<&str>,
    lines: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    if matches!(&approval.presentation, ApprovalPresentation::AccessReview)
        && approval.record.proposed_action.body.as_ref() == Some(expected_body)
        && let Some(summary) = summary
    {
        approval.presentation = ApprovalPresentation::ControllerSetup(summary.to_owned());
    }
    present_approval(approval, lines, output, interrupted).await
}

fn supplied_setup_summary(body: &serde_json::Value) -> Option<String> {
    let field = |name: &str| body.get(name)?.as_str().map(review_text);
    let helper = review_text(body.get("supplied_setup")?.get("name")?.as_str()?);
    let previous = match body.get("previous_setup")? {
        serde_json::Value::Null => "None".to_owned(),
        previous => review_text(previous.get("name")?.as_str()?),
    };
    let operations = body
        .get("operations")?
        .as_array()?
        .iter()
        .map(|operation| operation.as_str().map(review_text))
        .collect::<Option<Vec<_>>>()?;
    let checked = body.get("checked_installation")?;
    let checked_field = |name: &str| {
        checked
            .get(name)?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(review_text)
    };
    let checked_operations = checked
        .get("operations")?
        .as_array()?
        .iter()
        .map(|operation| {
            operation
                .as_str()
                .filter(|value| !value.is_empty())
                .map(review_text)
        })
        .collect::<Option<Vec<_>>>()?;
    if checked_operations.is_empty() {
        return None;
    }
    Some(format!(
        "Realm: {}\nAgent: {}\nSetup helper: {helper}\nPurpose: {}\nRealm change: {}\nSetup change: {}\nPrevious setup connection: {previous}\nLifetime: {}\nRaw MCP operations ({}): {}\nChecked installation route: {}\nChecked server: {}\nChecked scope: {}\nChecked operations ({}): {}",
        field("realm")?,
        field("agent")?,
        field("purpose")?,
        field("realm_change")?,
        field("setup_change")?,
        field("lifetime")?,
        operations.len(),
        operations.join(", "),
        checked_field("route")?,
        checked_field("server")?,
        checked_field("scope")?,
        checked_operations.len(),
        checked_operations.join(", "),
    ))
}

/// The account presenter must distinguish a native decision refusal from an
/// unusable terminal. This carries no approval or permission authority.
pub(crate) enum ApprovalPromptOutcome {
    Continue,
    Stopped,
    Refused(ConnectError),
}

pub(crate) async fn present_approval_outcome(
    mut approval: ApprovalPrompt,
    lines: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<ApprovalPromptOutcome, ConnectError> {
    let id = approval.record.approval_id.to_string();
    let body = approval.record.proposed_action.body.as_ref();
    let action = if matches!(
        &approval.presentation,
        ApprovalPresentation::AccessReview
            | ApprovalPresentation::SuppliedSetup
            | ApprovalPresentation::ControllerSetup(_)
    ) {
        // Access and combined reviews must display every host-held fact even
        // when their software portion also contains a frontend diff/summary.
        review_text(
            &serde_json::to_string_pretty(&body)
                .map_err(|e| ConnectError::native("present exact access review", e))?,
        )
    } else if let Some(body) = body.filter(|body| body.get("unified_summary").is_some()) {
        format!(
            "Host: {}\nKind: {}\nChange: {}\nBase: {}\nPinned package proposal:\n{}\nHost-rendered summary (controls use visible Unicode escapes):\n{}",
            review_text(body["host"].as_str().unwrap_or("")),
            review_text(body["kind"].as_str().unwrap_or("")),
            review_text(body["change_digest"].as_str().unwrap_or("")),
            review_text(
                &serde_json::to_string_pretty(&body["base"])
                    .map_err(|e| ConnectError::native("present exact approval base", e))?
            ),
            review_text(
                &serde_json::to_string_pretty(&body["proposal"])
                    .map_err(|e| ConnectError::native("present exact package proposal", e))?
            ),
            review_text(body["unified_summary"].as_str().unwrap_or(""))
        )
    } else if let Some(body) = body.filter(|body| body.get("unified_diff").is_some()) {
        format!(
            "Host: {}\nKind: {}\nChange: {}\nBase: {}\nHost-rendered diff (controls use visible Unicode escapes):\n{}",
            review_text(body["host"].as_str().unwrap_or("")),
            review_text(body["kind"].as_str().unwrap_or("")),
            review_text(body["change_digest"].as_str().unwrap_or("")),
            review_text(
                &serde_json::to_string_pretty(&body["base"])
                    .map_err(|e| ConnectError::native("present exact approval base", e))?
            ),
            review_text(body["unified_diff"].as_str().unwrap_or(""))
        )
    } else {
        review_text(
            &serde_json::to_string_pretty(&body)
                .map_err(|e| ConnectError::native("present exact approval request", e))?,
        )
    };
    // Unsupported summary shapes retain the complete original rendering.
    let compact = match &approval.presentation {
        ApprovalPresentation::SuppliedSetup => body.and_then(supplied_setup_summary),
        ApprovalPresentation::ControllerSetup(summary) => Some(summary.clone()),
        _ => None,
    };
    let shown = compact.as_deref().unwrap_or(&action);
    let details_hint = if compact.is_some() {
        format!("\nFull technical details: 'details {id}'.")
    } else {
        String::new()
    };
    writeln!(
        output,
        "{}\n{}{details_hint}\nType 'approve {id}' or 'deny {id}', or /quit.",
        review_text(&approval.record.proposed_action.summary),
        shown
    )
    .and_then(|_| output.flush())
    .map_err(|e| ConnectError::native("write approval prompt", e))?;
    tokio::pin!(interrupted);
    let outcome = async {
        loop {
            let line = tokio::select! {
                biased;
                signal = &mut interrupted => {
                    signal.map_err(|e| ConnectError::native("listen for cancellation", e))?;
                    return Ok(ApprovalPromptOutcome::Stopped);
                }
                _ = approval.closed() => {
                    writeln!(output, "This approval is no longer awaiting a decision.")
                        .map_err(|e| ConnectError::native("write approval retirement", e))?;
                    return Ok(ApprovalPromptOutcome::Continue);
                }
                line = lines.recv() => line.transpose()
                    .map_err(|e| ConnectError::native("read approval decision", e))?,
            };
            let Some(line) = line else {
                return Ok(ApprovalPromptOutcome::Stopped);
            };
            if line.trim() == "/quit" {
                return Ok(ApprovalPromptOutcome::Stopped);
            }
            if compact.is_some() && line.trim() == format!("details {id}") {
                writeln!(output,
                    "Exact technical details for {id}:\n{action}\nType 'approve {id}' or 'deny {id}', or /quit.")
                    .and_then(|_| output.flush())
                    .map_err(|e| ConnectError::native("write exact setup details", e))?;
                continue;
            }
            let decision = if line.trim() == format!("approve {id}") {
                ApprovalDecision::Approve
            } else if line.trim() == format!("deny {id}") {
                ApprovalDecision::Deny
            } else {
                writeln!(output, "Use the exact approval ID shown above, or /quit.")
                    .and_then(|_| output.flush())
                    .map_err(|e| ConnectError::native("explain approval input", e))?;
                continue;
            };
            if let Err(error) = approval.decide(decision) {
                return Ok(ApprovalPromptOutcome::Refused(error));
            }
            writeln!(output, "Decision recorded. Waiting for the native outcome.")
                .and_then(|_| output.flush())
                .map_err(|e| ConnectError::native("write approval decision", e))?;
            return Ok(ApprovalPromptOutcome::Continue);
        }
    }
    .await;
    lines
        .check_cleanup()
        .map_err(|e| ConnectError::native("restore approval input", e))?;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paired_host_tool::{PAIRED_HOST_FRONTEND_STATUS, PAIRED_HOST_FRONTEND_SUBMIT};
    use meerkat_core::{
        AgentToolDispatcher, ToolDef, ToolExecutionPolicy, ToolResult,
        ops::{ToolAccessPolicy, ToolDispatchOutcome},
        tool_execution_policy::ExecutionPolicyGatedDispatcher,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const INERT: &str = "connect_inert_approval_test";

    #[tokio::test]
    async fn frontend_reads_obey_caller_policy_session_identity_and_status_only_custody() {
        use crate::{
            paired_host::PairedHostClient,
            paired_host_tool::{
                PAIRED_HOST_FRONTEND_SOURCE_INSPECT, PAIRED_HOST_FRONTEND_SOURCE_TAGS,
                TOOLKIT_FRONTEND_CATALOG,
            },
            research::FRONTEND_RESEARCH,
        };
        for (name, request) in [
            (FRONTEND_RESEARCH, r#"{"query":"public source"}"#),
            (TOOLKIT_FRONTEND_CATALOG, "{}"),
            (
                PAIRED_HOST_FRONTEND_SOURCE_TAGS,
                r#"{"repo":"not-a-repository"}"#,
            ),
            (
                PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
                r#"{"repo":"not-a-repository","tag":"v1"}"#,
            ),
        ] {
            for deny in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let paired_host = PairedHostFrontend::with_research(
                    PairedHostClient::new("http://127.0.0.1:9").unwrap(),
                );
                let (approvals, mut presenter) =
                    LocalApprovals::create_paired_host(directory.path()).unwrap();
                let mut initial = SessionOptions::default();
                initial.build.tool_access_policy = Some(if deny {
                    ToolAccessPolicy::DenyList([name].into_iter().collect())
                } else {
                    ToolAccessPolicy::ReadOnly
                });
                let mut options =
                    crate::setup::paired_host_frontend_session(initial, &paired_host).unwrap();
                approvals
                    .attach_paired_host_direct(&mut options, &paired_host)
                    .unwrap();
                let scope = paired_host
                    .research()
                    .unwrap()
                    .bind(
                        &meerkat_core::Config::default(),
                        meerkat_core::Provider::OpenAI,
                        "unknown-local-test-model",
                        &mut options.build,
                    )
                    .unwrap();
                let session = SessionId::new();
                approvals.bind_session(&session).unwrap();
                let admission = options.build.tool_dispatch_admission.unwrap();
                let dispatcher = ExecutionPolicyGatedDispatcher::new(
                    options.build.external_tools.unwrap(),
                    ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap())
                        .unwrap(),
                )
                .with_dispatch_admission(admission.clone());
                let args = serde_json::value::RawValue::from_string(request.into()).unwrap();
                let call = ToolCallView {
                    id: "read-evidence",
                    name,
                    args: &args,
                };
                let context = ToolDispatchContext::default().with_runtime_identity(session, None);
                let outcome = dispatcher.dispatch_with_context(call, &context).await;
                let source_read = [
                    PAIRED_HOST_FRONTEND_SOURCE_TAGS,
                    PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
                ]
                .contains(&name);
                if source_read {
                    // Local input validation proves an allowed call reached
                    // dispatch without making any network request.
                    if deny {
                        assert!(matches!(&outcome, Err(ToolError::AccessDenied { .. })));
                    } else {
                        assert!(
                            matches!(&outcome, Err(ToolError::ExecutionFailed { message }) if *message == crate::paired_host::PairedHostError::InvalidSourceRepository.to_string())
                        );
                    }
                } else {
                    assert_eq!(outcome.is_err(), deny);
                }
                if !deny && !source_read {
                    let text = outcome.unwrap().result.text_content();
                    if name == FRONTEND_RESEARCH {
                        assert!(text.contains("unavailable"));
                    } else {
                        let observed: serde_json::Value = serde_json::from_str(&text).unwrap();
                        assert!(!observed["entries"].as_array().unwrap().is_empty());
                    }
                }
                assert!(
                    presenter.prompts.try_recv().is_err(),
                    "read-only evidence must not mint approval"
                );
                let wrong =
                    ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
                assert!(
                    dispatcher
                        .dispatch_with_context(call, &wrong)
                        .await
                        .is_err()
                );

                // Seed the already-qualified uncertain-submission state directly:
                // this compound tests its exclusion of the newly added read tool,
                // not another HTTP submission/recovery implementation.
                approvals.shared.state().unwrap().attempt = Some(Attempt {
                    approval_id: ApprovalId::new(),
                    input: AttemptInput::Dispatch {
                        call_id: "prior-submit".into(),
                        name: PAIRED_HOST_FRONTEND_SUBMIT.into(),
                        arguments: "{}".into(),
                    },
                    review_work: None,
                    review_scope: None,
                    dispatch_error: None,
                    deadline: Instant::now() + DECISION_LIFETIME,
                    custody: Custody::SpentUncertain,
                    held: None,
                    service: None,
                    service_entry: None,
                    policy_entry: None,
                    readback: Some(ReadbackTarget::PairedHost(PairedHostReadback::Change(
                        crate::paired_host::FrontendChangeDigest::new(&format!(
                            "hc-frontend-change-v1:sha256:{}",
                            "a".repeat(64)
                        ))
                        .unwrap(),
                    ))),
                    retired: watch::channel(false).0,
                });
                assert!(
                    dispatcher
                        .dispatch_with_context(call, &context)
                        .await
                        .is_err()
                );
                assert!(approvals.has_uncertain_dispatch());
                let status_args = serde_json::value::RawValue::from_string(serde_json::json!({"change_digest":format!("hc-frontend-change-v1:sha256:{}", "a".repeat(64))}).to_string()).unwrap();
                let status = ToolCallView {
                    id: "status",
                    name: PAIRED_HOST_FRONTEND_STATUS,
                    args: &status_args,
                };
                assert!(
                    admission
                        .await_dispatch_admission(
                            status,
                            Some(&context),
                            LiveBridgeEffectKind::ExternalIo
                        )
                        .await
                        .is_ok()
                );
                approvals.retire();
                assert!(
                    dispatcher
                        .dispatch_with_context(call, &context)
                        .await
                        .is_err()
                );
                drop(scope);
            }
        }
    }

    struct InertTool {
        names: &'static [&'static str],
        entered: AtomicUsize,
        started: tokio::sync::Notify,
        behavior: u8,
    }

    #[async_trait]
    impl AgentToolDispatcher for InertTool {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef::new(
                        *name,
                        "Inert local test tool",
                        serde_json::json!({"type":"object"}),
                    ))
                })
                .collect()
        }

        fn live_bridge_effect_kind(&self, _: &str) -> LiveBridgeEffectKind {
            LiveBridgeEffectKind::ToolDispatch
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            match self.behavior {
                1 => Err(ToolError::execution_failed("inert unknown result")),
                2 => std::future::pending().await,
                _ => Ok(ToolResult::new(call.id.into(), "inert completed".into(), false).into()),
            }
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        approvals: LocalApprovals,
        presenter: ApprovalPresenter,
        session: SessionId,
        inner: Arc<InertTool>,
        dispatcher: Arc<ExecutionPolicyGatedDispatcher<InertTool>>,
    }

    impl Fixture {
        fn new(lifetime: Duration, behavior: u8, policy: ToolAccessPolicy) -> Self {
            Self::with_actor(lifetime, behavior, policy, None)
        }

        fn with_actor(
            lifetime: Duration,
            behavior: u8,
            policy: ToolAccessPolicy,
            actor: Option<(ApprovalPrincipalId, SystemTime)>,
        ) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let (approvals, presenter) = match actor {
                Some((actor, expires)) => LocalApprovals::create_for_actor(
                    directory.path(),
                    lifetime,
                    &[INERT],
                    actor,
                    Some(expires),
                ),
                None => LocalApprovals::create_with(directory.path(), lifetime, &[INERT]),
            }
            .unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let inner = Arc::new(InertTool {
                names: &[
                    INERT,
                    "setup_prepare",
                    "setup_install",
                    "setup_inspect_package",
                ],
                entered: AtomicUsize::new(0),
                started: tokio::sync::Notify::new(),
                behavior,
            });
            let dispatcher = Arc::new(
                ExecutionPolicyGatedDispatcher::new(
                    inner.clone(),
                    ToolExecutionPolicy::resolve(policy).unwrap(),
                )
                .with_dispatch_admission(Arc::new(Admission(
                    approvals.shared.clone(),
                    false,
                    None,
                    None,
                    None,
                    None,
                ))),
            );
            Self {
                _directory: directory,
                approvals,
                presenter,
                session,
                inner,
                dispatcher,
            }
        }

        fn dispatch(
            &self,
            id: &str,
            name: &str,
            args: &str,
        ) -> tokio::task::JoinHandle<Result<ToolDispatchOutcome, ToolError>> {
            let dispatcher = self.dispatcher.clone();
            let context =
                ToolDispatchContext::default().with_runtime_identity(self.session.clone(), None);
            let (id, name, args) = (id.to_owned(), name.to_owned(), args.to_owned());
            tokio::spawn(async move {
                let args = serde_json::value::RawValue::from_string(args).unwrap();
                dispatcher
                    .dispatch_with_context(
                        ToolCallView {
                            id: &id,
                            name: &name,
                            args: &args,
                        },
                        &context,
                    )
                    .await
            })
        }

        async fn prompt(&mut self) -> ApprovalPrompt {
            tokio::time::timeout(Duration::from_secs(3), self.presenter.next())
                .await
                .unwrap()
                .unwrap()
        }

        fn entries(&self) -> Vec<ApprovalRecord> {
            self.approvals
                .shared
                .service
                .list(Default::default())
                .unwrap()
        }
    }

    fn permitted() -> ToolAccessPolicy {
        ToolAccessPolicy::AllowList(
            [
                INERT,
                "setup_prepare",
                "setup_install",
                "setup_inspect_package",
            ]
            .into_iter()
            .collect(),
        )
    }

    fn access_request(session: &SessionId) -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
            owner: ApprovalOwnerRef::Session {
                session_id: session.clone(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("fixture-selected-realm"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "Review the exact host-held access selection".into(),
                // Nonsecret presentation facts from a fixture host. They are
                // never deserialized as native principals or permission.
                body: Some(serde_json::json!({
                    "account":"fixture-account", "connection_revision":7,
                    "operation":"messages.list", "grantee":"fixture-member",
                    "requester":"fixture-requester", "lifetime":"until revoked"
                })),
            },
            risk: ApprovalRisk::Medium,
            request_body: Some(serde_json::json!({"selection":"fixture-selection-one"})),
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: Some((SystemTime::now() + Duration::from_secs(60)).into()),
            metadata: Default::default(),
            request_provenance: Some(serde_json::json!({"source":"fixture-host-owner"})),
        }
    }

    fn supplied_setup_request(session: &SessionId, previous: bool) -> ApprovalRequest {
        let mut request = access_request(session);
        request.proposed_action.body = Some(serde_json::json!({
            "realm":"fixture-realm", "agent":"fixture-agent",
            "purpose":"Prepare choices; installation and account access have separate reviews.",
            "realm_change":"Create governed operation; this persists if setup stops.",
            "setup_change":if previous {"Replace the prior setup connection."} else {"Save the supplied setup connection."},
            "previous_setup":if previous {serde_json::json!({"name":"prior-helper","command":"/prior/helper","args":["exact-prior-argument"]})} else {serde_json::Value::Null},
            "supplied_setup":{"name":"supplied-helper","command":"/exact/helper","args":["exact-long-launcher-argument"]},
            "operations":["setup_catalog","future_exact_operation"],
            "checked_installation":{
                "route":"fixture-checked-route", "server":"supplied-helper",
                "operations":["setup_install","setup_selected_install","setup_selected_check"],
                "scope":"Only the live checked session; each installation requires a separate human review. Raw MCP access is unchanged."
            },
            "lifetime":"Until revoked; cancellation does not undo an issued grant.",
            "reviewed_generation":17,
            "unified_summary":"This field must not select a frontend renderer.",
            "future_detail":"retained-exact-extra-fact"
        }));
        request
    }

    #[tokio::test]
    async fn supplied_setup_compact_review_keeps_exact_details_and_native_decision() {
        for (decision, previous, unsupported) in [
            (ApprovalDecision::Approve, false, false),
            (ApprovalDecision::Deny, true, false),
            (ApprovalDecision::Deny, true, true),
            (ApprovalDecision::Deny, false, true),
        ] {
            let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
            let mut request = supplied_setup_request(&fixture.session, previous);
            if unsupported {
                if previous {
                    request
                        .proposed_action
                        .body
                        .as_mut()
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove("purpose");
                } else {
                    request.proposed_action.body.as_mut().unwrap()["checked_installation"]["operations"] =
                        serde_json::json!([17]);
                }
            }
            let exact =
                review_text(&serde_json::to_string_pretty(&request.proposed_action.body).unwrap());
            let review = fixture.approvals.review_access(request.clone());
            let presentation = async {
                let prompt = fixture.presenter.next().await.unwrap();
                let id = prompt.record().approval_id.to_string();
                let (tx, rx) = mpsc::channel(8);
                let mut input = TerminalInput::scripted(rx);
                // Buffered generic answers and another prompt's ID never decide.
                for line in ["yes", "approve an-old-id", "details an-old-id"] {
                    tx.send(Ok(line.to_owned())).await.unwrap();
                }
                if !unsupported {
                    tx.send(Ok(format!("details {id}"))).await.unwrap();
                }
                tx.send(Ok(format!(
                    "{} {id}",
                    if decision == ApprovalDecision::Approve {
                        "approve"
                    } else {
                        "deny"
                    }
                )))
                .await
                .unwrap();
                let mut output = Vec::new();
                assert!(
                    present_supplied_setup_approval(
                        prompt,
                        &mut input,
                        &mut output,
                        std::future::pending()
                    )
                    .await
                    .unwrap()
                );
                let output = String::from_utf8(output).unwrap();
                assert_eq!(
                    output.matches(exact.as_str()).count(),
                    1,
                    "complete retained JSON is shown exactly"
                );
                assert!(output.contains("Use the exact approval ID"));
                if unsupported {
                    assert!(
                        !output.contains("Full technical details:"),
                        "unsupported shapes retain the complete view"
                    );
                } else {
                    let compact = output.split("Exact technical details for ").next().unwrap();
                    for operation in [
                        "setup_catalog",
                        "future_exact_operation",
                        "setup_install",
                        "setup_selected_install",
                        "setup_selected_check",
                    ] {
                        assert_eq!(
                            compact.matches(operation).count(),
                            1,
                            "every exact operation appears once"
                        );
                    }
                    for fact in [
                        "fixture-realm",
                        "fixture-agent",
                        "supplied-helper",
                        "governed operation",
                        "persists if setup stops",
                        "Until revoked",
                        "cancellation does not undo",
                        "fixture-checked-route",
                        "live checked session",
                        "separate human review",
                        "Raw MCP access is unchanged",
                    ] {
                        assert!(compact.contains(fact), "missing material fact: {fact}");
                    }
                    assert!(compact.contains(if previous {
                        "Previous setup connection: prior-helper"
                    } else {
                        "Previous setup connection: None"
                    }));
                    assert!(!compact.contains("exact-long-launcher-argument"));
                    assert!(!compact.contains("retained-exact-extra-fact"));
                    assert!(
                        output.contains(&format!("Exact technical details for {id}:\n{exact}"))
                    );
                    assert!(output.contains(&format!("Type 'approve {id}' or 'deny {id}'")));
                }
            };
            let (record, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(review, presentation)
            })
            .await
            .unwrap();
            let record = record.unwrap();
            assert_eq!(record.owner, request.owner);
            assert_eq!(record.requester, request.requester);
            assert_eq!(record.resource, request.resource);
            assert_eq!(record.proposed_action, request.proposed_action);
            assert_eq!(record.expires_at, request.expires_at);
            assert_eq!(record.decision.as_ref().unwrap().decision, decision);
            assert_eq!(fixture.entries(), vec![record]);
            assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
            assert!(!fixture.approvals.has_uncertain_dispatch());
        }
    }

    #[tokio::test]
    async fn controller_setup_compact_review_is_bound_to_exact_body_and_decision() {
        for (decision, same_body, supported) in [
            (ApprovalDecision::Approve, true, true),
            (ApprovalDecision::Deny, true, true),
            (ApprovalDecision::Deny, false, true),
            (ApprovalDecision::Deny, true, false),
        ] {
            let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
            let request = access_request(&fixture.session);
            let mut expected = request.proposed_action.body.clone().unwrap();
            if !same_body {
                expected["realm"] = serde_json::json!("another-realm");
            }
            let exact =
                review_text(&serde_json::to_string_pretty(&request.proposed_action.body).unwrap());
            let summary = "Model: fixture-model\nScope: fixture-realm, fixture-agent";
            let compact = same_body && supported;
            let review = fixture.approvals.review_access(request.clone());
            let presentation = async {
                let prompt = fixture.presenter.next().await.unwrap();
                let id = prompt.record().approval_id.to_string();
                let (tx, rx) = mpsc::channel(8);
                let mut input = TerminalInput::scripted(rx);
                for line in ["yes", "approve an-old-id", "details an-old-id"] {
                    tx.send(Ok(line.to_owned())).await.unwrap();
                }
                if compact {
                    tx.send(Ok(format!("details {id}"))).await.unwrap();
                }
                tx.send(Ok(format!(
                    "{} {id}",
                    if decision == ApprovalDecision::Approve {
                        "approve"
                    } else {
                        "deny"
                    }
                )))
                .await
                .unwrap();
                let mut output = Vec::new();
                assert!(
                    present_controller_setup_approval(
                        prompt,
                        &expected,
                        supported.then_some(summary),
                        &mut input,
                        &mut output,
                        std::future::pending()
                    )
                    .await
                    .unwrap()
                );
                let output = String::from_utf8(output).unwrap();
                assert_eq!(output.matches(exact.as_str()).count(), 1);
                assert!(output.contains("Use the exact approval ID"));
                assert_eq!(output.contains(summary), compact);
                assert_eq!(output.contains("Full technical details:"), compact);
                if compact {
                    assert!(
                        output.contains(&format!("Exact technical details for {id}:\n{exact}"))
                    );
                }
            };
            let (record, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(review, presentation)
            })
            .await
            .unwrap();
            let record = record.unwrap();
            assert_eq!(record.owner, request.owner);
            assert_eq!(record.requester, request.requester);
            assert_eq!(record.resource, request.resource);
            assert_eq!(record.proposed_action, request.proposed_action);
            assert_eq!(record.expires_at, request.expires_at);
            assert_eq!(record.decision.as_ref().unwrap().decision, decision);
            assert_eq!(fixture.entries(), vec![record]);
            assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
            assert!(!fixture.approvals.has_uncertain_dispatch());
        }
    }

    #[tokio::test]
    async fn supplied_setup_details_do_not_revive_expired_approval() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let request = supplied_setup_request(&fixture.session, false);
        let review = fixture.approvals.review_access(request.clone());
        let presentation = async {
            let prompt = fixture.presenter.next().await.unwrap();
            let id = prompt.record().approval_id.clone();
            fixture
                .approvals
                .fixture_expire_waiting_review(&id)
                .unwrap();
            let (tx, rx) = mpsc::channel(2);
            let mut input = TerminalInput::scripted(rx);
            tx.send(Ok(format!("details {id}"))).await.unwrap();
            tx.send(Ok(format!("approve {id}"))).await.unwrap();
            let mut output = Vec::new();
            assert!(
                present_supplied_setup_approval(
                    prompt,
                    &mut input,
                    &mut output,
                    std::future::pending()
                )
                .await
                .is_err()
            );
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains(&format!("Exact technical details for {id}:")));
            assert!(!output.contains("Decision recorded."));
        };
        let (record, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(review, presentation)
        })
        .await
        .unwrap();
        assert!(record.is_err());
        let entries = fixture.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].proposed_action, request.proposed_action);
        assert_eq!(entries[0].expires_at, request.expires_at);
        assert!(entries[0].decision.is_none());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn access_review_uses_existing_terminal_and_returns_exact_durable_decision_only() {
        for (decision, body_shape) in [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .flat_map(|decision| (0..5).map(move |body_shape| (decision, body_shape)))
        {
            let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
            let mut request = access_request(&fixture.session);
            let body = request.proposed_action.body.as_mut().unwrap();
            if body_shape == 1 || body_shape == 3 {
                body["unified_summary"] = serde_json::json!("software summary");
            }
            if body_shape == 2 || body_shape == 3 {
                body["unified_diff"] = serde_json::json!("software diff");
            }
            if body_shape == 4 {
                // Presentation/record bytes only, not a native policy approval
                // fixture. Retain and display the complete body above the old cap.
                body["member_tool_policy"] = serde_json::json!({
                    "proposal": {"candidate_canonical_json": format!("{{\"fixture\":\"{}\"}}\n", "x".repeat(100_000))}
                });
                let bytes = serde_json::to_vec(&request).unwrap();
                assert!(bytes.len() > MAX_ARGUMENT_BYTES && bytes.len() <= MAX_ACCESS_REVIEW_BYTES);
            }
            let exact_review =
                review_text(&serde_json::to_string_pretty(&request.proposed_action.body).unwrap());
            let review = fixture.approvals.review_access(request.clone());
            let presentation = async {
                let prompt = fixture.presenter.next().await.unwrap();
                let id = prompt.record().approval_id.to_string();
                let (tx, rx) = mpsc::channel(2);
                let mut input = TerminalInput::scripted(rx);
                tx.send(Ok("approve an-old-id".into())).await.unwrap();
                tx.send(Ok(format!(
                    "{} {id}",
                    match decision {
                        ApprovalDecision::Approve => "approve",
                        ApprovalDecision::Deny => "deny",
                    }
                )))
                .await
                .unwrap();
                let mut output = Vec::new();
                assert!(
                    present_approval(prompt, &mut input, &mut output, std::future::pending())
                        .await
                        .unwrap()
                );
                let output = String::from_utf8(output).unwrap();
                assert!(
                    output.contains(&exact_review),
                    "all access fields must be visible"
                );
                for fact in [
                    "fixture-account",
                    "messages.list",
                    "fixture-member",
                    "fixture-requester",
                    "until revoked",
                ] {
                    assert!(output.contains(fact));
                }
                assert!(output.contains("exact approval ID"));
            };
            let (record, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                tokio::join!(review, presentation)
            })
            .await
            .unwrap();
            let record = record.unwrap();
            assert_eq!(record.owner, request.owner);
            assert_eq!(record.requester, request.requester);
            assert_eq!(record.resource, request.resource);
            assert_eq!(record.proposed_action, request.proposed_action);
            assert_eq!(record.request_body, request.request_body);
            assert_eq!(record.request_provenance, request.request_provenance);
            assert_eq!(record.expires_at, request.expires_at);
            assert_eq!(record.decision.as_ref().unwrap().decision, decision);
            assert_eq!(
                record.decision.as_ref().unwrap().actor.as_str(),
                "connect-local-terminal"
            );
            assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
            assert!(!fixture.approvals.has_uncertain_dispatch());
            let reopened = ApprovalService::with_store(Arc::new(
                FileApprovalStore::open(fixture.approvals.store_path()).unwrap(),
            ))
            .unwrap();
            assert_eq!(reopened.get(&record.approval_id).unwrap(), record);
            assert_eq!(fixture.entries().len(), 1);
        }
    }

    #[tokio::test]
    async fn access_review_rejects_unbound_expired_incomplete_or_parallel_requests() {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, mut presenter) = LocalApprovals::create(directory.path()).unwrap();
        let approvals = Arc::new(approvals);
        let session = SessionId::new();
        let request = access_request(&session);
        assert!(approvals.review_access(request.clone()).await.is_err());
        approvals.bind_session(&session).unwrap();
        for case in 0..6 {
            let mut invalid_request = request.clone();
            match case {
                0 => {
                    invalid_request.owner = ApprovalOwnerRef::Session {
                        session_id: SessionId::new(),
                    }
                }
                1 => invalid_request.expires_at = None,
                2 => invalid_request.expires_at = Some(SystemTime::UNIX_EPOCH.into()),
                3 => invalid_request.proposed_action.body = None,
                4 => {
                    invalid_request.allowed_decisions =
                        [ApprovalDecision::Approve].into_iter().collect()
                }
                _ => {
                    invalid_request.proposed_action.body =
                        Some(serde_json::json!({"oversized":"x".repeat(MAX_ACCESS_REVIEW_BYTES)}))
                }
            }
            assert!(approvals.review_access(invalid_request).await.is_err());
        }
        assert!(
            approvals
                .shared
                .service
                .list(Default::default())
                .unwrap()
                .is_empty()
        );
        let owner = approvals.clone();
        let selected = request.clone();
        let review = tokio::spawn(async move { owner.review_access(selected).await });
        let mut prompt = tokio::time::timeout(Duration::from_secs(3), presenter.next())
            .await
            .unwrap()
            .unwrap();
        assert!(approvals.review_access(request).await.is_err());
        assert_eq!(
            approvals
                .shared
                .service
                .list(Default::default())
                .unwrap()
                .len(),
            1
        );
        prompt.decide(ApprovalDecision::Deny).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), review)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .status,
            ApprovalStatus::Denied
        );
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
    }

    #[tokio::test]
    async fn access_review_retirement_and_dropped_waiter_do_not_return_permission() {
        for case in ["pending", "recorded", "waiter-dropped"] {
            let directory = tempfile::tempdir().unwrap();
            let actor = ApprovalPrincipalId::new("actual-fixture-viewer").unwrap();
            let (approvals, mut presenter) = LocalApprovals::create_browser(
                directory.path(),
                actor.clone(),
                SystemTime::now() + Duration::from_secs(60),
            )
            .unwrap();
            let approvals = Arc::new(approvals);
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let owner = approvals.clone();
            let request = access_request(&session);
            let review = tokio::spawn(async move { owner.review_access(request).await });
            let mut prompt = tokio::time::timeout(Duration::from_secs(3), presenter.next())
                .await
                .unwrap()
                .unwrap();
            let id = prompt.record().approval_id.clone();
            if case == "recorded" {
                // On this current-thread test runtime the review cannot resume
                // between this synchronous native write and retirement.
                prompt.decide(ApprovalDecision::Approve).unwrap();
            }
            if case == "waiter-dropped" {
                review.abort();
                assert!(review.await.unwrap_err().is_cancelled());
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
            } else {
                approvals.retire();
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
                drop(prompt);
                assert!(
                    tokio::time::timeout(Duration::from_secs(3), review)
                        .await
                        .unwrap()
                        .unwrap()
                        .is_err()
                );
            }
            let actual = approvals.shared.service.get(&id).unwrap();
            if case == "recorded" {
                assert_eq!(actual.status, ApprovalStatus::Approved);
                assert_eq!(actual.decision.unwrap().actor, actor);
            } else {
                assert_eq!(actual.status, ApprovalStatus::Pending);
                assert!(actual.decision.is_none());
            }
            assert!(!approvals.has_uncertain_dispatch());
        }
    }

    #[tokio::test]
    async fn duplicate_decisions_and_identical_reentry_require_fresh_native_approval() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let first = fixture.dispatch("same-call-id", INERT, r#"{"value":"first"}"#);
        let mut original = fixture.prompt().await;
        let first_id = original.record().approval_id.clone();
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
        original.decide(ApprovalDecision::Approve).unwrap();
        first.await.unwrap().unwrap();
        assert!(original.decide(ApprovalDecision::Approve).is_err());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
        let second = fixture.dispatch("same-call-id", INERT, r#"{"value":"first"}"#);
        let mut current = fixture.prompt().await;
        assert_ne!(first_id, current.record().approval_id);
        assert!(original.decide(ApprovalDecision::Approve).is_err());
        assert!(!second.is_finished());
        current.decide(ApprovalDecision::Deny).unwrap();
        assert!(second.await.unwrap().is_err());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
        let records = fixture.entries();
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .any(|record| record.status == ApprovalStatus::Approved)
        );
        assert!(
            records
                .iter()
                .any(|record| record.status == ApprovalStatus::Denied)
        );
    }

    #[tokio::test]
    async fn changed_request_never_reuses_a_decision_and_is_shown_exactly() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let first = fixture.dispatch("same-id", INERT, r#"{"account":"one"}"#);
        let mut old = fixture.prompt().await;
        old.decide(ApprovalDecision::Deny).unwrap();
        assert!(first.await.unwrap().is_err());
        let second = fixture.dispatch("same-id", INERT, r#"{"account":"two"}"#);
        let mut new = fixture.prompt().await;
        assert_eq!(
            new.record.proposed_action.body.as_ref().unwrap()["arguments"]["account"],
            "two"
        );
        assert!(old.decide(ApprovalDecision::Approve).is_err());
        new.decide(ApprovalDecision::Approve).unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expiry_and_dropped_pending_waiter_never_dispatch() {
        let mut expired = Fixture::new(Duration::ZERO, 0, permitted());
        let dispatch = expired.dispatch("expired", INERT, "{}");
        let mut prompt = expired.prompt().await;
        assert!(dispatch.await.unwrap().is_err());
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        assert_eq!(expired.entries()[0].status, ApprovalStatus::Expired);
        assert_eq!(expired.inner.entered.load(Ordering::SeqCst), 0);

        let mut cancelled = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let dispatch = cancelled.dispatch("cancelled", INERT, "{}");
        let mut prompt = cancelled.prompt().await;
        dispatch.abort();
        assert!(dispatch.await.unwrap_err().is_cancelled());
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        assert_eq!(cancelled.inner.entered.load(Ordering::SeqCst), 0);
        // Cancellation is not a fabricated human denial in native audit.
        assert_eq!(cancelled.entries()[0].status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn post_admission_drop_and_unknown_stay_spent_without_replay() {
        for behavior in [1, 2] {
            let mut fixture = Fixture::new(DECISION_LIFETIME, behavior, permitted());
            let dispatch = fixture.dispatch("attempt", INERT, "{}");
            fixture
                .prompt()
                .await
                .decide(ApprovalDecision::Approve)
                .unwrap();
            if behavior == 2 {
                tokio::time::timeout(Duration::from_secs(3), fixture.inner.started.notified())
                    .await
                    .unwrap();
                dispatch.abort();
                assert!(dispatch.await.unwrap_err().is_cancelled());
            } else {
                assert!(dispatch.await.unwrap().is_err());
            }
            assert!(fixture.approvals.has_uncertain_dispatch());
            assert!(
                fixture
                    .dispatch("attempt", INERT, "{}")
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(
                fixture
                    .dispatch("inspect-after-uncertain", "setup_inspect_package", "{}")
                    .await
                    .unwrap()
                    .is_err()
            );
            assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.entries().len(), 1);
            assert_eq!(fixture.entries()[0].status, ApprovalStatus::Approved);
        }
    }

    #[tokio::test]
    async fn native_caller_ceiling_still_refuses_after_consent() {
        let mut fixture = Fixture::new(
            DECISION_LIFETIME,
            0,
            ToolAccessPolicy::DenyList([INERT].into_iter().collect()),
        );
        let dispatch = fixture.dispatch("denied-by-policy", INERT, "{}");
        fixture
            .prompt()
            .await
            .decide(ApprovalDecision::Approve)
            .unwrap();
        assert!(dispatch.await.unwrap().is_err());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
        assert!(!fixture.approvals.has_uncertain_dispatch());
    }

    #[tokio::test]
    async fn recorded_approval_cannot_release_a_retired_waiter_or_an_overlapping_call() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let dispatch = fixture.dispatch("held", INERT, "{}");
        let mut prompt = fixture.prompt().await;
        assert!(
            fixture
                .dispatch("overlap", INERT, "{}")
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(fixture.entries().len(), 1);
        prompt.decide(ApprovalDecision::Approve).unwrap();
        // This task has not yielded since the decision. Retire eligibility
        // before the retained native admission continuation can consume it.
        fixture.approvals.retire();
        assert!(dispatch.await.unwrap().is_err());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.entries()[0].status, ApprovalStatus::Approved);
        assert!(!fixture.approvals.has_uncertain_dispatch());
    }

    #[tokio::test]
    async fn native_decision_store_failure_never_releases_dispatch() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let dispatch = fixture.dispatch("write-failure", INERT, "{}");
        let mut prompt = fixture.prompt().await;
        let record = fixture.approvals.store_path();
        let retained = record.with_extension("retained");
        // Retain the native bytes, but remove the store path. Its read-before-
        // write now fails instead of acknowledging a durable decision.
        std::fs::rename(record, &retained).unwrap();
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        drop(prompt);
        assert!(dispatch.await.unwrap().is_err());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
        let audit =
            ApprovalService::with_store(Arc::new(FileApprovalStore::open(retained).unwrap()))
                .unwrap();
        assert_eq!(
            audit.list(Default::default()).unwrap()[0].status,
            ApprovalStatus::Pending
        );
    }

    #[tokio::test]
    async fn ordinary_preparation_passes_but_install_cannot_request_consent() {
        let fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        fixture
            .dispatch("prepare", "setup_prepare", "{}")
            .await
            .unwrap()
            .unwrap();
        assert!(
            fixture
                .dispatch("install", "setup_install", "{}")
                .await
                .unwrap()
                .is_err()
        );
        assert!(fixture.entries().is_empty());
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
        let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        assert!(
            fixture
                .dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: "other-session",
                        name: "setup_prepare",
                        args: &args,
                    },
                    &wrong
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn outside_catalog_metadata_inspection_requires_original_session_and_live_custody() {
        // This inert dispatcher checks native admission only. Actual metadata
        // parsing remains the supplied package inspector's responsibility.
        let fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        fixture
            .dispatch("inspect", "setup_inspect_package", "{}")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
        assert!(fixture.entries().is_empty());
        let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        assert!(
            fixture
                .dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: "other-session",
                        name: "setup_inspect_package",
                        args: &args,
                    },
                    &wrong,
                )
                .await
                .is_err()
        );
        fixture.approvals.retire();
        assert!(
            fixture
                .dispatch("retired", "setup_inspect_package", "{}")
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
        assert!(fixture.entries().is_empty());

        let denied = Fixture::new(
            DECISION_LIFETIME,
            0,
            ToolAccessPolicy::DenyList(["setup_inspect_package"].into_iter().collect()),
        );
        assert!(
            denied
                .dispatch("caller-denied", "setup_inspect_package", "{}")
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(denied.inner.entered.load(Ordering::SeqCst), 0);
        assert!(denied.entries().is_empty());
    }

    #[tokio::test]
    async fn restart_keeps_native_audit_but_cannot_restore_old_custody() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let dispatch = fixture.dispatch("before-restart", INERT, "{}");
        let mut old = fixture.prompt().await;
        old.decide(ApprovalDecision::Approve).unwrap();
        dispatch.await.unwrap().unwrap();
        fixture.approvals.retire();
        let reopened = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(fixture.approvals.store_path()).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            reopened.get(&old.record.approval_id).unwrap().status,
            ApprovalStatus::Approved
        );
        let (fresh, _presenter) = LocalApprovals::create(fixture._directory.path()).unwrap();
        assert_ne!(fresh.store_path(), fixture.approvals.store_path());
        fresh.bind_session(&SessionId::new()).unwrap();
        assert!(fresh.shared.service.get(&old.record.approval_id).is_err());
        assert!(old.decide(ApprovalDecision::Approve).is_err());
        assert!(
            fixture
                .dispatch("before-restart", INERT, "{}")
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn terminal_native_decision_refusal_preserves_same_presenter_for_later_review() {
        use std::{
            future::{Future, poll_fn},
            task::Poll,
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
            let review = fixture
                .approvals
                .review_access(access_request(&fixture.session));
            tokio::pin!(review);
            assert!(
                poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            let prompt = fixture.presenter.next().await.unwrap();
            let id = prompt.record().approval_id.clone();
            // Invalidate only this actual prompt's retained decision lifetime.
            // The waiting native approval and sole presenter remain owned.
            fixture
                .approvals
                .shared
                .state()
                .unwrap()
                .attempt
                .as_mut()
                .unwrap()
                .deadline = Instant::now();
            let (tx, rx) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(rx);
            tx.send(Ok(format!("approve {id}"))).await.unwrap();
            let shown = present_approval_outcome(
                prompt,
                &mut input,
                &mut Vec::new(),
                std::future::pending(),
            )
            .await;
            assert!(matches!(shown, Ok(ApprovalPromptOutcome::Refused(_))));
            assert!(review.await.is_err());
            assert_eq!(
                fixture.approvals.shared.service.get(&id).unwrap().status,
                ApprovalStatus::Pending
            );
            fixture.approvals.check_session(&fixture.session).unwrap();

            let later = fixture
                .approvals
                .review_access(access_request(&fixture.session));
            let respond = async {
                let mut next = fixture.presenter.next().await.unwrap();
                assert_ne!(next.record().approval_id, id);
                next.decide(ApprovalDecision::Approve).unwrap();
            };
            let (record, ()) = tokio::join!(later, respond);
            assert_eq!(record.unwrap().status, ApprovalStatus::Approved);
        })
        .await
        .expect("request-local terminal refusal stranded the later review");
    }

    #[tokio::test]
    async fn presenter_requires_fresh_id_and_uses_only_its_supplied_reader() {
        let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let dispatch = fixture.dispatch("present", INERT, r#"{"label":"\u001b[31mtest"}"#);
        let prompt = fixture.prompt().await;
        let id = prompt.record.approval_id.to_string();
        let (tx, rx) = mpsc::channel(2);
        let mut rx = TerminalInput::scripted(rx);
        tx.send(Ok("approve an-old-id".into())).await.unwrap();
        tx.send(Ok(format!("approve {id}"))).await.unwrap();
        let mut output = Vec::new();
        assert!(
            present_approval(prompt, &mut rx, &mut output, std::future::pending())
                .await
                .unwrap()
        );
        dispatch.await.unwrap().unwrap();
        assert!(!output.contains(&0x1b));
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("exact approval ID")
        );
        assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn presenter_eof_quit_and_interrupt_retire_the_wait_without_dispatch() {
        for exit in 0..3 {
            let mut fixture = Fixture::new(DECISION_LIFETIME, 0, permitted());
            let dispatch = fixture.dispatch("exit", INERT, "{}");
            let prompt = fixture.prompt().await;
            let (tx, rx) = mpsc::channel(1);
            let mut rx = TerminalInput::scripted(rx);
            if exit == 1 {
                tx.send(Ok("/quit".into())).await.unwrap();
            }
            drop(tx);
            let interrupted = async move {
                if exit == 2 {
                    Ok(())
                } else {
                    std::future::pending().await
                }
            };
            assert!(
                !present_approval(prompt, &mut rx, &mut Vec::new(), interrupted)
                    .await
                    .unwrap()
            );
            assert!(dispatch.await.unwrap().is_err());
            assert_eq!(fixture.inner.entered.load(Ordering::SeqCst), 0);
        }
    }

    struct PreparationModel {
        inner: meerkat_client::TestClient,
        issued: std::sync::atomic::AtomicBool,
        tool_name: &'static str,
    }

    #[async_trait]
    impl meerkat::LlmClient for PreparationModel {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat::LlmError> {
            meerkat::LlmClient::project_replay_messages(&self.inner, messages)
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures_util::Stream<Item = Result<meerkat::LlmEvent, meerkat::LlmError>>
                    + Send
                    + 'a,
            >,
        > {
            if self.issued.swap(true, Ordering::SeqCst) {
                return meerkat::LlmClient::stream(&self.inner, request);
            }
            assert!(request.tools.iter().any(|tool| tool.name == self.tool_name));
            Box::pin(futures_util::stream::iter([
                Ok(meerkat::LlmEvent::ToolCallComplete {
                    id: "native-context-check".into(),
                    name: self.tool_name.into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(meerkat::LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(meerkat::LlmEvent::Done {
                    outcome: meerkat::LlmDoneOutcome::success(meerkat::StopReason::ToolUse),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }
        async fn health_check(&self) -> Result<(), meerkat::LlmError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ordinary_preparation_uses_actual_connect_session_context_without_a_prompt() {
        ordinary_tool_uses_actual_connect_session_context("setup_prepare").await;
    }

    #[tokio::test]
    async fn outside_catalog_inspection_uses_actual_connect_session_context_without_a_prompt() {
        ordinary_tool_uses_actual_connect_session_context("setup_inspect_package").await;
    }

    async fn ordinary_tool_uses_actual_connect_session_context(tool_name: &'static str) {
        // The tool is inert. This proves native context propagation and the
        // ordinary-work bypass, not an MCP worker or descendant lifecycle.
        let directory = tempfile::tempdir().unwrap();
        let (approvals, mut presenter) = LocalApprovals::create(directory.path()).unwrap();
        let inner = Arc::new(InertTool {
            names: match tool_name {
                "setup_prepare" => &["setup_prepare"],
                "setup_inspect_package" => &["setup_inspect_package"],
                _ => unreachable!("explicit test tool"),
            },
            entered: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            behavior: 0,
        });
        let client = Arc::new(PreparationModel {
            inner: meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            issued: std::sync::atomic::AtomicBool::new(false),
            tool_name,
        });
        let storage = crate::RealmOptions {
            context_root: directory.path().to_owned(),
            state_root: Some(directory.path().join("state")),
            user_config_root: Some(directory.path().join("home")),
        }
        .resolve("approval-context")
        .unwrap();
        let mut options = SessionOptions::default();
        options.model.model = Some("gpt-5.4".into());
        options.build.llm_client_override =
            Some(meerkat::encode_llm_client_override_for_service(client));
        options.build.external_tools = Some(inner.clone());
        options.build.tool_access_policy = Some(ToolAccessPolicy::AllowList(
            [tool_name].into_iter().collect(),
        ));
        options.build.tool_dispatch_admission = Some(Arc::new(Admission(
            approvals.shared.clone(),
            false,
            None,
            None,
            None,
            None,
        )));
        let session = Box::pin(crate::ConnectSession::open_with_config(
            storage,
            meerkat_core::Config::default(),
            options,
        ))
        .await
        .unwrap();
        approvals.bind_session(session.id()).unwrap();
        let (admission, completion) = session
            .submit("Run the inert ordinary preparation fixture.".into())
            .await
            .unwrap();
        let wait = completion.wait();
        tokio::pin!(wait);
        let result = tokio::select! {
            result = &mut wait => result,
            _ = tokio::time::sleep(Duration::from_secs(20)) => {
                approvals.retire();
                session.cancel(&admission).await.unwrap();
                wait.await
            }
        };
        approvals.retire();
        session.close().await.unwrap();
        assert!(matches!(
            result.unwrap(),
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
        ));
        assert_eq!(inner.entered.load(Ordering::SeqCst), 1);
        assert!(
            approvals
                .shared
                .service
                .list(Default::default())
                .unwrap()
                .is_empty()
        );
        assert!(presenter.prompts.try_recv().is_err());
    }
    #[tokio::test]
    async fn browser_actor_is_retained_and_expired_prompt_is_local_refusal() {
        let actor = ApprovalPrincipalId::new("fixture-host-viewer").unwrap();
        let mut f = Fixture::with_actor(
            DECISION_LIFETIME,
            0,
            permitted(),
            Some((actor.clone(), SystemTime::now() + Duration::from_secs(30))),
        );
        let first = f.dispatch("expired", INERT, "{}");
        let mut prompt = f.prompt().await;
        f.approvals
            .shared
            .state()
            .unwrap()
            .attempt
            .as_mut()
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        assert!(matches!(
            prompt.decide(ApprovalDecision::Approve),
            Err(ConnectError::InvalidInput(_))
        ));
        drop(prompt);
        assert!(first.await.unwrap().is_err());
        assert!(!f.approvals.shared.state().unwrap().closed);
        let second = f.dispatch("valid", INERT, "{}");
        let mut prompt = f.prompt().await;
        prompt.decide(ApprovalDecision::Approve).unwrap();
        second.await.unwrap().unwrap();
        let records = f.entries();
        let recorded = records
            .iter()
            .filter_map(|r| r.decision.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].actor, actor);
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        assert_eq!(f.inner.entered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn poisoned_approval_custody_cannot_acknowledge_retirement() {
        let f = Fixture::new(DECISION_LIFETIME, 0, permitted());
        let shared = f.approvals.shared.clone();
        let _ = std::thread::spawn(move || {
            let _guard = shared.state.lock().unwrap();
            panic!("fixture poison");
        })
        .join();
        assert!(matches!(
            f.approvals.shared.state(),
            Err(ConnectError::Native { .. })
        ));
        assert!(f.approvals.retire_checked().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_retirement_waits_for_native_consequence_barrier_and_records_outcome_after_expiry()
     {
        use meerkat_core::{
            MobMemberBinding, PolicyDigest, PolicyEvaluationProvenance,
            PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
            PolicyRevision, ToolConsequenceFailure, ToolConsequenceNarrowingPolicy,
            ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot, ToolConsequenceRequest,
            ToolConsequenceVerdict,
        };
        struct Barrier {
            release: std::sync::Mutex<bool>,
            cv: std::sync::Condvar,
            entered: tokio::sync::Notify,
        }
        impl ToolConsequencePolicySnapshot for Barrier {
            fn provenance(&self) -> PolicyEvaluationProvenance {
                PolicyEvaluationProvenance {
                    revision: PolicyRevision(1),
                    digest: PolicyDigest::from_canonical_bytes(b"retirement-fixture"),
                }
            }
            fn evaluate(&self, _: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
                self.entered.notify_one();
                let guard = self.release.lock().unwrap();
                let (guard, _) = self
                    .cv
                    .wait_timeout_while(guard, Duration::from_secs(10), |ready| !*ready)
                    .unwrap();
                assert!(*guard, "bounded test barrier release");
                ToolConsequenceVerdict::Allow
            }
        }
        struct Provider {
            id: PolicyProviderId,
            barrier: Arc<Barrier>,
        }
        impl ToolConsequenceNarrowingPolicy for Provider {
            fn provider_id(&self) -> &PolicyProviderId {
                &self.id
            }
            fn generation(&self) -> PolicyProviderGeneration {
                PolicyProviderGeneration(1)
            }
            fn snapshot(
                &self,
                _: &PolicyId,
            ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure>
            {
                Ok(self.barrier.clone())
            }
        }
        let barrier = Arc::new(Barrier {
            release: std::sync::Mutex::new(false),
            cv: std::sync::Condvar::new(),
            entered: tokio::sync::Notify::new(),
        });
        let provider_id = PolicyProviderId::new("retirement-fixture").unwrap();
        let registry = Arc::new(
            ToolConsequencePolicyRegistry::new(
                vec![Arc::new(Provider {
                    id: provider_id.clone(),
                    barrier: barrier.clone(),
                })],
                PolicyEvaluationSupervisorConfig {
                    workers_per_provider: 1,
                    queue_capacity_per_provider: 1,
                    evaluation_deadline: Duration::from_secs(15),
                },
                None,
            )
            .unwrap(),
        );
        let bound = registry
            .bind(
                MobMemberBinding {
                    mob_id: "fixture".into(),
                    role: "fixture".into(),
                    member: "fixture".into(),
                },
                provider_id,
                PolicyId::new("retirement").unwrap(),
            )
            .unwrap();
        let expires = SystemTime::now() + Duration::from_secs(1);
        let mut f = Fixture::with_actor(
            DECISION_LIFETIME,
            0,
            permitted(),
            Some((
                ApprovalPrincipalId::new("retained-viewer").unwrap(),
                expires,
            )),
        );
        f.dispatcher = Arc::new(
            ExecutionPolicyGatedDispatcher::new(
                f.inner.clone(),
                ToolExecutionPolicy::resolve(permitted()).unwrap(),
            )
            .with_dispatch_admission(Arc::new(Admission(
                f.approvals.shared.clone(),
                false,
                None,
                None,
                None,
                None,
            )))
            .with_consequence_policy(bound),
        );
        let task = f.dispatch("held", INERT, "{}");
        let mut prompt = f.prompt().await;
        prompt.decide(ApprovalDecision::Approve).unwrap();
        barrier.entered.notified().await;
        assert!(f.approvals.has_uncertain_dispatch());
        assert_eq!(f.inner.entered.load(Ordering::SeqCst), 0);
        let approvals = Arc::new(f.approvals);
        let (retired, observed) = oneshot::channel();
        let check = tokio::spawn(crate::stdio::tests::retire_joined_dispatch(
            approvals.clone(),
            async move {
                task.await.unwrap().unwrap();
            },
            retired,
        ));
        observed.await.unwrap();
        assert!(!check.is_finished());
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(approvals.shared.live_state().is_err());
        *barrier.release.lock().unwrap() = true;
        barrier.cv.notify_all();
        check.await.unwrap();
        assert_eq!(f.inner.entered.load(Ordering::SeqCst), 1);
        assert!(!approvals.has_uncertain_dispatch());
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        assert!(
            f.dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: "after",
                        name: INERT,
                        args: &args
                    },
                    &ToolDispatchContext::default().with_runtime_identity(f.session, None)
                )
                .await
                .is_err()
        );
        assert_eq!(f.inner.entered.load(Ordering::SeqCst), 1);
    }
}
