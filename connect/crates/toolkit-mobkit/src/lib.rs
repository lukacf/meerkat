//! Optional MobKit tool surface, selected through an explicitly registered native bundle.
//!
//! The host authenticates a real runtime/task invocation. Agents supply only
//! request content. The bound installed native owner admits or recovers setup
//! work and advances the helper through existing owners. Direct construction
//! cannot execute this path, and context-free calls are denied. Handoff
//! observations do not establish installation, readiness or original-task completion.

pub mod discovery;
#[cfg(all(feature = "native-host", unix))]
pub mod host_gateway;
#[cfg(all(feature = "native-host", unix))]
pub mod installed_host;
#[cfg(all(feature = "native-host", unix))]
pub mod original_ingress;
#[cfg(all(feature = "native-host", unix))]
pub mod private_host;
#[cfg(all(feature = "native-host", unix))]
pub mod private_originals;

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolCatalogCapabilities, ToolDef, ToolDispatchContext,
    ToolError, ToolMutationClass, ToolResult, ops::ToolDispatchOutcome,
};
use meerkat_toolkit::{
    authority::{SetupRequestEvidence, SetupUseContext},
    request::{ContextReferenceOwner, FindAndInstallRequest, HostSetupRequest, RequestError},
    setup::{SetupError, SetupScope, SetupStore},
};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use toolkit_catalog::AcquisitionPhase;
use toolkit_package::{Id, Revision};
use toolkit_store::{StoreError, ToolkitStore};

pub const FIND_AND_INSTALL: &str = "find_and_install";
/// Reserved native host bundle. Selecting it still requires the member's
/// explicit tools.rust_bundles entry and current native operation authority.
pub const FIND_AND_INSTALL_BUNDLE: &str = "meerkat_toolkit.setup";
const MAX_TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;

/// Owned current host context. None of these values is accepted in tool JSON.
/// The host must obtain policy and time from its actual enforcement boundary.
pub struct HostAdmissionContext {
    pub tenant: Id,
    pub deployment: Id,
    pub realm: meerkat_core::connection::RealmId,
    pub target: meerkat_authorization_contracts::work_association::NativeWorkTarget,
    pub policy_revision: Revision,
    pub now_ms: u64,
}

impl HostAdmissionContext {
    fn borrowed<'a>(
        &'a self,
        authorization: &'a meerkat_core::WorkAuthorizationContext,
    ) -> SetupUseContext<'a> {
        SetupUseContext {
            tenant: &self.tenant,
            deployment: &self.deployment,
            realm: &self.realm,
            target: &self.target,
            authorization,
            policy_revision: &self.policy_revision,
            now_ms: self.now_ms,
        }
    }
}

/// The host supplies exact original history and current application placement.
/// Serialized setup evidence is not current permission. The dispatcher borrows
/// the native WorkAuthorizationContext from this actual tool invocation; the
/// host cannot replace it by returning a context from another dispatch.
pub struct HostToolAdmission {
    pub host: HostSetupRequest,
    pub evidence: SetupRequestEvidence,
    pub context: HostAdmissionContext,
}

/// Closed redacted host result. Do not place arbitrary provider, auth or policy
/// error strings into the model-visible tool response.
#[derive(Debug, Clone, Copy)]
pub enum ToolAdmissionError {
    Denied,
    Unavailable,
}

