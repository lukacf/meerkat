//! Class declarations become concrete policy only through the installed
//! application's existing policy owner. These preparation inputs are not an
//! admission, a roster, or evidence that native construction has finished.

use meerkat_core::{AgentToolDispatcher, CompiledMemberToolGrants, RealmId, ToolDef, ToolIdentity};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError,
    policy::{CompiledMemberClass, CompiledMemberClassInstance},
};
use serde::{Deserialize, Serialize};
use toolkit_package::Id;

/// The selected trusted factory's actual tool definitions, checked before
/// native preparation loses descriptions and input schemas. Each native
/// construction uses its own checked factory output. A later construction
/// reconstructs it through the trusted factory and checks the same declaration.
///
/// This type has no wire decoder: agent arguments cannot provide factory
/// implementations, tool definitions or installed artifact associations.
#[derive(Clone)]
pub struct PreparedMemberClassTools {
    class: CompiledMemberClass,
    dispatcher: std::sync::Arc<dyn AgentToolDispatcher>,
    definitions: Vec<ToolDef>,
    identities: Vec<ToolIdentity>,
}

impl PreparedMemberClassTools {
    /// Trusted composition only. The actual dispatcher comes from the host
    /// implementation factory registered for this packaged declaration.
    /// This checked output supplies one construction's actual dispatcher.
    /// Admission retains its definitions as a stable reconstruction contract;
    /// dispatcher object identity does not persist across builds or restart.
    pub fn from_factory(
        class: &CompiledMemberClass,
        factory: &Id,
        dispatcher: std::sync::Arc<dyn AgentToolDispatcher>,
    ) -> Result<Self, HostLifetimeError> {
        class.validate()?;
        let definitions = dispatcher
            .tools()
            .iter()
            .map(|tool| (**tool).clone())
            .collect::<Vec<_>>();
        if factory != &class.factory {
            return Err(HostLifetimeError::WrongOwner);
        }
        let identities = definitions
            .iter()
            .map(ToolDef::identity)
            .collect::<Vec<_>>();
        if identities.iter().enumerate().any(|(index, identity)| {
            identities[..index]
                .iter()
                .any(|prior| prior.name == identity.name)
        }) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        for callback in &class.callbacks {
            let declaration = callback.definition()?;
            let actual = definitions
                .iter()
                .find(|tool| tool.name == declaration.name)
                .ok_or(HostLifetimeError::InvalidSelection)?;
            if actual.description != declaration.description
                || actual.input_schema != declaration.input_schema
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        Ok(Self {
            class: class.clone(),
            dispatcher,
            definitions,
            identities,
        })
    }

    pub fn dispatcher(&self) -> std::sync::Arc<dyn AgentToolDispatcher> {
        self.dispatcher.clone()
    }
    pub fn definitions(&self) -> &[ToolDef] {
        &self.definitions
    }
    pub fn identities(&self) -> &[ToolIdentity] {
        &self.identities
    }
    pub fn class(&self) -> &CompiledMemberClass {
        &self.class
    }

    pub(crate) fn grants(
        &self,
        realm: &RealmId,
        instance: &CompiledMemberClassInstance,
    ) -> Result<CompiledMemberToolGrants, HostLifetimeError> {
        self.class.materialize(realm, instance)
    }
}

/// A trusted application's requested instance, before native held-add. The
/// existing policy owner validates its packaged class, caller and current
/// decision, and supplies the opaque admission retained by registration.
/// Neither this request nor its identifier grants authority.
pub struct MemberClassAdmissionRequest {
    pub id: Id,
    pub instance: CompiledMemberClassInstance,
    pub tools: PreparedMemberClassTools,
}

/// Concrete application decision retained in the existing policy owner's
/// canonical slot. It is not the desired roster or a native identity registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberClassAdmission {
    pub id: Id,
    pub class: CompiledMemberClass,
    pub instance: CompiledMemberClassInstance,
    /// Exact host-selected shared source. Absence retains ordinary behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<MemberClassWorkflowBinding>,
    /// Exact host-owned resource evidence for this workflow. This selects
    /// bytes only; current policy, native registration and physical/approval
    /// owners still decide whether those resources can be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_resource: Option<meerkat_toolkit::host_lifetime::control::SelectedHostArtifact>,
    /// Last destination allocated by the canonical policy owner for an actual
    /// native registration. The immutable instance supplies only its initial
    /// generation. Retiring an admission preserves this high-water mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<MemberClassDestination>,
    /// Exact custom callback identities. None means construction has not yet
    /// observed them; Some(empty) means an actual empty callback surface.
    /// Native builtins are recorded separately below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_surface: Option<Vec<ToolIdentity>>,
    /// Complete identities observed from this actual native preparation. None
    /// means no native surface has been authorized; Some(empty) is valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_surface: Option<Vec<ToolIdentity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute>,
    /// Add-owned standing intent and exact native parent references. Absence
    /// is legacy behavior, never permission to derive a mandate on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_operations: Option<MemberStandingOperations>,
    pub phase: MemberClassAdmissionPhase,
}

