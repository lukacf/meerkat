//! Console schedules: deferred console sends that a Console caller sets up
//! for an agent in the Console's schedule editor (`meta/schedule/*` on
//! `POST /console/rpc`, served by `crate::http_console::schedules`).
//!
//! A console schedule is an ordinary schedule in the gateway's one durable
//! `ScheduleService`, the service its schedule host drains. No other store,
//! row type or Core concept is involved. Its target is the host runnable
//! [`CONSOLE_SCHEDULE_RUNNABLE`], which Meta registers with the schedule host
//! ([`with_console_schedule_runnable`]), and the target's params are the
//! deferred send ([`ConsoleScheduledSend`]): the agent, the message and the
//! Console caller it is sent as, as `/console/send` resolves that caller.
//!
//! A fire is a console send by that caller through the Console's own send
//! path ([`ConsoleScheduleDoor`]), judged at the fire: the Console's admission
//! of the caller ([`ConsoleScheduleSender::admitted_now`]), then the send a
//! live `/console/send` by it takes after its admission (the Console's send
//! gate with its access decision for the caller, governance, and Core's
//! host-human admission). A fire never takes another way to the agent.
//!
//! Only the Console writes console schedules. Agents' schedule tools refuse
//! to create, change or read one ([`ConsoleScheduleToolGate`]), and SDK and
//! host runnables may not take the reserved runnable name
//! ([`refuse_reserved_runnable_name`]).

use std::sync::{Arc, PoisonError, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{
    HostRunnable, HostRunnableError, HostRunnableInvocation, HostRunnableName, HostRunnableOutcome,
    HostRunnableParams, HostRunnableTargetBinding, RunnableProbe, Schedule, ScheduleDomainError,
    ScheduleId, ScheduleRunnableHost, ScheduleService, ScheduleStoreError, TargetBinding,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::auth::session_fence::ConsoleSessionFences;
use crate::console_aggregator::ConsoleSendScope;
use crate::runtime::RuntimeDecisionState;
use crate::schedule_wiring::ScheduleFiringHostBinding;

/// The host runnable every console schedule targets. Reserved: Meta
/// registers it with the schedule host, and no SDK or host runnable may
/// take the name.
pub const CONSOLE_SCHEDULE_RUNNABLE: &str = "meta.console.scheduled_send";

/// The schema of a console schedule's send ([`ConsoleScheduledSend`]).
pub(crate) const CONSOLE_SCHEDULED_SEND_SCHEMA: &str = "meta.console.scheduled_send.v1";

/// How long after the gateway's schedule host starts (when the console
/// schedules' runnable is composed) a fire waits for the Console's send path
/// when no console router was built yet: a gateway builds its router after
/// it starts its schedule host, so a fire due at boot waits for it. Past
/// this window a fire with no send path fails at once.
const CONSOLE_DOOR_WAIT: Duration = Duration::from_secs(30);

/// The Console caller a console schedule fires as: the caller who last
/// defined it (created, changed or resumed it), as `/console/send` resolves
/// that caller. A fire is admitted as this caller
/// ([`Self::admitted_now`]), judges this caller's access and is deduplicated
/// in this caller's send scope, as a live send by it would be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ConsoleScheduleSender {
    /// An anonymous caller of an open Console: its shared local account.
    Anonymous,
    /// A verified Console human: the principal the access policy keys (the
    /// token's email), the provider its token was classified as, and the
    /// verified OIDC issuer and `sub`.
    Human {
        principal: String,
        provider: String,
        issuer: String,
        subject: String,
    },
    /// An authenticated principal with no verified human behind it (a
    /// service identity, or a token without a `sub`), with the provider its
    /// token was classified as and the issuer it verified against.
    Principal {
        principal: String,
        provider: String,
        issuer: String,
    },
}

impl ConsoleScheduleSender {
    /// The sender a Console request's caller is: its send scope, with the
    /// principal the access policy keys for it and, for a verified human, the
    /// provider its token was classified as. `None` for a scope no Console
    /// route resolves (the public Rust send API's).
    pub(crate) fn of_caller(
        scope: &ConsoleSendScope,
        principal: Option<&str>,
        human_provider: Option<&crate::AuthProvider>,
    ) -> Option<Self> {
        match scope {
            ConsoleSendScope::Anonymous => Some(Self::Anonymous),
            ConsoleSendScope::Human { issuer, subject } => Some(Self::Human {
                principal: principal?.to_string(),
                provider: crate::http_console::console_provider_name(human_provider?).to_string(),
                issuer: issuer.clone(),
                subject: subject.clone(),
            }),
            ConsoleSendScope::Principal {
                principal,
                provider,
                issuer,
            } => Some(Self::Principal {
                principal: principal.clone(),
                provider: provider.clone(),
                issuer: issuer.clone(),
            }),
            ConsoleSendScope::Unscoped => None,
        }
    }

    /// The principal the access policy keys for this sender: `None` for the
    /// open Console's shared account.
    pub(crate) fn principal(&self) -> Option<&str> {
        match self {
            Self::Anonymous => None,
            Self::Human { principal, .. } | Self::Principal { principal, .. } => {
                Some(principal.as_str())
            }
        }
    }

    /// The scope this sender's console sends are deduplicated in.
    pub(crate) fn scope(&self) -> ConsoleSendScope {
        match self {
            Self::Anonymous => ConsoleSendScope::Anonymous,
            Self::Human {
                issuer, subject, ..
            } => ConsoleSendScope::Human {
                issuer: issuer.clone(),
                subject: subject.clone(),
            },
            Self::Principal {
                principal,
                provider,
                issuer,
            } => ConsoleSendScope::Principal {
                principal: principal.clone(),
                provider: provider.clone(),
                issuer: issuer.clone(),
            },
        }
    }

    /// Whether the Console admits this sender now, as it admits a live
    /// request by it: an anonymous sender only while the Console requires no
    /// sign-in; a principal only under the issuer the Console trusts now,
    /// past the Console's route access (its provider policy and allowlist,
    /// where sign-in is required), and past its session fence, the schedule's
    /// definition (`defined_at`, in whole seconds since the epoch) standing
    /// in for the issue time of the token it was defined with, as a live
    /// token's `iat` does. `Err` says why the sender is refused.
    pub(crate) fn admitted_now(
        &self,
        decisions: &RuntimeDecisionState,
        fences: &ConsoleSessionFences,
        defined_at: u64,
    ) -> Result<(), String> {
        let (principal, provider, issuer) = match self {
            Self::Anonymous if decisions.console.require_app_auth => {
                return Err(
                    "the Console requires sign-in now, and an anonymous caller defined this \
                     schedule"
                        .to_string(),
                );
            }
            Self::Anonymous => return Ok(()),
            Self::Human {
                principal,
                provider,
                issuer,
                ..
            }
            | Self::Principal {
                principal,
                provider,
                issuer,
            } => (principal, provider, issuer),
        };
        if fences.not_before(principal).is_some_and(|not_before| {
            defined_at < crate::runtime::session_fence_first_accepted_iat(not_before)
        }) {
            return Err(format!(
                "the Console sessions of {principal} were revoked after the schedule was defined"
            ));
        }
        let trusted =
            crate::auth::parse_oidc_discovery_json(&decisions.trusted_oidc.discovery_json)
                .is_ok_and(|discovery| discovery.issuer == *issuer);
        if !trusted {
            return Err(format!(
                "the Console no longer trusts the issuer {principal} signed in with"
            ));
        }
        let provider =
            crate::http_console::console_provider_of_name(provider).ok_or_else(|| {
                format!("the Console does not know the provider {principal} signed in with")
            })?;
        crate::decisions::enforce_console_route_access(
            &decisions.auth,
            &decisions.console,
            &crate::ConsoleAccessRequest {
                provider,
                email: principal.clone(),
            },
        )
        .map_err(|error| format!("the Console no longer admits {principal}: {error}"))
    }
}

/// A console schedule's deferred send, held as its host-runnable params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsoleScheduledSend {
    /// [`CONSOLE_SCHEDULED_SEND_SCHEMA`].
    pub(crate) schema: String,
    /// The agent the message goes to, as the console names it.
    pub(crate) identity: String,
    /// The message text.
    pub(crate) content: String,
    /// The Console caller the message is sent as.
    pub(crate) sender: ConsoleScheduleSender,
    /// When the sender last defined the send, in whole seconds since the
    /// epoch. A fire refuses a verified human's send whose Console sessions
    /// were revoked after it.
    pub(crate) defined_at: u64,
}