/// Bound by application composition, never selected through model arguments.
/// Check the actual session attachment, admitted interaction/task, configured
/// optional capability and current requester ceiling before issuing context.
/// Tool call IDs and public runtime IDs are correlations, not authorization.
/// Correlation and continuation must be stable under a lost-response replay;
/// changed exact content under that correlation must conflict, not mint work.
/// Unknown/missing context and unsupported durable execution fail closed.
/// The host must load the exact original authority from authenticated invocation
/// custody and verify it against that expected reference, never its own hash alone.
/// The current destination comes from trusted placement, including realm and scope.
/// The host must retain the same original authority identity across retries,
/// revalidating it under current policy rather than issuing a fresh correlation.
/// A changed policy requires an explicit new host admission, not an implicit
/// duplicate. Authenticated setup-worker and continuation recovery paths must
/// be configured before this optional capability is exposed.
#[async_trait]
pub trait SetupToolHost: Send + Sync {
    async fn admit_invocation(
        &self,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<HostToolAdmission, ToolAdmissionError>;
}

/// Initial admission observation. A terminal phase on exact replay remains a
/// historical acquisition fact, never a fresh grant or business-task result.
#[derive(Serialize)]
struct OperationReply {
    operation: String,
    revision: u64,
    phase: AcquisitionPhase,
    created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion: Option<&'static str>,
    #[cfg(all(feature = "native-host", unix))]
    #[serde(skip_serializing_if = "Option::is_none")]
    worker: Option<installed_host::acquisition::SetupWorkerObservation>,
}

/// A leaf dispatcher installed explicitly for selected agents. Constructing this
/// value does not register it globally or grant runtime access. MobKit hosts
/// compose it with their existing tools and preserve selection on revival.
pub struct FindAndInstallTool {
    admission: FindAndInstallAdmission,
    tools: Arc<[Arc<ToolDef>]>,
    admission_timeout: Duration,
}

// Standalone callers retain their existing admission composition. The installed
// leaf has no store/source dependency until its one final owner is bound.
enum FindAndInstallAdmission {
    Direct(AdmissionDependencies),
    #[cfg(all(feature = "native-host", unix))]
    Installed(Arc<installed_host::acquisition::AcquisitionToolBinding>),
}
#[derive(Clone)]
pub(crate) struct AdmissionDependencies {
    store: Arc<dyn ToolkitStore>,
    conditional_store: Option<Arc<toolkit_store::SqliteStore>>,
    scope: SetupScope,
    host: Arc<dyn SetupToolHost>,
    references: Arc<dyn ContextReferenceOwner>,
}
struct AdmissionOwner {
    dependencies: AdmissionDependencies,
    #[cfg(all(feature = "native-host", unix))]
    acquisition: Option<Arc<installed_host::acquisition::InstalledAcquisition>>,
}

impl FindAndInstallTool {
    /// Retain standalone admission dependencies for protocol composition.
    /// Production dispatch requires the installed native owner; this constructor
    /// cannot enroll one or enable native setup execution.
    pub fn new(
        store: Arc<dyn ToolkitStore>,
        scope: SetupScope,
        host: Arc<dyn SetupToolHost>,
        references: Arc<dyn ContextReferenceOwner>,
        admission_timeout: Duration,
    ) -> Result<Self, ToolAdmissionError> {
        Self::validate_timeout(admission_timeout)?;
        store.capabilities().require_durable_contract()
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        Ok(Self {
            admission: FindAndInstallAdmission::Direct(AdmissionDependencies {
                store, conditional_store: None, scope, host, references,
            }),
            tools: vec![Arc::new(definition())].into(),
            admission_timeout,
        })
    }

    fn validate_timeout(timeout: Duration) -> Result<(), ToolAdmissionError> {
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(ToolAdmissionError::Unavailable);
        }
        Ok(())
    }

    #[cfg(all(feature = "native-host", unix))]
    pub(crate) fn enrolled(
        binding: Arc<installed_host::acquisition::AcquisitionToolBinding>,
        admission_timeout: Duration,
    ) -> Result<Self, ToolAdmissionError> {
        Self::validate_timeout(admission_timeout)?;
        Ok(Self {
            admission: FindAndInstallAdmission::Installed(binding),
            tools: vec![Arc::new(definition())].into(),
            admission_timeout,
        })
    }