/// A finite new-Add mandate. These IDs are persisted before issuance; only
/// the native grant owner proves issuance/currentness. Parents alone do not
/// authorize an input. Any peer child must be confined to its actual incoming
/// delivery work and ended with that work, never reused for proactive sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberStandingOperations {
    pub peer_message_parent: meerkat_authorization_contracts::evidence::EvidenceId,
    pub recorder_write_parent: meerkat_authorization_contracts::evidence::EvidenceId,
    pub recorder_scope: meerkat_core::MemoryScopeRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued: Option<MemberStandingParents>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberStandingParents {
    pub peer_message: meerkat_authorization_contracts::work_association::GrantLineageRef,
    pub recorder_write: meerkat_authorization_contracts::work_association::GrantLineageRef,
}

/// Immutable association with the existing workflow/source owners. This is
/// neither an operation envelope nor permission to submit the planning input.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberClassWorkflowBinding {
    pub workflow: meerkat_toolkit::setup::SetupWorkflowRef,
    pub parent: crate::private_originals::PrivateOriginalRef,
    pub original: crate::private_originals::PrivateOriginalRef,
}
impl std::fmt::Debug for MemberClassWorkflowBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberClassWorkflowBinding")
            .field("workflow", &self.workflow)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberClassDestination {
    pub context: meerkat_authorization_contracts::evidence::EvidenceId,
    pub context_generation: u64,
}

/// A read of the existing canonical policy slot, used only as a conditional
/// re-admission or resource-selection precondition. It is not native identity
/// or permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberClassCheckpoint {
    pub admission: Id,
    pub revision: u64,
}

/// One canonical slot read and its separately checked authorization. Inspecting
/// a retained record, including a retired one, does not admit native work.
#[derive(Clone, Debug)]
pub struct MemberClassObservation {
    pub checkpoint: MemberClassCheckpoint,
    pub admission: MemberClassAdmission,
    pub authorization: Result<(), HostLifetimeError>,
}