impl ConsoleScheduledSend {
    pub(crate) fn new(
        identity: String,
        content: String,
        sender: ConsoleScheduleSender,
        defined_at: u64,
    ) -> Self {
        Self {
            schema: CONSOLE_SCHEDULED_SEND_SCHEMA.to_string(),
            identity,
            content,
            sender,
            defined_at,
        }
    }

    /// The schedule target that carries this send.
    pub(crate) fn target(&self) -> Result<TargetBinding, String> {
        let params = serde_json::to_value(self).map_err(|error| error.to_string())?;
        let params = HostRunnableParams::try_from(params).map_err(|error| error.to_string())?;
        let runnable = HostRunnableName::parse(CONSOLE_SCHEDULE_RUNNABLE)
            .map_err(|error| error.to_string())?;
        Ok(TargetBinding::host_runnable(HostRunnableTargetBinding {
            runnable,
            params: Some(params),
        }))
    }

    /// The send `target` carries, when it is a console schedule's.
    pub(crate) fn of_target(target: &TargetBinding) -> Option<Self> {
        let TargetBinding::HostRunnable(binding) = target else {
            return None;
        };
        if binding.runnable.as_str() != CONSOLE_SCHEDULE_RUNNABLE {
            return None;
        }
        Self::from_params(binding.params.as_ref()?.get())
    }

    /// The send a console schedule's params text holds, when it is one of
    /// this schema.
    pub(crate) fn from_params(text: &str) -> Option<Self> {
        let send: Self = serde_json::from_str(text).ok()?;
        (send.schema == CONSOLE_SCHEDULED_SEND_SCHEMA).then_some(send)
    }
}

/// Whether `schedule` is a console schedule: its target names the console's
/// runnable, whatever its params hold.
pub(crate) fn is_console_schedule(schedule: &Schedule) -> bool {
    targets_console_runnable(&schedule.target)
}