    fn admission_owner(&self) -> Result<AdmissionOwner, ToolError> {
        match &self.admission {
            FindAndInstallAdmission::Direct(dependencies) => Ok(AdmissionOwner {
                dependencies: dependencies.clone(),
                #[cfg(all(feature = "native-host", unix))]
                acquisition: None,
            }),
            #[cfg(all(feature = "native-host", unix))]
            FindAndInstallAdmission::Installed(binding) => {
                let owner = binding.owner().ok_or_else(|| {
                    #[cfg(test)]
                    aa1_failure("leaf.installed_owner", "Unavailable");
                    unavailable()
                })?;
                Ok(AdmissionOwner {
                    dependencies: owner.tool_admission(),
                    acquisition: Some(owner),
                })
            }
        }
    }

    fn execution_owner(&self) -> Result<AdmissionOwner, ToolError> {
        let owner = self.admission_owner()?;
        #[cfg(all(feature = "native-host", unix))]
        if let Some(acquisition) = &owner.acquisition {
            acquisition.require_native_runtime().map_err(|error| {
                #[cfg(test)]
                aa1_failure("leaf.native_runtime", match error {
                    private_originals::PrivateOriginalError::Denied => "PrivateOriginalError::Denied",
                    private_originals::PrivateOriginalError::Conflict => "PrivateOriginalError::Conflict",
                    private_originals::PrivateOriginalError::Unavailable => "PrivateOriginalError::Unavailable",
                });
                let _ = error;
                unavailable()
            })?;
            return Ok(owner);
        }
        // Standalone dependencies never select the installed native owner.
        let _ = owner;
        #[cfg(test)]
        aa1_failure("leaf.direct_owner", "Unavailable");
        Err(unavailable())
    }

    async fn admit(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        owner: AdmissionOwner,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != FIND_AND_INSTALL {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        // The caller retained the exact installed owner before entering this
        // protocol. Native context and canonical source checks still authorize
        // this particular invocation; composition alone grants no work.
        let dependencies = &owner.dependencies;
        // Transcript interaction lineage is optional in a native admitted
        // turn. The installed source owner below requires the exact retained
        // work, RuntimeInput scope and canonical source association instead.
        if context.origin_session_id().is_none() {
            return Err(ToolError::AccessDenied {
                name: FIND_AND_INSTALL.into(),
            });
        }
        let authorization =
            context
                .work_authorization()
                .ok_or_else(|| ToolError::AccessDenied {
                    name: FIND_AND_INSTALL.into(),
                })?;
        if call.id.is_empty()
            || call.id.len() > 512
            || call.args.get().len() > MAX_TOOL_ARGUMENT_BYTES
        {
            return Err(invalid_arguments());
        }
        let request: FindAndInstallRequest = call.parse_args().map_err(|_| invalid_arguments())?;
        let mut admission = dependencies
            .host
            .admit_invocation(call.id, context)
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_admission_failure("leaf.host_admit", &error);
                match error {
                    ToolAdmissionError::Denied => ToolError::AccessDenied {
                        name: FIND_AND_INSTALL.into(),
                    },
                    ToolAdmissionError::Unavailable => unavailable(),
                }
            })?;
        let use_context = admission.context.borrowed(authorization);
        let store = match &dependencies.conditional_store {
            Some(store) => SetupStore::new_conditional(store.as_ref(), dependencies.scope.clone()),
            None => SetupStore::new(dependencies.store.as_ref(), dependencies.scope.clone()),
        }.map_err(|error| {
            #[cfg(test)]
            aa1_setup_failure("leaf.store", &error);
            let _ = error;
            unavailable()
        })?;
        let first = store.admit_or_recover(
            request.clone(), admission.host.clone(), &admission.evidence,
            &use_context, dependencies.references.as_ref(),
        ).await;
        let admitted = match first {
            Err(SetupError::Store(StoreError::AdmissionConflict)) if dependencies.conditional_store.is_some() => {
                // Two first admissions can observe no acquisition row, then
                // receive different clock terms. Read the winning terms once
                // through the same current installed owner. This retries no
                // work/effect and never changes the canonical raw request.
                let recovered = dependencies.host.admit_invocation(call.id, context).await
                    .map_err(|error| {
                        #[cfg(test)]
                        aa1_admission_failure("leaf.host_readback", &error);
                        match error {
                            ToolAdmissionError::Denied => ToolError::AccessDenied { name: FIND_AND_INSTALL.into() },
                            ToolAdmissionError::Unavailable => unavailable(),
                        }
                    })?;
                if !same_installed_admission(&admission, &recovered) {
                    return Err(setup_error(SetupError::Store(StoreError::AdmissionConflict)));
                }
                admission = recovered;
                store.admit_or_recover(
                    request, admission.host.clone(), &admission.evidence,
                    &admission.context.borrowed(authorization), dependencies.references.as_ref(),
                ).await.map_err(setup_error)?
            }
            result => result.map_err(setup_error)?,
        };
        #[cfg(all(feature = "native-host", unix))]
        let worker = if let Some(acquisition) = &owner.acquisition {
            let operation = Id::new(admitted.snapshot.key().id.clone()).map_err(|_| {
                #[cfg(test)]
                aa1_failure("leaf.operation_id", "IdError");
                unavailable()
            })?;
            Some(acquisition.advance(&operation, call, context).await.map_err(|error| {
                #[cfg(test)]
                aa1_setup_failure("leaf.advance", &error);
                setup_error(error)
            })?)
        } else {
            None
        };
        let reply = OperationReply {
            operation: admitted.snapshot.key().id.clone(),
            revision: admitted.snapshot.state().revision,
            phase: admitted.snapshot.state().lifecycle_phase,
            created: admitted.created,
            completion: admitted.snapshot.approved_service_was_admitted().then_some(
                "The approved follow-up instruction was admitted. The agent reports its task result separately."),
            #[cfg(all(feature = "native-host", unix))]
            worker,
        };
        let content = serde_json::to_string(&reply).map_err(|_| {
            #[cfg(test)]
            aa1_failure("leaf.reply_encoding", "serde_json::Error");
            unavailable()
        })?;
        Ok(ToolResult::new(call.id.into(), content, false).into())
    }
}