impl MemberClassAdmission {
    /// Project the canonical registration selection, never change the
    /// application owner's immutable class/instance declaration.
    pub fn selected_member(
        &self,
    ) -> Result<meerkat_toolkit::host_lifetime::policy::PreparedNativeMember, HostLifetimeError>
    {
        let destination = self.destination.as_ref().ok_or(HostLifetimeError::Denied)?;
        let mut member = self.instance.member.clone();
        member.context_generation = destination.context_generation;
        Ok(member)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
/// Connect authorization state, not native identity or runtime lifecycle.
/// Active means this source authorization was published from held native
/// evidence. It does not assert that the native member is currently runnable.
pub enum MemberClassAdmissionPhase {
    Reserved,
    Prepared {
        context: meerkat_authorization_contracts::evidence::EvidenceId,
        context_generation: u64,
    },
    Active {
        context: meerkat_authorization_contracts::evidence::EvidenceId,
        context_generation: u64,
    },
    Retired,
}

/// The same trusted application policy participant authorizes this complete
/// class/instance binding, including recovery. A recovered record is evidence
/// to inspect, never an authorization by itself.
pub struct ApplicationMemberPolicyRequest<'a> {
    pub owner: &'a meerkat_core::PrincipalRef,
    pub realm: &'a RealmId,
    pub deployment: &'a str,
    pub admission: &'a MemberClassAdmission,
    pub canonical_policy: &'a [u8],
}

/// One admitted request on the same canonical policy owner. It is deliberately
/// neither serializable nor constructible from an agent's request fields.
/// A declaration-only reservation has no dispatcher. The trusted native
/// customizer checks and binds the actual output during each construction.
pub struct AdmittedMemberClass {
    pub(crate) owner: std::sync::Arc<super::tool_policy::service_policy::ServicePolicyOwner>,
    pub(crate) admission: MemberClassAdmission,
    pub(crate) tools: Option<PreparedMemberClassTools>,
    pub(crate) add_token: Option<MemberClassAddToken>,
}

/// One process-owned transfer right, never native registration evidence or a
/// canonical lifecycle fact. Only a newly persisted reservation creates it.
pub(crate) struct MemberClassAddToken(std::sync::atomic::AtomicBool);
impl MemberClassAddToken {
    pub(crate) fn new() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
    pub(crate) fn claimed(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(crate) fn consume(&self) -> Result<(), HostLifetimeError> {
        use std::sync::atomic::Ordering;
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| HostLifetimeError::Denied)
    }
}

impl AdmittedMemberClass {
    /// Transfer this new reservation to the host's already reserved Add or
    /// unaccepted-reservation retirement task.
    /// The host must synchronously spawn that task after success, without an
    /// await or fallible preflight that could drop accepted cleanup ownership.
    pub fn claim_add(&self) -> Result<(), HostLifetimeError> {
        self.owner.claim_member_add(
            &self.admission,
            self.add_token.as_ref().ok_or(HostLifetimeError::Denied)?,
        )
    }
    pub fn dispatcher(&self) -> Option<std::sync::Arc<dyn AgentToolDispatcher>> {
        self.tools
            .as_ref()
            .map(PreparedMemberClassTools::dispatcher)
    }
    pub fn class(&self) -> &CompiledMemberClass {
        &self.admission.class
    }
    /// Check evidence received inside the running host's genuine native held
    /// callback. The public observation is not itself publication authority.
    pub fn verify_held(
        &self,
        held: &meerkat_mobkit::identity_first::HeldIdentityMember,
    ) -> Result<meerkat_authorization_contracts::evidence::EvidenceId, HostLifetimeError> {
        let current = self
            .owner
            .observed_admission(&self.admission.instance.member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        let canonical =
            meerkat_mobkit::member_comms_id::roster_member_id_for_identity(held.identity.as_str());
        if canonical.as_str() != current.instance.member.member.member
            || current.id != self.admission.id
            || current.class != self.admission.class
            || current.instance != self.admission.instance
            || current.workflow != self.admission.workflow
            || current.installed_surface.is_none()
            || self.tools.as_ref().is_some_and(|tools| {
                current.installed_surface.as_deref() != Some(tools.identities())
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        let context = match &current.phase {
            MemberClassAdmissionPhase::Prepared {
                context,
                context_generation,
            } if current.destination.as_ref().is_some_and(|selected| {
                selected.context == *context && selected.context_generation == *context_generation
            }) =>
            {
                context.clone()
            }
            _ => return Err(HostLifetimeError::Denied),
        };
        if current
            .destination
            .as_ref()
            .is_none_or(|selected| selected.context != context)
        {
            return Err(HostLifetimeError::Denied);
        }
        if current.native_surface.as_ref()
            != Some(&native_surface_identities(
                &held.finished_surface.compiled_against,
            )?)
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.owner
            .verify_member_surface_binding(&held.finished_surface)?;
        Ok(context)
    }

    pub async fn retire(&self) -> Result<(), HostLifetimeError> {
        self.owner.retire_member(&self.admission.id).await
    }
    pub fn instance(&self) -> &CompiledMemberClassInstance {
        &self.admission.instance
    }
    pub fn workflow(&self) -> Option<&MemberClassWorkflowBinding> {
        self.admission.workflow.as_ref()
    }
    pub fn admission_id(&self) -> &Id {
        &self.admission.id
    }
}

impl MemberClassAdmissionRequest {
    pub(crate) fn validate(
        &self,
        realm: &RealmId,
        classes: &[CompiledMemberClass],
    ) -> Result<CompiledMemberToolGrants, HostLifetimeError> {
        if self.instance.member.setup_capability.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let class = classes
            .iter()
            .find(|class| class.id == self.instance.class)
            .ok_or(HostLifetimeError::Denied)?;
        if !class.purpose.is_ordinary() || class != self.tools.class() {
            return Err(HostLifetimeError::Denied);
        }
        self.tools.grants(realm, &self.instance)
    }
}

/// A trusted application reconstructs one class's actual implementation for
/// this selected instance on fresh construction and restart. This interface
/// carries no executable code or factory choice from agent arguments.
pub trait MemberClassFactory: Send + Sync {
    fn prepare(
        &self,
        class: &CompiledMemberClass,
        instance: &CompiledMemberClassInstance,
    ) -> Result<PreparedMemberClassTools, HostLifetimeError>;
}

/// Collect the actual composed native surface, including an exact deferred
/// catalog. No source is inferred from a tool name or a provenance label.
pub(crate) fn native_surface_identities(
    surface: &meerkat_core::PreparedToolSurface,
) -> Result<Vec<ToolIdentity>, HostLifetimeError> {
    if surface
        .projected
        .iter()
        .any(|identity| !surface.tools.contains(identity))
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let mut identities = Vec::<ToolIdentity>::new();
    let catalog = match &surface.catalog {
        meerkat_core::PreparedToolCatalog::Complete(entries) => entries
            .iter()
            .map(|entry| &entry.identity)
            .collect::<Vec<_>>(),
        meerkat_core::PreparedToolCatalog::VisibleOnly => Vec::new(),
    };
    for identity in surface.tools.iter().chain(catalog) {
        if let Some(prior) = identities.iter().find(|prior| prior.name == identity.name) {
            if prior != identity {
                return Err(HostLifetimeError::InvalidSelection);
            }
        } else {
            identities.push(identity.clone());
        }
    }
    identities.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(identities)
}

pub(crate) mod callbacks;
mod customizer;

pub(crate) fn customize_member_classes(
    policy: super::tool_policy::PreparedInstalledToolPolicy,
    inner: std::sync::Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
    factory: std::sync::Arc<dyn MemberClassFactory>,
) -> std::sync::Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> {
    std::sync::Arc::new(customizer::MemberClassCustomizer {
        policy,
        inner,
        factory: Some(factory),
    })
}

/// The selected gateway customizer owns callback construction and artifact
/// verification. Observe its actual dispatcher after it returns, without
/// replacing it or accepting definitions from an admission request.
pub(crate) fn customize_gateway_member_classes(
    policy: super::tool_policy::PreparedInstalledToolPolicy,
    inner: std::sync::Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>,
) -> std::sync::Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> {
    std::sync::Arc::new(customizer::MemberClassCustomizer {
        policy,
        inner,
        factory: None,
    })
}