/// Whether `target` names the console's runnable.
pub(crate) fn targets_console_runnable(target: &TargetBinding) -> bool {
    matches!(
        target,
        TargetBinding::HostRunnable(binding) if binding.runnable.as_str() == CONSOLE_SCHEDULE_RUNNABLE
    )
}

/// Refuse `name` for an SDK or host runnable: the console's runnable name is
/// reserved for console schedules.
pub(crate) fn refuse_reserved_runnable_name(name: &HostRunnableName) -> Result<(), String> {
    if name.as_str() == CONSOLE_SCHEDULE_RUNNABLE {
        return Err(format!(
            "host runnable name '{name}' is reserved for console schedules"
        ));
    }
    Ok(())
}

/// The schedule service a console schedule lives in, as the gateway's
/// schedule host attached it to the runtime, with the binding that says
/// whether a firing host drains it.
#[derive(Clone)]
pub(crate) struct AttachedConsoleSchedules {
    pub(crate) service: ScheduleService,
    pub(crate) firing: ScheduleFiringHostBinding,
}

/// Where a runtime's console schedules live: empty until the gateway's
/// schedule host attaches its schedule service
/// ([`Self::attach`]), and empty for good on a runtime that runs none.
/// Shared by every clone of the runtime that holds it.
#[derive(Clone, Default)]
pub struct ConsoleSchedules(Arc<RwLock<Option<AttachedConsoleSchedules>>>);

impl ConsoleSchedules {
    /// Attach the schedule service the gateway's schedule host drains, and
    /// the binding that says whether a firing host does. The Console lists
    /// console schedules from then on, and writes them while the firing host
    /// is bound.
    pub fn attach(&self, service: ScheduleService, firing: ScheduleFiringHostBinding) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) =
            Some(AttachedConsoleSchedules { service, firing });
    }

    /// The attached schedule service, if any.
    pub(crate) fn attached(&self) -> Option<AttachedConsoleSchedules> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The Console's own send path, as a console schedule's fire takes it: what
/// `/console/send` sends through, with its gate, governance and admission.
#[async_trait]
pub(crate) trait ConsoleScheduleDoor: Send + Sync {
    /// Send `send` as its sender, deduplicated under `delivery_key` (the
    /// occurrence's delivery idempotency key) in the sender's scope. `Err`
    /// carries why the Console refused it; nothing was sent then.
    async fn send(&self, send: &ConsoleScheduledSend, delivery_key: &str) -> Result<(), String>;
}

type DoorSender = watch::Sender<Option<Arc<dyn ConsoleScheduleDoor>>>;

/// The Console's send path for console schedule fires: installed by each
/// console router build (the router built last is the one fires take),
/// cleared when the console projection it sends through is replaced and at
/// shutdown, and held weakly by the console schedules' runnable, so a fire
/// never keeps a stopped runtime's Console alive.
#[derive(Clone)]
pub struct ConsoleScheduleDoorSlot(Arc<DoorSender>);

impl Default for ConsoleScheduleDoorSlot {
    fn default() -> Self {
        Self(Arc::new(watch::channel(None).0))
    }
}

impl ConsoleScheduleDoorSlot {
    /// Install the Console's send path; it replaces the one before.
    pub(crate) fn install(&self, door: Arc<dyn ConsoleScheduleDoor>) {
        self.0.send_replace(Some(door));
    }

    /// Remove the Console's send path, and what it holds: the console
    /// projection it sends through was replaced, or the runtime shuts down.
    /// Fires fail until a console router build installs a send path again.
    pub(crate) fn clear(&self) {
        self.0.send_replace(None);
    }

    fn downgrade(&self) -> Weak<DoorSender> {
        Arc::downgrade(&self.0)
    }
}

/// The host runnable of every console schedule: a fire sends the
/// schedule's message as its sender through the Console's send path.
struct ConsoleScheduleRunnable {
    doors: Weak<DoorSender>,
    /// The end of the window in which a fire waits for a console router
    /// build: [`CONSOLE_DOOR_WAIT`] after the runnable was composed, as the
    /// gateway's schedule host started.
    wait_until: std::time::Instant,
}

impl ConsoleScheduleRunnable {
    /// The Console's send path, waiting for one within the window that
    /// opened when the schedule host started, when no console router was
    /// built yet.
    async fn door(&self) -> Result<Arc<dyn ConsoleScheduleDoor>, String> {
        const STOPPED: &str = "the console runtime has stopped";
        const NO_DOOR: &str =
            "this gateway has no Console send path now, so a console schedule cannot be sent";
        let Some(doors) = self.doors.upgrade() else {
            return Err(STOPPED.to_string());
        };
        let deadline = tokio::time::Instant::from_std(self.wait_until);
        let mut installed = doors.subscribe();
        drop(doors);
        let waited = tokio::time::timeout_at(deadline, installed.wait_for(Option::is_some)).await;
        match waited {
            Ok(Ok(door)) => door.clone().ok_or_else(|| NO_DOOR.to_string()),
            Ok(Err(_)) => Err(STOPPED.to_string()),
            Err(_) => Err(NO_DOOR.to_string()),
        }
    }
}