// Only the installed owner may choose the already persisted clock terms. Every
// non-time term must still identify the same exact request and native source.
fn same_installed_admission(first: &HostToolAdmission, replay: &HostToolAdmission) -> bool {
    let a = first.evidence.constraints();
    let b = replay.evidence.constraints();
    first.evidence.association() == replay.evidence.association()
        && a.tenant == b.tenant && a.deployment == b.deployment
        && a.policy_revision == b.policy_revision && a.ceiling == b.ceiling
        && first.host.target == replay.host.target
        && first.host.correlation == replay.host.correlation
        && first.host.parent_operation == replay.host.parent_operation
        && first.host.setup_helper == replay.host.setup_helper
        && first.host.continuation == replay.host.continuation
        && first.host.environment_revision == replay.host.environment_revision
        && first.host.limits.max_workers == replay.host.limits.max_workers
        && first.host.limits.max_effect_attempts == replay.host.limits.max_effect_attempts
        && first.context.tenant == replay.context.tenant
        && first.context.deployment == replay.context.deployment
        && first.context.realm == replay.context.realm
        && first.context.target == replay.context.target
        && first.context.policy_revision == replay.context.policy_revision
}

#[async_trait]
impl AgentToolDispatcher for FindAndInstallTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        // One immutable leaf tool: the default visible catalog is complete.
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == FIND_AND_INSTALL {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != FIND_AND_INSTALL {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        Err(ToolError::AccessDenied {
            name: FIND_AND_INSTALL.into(),
        })
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != FIND_AND_INSTALL {
            return Err(ToolError::NotFound { name: call.name.into() });
        }
        let owner = self.execution_owner()?;
        tokio::time::timeout(
            self.admission_timeout,
            self.admit(call, context, owner),
        )
        .await
        .map_err(|_| {
            #[cfg(test)]
            aa1_failure("leaf.timeout", "Elapsed");
            unavailable()
        })?
    }
}

