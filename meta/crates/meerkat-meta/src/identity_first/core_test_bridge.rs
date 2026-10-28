//! Test fixture: a session bridge whose only answers are Core's member
//! reads, taken from a [`CoreTestMembers`].
//!
//! An identity's status, runtime id and session are Core's member
//! history. Tests whose subject is a view over that history (console and RPC
//! resolution, delivery routing, completion waits) bind the members they need
//! with [`bind_live`] instead of seeding a Meta continuity record. Every Core
//! write refuses, except a retire and a release: a retire ends the member,
//! and the history keeps its binding as retired; a release (Meta's retire
//! verb) leaves the member seated as it is. A bridge built with
//! [`CoreReadsBridge::with_registration_reload`] also answers Core's
//! registration reload, which leaves the member's binding as it is, one
//! built with [`CoreReadsBridge::with_host_human_input`] takes local host
//! human input on the live member's session, and one built with
//! [`CoreReadsBridge::with_deliveries`] admits a delivery to a live member on
//! its current session, as Core's delivery does.
//! [`CoreReadsBridge::fail_history_reads`] places a fault on chosen reads of
//! Core's member history, and [`CoreReadsBridge::refuse_release_of`] has
//! Core refuse a member's release, retryably, as Core does while the member
//! is being woken or has work in flight.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;

use super::IdentityRuntime;
use super::bridge::{
    BridgeDelivery, BridgeError, BridgeMemberReload, HostHumanInputError, MemberRuntimeBinding,
    ResumeSessionOutcome, SessionBridge,
};
use super::core_test_members::CoreTestMembers;
use super::types::{
    AgentBuildDraft, AgentIdentity, AgentRuntimeId, DurableAgentSpec, MemberReloadDisposition,
    SessionSnapshot,
};

/// Core holds `identity`'s member live at `generation`, bound to `session`
/// (`None` for a peer-only member) under the role `profile`. Its runtime id
/// is `rt:{identity}:{generation}`.
pub(crate) fn bind_live(
    core: &CoreTestMembers,
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    generation: u64,
    session: Option<SessionId>,
) -> Result<AgentRuntimeId, BridgeError> {
    let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
        .map_err(|error| BridgeError::Mob(error.to_string()))?;
    core.bind(
        identity,
        &MemberRuntimeBinding {
            agent_runtime_id: agent_runtime_id.clone(),
            session_id: session,
        },
        profile,
    );
    Ok(agent_runtime_id)
}

/// Register `spec` with Core holding its member live at `generation` on
/// `session`: the shape a materialized identity has at rest.
pub(crate) async fn register_live(
    runtime: &IdentityRuntime,
    core: &CoreTestMembers,
    spec: DurableAgentSpec,
    generation: u64,
    session: SessionId,
) -> Result<AgentRuntimeId, BridgeError> {
    let runtime_id = bind_live(
        core,
        &spec.identity,
        &spec.profile,
        generation,
        Some(session),
    )?;
    runtime.register(spec).await;
    Ok(runtime_id)
}

/// A bridge that answers Core's member reads from [`Self::core`] and commits
/// nothing but retires and releases.
#[derive(Default)]
pub(crate) struct CoreReadsBridge {
    pub(crate) core: CoreTestMembers,
    /// What Core's registration reload (`MobHandle::reload_member_registration`)
    /// answers for a live member. `None`: this bridge has no registration
    /// reload, so a reload retires the member and wakes it.
    registration_reload: Option<MemberReloadDisposition>,
    /// Set by [`Self::fail_history_reads`]. `None`: every read of Core's
    /// member history answers.
    history_fault: Mutex<Option<HistoryReadFault>>,
    /// Set by [`Self::with_host_human_input`]: this bridge takes local host
    /// human input. Otherwise it refuses it, as the trait default does.
    host_human_input: bool,
    /// The host human inputs this bridge took.
    host_human_inputs: AtomicUsize,
    /// Set by [`Self::with_deliveries`]: this bridge admits a delivery to a
    /// live member. Otherwise it refuses every delivery.
    delivers: bool,
    /// The incarnation each delivery this bridge admitted went to, in order.
    delivered: Mutex<Vec<AgentRuntimeId>>,
    /// The identities whose release Core refuses, retryably
    /// ([`Self::refuse_release_of`]). Empty: every release goes ahead.
    release_refusals: Mutex<BTreeSet<AgentIdentity>>,
    /// Each release this bridge carried out, in order.
    releases: Mutex<Vec<AgentIdentity>>,
}

/// The reads of Core's member history that still answer, then the reads
/// the fault fails.
struct HistoryReadFault {
    answered: u64,
    failed: u64,
}

impl CoreReadsBridge {
    /// A bridge whose registration reload answers `disposition` for the live
    /// member in place: the member keeps its session and its generation, as
    /// Core's reload does.
    pub(crate) fn with_registration_reload(disposition: MemberReloadDisposition) -> Self {
        Self {
            registration_reload: Some(disposition),
            ..Self::default()
        }
    }