#[async_trait]
impl HostRunnable for ConsoleScheduleRunnable {
    async fn run(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        let failed = |detail: String| HostRunnableError::Failed { detail };
        let send = invocation
            .params
            .as_deref()
            .and_then(|params| ConsoleScheduledSend::from_params(params.get()))
            .ok_or_else(|| failed("the console schedule's send cannot be read".to_string()))?;
        let door = self.door().await.map_err(failed)?;
        door.send(&send, &invocation.delivery_idempotency_key)
            .await
            .map_err(failed)?;
        Ok(HostRunnableOutcome::completed())
    }
}

/// A schedule host's runnables with the console schedules' runnable in
/// front of them.
struct WithConsoleScheduleRunnable {
    console: ConsoleScheduleRunnable,
    inner: Option<Arc<dyn ScheduleRunnableHost>>,
}

#[async_trait]
impl ScheduleRunnableHost for WithConsoleScheduleRunnable {
    fn probe_runnable(&self, runnable: &HostRunnableName) -> RunnableProbe {
        if runnable.as_str() == CONSOLE_SCHEDULE_RUNNABLE {
            return RunnableProbe::Registered;
        }
        self.inner.as_ref().map_or(RunnableProbe::Unknown, |inner| {
            inner.probe_runnable(runnable)
        })
    }

    async fn run_occurrence(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        if invocation.runnable.as_str() == CONSOLE_SCHEDULE_RUNNABLE {
            return self.console.run(invocation).await;
        }
        match &self.inner {
            Some(inner) => inner.run_occurrence(invocation).await,
            None => Err(HostRunnableError::Unregistered {
                name: invocation.runnable,
            }),
        }
    }
}

/// `runnable_host` with the console schedules' runnable composed in front,
/// sending through the Console send path `doors` holds. Composed as the
/// gateway's schedule host starts: a fire claimed before the console router
/// is built waits for it until [`CONSOLE_DOOR_WAIT`] after now.
pub(crate) fn with_console_schedule_runnable(
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    doors: &ConsoleScheduleDoorSlot,
) -> Arc<dyn ScheduleRunnableHost> {
    console_schedule_runnable_waiting(runnable_host, doors, CONSOLE_DOOR_WAIT)
}

/// [`with_console_schedule_runnable`] whose fires wait for a console router
/// build until `wait` after now.
fn console_schedule_runnable_waiting(
    runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    doors: &ConsoleScheduleDoorSlot,
    wait: Duration,
) -> Arc<dyn ScheduleRunnableHost> {
    Arc::new(WithConsoleScheduleRunnable {
        console: ConsoleScheduleRunnable {
            doors: doors.downgrade(),
            wait_until: std::time::Instant::now() + wait,
        },
        inner: runnable_host,
    })
}

/// The agent schedule tools that name a schedule by its id.
const SCHEDULE_TOOLS_BY_ID: &[&str] = &[
    "meerkat_schedule_get",
    "meerkat_schedule_update",
    "meerkat_schedule_pause",
    "meerkat_schedule_resume",
    "meerkat_schedule_delete",
    "meerkat_schedule_occurrences",
];

/// The agent schedule tools that write a target.
const SCHEDULE_TOOLS_WITH_TARGET: &[&str] = &["meerkat_schedule_create", "meerkat_schedule_update"];

/// Agents' schedule tools leave console schedules alone: a console schedule
/// is a Console caller's deferred send, and only the Console, which judges
/// that caller's access, writes it.
///
/// A create or update whose target names the console's runnable is refused,
/// so no agent sends as a Console caller; every verb that names a console
/// schedule by its id (get, update, pause, resume, delete, occurrences) is
/// refused, and so is one whose schedule cannot be read (it may be a console
/// schedule); and `meerkat_schedule_list` leaves console schedules out. Every
/// other call reaches the tools unchanged.
pub(crate) struct ConsoleScheduleToolGate {
    inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
    service: ScheduleService,
}

impl ConsoleScheduleToolGate {
    pub(crate) fn new(
        inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
        service: ScheduleService,
    ) -> Self {
        Self { inner, service }
    }

    /// Why `name` with `args` is refused, if it is.
    async fn refusal(&self, name: &str, args: &Value) -> Option<meerkat_core::ToolError> {
        if SCHEDULE_TOOLS_WITH_TARGET.contains(&name)
            && args.get("target").is_some_and(target_value_names_console)
        {
            return Some(meerkat_core::ToolError::execution_failed(format!(
                "{name} refused: the host runnable {CONSOLE_SCHEDULE_RUNNABLE} sends a \
                 console schedule as its Console caller, and only the Console schedules it"
            )));
        }
        if !SCHEDULE_TOOLS_BY_ID.contains(&name) {
            return None;
        }
        // An id that does not parse is the tools' own refusal.
        let schedule_id = ScheduleId::parse(args.get("schedule_id")?.as_str()?).ok()?;
        match self.service.get(&schedule_id).await {
            Ok(schedule) => is_console_schedule(&schedule).then(|| {
                meerkat_core::ToolError::execution_failed(format!(
                    "{name} refused: schedule {schedule_id} is a console schedule, a Console \
                     caller's deferred message to an agent; only the Console reads or changes it"
                ))
            }),
            // No schedule has the id: the tools answer it as not found.
            Err(ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. })) => None,
            // Whether it is a console schedule cannot be read now: refused,
            // since letting it through could change one.
            Err(error) => Some(meerkat_core::ToolError::execution_failed(format!(
                "{name} refused: schedule {schedule_id} cannot be read now ({error}), so it may \
                 be a console schedule; try again"
            ))),
        }
    }

    /// `meerkat_schedule_list` without the console schedules, in the
    /// listing's own shape.
    async fn list_without_console_schedules(
        &self,
        call_id: &str,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let schedules = self.service.list().await.map_err(|error| {
            meerkat_core::ToolError::execution_failed(format!(
                "meerkat_schedule_list failed: {error}"
            ))
        })?;
        let schedules = schedules
            .into_iter()
            .filter(|schedule| !is_console_schedule(schedule))
            .collect::<Vec<_>>();
        let schedules = serde_json::to_value(&schedules).map_err(|error| {
            meerkat_core::ToolError::execution_failed(format!(
                "failed to encode meerkat_schedule_list result: {error}"
            ))
        })?;
        let listed = serde_json::json!({ "schedules": schedules });
        Ok(
            meerkat_core::types::ToolResult::new(call_id.to_string(), listed.to_string(), false)
                .into(),
        )
    }
}