fn request_error(error: RequestError) -> ToolError {
    match error {
        RequestError::EmptyRequest | RequestError::InputLimit => invalid_arguments(),
        RequestError::Encoding(_)
        | RequestError::Authority(meerkat_toolkit::authority::SetupContextError::InvalidEncoding) => unavailable(),
        _ => ToolError::AccessDenied {
            name: FIND_AND_INSTALL.into(),
        },
    }
}
fn setup_error(error: SetupError) -> ToolError {
    #[cfg(test)]
    aa1_setup_failure("leaf.setup_projection", &error);
    match error {
        SetupError::Request(error) => request_error(error),
        SetupError::WrongScope | SetupError::Denied => ToolError::AccessDenied { name: FIND_AND_INSTALL.into() },
        SetupError::Store(StoreError::AdmissionConflict) => ToolError::InvalidArguments {
            name: FIND_AND_INSTALL.into(),
            reason: "This invocation already has different persisted input. Repeat its original input or use a new host-admitted invocation.".into(),
        },
        _ => unavailable(),
    }
}
// Failure-only diagnostics for the existing exact AA1 executor. Labels are
// fixed variants: no error payload, request, source, grant or identity is logged.
#[cfg(test)]
fn aa1_failure(stage: &'static str, variant: &'static str) {
    eprintln!("AA1 failure: stage={stage} variant={variant}");
}

#[cfg(test)]
fn aa1_admission_failure(stage: &'static str, error: &ToolAdmissionError) {
    aa1_failure(stage, match error {
        ToolAdmissionError::Denied => "ToolAdmissionError::Denied",
        ToolAdmissionError::Unavailable => "ToolAdmissionError::Unavailable",
    });
}

#[cfg(test)]
fn aa1_setup_failure(stage: &'static str, error: &SetupError) {
    use meerkat_toolkit::work_admission::SetupWorkAdmissionError as WorkError;
    let variant = match error {
        SetupError::Store(error) => match error {
            StoreError::OwnerUnavailable => "Store::OwnerUnavailable",
            StoreError::Invalid(_) => "Store::Invalid",
            StoreError::NotFound => "Store::NotFound",
            StoreError::AlreadyExists => "Store::AlreadyExists",
            StoreError::AdmissionConflict => "Store::AdmissionConflict",
            StoreError::Conflict => "Store::Conflict",
            StoreError::AdmissionExpired => "Store::AdmissionExpired",
            StoreError::EffectConflict => "Store::EffectConflict",
            StoreError::Claimed => "Store::Claimed",
            StoreError::Fenced => "Store::Fenced",
            StoreError::EffectState => "Store::EffectState",
            StoreError::UnsupportedFormat(_) => "Store::UnsupportedFormat",
            StoreError::Synchronization => "Store::Synchronization",
            StoreError::Worker => "Store::Worker",
            StoreError::Storage(_) => "Store::Storage",
            StoreError::Encoding(_) => "Store::Encoding",
        },
        SetupError::WorkAdmission(error) => match error {
            WorkError::NativeExecutionUnavailable => "WorkAdmission::NativeExecutionUnavailable",
            WorkError::DurableStoreRequired => "WorkAdmission::DurableStoreRequired",
            WorkError::OwnerTaskFailed => "WorkAdmission::OwnerTaskFailed",
            WorkError::Timeout => "WorkAdmission::Timeout",
            WorkError::MissingReceipt => "WorkAdmission::MissingReceipt",
            WorkError::Denied => "WorkAdmission::Denied",
            WorkError::ScopeMismatch => "WorkAdmission::ScopeMismatch",
            WorkError::ReservedCorrelation => "WorkAdmission::ReservedCorrelation",
            WorkError::Encoding(_) => "WorkAdmission::Encoding",
            WorkError::WorkGraph(_) => "WorkAdmission::WorkGraph",
        },
        SetupError::Request(_) => "SetupError::Request",
        SetupError::Binding(_) => "SetupError::Binding",
        SetupError::Continuation(_) => "SetupError::Continuation",
        SetupError::WorkGraph(_) => "SetupError::WorkGraph",
        SetupError::WrongScope => "SetupError::WrongScope",
        SetupError::InvalidRecord => "SetupError::InvalidRecord",
        SetupError::UnsupportedRecordVersion => "SetupError::UnsupportedRecordVersion",
        SetupError::NativeExecutionUnavailable => "SetupError::NativeExecutionUnavailable",
        SetupError::InvalidSelection => "SetupError::InvalidSelection",
        SetupError::Rejected => "SetupError::Rejected",
        SetupError::Denied => "SetupError::Denied",
        SetupError::Encoding(_) => "SetupError::Encoding",
    };
    aa1_failure(stage, variant);
}