    /// A bridge that also takes local host human input on the live member's
    /// session, as Core's host-human admission does, and counts each input.
    pub(crate) fn with_host_human_input() -> Self {
        Self {
            host_human_input: true,
            ..Self::default()
        }
    }

    /// The host human inputs this bridge took.
    pub(crate) fn host_human_inputs(&self) -> usize {
        self.host_human_inputs.load(Ordering::SeqCst)
    }

    /// A bridge that also admits a delivery to a live member on its current
    /// session, as Core's delivery does, and records the incarnation each
    /// delivery went to. A delivery to an incarnation that is not its
    /// identity's live member is refused.
    pub(crate) fn with_deliveries() -> Self {
        Self {
            delivers: true,
            ..Self::default()
        }
    }

    /// The incarnation each delivery this bridge admitted went to, in order.
    pub(crate) fn delivered(&self) -> Vec<AgentRuntimeId> {
        self.delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// From now on, the next `answered` reads of Core's member history
    /// answer, the `failed` reads after them fail as a store fault does, and
    /// later reads answer again: deterministic, so a test places a transient
    /// fault, or a lasting one (`u64::MAX`), on exactly the read under test.
    pub(crate) fn fail_history_reads(&self, answered: u64, failed: u64) {
        *self
            .history_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(HistoryReadFault { answered, failed });
    }

    /// From now on, Core refuses the release of `identity`'s member,
    /// retryably (Core's `LifecycleOperationPending`, which the bridge
    /// answers as [`BridgeError::MemberReleaseRefused`]), as it does while
    /// the member is being woken or has work in flight. Nothing changes for
    /// the member. [`Self::allow_release_of`] ends the refusal.
    pub(crate) fn refuse_release_of(&self, identity: &AgentIdentity) {
        self.release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    /// Core releases `identity`'s member again.
    pub(crate) fn allow_release_of(&self, identity: &AgentIdentity) {
        self.release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(identity);
    }

    /// Each release this bridge carried out, in order. A refused release is
    /// not one.
    pub(crate) fn releases(&self) -> Vec<AgentIdentity> {
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// One read of Core's member history against the fault, if any.
    fn take_history_read(&self) -> Result<(), BridgeError> {
        let mut guard = self
            .history_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(fault) = guard.as_mut() else {
            return Ok(());
        };
        if fault.answered > 0 {
            fault.answered -= 1;
            return Ok(());
        }
        if fault.failed > 0 {
            fault.failed -= 1;
            return Err(BridgeError::Mob(
                "Core's member history cannot be read (test fault)".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionBridge for CoreReadsBridge {
    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "create_session",
        })
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "resume_session",
        })
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.take_history_read()?;
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        if !self.delivers {
            return Err(BridgeError::Unsupported {
                operation: "deliver_admitted",
            });
        }
        // Core admits the input on the live member's current session.
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), _)) => {
                self.delivered
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(runtime_id.clone());
                Ok(session_id)
            }
            _ => Err(BridgeError::Mob(format!(
                "{runtime_id} is not a live member bound to a session"
            ))),
        }
    }

    async fn deliver_host_human_input(
        &self,
        runtime_id: &AgentRuntimeId,
        expected_session: &SessionId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        if !self.host_human_input {
            // The trait default's refusal, unchanged.
            return Err(BridgeError::HostHumanInput(
                HostHumanInputError::Unsupported(
                    "this session bridge has no local host-human input seam".to_string(),
                ),
            ));
        }
        // Core admits it on the live member's current session, or refuses.
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), _)) if session_id == *expected_session => {
                self.host_human_inputs.fetch_add(1, Ordering::SeqCst);
                Ok(session_id)
            }
            _ => Err(BridgeError::HostHumanInput(
                HostHumanInputError::BindingChanged,
            )),
        }
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "checkpoint_session",
        })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    /// Core releases the member's materialization: it stays seated, on the
    /// same session and generation, so its binding is left as it is. A
    /// release [`Self::refuse_release_of`] names is refused, retryably, and
    /// changes nothing.
    async fn release_member(&self, identity: &AgentIdentity) -> Result<(), BridgeError> {
        if self
            .release_refusals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity)
        {
            return Err(BridgeError::MemberReleaseRefused {
                identity: crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
                detail: format!(
                    "a lifecycle operation on {identity} is in progress: it is being woken or \
                     has work in flight"
                ),
            });
        }
        self.releases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(identity.clone());
        Ok(())
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        let Some(disposition) = self.registration_reload else {
            return Ok(None);
        };
        match self.core.live_incarnation(runtime_id) {
            Some((Some(session_id), generation)) => Ok(Some(BridgeMemberReload {
                disposition,
                session_id,
                registration_generation: generation,
            })),
            _ => Err(BridgeError::Mob(format!(
                "{runtime_id} is not a live member bound to a session"
            ))),
        }
    }
}