/// Whether a tool call's `target` argument names the console's runnable.
fn target_value_names_console(target: &Value) -> bool {
    target.get("target_kind").and_then(Value::as_str) == Some("host_runnable")
        && target.get("runnable").and_then(Value::as_str) == Some(CONSOLE_SCHEDULE_RUNNABLE)
}

#[async_trait]
impl meerkat_core::AgentToolDispatcher for ConsoleScheduleToolGate {
    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner.tools()
    }

    /// The gate refuses calls at dispatch time and advertises the inner
    /// surface unchanged.
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    /// Forwards AS the context entry point: the trait default would drop the
    /// turn's dispatch context.
    async fn dispatch_with_context(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        // Arguments that do not parse reach the tools, which refuse them.
        if let Ok(args) = serde_json::from_str::<Value>(call.args.get()) {
            if let Some(refusal) = self.refusal(call.name, &args).await {
                return Err(refusal);
            }
            if call.name == "meerkat_schedule_list" {
                return self.list_without_console_schedules(call.id).await;
            }
        }
        self.inner.dispatch_with_context(call, context).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat::{
        CreateScheduleRequest, DisabledScheduleStore, IntervalTriggerSpec, MemoryScheduleStore,
        ScheduleToolDispatcher, TriggerSpec,
    };
    use meerkat_core::AgentToolDispatcher;
    use meerkat_core::types::ToolCallView;
    use serde_json::json;
    use serde_json::value::RawValue;

    const ISSUER: &str = "https://issuer.example.test";

    fn sender() -> ConsoleScheduleSender {
        ConsoleScheduleSender::Human {
            principal: "erin@example.test".to_string(),
            provider: "generic_oidc".to_string(),
            issuer: ISSUER.to_string(),
            subject: "erin-sub".to_string(),
        }
    }

    fn robot() -> ConsoleScheduleSender {
        ConsoleScheduleSender::Principal {
            principal: "svc:robot".to_string(),
            provider: "service_identity".to_string(),
            issuer: ISSUER.to_string(),
        }
    }

    /// A sender is the Console caller as `/console/send` resolves it: its
    /// send scope, the principal the access policy keys and, for a verified
    /// human, its token's provider, round-tripped.
    #[test]
    fn a_sender_keeps_its_callers_send_scope_and_principal() {
        let human = ConsoleSendScope::Human {
            issuer: ISSUER.to_string(),
            subject: "erin-sub".to_string(),
        };
        let erin = ConsoleScheduleSender::of_caller(
            &human,
            Some("erin@example.test"),
            Some(&crate::AuthProvider::GenericOidc),
        )
        .unwrap();
        assert_eq!(erin, sender());
        assert_eq!(erin.scope(), human);
        assert_eq!(erin.principal(), Some("erin@example.test"));
        assert_eq!(
            ConsoleScheduleSender::of_caller(&human, Some("erin@example.test"), None),
            None,
            "a human is recorded with its token's provider"
        );

        let service = ConsoleSendScope::Principal {
            principal: "svc:robot".to_string(),
            provider: "service_identity".to_string(),
            issuer: ISSUER.to_string(),
        };
        let recorded = ConsoleScheduleSender::of_caller(&service, Some("svc:robot"), None).unwrap();
        assert_eq!(recorded, robot());
        assert_eq!(recorded.scope(), service);
        assert_eq!(recorded.principal(), Some("svc:robot"));

        let open =
            ConsoleScheduleSender::of_caller(&ConsoleSendScope::Anonymous, None, None).unwrap();
        assert_eq!(open, ConsoleScheduleSender::Anonymous);
        assert_eq!(open.scope(), ConsoleSendScope::Anonymous);
        assert_eq!(open.principal(), None);
        assert_eq!(
            serde_json::to_value(&open).unwrap(),
            json!({ "kind": "anonymous" })
        );

        assert_eq!(
            ConsoleScheduleSender::of_caller(&ConsoleSendScope::Unscoped, None, None),
            None
        );
    }

    /// Console decisions that trust `issuer`, require sign-in as
    /// `require_app_auth` says, and admit `allowlist` under the generic OIDC
    /// provider.
    fn decisions(require_app_auth: bool, issuer: &str, allowlist: &[&str]) -> RuntimeDecisionState {
        let mut decisions = RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth,
                ..crate::ConsolePolicy::default()
            },
            None,
        );
        decisions.trusted_oidc.discovery_json =
            json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
        decisions.auth = crate::AuthPolicy {
            default_provider: crate::AuthProvider::GenericOidc,
            email_allowlist: allowlist.iter().map(|entry| (*entry).to_string()).collect(),
        };
        decisions
    }

    /// A fire's sender is admitted as a live request by it is: an anonymous
    /// sender only while no sign-in is required; a principal under the
    /// issuer the Console trusts, through its provider policy and allowlist
    /// where sign-in is required, and past its session fence, whatever kind
    /// of principal it is.
    #[test]
    fn the_console_admits_a_sender_as_it_admits_a_live_caller() {
        let fences = ConsoleSessionFences::detached();
        let defined_at = 1_700_000_000;
        let open = decisions(false, ISSUER, &[]);
        let signed_in = decisions(true, ISSUER, &["erin@example.test", "svc:robot"]);

        let anonymous = ConsoleScheduleSender::Anonymous;
        assert_eq!(anonymous.admitted_now(&open, &fences, defined_at), Ok(()));
        let refused = anonymous
            .admitted_now(&signed_in, &fences, defined_at)
            .unwrap_err();
        assert!(refused.contains("requires sign-in"), "{refused}");

        for principal in [sender(), robot()] {
            assert_eq!(principal.admitted_now(&open, &fences, defined_at), Ok(()));
            assert_eq!(
                principal.admitted_now(&signed_in, &fences, defined_at),
                Ok(())
            );
            let removed = principal
                .admitted_now(&decisions(true, ISSUER, &[]), &fences, defined_at)
                .unwrap_err();
            assert!(removed.contains("no longer admits"), "{removed}");
            let elsewhere = principal
                .admitted_now(
                    &decisions(
                        true,
                        "https://other.example.test",
                        &["erin@example.test", "svc:robot"],
                    ),
                    &fences,
                    defined_at,
                )
                .unwrap_err();
            assert!(elsewhere.contains("no longer trusts"), "{elsewhere}");
        }
        let mut google = signed_in;
        google.auth.default_provider = crate::AuthProvider::GoogleOAuth;
        let other_provider = sender()
            .admitted_now(&google, &fences, defined_at)
            .unwrap_err();
        assert!(
            other_provider.contains("no longer admits"),
            "{other_provider}"
        );

        // A revocation after the definition ends the schedule's sends, for a
        // service identity as for a human; one before it does not.
        let revoked_at =
            |seconds: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        for principal in [sender(), robot()] {
            let state_dir = tempfile::tempdir().unwrap();
            let fences = ConsoleSessionFences::open_in_state_dir(state_dir.path()).unwrap();
            fences
                .revoke_sessions(principal.principal().unwrap(), revoked_at(defined_at - 600))
                .unwrap();
            assert_eq!(principal.admitted_now(&open, &fences, defined_at), Ok(()));
            fences
                .revoke_sessions(principal.principal().unwrap(), revoked_at(defined_at))
                .unwrap();
            let revoked = principal
                .admitted_now(&open, &fences, defined_at)
                .unwrap_err();
            assert!(revoked.contains("revoked"), "{revoked}");
        }
    }

    fn send() -> ConsoleScheduledSend {
        ConsoleScheduledSend::new(
            "agent:notes".to_string(),
            "Post the morning summary.".to_string(),
            sender(),
            1_700_000_000,
        )
    }

    fn every_hour() -> TriggerSpec {
        TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: chrono::Utc::now() + chrono::Duration::hours(1),
            every_seconds: 3600,
            end_at_utc: None,
        })
    }

    fn create_request(target: TargetBinding) -> CreateScheduleRequest {
        CreateScheduleRequest {
            name: None,
            description: None,
            trigger: every_hour(),
            target,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        }
    }

    /// A console schedule's send round-trips through its host-runnable
    /// target, and only the console's runnable with this schema reads as one.
    #[test]
    fn a_console_send_round_trips_through_its_target() {
        let target = send().target().unwrap();
        assert!(targets_console_runnable(&target));
        assert_eq!(ConsoleScheduledSend::of_target(&target), Some(send()));

        let other = TargetBinding::host_runnable(HostRunnableTargetBinding {
            runnable: HostRunnableName::parse("nightly-report").unwrap(),
            params: Some(
                HostRunnableParams::try_from(serde_json::to_value(send()).unwrap()).unwrap(),
            ),
        });
        assert!(!targets_console_runnable(&other));
        assert_eq!(ConsoleScheduledSend::of_target(&other), None);

        let mut other_schema = serde_json::to_value(send()).unwrap();
        other_schema["schema"] = json!("meta.console.scheduled_send.v0");
        assert_eq!(
            ConsoleScheduledSend::from_params(&other_schema.to_string()),
            None
        );
    }

    /// The console's runnable name is reserved for console schedules.
    #[test]
    fn the_console_runnable_name_is_reserved() {
        let reserved = HostRunnableName::parse(CONSOLE_SCHEDULE_RUNNABLE).unwrap();
        assert!(refuse_reserved_runnable_name(&reserved).is_err());
        let other = HostRunnableName::parse("nightly-report").unwrap();
        assert!(refuse_reserved_runnable_name(&other).is_ok());
    }

    struct RecordingDoor(std::sync::Mutex<Vec<(ConsoleScheduledSend, String)>>);

    #[async_trait]
    impl ConsoleScheduleDoor for RecordingDoor {
        async fn send(
            &self,
            send: &ConsoleScheduledSend,
            delivery_key: &str,
        ) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((send.clone(), delivery_key.to_string()));
            Ok(())
        }
    }

    fn invocation(runnable: &str, params: Option<Value>) -> HostRunnableInvocation {
        HostRunnableInvocation {
            occurrence_id: meerkat::OccurrenceId::new(),
            schedule_id: ScheduleId::new(),
            delivery_idempotency_key: "schedule:s:occurrence:o".to_string(),
            runnable: HostRunnableName::parse(runnable).unwrap(),
            trigger_time: chrono::Utc::now(),
            params: params.map(|params| RawValue::from_string(params.to_string()).unwrap()),
        }
    }

    /// The composed runnable host serves the console's runnable through the
    /// installed Console send path, with the occurrence's delivery key, and
    /// leaves every other name to the host's own runnables.
    #[tokio::test]
    async fn the_console_runnable_sends_through_the_installed_door() {
        let doors = ConsoleScheduleDoorSlot::default();
        let host = with_console_schedule_runnable(None, &doors);
        assert_eq!(
            host.probe_runnable(&HostRunnableName::parse(CONSOLE_SCHEDULE_RUNNABLE).unwrap()),
            RunnableProbe::Registered
        );
        assert_eq!(
            host.probe_runnable(&HostRunnableName::parse("nightly-report").unwrap()),
            RunnableProbe::Unknown
        );
        assert!(matches!(
            host.run_occurrence(invocation("nightly-report", None))
                .await,
            Err(HostRunnableError::Unregistered { .. })
        ));

        let door = Arc::new(RecordingDoor(std::sync::Mutex::new(Vec::new())));
        doors.install(door.clone());
        host.run_occurrence(invocation(
            CONSOLE_SCHEDULE_RUNNABLE,
            Some(serde_json::to_value(send()).unwrap()),
        ))
        .await
        .unwrap();
        assert_eq!(
            *door.0.lock().unwrap(),
            vec![(send(), "schedule:s:occurrence:o".to_string())]
        );

        // A send that does not read is refused, and nothing is sent.
        let refused = host
            .run_occurrence(invocation(
                CONSOLE_SCHEDULE_RUNNABLE,
                Some(json!({ "identity": "agent:notes" })),
            ))
            .await;
        assert!(matches!(refused, Err(HostRunnableError::Failed { .. })));
        assert_eq!(door.0.lock().unwrap().len(), 1);
    }

    /// A fire whose runtime stopped (its door slot dropped) is refused at
    /// once instead of waiting for a Console.
    #[tokio::test]
    async fn a_fire_after_the_runtime_stopped_is_refused() {
        let doors = ConsoleScheduleDoorSlot::default();
        let host = with_console_schedule_runnable(None, &doors);
        drop(doors);
        let detail = match host
            .run_occurrence(invocation(
                CONSOLE_SCHEDULE_RUNNABLE,
                Some(serde_json::to_value(send()).unwrap()),
            ))
            .await
        {
            Err(HostRunnableError::Failed { detail }) => detail,
            other => panic!("a stopped runtime refuses the fire: {other:?}"),
        };
        assert!(detail.contains("stopped"), "{detail}");
    }

    /// A fire claimed before the console router is built (an occurrence due
    /// at boot, on a gateway that builds its router after starting its
    /// schedule host) waits for the router, and sends once the router
    /// installs its send path within the window that opened when the host
    /// started.
    #[tokio::test]
    async fn a_fire_claimed_before_the_console_router_waits_for_it() {
        let doors = ConsoleScheduleDoorSlot::default();
        let host = with_console_schedule_runnable(None, &doors);
        let pending = tokio::spawn({
            let host = Arc::clone(&host);
            async move {
                host.run_occurrence(invocation(
                    CONSOLE_SCHEDULE_RUNNABLE,
                    Some(serde_json::to_value(send()).unwrap()),
                ))
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !pending.is_finished(),
            "the fire waits for the Console's send path"
        );
        let door = Arc::new(RecordingDoor(std::sync::Mutex::new(Vec::new())));
        doors.install(door.clone());
        pending.await.unwrap().unwrap();
        assert_eq!(
            *door.0.lock().unwrap(),
            vec![(send(), "schedule:s:occurrence:o".to_string())]
        );
    }

    /// Past the window that opened when the schedule host started, a fire
    /// with no Console send path fails at once, and a cleared send path (its
    /// projection replaced, or its runtime shutting down) is released and
    /// sends nothing.
    #[tokio::test]
    async fn a_fire_without_a_console_send_path_fails_at_once() {
        let doors = ConsoleScheduleDoorSlot::default();
        let host = console_schedule_runnable_waiting(None, &doors, Duration::ZERO);
        let fire = || {
            host.run_occurrence(invocation(
                CONSOLE_SCHEDULE_RUNNABLE,
                Some(serde_json::to_value(send()).unwrap()),
            ))
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            fire().await,
            Err(HostRunnableError::Failed { .. })
        ));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a fire past the boot window does not wait for a Console"
        );

        let door = Arc::new(RecordingDoor(std::sync::Mutex::new(Vec::new())));
        doors.install(door.clone());
        fire().await.unwrap();
        doors.clear();
        assert!(matches!(
            fire().await,
            Err(HostRunnableError::Failed { .. })
        ));
        assert_eq!(
            door.0.lock().unwrap().len(),
            1,
            "a cleared send path sends nothing"
        );
        assert_eq!(
            Arc::strong_count(&door),
            1,
            "a cleared send path is released"
        );
    }

    /// The gate refuses every verb on a schedule it cannot read, since it may
    /// be a console schedule: nothing reaches the tools. An id no schedule
    /// has stays the tools' own answer.
    #[tokio::test]
    async fn agent_schedule_tools_refuse_a_schedule_they_cannot_read() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let console = service
            .create(create_request(send().target().unwrap()))
            .await
            .unwrap();
        let unreadable = ConsoleScheduleToolGate::new(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            ScheduleService::new(Arc::new(DisabledScheduleStore)),
        );
        let console_id = raw(&json!({ "schedule_id": console.schedule_id.to_string() }));
        for verb in SCHEDULE_TOOLS_BY_ID {
            let refused = unreadable
                .dispatch(call(verb, &console_id))
                .await
                .expect_err("a schedule the gate cannot read is refused");
            assert!(
                refused.to_string().contains("cannot be read"),
                "{verb}: {refused}"
            );
        }
        assert_eq!(
            service.get(&console.schedule_id).await.unwrap().phase,
            meerkat::SchedulePhase::Active,
            "the console schedule is untouched"
        );

        let readable = ConsoleScheduleToolGate::new(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            service.clone(),
        );
        let missing = raw(&json!({ "schedule_id": ScheduleId::new().to_string() }));
        let answer = match readable
            .dispatch(call("meerkat_schedule_pause", &missing))
            .await
        {
            Ok(outcome) => outcome.result.text_content(),
            Err(error) => error.to_string(),
        };
        assert!(!answer.contains("cannot be read"), "{answer}");
        assert!(!answer.contains("console schedule"), "{answer}");
    }

    fn call<'a>(name: &'a str, args: &'a RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "call-1",
            name,
            args,
        }
    }

    fn raw(value: &Value) -> Box<RawValue> {
        RawValue::from_string(value.to_string()).unwrap()
    }

    /// Agents' schedule tools cannot send as a Console caller: a create or
    /// update naming the console's runnable is refused, every verb on a
    /// console schedule is refused, and the listing leaves console schedules
    /// out, while the agent's own schedules pass as before.
    #[tokio::test]
    async fn agent_schedule_tools_leave_console_schedules_alone() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let console = service
            .create(create_request(send().target().unwrap()))
            .await
            .unwrap();
        let own_target = json!({ "target_kind": "host_runnable", "runnable": "nightly-report" });
        let own = service
            .create(create_request(serde_json::from_value(own_target).unwrap()))
            .await
            .unwrap();
        let gate = ConsoleScheduleToolGate::new(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            service.clone(),
        );

        let forged = json!({
            "target_kind": "host_runnable",
            "runnable": CONSOLE_SCHEDULE_RUNNABLE,
            "params": serde_json::to_value(send()).unwrap(),
        });
        let create = raw(&json!({
            "trigger": serde_json::to_value(every_hour()).unwrap(),
            "target": forged,
        }));
        let refused = gate
            .dispatch(call("meerkat_schedule_create", &create))
            .await
            .expect_err("an agent cannot create a console schedule");
        assert!(
            refused.to_string().contains(CONSOLE_SCHEDULE_RUNNABLE),
            "{refused}"
        );
        let retarget = raw(&json!({
            "schedule_id": own.schedule_id.to_string(),
            "target": forged,
        }));
        assert!(
            gate.dispatch(call("meerkat_schedule_update", &retarget))
                .await
                .is_err(),
            "an agent cannot retarget its schedule to the console's runnable"
        );

        let console_id = raw(&json!({ "schedule_id": console.schedule_id.to_string() }));
        for verb in SCHEDULE_TOOLS_BY_ID {
            let refused = gate
                .dispatch(call(verb, &console_id))
                .await
                .expect_err("every verb on a console schedule is refused");
            assert!(
                refused.to_string().contains("console schedule"),
                "{verb}: {refused}"
            );
        }
        assert_eq!(
            service.get(&console.schedule_id).await.unwrap().phase,
            meerkat::SchedulePhase::Active,
            "the console schedule is untouched"
        );

        let empty = raw(&json!({}));
        let listed = gate
            .dispatch(call("meerkat_schedule_list", &empty))
            .await
            .unwrap();
        let listed: Value = serde_json::from_str(&listed.result.text_content()).unwrap();
        let ids = listed["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|schedule| schedule["schedule_id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![own.schedule_id.to_string()]);

        let own_id = raw(&json!({ "schedule_id": own.schedule_id.to_string() }));
        gate.dispatch(call("meerkat_schedule_pause", &own_id))
            .await
            .expect("the agent's own schedule passes");
        assert_eq!(
            service.get(&own.schedule_id).await.unwrap().phase,
            meerkat::SchedulePhase::Paused
        );
    }
}