fn invalid_arguments() -> ToolError {
    ToolError::InvalidArguments { name: FIND_AND_INSTALL.into(), reason: "Expected the original request, user_context and caller_observations within the inline limit; authority and routing come from the host.".into() }
}
fn unavailable() -> ToolError {
    ToolError::unavailable(
        FIND_AND_INSTALL,
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
    )
}

/// The model can preserve ambiguity and supply observations, but cannot forge
/// any host-routing or authority field through this strict content schema.
pub fn definition() -> ToolDef {
    ToolDef::new(
        FIND_AND_INSTALL,
        "Ask dedicated setup agents to find, reuse or connect integrations needed for this task. Preserve the user's complete wording, constraints and ambiguity; include supplied context and label your observations. The installed host checks current source, helper and continuation authority before setup can run. When the setup worker reports input_admitted, setup continues asynchronously: the host automatically delivers its outcome back to this task and resumes it subject to the host's current authority checks. You can finish this turn without asking the user to report completion or send another message to wake you. Required review and authentication still use the protected setup controls. Admission is not installation, attachment readiness or permission to use a service. Attachment failure is not an access-denial or grant-revocation result: do not infer that no access was granted. Describe attachment readiness separately from approval and grant state; the access owner remains authoritative for current grant state. After a failed setup, do not replay its fenced replacement instruction or reuse its approval. A new find_and_install call is a separate proposal: if the original user request still covers further setup and current authority permits it, you may call again without another chat authorization solely because the prior attempt failed. Preserve the original limits and include the observed failure in caller_observations. Any new software or access still requires its ordinary separate reviews. Do not loop on an unchanged failed choice.",
        serde_json::json!({
            "type": "object", "additionalProperties": false, "required": ["request"],
            "properties": {
                "request": { "type": "string", "minLength": 1 },
                "user_context": { "type": "array", "maxItems": 256, "items": {
                    "oneOf": [ { "type": "string" }, { "$ref": "#/$defs/context_reference" } ]
                } },
                "caller_observations": { "type": "array", "maxItems": 256, "items": { "type": "string" } }
            },
            "$defs": { "context_reference": {
                "type": "object", "additionalProperties": false,
                "required": ["owner", "id", "revision", "content_digest"],
                "properties": {
                    "owner": {"type":"string"}, "id": {"type":"string"}, "revision": {"type":"string"},
                    "content_digest": {"type":"string", "pattern":"^[0-9a-f]{64}$"}
                }
            } }
        }),
    )
}

#[cfg(test)]
impl FindAndInstallTool {
    async fn dispatch_protocol_fixture(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let owner = self.admission_owner()?;
        tokio::time::timeout(
            self.admission_timeout,
            self.admit(call, context, owner),
        )
        .await
        .map_err(|_| unavailable())?
    }
}

#[cfg(test)]
mod protocol_tests;
