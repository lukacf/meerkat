//! Session bridge: connects the identity-first control plane to the Meerkat
//! session pipeline for real session creation, delivery, and retirement.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::lifecycle::run_primitive::{
    OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
};
use meerkat_core::types::HandlingMode;
use meerkat_mob::ids::AgentIdentity as MobAgentIdentity;
use meerkat_mob::launch::MemberLaunchMode;
use meerkat_mob::{
    MobHandle, MobSessionService, ResumeOverrideField, SpawnContinuityIntent, SpawnMemberSpec,
    SpawnSystemPromptOverride, WorkOrigin, WorkRef, WorkSpec,
};

use crate::mob_handle_runtime::{
    content_input_has_images, is_previous_member_cleanup_ambiguous_error,
    is_recoverable_lifecycle_cleanup_error, model_capabilities_for_member_with_config,
    topology_restore_failed_peer_ids,
};

use crate::actor_loop_health::{ActorLoopHealth, ActorLoopHealthState};

use super::delivery_scope::{ScopeMismatch, ScopedDeliveryError, ScopedRetireError};
use super::types::{
    AgentBuildDraft, AgentIdentity, AgentRuntimeId, CheckpointVersion, ContinuityGeneration,
    DurableAgentSpec, FencingToken, RoleMigrationDeclaration, SessionSnapshot,
};

fn is_missing_event_injector_error(error: &str) -> bool {
    error.contains("missing event injector capability")
        || (error.contains("missing required capability")
            && error.contains("interaction_event_injector"))
}

fn is_missing_bridge_session_snapshot_error(error: &str) -> bool {
    error.contains("missing bridge session snapshot")
}

/// Core's typed "no durable session exists for this id"
/// (`SessionUnavailableForResume { reason: Absent }`), read as a variant
/// match. This bridge spawns nothing fresh on it: the resume and the delivery
/// repair both refuse it typed, and the caller decides whether the identity
/// wakes on a fresh session. Archived-but-intact, recovery-held, quarantined
/// and every unknown failure carry transcripts, and no wording change can
/// turn them into "absent". (The pre-0.8.9 form matched the prose "missing
/// durable session snapshot" by substring. Distinct from
/// [`is_missing_bridge_session_snapshot_error`], which is the
/// *delivery*-time revival wording and only routes INTO the repair attempt.)
///
/// Read through [`classify_resume_error`], so the shared retirement and
/// lifecycle wrappers a joined resume observer receives read here exactly as
/// they do in the refusal's kind: one predicate, not two that can disagree.
fn durable_snapshot_is_typed_absent(error: &meerkat_mob::MobError) -> bool {
    matches!(
        classify_resume_error(error),
        ResumeRejectionKind::Unavailable {
            reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
        }
    )
}

/// Whether a delivery failure is stale runtime state the deliver path repairs
/// (resume onto the same durable session, re-admitting queued inputs) before
/// retrying. Typed carriers are matched on the variant; the text classes
/// below cover meerkat refusals that are not typed yet.
fn is_repairable_bridge_delivery(error: &BridgeError) -> bool {
    match error {
        BridgeError::MemberRuntimeDetached { .. } => true,
        // A closed mob, one whose Shutdown is still draining, or a member
        // closed while its identity converges refused the delivery before
        // admission. No repair reopens it (a repair of a converging member
        // would retire the executor the convergence is replacing), and its
        // text never reaches the classes below.
        BridgeError::MobAdmissionClosed { .. }
        | BridgeError::MobShutdownInProgress { .. }
        | BridgeError::IdentityConvergenceAdmissionClosed { .. } => false,
        other => is_repairable_bridge_delivery_error(&other.to_string()),
    }
}

fn is_repairable_bridge_delivery_error(error: &str) -> bool {
    is_missing_event_injector_error(error)
        || is_missing_bridge_session_snapshot_error(error)
        || is_previous_member_cleanup_ambiguous_error(error)
}

/// meerkat-runtime's durability-degraded refusal, as rendered by meerkat
/// 0.8.33 (`RuntimeDriverError::RecoveryRepairBlocked` from
/// `mark_durability_reload_required`): the member's runtime shell can no
/// longer reconcile durable state with the live runtime after a failed
/// commit, and it refuses every ordinary input until a registration-authorized
/// cold reload replaces the registration.
///
/// Two stable fragments are matched because the reason reaches this bridge
/// flattened through `MobError`'s `Display` (0.8.33 has no typed variant;
/// 0.8.34 adds `MobError::MemberReloadRequired`, which
/// [`classify_submit_mob_error`] will match first): the error's own prefix and
/// the reload sentence `mark_durability_reload_required` composes. Neither
/// overlaps the repairable-stale-state vocabulary above, and the deliver path
/// checks this class BEFORE the repairable one, so a repair-blocked member is
/// never routed into `repair_member_for_delivery` (a destructive retire that
/// cannot clear a durability degradation and destroys queued inputs).
fn is_reload_required_bridge_delivery_error(error: &str) -> bool {
    error.contains("Runtime recovery is repair-blocked")
        || error.contains("registration-authorized cold reload is required")
        || error.contains("RecoveryRepairBlocked")
}

/// Type the submit-side `MobError` at the boundary instead of flattening it
/// to [`BridgeError::Mob`] text: a durability-degraded refusal becomes
/// [`BridgeError::ReloadRequired`] so the identity runtime can run exactly
/// one non-destructive reload and retry, a closed or shutting-down mob and a
/// member closed for identity convergence are typed so no repair ever reads
/// them, and everything else keeps the historical text form the substring
/// repair classifier reads.
fn classify_submit_mob_error(
    member_id: &MobAgentIdentity,
    error: meerkat_mob::MobError,
    deadline: &ActorAdmissionDeadline,
) -> BridgeError {
    match error {
        // meerkat's pre-admission carrier refusal for the member's actual
        // runtime mode (for example a completion-bearing admission to an
        // autonomous_host member). Typed, so a tracked delivery can fall back
        // to the ingress lane on the variant, never on its text.
        meerkat_mob::MobError::UnsupportedForMode { mode, reason } => {
            BridgeError::UnsupportedForMode {
                identity: member_id.clone(),
                mode,
                // meerkat's own rendering ("unsupported for runtime mode
                // {mode}: {reason}"), so the message is unchanged.
                detail: meerkat_mob::MobError::UnsupportedForMode { mode, reason }.to_string(),
            }
        }
        // meerkat 0.8.34: the durability-degraded refusal is typed at the mob
        // boundary. Matched BEFORE the text fallback so the reason text can
        // change without losing the class.
        meerkat_mob::MobError::MemberReloadRequired { member_id, reason } => {
            BridgeError::ReloadRequired {
                identity: member_id,
                reason,
            }
        }
        // meerkat 0.8.34: per-member single-flight lane is full. Retryable
        // backpressure for this one member; it says nothing about the loop.
        meerkat_mob::MobError::MemberAdmissionBacklogFull { member_id, depth } => {
            tracing::warn!(
                identity = %member_id,
                depth,
                "delivery refused: the member's admission lane is full; retry once it drains"
            );
            BridgeError::AdmissionBacklogFull {
                identity: member_id,
                depth,
            }
        }
        // meerkat (#1248): the member's session has no committed runtime
        // attachment. Typed, so the deliver path routes it into the resume
        // repair on the variant, never on its text.
        error @ meerkat_mob::MobError::MemberRuntimeDetached { .. } => {
            let meerkat_mob::MobError::MemberRuntimeDetached { session_id, .. } = &error else {
                unreachable!("matched above")
            };
            BridgeError::MemberRuntimeDetached {
                session_id: session_id.clone(),
                detail: error.to_string(),
            }
        }
        // A reply deadline can expire after runtime admission has started.
        // Preserve the observation, never infer execution fate from it.
        meerkat_mob::MobError::ActorCommandTimedOut {
            command_kind,
            stage,
        } => {
            tracing::warn!(
                identity = %member_id,
                command_kind,
                stage,
                "bounded actor command exceeded the delivery admission budget; execution fate \
                 is not established by this timeout"
            );
            BridgeError::ActorAdmissionTimeout {
                operation: "deliver.submit_work",
                identity: member_id.clone(),
                waited: deadline.started.elapsed(),
                command: Some(ActorCommandTimeout {
                    command_kind,
                    stage,
                }),
            }
        }
        // The mob's admission is closed, for any close reason: nothing was
        // delivered, and no repair or reload can reopen it.
        meerkat_mob::MobError::AdmissionClosed { mob_id, reason } => {
            let detail = meerkat_mob::MobError::AdmissionClosed {
                mob_id: mob_id.clone(),
                reason,
            }
            .to_string();
            BridgeError::MobAdmissionClosed {
                identity: member_id.clone(),
                mob_id,
                reason,
                detail,
            }
        }
        // A Shutdown of the mob is still draining: nothing was delivered, and
        // the actor is not gone (the Shutdown can still fail), so retry later.
        meerkat_mob::MobError::ShutdownInProgress { mob_id } => {
            let detail = meerkat_mob::MobError::ShutdownInProgress {
                mob_id: mob_id.clone(),
            }
            .to_string();
            BridgeError::MobShutdownInProgress {
                identity: member_id.clone(),
                mob_id,
                detail,
            }
        }
        // The member's desired material is committed and its replacement
        // executor is not active yet: meerkat keeps the member's admission
        // closed until the identity converges. Nothing was delivered; retry
        // once it has converged.
        meerkat_mob::MobError::IdentityConvergenceAdmissionClosed { member_id } => {
            let detail = meerkat_mob::MobError::IdentityConvergenceAdmissionClosed {
                member_id: member_id.clone(),
            }
            .to_string();
            BridgeError::IdentityConvergenceAdmissionClosed {
                identity: member_id,
                detail,
            }
        }
        other => {
            let rendered = other.to_string();
            if is_reload_required_bridge_delivery_error(&rendered) {
                return BridgeError::ReloadRequired {
                    identity: member_id.clone(),
                    reason: rendered,
                };
            }
            BridgeError::Mob(rendered)
        }
    }
}

/// The refusal a bridge without a scoped seam returns.
const SCOPED_DELIVERY_UNSUPPORTED: &str =
    "this session bridge has no scoped delivery seam (custom and remote bridges are unsupported)";

/// A scoped admission round trip that returned no answer. Only an actor call
/// the probe proves was never started is a refusal; everything else leaves
/// the admission fate unknown.
fn classify_scoped_bound_error(error: BridgeError) -> ScopedDeliveryError {
    match error {
        BridgeError::ActorLoopStalled {
            observation: ActorCallObservation::BeforeCall,
            ..
        }
        | BridgeError::ActorTerminated {
            observation: ActorCallObservation::BeforeCall,
            ..
        } => ScopedDeliveryError::Rejected {
            detail: error.to_string(),
        },
        error => ScopedDeliveryError::Uncertain {
            detail: error.to_string(),
        },
    }
}

/// Classify meerkat's answer to a scoped submit. A moved binding is a stale
/// scope; the listed refusals happen before admission; anything else is
/// conservatively uncertain, so a caller recovers instead of assuming the
/// delivery never landed.
fn classify_scoped_submit_mob_error(error: meerkat_mob::MobError) -> ScopedDeliveryError {
    use meerkat_mob::MobError;
    match error {
        // The member lost its session binding entirely (it became peer-only
        // or remotely hosted): it cannot be scope-bound at all.
        MobError::StaleDeliveryScope {
            actual_session: None,
            agent_identity,
            ..
        }
        | MobError::DeliveryScopeUnavailable { agent_identity, .. } => {
            ScopedDeliveryError::Unsupported {
                detail: format!(
                    "member '{agent_identity}': {}",
                    super::runtime::NO_NATIVE_SESSION_BINDING
                ),
            }
        }
        // A scope captured at another incarnation of the mob (one destroyed
        // and created again under its id) names a member that is gone for
        // good; meerkat refuses it before it reads or binds anything.
        error @ (MobError::StaleDeliveryScope { .. }
        | MobError::StaleFenceToken { .. }
        | MobError::IncarnationMismatch { .. }
        | MobError::MemberNotFound(_)) => ScopedDeliveryError::StaleScope {
            mismatch: ScopeMismatch::MemberBinding,
            detail: error.to_string(),
        },
        // A closed admission admits nothing, whatever closed it (an idle
        // close, the last owner's release, a stand-down, a shutdown, a
        // destroy or an actor exit), and neither does a member closed for
        // identity convergence: both are definite refusals.
        error @ (MobError::UnsupportedForMode { .. }
        | MobError::MemberReloadRequired { .. }
        | MobError::MemberAdmissionBacklogFull { .. }
        | MobError::MemberRuntimeDetached { .. }
        | MobError::NotExternallyAddressable(_)
        | MobError::InvalidTransition { .. }
        | MobError::ActorCommandChannelClosed
        | MobError::MemberRetirementInProgress { .. }
        | MobError::RetirementInProgress { .. }
        | MobError::AdmissionClosed { .. }
        | MobError::IdentityConvergenceAdmissionClosed { .. }) => ScopedDeliveryError::Rejected {
            detail: error.to_string(),
        },
        error => ScopedDeliveryError::Uncertain {
            detail: error.to_string(),
        },
    }
}

/// Classify meerkat's answer to a scoped retire. Another current
/// incarnation, the scope's incarnation under another session, or a scope
/// from another incarnation of the mob, is a stale scope (final for the
/// scope). The listed refusals retire nothing.
/// Anything else is the retirement's own failure.
fn classify_scoped_retire_mob_error(error: meerkat_mob::MobError) -> ScopedRetireError {
    use meerkat_mob::MobError;
    match error {
        error @ MobError::StaleFenceToken { .. } => ScopedRetireError::StaleScope {
            mismatch: ScopeMismatch::LeaseFencingToken,
            detail: error.to_string(),
        },
        error @ (MobError::StaleDeliveryScope { .. }
        | MobError::StaleMemberOperatorAuthority { .. }
        | MobError::IncarnationMismatch { .. }) => ScopedRetireError::StaleScope {
            mismatch: ScopeMismatch::MemberBinding,
            detail: error.to_string(),
        },
        error @ (MobError::InvalidTransition { .. }
        | MobError::ScopeDenied(_)
        | MobError::ActorCommandChannelClosed) => ScopedRetireError::Rejected {
            detail: error.to_string(),
        },
        error => ScopedRetireError::Retire(super::runtime::IdentityRuntimeError::Internal(
            format!("bridge scoped retire: {error}"),
        )),
    }
}

/// Type the reload-primitive `MobError` at the bridge boundary. `Refused`
/// (store not healthy yet, retryable later) and `TimedOut` (bounded by
/// meerkat's `MEMBER_RELOAD_TOTAL_TIMEOUT`, names the stage) keep their class;
/// everything else keeps the text form.
/// The session a typed resume hold names, read off the typed error itself
/// (`SessionError::WholeBlobAuditedEndpointDivergence { id }`, through the
/// shared failure wrappers) or off meerkat's structured `session_id` field
/// for wire-carried holds. `None` when the carrier names no session.
fn reload_hold_session_id(error: &meerkat_mob::MobError) -> Option<meerkat_core::types::SessionId> {
    match error {
        meerkat_mob::MobError::SessionError(
            meerkat_core::SessionError::WholeBlobAuditedEndpointDivergence { id },
        ) => Some(id.clone()),
        meerkat_mob::MobError::SharedRetirementFailure(inner)
        | meerkat_mob::MobError::SharedLifecycleFailure(inner) => reload_hold_session_id(inner),
        other => other
            .structured_data()
            .as_ref()
            .and_then(|data| data.get("session_id"))
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| meerkat_core::types::SessionId::parse(raw).ok()),
    }
}

/// Classify a member reload or ensure failure. `operation` names the actor
/// round trip the caller ran, so a timeout reports the verb that waited.
fn classify_reload_mob_error(
    operation: &'static str,
    member_id: &MobAgentIdentity,
    error: meerkat_mob::MobError,
    deadline: &ActorAdmissionDeadline,
) -> BridgeError {
    // A typed audited-endpoint divergence is not "the store is not healthy
    // yet": it is a stable verdict only the operator's repair clears. Read it
    // typed before the retryable refusal arm so the reload verb parks the
    // member instead of advertising a retry that can never succeed.
    if durable_resume_hold_of(&error)
        == Some(meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence)
        && let Some(session_id) = reload_hold_session_id(&error)
    {
        tracing::error!(
            identity = %member_id,
            session_id = %session_id,
            error = %error,
            "member reload refused: the durable session needs the sanctioned \
             audited-endpoint repair; registration retained; parking typed instead of retrying"
        );
        return BridgeError::SessionRepairRequired {
            identity: member_id.clone(),
            session_id,
            detail: error.to_string(),
        };
    }
    match error {
        meerkat_mob::MobError::ActorCommandTimedOut {
            command_kind,
            stage,
        } => BridgeError::ActorAdmissionTimeout {
            operation,
            identity: member_id.clone(),
            waited: deadline.started.elapsed(),
            command: Some(ActorCommandTimeout {
                command_kind,
                stage,
            }),
        },
        meerkat_mob::MobError::MemberReloadRefused { session_id, reason } => {
            tracing::warn!(
                identity = %member_id,
                session_id = %session_id,
                reason = %reason,
                "member reload refused: the store is not healthy yet; registration retained; \
                 retry later"
            );
            BridgeError::ReloadRefused {
                identity: member_id.clone(),
                session_id: session_id.to_string(),
                reason,
            }
        }
        meerkat_mob::MobError::MemberReloadTimedOut { session_id, stage } => {
            tracing::error!(
                identity = %member_id,
                session_id = %session_id,
                stage = %stage,
                "member reload timed out (MEMBER_RELOAD_TOTAL_TIMEOUT); inspect before retrying"
            );
            BridgeError::ReloadTimedOut {
                identity: member_id.clone(),
                session_id: session_id.to_string(),
                stage: stage.to_string(),
            }
        }
        other => BridgeError::Mob(format!("reload_member_registration: {other}")),
    }
}

/// A missing degradation marker is not a positive readiness observation:
/// upstream also returns `None` for an absent registration.
pub(crate) fn member_durability_from_machine(
    required: Option<meerkat_runtime::SessionDurabilityReloadRequired>,
) -> Option<super::types::MemberDurability> {
    match required {
        None => None,
        // `SessionDurabilityReloadRequired` is `#[non_exhaustive]`: read the
        // named fields only, never destructure or construct it.
        Some(required) => Some(member_durability_degraded(
            required.operation,
            required.reason,
        )),
    }
}

/// The degraded verdict in identity-first vocabulary.
fn member_durability_degraded(operation: String, reason: String) -> super::types::MemberDurability {
    super::types::MemberDurability::ReloadRequired { operation, reason }
}

/// What the mob primitive did to a member's runtime registration, projected
/// into identity-first vocabulary. `session_id`/`registration_generation` are
/// meerkat's registration-level values, reported for the operator log; the
/// identity's continuity binding is authoritative and unchanged by a reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeMemberReload {
    pub disposition: super::types::MemberReloadDisposition,
    pub session_id: meerkat_core::types::SessionId,
    pub registration_generation: u64,
}

fn bridge_member_reload(outcome: meerkat_mob::MemberReloadOutcome) -> BridgeMemberReload {
    let disposition = match outcome.disposition {
        meerkat_mob::MemberReloadDisposition::Discarded => {
            super::types::MemberReloadDisposition::Discarded
        }
        meerkat_mob::MemberReloadDisposition::NotDegraded => {
            super::types::MemberReloadDisposition::NotDegraded
        }
        meerkat_mob::MemberReloadDisposition::NotCurrent => {
            super::types::MemberReloadDisposition::NotCurrent
        }
        // meerkat (#1248): the registration had lost its runtime attachment;
        // the reload completed the pending unregister and re-attached the
        // same session. A real replacement of the live registration.
        meerkat_mob::MemberReloadDisposition::Reattached => {
            super::types::MemberReloadDisposition::Reattached
        }
    };
    BridgeMemberReload {
        disposition,
        session_id: outcome.session_id,
        registration_generation: outcome.generation.get(),
    }
}

fn is_recoverable_bridge_respawn_cleanup_error(error: &str) -> bool {
    is_recoverable_lifecycle_cleanup_error(error)
}

fn is_member_already_exists_error(error: &meerkat_mob::MobError) -> bool {
    matches!(error, meerkat_mob::MobError::MemberAlreadyExists(_))
}

/// What one read of Core's identity intent row says about who manages an
/// identity.
///
/// Meta writes no Core identity intent row, so a row Core holds for an
/// identity is the host's: the host adopted the identity and owns its
/// materialization.
#[derive(Debug, Clone)]
enum IdentityIntentRead {
    /// No row: nothing but this bridge's caller manages the identity, and the
    /// legacy path runs.
    Missing,
    /// A valid row, Present or Absent: the host adopted the identity.
    HostAdopted,
    /// An unsupported or malformed row, or a failed read. It says nothing
    /// about who manages the identity, so it is no clearance either way: the
    /// caller refuses retryable.
    Unreadable(String),
}

/// Poll cadence while a resume waits for a spawn in custody to converge.
const CONVERGENCE_HANDOFF_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Cadence of the non-destructive re-offer while a resume waits on a spawn
/// that is still in asynchronous custody for the same member.
///
/// A pending spawn that FAILS leaves nothing observable outside the actor: no
/// roster member, no session binding. Re-offering the resume at this cadence
/// is the only way to notice that the custody ended empty, and it is safe:
/// `MemberAlreadyExists` means still occupied, success means this call became
/// the materialization. The wait as a whole is bounded by the bridge's actor
/// admission budget, the one budget every actor round trip already lives under.
const IN_FLIGHT_SPAWN_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// What the roster says about the occupant a resume just collided with.
///
/// `MemberAlreadyExists` is raised by two different guards: the roster itself,
/// and MobMachine's member-admission probe, which also refuses a duplicate
/// while a spawn for the identity is still in asynchronous custody. Only the
/// first names something that can be stale. The second is a build in progress
/// that the roster does not show yet, so "collision without a roster member"
/// is the observable signature of an in-flight spawn. A 2026-09-22 production incident (item 3)
/// read that signature as a stale occupant: the retire was inert, the resume
/// retry collided with the build that had just finished, and the identity was
/// marked Broken over a race that needed 250 ms of patience.
#[derive(Debug)]
pub(crate) enum CollidingOccupant {
    /// The roster holds a committed member under this identity. The
    /// preconditioned retire path decides what happens to it.
    Committed,
    /// No roster member: the collision came from a spawn still in custody (or
    /// the projection window just before the roster insert). Convergence to
    /// await, never debris to clear.
    InFlight,
    /// The roster read itself failed. Nothing may be destroyed on that basis.
    Unobservable(meerkat_mob::MobError),
}

/// Pure mapping from a roster probe to [`CollidingOccupant`]; generic over the
/// entry so the mapping is testable without manufacturing a roster row.
fn classify_colliding_occupant<T>(
    probe: Result<Option<T>, meerkat_mob::MobError>,
) -> CollidingOccupant {
    match probe {
        Ok(Some(_)) => CollidingOccupant::Committed,
        Ok(None) => CollidingOccupant::InFlight,
        Err(error) => CollidingOccupant::Unobservable(error),
    }
}

/// Retire a reset's superseded member through Core. A member Core no longer
/// holds is already retired.
///
/// Without a continuity session copy this is the whole step: Core holds the
/// only copy of the superseded session and its retire archives it, so there
/// is no Meta projection to abandon first.
async fn retire_reset_superseded<Retire, RetireFuture>(
    member_id: &MobAgentIdentity,
    session_id: &meerkat_core::types::SessionId,
    retire: Retire,
) -> Result<(), BridgeError>
where
    Retire: FnOnce() -> RetireFuture,
    RetireFuture: Future<Output = Result<(), meerkat_mob::MobError>>,
{
    match retire().await {
        Ok(()) | Err(meerkat_mob::MobError::MemberNotFound(_)) => Ok(()),
        Err(error) => Err(BridgeError::Mob(format!(
            "reset retire cleanup failed for superseded member {member_id} \
             (session {session_id}): {error}"
        ))),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MemberRepairRespawnFailure {
    DegradedTopologyRestore { failed_peer_ids: Vec<String> },
    RecoverableCleanup,
    Fatal(String),
}

fn classify_member_repair_respawn_failure(
    error: &meerkat_mob::MobRespawnError,
) -> MemberRepairRespawnFailure {
    if let Some(failed_peer_ids) = topology_restore_failed_peer_ids(error) {
        return MemberRepairRespawnFailure::DegradedTopologyRestore { failed_peer_ids };
    }
    if is_recoverable_bridge_respawn_cleanup_error(&error.to_string()) {
        return MemberRepairRespawnFailure::RecoverableCleanup;
    }
    MemberRepairRespawnFailure::Fatal(error.to_string())
}

/// Rebuild spec for the delivery-repair fresh spawn: the SAME member
/// identity with its pre-delivery role and labels, `Fresh` launch mode.
/// Reached only from the recoverable-cleanup arm of the legacy respawn
/// ladder in `repair_member_for_delivery`, gated on verified roster absence.
fn fresh_member_spec_from_pre_delivery_entry(
    member_id: &MobAgentIdentity,
    role: meerkat_mob::ProfileName,
    labels: BTreeMap<String, String>,
) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(role, member_id.clone());
    spec.continuity_intent = repair_continuity_intent(&labels);
    if !labels.is_empty() {
        spec = spec.with_labels(labels);
    }
    spec
}

/// The durable-identity marker a delivery repair carries over to the member
/// it rebuilds. Every identity-first spawn labels its member with the
/// identity (`agent_identity`, see [`build_spawn_spec`]), and the rebuilt
/// member must stay in Core's durable identity member read exactly like the
/// member it replaces; a member without the label was never marked.
fn repair_continuity_intent(labels: &BTreeMap<String, String>) -> SpawnContinuityIntent {
    match labels.get("agent_identity") {
        Some(identity) => SpawnContinuityIntent::DurableIdentity {
            continuity_key: identity.clone(),
        },
        None => SpawnContinuityIntent::Ephemeral,
    }
}

// ---------------------------------------------------------------------------
// BridgeError
// ---------------------------------------------------------------------------

/// Upstream actor-command deadline observations, not an execution verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ActorCommandTimeout {
    pub command_kind: &'static str,
    pub stage: &'static str,
}

pub(crate) fn actor_timeout_data(
    operation: &'static str,
    waited: Duration,
    command: Option<ActorCommandTimeout>,
) -> serde_json::Value {
    match command {
        Some(command) => serde_json::json!({
            "kind": "mob_actor_command_timed_out",
            "command_kind": command.command_kind,
            "stage": command.stage,
            "deadline_reached": true,
        }),
        None => serde_json::json!({
            "kind": "actor_admission_timeout",
            "operation": operation,
            "waited_ms": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
            "deadline_reached": true,
        }),
    }
}

pub(crate) fn fmt_actor_timeout(
    f: &mut std::fmt::Formatter<'_>,
    operation: &str,
    identity: &dyn std::fmt::Display,
    waited: Duration,
    command: Option<ActorCommandTimeout>,
) -> std::fmt::Result {
    write!(
        f,
        "actor call `{operation}` for {identity} did not answer within the admission budget \
         ({waited:?})"
    )?;
    if let Some(command) = command {
        write!(
            f,
            "; command `{}` at stage `{}`",
            command.command_kind, command.stage
        )?;
    }
    f.write_str(
        "; execution fate and stall scope are not established by this timeout; \
         consult runtime idempotency and durable fate before retrying",
    )
}

/// Whether the probe refused an unpolled call or interrupted its observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorCallObservation {
    BeforeCall,
    InFlight,
}

impl std::fmt::Display for ActorCallObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BeforeCall => "this actor call was not started",
            Self::InFlight => {
                "observation of an in-flight actor call ended; execution fate is not established"
            }
        })
    }
}

/// Errors from session bridge operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum BridgeError {
    HostHumanInput(HostHumanInputError),
    /// The underlying mob operation failed.
    Mob(String),
    /// meerkat refused the delivery's carriers for the member's ACTUAL runtime
    /// mode, before admission, so nothing was delivered
    /// (`MobError::UnsupportedForMode`, read typed). `mode` is meerkat's, from
    /// the member's roster entry, and can differ from the mode Meta resolves
    /// from the current profile when that profile changed without a respawn.
    /// `detail` is meerkat's rendering, shown exactly as the untyped mob error
    /// was.
    UnsupportedForMode {
        identity: MobAgentIdentity,
        mode: meerkat_mob::MobRuntimeMode,
        detail: String,
    },
    /// A required field was missing or invalid.
    InvalidInput(String),
    /// Core refused the resume, typed. `session_id` is the durable session the
    /// resume asked Core for. Only `Unavailable { reason: Absent }` (Core holds
    /// no durable session under that id) leaves nothing to keep: the caller
    /// decides whether the identity wakes on a fresh session in its place,
    /// and names the replaced session when it does. Every other kind keeps
    /// the identity -> session binding intact, because the durable transcript
    /// is the only copy of the conversation.
    ResumeRejected {
        kind: ResumeRejectionKind,
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
    /// The member's durable session needs the sanctioned audited-endpoint
    /// repair before any resume or reload can read it (meerkat's typed
    /// `SessionError::WholeBlobAuditedEndpointDivergence`, read typed off the
    /// mob error). Raised by the registration reload path; the resume path
    /// raises the same fact as `ResumeRejected { kind: Held { hold:
    /// AuditedEndpointDivergence } }`. The registration and the durable session
    /// are retained unchanged; nothing is retryable until the operator runs
    /// the repair CLI and then `meta/reload_member`.
    SessionRepairRequired {
        identity: MobAgentIdentity,
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
    /// Provider authentication refused the member build. meerkat resolves the
    /// profile's credential at build time and fails typed
    /// (`SessionError::provider_auth_failure`, carried across the
    /// session-builder boundary as secret-free structured data), so the
    /// bridge reads it typed here instead of flattening it into
    /// [`Self::Mob`] text. `failure` names the `AuthErrorKind`, provider and
    /// binding; the identity runtime decides from the KIND whether an
    /// unattended retry can ever succeed (a missing secret cannot change
    /// without operator action, an expired lease can) and parks the identity
    /// with the reason instead of heal-looping. `detail` is the bridge step
    /// plus meerkat's public message, for operators. On the resume door this
    /// replaces [`Self::ResumeRejected`]; the durable session is preserved
    /// exactly as for any other rejected resume.
    ProviderAuthRejected {
        failure: meerkat_core::service::SessionProviderAuthFailure,
        detail: String,
    },
    /// Another runtime owner hosts the session (meerkat's typed
    /// `SessionError::ServedElsewhere`): it holds the session's hosting
    /// claim, so this owner neither attaches the session nor writes its
    /// durable state. A hold, not a fault: it clears when that owner releases
    /// the session. Retry later, with backoff; nothing here retries it.
    ServedElsewhere {
        session_id: meerkat_core::types::SessionId,
    },
    /// The session's cross-process hosting claim cannot be taken for a
    /// reason other than another holder (meerkat's typed
    /// `SessionError::HostingUnavailable`). Nothing was attached or written
    /// without the claim. A hold, not a fault: it clears once the claim can
    /// be taken. Retry later, with backoff; nothing here retries it.
    HostingUnavailable {
        session_id: meerkat_core::types::SessionId,
    },
    /// A mob-actor round trip in the delivery path did not answer within the
    /// attempt's admission budget. meerkat's mob actor is ONE serialized
    /// command loop and `MobHandle::send_actor_command` has no timeout of its
    /// own, so a blocked handler CAN freeze every member's dispatch — but one
    /// expired budget cannot establish that scope. Production 2026-08-14: this
    /// fired while two other members demonstrably drained (the blocked wait
    /// was in a spawned admission task / the member's own runtime-loop layer,
    /// not the shared loop), and the old "loop is not draining" wording sent
    /// the whole incident after a fleet-wide wedge that was not happening.
    /// The observation is exactly: THIS round trip, for THIS member, was not
    /// answered in time. Loop-scope verdicts belong to the ActorLoopStalled
    /// probe, whose timed heartbeat is the only observation that can honestly
    /// make them. This bounds observation, not execution: delivery may have
    /// started. Never treat it as a repairable stale-runtime-state error:
    /// repairing a member cannot unblock whatever the wait was parked on.
    ActorAdmissionTimeout {
        /// Which actor round trip expired, e.g. `deliver.submit_work`.
        operation: &'static str,
        /// The mob member the round trip was addressed to.
        identity: MobAgentIdentity,
        /// Total time this delivery attempt spent waiting on the actor.
        waited: Duration,
        /// Exact upstream command/stage when reported; absent for a local
        /// observation deadline, which supplies no upstream stage evidence.
        command: Option<ActorCommandTimeout>,
    },
    /// The actor-loop probe has an OPEN stall, so this delivery was refused
    /// before the call or while its observation was in flight. The latter
    /// does not retract a command. Circuit breaker, not a timeout: the caller waited at most
    /// until the stall opened, never the admission budget (a 2026-09-04 production incident
    /// waited 600 s per send behind a stall the probe had already paged).
    /// `stall_id` is the `ErrorEvent::ActorLoopStalled` correlation id, so the
    /// refusal joins to the open incident. Deliveries resume on the correlated
    /// `ActorLoopRecovered`. Never repairable: repairing a member cannot
    /// unblock the shared loop.
    ActorLoopStalled {
        /// Which actor round trip was refused, e.g. `deliver.submit_work`.
        operation: &'static str,
        /// The mob member the round trip was addressed to.
        identity: MobAgentIdentity,
        /// The open stall's id (`ErrorEvent::ActorLoopStalled { stall_id }`).
        stall_id: u64,
        /// How long the stall had been open when this delivery was refused.
        stalled_for: Duration,
        observation: ActorCallObservation,
    },
    /// The actor-loop probe observed the actor's channels CLOSED: the loop is
    /// gone and nothing in this process can restart it. Every delivery fails
    /// with this until the process restarts. Distinct from
    /// [`Self::ActorLoopStalled`] so an operator never waits for a recovery
    /// that cannot come.
    ActorTerminated {
        operation: &'static str,
        identity: MobAgentIdentity,
        /// The channel-closed evidence the probe observed.
        detail: String,
        observation: ActorCallObservation,
    },
    /// meerkat refused admission because the member's runtime registration is
    /// durability-degraded and requires a registration-authorized cold reload
    /// (`RecoveryRepairBlocked` / `MemberReloadRequired`). Typed so the
    /// identity runtime runs exactly ONE non-destructive `reload_member` and
    /// retries, then fails typed. Never routed to `repair_member_for_delivery`:
    /// that is a destructive retire that cannot clear a durability degradation
    /// and destroys the member's queued inputs.
    ReloadRequired {
        identity: MobAgentIdentity,
        /// meerkat's reason text, verbatim.
        reason: String,
    },
    /// meerkat 0.8.34 refused the delivery before dispatch because this
    /// member's per-member admission lane already holds its maximum number of
    /// parked deliveries (`MEMBER_ADMISSION_LANE_CAPACITY`). Backpressure for
    /// exactly one member, never a verdict on the mob or the loop: RETRYABLE
    /// once the lane drains. Never repair (the member is healthy, just busy),
    /// never reload, never `Mob(String)`.
    AdmissionBacklogFull {
        identity: MobAgentIdentity,
        /// Deliveries already parked behind the member's in-flight admission.
        depth: usize,
    },
    /// meerkat refused dispatch because the member's session has no committed
    /// runtime attachment (typed `MobError::MemberRuntimeDetached`, for
    /// example a runtime-loop teardown whose unregister never completed).
    /// Repairable stale runtime state: the deliver path resume-repairs the
    /// member onto the same durable session, re-admitting its queued inputs,
    /// and retries. `detail` is meerkat's rendering.
    MemberRuntimeDetached {
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
    /// meerkat 0.8.34 REFUSED a member registration reload because the durable
    /// resume authority is still unreadable: the store is not healthy yet.
    /// The degraded registration and live shell are retained and the next
    /// send stays reload-required. RETRYABLE once the store is healthy; a
    /// reload attempted while commits still fail would only re-degrade the
    /// fresh registration, so the refusal is the correct answer.
    ReloadRefused {
        identity: MobAgentIdentity,
        session_id: String,
        /// meerkat's reason, verbatim (names the unreadable authority).
        reason: String,
    },
    /// meerkat 0.8.34's member registration reload exceeded
    /// `MEMBER_RELOAD_TOTAL_TIMEOUT` (45 s) at `stage`
    /// (`durability_reload_discard`, `live_session_revival`, or
    /// `runtime_readiness`). NOT retryable without inspection: the reload may
    /// have partially progressed, and the stage says where to look.
    ReloadTimedOut {
        identity: MobAgentIdentity,
        session_id: String,
        stage: String,
    },
    /// The work WAS admitted and its turn reached a FAILED terminal.
    ///
    /// Distinct from [`Self::Mob`] on purpose: this names an outcome, not an
    /// admission problem. The turn ran. Never retry on it - a retry would run
    /// the member's turn a second time - and never confuse it with the delivery
    /// having failed to start.
    CompletionFailed(String),
    /// The work WAS admitted, its turn reached a SUCCESSFUL terminal, and only
    /// then did resolving the runtime's session id fail.
    ///
    /// The member did the work. Nothing about this is an admission problem, so
    /// it must never be routed into repair-and-retry: repairing the member
    /// cannot undo a turn that already ran, and resubmitting would run it
    /// twice. Non-retryable by construction.
    ///
    /// Distinct from [`Self::CompletionFailed`] because the outcomes differ for
    /// an operator: there, the turn failed; here, the turn succeeded and only
    /// the post-hoc projection of its session id did not. The resolution detail
    /// is carried verbatim and is deliberately NOT reclassified - this is a
    /// distinct variant, and the delivery path's substring classifier only ever
    /// sees admission errors, never this one.
    PostAdmissionResolutionFailed(String),
    /// This bridge does not implement completion-bearing delivery.
    ///
    /// The completion-bearing methods default to this so that every existing
    /// [`SessionBridge`] implementor keeps compiling unchanged; only the
    /// concrete mob bridge, which can reach meerkat's `WorkTurnHandle`,
    /// overrides them.
    ///
    /// Returned BEFORE any submission, so a caller that sees it knows nothing
    /// was delivered. **A caller that needs completion must fail closed here.**
    /// Do NOT fall back to the ingress-only `deliver_*` methods: those return
    /// at admission, so falling back would report success for a turn whose
    /// outcome is unknown - which is precisely the false-success this seam
    /// exists to remove.
    CompletionUnsupported(String),
    /// This bridge does not implement `operation`. The trait's default for a
    /// read or verb a custom bridge has not implemented, so a caller never
    /// takes a missing seam for an empty answer.
    Unsupported {
        operation: &'static str,
    },
    /// Core holds an identity intent row for `identity`. Meta writes none, so
    /// the row is the host's: the host adopted the identity and owns its
    /// materialization. Refused before this bridge attached, ensured or
    /// retired anything; nothing changed. No retry of the same call succeeds
    /// while the host's row stands.
    HostAdopted {
        identity: AgentIdentity,
    },
    /// Core's incarnation for `identity` changed while this bridge read the
    /// member, twice in a row: a respawn landed between the incarnation read
    /// and the session read each time, so no consistent pair was read and
    /// nothing was recorded. Retryable once the member settles.
    LiveMemberMoved {
        identity: AgentIdentity,
    },
    /// The placement host refused to materialize the member because its
    /// realm backend is not persistent while the spawn needs durable
    /// sessions (meerkat's typed `BridgeRejectionCause::RealmBackendUnavailable`,
    /// read off the mob error, never from its text). Every identity-first
    /// spawn needs durable sessions, so no retry changes this without operator
    /// action. `detail` is the bridge step plus meerkat's message.
    RealmBackendUnavailable {
        detail: String,
    },
    /// meerkat refused the spawn because `profile` declares `durable = true`
    /// while the mob's member sessions do not outlive the process (its
    /// typed `MobError::NonDurableService`, read off the mob error, never
    /// from its text). Refused before anything was created, and no retry
    /// changes it until the mob runs on a persistent realm or the profile
    /// drops `durable`. `detail` is the bridge step plus meerkat's message.
    NonDurableService {
        profile: meerkat_mob::ProfileName,
        detail: String,
    },
    /// meerkat refused the delivery because the mob's admission is closed
    /// (`MobError::AdmissionClosed`, read typed), for any close reason: an
    /// idle close, the last owner's release, a stand-down, a Shutdown, a
    /// destroy, a durability fail-stop, a lost actor or a failed dispatch.
    /// Refused before admission, so nothing was delivered. Never repaired (a
    /// repair cannot reopen a closed mob) and never reloaded. `detail` is
    /// meerkat's rendering, shown exactly as the untyped mob error was.
    MobAdmissionClosed {
        identity: MobAgentIdentity,
        mob_id: meerkat_mob::MobId,
        reason: meerkat_mob::MobAdmissionCloseReason,
        detail: String,
    },
    /// meerkat refused the delivery because a Shutdown of the mob is still
    /// draining (`MobError::ShutdownInProgress`, read typed). Nothing was
    /// delivered. The actor is not gone: the Shutdown can still fail and
    /// leave it serving, so this is a retry-later refusal, never a terminal
    /// claim. Never repaired and never reloaded. `detail` is meerkat's
    /// rendering.
    MobShutdownInProgress {
        identity: MobAgentIdentity,
        mob_id: meerkat_mob::MobId,
        detail: String,
    },
    /// meerkat refused the delivery because the member's admission is closed
    /// while its identity converges
    /// (`MobError::IdentityConvergenceAdmissionClosed`, read typed): its
    /// desired material is committed and its replacement executor is not
    /// active yet. Nothing was delivered. A retry-later refusal for this one
    /// member: never repaired (a repair would retire the executor the
    /// convergence is replacing) and never reloaded. `detail` is meerkat's
    /// rendering.
    IdentityConvergenceAdmissionClosed {
        identity: MobAgentIdentity,
        detail: String,
    },
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostHumanInput(error) => write!(f, "{error}"),
            Self::Mob(msg)
            | Self::UnsupportedForMode { detail: msg, .. }
            | Self::MobAdmissionClosed { detail: msg, .. }
            | Self::MobShutdownInProgress { detail: msg, .. }
            | Self::IdentityConvergenceAdmissionClosed { detail: msg, .. } => {
                write!(f, "session bridge mob error: {msg}")
            }
            Self::CompletionFailed(msg) => write!(
                f,
                "session bridge turn was admitted and then failed: {msg}; the turn RAN - \
                 do not retry"
            ),
            Self::PostAdmissionResolutionFailed(msg) => write!(
                f,
                "session bridge turn was admitted and COMPLETED, then resolving its runtime \
                 session id failed: {msg}; the turn RAN - do not retry"
            ),
            Self::CompletionUnsupported(msg) => write!(
                f,
                "session bridge does not support completion-bearing delivery: {msg}; \
                 nothing was submitted"
            ),
            Self::Unsupported { operation } => {
                write!(f, "this session bridge does not implement {operation}")
            }
            Self::InvalidInput(msg) => write!(f, "session bridge invalid input: {msg}"),
            Self::ResumeRejected {
                kind,
                session_id,
                detail,
            } => write!(
                f,
                "session bridge resume of session {session_id} rejected ({kind:?}): {detail}"
            ),
            Self::SessionRepairRequired {
                identity,
                session_id,
                detail,
            } => write!(
                f,
                "session bridge: member {identity} session {session_id} needs the sanctioned \
                 audited-endpoint repair before it can be resumed or reloaded (every message is \
                 intact; no retry can change this): {detail}"
            ),
            Self::ProviderAuthRejected { failure, detail } => write!(
                f,
                "session bridge member build refused by provider authentication (kind={}, \
                 provider={}): {detail}",
                failure.kind.as_str(),
                failure.provider.as_str()
            ),
            Self::ServedElsewhere { session_id } => write!(
                f,
                "session bridge: session {session_id} is served by another runtime owner; this \
                 clears when that owner releases it, retry later"
            ),
            Self::HostingUnavailable { session_id } => write!(
                f,
                "session bridge: session {session_id} cannot be hosted, its hosting claim is \
                 unavailable; this clears once the claim can be taken, retry later"
            ),
            Self::RealmBackendUnavailable { detail } => write!(
                f,
                "session bridge member build refused: the placement host's realm backend is \
                 not persistent and the member needs durable sessions: {detail}"
            ),
            Self::NonDurableService { profile, detail } => write!(
                f,
                "session bridge member build refused: profile '{profile}' declares durable \
                 workers and the mob's member sessions do not outlive the process: {detail}"
            ),
            Self::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            } => fmt_actor_timeout(f, operation, identity, *waited, *command),
            Self::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            } => write!(
                f,
                "session bridge actor call `{operation}` for member {identity}: the mob \
                 actor loop has an open stall (stall_id {stall_id}, open for {stalled_for:?}); \
                 {observation}. Actor recovery is reported with stall_id {stall_id}; \
                 inspect runtime fate before retrying an in-flight call"
            ),
            Self::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            } => write!(
                f,
                "session bridge actor call `{operation}` for member {identity}: the mob \
                 actor loop TERMINATED ({detail}); this is not a stall and will not recover, \
                 the process must restart; {observation}"
            ),
            Self::ReloadRequired { identity, reason } => write!(
                f,
                "session bridge delivery to member {identity} refused: the member's runtime \
                 registration requires a registration-authorized cold reload \
                 (meta/reload_member; non-destructive, same session and generation): {reason}"
            ),
            Self::MemberRuntimeDetached { detail, .. } => {
                write!(f, "session bridge mob error: {detail}")
            }
            Self::AdmissionBacklogFull { identity, depth } => write!(
                f,
                "session bridge delivery to member {identity} refused: the member's admission \
                 lane is full ({depth} deliveries parked behind its in-flight admission); \
                 nothing was queued; retry once the lane drains (this is per-member \
                 backpressure, not a stalled loop)"
            ),
            Self::ReloadRefused {
                identity,
                session_id,
                reason,
            } => write!(
                f,
                "session bridge reload of member {identity} (session {session_id}) refused: the \
                 store is not healthy, the durable resume authority is still unreadable; the \
                 registration was retained unchanged; retry later once the store is healthy: \
                 {reason}"
            ),
            Self::ReloadTimedOut {
                identity,
                session_id,
                stage,
            } => write!(
                f,
                "session bridge reload of member {identity} (session {session_id}) timed out at \
                 stage `{stage}` (MEMBER_RELOAD_TOTAL_TIMEOUT); the reload may have partially \
                 progressed; inspect the member before retrying"
            ),
            Self::HostAdopted { identity } => write!(
                f,
                "session bridge: identity {identity} is host-adopted (Core holds the host's \
                 identity intent row for it), so this bridge does not ensure, attach or retire \
                 its member; nothing was changed"
            ),
            Self::LiveMemberMoved { identity } => write!(
                f,
                "session bridge: the member of {identity} moved to a new incarnation while it \
                 was being read, twice in a row; nothing was recorded (retryable once it settles)"
            ),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Errors that can occur before a completion-bearing delivery has produced an
/// admitted-turn receipt.
///
/// This enum deliberately cannot represent a terminal failure or a
/// post-admission projection failure. Those states exist only in
/// [`BridgeTurnError`], returned by [`BridgeTurnReceipt::wait`]. Keeping the
/// two transition errors disjoint avoids inventing a terminal outcome when
/// no receipt was observed. A missing receipt does not prove nonexecution.
#[derive(Debug)]
pub enum BridgeAdmissionError {
    HostHumanInput(HostHumanInputError),
    GovernedInput(GovernedInputError),
    /// The bridge cannot create completion-bearing receipts. Nothing was
    /// submitted.
    CompletionUnsupported(String),
    /// The underlying mob operation failed before a receipt was observed.
    Mob(String),
    /// meerkat refused the delivery's carriers for the member's ACTUAL runtime
    /// mode, before admission, so nothing was delivered
    /// (`MobError::UnsupportedForMode`, read typed). `mode` is meerkat's, from
    /// the member's roster entry, and can differ from the mode Meta resolves
    /// from the current profile when that profile changed without a respawn.
    /// `detail` is meerkat's rendering, shown exactly as the untyped mob error
    /// was.
    UnsupportedForMode {
        identity: MobAgentIdentity,
        mode: meerkat_mob::MobRuntimeMode,
        detail: String,
    },
    /// A required field was missing or invalid before admission.
    InvalidInput(String),
    /// Resume was rejected before the delivery could be admitted.
    ResumeRejected {
        kind: ResumeRejectionKind,
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
    /// Provider authentication refused the member build before admission.
    /// Carries meerkat's typed secret-free failure so the round trip back to
    /// [`BridgeError::ProviderAuthRejected`] is lossless and the
    /// deterministic-rejection classifier still reads the kind.
    ProviderAuthRejected {
        failure: meerkat_core::service::SessionProviderAuthFailure,
        detail: String,
    },
    /// The serialized actor admission round trip exceeded its budget.
    ActorAdmissionTimeout {
        operation: &'static str,
        identity: MobAgentIdentity,
        waited: Duration,
        command: Option<ActorCommandTimeout>,
    },
    /// The probe observed a stall; `observation` distinguishes early refusal
    /// from an interrupted in-flight observation.
    ActorLoopStalled {
        operation: &'static str,
        identity: MobAgentIdentity,
        stall_id: u64,
        stalled_for: Duration,
        observation: ActorCallObservation,
    },
    /// The probe observed termination; an in-flight call's fate is unknown.
    ActorTerminated {
        operation: &'static str,
        identity: MobAgentIdentity,
        detail: String,
        observation: ActorCallObservation,
    },
    /// Refused before admission: the member needs a cold reload.
    ReloadRequired {
        identity: MobAgentIdentity,
        reason: String,
    },
    /// Refused before admission: the member's admission lane is full
    /// (retryable per-member backpressure).
    AdmissionBacklogFull {
        identity: MobAgentIdentity,
        depth: usize,
    },
    /// The automatic reload before admission was refused: store not healthy
    /// yet (retryable later).
    ReloadRefused {
        identity: MobAgentIdentity,
        session_id: String,
        reason: String,
    },
    /// The automatic reload before admission timed out at `stage`.
    ReloadTimedOut {
        identity: MobAgentIdentity,
        session_id: String,
        stage: String,
    },
    /// A legacy bridge path produced a post-admission error before a receipt
    /// existed. This is an implementation invariant failure, not a terminal
    /// outcome, and no turn may be claimed to have run from it.
    InvariantViolation(String),
    /// Refused before admission: the mob's admission is closed, for any
    /// close reason. Nothing was delivered.
    MobAdmissionClosed {
        identity: MobAgentIdentity,
        mob_id: meerkat_mob::MobId,
        reason: meerkat_mob::MobAdmissionCloseReason,
        detail: String,
    },
    /// Refused before admission: a Shutdown of the mob is still draining
    /// (retry later; the actor is not gone). Nothing was delivered.
    MobShutdownInProgress {
        identity: MobAgentIdentity,
        mob_id: meerkat_mob::MobId,
        detail: String,
    },
    /// Refused before admission: the member's admission is closed while its
    /// identity converges (retry once it has converged). Nothing was
    /// delivered.
    IdentityConvergenceAdmissionClosed {
        identity: MobAgentIdentity,
        detail: String,
    },
}

impl std::fmt::Display for BridgeAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostHumanInput(error) => write!(f, "{error}"),
            Self::GovernedInput(error) => write!(f, "{error}"),
            Self::CompletionUnsupported(msg) => write!(
                f,
                "session bridge does not support completion-bearing delivery: {msg}; \
                 nothing was submitted"
            ),
            Self::Mob(msg)
            | Self::UnsupportedForMode { detail: msg, .. }
            | Self::MobAdmissionClosed { detail: msg, .. }
            | Self::MobShutdownInProgress { detail: msg, .. }
            | Self::IdentityConvergenceAdmissionClosed { detail: msg, .. } => {
                write!(f, "session bridge mob error before receipt: {msg}")
            }
            Self::InvalidInput(msg) => {
                write!(f, "session bridge invalid input before admission: {msg}")
            }
            Self::ResumeRejected {
                kind,
                session_id,
                detail,
            } => write!(
                f,
                "session bridge resume of session {session_id} rejected before admission \
                 ({kind:?}): {detail}"
            ),
            Self::ProviderAuthRejected { failure, detail } => write!(
                f,
                "session bridge member build refused by provider authentication before \
                 admission (kind={}, provider={}): {detail}",
                failure.kind.as_str(),
                failure.provider.as_str()
            ),
            Self::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            } => fmt_actor_timeout(f, operation, identity, *waited, *command),
            Self::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            } => write!(
                f,
                "session bridge actor call `{operation}` for member {identity}: \
                 the mob actor loop has an open stall (stall_id {stall_id}, open for \
                 {stalled_for:?}); {observation}"
            ),
            Self::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            } => write!(
                f,
                "session bridge actor call `{operation}` for member {identity}: \
                 the mob actor loop TERMINATED ({detail}); the process must restart; {observation}"
            ),
            Self::ReloadRequired { identity, reason } => write!(
                f,
                "session bridge delivery to member {identity} refused before admission: the \
                 member's runtime registration requires a registration-authorized cold reload \
                 (meta/reload_member): {reason}"
            ),
            Self::AdmissionBacklogFull { identity, depth } => write!(
                f,
                "session bridge delivery to member {identity} refused before admission: the \
                 member's admission lane is full ({depth} parked); nothing was queued; retry \
                 once the lane drains"
            ),
            Self::ReloadRefused {
                identity,
                session_id,
                reason,
            } => write!(
                f,
                "session bridge reload of member {identity} (session {session_id}) refused before \
                 admission: store not healthy, reload refused, retry later: {reason}"
            ),
            Self::ReloadTimedOut {
                identity,
                session_id,
                stage,
            } => write!(
                f,
                "session bridge reload of member {identity} (session {session_id}) timed out \
                 before admission at stage `{stage}`; inspect before retrying"
            ),
            Self::InvariantViolation(msg) => {
                write!(f, "session bridge admission invariant violated: {msg}")
            }
        }
    }
}

impl std::error::Error for BridgeAdmissionError {}

impl From<BridgeError> for BridgeAdmissionError {
    fn from(error: BridgeError) -> Self {
        match error {
            BridgeError::HostHumanInput(error) => Self::HostHumanInput(error),
            BridgeError::CompletionUnsupported(detail) => Self::CompletionUnsupported(detail),
            error @ BridgeError::Unsupported { .. } => Self::Mob(error.to_string()),
            // A placement host's refusal to spawn; admission keeps its text.
            error @ BridgeError::RealmBackendUnavailable { .. } => Self::Mob(error.to_string()),
            // A durable profile's spawn refusal; admission keeps its text.
            error @ BridgeError::NonDurableService { .. } => Self::Mob(error.to_string()),
            // The heal seam's hosting holds; admission keeps their text.
            error @ (BridgeError::ServedElsewhere { .. }
            | BridgeError::HostingUnavailable { .. }) => Self::Mob(error.to_string()),
            BridgeError::Mob(detail) => Self::Mob(detail),
            // Reached only where the deliver path did not repair it (for
            // example host human input); the text is meerkat's rendering.
            BridgeError::MemberRuntimeDetached { detail, .. } => Self::Mob(detail),
            BridgeError::UnsupportedForMode {
                identity,
                mode,
                detail,
            } => Self::UnsupportedForMode {
                identity,
                mode,
                detail,
            },
            BridgeError::InvalidInput(detail) => Self::InvalidInput(detail),
            BridgeError::ResumeRejected {
                kind,
                session_id,
                detail,
            } => Self::ResumeRejected {
                kind,
                session_id,
                detail,
            },
            // The registration reload surfaced the same typed fact the resume
            // path reports as a rejection kind; nothing was admitted.
            BridgeError::SessionRepairRequired {
                identity,
                session_id,
                detail,
            } => Self::ResumeRejected {
                kind: ResumeRejectionKind::Held {
                    hold: meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence,
                },
                detail: format!("member {identity} session {session_id}: {detail}"),
                session_id,
            },
            BridgeError::ProviderAuthRejected { failure, detail } => {
                Self::ProviderAuthRejected { failure, detail }
            }
            BridgeError::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            } => Self::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            },
            BridgeError::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            } => Self::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            },
            BridgeError::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            } => Self::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            },
            BridgeError::ReloadRequired { identity, reason } => {
                Self::ReloadRequired { identity, reason }
            }
            BridgeError::AdmissionBacklogFull { identity, depth } => {
                Self::AdmissionBacklogFull { identity, depth }
            }
            BridgeError::ReloadRefused {
                identity,
                session_id,
                reason,
            } => Self::ReloadRefused {
                identity,
                session_id,
                reason,
            },
            BridgeError::ReloadTimedOut {
                identity,
                session_id,
                stage,
            } => Self::ReloadTimedOut {
                identity,
                session_id,
                stage,
            },
            BridgeError::CompletionFailed(detail) => Self::InvariantViolation(format!(
                "completion failure escaped before an admitted-turn receipt existed: {detail}"
            )),
            BridgeError::PostAdmissionResolutionFailed(detail) => {
                Self::InvariantViolation(format!(
                    "post-admission projection failure escaped before an admitted-turn receipt \
                     existed: {detail}"
                ))
            }
            // Raised by the resume collision arm and `ensure_member_live`
            // only, never on the delivery path.
            error @ BridgeError::HostAdopted { .. } => Self::Mob(error.to_string()),
            // Raised by `live_member` and `ensure_member_live` only.
            error @ BridgeError::LiveMemberMoved { .. } => Self::Mob(error.to_string()),
            BridgeError::MobAdmissionClosed {
                identity,
                mob_id,
                reason,
                detail,
            } => Self::MobAdmissionClosed {
                identity,
                mob_id,
                reason,
                detail,
            },
            BridgeError::MobShutdownInProgress {
                identity,
                mob_id,
                detail,
            } => Self::MobShutdownInProgress {
                identity,
                mob_id,
                detail,
            },
            BridgeError::IdentityConvergenceAdmissionClosed { identity, detail } => {
                Self::IdentityConvergenceAdmissionClosed { identity, detail }
            }
        }
    }
}

impl From<BridgeAdmissionError> for BridgeError {
    fn from(error: BridgeAdmissionError) -> Self {
        match error {
            BridgeAdmissionError::HostHumanInput(error) => Self::HostHumanInput(error),
            // Only the governed admission produces it; its rendering keeps
            // the typed reason.
            BridgeAdmissionError::GovernedInput(error) => Self::Mob(error.to_string()),
            BridgeAdmissionError::CompletionUnsupported(detail) => {
                Self::CompletionUnsupported(detail)
            }
            BridgeAdmissionError::Mob(detail) => Self::Mob(detail),
            BridgeAdmissionError::UnsupportedForMode {
                identity,
                mode,
                detail,
            } => Self::UnsupportedForMode {
                identity,
                mode,
                detail,
            },
            BridgeAdmissionError::InvalidInput(detail) => Self::InvalidInput(detail),
            BridgeAdmissionError::ResumeRejected {
                kind,
                session_id,
                detail,
            } => Self::ResumeRejected {
                kind,
                session_id,
                detail,
            },
            BridgeAdmissionError::ProviderAuthRejected { failure, detail } => {
                Self::ProviderAuthRejected { failure, detail }
            }
            BridgeAdmissionError::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            } => Self::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            },
            BridgeAdmissionError::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            } => Self::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                stalled_for,
                observation,
            },
            BridgeAdmissionError::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            } => Self::ActorTerminated {
                operation,
                identity,
                detail,
                observation,
            },
            BridgeAdmissionError::ReloadRequired { identity, reason } => {
                Self::ReloadRequired { identity, reason }
            }
            BridgeAdmissionError::AdmissionBacklogFull { identity, depth } => {
                Self::AdmissionBacklogFull { identity, depth }
            }
            BridgeAdmissionError::ReloadRefused {
                identity,
                session_id,
                reason,
            } => Self::ReloadRefused {
                identity,
                session_id,
                reason,
            },
            BridgeAdmissionError::ReloadTimedOut {
                identity,
                session_id,
                stage,
            } => Self::ReloadTimedOut {
                identity,
                session_id,
                stage,
            },
            BridgeAdmissionError::InvariantViolation(detail) => Self::Mob(detail),
            BridgeAdmissionError::MobAdmissionClosed {
                identity,
                mob_id,
                reason,
                detail,
            } => Self::MobAdmissionClosed {
                identity,
                mob_id,
                reason,
                detail,
            },
            BridgeAdmissionError::MobShutdownInProgress {
                identity,
                mob_id,
                detail,
            } => Self::MobShutdownInProgress {
                identity,
                mob_id,
                detail,
            },
            BridgeAdmissionError::IdentityConvergenceAdmissionClosed { identity, detail } => {
                Self::IdentityConvergenceAdmissionClosed { identity, detail }
            }
        }
    }
}

/// Errors from an exact turn receipt after admission has already occurred.
///
/// Admission-only states are unrepresentable here. A caller holding this error
/// knows a turn was admitted and therefore must never repair-and-resubmit it.
#[derive(Debug)]
pub enum BridgeTurnError {
    /// The admitted turn reached a failed terminal. When session resolution
    /// also failed, its detail is retained as secondary evidence.
    CompletionFailed(String),
    /// The admitted turn succeeded, but resolving its session id failed.
    PostAdmissionResolutionFailed(String),
}

impl std::fmt::Display for BridgeTurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompletionFailed(detail) => write!(
                f,
                "session bridge turn was admitted and then failed: {detail}; the turn RAN - \
                 do not retry"
            ),
            Self::PostAdmissionResolutionFailed(detail) => write!(
                f,
                "session bridge turn was admitted and COMPLETED, then resolving its runtime \
                 session id failed: {detail}; the turn RAN - do not retry"
            ),
        }
    }
}

impl std::error::Error for BridgeTurnError {}

impl From<BridgeTurnError> for BridgeError {
    fn from(error: BridgeTurnError) -> Self {
        match error {
            BridgeTurnError::CompletionFailed(detail) => Self::CompletionFailed(detail),
            BridgeTurnError::PostAdmissionResolutionFailed(detail) => {
                Self::PostAdmissionResolutionFailed(detail)
            }
        }
    }
}

/// Typed classification of a rejected resume, read from Core's typed errors
/// only: no error text is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeRejectionKind {
    /// Core's session owner refused the resume with a typed reason
    /// (`MobError::SessionUnavailableForResume`). `Absent` means Core holds no
    /// durable session under the id. Every other reason leaves the durable
    /// copies intact: archived, an unprovable committed boundary,
    /// contradictory durable authority, or authority that changed while the
    /// body loaded.
    Unavailable {
        reason: meerkat_mob::error::SessionResumeUnavailableReason,
    },
    /// Core holds the session's resume (`DurableResumeHold`, carried by
    /// `MobError::MemberRestoreFailed` or by a session error's structured
    /// data). The transcript is retained; the hold names what clears it.
    Held {
        hold: meerkat_core::service::DurableResumeHold,
    },
    /// A role migration this activation declared for the identity was not
    /// applied: meerkat refused it (`MobError::MemberRoleMigrationRequired` or
    /// `MemberRoleMigrationRejected`), or the member the mob restored still
    /// runs another role and this resume may not restamp it (for example a
    /// Present identity intent owns its materialization). The member is never
    /// attached under the undeclared role: a declaration is applied or refused,
    /// never a silent no-op.
    RoleMigrationNotApplied,
    /// meerkat's typed restore failure (`MobError::MemberRestoreFailed`)
    /// without a resume hold: the member is Broken-in-roster upstream with
    /// restore diagnostics preserved.
    MemberRestoreFailed,
    /// Any other resume-time failure.
    Other,
    /// The probe a repair runs before it retires the member could not tell
    /// whether the resume source exists: `authority` (the archive authority)
    /// failed to answer. Nothing was disposed or retired; retryable once the
    /// authority answers.
    SourceUnanswered { authority: &'static str },
}

/// meerkat's typed resume hold carried by a mob error, when it carries one.
///
/// Two typed carriers, both read without display text: the in-process
/// `MobError::SessionError` variant (through meerkat-mob's shared retirement /
/// lifecycle failure wrappers, exactly as [`provider_auth_rejection`] reads
/// them) exposes [`meerkat_core::service::SessionError::durable_resume_hold`];
/// every other shape is read back off meerkat's structured `durable_resume_hold`
/// wire token through `SessionError::durable_resume_hold_from_data`, the same
/// table the meerkat RPC client uses to re-type a hold across the transport.
/// An unknown token fails closed as `None`.
pub(crate) fn durable_resume_hold_of(
    error: &meerkat_mob::MobError,
) -> Option<meerkat_core::service::DurableResumeHold> {
    match error {
        meerkat_mob::MobError::SessionError(session_error) => session_error.durable_resume_hold(),
        // meerkat 0.8.41: a Broken member's recorded restore failure keeps the
        // hold it was, so every later refusal for that member (a repeated
        // reload, a send) classifies typed instead of as restore-failure prose.
        meerkat_mob::MobError::MemberRestoreFailed {
            hold: Some(hold), ..
        } => Some(*hold),
        meerkat_mob::MobError::SharedRetirementFailure(inner)
        | meerkat_mob::MobError::SharedLifecycleFailure(inner) => durable_resume_hold_of(inner),
        other => other
            .structured_data()
            .as_ref()
            .and_then(meerkat_core::service::SessionError::durable_resume_hold_from_data),
    }
}

/// Log and construct the typed resume rejection for one failed resume step.
/// Deliberately loud: a rejected resume degrades the identity until a
/// reconcile retry succeeds, and the operator needs the real (classified)
/// error — not a generic fallback reason.
/// A role migration declared for this resume: `(from_role, to_role)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredRoleMigration {
    from_role: meerkat_mob::ProfileName,
    to_role: meerkat_mob::ProfileName,
}

impl DeclaredRoleMigration {
    /// The migration a resume spec carries, if it declares one.
    fn of(spawn_spec: &SpawnMemberSpec) -> Option<Self> {
        spawn_spec
            .launch_mode
            .resume_from_role()
            .map(|from_role| Self {
                from_role: from_role.clone(),
                to_role: spawn_spec.role_name.clone(),
            })
    }
}

/// Refuse to attach a member that does not run the role its declared
/// migration names. Typed, logged, and never a silent no-op.
fn role_migration_not_applied(
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
    migration: &DeclaredRoleMigration,
    observed_role: Option<&meerkat_mob::ProfileName>,
    reason: &str,
) -> BridgeError {
    let observed = observed_role.map_or("no rostered member", |role| role.as_str());
    tracing::error!(
        identity = %identity,
        session_id = %session_id,
        from_role = %migration.from_role,
        to_role = %migration.to_role,
        observed_role = observed,
        reason,
        "declared role migration not applied; refusing to attach the member under an \
         undeclared role (durable session preserved, identity degraded)"
    );
    BridgeError::ResumeRejected {
        kind: ResumeRejectionKind::RoleMigrationNotApplied,
        session_id: session_id.clone(),
        detail: format!(
            "declared role migration of {identity} from '{}' to '{}' was not applied: \
             {reason} (observed role: {observed})",
            migration.from_role, migration.to_role
        ),
    }
}

fn resume_rejected(
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
    error: &meerkat_mob::MobError,
    step: &str,
) -> BridgeError {
    if let Some(failure) = provider_auth_rejection(error) {
        tracing::error!(
            identity = %identity,
            session_id = %session_id,
            auth_kind = failure.kind.as_str(),
            provider = failure.provider.as_str(),
            step,
            error = %error,
            "resume refused by provider authentication; durable session preserved, identity \
             degraded (a deterministic credential kind parks instead of heal-looping)"
        );
        return BridgeError::ProviderAuthRejected {
            failure,
            detail: format!("{step}: {error}"),
        };
    }
    if let Some((_, profile)) = non_durable_service_refusal(error) {
        tracing::error!(
            identity = %identity,
            session_id = %session_id,
            profile = %profile,
            step,
            error = %error,
            "resume refused: the profile declares durable workers and the mob's member \
             sessions do not outlive the process; durable session preserved, refused on every \
             wake until the realm or the profile changes"
        );
        return BridgeError::NonDurableService {
            profile: profile.clone(),
            detail: format!("{step}: {error}"),
        };
    }
    let kind = classify_resume_error(error);
    if matches!(
        kind,
        ResumeRejectionKind::Unavailable {
            reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
        }
    ) {
        tracing::warn!(
            identity = %identity,
            session_id = %session_id,
            step,
            error = %error,
            "resume refused: Core holds no durable session under this id; the caller decides"
        );
    } else {
        tracing::error!(
            identity = %identity,
            session_id = %session_id,
            kind = ?kind,
            step,
            error = %error,
            "resume rejected; durable session preserved, identity degraded pending reconcile \
             retry (refusing fresh-spawn fallback)"
        );
    }
    BridgeError::ResumeRejected {
        kind,
        session_id: session_id.clone(),
        detail: format!("{step}: {error}"),
    }
}

/// Recover meerkat's typed provider-auth build refusal from a mob error, when
/// it carries one. Both carriers are read typed, never from display text: an
/// in-process build fails as `MobError::SessionError` whose
/// `SessionError::provider_auth_failure_data` is the secret-free
/// `SessionProviderAuthFailure`; a placed or remote materialize fails as a
/// `MaterializeBuildRejected` bridge rejection whose
/// `MemberBuildRejection::ProviderAuth` projects back onto the same type.
/// Anything else is `None`.
pub(crate) fn provider_auth_rejection(
    error: &meerkat_mob::MobError,
) -> Option<meerkat_core::service::SessionProviderAuthFailure> {
    match error {
        meerkat_mob::MobError::SessionError(session_error) => {
            session_error.provider_auth_failure_data()
        }
        meerkat_mob::MobError::BridgeCommandRejected {
            cause:
                meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause::MaterializeBuildRejected {
                    cause,
                },
            ..
        } => cause.provider_auth_failure(),
        // meerkat-mob shares ONE owned failure with every concurrent observer
        // of a retirement or lifecycle operation through these transparent
        // wrappers (a joined resume failure always arrives this way). Read
        // through them exactly as meerkat-mob's own accessors do, or a wrapped
        // refusal falls back to the untyped path and heal-loops.
        meerkat_mob::MobError::SharedRetirementFailure(inner)
        | meerkat_mob::MobError::SharedLifecycleFailure(inner) => provider_auth_rejection(inner),
        _ => None,
    }
}

/// Whether meerkat refused the spawn because the placement host's realm
/// backend is not persistent while the member needs durable sessions. Read
/// typed from the bridge rejection cause, which meerkat carries through its
/// shared failure wrappers.
fn realm_backend_unavailable(error: &meerkat_mob::MobError) -> bool {
    use meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause;
    matches!(
        error.bridge_rejection_cause(),
        Some(BridgeRejectionCause::RealmBackendUnavailable)
    )
}

/// meerkat's typed refusal of a spawn whose profile declares
/// `durable = true` on a mob whose member sessions do not outlive the
/// process (`MobError::NonDurableService`), with the refused profile. Read
/// through meerkat's shared failure wrappers, as the other typed refusals
/// are. The refusal itself is returned too, so a surface can render it with
/// meerkat's own code and data.
pub(crate) fn non_durable_service_refusal(
    error: &meerkat_mob::MobError,
) -> Option<(&meerkat_mob::MobError, &meerkat_mob::ProfileName)> {
    match error {
        meerkat_mob::MobError::NonDurableService { profile, .. } => Some((error, profile)),
        meerkat_mob::MobError::SharedRetirementFailure(inner)
        | meerkat_mob::MobError::SharedLifecycleFailure(inner) => {
            non_durable_service_refusal(inner)
        }
        _ => None,
    }
}

/// Map a member spawn failure onto the bridge vocabulary: the typed
/// provider-auth, non-persistent-realm or durable-profile refusal when
/// meerkat carried one, otherwise the display text exactly as before.
fn spawn_bridge_error(step: &str, error: &meerkat_mob::MobError) -> BridgeError {
    if let Some(failure) = provider_auth_rejection(error) {
        return BridgeError::ProviderAuthRejected {
            failure,
            detail: format!("{step}: {error}"),
        };
    }
    if realm_backend_unavailable(error) {
        return BridgeError::RealmBackendUnavailable {
            detail: format!("{step}: {error}"),
        };
    }
    if let Some((_, profile)) = non_durable_service_refusal(error) {
        return BridgeError::NonDurableService {
            profile: profile.clone(),
            detail: format!("{step}: {error}"),
        };
    }
    BridgeError::Mob(error.to_string())
}

/// The respawn-shaped twin of [`spawn_bridge_error`]: a destructive reset
/// spawns its successor through meerkat's respawn, whose error nests the mob
/// cause under `Mob` or `SpawnAfterRetireWithCause`.
fn respawn_bridge_error(step: &str, error: &meerkat_mob::MobRespawnError) -> BridgeError {
    let cause = match error {
        meerkat_mob::MobRespawnError::Mob(cause)
        | meerkat_mob::MobRespawnError::SpawnAfterRetireWithCause { cause, .. } => Some(cause),
        _ => None,
    };
    if let Some(failure) = cause.and_then(provider_auth_rejection) {
        return BridgeError::ProviderAuthRejected {
            failure,
            detail: format!("{step}: {error}"),
        };
    }
    if cause.is_some_and(realm_backend_unavailable) {
        return BridgeError::RealmBackendUnavailable {
            detail: format!("{step}: {error}"),
        };
    }
    if let Some((_, profile)) = cause.and_then(non_durable_service_refusal) {
        return BridgeError::NonDurableService {
            profile: profile.clone(),
            detail: format!("{step}: {error}"),
        };
    }
    BridgeError::Mob(format!("{step}: {error}"))
}

/// Classify a resume-spawn failure from Core's typed errors only.
fn classify_resume_error(error: &meerkat_mob::MobError) -> ResumeRejectionKind {
    // meerkat-mob shares ONE owned failure with every concurrent observer of
    // a retirement or lifecycle operation through these transparent wrappers:
    // the collision retire before a resume retry and a joined resume observer
    // both hand their failure to `resume_rejected` this way. Classify what
    // they carry, exactly as `provider_auth_rejection` does, or a wrapped
    // typed refusal reads as `Other`.
    if let meerkat_mob::MobError::SharedRetirementFailure(inner)
    | meerkat_mob::MobError::SharedLifecycleFailure(inner) = error
    {
        return classify_resume_error(inner);
    }
    // A durable resume hold is never transient: Core keeps the transcript and
    // the hold names what clears it.
    if let Some(hold) = durable_resume_hold_of(error) {
        return ResumeRejectionKind::Held { hold };
    }
    if let meerkat_mob::MobError::SessionUnavailableForResume { reason, .. } = error {
        return ResumeRejectionKind::Unavailable { reason: *reason };
    }
    if matches!(
        error,
        meerkat_mob::MobError::MemberRoleMigrationRequired { .. }
            | meerkat_mob::MobError::MemberRoleMigrationRejected { .. }
    ) {
        return ResumeRejectionKind::RoleMigrationNotApplied;
    }
    if matches!(error, meerkat_mob::MobError::MemberRestoreFailed { .. }) {
        return ResumeRejectionKind::MemberRestoreFailed;
    }
    ResumeRejectionKind::Other
}

/// Typed reason a requested resume could not reuse the persisted runtime
/// binding and the identity woke on a fresh member spawn instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeFallbackReason {
    /// The persisted session/runtime identity is incompatible with the current
    /// mob runtime binding.
    RuntimeIdentityIncompatible { detail: String },
    /// Core holds no durable session under the recorded id (its typed
    /// `SessionResumeUnavailableReason::Absent`). There is no transcript to
    /// preserve, so a fresh spawn in its place is recovery, not data loss.
    NeverPersisted { detail: String },
}

/// Result of attempting to materialize a persisted identity through resume.
///
/// Every variant carries the runtime binding Core committed for the member
/// (`agent_runtime_id`, read back from the roster after the spawn or attach),
/// never one the caller supplied: MobMachine mints the generation, and a
/// resume after a retirement runs under a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeSessionOutcome {
    /// The persisted session was resumed as-is.
    Resumed {
        session_id: meerkat_core::types::SessionId,
        agent_runtime_id: AgentRuntimeId,
    },
    /// Custody and bookkeeping are reconciled, but the session is NOT yet
    /// usable or resumed.
    ///
    /// Returned when an occupant is proven to be ours and already bound to this
    /// session, but its owning identity has no authoritative registration - the
    /// legacy no-intent case. The bridge cannot establish that registration: it
    /// requires the generation, checkpoint version and fencing token that live
    /// in the caller's continuity record, and manufacturing them here would be a
    /// second source of truth.
    ///
    /// The caller MUST call `register_session_runtime_state` from its
    /// authoritative record and only then treat the session as resumed. Nothing
    /// may commit a runtime boundary before that: the continuity adapter refuses
    /// a head-canonical session whose owner was never registered, and it is
    /// right to.
    AttachedPendingRegistration {
        session_id: meerkat_core::types::SessionId,
        agent_runtime_id: AgentRuntimeId,
    },
    /// Resume was rejected for a typed compatibility reason and a fresh member
    /// was spawned instead.
    FreshSpawned {
        session_id: meerkat_core::types::SessionId,
        agent_runtime_id: AgentRuntimeId,
        reason: ResumeFallbackReason,
    },
}

impl ResumeSessionOutcome {
    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        match self {
            Self::Resumed { session_id, .. }
            | Self::AttachedPendingRegistration { session_id, .. }
            | Self::FreshSpawned { session_id, .. } => session_id,
        }
    }

    /// The incarnation Core committed for the member this outcome describes.
    #[must_use]
    pub fn agent_runtime_id(&self) -> &AgentRuntimeId {
        match self {
            Self::Resumed {
                agent_runtime_id, ..
            }
            | Self::AttachedPendingRegistration {
                agent_runtime_id, ..
            }
            | Self::FreshSpawned {
                agent_runtime_id, ..
            } => agent_runtime_id,
        }
    }

    #[must_use]
    pub fn fallback_reason(&self) -> Option<&ResumeFallbackReason> {
        match self {
            Self::Resumed { .. } | Self::AttachedPendingRegistration { .. } => None,
            Self::FreshSpawned { reason, .. } => Some(reason),
        }
    }

    /// Whether this outcome still needs authoritative registration before the
    /// session may be treated as resumed or a runtime boundary committed.
    ///
    /// Deliberately explicit rather than inferred from `session_id`: a caller
    /// that forgets this hands a head-canonical session to a continuity adapter
    /// that will refuse it, and the refusal surfaces far from the cause.
    #[must_use]
    pub fn needs_owner_registration(&self) -> bool {
        matches!(self, Self::AttachedPendingRegistration { .. })
    }
}

// ---------------------------------------------------------------------------
// Bounded actor admission
// ---------------------------------------------------------------------------

/// Default budget one delivery attempt may spend waiting on the mob actor's
/// command loop, across ALL of that attempt's round trips.
///
/// The bounded calls are ADMISSION round trips (`get_member`,
/// `submit_work_with_mode` at `IngressAccepted`): they hand a command to the
/// actor and return once it is admitted at runtime ingress — they never await
/// turn completion. Each is an in-process channel send plus a machine-state
/// read on a responsive actor, so healthy latency is orders of magnitude
/// inside this budget and it cannot fire on the healthy path.
///
/// The value is therefore not sized against the healthy path but against the
/// one legitimate reason admission is slow: the mob actor is a single
/// serialized command loop, so an admission call can queue behind another
/// member's entire in-flight turn. Ten minutes is ~6.7x the 90s this crate
/// already treats as the outer bound for waiting on a turn's *output*
/// (`IdentityRuntimeConfig::default_timeout`) and ~3.3x the worst measured
/// single-turn latency in the deployment that motivated this bound (180s for
/// a one-word turn on a 94 MB document). It sits deliberately BELOW the
/// 962-second dispatch hang observed in production, so the pathology this
/// exists to name still produces a signal.
///
/// Trade-off, stated plainly: ten minutes is a terrible wait and a much
/// shorter bound would diagnose far faster. Shorter was rejected — at 30-90s
/// the timeout would fire while another member is merely running a long tool
/// chain, converting a latency problem into dropped deliveries, which is
/// worse than no bound at all. Deployments whose turns are known to be short
/// should tighten this through the env knob below.
const BRIDGE_ACTOR_ADMISSION_BUDGET: Duration = Duration::from_mins(10);

/// Effective admission budget: `META_BRIDGE_ACTOR_ADMISSION_SECS` overrides
/// the default, clamped to [1, 3600] seconds. The floor keeps `0` from
/// meaning "reject everything"; the ceiling keeps a mistyped value from
/// silently restoring the unbounded hang this replaces.
fn bridge_actor_admission_budget() -> Duration {
    parse_bridge_actor_admission_budget(
        std::env::var("META_BRIDGE_ACTOR_ADMISSION_SECS")
            .ok()
            .as_deref(),
    )
}

fn parse_bridge_actor_admission_budget(raw: Option<&str>) -> Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.clamp(1, 3600)))
        .unwrap_or(BRIDGE_ACTOR_ADMISSION_BUDGET)
}

/// One delivery attempt's shared deadline for mob-actor round trips.
///
/// `MobHandle::send_actor_command` awaits both the command send and the reply
/// with no timeout, so a blocked actor hangs the dispatch silently. Every
/// round trip in a delivery attempt runs under this ONE deadline rather than
/// a per-call timeout: the delivery path makes three or four serialized round
/// trips, and three serialized bounds would cost three budgets — a worse
/// worst case than the hang being contained.
struct ActorAdmissionDeadline {
    started: tokio::time::Instant,
    deadline: tokio::time::Instant,
    /// The actor-loop probe's shared verdict, when a probe is wired. `None`
    /// (validation-only compositions, unit tests) keeps the pure deadline.
    health: Option<Arc<ActorLoopHealth>>,
}

impl ActorAdmissionDeadline {
    /// A pure deadline with no probe wired (unit tests of the budget alone).
    #[cfg(test)]
    fn new(budget: Duration) -> Self {
        Self::with_health(budget, None)
    }

    fn with_health(budget: Duration, health: Option<Arc<ActorLoopHealth>>) -> Self {
        let started = tokio::time::Instant::now();
        Self {
            started,
            deadline: started + budget,
            health,
        }
    }

    /// The typed refusal for an unhealthy loop verdict, or `None` while the
    /// loop reads live.
    fn refusal_for(
        state: &ActorLoopHealthState,
        operation: &'static str,
        identity: &MobAgentIdentity,
        observation: ActorCallObservation,
    ) -> Option<BridgeError> {
        match state {
            ActorLoopHealthState::Live => None,
            ActorLoopHealthState::Stalled { stall_id, since } => {
                tracing::warn!(
                    operation,
                    identity = %identity,
                    stall_id,
                    stalled_for_ms = since.elapsed().as_millis(),
                    %observation,
                    "actor call observation ended: the mob actor loop has an open stall"
                );
                Some(BridgeError::ActorLoopStalled {
                    operation,
                    identity: identity.clone(),
                    stall_id: *stall_id,
                    stalled_for: since.elapsed(),
                    observation,
                })
            }
            ActorLoopHealthState::Terminated { detail, .. } => {
                tracing::error!(
                    operation,
                    identity = %identity,
                    detail = %detail,
                    %observation,
                    "delivery refused: the mob actor loop terminated; the process must restart"
                );
                Some(BridgeError::ActorTerminated {
                    operation,
                    identity: identity.clone(),
                    detail: detail.clone(),
                    observation,
                })
            }
        }
    }

    /// Keep inspection inside the bridge's existing health and deadline
    /// boundary while retrying only refused member-status observations.
    async fn observe_member_status<T, Read, ReadFuture>(
        &self,
        identity: &MobAgentIdentity,
        read: Read,
    ) -> Result<T, BridgeError>
    where
        Read: FnMut() -> ReadFuture,
        ReadFuture: Future<Output = Result<T, meerkat_mob::MobError>>,
    {
        self.bound(
            "inspect.member_status",
            identity,
            crate::member_status_observation::retry_member_status_observation(self.deadline, read),
        )
        .await?
        .map_err(|error| match error {
            meerkat_mob::MobError::ActorCommandTimedOut {
                command_kind: "MemberStatus",
                stage: "member_status_observation",
            } => BridgeError::ActorAdmissionTimeout {
                operation: "inspect.member_status",
                identity: identity.clone(),
                waited: self.started.elapsed(),
                command: None,
            },
            other => BridgeError::Mob(other.to_string()),
        })
    }

    /// Await one actor round trip under the attempt's remaining budget. A
    /// responsive actor takes the `timeout_at` pass-through: no added
    /// latency, no allocation, no behavioural change. Expiry is the only path
    /// that logs or allocates.
    async fn bound<T, F>(
        &self,
        operation: &'static str,
        identity: &MobAgentIdentity,
        call: F,
    ) -> Result<T, BridgeError>
    where
        F: Future<Output = T>,
    {
        // Circuit breaker: an OPEN stall (or a terminated loop) refuses the
        // round trip before it is queued. Checked on every hop, not once per
        // attempt, so a stall that opens between two serialized hops still
        // cuts the attempt short.
        if let Some(health) = self.health.as_ref()
            && let Some(refusal) = Self::refusal_for(
                &health.snapshot(),
                operation,
                identity,
                ActorCallObservation::BeforeCall,
            )
        {
            return Err(refusal);
        }
        let outcome = match self.health.as_ref() {
            // Race the call against the NEXT unhealthy verdict: a stall that
            // opens while this hop is parked behind it ends the wait the
            // instant the probe pages, instead of at the admission budget.
            Some(health) => {
                let call = std::pin::pin!(call);
                let unhealthy = health.unhealthy();
                let unhealthy = std::pin::pin!(unhealthy);
                match tokio::time::timeout_at(
                    self.deadline,
                    futures::future::select(call, unhealthy),
                )
                .await
                {
                    Ok(futures::future::Either::Left((value, _))) => Ok(Ok(value)),
                    Ok(futures::future::Either::Right((state, _))) => Ok(Err(state)),
                    Err(elapsed) => Err(elapsed),
                }
            }
            None => tokio::time::timeout_at(self.deadline, call).await.map(Ok),
        };
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(state)) => {
                Err(
                    Self::refusal_for(&state, operation, identity, ActorCallObservation::InFlight)
                        .unwrap_or(
                            // `unhealthy()` only resolves on a refusing state; keep the
                            // arm total without inventing a new failure class.
                            BridgeError::ActorAdmissionTimeout {
                                operation,
                                identity: identity.clone(),
                                waited: self.started.elapsed(),
                                command: None,
                            },
                        ),
                )
            }
            Err(_) => {
                let waited = self.started.elapsed();
                tracing::warn!(
                    operation,
                    identity = %identity,
                    waited_ms = waited.as_millis(),
                    "mob actor did not answer within the delivery admission budget; the actor \
                     round trip was not answered within budget — delivery abandoned. \
                     Scope unknown from one timeout: the loop may be stalled or only this \
                     member's call blocked; the ActorLoopStalled probe carries the loop-scope \
                     verdict"
                );
                Err(BridgeError::ActorAdmissionTimeout {
                    operation,
                    identity: identity.clone(),
                    waited,
                    command: None,
                })
            }
        }
    }
}

/// Build the internal-delivery [`WorkSpec`] for one member turn.
///
/// Pure by design so the threading is unit-testable: every optional carrier
/// on the deliver surface must reach the spec unchanged.
///
/// - Ask 1: `injected_context` rides as separate typed bodies rather than
///   being fused into the user's message text; WorkSpec carries it to the
///   StartTurnRequest, where meerkat stamps each entry as the
///   InjectedContext transcript role (excluded from compaction indexing).
/// - meerkat 0.8.11: `system_prompt` is one ordinary System message authored
///   for this exact turn (never member/session configuration); WorkSpec
///   carries it to the member StartTurnRequest, where meerkat appends it at
///   the turn's admitted transcript boundary.
/// - meerkat 0.7.25 ask 15 addendum: a host-supplied interaction id rides
///   WorkSpec into runtime admission, so this turn's live events AND its
///   committed transcript messages carry the SAME id the console minted at
///   send time — the exact live↔history join the console dedup needs. Only
///   UUID-form ids exist here (the identity-first console send mints v5
///   UUIDs; `meta/send` and `meta/send_message` reserve UUIDs); anything
///   else is skipped rather than corrupted.
fn internal_bridge_work_spec(
    content: &meerkat_core::ContentInput,
    system_prompt: Option<&str>,
    injected_context: &[meerkat_core::ContentInput],
    interaction_id: Option<&str>,
) -> WorkSpec {
    let mut spec = WorkSpec::new(content.clone(), WorkOrigin::Internal);
    if let Some(system_prompt) = system_prompt {
        spec = spec.with_system_prompt(system_prompt);
    }
    if !injected_context.is_empty() {
        spec = spec.with_injected_context(injected_context.to_vec());
    }
    if let Some(raw) = interaction_id {
        match raw.parse::<uuid::Uuid>() {
            Ok(id) => {
                spec = spec.with_interaction_id(meerkat_core::interaction::InteractionId(id));
            }
            Err(_) => {
                tracing::debug!(
                    interaction_id = %raw,
                    "non-UUID interaction id not threaded into runtime admission"
                );
            }
        }
    }
    spec
}

/// Spec-shaping inputs for one internal bridge delivery, grouped so the
/// submit path carries them as a unit rather than five loose parameters.
struct InternalBridgeWork<'a> {
    content: &'a meerkat_core::ContentInput,
    system_prompt: Option<&'a str>,
    injected_context: &'a [meerkat_core::ContentInput],
    interaction_id: Option<&'a str>,
    /// meerkat 0.8.15 internal-lane dedup: when present, the submit goes
    /// through `submit_work_with_mode_and_delivery_identity` and meerkat
    /// derives the WorkRef from mob + member identity + idempotency key -
    /// stable across lease-expiry reclaim, so a crash redelivery of the
    /// same identity resolves to the SAME work instead of a duplicate turn.
    delivery_identity: Option<&'a meerkat_mob::MobDeliveryIdentity>,
    /// The leased local session, present only on the explicit human lane.
    host_human_session: Option<&'a meerkat_core::SessionId>,
    /// Selected skills for the explicit human lane, resolved natively by
    /// the target member. Empty on every other lane.
    skill_references: &'a [meerkat_core::skills::SkillKey],
}

/// Which lane a submission runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeSubmitMode {
    /// Return as soon as the work is admitted. The historical behaviour.
    AdmissionOnly,
    /// Admit through the completion-bearing verb and hand the caller the turn
    /// handle to await.
    CompletionBearing,
}

/// What a completion-bearing receipt yields once its turn is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptOutput {
    /// Only the terminal ([`BridgeTurnReceipt::wait`]).
    Terminal,
    /// The terminal plus the turn's own output, projected from its committed
    /// run result ([`BridgeTurnReceipt::wait_with_output`]).
    TurnOutput,
}

/// The admitted turn's own output: meerkat's bounded projection of THIS work
/// item's committed run result.
///
/// A successful terminal WITHOUT a run result (`CompletedWithoutResult`) is
/// NOT "no text". meerkat reports it when the input was consumed without a
/// run of its own: folded into a run already in progress (a live-boundary
/// join) or deduplicated onto an earlier admission whose turn had already
/// ended (meerkat-runtime `CompletionOutcome::CompletedWithoutResult`: "the
/// input was consumed but produced no RunResult"). It is reported as
/// [`TurnOutput::NoOwnResult`](super::types::TurnOutput::NoOwnResult), so a
/// deduplicated re-admission can never overwrite or impersonate the output of
/// the turn that did run.
/// [`project_turn_output`] for a member turn (a governed delivery).
async fn project_member_turn_output(
    turn: meerkat_mob::MemberTurnHandle,
    spec: meerkat_mob::BoundedResultSpec,
) -> Result<super::types::TurnOutput, String> {
    match turn.wait_bounded(spec).await {
        Ok(result) => {
            let bounded = result.result().result();
            let text = bounded.text();
            Ok(if text.is_empty() {
                super::types::TurnOutput::Empty
            } else {
                super::types::TurnOutput::Text {
                    text: text.to_string(),
                    truncated: matches!(
                        bounded.status(),
                        meerkat_mob::BoundedHelperResultStatus::CompletedTruncated
                    ),
                }
            })
        }
        Err(error) => match error.failure() {
            meerkat_mob::BoundedTurnFailure::CompletedWithoutResult { .. } => {
                Ok(super::types::TurnOutput::NoOwnResult)
            }
            _ => Err(error.to_string()),
        },
    }
}

async fn project_turn_output(
    turn: meerkat_mob::WorkTurnHandle,
    spec: meerkat_mob::BoundedResultSpec,
) -> Result<super::types::TurnOutput, String> {
    match turn.wait_bounded(spec).await {
        Ok(result) => {
            let bounded = result.result().result();
            let text = bounded.text();
            Ok(if text.is_empty() {
                super::types::TurnOutput::Empty
            } else {
                super::types::TurnOutput::Text {
                    text: text.to_string(),
                    truncated: matches!(
                        bounded.status(),
                        meerkat_mob::BoundedHelperResultStatus::CompletedTruncated
                    ),
                }
            })
        }
        Err(error) => match error.failure() {
            meerkat_mob::BoundedTurnFailure::CompletedWithoutResult { .. } => {
                Ok(super::types::TurnOutput::NoOwnResult)
            }
            _ => Err(error.to_string()),
        },
    }
}

/// Admit one internal bridge delivery.
///
/// [`BridgeSubmitMode::AdmissionOnly`] is the ingress-only path and is
/// unchanged: it returns as soon as the work is admitted, yielding `None`.
///
/// [`BridgeSubmitMode::CompletionBearing`] admits through meerkat's
/// `start_work_with_mode*` and returns the resulting [`WorkTurnHandle`]
/// UNAWAITED. This function never awaits a turn: the caller decides when, and
/// does so only after the repair-and-retry decision is settled, so a turn that
/// ran and then failed can never be mistaken for a delivery that never landed.
///
/// One code path with one typed mode, deliberately - a separate copy would let
/// the completion lane drift from the ingress lane on admission-budget or
/// stale-state repair semantics, which are exactly the parts that are hard to
/// get right twice.
///
/// `deadline` bounds the ACTOR ROUND TRIP only. Completion is not awaited here
/// at all, so nothing in this function can spend the admission budget on a
/// turn.
async fn submit_internal_bridge_work(
    handle: &MobHandle,
    member_id: &MobAgentIdentity,
    work: InternalBridgeWork<'_>,
    handling_mode: HandlingMode,
    deadline: &ActorAdmissionDeadline,
    mode: BridgeSubmitMode,
) -> Result<Option<meerkat_mob::WorkTurnHandle>, BridgeError> {
    let entry = deadline
        .bound(
            "deliver.get_member",
            member_id,
            handle.get_member(member_id),
        )
        .await?
        .map_err(|err| {
            if work.host_human_session.is_some() {
                BridgeError::HostHumanInput(HostHumanInputError::Mob(Box::new(err)))
            } else {
                BridgeError::Mob(err.to_string())
            }
        })?
        .ok_or_else(|| {
            if work.host_human_session.is_some() {
                BridgeError::HostHumanInput(HostHumanInputError::BindingChanged)
            } else {
                BridgeError::Mob(format!("member not found: {member_id}"))
            }
        })?;
    let spec = internal_bridge_work_spec(
        work.content,
        work.system_prompt,
        work.injected_context,
        work.interaction_id,
    );
    if let Some(expected_session) = work.host_human_session {
        if entry.bridge_session_id() != Some(expected_session) {
            return Err(BridgeError::HostHumanInput(
                HostHumanInputError::BindingChanged,
            ));
        }
        let delivery_identity = work.delivery_identity.cloned().ok_or_else(|| {
            BridgeError::HostHumanInput(HostHumanInputError::Unsupported(
                "host human input requires a stable delivery identity".to_string(),
            ))
        })?;
        // Selected skills ride the same fenced admission as the content, so
        // they reach exactly this member and join the runtime's replay
        // identity for the delivery.
        let options = if work.skill_references.is_empty() {
            meerkat_mob::MemberTurnOptions::new()
        } else {
            meerkat_mob::MemberTurnOptions::new()
                .with_skill_references(work.skill_references.to_vec())
        };
        let result = match mode {
            BridgeSubmitMode::AdmissionOnly => deadline
                .bound(
                    "deliver.submit_host_human_input",
                    member_id,
                    handle.submit_host_human_input_with_options_bounded(
                        entry.agent_runtime_id,
                        entry.fence_token,
                        spec,
                        handling_mode,
                        options,
                        delivery_identity,
                        deadline.deadline.into_std(),
                    ),
                )
                .await?
                .map(|_| None),
            BridgeSubmitMode::CompletionBearing => deadline
                .bound(
                    "deliver.start_host_human_input",
                    member_id,
                    handle.start_host_human_input_with_options_bounded(
                        entry.agent_runtime_id,
                        entry.fence_token,
                        spec,
                        handling_mode,
                        options,
                        delivery_identity,
                        deadline.deadline.into_std(),
                    ),
                )
                .await?
                .map(Some),
        };
        return result.map_err(|error| match error {
            error @ (meerkat_mob::MobError::MemberReloadRequired { .. }
            | meerkat_mob::MobError::MemberAdmissionBacklogFull { .. }
            | meerkat_mob::MobError::ActorCommandTimedOut { .. }) => {
                classify_submit_mob_error(member_id, error, deadline)
            }
            error => BridgeError::HostHumanInput(HostHumanInputError::Mob(Box::new(error))),
        });
    }
    if matches!(mode, BridgeSubmitMode::CompletionBearing) {
        // meerkat refuses a completion sender for an autonomous_host member's
        // conversational work (`validate_member_turn_carriers`: "tracked turn
        // completion is not supported by autonomous inbox delivery"). Decide
        // from the LIVE roster entry, the value meerkat checks: it is fixed at
        // spawn and kept on restore, so it can differ from Meta's desired
        // spec (a same-profile override hot reload, a repair respawn that
        // drops the override, a profile Meta cannot read). Refused typed
        // BEFORE anything is submitted, so a tracked delivery falls back to
        // the ingress lane; meerkat's own refusal stays the backstop.
        if entry.runtime_mode == meerkat_mob::MobRuntimeMode::AutonomousHost {
            return Err(BridgeError::UnsupportedForMode {
                identity: member_id.clone(),
                mode: entry.runtime_mode,
                detail: meerkat_mob::MobError::UnsupportedForMode {
                    mode: entry.runtime_mode,
                    reason: "tracked turn completion is not supported by autonomous inbox \
                             delivery"
                        .to_string(),
                }
                .to_string(),
            });
        }
        // meerkat 0.8.43 has no deadline-taking `start_work*` variant, and it
        // needs none here: the stale-queued-work protection the
        // `submit_work*_bounded` verbs below give is reached the same way on
        // this path. On timeout `deadline.bound` DROPS the admission future;
        // `start_work*` sends its `SubmitWork` from a task spawned by
        // `meerkat_runtime::stack_relief::relieve_caller_stack`, whose
        // `AbortOnDrop` guard aborts that task and so drops the reply
        // receiver; and the mob actor skips a `SubmitWork` whose reply channel
        // is closed before dispatch, recording no ingress ("the caller left
        // between dequeue and dispatch", meerkat-mob actor.rs). The bounded
        // submit verbs reach the same skip by dropping their receiver on their
        // own reply deadline. Neither can retract an admission the actor has
        // already dispatched, so neither timeout proves nonexecution.
        let turn = match work.delivery_identity {
            Some(delivery_identity) => deadline
                .bound(
                    "deliver.start_work",
                    member_id,
                    handle.start_work_with_mode_and_delivery_identity(
                        entry.agent_runtime_id.clone(),
                        entry.fence_token,
                        spec,
                        handling_mode,
                        delivery_identity.clone(),
                    ),
                )
                .await?
                .map_err(|err| classify_submit_mob_error(member_id, err, deadline))?,
            None => deadline
                .bound(
                    "deliver.start_work",
                    member_id,
                    handle.start_work_with_mode(
                        entry.agent_runtime_id.clone(),
                        entry.fence_token,
                        WorkRef::new(),
                        spec,
                        handling_mode,
                    ),
                )
                .await?
                .map_err(|err| classify_submit_mob_error(member_id, err, deadline))?,
        };
        // Hand the handle back UNAWAITED. The caller awaits it only after the
        // admission-retry decision is settled, so a turn that ran and then
        // failed can never be mistaken for a delivery that never landed and
        // resubmitted - which would run the member's turn twice.
        return Ok(Some(turn));
    }
    // The upstream deadline can skip still-queued work, but cannot retract
    // admission already in progress. Neither deadline proves nonexecution.
    let actor_deadline = deadline.deadline.into_std();
    match work.delivery_identity {
        // meerkat's bounded delivery-identity submit is scope-bound: it pins
        // the member's current session and validates it inside admission.
        // This generic lane scopes to whatever the member is bound to right
        // now (a capture immediately before submit), which is the binding it
        // always targeted; hosts that persist a scope use the scoped
        // capability instead. A member with no session binding (a remotely
        // hosted peer) cannot be scope-bound, so it keeps the unscoped
        // delivery-identity submit, bounded by this admission deadline:
        // dropping it on timeout drops the reply receiver, and the actor
        // skips a SubmitWork whose caller left before dispatch.
        Some(delivery_identity) => match handle.capture_member_delivery_scope(member_id).await {
            Ok(scope) => deadline
                .bound(
                    "deliver.submit_work",
                    member_id,
                    handle.submit_work_with_mode_and_delivery_identity_bounded(
                        &scope,
                        spec,
                        handling_mode,
                        delivery_identity.clone(),
                        actor_deadline,
                    ),
                )
                .await?
                .map(|_| None)
                .map_err(|err| classify_submit_mob_error(member_id, err, deadline)),
            Err(meerkat_mob::MobError::DeliveryScopeUnavailable { .. }) => deadline
                .bound(
                    "deliver.submit_work",
                    member_id,
                    handle.submit_work_with_mode_and_delivery_identity(
                        entry.agent_runtime_id.clone(),
                        entry.fence_token,
                        spec,
                        handling_mode,
                        delivery_identity.clone(),
                    ),
                )
                .await?
                .map(|_| None)
                .map_err(|err| classify_submit_mob_error(member_id, err, deadline)),
            Err(err) => Err(classify_submit_mob_error(member_id, err, deadline)),
        },
        None => deadline
            .bound(
                "deliver.submit_work",
                member_id,
                handle.submit_work_with_mode_bounded(
                    entry.agent_runtime_id.clone(),
                    entry.fence_token,
                    WorkRef::new(),
                    spec,
                    handling_mode,
                    actor_deadline,
                ),
            )
            .await?
            .map(|_| None)
            .map_err(|err| classify_submit_mob_error(member_id, err, deadline)),
    }
}

// ---------------------------------------------------------------------------
// Committed-boundary heal seam (2026-07-29 heal/re-Break incident)
// ---------------------------------------------------------------------------

/// Typed outcome of a committed-boundary heal attempt against the durable
/// session head — the meta-side mirror of meerkat's
/// `CommittedBoundaryRecovery`.
///
/// The continuity repair supervisor consults this BEFORE re-registering a
/// Broken identity as healable. Without a real recovery step, "heal" only
/// reset the runtime entry while the durable head stayed an intra-turn
/// projection, so the next materialization re-Broke the identity — measured
/// in production (2026-07-29) as an infinite heal/re-Break loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedBoundaryRepair {
    /// The durable head is already strict-resume-acceptable; nothing written.
    AlreadyCommitted,
    /// Machine-authorized recovery persisted a committed boundary head; a
    /// subsequent resume of the durable session is expected to succeed.
    Recovered,
    /// Terminal typed verdict: the proof inputs for a committed boundary are
    /// absent (or the machine held). Stable across calls — callers must NOT
    /// retry-loop it; surface `reason` to operators instead.
    Unprovable { reason: String },
    /// The heal authority refused to read the durable head at all: the
    /// committed WholeBlob document fails meerkat's audited-endpoint guard
    /// (`SessionError::WholeBlobAuditedEndpointDivergence`). Rows intact, no
    /// retry can change it, the sanctioned `rkat session repair-wholeblob`
    /// repair is the only way forward. Callers park the identity with the
    /// typed repair hold instead of retrying recovery on a timer.
    RepairRequired {
        session_id: meerkat_core::types::SessionId,
        detail: String,
    },
    /// This bridge exposes no heal seam. Callers keep the legacy behavior
    /// (reconcile retries the resume directly).
    Unsupported,
}

/// Host-injectable authority that can drive the durable session head to a
/// strict-resume-acceptable committed boundary.
///
/// The production implementation wraps meerkat's
/// `PersistentSessionService::recover_committed_boundary`; it is injected
/// into [`MobSessionBridge`] at composition time because the bridge only
/// holds the session service as `dyn MobSessionService`, which does not
/// expose the concrete heal API.
#[async_trait]
pub trait CommittedBoundaryRecoverer: Send + Sync {
    /// The persistent session owner's exact completion reader, not a bootstrap
    /// adapter override whose live driver may not contain this input.
    fn runtime_completion_observer(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        None
    }

    /// Attempt recovery for the given durable session.
    ///
    /// # Errors
    ///
    /// `Err` is reserved for genuinely retryable failures (a live session
    /// owning the head mid-turn, store I/O, CAS races). A terminal verdict is
    /// `Ok(CommittedBoundaryRepair::Unprovable { .. })`, never an error.
    ///
    /// A session another runtime owner hosts, or whose hosting claim cannot
    /// be taken, is the typed retryable [`BridgeError::ServedElsewhere`] or
    /// [`BridgeError::HostingUnavailable`]: a hold that clears when that
    /// owner releases the session or the claim can be taken. Meta runs no
    /// repair supervisor that calls this, so the caller owns the backoff.
    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError>;
}

pub(crate) struct CompletionObservedRecoverer {
    pub(crate) inner: Option<Arc<dyn CommittedBoundaryRecoverer>>,
    pub(crate) observer: Arc<meerkat_runtime::MeerkatMachine>,
}

#[async_trait]
impl CommittedBoundaryRecoverer for CompletionObservedRecoverer {
    fn runtime_completion_observer(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        Some(Arc::clone(&self.observer))
    }

    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        match &self.inner {
            Some(inner) => inner.recover_committed_boundary(session_id).await,
            None => Ok(CommittedBoundaryRepair::Unsupported),
        }
    }
}

/// The production heal authority: meerkat's `PersistentSessionService`
/// driving the durable head to a boundary-commit-provenance checkpoint
/// through machine-authorized recovery (meerkat >= 0.8.11).
///
/// Contract mapping, per the heal API: the typed `Unprovable` VERDICT is
/// terminal and stable across calls (callers must not retry-loop it); only
/// the error tier (`Busy` mid-turn, store I/O, CAS races) is retryable and
/// maps to `Err` here.
#[async_trait]
impl<B> CommittedBoundaryRecoverer for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    fn runtime_completion_observer(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        crate::mob_handle_runtime::observed_runtime_adapter(self)
    }

    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        // Fully qualified: the inherent method and this trait method share a
        // name, and the inherent one is the upstream API being consumed.
        match meerkat_session::PersistentSessionService::recover_committed_boundary(
            self, session_id,
        )
        .await
        {
            Ok(meerkat_session::CommittedBoundaryRecovery::AlreadyCommitted) => {
                Ok(CommittedBoundaryRepair::AlreadyCommitted)
            }
            Ok(meerkat_session::CommittedBoundaryRecovery::Recovered { message_count }) => {
                tracing::info!(
                    %session_id,
                    message_count,
                    "machine-authorized recovery persisted a committed durable head"
                );
                Ok(CommittedBoundaryRepair::Recovered)
            }
            Ok(meerkat_session::CommittedBoundaryRecovery::Unprovable { reason }) => {
                Ok(CommittedBoundaryRepair::Unprovable { reason })
            }
            Err(error) => map_committed_boundary_recovery_error(error),
        }
    }
}

/// Disposition of the heal authority's error tier, per the heal contract:
/// `Err` is reserved for failures retrying can genuinely clear (`Busy`
/// mid-turn, store I/O, CAS races, a held tail awaiting the recovery commit
/// itself). Typed refusals that only an EXTERNAL change can clear — a
/// conflicting live runtime quiescing (`DurableTailRecoveryRefused`) or an
/// operator resolving forked evidence (`DurableEvidenceQuarantined`) — are
/// terminal `Unprovable` verdicts. Letting them escape as `Err` loops the
/// reconcile repair pass forever against a verdict no retry can change,
/// instead of parking the identity with the refusal in front of an operator.
///
/// The hosting holds differ, and stay `Err`: another runtime owner serving
/// the session (`ServedElsewhere`) or a hosting claim that cannot be taken
/// (`HostingUnavailable`) needs no operator. Each clears when the other owner
/// stops serving the session or the claim frees, and Core treats both as
/// retryable, like a busy session. They are typed
/// ([`BridgeError::ServedElsewhere`], [`BridgeError::HostingUnavailable`]) so
/// a caller can back off, and Meta runs no repair pass that would loop on
/// them.
fn map_committed_boundary_recovery_error(
    error: meerkat_core::SessionError,
) -> Result<CommittedBoundaryRepair, BridgeError> {
    match error {
        error @ (meerkat_core::SessionError::DurableTailRecoveryRefused { .. }
        | meerkat_core::SessionError::DurableEvidenceQuarantined { .. }) => {
            Ok(CommittedBoundaryRepair::Unprovable {
                reason: error.to_string(),
            })
        }
        // The one hold an operator must repair by hand: recovery cannot read
        // the committed document, so retrying recovery every pass is the
        // A downstream app's 2026-09-22 retry storm. Typed verdict, typed park.
        error @ meerkat_core::SessionError::WholeBlobAuditedEndpointDivergence { .. } => {
            let meerkat_core::SessionError::WholeBlobAuditedEndpointDivergence { id } = &error
            else {
                unreachable!("bound by the match arm above")
            };
            Ok(CommittedBoundaryRepair::RepairRequired {
                session_id: id.clone(),
                detail: error.to_string(),
            })
        }
        // Another owner hosts the session, or its hosting claim cannot be
        // taken: holds that clear on their own, so they stay retryable, but
        // typed, so a caller can back off instead of retrying every pass.
        meerkat_core::SessionError::ServedElsewhere { id } => {
            Err(BridgeError::ServedElsewhere { session_id: id })
        }
        meerkat_core::SessionError::HostingUnavailable { id } => {
            Err(BridgeError::HostingUnavailable { session_id: id })
        }
        error => Err(BridgeError::Mob(format!(
            "committed-boundary recovery: {error}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// SessionBridge trait
// ---------------------------------------------------------------------------

/// Which mob door a governed delivery enters: the external door every
/// caller uses, or the internal door of the trusted host's own retained input
/// to a member it composed. Chosen only by the identity runtime's two
/// governed entry points, never by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GovernedDoor {
    External,
    TrustedHostInternal,
}

/// Why a governed send did not return an admitted turn. Whether the input
/// was admitted is the native admission custody's settlement (see
/// [`GovernedAdmissionRecord`]), never this error's kind; the identity
/// runtime reports any submission it cannot show was not admitted as
/// [`Self::AdmissionUnresolved`]. A native refusal carries the native verdict
/// unchanged.
#[derive(Debug)]
pub enum GovernedInputError {
    /// The member cannot take governed input (for example it is not a local
    /// session-backed member, or no bridge or runtime is bound).
    Unsupported(String),
    /// The member's session changed or is missing; the input was not
    /// retargeted.
    BindingChanged,
    /// The trusted host binder could not bind the input for the member's
    /// session.
    Bind(String),
    /// The input reached admission without its authority association or
    /// trusted ingress observation.
    Unbound,
    /// The mob or native admission answered the submitted input with an
    /// error. The error alone does not show whether the input was admitted:
    /// only the native custody settlement does, and the identity runtime
    /// reports this variant only after a `NotAdmitted` settlement.
    Mob(Box<meerkat_mob::MobError>),
    /// The submitted input's native admission is not known to have refused
    /// it: its custody settled `Admitted` or `Uncertain` without an accepted
    /// acknowledgement, or no settlement was observed.
    /// The input may have run. The host's settlement custody was neither
    /// promoted nor withdrawn: it was handed to the binder's
    /// [`GovernedInputBinder::retain_unresolved`].
    AdmissionUnresolved { detail: String },
    /// The bound input's host entry fence refused it at the final protected
    /// entry, immediately before native admission: nothing was submitted and
    /// the host's settlement custody was withdrawn.
    EntryRefused(GovernedEntryRefusal),
}

pub use meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement;

/// The host side of the member session's native admission custody for one
/// governed submission. Its custody object is a thin, synchronous,
/// infallible adapter: native admission settles it exactly once
/// (`NotAdmitted`, `Admitted` or `Uncertain`), possibly under native guards,
/// and it only records that settlement. The owning operation reads it after
/// native admission returns and acts on it outside the native guards. No
/// settlement recorded (a lost result) reads as `None`.
#[derive(Clone, Default)]
pub struct GovernedAdmissionRecord(Arc<std::sync::Mutex<Option<NativeInputAdmissionSettlement>>>);

impl GovernedAdmissionRecord {
    /// The custody to hand to native admission for this submission, already
    /// guarded (see [`GovernedDeliveryCustody`]).
    pub fn custody(&self) -> GovernedDeliveryCustody {
        GovernedDeliveryCustody::new(Box::new(RecordingCustody(Arc::clone(&self.0))))
    }

    /// The native settlement, once native admission settled the custody.
    pub fn settlement(&self) -> Option<NativeInputAdmissionSettlement> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct RecordingCustody(Arc<std::sync::Mutex<Option<NativeInputAdmissionSettlement>>>);

impl meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody for RecordingCustody {
    fn settle(
        self: Box<Self>,
        settlement: NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(settlement);
        None
    }
}

/// A native admission custody guarded from before a governed delivery is
/// called until it is handed to native admission. It is the
/// [`SessionBridge::begin_governed_delivery`] argument, so the guard exists
/// before the delivery future does: dropping that future before it is first
/// polled, or any answer before submission, settles the custody
/// `NotAdmitted` exactly once. Once submitted, only native admission
/// settles it.
pub struct GovernedDeliveryCustody {
    custody: Option<Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>>,
    entry: Option<(Arc<dyn GovernedEntryFence>, GovernedEntryRecord)>,
}

impl GovernedDeliveryCustody {
    pub fn new(
        custody: Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>,
    ) -> Self {
        Self {
            custody: Some(custody),
            entry: None,
        }
    }

    /// Guard [`Self::submit`] with the bound input's host entry fence; its
    /// entry is recorded in `record`.
    pub(crate) fn with_entry_fence(
        mut self,
        fence: Option<Arc<dyn GovernedEntryFence>>,
        record: &GovernedEntryRecord,
    ) -> Self {
        self.entry = fence.map(|fence| (fence, record.clone()));
        self
    }

    /// Hand the custody to native admission, which then owns its settlement.
    ///
    /// This is the final protected entry, after every member lookup and
    /// immediately before admission: a bound input's host entry fence is
    /// entered here, synchronously. A refused fence keeps the custody, which
    /// settles `NotAdmitted` as it is dropped: nothing is submitted.
    pub fn submit(
        mut self,
    ) -> Option<Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>>
    {
        self.custody.as_ref()?;
        if let Some((fence, record)) = self.entry.take() {
            match fence.enter() {
                Ok(permit) => record.entered(permit),
                Err(refusal) => {
                    record.refused(refusal);
                    return None;
                }
            }
        }
        self.custody.take()
    }
}

impl std::fmt::Debug for GovernedDeliveryCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedDeliveryCustody")
            .field("held", &self.custody.is_some())
            .field("fenced", &self.entry.is_some())
            .finish()
    }
}

impl Drop for GovernedDeliveryCustody {
    fn drop(&mut self) {
        if let Some(custody) = self.custody.take() {
            let _ = custody.settle(NativeInputAdmissionSettlement::NotAdmitted);
        }
    }
}

impl std::fmt::Display for GovernedInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "governed input unsupported: {reason}"),
            Self::BindingChanged => f.write_str(
                "governed input binding changed or is missing; input was not retargeted",
            ),
            Self::Bind(reason) => write!(f, "governed input could not be bound: {reason}"),
            Self::Unbound => f.write_str(
                "governed input carries no authority association or ingress observation",
            ),
            Self::Mob(error) => write!(f, "governed input refused: {error}"),
            Self::AdmissionUnresolved { detail } => write!(
                f,
                "governed input admission is unresolved: {detail}; the input may have run \
                 and its settlement is retained unsettled by the host"
            ),
            Self::EntryRefused(refusal) => {
                write!(f, "governed input refused at its final entry: {refusal}")
            }
        }
    }
}

impl std::error::Error for GovernedInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mob(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

/// What the trusted host binds one governed delivery for: the member's exact
/// current local session and that session actor's actual controller client,
/// pinned by the session owner (never rebuilt from a serialized selection),
/// and the identity's REGISTERED roster row.
///
/// The row is read with the Active/session check, under the identity's
/// lifecycle lock, which this delivery holds through native submission and
/// which every row writer takes: a host that compares it with the row its
/// input was authored from, inside [`GovernedInputBinder::bind`], checks the
/// row the input is admitted under, and refusing there submits nothing. It
/// is the registered row, not the roster provider's desired row and not
/// proof that the member's actor adopted every field. `bind` must not await
/// any operation on the same identity that takes its lifecycle lock (a
/// reset, send, materialization or row update): that lock is held here.
pub struct GovernedBindContext {
    pub session_id: meerkat_core::types::SessionId,
    pub controller: meerkat_core::ControllerModelClient,
    /// The full registered row, including `additional_instructions`.
    pub spec: super::types::DurableAgentSpec,
}

/// The member session's native admission acknowledgement of one governed
/// input: the admitting session, its canonical admitted input (the existing
/// one when native admission deduplicated it) and whether it was
/// deduplicated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernedAdmission {
    pub session_id: meerkat_core::types::SessionId,
    pub input_id: meerkat_core::lifecycle::InputId,
    pub deduplicated: bool,
}

/// The host's settlement custody for one bound governed input (for example
/// its pending authenticated original). Meta calls [`Self::accepted`]
/// exactly once, only when the member's native admission ACCEPTED that exact
/// input as new work. A native replay (deduplicated onto an existing input)
/// never settles it: the retry's custody is dropped, and the canonical
/// existing input is reported in [`GovernedAdmission`]. Dropping it
/// unsettled is the host's withdrawal; Meta drops it only on a definite
/// answer that leaves no new admitted work. When admission is unresolved it
/// is handed to [`GovernedInputBinder::retain_unresolved`] instead.
pub trait GovernedSettlement: Send + Sync {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String>;
}

/// One bound governed input with its host settlement custody.
pub struct BoundGovernedInput {
    pub input: meerkat_runtime::Input,
    pub settlement: Box<dyn GovernedSettlement>,
    /// The host's final-entry fence for this input (for example the exact
    /// retained login it was bound for), entered at the final protected
    /// entry immediately before native admission. `None` for a host without
    /// one: delivery is unchanged.
    pub entry_fence: Option<Arc<dyn GovernedEntryFence>>,
}

/// A host's final-entry fence for one bound governed input.
///
/// Native calls [`Self::enter`] at the final protected entry: after the
/// binder returned and after every member lookup, immediately before the
/// custody is handed to native admission, under the identity's lifecycle
/// custody. A successful `enter` reserves this input's admission attempt; it
/// is not yet an entered result. `enter`, and the permit's `finish` and drop,
/// update only the fence's local reservation bookkeeping under a short local
/// lock that also orders a close against entry: they must not await, call
/// back into the runtime, take an identity lease or reach a remote process.
/// An owner with an absolute expiry evaluates it in `enter` itself.
///
/// Closing the fence (the host's retirement) and `enter` are ordered by the
/// fence: a close that wins makes every later `enter` refuse, so that
/// operation is unentered; a reservation that wins proceeds to native
/// admission, and the close waits only for that admission decision, never
/// for a turn. A close may observe: the fence is closed, the reservations
/// taken before it drained, and whether any reported
/// [`GovernedEntryOutcome::Uncertain`] (or was dropped unreported). It must
/// not claim completed cleanup or that a reserved attempt was not admitted
/// later. Retiring one fence never cancels unrelated accepted work.
///
/// This fence guards only native admission of the bound input. Protected
/// effects a host takes before it returns the bound input (for example
/// issuing a controller-child grant or publishing a binding inside its
/// binder) and its presenter or account entries are not covered: the host
/// orders them against the same retained owner itself, through their own
/// owners.
pub trait GovernedEntryFence: Send + Sync {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal>;
}

/// One reserved admission attempt inside a [`GovernedEntryFence`]. Native
/// reports exactly one outcome when the admission's decision is known; a
/// permit dropped unreported means [`GovernedEntryOutcome::Uncertain`]:
/// fenced but unsettled, never withdrawn or finished. `finish` and the
/// permit's drop run while the identity's lifecycle custody is held and obey
/// the same restriction as [`GovernedEntryFence::enter`]: local, short, no
/// await, no reentry, no remote call. A retirement that observed an
/// `Uncertain` or dropped permit stays qualified as unsettled and is never
/// reported as completed cleanup. The outcome releases
/// the fence's wait for this admission decision only; the input's settlement
/// stays with its existing native and host custody.
pub trait GovernedEntryPermit: Send {
    fn finish(self: Box<Self>, outcome: GovernedEntryOutcome);
}

/// How one entered use of a fence settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernedEntryOutcome {
    /// Native admission did not admit it (`NotAdmitted`): unentered.
    Unentered,
    /// Native admission admitted it: new work, a native replay of the
    /// canonical input, or admitted work whose host promotion then failed.
    /// Its settlement stays with its existing owner.
    Entered,
    /// Native admission is unresolved (not failed, not cancelled): the
    /// input may have run, and the host's custody went to
    /// [`GovernedInputBinder::retain_unresolved`].
    Uncertain,
}

/// Why a fence refused entry. Each is distinct: an unavailable owner is not
/// proof of retirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernedEntryRefusal {
    /// The host knows the fence's owner retired or closed.
    Retired,
    /// The owner's own absolute expiry passed.
    Expired,
    /// The owner is unreachable (for example its transport was lost). Not
    /// proof of retirement.
    Unavailable(String),
    /// The caller's authentication or access no longer holds at the entry
    /// (for example a Console human's token or access was revoked).
    NotAuthorized(String),
}

impl std::fmt::Display for GovernedEntryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retired => f.write_str("the entry owner retired"),
            Self::Expired => f.write_str("the entry owner expired"),
            Self::Unavailable(detail) => write!(f, "the entry owner is unavailable: {detail}"),
            Self::NotAuthorized(detail) => write!(f, "the caller is not authorized: {detail}"),
        }
    }
}

/// What happened at one submission's final entry: nothing (no fence or no
/// submission), a permit held until its outcome is reported, or a refusal.
#[derive(Clone, Default)]
pub(crate) struct GovernedEntryRecord(Arc<std::sync::Mutex<GovernedEntryState>>);

#[derive(Default)]
enum GovernedEntryState {
    #[default]
    NotEntered,
    Entered(Box<dyn GovernedEntryPermit>),
    Refused(GovernedEntryRefusal),
    Finished,
}

impl GovernedEntryRecord {
    fn state(&self) -> std::sync::MutexGuard<'_, GovernedEntryState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn entered(&self, permit: Box<dyn GovernedEntryPermit>) {
        *self.state() = GovernedEntryState::Entered(permit);
    }

    fn refused(&self, refusal: GovernedEntryRefusal) {
        *self.state() = GovernedEntryState::Refused(refusal);
    }

    /// The fence's refusal, when the final entry was refused.
    pub(crate) fn refusal(&self) -> Option<GovernedEntryRefusal> {
        match &*self.state() {
            GovernedEntryState::Refused(refusal) => Some(refusal.clone()),
            _ => None,
        }
    }

    /// Report the entered use's outcome, once; nothing when no permit was
    /// taken.
    pub(crate) fn finish(&self, outcome: GovernedEntryOutcome) {
        let state = std::mem::replace(&mut *self.state(), GovernedEntryState::Finished);
        match state {
            GovernedEntryState::Entered(permit) => permit.finish(outcome),
            other => *self.state() = other,
        }
    }
}

/// The trusted host's binder for one governed delivery: it binds the raw
/// input for the member's exact current session (the session's scoped
/// destination on its current registration, the host's authenticated
/// original and the context's pinned controller), as a stock host binds a
/// prompt before acceptance, and returns it with its settlement custody.
/// Meta never mints, alters, drops or retargets the association; it carries
/// the bound input to the member's native admission under the identity's
/// lease and settles the custody from that admission's answer.
#[async_trait]
pub trait GovernedInputBinder: Send + Sync {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, String>;

    /// Take back custody whose submission's native admission is unresolved,
    /// to keep it until the host can settle it from the native owner's
    /// actual record. Called from the runtime-owned operation, whether or
    /// not its caller is still waiting.
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str);
}

/// The session owner's actor operations, captured where the concrete session
/// service is still known (the erased mob session service carries none):
/// the controller pin, and the live MCP attachment to the current member.
#[async_trait]
pub trait ControllerPinSource: Send + Sync {
    /// The session's current live actor, without waiting on its turn.
    async fn live_actor(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_session::LiveSessionActorWitness>;

    /// The actual controller client of exactly that actor. The owner
    /// establishes its own actor boundary; an actor replaced meanwhile is
    /// refused.
    async fn pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, String>;

    /// [`Self::pin_actor`] for a caller that must never wait on a member
    /// turn, such as the comms receive path. The owner only tries the
    /// actor's turn boundary: a held boundary refuses
    /// [`TryPinControllerError::TurnBoundaryHeld`](meerkat_session::TryPinControllerError::TurnBoundaryHeld)
    /// at once and nothing is asked of the actor; an actor that is gone,
    /// replaced or failed refuses
    /// [`TryPinControllerError::Session`](meerkat_session::TryPinControllerError::Session).
    /// An owner without a non-waiting pin refuses with
    /// `Session(SessionError::Unsupported)`, never with a client.
    async fn try_pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, meerkat_session::TryPinControllerError> {
        let _ = actor;
        Err(meerkat_session::TryPinControllerError::Session(
            meerkat_core::service::SessionError::Unsupported(
                "this session owner has no non-waiting controller pin".to_string(),
            ),
        ))
    }

    /// Attach one reviewed MCP server to the session's current live actor,
    /// through that actor. It never waits for a held turn boundary: `Busy`
    /// means that boundary is held (not necessarily an active model turn)
    /// and grants nothing; whether to try again is the caller's owner's
    /// decision. `Refused(NotCurrent)` without a live actor. It is not
    /// otherwise wait-free: the actor stages and applies the add before it
    /// answers. An owner without such an entry refuses as unsupported. The
    /// receipt carries the exact attempt, observed only through that actor.
    async fn try_attach_live_mcp_server(
        &self,
        session_id: &meerkat_core::types::SessionId,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        let _ = (session_id, attachment);
        Err(meerkat_core::mcp_attachment::LiveMcpAttachError::Unsupported)
    }
}

#[async_trait]
impl<B> ControllerPinSource for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn live_actor(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_session::LiveSessionActorWitness> {
        self.live_session_actor_witness(session_id).await
    }

    async fn pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, String> {
        let lease = self
            .acquire_live_session_actor_turn_boundary_lease_exact(actor)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the session actor was replaced".to_string())?;
        self.pin_controller_client_for_actor(&lease)
            .await
            .map_err(|error| error.to_string())
    }

    async fn try_pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, meerkat_session::TryPinControllerError> {
        meerkat_session::PersistentSessionService::try_pin_controller_client_for_actor(self, actor)
            .await
    }

    // This entry does not wait for a held turn boundary: one answers `Busy`
    // at once, so a caller inside the member's own turn cannot wait on
    // itself. The attach that follows still awaits the actor.
    async fn try_attach_live_mcp_server(
        &self,
        session_id: &meerkat_core::types::SessionId,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        meerkat_session::PersistentSessionService::try_attach_live_mcp_server(
            self, session_id, attachment,
        )
        .await
    }
}

#[async_trait]
impl<B> ControllerPinSource for meerkat_session::EphemeralSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn live_actor(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_session::LiveSessionActorWitness> {
        self.live_session_actor_witness(session_id).await
    }

    async fn pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, String> {
        self.pin_controller_client_for_actor(actor)
            .await
            .map_err(|error| error.to_string())
    }

    // The session service has a non-waiting pin only off wasm32; there the
    // trait's unsupported refusal stands.
    #[cfg(not(target_arch = "wasm32"))]
    async fn try_pin_actor(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, meerkat_session::TryPinControllerError> {
        meerkat_session::EphemeralSessionService::try_pin_controller_client_for_actor(self, actor)
            .await
    }
}

/// A governed delivery the member's native admission answered: the receipt
/// that reports the turn's own output, and the admission acknowledgement.
pub struct GovernedDelivery {
    pub receipt: BridgeTurnReceipt,
    pub admission: Option<GovernedAdmission>,
}

/// Human designation is trusted-host-only, never inferred from prompt text.
/// Upstream strict replay and completion refusals retain their typed variants.
#[derive(Debug)]
pub enum HostHumanInputError {
    Unsupported(String),
    BindingChanged,
    /// The console retained the spent reservation, but not an exact prepared
    /// runtime payload. Re-preparing could change memory or the incarnation.
    ReplayUnavailable {
        interaction_id: String,
        input_frame_id: String,
    },
    Mob(Box<meerkat_mob::MobError>),
}

impl std::fmt::Display for HostHumanInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "host human input unsupported: {reason}"),
            Self::BindingChanged => f.write_str(
                "host human input binding changed or is missing; input was not retargeted",
            ),
            Self::ReplayUnavailable { .. } => f.write_str(
                "host human input admission fate is unavailable; original reservation retained, \
                 no retry was submitted",
            ),
            Self::Mob(error) => write!(f, "host human input: {error}"),
        }
    }
}

impl std::error::Error for HostHumanInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mob(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl HostHumanInputError {
    pub fn structured_data(&self) -> serde_json::Value {
        use meerkat_mob::MobError;
        let kind = match self {
            Self::ReplayUnavailable {
                interaction_id,
                input_frame_id,
            } => {
                return serde_json::json!({
                    "kind": "host_human_input_replay_unavailable",
                    "interaction_id": interaction_id,
                    "input_frame_id": input_frame_id,
                    "execution_fate": "unknown",
                    "retry_submitted": false,
                });
            }
            Self::Unsupported(_) => "host_human_input_unsupported",
            Self::BindingChanged => "host_human_input_binding_changed",
            Self::Mob(error) => match error.as_ref() {
                MobError::WorkInputIdempotencyConflict {
                    session_id,
                    input_id,
                } => {
                    return serde_json::json!({
                        "kind": "work_input_idempotency_conflict",
                        "session_id": session_id, "input_id": input_id,
                    });
                }
                MobError::WorkInputCompletionUnavailable {
                    session_id,
                    input_id,
                } => {
                    return serde_json::json!({
                        "kind": "work_input_completion_unavailable",
                        "session_id": session_id, "input_id": input_id,
                    });
                }
                MobError::UnsupportedForMode { .. } => "host_human_input_unsupported",
                MobError::InjectedContextUndeliverable { .. } => "injected_context_undeliverable",
                MobError::ActorCommandTimedOut {
                    command_kind,
                    stage,
                } => {
                    return serde_json::json!({
                        "kind": "admission_timeout", "command_kind": command_kind,
                        "stage": stage, "execution_fate": "unknown",
                    });
                }
                _ => "host_human_input_refused",
            },
        };
        serde_json::json!({ "kind": kind })
    }
}

/// One fully-admitted member delivery: everything a bridge needs to hand a
/// turn to the mob work lane, as ONE typed request instead of a growing
/// parameter staircase. The identity runtime's shared delivery preparation
/// (defang, taint session attribution, ambient memory injection) is exactly
/// the code that POPULATES this request before
/// [`SessionBridge::deliver_admitted`].
///
/// `delivery_identity` is the meerkat 0.8.15 internal-lane dedup carrier
/// (idempotency key + occurrence-correlation UUID, validated upstream). Its
/// semantics are fail-closed end to end: the runtime refuses half-formed or
/// non-canonical pairs typed BEFORE admission, and an implementation that
/// cannot honor a present identity must refuse typed rather than deliver
/// without dedup. When the identity is present its correlation id also
/// rides as `interaction_id`.
#[derive(Debug, Clone)]
pub struct BridgeDelivery {
    pub content: meerkat_core::ContentInput,
    pub handling_mode: HandlingMode,
    /// Per-turn System message (meerkat 0.8.11 `WorkSpec::system_prompt`).
    pub system_prompt: Option<String>,
    /// Typed ambient injection bodies delivered alongside - never fused
    /// into - the user content (meerkat 0.7.12 ask 1).
    pub injected_context: Vec<meerkat_core::ContentInput>,
    /// Host-minted interaction id threaded into runtime admission
    /// (meerkat 0.7.25 ask 15 addendum).
    pub interaction_id: Option<String>,
    /// Internal-lane dedup identity (see the struct docs).
    pub delivery_identity: Option<meerkat_mob::MobDeliveryIdentity>,
    /// Selected skills for this exact turn. Only the explicit local
    /// host-human lane carries them (as native member-turn skill
    /// references); every other lane refuses a nonempty selection typed
    /// instead of delivering the turn without it.
    pub skill_references: Vec<meerkat_core::skills::SkillKey>,
}

impl BridgeDelivery {
    pub fn new(content: meerkat_core::ContentInput, handling_mode: HandlingMode) -> Self {
        Self {
            content,
            handling_mode,
            system_prompt: None,
            injected_context: Vec::new(),
            interaction_id: None,
            delivery_identity: None,
            skill_references: Vec::new(),
        }
    }
}

/// An ADMITTED turn, owned by the caller and awaitable OUTSIDE any lock.
///
/// This type exists for one reason: the identity lifecycle lock must cover
/// validation, admission and bounded session resolution, and NOTHING else.
/// Awaiting an LLM turn under that lock serialises same-identity sends behind a
/// model call and blocks every lifecycle operation - reset, retire, alias
/// rebind - for the turn's whole duration.
///
/// [`Self::wait`] takes `self` BY VALUE, which prevents awaiting one twice.
///
/// It does NOT make dropping one impossible: a receipt can still be dropped
/// without being awaited, abandoning a running turn. `#[must_use]` makes the
/// common case of ignoring the return value a warning, and that is the whole of
/// the protection - the rest is the caller's discipline.
#[must_use = "dropping a BridgeTurnReceipt abandons an ADMITTED, RUNNING turn without ever \
              observing its terminal"]
pub struct BridgeTurnReceipt {
    /// Session resolution ALREADY RAN, under the admission deadline, before the
    /// caller was handed this receipt.
    ///
    /// Carried as a `Result` and deliberately NOT `?`-propagated at admission
    /// time: returning early on a resolution failure would drop the completion
    /// handle of a turn that is already running. The pair is mapped in
    /// [`Self::wait`], after the turn has reached terminal.
    session_result: Result<meerkat_core::types::SessionId, String>,
    /// `'static` because the receipt is held across an UNLOCK, so it must not
    /// borrow from the bridge or from any guard. `Send` because the runtime's
    /// send future must stay `Send`. Yields the turn's own output when the
    /// receipt was built with [`Self::with_output`], `None` otherwise.
    completion: std::pin::Pin<
        Box<dyn Future<Output = Result<Option<super::types::TurnOutput>, String>> + Send + 'static>,
    >,
}

impl std::fmt::Debug for BridgeTurnReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeTurnReceipt")
            .field("session_result", &self.session_result)
            .finish_non_exhaustive()
    }
}

impl BridgeTurnReceipt {
    /// Build a receipt from an admitted turn.
    ///
    /// Generic over the completion future so out-of-crate [`SessionBridge`]
    /// implementors can build one from whatever handle they own, without
    /// naming a boxed type.
    ///
    /// Construct this the moment the work is admitted and BEFORE inspecting
    /// `session_result`, so no code path can inspect a failure and return
    /// without carrying the completion handle forward.
    pub fn new<F, ResolutionError, CompletionError>(
        session_result: Result<meerkat_core::types::SessionId, ResolutionError>,
        completion: F,
    ) -> Self
    where
        F: Future<Output = Result<(), CompletionError>> + Send + 'static,
        ResolutionError: std::fmt::Display,
        CompletionError: std::fmt::Display,
    {
        Self {
            session_result: session_result.map_err(|error| error.to_string()),
            completion: Box::pin(async move {
                completion
                    .await
                    .map(|()| None)
                    .map_err(|error| error.to_string())
            }),
        }
    }

    /// [`Self::new`] for a completion that also yields the admitted turn's own
    /// output, read by [`Self::wait_with_output`].
    pub fn with_output<F, ResolutionError, CompletionError>(
        session_result: Result<meerkat_core::types::SessionId, ResolutionError>,
        completion: F,
    ) -> Self
    where
        F: Future<Output = Result<super::types::TurnOutput, CompletionError>> + Send + 'static,
        ResolutionError: std::fmt::Display,
        CompletionError: std::fmt::Display,
    {
        Self {
            session_result: session_result.map_err(|error| error.to_string()),
            completion: Box::pin(async move {
                completion
                    .await
                    .map(Some)
                    .map_err(|error| error.to_string())
            }),
        }
    }

    /// The session this turn was admitted onto, if resolution succeeded.
    ///
    /// Read-only: the field is private so no caller can inspect resolution,
    /// decide it failed, and return without awaiting the running turn.
    pub fn resolved_session(&self) -> Option<&meerkat_core::types::SessionId> {
        self.session_result.as_ref().ok()
    }

    /// Why post-admission session resolution failed, if it did. Read-only,
    /// like [`Self::resolved_session`].
    pub fn session_resolution_error(&self) -> Option<&str> {
        self.session_result.as_ref().err().map(String::as_str)
    }

    /// Await this turn's terminal and return its own outcome and output,
    /// INDEPENDENT of post-admission session resolution.
    ///
    /// The per-turn output comes from the turn's exact completion and never
    /// needs the session id; resolution only feeds session reconciliation, and
    /// a caller that reads it separately ([`Self::session_resolution_error`])
    /// reports that failure on its own channel. So a turn that ran and
    /// succeeded is reported as such even when its session could not be
    /// projected, instead of as a failure a retrying caller would run twice.
    pub async fn wait_turn_output(self) -> Result<super::types::TurnOutput, String> {
        self.completion
            .await
            .map(|output| output.unwrap_or(super::types::TurnOutput::Unavailable))
    }

    /// Await this turn's terminal, then map the (resolution, terminal) pair.
    ///
    /// | resolution | terminal | result                                        |
    /// |------------|----------|-----------------------------------------------|
    /// | `Ok`       | `Ok`     | `Ok(session_id)`                              |
    /// | `Ok`       | `Err`    | [`BridgeTurnError::CompletionFailed`]         |
    /// | `Err`      | `Err`    | `CompletionFailed`, carrying both details     |
    /// | `Err`      | `Ok`     | [`BridgeTurnError::PostAdmissionResolutionFailed`]|
    ///
    /// There is no early return: the turn is already running, so its terminal
    /// is always awaited before any error is produced.
    pub async fn wait(self) -> Result<meerkat_core::types::SessionId, BridgeTurnError> {
        self.wait_with_output()
            .await
            .map(|(session_id, _output)| session_id)
    }

    /// [`Self::wait`], also returning the turn's own output
    /// ([`TurnOutput::Unavailable`](super::types::TurnOutput::Unavailable)
    /// when the receipt was built with [`Self::new`], i.e. the bridge cannot
    /// report per-turn output). Same (resolution, terminal) mapping.
    pub async fn wait_with_output(
        self,
    ) -> Result<(meerkat_core::types::SessionId, super::types::TurnOutput), BridgeTurnError> {
        let terminal_result = self.completion.await;
        match (self.session_result, terminal_result) {
            (Ok(session_id), Ok(output)) => Ok((
                session_id,
                output.unwrap_or(super::types::TurnOutput::Unavailable),
            )),
            (Ok(_), Err(err)) => Err(BridgeTurnError::CompletionFailed(err)),
            (Err(resolve_err), Err(err)) => Err(BridgeTurnError::CompletionFailed(format!(
                "{err}; post-admission session resolution also failed: {resolve_err}"
            ))),
            (Err(resolve_err), Ok(_)) => {
                Err(BridgeTurnError::PostAdmissionResolutionFailed(resolve_err))
            }
        }
    }
}

/// The runtime binding Core committed for a member: its incarnation and the
/// bridge session bound to it.
///
/// Returned by [`SessionBridge::create_session`] and
/// [`SessionBridge::reset_member_to_successor`], and carried by every
/// [`ResumeSessionOutcome`], rather than pre-computed by the caller: MobMachine
/// mints the generation and the session, so only the bridge learns what they
/// actually are. The bridge's later calls for the member take this
/// `agent_runtime_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRuntimeBinding {
    /// The incarnation MobMachine minted, in its public `rt:` spelling.
    pub agent_runtime_id: AgentRuntimeId,
    /// The bridge session bound to that incarnation. `None` only for a
    /// peer-only (externally bound) member, which Core binds to no session.
    pub session_id: Option<meerkat_core::types::SessionId>,
}

/// Bridge between the identity-first control plane and the Meerkat session
/// pipeline. Each method maps an identity-layer operation to its concrete
/// mob-level counterpart.
#[async_trait]
pub trait SessionBridge: Send + Sync {
    /// Whether the lower mob plane already contains a caller-owned raw member
    /// whose public alias collides with a durable identity. Identity restore
    /// uses this under the shared namespace reservation before publishing or
    /// materializing a durable owner.
    async fn raw_member_alias_exists(&self, _alias: &str) -> Result<bool, BridgeError> {
        Ok(false)
    }

    /// Install the actor-loop probe's shared verdict. The concrete mob bridge
    /// fails admission round trips fast while a stall is open; bridges that do
    /// not touch the mob actor ignore it.
    fn observe_actor_loop_health(&self, _health: Arc<ActorLoopHealth>) {}

    /// meerkat's non-destructive member registration reload
    /// (`MobHandle::reload_member_registration`, 0.8.34). `Ok(None)` means
    /// this bridge exposes no such primitive and the identity runtime falls
    /// back to retire-then-rematerialize. The identity's continuity binding is
    /// never touched by this call.
    async fn reload_member_registration(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        Ok(None)
    }

    /// Release the `ToolsNotPublished` run-start hold the mob build placed on
    /// a restored member whose customizer tools were not published before the
    /// build (#563): its tools are published now, so it may start runs.
    /// Bridges without run-start holds have nothing to release. A member that
    /// is not in the mob, or does not hold the reason, is a no-op.
    async fn release_member_run_starts_for_published_tools(
        &self,
        _member_id: &MobAgentIdentity,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// meerkat's durability verdict for the member's live registration
    /// (`MeerkatMachine::durability_reload_required`, 0.8.34), or `None` when
    /// this bridge cannot observe it.
    async fn member_durability(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Option<super::types::MemberDurability> {
        None
    }

    /// The exact Core registration occupying `session_id` now
    /// (`MeerkatMachine::current_session_registration_witness`), or `None`
    /// when the session holds none or this bridge cannot observe it. Core
    /// hands out only a current registration, so a caller that needs an
    /// incarnation's registration reads it before that incarnation ends.
    async fn member_registration(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_runtime::RuntimeSessionRegistrationWitness> {
        None
    }

    /// Spawn a new mob member for a freshly-created identity.
    ///
    /// The caller names no runtime id and no session: MobMachine mints both,
    /// and the result is the binding it committed. The bridge's later calls
    /// for the member take that `agent_runtime_id`.
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError>;

    /// [`Self::create_session`] for a fresh identity of a held bootstrap
    /// `pass`: the member is spawned held within the pass's setup deadline,
    /// reports its finished surface and its actor's controller route
    /// observation to the pass and waits for the pass's terminal decision.
    /// On the pass's `Activate` the member's own activation is accepted or
    /// expired by Core against the same deadline: accepted, it is published
    /// and the create completes as an ordinary create with its actual
    /// outcome. Expired, aborted, or not held by the deadline, the create
    /// fails once Core has settled the member (its cleanup outcome kept in
    /// the error); Core never published it, so the identity gains no binding.
    /// A bridge that cannot hold a member refuses.
    async fn create_session_held(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        pass: &super::held_bootstrap::HeldCreatePass,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let _ = (spec, draft, pass);
        Err(BridgeError::Mob(format!(
            "this bridge cannot hold the member it creates for {identity}"
        )))
    }

    /// Replace a member with its successor as ONE authoritative transition.
    ///
    /// Meta reset is destructive: it advances the continuity generation and
    /// creates fresh continuity and session. Under the durable-roster contract
    /// the successor occupies the SAME roster row, so this cannot be a create
    /// (it would collide with its own predecessor) and must not be composed
    /// from `retire_member` plus `create_session` either - splitting the
    /// transition reopens the compensation races that a single authority
    /// operation avoids.
    ///
    /// Meerkat's respawn is that single operation: it snapshots the stable
    /// identity, computes the machine-owned successor generation, terminally
    /// retires the old row, and spawns the exact successor under the same
    /// roster identity.
    ///
    /// Implementations return the CONCRETE binding they committed. The caller
    /// records its continuity from that value instead of pre-computing a
    /// runtime id and session id and telling the bridge what they will be.
    ///
    /// The default fails closed on purpose. A custom bridge has to implement
    /// this deliberately, because quietly composing retire and create would
    /// look like it worked while reintroducing exactly the race this exists to
    /// remove.
    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::InvalidInput(format!(
            "this SessionBridge does not implement destructive reset successors, so identity \
             {identity} cannot be reset: the transition must be one authoritative respawn and \
             must not be composed from retire plus create"
        )))
    }

    /// Capture the inputs other senders queued on `identity`'s member, whose
    /// session is `session_id`, that are still waiting to run: a fresh life
    /// replaces the member, and its queue would go with it. Each pending
    /// input that cannot be carried is logged by id. An `Err` means the
    /// queue could not be observed, so the caller must not replace the
    /// member. The default captures nothing.
    async fn capture_queued_member_inputs(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<QueuedMemberInputs, BridgeError> {
        Ok(QueuedMemberInputs::default())
    }

    /// Admit the inputs [`Self::capture_queued_member_inputs`] captured to
    /// `identity`'s fresh member on `session_id`, in their admission order.
    /// Each input the fresh member does not accept is logged by id. The
    /// default admits nothing.
    async fn readmit_queued_member_inputs(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _inputs: QueuedMemberInputs,
    ) {
    }

    /// Resume `identity` onto its committed session.
    ///
    /// `session_id` is the session Core last bound to the identity; the
    /// bridge passes it to Core as the resume target, and Core loads the
    /// conversation from its own session store and mints the incarnation.
    /// The caller names no runtime id and hands over no session payload. The
    /// outcome reports the binding Core committed, and the bridge's later
    /// calls for the member take its `agent_runtime_id`.
    ///
    /// `resume_from_role` names the identity's previous role when it differs
    /// from the role `spec` declares; Core verifies it against its durable
    /// record and refuses typed on a mismatch. `None` carries no migration
    /// authority of the caller's own.
    ///
    /// When Core holds the session, this performs a true resume
    /// (conversation history intact). A session Core holds no durable copy
    /// of is refused typed, `BridgeError::ResumeRejected` with
    /// `ResumeRejectionKind::Unavailable { reason: Absent }` naming the
    /// session: the bridge never spawns fresh in its place, the caller
    /// decides.
    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError>;

    /// The role this activation's host declared `identity` migrates from
    /// (`InitParams.role_migrations`), if any. `None` carries no migration
    /// authority. A caller resuming across a declared profile change passes it
    /// to [`Self::resume_session`] as `resume_from_role`; the bridge applies
    /// no declaration by itself.
    fn declared_role_migration(
        &self,
        _identity: &AgentIdentity,
    ) -> Option<meerkat_mob::ProfileName> {
        None
    }

    /// Core's member for `identity`: the incarnation Core committed and the
    /// session Core projects for it, read as one consistent pair. A pure
    /// read: no revive and no bookkeeping. `Ok(None)` means Core holds no
    /// member for the identity.
    ///
    /// A `None` session means only that Core projects no session binding for
    /// the member: a peer-only member, or a session-backed one whose binding
    /// is not projected at this moment. The caller decides peer-only from its
    /// own spec, never from this answer, and refuses typed when a
    /// session-backed spec reads no session. A respawn that lands between the
    /// incarnation read and the session read twice in a row refuses with
    /// [`BridgeError::LiveMemberMoved`].
    async fn live_member(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "live_member",
        })
    }

    /// Core's delivery scope for `identity`'s member: incarnation, fence and
    /// session from one machine-state read. `Ok(None)` only when Core holds no
    /// member. A member bound to no session (peer-only) has no scope and is an
    /// error here; [`Self::live_member`] reads it.
    async fn live_member_scope(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "live_member_scope",
        })
    }

    /// Make sure Core's member for `identity` is live and record it for this
    /// bridge's runtime-id-keyed calls. Core revives a session-backed member
    /// whose live materialization it retired; a peer-only member has nothing
    /// to revive. Runs no turn. `Ok(None)` means Core holds no member. The
    /// binding is [`Self::live_member`]'s read, with the same meaning of a
    /// `None` session: Core answers the revive of a member with no projected
    /// session binding as not found, so the caller decides peer-only from its
    /// own spec and refuses typed when a session-backed spec reads no session.
    /// Refuses with [`BridgeError::HostAdopted`], before any other Core call,
    /// when Core holds an identity intent row for `identity`: Meta writes
    /// none, so the row is the host's.
    async fn ensure_member_live(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "ensure_member_live",
        })
    }

    /// Core's output-only convergence status for `identity`, unclassified.
    async fn identity_convergence_status(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<
        meerkat_mob::identity::IdentityStoredObservation<
            meerkat_mob::identity::IdentityConvergenceStatus,
        >,
        BridgeError,
    > {
        Err(BridgeError::Unsupported {
            operation: "identity_convergence_status",
        })
    }

    /// Whether Core's identity intent row for `identity` declares a live
    /// member: `Some(true)` for `Present`, `Some(false)` for `Absent`, and
    /// `None` while Core holds no readable row.
    async fn identity_intent_present(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<Option<bool>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "identity_intent_present",
        })
    }

    /// Core's member history: one fold of the mob's journal with its
    /// identity intent rows, read in one store transaction, so every answer
    /// taken from one history agrees with the others.
    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "member_binding_history",
        })
    }

    /// `identity`'s latest binding since the latest mob reset, live or
    /// retired. `Ok(None)` means Core never spawned it since then.
    async fn last_member_binding(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "last_member_binding",
        })
    }

    /// Whether Core holds an identity intent row for `identity` (a host
    /// adopted it), and whether that row reads.
    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "identity_management",
        })
    }

    /// Core's live members spawned with a durable identity intent.
    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "durable_identity_members",
        })
    }

    /// Whether Core keeps `session_id` beyond its member, so a member
    /// retired from it can resume it. `Ok(false)` on a runtime whose
    /// sessions live in memory only (Core's session service persists none):
    /// retiring the member would end the session's only copy.
    async fn session_outlives_member(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "session_outlives_member",
        })
    }

    /// Core's members that carry the identity-first runtime's
    /// `agent_identity` label naming the identity their member id encodes:
    /// the members Meta spawned, Meta 0.9.3's unmarked ones included. Read
    /// only to retire one the roster no longer lists; the label never
    /// grants an identity.
    async fn identity_labelled_members(
        &self,
    ) -> Result<Vec<meerkat_mob::AgentIdentity>, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "identity_labelled_members",
        })
    }

    /// Core's members it holds Broken (it could not restore them, for
    /// example because a restored member's comms identity is missing on this
    /// host), each with Core's reason, read from Core's member list without
    /// the mob actor. A bridge without the read reports none.
    async fn broken_members(
        &self,
    ) -> Result<BTreeMap<meerkat_mob::AgentIdentity, String>, BridgeError> {
        Ok(BTreeMap::new())
    }

    /// Whether Core holds a durable copy of `session_id`: Core's latest
    /// committed metadata for that exact session, archived sessions
    /// included, read without the session's actor. `Ok(false)` only when
    /// Core's session authority answers that it holds none.
    async fn durable_session_held(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::Unsupported {
            operation: "durable_session_held",
        })
    }

    /// Deliver ONE fully-admitted request to an active mob member. The
    /// SINGLE authority-bearing delivery method: every convenience form
    /// below is a provided forwarder that BUILDS a [`BridgeDelivery`], so no
    /// implementation can receive delivery authority it did not explicitly
    /// accept. REQUIRED on purpose (the optional-default authority-drop bug
    /// class): an implementation that cannot honor
    /// `delivery.delivery_identity` must FAIL CLOSED with a typed error -
    /// a caller holding an identity gets dedup or a refusal, never a silent
    /// at-least-once downgrade.
    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError>;

    /// Explicit local human admission. Custom/remote bridges must opt in;
    /// defaulting to generic work would silently erase conversational intent.
    async fn deliver_host_human_input(
        &self,
        _runtime_id: &AgentRuntimeId,
        _expected_session: &meerkat_core::SessionId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::SessionId, BridgeError> {
        Err(BridgeError::HostHumanInput(
            HostHumanInputError::Unsupported(
                "this session bridge has no local host-human input seam".to_string(),
            ),
        ))
    }

    /// Capture the native delivery scope of the member bound to
    /// `runtime_id`. Strictly observational. Custom and remote bridges have
    /// no scoped seam and must not fabricate one.
    async fn capture_member_delivery_scope(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<meerkat_mob::MemberDeliveryScope, ScopedDeliveryError> {
        Err(ScopedDeliveryError::Unsupported {
            detail: SCOPED_DELIVERY_UNSUPPORTED.to_string(),
        })
    }

    /// Submit exactly one delivery to the member session `scope` pins,
    /// returning meerkat's receipt. Never materializes, repairs, retargets or
    /// prepares the delivery: a moved scope is refused typed.
    async fn submit_at_scope(
        &self,
        _runtime_id: &AgentRuntimeId,
        _scope: &meerkat_mob::MemberDeliveryScope,
        _content: &meerkat_core::ContentInput,
        _handling_mode: HandlingMode,
        _delivery_identity: &meerkat_mob::MobDeliveryIdentity,
    ) -> Result<meerkat_mob::WorkDeliveryReceipt, ScopedDeliveryError> {
        Err(ScopedDeliveryError::Unsupported {
            detail: SCOPED_DELIVERY_UNSUPPORTED.to_string(),
        })
    }

    /// Read one delivery's state from the ORIGINAL session `scope` pins,
    /// never from the member's current session.
    async fn recover_at_scope(
        &self,
        _scope: &meerkat_mob::MemberDeliveryScope,
        _delivery_identity: &meerkat_mob::MobDeliveryIdentity,
        _deadline: std::time::Instant,
    ) -> Result<meerkat_mob::ScopedWorkRecovery, ScopedDeliveryError> {
        Err(ScopedDeliveryError::Unsupported {
            detail: SCOPED_DELIVERY_UNSUPPORTED.to_string(),
        })
    }

    /// Retire exactly the member incarnation `scope` pins, through
    /// meerkat's scoped retire: the member's `spawned_by` descendants, the
    /// member, then the descendants again, each step checked against the
    /// scope in the actor command that retires it. `runtime_id` is the
    /// incarnation's public alias, whose bookkeeping the bridge drops once it
    /// is retired. Custom and remote bridges have no scoped seam and must not
    /// fall back to an unscoped retire.
    async fn retire_at_scope(
        &self,
        _runtime_id: &AgentRuntimeId,
        _scope: &meerkat_mob::MemberDeliveryScope,
    ) -> Result<(), ScopedRetireError> {
        Err(ScopedRetireError::Unsupported {
            detail: SCOPED_DELIVERY_UNSUPPORTED.to_string(),
        })
    }

    /// Deliver content to an active mob member.
    async fn deliver(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.deliver_admitted(
            runtime_id,
            BridgeDelivery::new(content.clone(), HandlingMode::Queue),
        )
        .await
    }

    /// Deliver content to an active mob member using a caller-selected turn
    /// handling mode.
    async fn deliver_with_mode(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.deliver_admitted(
            runtime_id,
            BridgeDelivery::new(content.clone(), handling_mode),
        )
        .await
    }

    /// Deliver content plus a separate `injected_context` body (meerkat
    /// 0.7.12 ask 1: typed ambient injection alongside — not fused into —
    /// the user's message) and an optional host-minted interaction id
    /// (meerkat 0.7.25 ask 15 addendum: threaded into runtime admission so
    /// transcript messages join the caller's live interaction frames).
    async fn deliver_with_mode_and_context(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        let mut delivery = BridgeDelivery::new(content.clone(), handling_mode);
        delivery.injected_context = injected_context.to_vec();
        delivery.interaction_id = interaction_id.map(ToString::to_string);
        self.deliver_admitted(runtime_id, delivery).await
    }

    /// [`Self::deliver_with_mode_context_and_system_prompt`], but returning
    /// only after the runtime has COMMITTED this turn's terminal boundary.
    ///
    /// The ingress-only `deliver_*` methods above are unchanged and remain the
    /// normal path: they return as soon as the work is admitted, which is what
    /// production callers want. This sibling exists for callers that need proof
    /// the turn actually finished - principally tests, which otherwise infer
    /// completion from a timer or from a session-wide event stream. Both
    /// inferences are unsound: a timer elapses whether or not the turn ran, and
    /// a session-wide stream cannot attribute a terminal to one specific
    /// delivery.
    ///
    /// Defaults to [`BridgeError::CompletionUnsupported`] so that every
    /// existing implementor compiles unchanged. The concrete mob bridge
    /// overrides it using meerkat's `MobHandle::start_work_with_mode*` and
    /// `WorkTurnHandle::wait`, which carry an exact per-work-item completion
    /// signal.
    async fn deliver_awaiting_commit_with_mode_context_and_system_prompt(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        let receipt = self
            .begin_awaiting_commit(
                runtime_id,
                content,
                system_prompt,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
            .map_err(BridgeError::from)?;
        receipt.wait().await.map_err(BridgeError::from)
    }

    /// Admit a turn and hand back a [`BridgeTurnReceipt`] the caller can await
    /// LATER - crucially, after releasing whatever lock it holds.
    ///
    /// This is the primitive;
    /// [`Self::deliver_awaiting_commit_with_mode_context_and_system_prompt`] is
    /// `begin` + `wait` composed, and exists only for callers with no lock to
    /// release.
    ///
    /// Returns only after BOUNDED admission plus BOUNDED session resolution -
    /// both inside the admission deadline, neither of them the LLM turn. The
    /// resolution outcome rides in `session_result` rather than being
    /// `?`-propagated, because an early return after admission drops the handle
    /// of a turn that is already running.
    ///
    /// Defaults to [`BridgeAdmissionError::CompletionUnsupported`] BEFORE any
    /// submit, so a caller that sees it knows nothing was delivered.
    async fn begin_awaiting_commit(
        &self,
        _runtime_id: &AgentRuntimeId,
        _content: &meerkat_core::ContentInput,
        _system_prompt: Option<&str>,
        _injected_context: &[meerkat_core::ContentInput],
        _handling_mode: HandlingMode,
        _interaction_id: Option<&str>,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        Err(BridgeAdmissionError::CompletionUnsupported(
            "this bridge implements ingress-only delivery".to_string(),
        ))
    }

    /// Whether [`Self::begin_delivery_with_output`] is implemented: turns this
    /// bridge admits can report their own output, so a delivery can be
    /// tracked by ticket. The identity runtime decides the lane from this
    /// before delivering, so an untracked bridge is never attempted and then
    /// retried.
    fn tracks_turn_output(&self) -> bool {
        false
    }

    /// Admit `delivery` and hand back a receipt whose completion carries the
    /// admitted turn's own output ([`BridgeTurnReceipt::wait_turn_output`]).
    ///
    /// The receipt holds the admitted turn's own completion, so the output
    /// comes from that turn's committed run result, never from the session's
    /// latest text, and never needs the delivery's `interaction_id` to name
    /// the turn (the caller's interaction id rides unchanged). A successful
    /// terminal without a run result of its own is
    /// [`TurnOutput::NoOwnResult`](super::types::TurnOutput::NoOwnResult),
    /// never an empty output. Same admission contract as
    /// [`Self::begin_awaiting_commit`]; the receipt is awaited later, outside
    /// any lock.
    ///
    /// Defaults to [`BridgeAdmissionError::CompletionUnsupported`] BEFORE any
    /// delivery, matching [`Self::tracks_turn_output`]'s `false`.
    async fn begin_delivery_with_output(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        Err(BridgeAdmissionError::CompletionUnsupported(
            "this bridge cannot report per-turn output".to_string(),
        ))
    }

    /// Admit one already-bound governed input to the member bound to
    /// `runtime_id`, whose current session must be `session_id`, through its
    /// native admission, returning a receipt that reports the turn's own
    /// output. `custody` is handed to the member session's native admission,
    /// which settles it exactly once; a bridge that answers before submitting,
    /// or a delivery future dropped before it is first polled, settles it
    /// `NotAdmitted` through its guard. The identity runtime decides the host
    /// custody from that settlement, never from the error. Defaults to
    /// unsupported BEFORE any delivery.
    async fn begin_governed_delivery(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
        _input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        drop(custody);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported(
                "this bridge does not admit governed input".to_string(),
            ),
        ))
    }

    /// [`Self::begin_governed_delivery`] as internal work: the trusted host's
    /// own retained, already-bound input to a member it composed, which need
    /// not admit external work. The same exact-session check, native
    /// admission, custody settlement and definitive admission wait apply.
    /// Only the identity runtime's trusted-host dispatch door calls it; no
    /// external or wire caller selects it. Defaults to unsupported BEFORE
    /// any delivery.
    async fn begin_internal_governed_delivery(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
        _input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        drop(custody);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported(
                "this bridge does not admit internal governed input".to_string(),
            ),
        ))
    }

    /// The actual controller client of the member's live session actor, for
    /// binding a governed input. Defaults to unsupported.
    async fn governed_controller(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported("this bridge has no controller pin source".to_string()),
        ))
    }

    /// Attach one reviewed MCP server live to the member session's own
    /// actor; a held turn boundary answers `Busy` instead of being waited
    /// for (see [`ControllerPinSource::try_attach_live_mcp_server`]).
    /// Defaults to unsupported.
    async fn attach_live_mcp_server(
        &self,
        session_id: &meerkat_core::types::SessionId,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        let _ = (session_id, attachment);
        Err(meerkat_core::mcp_attachment::LiveMcpAttachError::Unsupported)
    }

    /// Deliver content plus one ordinary System message authored for this
    /// exact turn (meerkat 0.8.11: `WorkSpec::system_prompt`, appended at the
    /// turn's admitted transcript boundary — per-turn content, never
    /// member/session configuration), alongside the optional injected
    /// context and interaction identity of
    /// [`Self::deliver_with_mode_and_context`].
    async fn deliver_with_mode_context_and_system_prompt(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        let mut delivery = BridgeDelivery::new(content.clone(), handling_mode);
        delivery.system_prompt = system_prompt.map(ToString::to_string);
        delivery.injected_context = injected_context.to_vec();
        delivery.interaction_id = interaction_id.map(ToString::to_string);
        self.deliver_admitted(runtime_id, delivery).await
    }

    /// Checkpoint the current session state for a mob member.
    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError>;

    /// Retire a mob member.
    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError>;

    /// Retire a member whose session was superseded by a committed destructive
    /// reset, then remove the old session's bridge persistence authority.
    ///
    /// The default keeps compatibility for custom bridges that do not expose
    /// Meerkat's retained archive-cleanup anchor. The concrete Mob bridge
    /// overrides this with the quiesce / CAS-abandon / exact-retry protocol.
    async fn retire_reset_superseded_member(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.retire_member(runtime_id).await?;
        self.unregister_session_runtime_state(session_id).await
    }

    /// Wire two active same-mob members by their concrete runtime IDs.
    async fn wire_peer(&self, _a: &AgentRuntimeId, _b: &AgentRuntimeId) -> Result<(), BridgeError> {
        Err(BridgeError::Mob("peer wiring not supported".to_string()))
    }

    /// Wire many active same-mob member pairs by their concrete runtime IDs.
    async fn wire_peers_batch(
        &self,
        edges: &[(AgentRuntimeId, AgentRuntimeId)],
    ) -> Result<(), BridgeError> {
        for (a, b) in edges {
            self.wire_peer(a, b).await?;
        }
        Ok(())
    }

    /// Return currently materialized same-mob member wires by concrete runtime IDs.
    async fn current_member_wires(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        Ok(Vec::new())
    }

    /// Return edges with either directed half present. The healthy
    /// `current_member_wires` projection intentionally reports reciprocal
    /// edges only; mutation/recovery needs this stronger diagnostic surface
    /// to remove orphan halves.
    async fn current_member_wires_any_half(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.current_member_wires().await
    }

    /// Unwire two active same-mob members by their concrete runtime IDs.
    async fn unwire_peer(
        &self,
        _a: &AgentRuntimeId,
        _b: &AgentRuntimeId,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::Mob("peer unwiring not supported".to_string()))
    }

    /// Inspect the current execution state of a mob member.
    async fn inspect_member(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        Err(BridgeError::Mob("inspect not supported".to_string()))
    }

    /// Register identity ownership for a concrete bridge session.
    ///
    /// Bridges that install a continuity-backed session store use this to
    /// ensure subsequent Meerkat session saves checkpoint under the durable
    /// identity/generation/fencing tuple.
    async fn register_session_runtime_state(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _identity: &AgentIdentity,
        _generation: ContinuityGeneration,
        checkpoint_version: CheckpointVersion,
        _fencing_token: FencingToken,
    ) -> Result<CheckpointVersion, BridgeError> {
        Ok(checkpoint_version)
    }

    /// Temporarily quiesce persistence for a concrete bridge session while
    /// external lease authority is rotated. Implementations must drain writes
    /// admitted before this call and reject later mutations until a successful
    /// [`Self::register_session_runtime_state`] publishes the replacement
    /// fencing token. The compatibility default uses permanent unregister;
    /// bridges with an in-process session-store adapter should preserve state.
    async fn suspend_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.unregister_session_runtime_state(session_id).await
    }

    /// Remove identity ownership metadata for a concrete bridge session.
    ///
    /// This is used when an identity lifecycle operation aborts after session
    /// runtime state was registered but before continuity commits.
    async fn unregister_session_runtime_state(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Attempt to make the durable session head strict-resume-acceptable
    /// BEFORE the continuity repair supervisor re-registers its identity as
    /// healable (2026-07-29 heal/re-Break incident).
    ///
    /// Meta runs no continuity repair supervisor today: a caller owns any
    /// retry and its backoff, including for the typed hosting holds
    /// ([`BridgeError::ServedElsewhere`], [`BridgeError::HostingUnavailable`];
    /// see [`CommittedBoundaryRecoverer::recover_committed_boundary`]).
    ///
    /// The compatibility default declares no heal seam: custom bridges keep
    /// today's behavior, where reconcile retries the resume directly.
    async fn recover_committed_boundary(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        Ok(CommittedBoundaryRepair::Unsupported)
    }
}

/// Lightweight inspection of a mob member's current execution state.
#[derive(Debug, Clone)]
pub struct MemberInspection {
    pub output_preview: Option<String>,
    pub is_final: bool,
    pub peer_reachable_count: usize,
    /// Why `output_preview` carries no observation of the member's session
    /// view (meerkat 0.8.45's typed marker: the bounded read hit its
    /// deadline, failed, found no session, or the member is retiring). When
    /// set, a missing `output_preview` is not an empty member.
    pub preview_unavailable: Option<meerkat_mob::MemberPreviewUnavailable>,
}

/// Which restored LLM-identity fields diverge from the profile declaration
/// with no resume-override mask covering them. Pure comparison seam behind
/// [`MobSessionBridge::log_unmasked_resume_divergence`]: a field counts only
/// when it is unmasked (durable metadata will win), declared (for provider —
/// an undeclared provider states no intent), and different.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnmaskedResumeDivergence {
    model: bool,
    provider: bool,
}

fn unmasked_resume_divergence(
    mask: &meerkat_core::service::ResumeOverrideMask,
    declared_model: &str,
    declared_provider: Option<meerkat_core::Provider>,
    restored_model: &str,
    restored_provider: meerkat_core::Provider,
) -> UnmaskedResumeDivergence {
    UnmaskedResumeDivergence {
        model: !mask.model && restored_model != declared_model,
        provider: !mask.provider
            && declared_provider.is_some_and(|declared| restored_provider != declared),
    }
}

// ---------------------------------------------------------------------------
// MobSessionBridge — real implementation backed by MobHandle
// ---------------------------------------------------------------------------

/// Concrete `SessionBridge` backed by a `MobHandle`.
///
/// The mob-layer `MobAgentIdentity` is the DURABLE identity's comms-safe
/// encoding, for every binding. `AgentRuntimeId` is incarnation detail and is
/// not a roster spelling: Meerkat defines it as a stable identity plus a
/// machine-owned generation, and `SpawnMemberSpec.identity` is the roster
/// identity whose generation Meerkat mints itself. This used to be the other way
/// round, with the runtime id lowered into the roster slot and the durable
/// identity used only for external bindings, which made the roster id
/// per-incarnation and durable state keyed on it die with the binding.
/// Per-identity temporary auto-compaction floors for the `compact_member`
/// operator verb.
///
/// A floor recorded here is applied by [`MobSessionBridge`] to the NEXT spawn
/// spec it builds for that identity: the resolved definition profile is
/// snapshotted into `SpawnMemberSpec::override_profile` with
/// `auto_compact_threshold` replaced by the floor, which meerkat-mob lowers
/// into `SessionBuildOptions::auto_compact_threshold_override` (the strongest
/// compaction-threshold authority). The registry is in-memory only and the
/// verb clears the entry after the forced compaction fires; a floor is never
/// persisted, so a crash mid-verb degrades to the original profile value on
/// the next boot rather than freezing a compact-every-turn build.
#[derive(Debug, Default)]
pub struct CompactionFloorRegistry {
    floors: std::sync::Mutex<std::collections::BTreeMap<String, CompactionFloorEntry>>,
    operations: std::sync::Mutex<
        std::collections::BTreeMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>,
    >,
}

#[derive(Debug)]
struct CompactionFloorEntry {
    floor: std::num::NonZeroU64,
    operation_id: Option<String>,
    apply_to_build: bool,
}

pub(crate) struct CompactionFloorChange<'a> {
    pub(crate) registry: &'a CompactionFloorRegistry,
    pub(crate) operation_id: &'a str,
    pub(crate) floor: Option<std::num::NonZeroU64>,
}

impl CompactionFloorRegistry {
    pub(crate) fn set_owned(
        &self,
        identity: &AgentIdentity,
        operation_id: &str,
        floor: std::num::NonZeroU64,
    ) {
        self.floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                identity.to_string(),
                CompactionFloorEntry {
                    floor,
                    operation_id: Some(operation_id.to_string()),
                    apply_to_build: true,
                },
            );
    }

    pub(crate) fn finish_owned_build(&self, identity: &AgentIdentity, operation_id: &str) {
        let mut floors = self
            .floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = floors.get_mut(identity.as_str())
            && entry.operation_id.as_deref() == Some(operation_id)
        {
            entry.apply_to_build = false;
        }
    }

    pub(crate) fn clear_owned(&self, identity: &AgentIdentity, operation_id: &str) {
        let mut floors = self
            .floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if floors
            .get(identity.as_str())
            .is_some_and(|entry| entry.operation_id.as_deref() == Some(operation_id))
        {
            floors.remove(identity.as_str());
        }
    }

    fn for_build(&self, identity: &AgentIdentity) -> Option<std::num::NonZeroU64> {
        self.floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity.as_str())
            .filter(|entry| entry.apply_to_build)
            .map(|entry| entry.floor)
    }

    pub(crate) fn operation_lock(&self, identity: &AgentIdentity) -> Arc<tokio::sync::Mutex<()>> {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(lock) = operations
            .get(identity.as_str())
            .and_then(std::sync::Weak::upgrade)
        {
            return lock;
        }
        operations.retain(|_, lock| lock.strong_count() > 0);
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        operations.insert(identity.to_string(), Arc::downgrade(&lock));
        lock
    }

    /// Record a temporary floor for `identity`, returning any previous one.
    pub fn set(
        &self,
        identity: &AgentIdentity,
        floor: std::num::NonZeroU64,
    ) -> Option<std::num::NonZeroU64> {
        self.floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                identity.as_str().to_string(),
                CompactionFloorEntry {
                    floor,
                    operation_id: None,
                    apply_to_build: true,
                },
            )
            .map(|entry| entry.floor)
    }

    /// Clear the floor for `identity`, returning it if one was recorded.
    pub fn clear(&self, identity: &AgentIdentity) -> Option<std::num::NonZeroU64> {
        self.floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(identity.as_str())
            .map(|entry| entry.floor)
    }

    /// The floor currently recorded for `identity`, if any.
    pub fn get(&self, identity: &AgentIdentity) -> Option<std::num::NonZeroU64> {
        self.floors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity.as_str())
            .map(|entry| entry.floor)
    }
}

pub struct MobSessionBridge {
    handle: MobHandle,
    /// Session store used for checkpoint (loading session data to serialize).
    session_store: Option<Arc<dyn meerkat::SessionStore>>,
    /// Session service used to project the live effective model for capability checks.
    session_service: Option<Arc<dyn MobSessionService>>,
    /// The host config the mob's members are built with, so the image check
    /// judges a self-hosted or config-declared model by its `vision` flag as
    /// Core does. `None` judges with the static model catalog alone.
    member_model_config: Option<Arc<meerkat::Config>>,
    runtime_members: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
    runtime_sessions: Arc<tokio::sync::RwLock<HashMap<String, meerkat_core::types::SessionId>>>,
    /// Lazily-minted ops-owner bridge session for external (peer-only)
    /// members, used when the mob was created without machine-bound owner
    /// bridge-session authority. Stable for the bridge lifetime so every
    /// external member shares one generated operation owner.
    generated_external_owner_session: std::sync::OnceLock<meerkat_core::types::SessionId>,
    /// Heal authority for the continuity repair supervisor. `None` means no
    /// heal seam ([`CommittedBoundaryRepair::Unsupported`]); composition
    /// injects the concrete meerkat-backed recoverer where available.
    committed_boundary_recoverer: Option<Arc<dyn CommittedBoundaryRecoverer>>,
    /// The session owner's controller pin operation for governed binding.
    controller_pin_source: Option<Arc<dyn ControllerPinSource>>,
    /// The pinned controller of each session's actor incarnation.
    controller_pins: std::sync::Mutex<
        std::collections::HashMap<
            meerkat_core::types::SessionId,
            (
                meerkat_session::LiveSessionActorWitness,
                meerkat_core::ControllerModelClient,
            ),
        >,
    >,
    /// Identities whose unmasked resume divergence was already logged this
    /// boot (the bridge lives for one boot). See
    /// [`Self::log_unmasked_resume_divergence`].
    resume_divergence_logged: std::sync::Mutex<std::collections::HashSet<String>>,
    /// This activation's host-declared member role migrations, keyed by exact
    /// identity. Empty means no identity may migrate its durable role on
    /// resume.
    ///
    /// Lives for one boot alongside the bridge and is never persisted: Meta
    /// has no store that can reconstruct it, so a declaration dropped from the
    /// next boot payload is simply gone. See [`RoleMigrationDeclaration`].
    role_migration_declarations: HashMap<AgentIdentity, meerkat_mob::ProfileName>,
    /// Budget one delivery attempt may spend waiting on the mob actor.
    /// Resolved once at construction so the success path pays nothing per
    /// call. See [`BRIDGE_ACTOR_ADMISSION_BUDGET`].
    actor_admission_budget: Duration,
    /// Late-bound actor-loop probe verdict (installed by the unified runtime
    /// through [`SessionBridge::observe_actor_loop_health`]). While a stall is
    /// open, admission round trips fail fast naming the `stall_id`.
    actor_loop_health: std::sync::RwLock<Option<Arc<ActorLoopHealth>>>,
    /// Machine-level ingress authority for the repair disposal paths (production
    /// run 33758a41: repair destroyed 15 queued inputs with the member).
    /// Lets repair capture a member's queued/steered inputs BEFORE the
    /// destructive retire and re-admit them into the healed successor
    /// session. `None` (validation-only compositions) keeps the legacy
    /// destroy-on-dispose behavior, minus the carry observability.
    runtime_ingress_authority: Option<Arc<dyn meerkat_runtime::SessionServiceRuntimeExt>>,
    /// Temporary per-identity compaction floors for the `compact_member`
    /// operator verb, consulted at every spawn-spec build. Always present;
    /// composition shares it out via [`Self::compaction_floors`].
    compaction_floors: Arc<CompactionFloorRegistry>,
}

/// One queued/steered member input captured before a repair disposal, for
/// re-admission into the healed successor session (production run 33758a41: the
/// disposal destroyed 15 queued review inputs with the member).
struct CarriedMemberInput {
    original_input_id: meerkat_core::lifecycle::InputId,
    admission_sequence: Option<u64>,
    input: meerkat_runtime::Input,
}

/// Pre-disposal capture of one member session's pending machine ingress.
/// What repair may do with ONE pending input, decided purely by its class.
///
/// Extracted from `capture_pending_member_ingress` so the decision can be
/// tested without a live runtime authority. The inline version could only be
/// checked through a full repair, where the only observable was prompt text, and
/// that cannot distinguish an input that was RE-ADMITTED from one that merely
/// sits in the successor's transcript history.
enum PendingInputCarry {
    /// Boxed: `Input` is ~950 bytes and would otherwise make every
    /// `Uncarryable` classification pay for the carried payload it does not
    /// have.
    Carry(Box<meerkat_runtime::Input>),
    Uncarryable {
        class: &'static str,
        reason: String,
    },
}

/// The carry lane for a pending input, or the named reason there is none.
///
/// This is the single owner of "what may cross a repair". A class with no carry
/// lane is NAMED rather than silently dropped, because the disposal that follows
/// destroys it and an unnamed loss is unreviewable.
fn classify_pending_input_for_carry(
    persisted_input: Option<meerkat_runtime::Input>,
) -> PendingInputCarry {
    match persisted_input {
        // User-originated work: the whole point of the carry.
        Some(
            input @ (meerkat_runtime::Input::Prompt(_)
            | meerkat_runtime::Input::Peer(_)
            | meerkat_runtime::Input::ExternalEvent(_)),
        ) => PendingInputCarry::Carry(Box::new(input)),
        Some(meerkat_runtime::Input::FlowStep(_)) => PendingInputCarry::Uncarryable {
            class: "flow-step",
            reason: "flow-step correlation is owned by the flow engine and cannot be \
                     re-admitted raw; the loss is bounded to the interrupted flow, \
                     which observes its step's terminal and owns the retry"
                .to_string(),
        },
        Some(meerkat_runtime::Input::Continuation(_) | meerkat_runtime::Input::Operation(_)) => {
            PendingInputCarry::Uncarryable {
                class: "runtime-internal",
                reason: "continuation/operation inputs are machine-internal and cannot be \
                         re-admitted raw; the successor runtime re-derives its own; the \
                         loss is bounded to the disposed runtime's in-flight bookkeeping"
                    .to_string(),
            }
        }
        Some(_) => PendingInputCarry::Uncarryable {
            class: "unrecognized-class",
            reason: "the pending input's class is unknown to this build; no carry \
                     lane exists for it"
                .to_string(),
        },
        None => PendingInputCarry::Uncarryable {
            class: "payload-unavailable",
            reason: "the runtime retained no payload for this pending input".to_string(),
        },
    }
}

struct PendingIngressCapture {
    /// Inputs whose payload survives re-admission (Prompt / Peer /
    /// ExternalEvent classes), in admission order.
    carryable: Vec<CarriedMemberInput>,
    /// Pending inputs repair cannot carry: `(input id, class, reason)`.
    /// Logged loudly per item before disposal proceeds.
    uncarryable: Vec<(meerkat_core::lifecycle::InputId, &'static str, String)>,
}

impl PendingIngressCapture {
    fn empty() -> Self {
        Self {
            carryable: Vec::new(),
            uncarryable: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.carryable.is_empty() && self.uncarryable.is_empty()
    }
}

/// The inputs other senders queued on a member that were still waiting to
/// run, captured by [`SessionBridge::capture_queued_member_inputs`] before a
/// fresh life replaces the member, for
/// [`SessionBridge::readmit_queued_member_inputs`] to admit to the fresh
/// member.
#[derive(Default)]
pub struct QueuedMemberInputs {
    capture: Option<PendingIngressCapture>,
}

impl QueuedMemberInputs {
    /// How many captured inputs the fresh member can be given.
    pub fn carryable(&self) -> usize {
        self.capture
            .as_ref()
            .map_or(0, |capture| capture.carryable.len())
    }
}

/// Re-mint the runtime identity of a carried input for successor admission.
///
/// The disposal durably terminalized the original admission (abandonment),
/// so the successor must admit a NEW input: reusing the original `InputId`
/// collides with the terminal ledger row, and reusing the idempotency key
/// would dedup the carry into that terminal record and silently drop it.
/// Everything else — content, handling mode, visibility, correlation — rides
/// unchanged.
fn remint_carried_input_identity(
    mut input: meerkat_runtime::Input,
) -> Option<meerkat_runtime::Input> {
    let header = match &mut input {
        meerkat_runtime::Input::Prompt(i) => &mut i.header,
        meerkat_runtime::Input::Peer(i) => &mut i.header,
        meerkat_runtime::Input::FlowStep(i) => &mut i.header,
        meerkat_runtime::Input::ExternalEvent(i) => &mut i.header,
        meerkat_runtime::Input::Continuation(i) => &mut i.header,
        meerkat_runtime::Input::Operation(i) => &mut i.header,
        // `Input` is #[non_exhaustive]: a variant this build does not know
        // has no reachable header, so it cannot be re-identified for
        // successor admission.
        _ => return None,
    };
    header.id = meerkat_core::lifecycle::InputId::new();
    if let Some(key) = header.idempotency_key.take() {
        tracing::debug!(
            idempotency_key = %key,
            readmitted_input_id = %header.id,
            "carried input drops its idempotency key: the original admission was \
             durably terminalized by the repair disposal"
        );
    }
    Some(input)
}

impl MobSessionBridge {
    /// Create a new bridge wrapping the given mob handle.
    pub fn new(handle: MobHandle) -> Self {
        Self {
            handle,
            session_store: None,
            session_service: None,
            member_model_config: None,
            runtime_members: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            generated_external_owner_session: std::sync::OnceLock::new(),
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            controller_pins: std::sync::Mutex::new(std::collections::HashMap::new()),
            resume_divergence_logged: std::sync::Mutex::new(std::collections::HashSet::new()),
            role_migration_declarations: HashMap::new(),
            actor_admission_budget: bridge_actor_admission_budget(),
            actor_loop_health: std::sync::RwLock::new(None),
            runtime_ingress_authority: None,
            compaction_floors: Arc::new(CompactionFloorRegistry::default()),
        }
    }

    /// Create a new bridge with session-service access for live model capability checks.
    pub fn with_session_service(
        handle: MobHandle,
        session_service: Arc<dyn MobSessionService>,
    ) -> Self {
        Self {
            handle,
            session_store: None,
            session_service: Some(session_service),
            member_model_config: None,
            runtime_members: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            generated_external_owner_session: std::sync::OnceLock::new(),
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            controller_pins: std::sync::Mutex::new(std::collections::HashMap::new()),
            resume_divergence_logged: std::sync::Mutex::new(std::collections::HashSet::new()),
            role_migration_declarations: HashMap::new(),
            actor_admission_budget: bridge_actor_admission_budget(),
            actor_loop_health: std::sync::RwLock::new(None),
            runtime_ingress_authority: None,
            compaction_floors: Arc::new(CompactionFloorRegistry::default()),
        }
    }

    /// Create a new bridge with an explicit session store for checkpoint support.
    pub fn with_session_store(
        handle: MobHandle,
        session_store: Arc<dyn meerkat::SessionStore>,
    ) -> Self {
        Self {
            handle,
            session_store: Some(session_store),
            session_service: None,
            member_model_config: None,
            runtime_members: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            generated_external_owner_session: std::sync::OnceLock::new(),
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            controller_pins: std::sync::Mutex::new(std::collections::HashMap::new()),
            resume_divergence_logged: std::sync::Mutex::new(std::collections::HashSet::new()),
            role_migration_declarations: HashMap::new(),
            actor_admission_budget: bridge_actor_admission_budget(),
            actor_loop_health: std::sync::RwLock::new(None),
            runtime_ingress_authority: None,
            compaction_floors: Arc::new(CompactionFloorRegistry::default()),
        }
    }

    /// Create a bridge with checkpoint and live capability support.
    pub fn with_session_store_and_service(
        handle: MobHandle,
        session_store: Arc<dyn meerkat::SessionStore>,
        session_service: Arc<dyn MobSessionService>,
    ) -> Self {
        Self {
            handle,
            session_store: Some(session_store),
            session_service: Some(session_service),
            member_model_config: None,
            runtime_members: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            generated_external_owner_session: std::sync::OnceLock::new(),
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            controller_pins: std::sync::Mutex::new(std::collections::HashMap::new()),
            resume_divergence_logged: std::sync::Mutex::new(std::collections::HashSet::new()),
            role_migration_declarations: HashMap::new(),
            actor_admission_budget: bridge_actor_admission_budget(),
            actor_loop_health: std::sync::RwLock::new(None),
            runtime_ingress_authority: None,
            compaction_floors: Arc::new(CompactionFloorRegistry::default()),
        }
    }

    /// The shared per-identity compaction-floor registry this bridge consults
    /// at every spawn-spec build. Composition hands this to the RPC layer so
    /// the `compact_member` operator verb can arm/disarm floors on the same
    /// registry the materialization path reads.
    pub fn compaction_floors(&self) -> Arc<CompactionFloorRegistry> {
        Arc::clone(&self.compaction_floors)
    }

    /// Apply a pending `compact_member` floor to a freshly built spawn spec.
    ///
    /// The floor must ride the typed profile carrier meerkat-mob reads
    /// (`override_profile.auto_compact_threshold` ->
    /// `SessionBuildOptions::auto_compact_threshold_override`), so it needs a
    /// profile snapshot: the one the spec build already installed (model pin /
    /// provider params), else the resolved inline definition profile. A
    /// realm-ref profile resolves to no inline snapshot; fail closed rather
    /// than silently materializing a member the verb believes is floored.
    fn apply_compaction_floor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        spawn_spec: &mut SpawnMemberSpec,
    ) -> Result<(), BridgeError> {
        let Some(floor) = self.compaction_floors.for_build(identity) else {
            return Ok(());
        };
        let Some(mut profile) = spawn_spec
            .override_profile
            .take()
            .or_else(|| self.base_profile_for_spec(spec))
        else {
            return Err(BridgeError::InvalidInput(format!(
                "compact_member floor is armed for identity {} but profile '{}' resolves to no \
                 inline definition profile to carry it; realm-ref profiles are not supported by \
                 this verb",
                identity.as_str(),
                spec.profile.as_str(),
            )));
        };
        profile.auto_compact_threshold = Some(floor);
        spawn_spec.override_profile = Some(profile);
        Ok(())
    }

    /// Override the per-delivery-attempt mob-actor admission budget.
    ///
    /// Hosts whose turns are known to be short can tighten the default; tests
    /// use this to exercise the bound without waiting out the real budget.
    #[must_use]
    pub fn with_actor_admission_budget(mut self, budget: Duration) -> Self {
        self.actor_admission_budget = budget;
        self
    }

    /// Inject the committed-boundary heal authority (2026-07-29 incident).
    ///
    /// Without it, [`SessionBridge::recover_committed_boundary`] reports
    /// [`CommittedBoundaryRepair::Unsupported`] and the continuity repair
    /// supervisor falls back to plain reconcile retries.
    #[must_use]
    pub fn with_committed_boundary_recoverer(
        mut self,
        recoverer: Arc<dyn CommittedBoundaryRecoverer>,
    ) -> Self {
        self.committed_boundary_recoverer = Some(recoverer);
        self
    }

    /// Install the session owner's controller pin operation, used to give a
    /// governed binder the member session actor's actual controller.
    pub fn with_controller_pin_source(mut self, source: Arc<dyn ControllerPinSource>) -> Self {
        self.controller_pin_source = Some(source);
        self
    }

    /// Install this activation's host-declared role migrations.
    ///
    /// The resulting map lives for the life of this bridge, which is exactly
    /// one boot, and is never persisted. Meta only ever reads it; it never
    /// adds to it, never infers an entry from durable state, and never retries
    /// a resume that Meerkat refused.
    #[must_use]
    pub fn with_role_migration_declarations(
        mut self,
        declarations: impl IntoIterator<Item = RoleMigrationDeclaration>,
    ) -> Self {
        self.role_migration_declarations = role_migration_declaration_map(declarations);
        self
    }

    /// Inject the machine-level ingress authority the repair disposal paths
    /// use to carry a member's queued work across a heal (production run 33758a41:
    /// disposal destroyed 15 queued inputs with the member). Compositions
    /// with a runtime machine pass it here; without it, repair proceeds but
    /// cannot observe or carry pending inputs.
    #[must_use]
    pub fn with_runtime_ingress_authority(
        mut self,
        authority: Arc<dyn meerkat_runtime::SessionServiceRuntimeExt>,
    ) -> Self {
        self.runtime_ingress_authority = Some(authority);
        self
    }

    /// Judge image deliveries with the host config the mob's members are built
    /// with ([`crate::mob_handle_runtime::MobRuntime::member_model_config`]),
    /// so a self-hosted or config-declared model's `vision` flag counts as it
    /// does in Core. Without it the static model catalog alone answers.
    #[must_use]
    pub fn with_member_model_config(mut self, config: Arc<meerkat::Config>) -> Self {
        self.member_model_config = Some(config);
        self
    }

    /// The effective per-delivery-attempt mob-actor admission budget.
    #[must_use]
    pub fn actor_admission_budget(&self) -> Duration {
        self.actor_admission_budget
    }

    async fn remember_runtime_member(
        &self,
        runtime_id: &AgentRuntimeId,
        member_id: &MobAgentIdentity,
    ) {
        self.runtime_members.write().await.insert(
            runtime_id.as_str().to_string(),
            member_id.as_str().to_string(),
        );
    }

    async fn remember_runtime_session(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) {
        self.runtime_sessions
            .write()
            .await
            .insert(runtime_id.as_str().to_string(), session_id.clone());
    }

    async fn forget_runtime_member(&self, runtime_id: &AgentRuntimeId) {
        self.runtime_members
            .write()
            .await
            .remove(runtime_id.as_str());
        self.runtime_sessions
            .write()
            .await
            .remove(runtime_id.as_str());
    }

    /// Forget every runtime binding this bridge recorded for `member_id`, for
    /// a member whose incarnation is being replaced: the caller does not hold
    /// the old incarnation's id, and the successor's comes back from Core.
    async fn forget_member_bindings(&self, member_id: &MobAgentIdentity) {
        let stale: Vec<String> = {
            let mut members = self.runtime_members.write().await;
            let stale: Vec<String> = members
                .iter()
                .filter(|(_, member)| member.as_str() == member_id.as_str())
                .map(|(runtime_id, _)| runtime_id.clone())
                .collect();
            for runtime_id in &stale {
                members.remove(runtime_id);
            }
            stale
        };
        let mut sessions = self.runtime_sessions.write().await;
        for runtime_id in &stale {
            sessions.remove(runtime_id);
        }
    }

    /// The incarnation Core committed for `member_id`, read back from the
    /// roster rather than predicted: MobMachine mints the generation, and a
    /// spawn after a retirement mints a new one.
    async fn committed_runtime_id(
        &self,
        identity: &AgentIdentity,
        member_id: &MobAgentIdentity,
    ) -> Result<AgentRuntimeId, BridgeError> {
        let entry = self
            .handle
            .get_member(member_id)
            .await
            .map_err(|error| {
                BridgeError::Mob(format!(
                    "reading the committed roster entry for {identity}: {error}"
                ))
            })?
            .ok_or_else(|| {
                BridgeError::Mob(format!(
                    "no committed roster entry for {identity} under {member_id}"
                ))
            })?;
        public_runtime_id(identity, &entry.agent_runtime_id)
    }

    /// Record the incarnation Core committed for `member_id` as bound to
    /// `session_id`, and report it.
    async fn attach_committed_member(
        &self,
        identity: &AgentIdentity,
        member_id: &MobAgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<AgentRuntimeId, BridgeError> {
        let agent_runtime_id = self.committed_runtime_id(identity, member_id).await?;
        self.remember_runtime_member(&agent_runtime_id, member_id)
            .await;
        self.remember_runtime_session(&agent_runtime_id, session_id)
            .await;
        Ok(agent_runtime_id)
    }

    /// The roster member id bound to a live runtime id.
    ///
    /// Read from bookkeeping, never derived. The old fallback re-derived a
    /// roster id from the runtime id's spelling, but `AgentRuntimeId` is an
    /// opaque newtype here - it validates only non-empty/no-space/no-slash, not
    /// `rt:{identity}:{generation}` - so a malformed or unexpected id would
    /// manufacture a roster identity that names nothing. The mapping is
    /// populated at every create/resume materialization; its absence is a
    /// bookkeeping gap and retryable, not something to paper over.
    async fn member_id_for_runtime_id(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<MobAgentIdentity, BridgeError> {
        let members = self.runtime_members.read().await;
        if let Some(member) = members.get(runtime_id.as_str()) {
            return Ok(MobAgentIdentity::from(member.as_str()));
        }
        // Bookkeeping first, then STRICT derivation. `durable_identity_from_runtime_alias`
        // requires the full `rt:{identity}:{generation}` production and a numeric
        // generation, so a malformed or unexpected id cannot manufacture a roster
        // identity the way `logical_memory_identity` could - that one falls back to
        // returning its input unchanged, which is how a bare string became a
        // "durable identity". Anything that is not a well-formed runtime alias is a
        // typed error rather than a guess.
        crate::member_comms_id::durable_identity_from_runtime_alias(runtime_id.as_str())
            .map(|identity| crate::member_comms_id::mob_member_id(&identity))
            .ok_or_else(|| {
                BridgeError::Mob(format!(
                    "no roster member recorded for runtime binding {runtime_id} and it is not a \
                     well-formed rt:{{identity}}:{{generation}} alias, so its durable identity \
                     cannot be derived (retryable: the mapping is populated at create/resume \
                     materialization)"
                ))
            })
    }

    /// Retire one session-owned member through Core and verify that the
    /// structural roster entry is actually gone: a retire that reports success
    /// while the roster still holds the member is an error, not a success.
    async fn retire_session_owned_member_to_absence(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<(), meerkat_mob::MobError> {
        match self.handle.retire(member_id.clone()).await {
            Ok(()) | Err(meerkat_mob::MobError::MemberNotFound(_)) => {}
            Err(error) => return Err(error),
        }

        if self
            .handle
            .list_all_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == *member_id)
        {
            return Err(meerkat_mob::MobError::Internal(format!(
                "session-owned retire reported success but retained roster anchor {member_id}"
            )));
        }
        Ok(())
    }

    /// Capture a member session's pending machine ingress BEFORE a repair
    /// disposal destroys it (production run 33758a41: queue_len=5 steer_queue_len=10
    /// destroyed with the member). Pending = admitted but not yet run
    /// (`Accepted`/`Queued`); mid-run and terminal inputs are not queue work.
    /// Best-effort observation: an unregistered runtime or a probe fault
    /// degrades to an empty capture (there is then nothing this bridge can
    /// see, let alone carry).
    /// Resolve the machine-level ingress authority for repair carry: an
    /// explicitly injected one wins; otherwise the session service's runtime
    /// adapter (the gateway's `MeerkatMachine`) serves.
    ///
    /// A refused acquisition is an error, not an absent authority: the owner
    /// exists but would not serve, so its queue may hold work.
    fn resolved_runtime_ingress_authority(
        &self,
    ) -> Result<
        Option<Arc<dyn meerkat_runtime::SessionServiceRuntimeExt>>,
        meerkat_runtime::RuntimeDriverError,
    > {
        if let Some(explicit) = self.runtime_ingress_authority.as_ref() {
            return Ok(Some(Arc::clone(explicit)));
        }
        let Some(service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        Ok(service
            .acquire_runtime_adapter(None)?
            .map(|machine| machine as Arc<dyn meerkat_runtime::SessionServiceRuntimeExt>))
    }

    /// `Err` when the runtime owner refused to serve: its queue may hold
    /// member inputs, so the caller must defer the disposal rather than
    /// destroy what it could not observe.
    async fn capture_pending_member_ingress(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<PendingIngressCapture, meerkat_mob::MobError> {
        let authority = match self.resolved_runtime_ingress_authority() {
            Ok(Some(authority)) => authority,
            Ok(None) => {
                tracing::debug!(
                    session_id = %session_id,
                    "no runtime ingress authority; repair cannot observe or carry queued \
                     member inputs"
                );
                return Ok(PendingIngressCapture::empty());
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    %error,
                    "runtime adapter acquisition refused; deferring repair disposal so \
                     queued member inputs are not lost"
                );
                return Err(meerkat_mob::MobError::Internal(format!(
                    "repair disposal deferred: the runtime owner refused to serve ({error}), \
                     so queued member inputs could not be captured"
                )));
            }
        };
        let active = match authority.list_active_inputs(session_id).await {
            Ok(ids) => ids,
            Err(error) => {
                // NotReady / NotFound = no registered runtime = no live queue
                // to destroy. Other faults mean the queue is unobservable;
                // disposal proceeds exactly as it did before this seam.
                tracing::debug!(
                    session_id = %session_id,
                    error = %error,
                    "pending-ingress probe unavailable before repair disposal"
                );
                return Ok(PendingIngressCapture::empty());
            }
        };
        let mut capture = PendingIngressCapture::empty();
        for input_id in active {
            let stored = match authority.input_state(session_id, &input_id).await {
                Ok(Some(stored)) => stored,
                Ok(None) => continue,
                Err(error) => {
                    capture
                        .uncarryable
                        .push((input_id, "state-unreadable", error.to_string()));
                    continue;
                }
            };
            if stored.seed.terminal_outcome.is_some() {
                continue;
            }
            match stored.seed.phase {
                meerkat_runtime::InputLifecycleState::Accepted
                | meerkat_runtime::InputLifecycleState::Queued => {}
                meerkat_runtime::InputLifecycleState::Staged
                | meerkat_runtime::InputLifecycleState::Applied
                | meerkat_runtime::InputLifecycleState::AppliedPendingConsumption => {
                    capture.uncarryable.push((
                        input_id,
                        "mid-run",
                        format!(
                            "input was {:?} at repair disposal; the disposal cancel \
                             terminalizes it and the sender observes that terminal",
                            stored.seed.phase
                        ),
                    ));
                    continue;
                }
                meerkat_runtime::InputLifecycleState::Consumed
                | meerkat_runtime::InputLifecycleState::Superseded
                | meerkat_runtime::InputLifecycleState::Coalesced
                | meerkat_runtime::InputLifecycleState::Abandoned => continue,
                other => {
                    // #[non_exhaustive]: an unknown phase cannot be proven
                    // pending-and-idle, so it is not carried — but it IS
                    // named before disposal destroys it.
                    capture.uncarryable.push((
                        input_id,
                        "unrecognized-phase",
                        format!("input was in unrecognized lifecycle phase {other:?}"),
                    ));
                    continue;
                }
            }
            match classify_pending_input_for_carry(stored.state.persisted_input) {
                PendingInputCarry::Carry(input) => {
                    capture.carryable.push(CarriedMemberInput {
                        original_input_id: input_id,
                        admission_sequence: stored.seed.admission_sequence,
                        input: *input,
                    });
                }
                PendingInputCarry::Uncarryable { class, reason } => {
                    capture.uncarryable.push((input_id, class, reason));
                }
            }
        }
        capture
            .carryable
            .sort_by_key(|entry| entry.admission_sequence.unwrap_or(u64::MAX));
        Ok(capture)
    }

    /// Loud pre-disposal record of what the repair is about to do with a
    /// member's queued work: what it will carry, and — per item, with ids —
    /// what it is about to DESTROY because no carry lane exists for it.
    fn log_pending_ingress_before_repair_disposal(
        &self,
        member_id: &MobAgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        capture: &PendingIngressCapture,
    ) {
        if capture.is_empty() {
            return;
        }
        tracing::warn!(
            member_id = %member_id,
            session_id = %session_id,
            carryable = capture.carryable.len(),
            destroyed = capture.uncarryable.len(),
            "repair disposal found pending queued inputs on the member: carrying \
             the carryable set to the healed successor; anything listed below is \
             destroyed with the member"
        );
        for (input_id, class, reason) in &capture.uncarryable {
            tracing::warn!(
                member_id = %member_id,
                session_id = %session_id,
                input_id = %input_id,
                class,
                reason = %reason,
                "repair disposal DESTROYS a pending member input it cannot carry"
            );
        }
    }

    /// Re-admit captured queue work into the healed successor session through
    /// ordinary machine admission. Per-item failures are loud errors — the
    /// heal itself stands (a healed member minus one carried input is still
    /// strictly better than a Broken member), but every lost input is named.
    async fn readmit_carried_inputs(
        &self,
        member_id: &MobAgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        capture: PendingIngressCapture,
    ) {
        if capture.carryable.is_empty() {
            return;
        }
        let Ok(Some(authority)) = self.resolved_runtime_ingress_authority() else {
            // Unreachable in practice: a non-empty capture required the
            // authority. Fail loud rather than silently dropping.
            tracing::error!(
                member_id = %member_id,
                session_id = %session_id,
                lost = capture.carryable.len(),
                "runtime ingress authority disappeared between capture and carry; \
                 captured queued inputs are lost"
            );
            return;
        };
        let total = capture.carryable.len();
        let mut carried = 0usize;
        for entry in capture.carryable {
            let CarriedMemberInput {
                original_input_id,
                input,
                ..
            } = entry;
            let Some(input) = remint_carried_input_identity(input) else {
                tracing::error!(
                    member_id = %member_id,
                    session_id = %session_id,
                    original_input_id = %original_input_id,
                    "carried input has no re-identifiable header in this build; the \
                     input is lost"
                );
                continue;
            };
            let readmitted_input_id = input.id().clone();
            match authority.accept_input(session_id, input).await {
                Ok(meerkat_runtime::AcceptOutcome::Accepted { .. }) => {
                    carried += 1;
                    tracing::info!(
                        member_id = %member_id,
                        session_id = %session_id,
                        original_input_id = %original_input_id,
                        readmitted_input_id = %readmitted_input_id,
                        "carried a queued member input into the healed successor session"
                    );
                }
                Ok(other) => {
                    let outcome = match &other {
                        meerkat_runtime::AcceptOutcome::Deduplicated { .. } => "deduplicated",
                        meerkat_runtime::AcceptOutcome::Rejected { .. } => "rejected",
                        _ => "unrecognized",
                    };
                    tracing::error!(
                        member_id = %member_id,
                        session_id = %session_id,
                        original_input_id = %original_input_id,
                        outcome,
                        "successor admission did not accept a carried queued input; \
                         the input is lost"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        member_id = %member_id,
                        session_id = %session_id,
                        original_input_id = %original_input_id,
                        error = %error,
                        "failed to re-admit a carried queued input into the healed \
                         successor; the input is lost"
                    );
                }
            }
        }
        tracing::warn!(
            member_id = %member_id,
            session_id = %session_id,
            carried,
            total,
            "repair carried queued member inputs into the healed successor session"
        );
    }

    async fn member_wires(
        &self,
        require_reciprocal: bool,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        let members = self.handle.list_members_including_retiring().await;
        let member_runtimes = self
            .runtime_members
            .read()
            .await
            .iter()
            .map(|(runtime, member)| (member.clone(), runtime.clone()))
            .collect::<HashMap<_, _>>();
        let active_ids = members
            .iter()
            .map(|member| member.agent_identity.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        let mut directed = std::collections::BTreeSet::new();
        for member in &members {
            let a = member.agent_identity.to_string();
            for peer in &member.wired_to {
                let b = peer.to_string();
                if active_ids.contains(&b) {
                    directed.insert((a.clone(), b));
                }
            }
        }
        let edges = directed
            .iter()
            .filter(|(a, b)| !require_reciprocal || directed.contains(&(b.clone(), a.clone())))
            .map(|(a, b)| {
                if a <= b {
                    (a.clone(), b.clone())
                } else {
                    (b.clone(), a.clone())
                }
            })
            .collect::<std::collections::BTreeSet<_>>();
        // Wires are reported as runtime ids, so each roster member id has to be
        // resolved to the incarnation it names. It must NOT be derived from the
        // roster id.
        //
        // The old fallback decoded the roster id and parsed the result as an
        // AgentRuntimeId. Since the stable-identity lowering a roster id decodes
        // to a bare durable identity like `agent:alpha`, and `AgentRuntimeId`
        // validates only non-empty/no-space/no-slash - so that parse SUCCEEDS
        // and mints a runtime id that names no incarnation at all.
        //
        // The bookkeeping answers first. An identity member (labelled
        // `agent_identity` at every identity-first spawn) it holds no binding
        // for, such as one Core restored that this process has not
        // materialized, is resolved through the incarnation Core committed
        // for it: the read the identity runtime's binding epoch makes
        // (`live_member`), so the projection and the epoch name the same
        // incarnation and a present wire is never read as absent. A raw member
        // (a worker a member spawned, say) the identity topology never
        // manages: bound, its wires are filtered out downstream, so unbound
        // they are left out instead of failing every other wire.
        //
        // The roster listing carries no runtime id, so the committed
        // incarnation is a second read after it. A respawn between the two
        // reads is caught where a plan commits: the commit revalidates the
        // endpoints' bindings (`peer_bindings_moved`), the plan goes stale,
        // and it is made again from fresh reads.
        let endpoints = edges
            .iter()
            .flat_map(|(a, b)| [a.as_str(), b.as_str()])
            .collect::<std::collections::BTreeSet<_>>();
        let mut committed: HashMap<String, Option<AgentRuntimeId>> = HashMap::new();
        for member in &members {
            let member_id = member.agent_identity.to_string();
            if member_runtimes.contains_key(&member_id) || !endpoints.contains(member_id.as_str()) {
                continue;
            }
            let Some(identity) = member.labels.get("agent_identity") else {
                continue;
            };
            let runtime_id = self
                .committed_identity_member_runtime_id(identity, &member.agent_identity)
                .await?;
            committed.insert(member_id, runtime_id);
        }
        let resolve = |member_id: &str| -> Result<Option<AgentRuntimeId>, BridgeError> {
            let Some(runtime_id) = member_runtimes.get(member_id) else {
                return Ok(committed.get(member_id).cloned().flatten());
            };
            AgentRuntimeId::parse(runtime_id).map(Some).map_err(|error| {
                BridgeError::Mob(format!(
                    "recorded runtime binding {runtime_id} for roster member {member_id} is not \
                     a valid runtime id: {error}"
                ))
            })
        };
        let mut wires = Vec::new();
        for (a, b) in edges {
            if let (Some(runtime_a), Some(runtime_b)) = (resolve(&a)?, resolve(&b)?) {
                wires.push((runtime_a, runtime_b));
            }
        }
        Ok(wires)
    }

    /// The incarnation Core committed for the identity member `member_id`,
    /// labelled with `label` as its durable identity, read back from the
    /// roster as [`SessionBridge::live_member`] reads it; `None` when Core
    /// holds no such member. A label that names another member, or a read
    /// that fails, refuses.
    async fn committed_identity_member_runtime_id(
        &self,
        label: &str,
        member_id: &MobAgentIdentity,
    ) -> Result<Option<AgentRuntimeId>, BridgeError> {
        let identity = AgentIdentity::parse(label).map_err(|error| {
            BridgeError::Mob(format!(
                "roster member {member_id} carries an unusable agent_identity label {label}: \
                 {error}"
            ))
        })?;
        if member_id_for_identity(&identity) != *member_id {
            return Err(BridgeError::Mob(format!(
                "roster member {member_id} is labelled with identity {identity}, which is \
                 another member's"
            )));
        }
        match self.handle.get_member(member_id).await {
            Ok(Some(entry)) => public_runtime_id(&identity, &entry.agent_runtime_id).map(Some),
            Ok(None) => Ok(None),
            Err(error) => Err(BridgeError::Mob(format!(
                "reading the committed roster entry for {identity}: {error}"
            ))),
        }
    }

    async fn runtime_session_id(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Option<meerkat_core::types::SessionId> {
        self.runtime_sessions
            .read()
            .await
            .get(runtime_id.as_str())
            .cloned()
    }

    /// The role the committed roster member for `member_id` runs, or `None`
    /// when no member is committed (a spawn still in custody).
    async fn rostered_role(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<Option<meerkat_mob::ProfileName>, meerkat_mob::MobError> {
        Ok(self
            .handle
            .get_member(member_id)
            .await?
            .map(|entry| entry.role))
    }

    /// Whether the member a resume collided with has not taken the migration
    /// this resume declares: it is committed and runs a role other than the
    /// declared target. The mob actor's explicit resume restores members
    /// under their durable role before this bridge runs, so this is how a
    /// declared migration shows up on the boot path.
    async fn occupant_misses_declared_role(
        &self,
        migration: Option<&DeclaredRoleMigration>,
        member_id: &MobAgentIdentity,
    ) -> Result<Option<meerkat_mob::ProfileName>, meerkat_mob::MobError> {
        let Some(migration) = migration else {
            return Ok(None);
        };
        Ok(self
            .rostered_role(member_id)
            .await?
            .filter(|role| *role != migration.to_role))
    }

    /// After attaching to, or re-resuming, a member under a declared
    /// migration, the member must run the declared role. Anything else is a
    /// typed refusal and the bookkeeping this resume recorded is dropped, so
    /// a declaration is applied or refused, never silently ignored.
    async fn require_declared_role(
        &self,
        migration: Option<&DeclaredRoleMigration>,
        member_id: &MobAgentIdentity,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        let Some(migration) = migration else {
            return Ok(());
        };
        let refusal = match self.rostered_role(member_id).await {
            Ok(Some(role)) if role == migration.to_role => return Ok(()),
            Ok(observed) => role_migration_not_applied(
                identity,
                session_id,
                migration,
                observed.as_ref(),
                "the member this resume attached does not run the declared role",
            ),
            Err(error) => role_migration_not_applied(
                identity,
                session_id,
                migration,
                None,
                &format!("the member's role could not be read back: {error}"),
            ),
        };
        self.forget_member_bindings(member_id).await;
        Err(refusal)
    }
}

/// One read of Core's identity intent row for `identity`: the host-adoption
/// gate of the resume collision arm and of `ensure_member_live`, taken before
/// either attaches, ensures or retires anything (r3 Q7).
///
/// Keyed by the comms-safe ENCODED member id, as the roster is: the identity
/// store does not know the public alias.
async fn read_identity_intent(handle: &MobHandle, identity: &AgentIdentity) -> IdentityIntentRead {
    classify_identity_intent(
        handle
            .identity_intent(&member_id_for_identity(identity))
            .await,
    )
}

/// Classify one identity intent read outcome: a pure function of it, so a
/// table pins all six outcomes. Generic over the row, so it cannot look inside
/// one: a Present and an Absent intent are one answer by construction.
fn classify_identity_intent<T>(
    read: Result<meerkat_mob::identity::IdentityStoredObservation<T>, meerkat_mob::MobError>,
) -> IdentityIntentRead {
    match read {
        Ok(meerkat_mob::identity::IdentityStoredObservation::Missing) => {
            IdentityIntentRead::Missing
        }
        Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(_)) => {
            IdentityIntentRead::HostAdopted
        }
        Ok(meerkat_mob::identity::IdentityStoredObservation::Unsupported {
            evidence_digest,
            detail,
        }) => IdentityIntentRead::Unreadable(format!(
            "unsupported identity intent row (evidence {evidence_digest}): {detail}"
        )),
        Ok(meerkat_mob::identity::IdentityStoredObservation::Malformed {
            evidence_digest,
            detail,
        }) => IdentityIntentRead::Unreadable(format!(
            "malformed identity intent row (evidence {evidence_digest}): {detail}"
        )),
        Err(error) => {
            IdentityIntentRead::Unreadable(format!("identity intent read failed: {error}"))
        }
    }
}

/// The resume collision arm's host-adoption decision on one intent read.
/// `Ok` takes the legacy path; a host-adopted identity refuses typed; an
/// unreadable row refuses retryable (`ResumeRejected` of kind `Other`).
fn resume_collision_intent_gate(
    read: IdentityIntentRead,
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
) -> Result<(), BridgeError> {
    match read {
        IdentityIntentRead::Missing => Ok(()),
        IdentityIntentRead::HostAdopted => Err(BridgeError::HostAdopted {
            identity: identity.clone(),
        }),
        IdentityIntentRead::Unreadable(detail) => Err(resume_rejected(
            identity,
            session_id,
            &meerkat_mob::MobError::Internal(format!(
                "roster collision on {session_id}, but whether the host adopted {identity} \
                 cannot be read ({detail}); nothing was attached or retired (retryable once \
                 the identity intent row is readable)"
            )),
            "identity intent unreadable",
        )),
    }
}

/// `ensure_member_live`'s host-adoption decision on one intent read. `Ok`
/// ensures; a host-adopted identity refuses typed; an unreadable row refuses
/// retryable.
fn ensure_member_live_intent_gate(
    read: IdentityIntentRead,
    identity: &AgentIdentity,
) -> Result<(), BridgeError> {
    match read {
        IdentityIntentRead::Missing => Ok(()),
        IdentityIntentRead::HostAdopted => Err(BridgeError::HostAdopted {
            identity: identity.clone(),
        }),
        IdentityIntentRead::Unreadable(detail) => Err(BridgeError::Mob(format!(
            "ensuring the member of {identity}: whether the host adopted it cannot be read \
             ({detail}); nothing was ensured (retryable once the identity intent row is \
             readable)"
        ))),
    }
}

/// What the durable, non-actor authorities say about a resume source before
/// a repair retires the member that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResumeSourceProbe {
    /// An authority knows the session, or no durable authority is installed
    /// to ask: the repair goes ahead, as it did before the guard existed.
    NotAbsent,
    /// At least one authority answered and none knows the session: the
    /// member holds the only live copy, so the repair refuses before its
    /// retire.
    ConfirmedAbsent,
    /// An authority's read failed, so absence is unknown. The repair refuses
    /// retryable before its retire and disposes nothing.
    Unanswered {
        authority: &'static str,
        error: String,
    },
}

impl ResumeSourceProbe {
    fn unanswered(
        session_id: &meerkat_core::types::SessionId,
        authority: &'static str,
        error: &dyn std::fmt::Display,
    ) -> Self {
        tracing::warn!(
            session_id = %session_id,
            authority,
            error = %error,
            "resume source absence probe failed; the repair refuses before its retire \
             (retryable)"
        );
        Self::Unanswered {
            authority,
            error: error.to_string(),
        }
    }
}

/// The delivery repair's retire precondition. `Ok` lets the repair go on to
/// its retire. A confirmed-absent source refuses `Unavailable { Absent }`, so
/// the caller decides whether the identity wakes on a fresh session; an
/// unanswered probe refuses `SourceUnanswered`, which is retryable. Neither
/// refusal disposes anything.
fn delivery_repair_retire_gate(
    probe: ResumeSourceProbe,
    session_id: &meerkat_core::types::SessionId,
) -> Result<(), BridgeError> {
    match probe {
        ResumeSourceProbe::NotAbsent => Ok(()),
        ResumeSourceProbe::ConfirmedAbsent => Err(BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::Unavailable {
                reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
            },
            session_id: session_id.clone(),
            detail: format!(
                "delivery repair refused before its retire: the resume source {session_id} is \
                 confirmed absent, so retiring the member would destroy the only live copy of \
                 the session; nothing was disposed"
            ),
        }),
        ResumeSourceProbe::Unanswered { authority, error } => Err(BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::SourceUnanswered { authority },
            session_id: session_id.clone(),
            detail: format!(
                "delivery repair refused before its retire: the {authority} could not say \
                 whether the resume source {session_id} exists ({error}); nothing was disposed \
                 (retryable)"
            ),
        }),
    }
}

/// The collision repair's retire precondition, with the same three verdicts.
/// A confirmed-absent source keeps the collision arm's `resume_rejected`
/// refusal; an unanswered probe refuses `SourceUnanswered`, which is
/// retryable. Neither refusal retires anything.
fn collision_retire_gate(
    probe: ResumeSourceProbe,
    identity: &AgentIdentity,
    session_id: &meerkat_core::types::SessionId,
) -> Result<(), BridgeError> {
    match probe {
        ResumeSourceProbe::NotAbsent => Ok(()),
        ResumeSourceProbe::ConfirmedAbsent => Err(resume_rejected(
            identity,
            session_id,
            &meerkat_mob::MobError::Internal(format!(
                "collision retire refused: the resume source for {session_id} is confirmed \
                 absent, so retiring the stale member would destroy the only live copy of the \
                 session"
            )),
            "collision retire precondition",
        )),
        ResumeSourceProbe::Unanswered { authority, error } => Err(BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::SourceUnanswered { authority },
            session_id: session_id.clone(),
            detail: format!(
                "collision retire precondition: the {authority} could not say whether the \
                 resume source for {session_id} exists ({error}); nothing was retired \
                 (retryable)"
            ),
        }),
    }
}

impl MobSessionBridge {
    /// Wait, bounded by the actor admission budget, for a spawn that is still
    /// in asynchronous custody for `member_id` to settle, then adopt its
    /// result instead of retiring it.
    ///
    /// The build in custody is a resume of this very identity (the client
    /// retry shape: a send timed out and was re-sent while the first spawn sat
    /// behind a backlog), so its completion IS the materialization this call
    /// was asked for. Convergence is read from the machine-state projection,
    /// the read the rest of this bridge already trusts. The only spawn issued
    /// here is the periodic non-destructive re-offer described on
    /// [`IN_FLIGHT_SPAWN_PROBE_INTERVAL`]. Nothing here retires anything, and
    /// a build that converged onto a DIFFERENT session is refused typed, not
    /// adopted and not destroyed.
    async fn await_in_flight_spawn_and_attach(
        &self,
        member_id: &MobAgentIdentity,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        spawn_spec: &SpawnMemberSpec,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        let budget = self.actor_admission_budget();
        let started = tokio::time::Instant::now();
        let deadline = started + budget;
        let mut next_probe = started + IN_FLIGHT_SPAWN_PROBE_INTERVAL;
        tracing::info!(
            identity = %identity,
            member_id = %member_id,
            session_id = %session_id,
            budget_secs = budget.as_secs(),
            "resume_session collided with a spawn still in asynchronous custody for this \
             member; awaiting its convergence instead of retiring it"
        );
        loop {
            // Adopted only once Core has committed the member's roster entry:
            // the session binding can be projected before the spawn's roster
            // commit, and attaching in between reads no committed entry.
            if self
                .handle
                .resolve_bridge_session_id(member_id)
                .await
                .as_ref()
                == Some(session_id)
                && matches!(self.handle.get_member(member_id).await, Ok(Some(_)))
            {
                tracing::info!(
                    identity = %identity,
                    member_id = %member_id,
                    session_id = %session_id,
                    waited_ms = started.elapsed().as_millis() as u64,
                    "the spawn in custody converged onto the resumed session; adopted rather \
                     than retired"
                );
                let agent_runtime_id = self
                    .attach_committed_member(identity, member_id, session_id)
                    .await?;
                // NOT Resumed: this call did not materialize the member, so
                // the caller must register its own authority from its record
                // before treating the session as resumed (idempotent).
                return Ok(ResumeSessionOutcome::AttachedPendingRegistration {
                    session_id: session_id.clone(),
                    agent_runtime_id,
                });
            }
            match self.handle.get_member(member_id).await {
                Ok(Some(_)) => {
                    // Committed. Bound elsewhere means this resume can neither
                    // adopt it nor destroy it; a binding not yet projected
                    // means keep polling.
                    if let Some(bound) = self.handle.resolve_bridge_session_id(member_id).await
                        && bound != *session_id
                    {
                        return Err(resume_rejected(
                            identity,
                            session_id,
                            &meerkat_mob::MobError::Internal(format!(
                                "the spawn in custody for {member_id} converged onto session \
                                 {bound}, not the resumed session {session_id}; refusing to \
                                 adopt or retire it (retryable once identity intent and the \
                                 durable record agree)"
                            )),
                            "in-flight spawn converged elsewhere",
                        ));
                    }
                }
                Ok(None) => {
                    if tokio::time::Instant::now() >= next_probe {
                        next_probe = tokio::time::Instant::now() + IN_FLIGHT_SPAWN_PROBE_INTERVAL;
                        match self.spawn_member_spec(spawn_spec.clone()).await {
                            Ok(()) => {
                                tracing::info!(
                                    identity = %identity,
                                    member_id = %member_id,
                                    session_id = %session_id,
                                    waited_ms = started.elapsed().as_millis() as u64,
                                    "the spawn in custody ended without a member; this resume \
                                     materialized the session itself"
                                );
                                let agent_runtime_id = self
                                    .attach_committed_member(identity, member_id, session_id)
                                    .await?;
                                return Ok(ResumeSessionOutcome::Resumed {
                                    session_id: session_id.clone(),
                                    agent_runtime_id,
                                });
                            }
                            Err(error) if is_member_already_exists_error(&error) => {}
                            Err(error) => {
                                return Err(resume_rejected(
                                    identity,
                                    session_id,
                                    &error,
                                    "resume retry after in-flight spawn",
                                ));
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        identity = %identity,
                        member_id = %member_id,
                        error = %error,
                        "roster read failed while awaiting a spawn in custody; retrying"
                    );
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(resume_rejected(
                    identity,
                    session_id,
                    &meerkat_mob::MobError::Internal(format!(
                        "a spawn for {member_id} was still in asynchronous custody when this \
                         resume collided with it, and it did not converge within {}s; nothing \
                         was retired (retryable once the in-flight spawn settles)",
                        budget.as_secs()
                    )),
                    "in-flight spawn convergence",
                ));
            }
            tokio::time::sleep(CONVERGENCE_HANDOFF_POLL).await;
        }
    }

    /// Precondition probe for the collision-repair retire: confirmed absence
    /// of the resume target in DURABLE, NON-ACTOR authority. The raw injected
    /// `session_store` row is deliberately NOT consulted here: since the
    /// 0.8.11 store-owned repin, runtime-backed compositions persist through
    /// RuntimeStore authority and never project into a raw injected
    /// SessionStore, so an empty raw row is not evidence of absence there.
    ///
    /// Every probe here must be answerable WITHOUT the session's live actor:
    /// this runs while the stale member may be wedged mid-turn, and an
    /// actor-routed read (`service.read`) waits on the very member the
    /// repair is about to dispose - a self-deadlock, proven live at the
    /// meerkat 0.8.13 repin (the collision arm hung inside this probe).
    /// `session_known_to_archive_authority` is the durable disposal-routing
    /// predicate that answers from archive authority instead.
    ///
    /// Verdict: `ConfirmedAbsent` (refuse the destructive retire) only when
    /// the archive authority answers that it does not know the session. A
    /// probe fault is `Unanswered`: absence is unknown, so both repairs
    /// refuse retryable before their retire rather than risk destroying the
    /// only live copy. It is logged at warn, naming the session and the
    /// authority. With no session service installed the verdict is
    /// `NotAbsent`, and the repair goes ahead as it did before this guard.
    async fn probe_resume_source(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> ResumeSourceProbe {
        let Some(service) = self.session_service.as_ref() else {
            return ResumeSourceProbe::NotAbsent;
        };
        match service.session_known_to_archive_authority(session_id).await {
            Ok(true) => ResumeSourceProbe::NotAbsent,
            Ok(false) => ResumeSourceProbe::ConfirmedAbsent,
            Err(error) => ResumeSourceProbe::unanswered(session_id, "archive authority", &error),
        }
    }

    /// After a rejected resume, verify the durable session actually survived.
    /// On meerkat 0.7.28 and earlier the spawn rollback compensated late
    /// resume-spawn failures by archiving the session it was resuming (Bug I,
    /// ask 31; fixed in 0.7.29: resumed provisions restore to durable idle, and
    /// retired-with-intact-snapshot sessions auto-revive on resume). The probe
    /// stays as a regression tripwire: a GONE result on 0.7.29 or later is
    /// always a platform bug worth a loud, immediate signal.
    async fn verify_durable_session_after_rejected_resume(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) {
        let Some(store) = self.session_store.as_ref() else {
            return;
        };
        match store.load_meta(session_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                tracing::error!(
                    identity = %identity,
                    session_id = %session_id,
                    "durable session is GONE after the rejected resume (Bug I class); this \
                     should be impossible on meerkat >=0.7.29 (ask 31: non-destructive resume \
                     rollback + auto-revival) — report upstream; recovery needs a pre-damage \
                     store restore"
                );
            }
            Err(error) => {
                tracing::warn!(
                    identity = %identity,
                    session_id = %session_id,
                    %error,
                    "could not verify durable session presence after the rejected resume"
                );
            }
        }
    }

    /// Resolve the inline definition profile backing `spec.profile`, used as
    /// the base when a draft model override must be projected into the typed
    /// `override_profile` spawn owner. Realm-ref bindings resolve to `None`.
    fn base_profile_for_spec(&self, spec: &DurableAgentSpec) -> Option<meerkat_mob::Profile> {
        self.handle
            .definition()
            .resolve_inline_profile(&spec.profile)
            .cloned()
    }

    /// Resume-divergence tripwire: when the durable session restores a
    /// model/provider different from the profile's declaration and no
    /// `resume_overrides` mask covers the field, say so at INFO — once per
    /// identity per boot. Ordinary inline profiles intentionally remain
    /// unmasked so durable routing changes such as a realized `brain_swap`
    /// survive cold restart. Explicit model/provider masks are normalized as a
    /// coherent pair by
    /// `crate::mob_handle_runtime::auto_mark_declared_resume_overrides`.
    /// Realm-ref profiles do NOT fire: `base_profile_for_spec` resolves inline
    /// bindings only, and the `RealmProfileStore` is not threaded into this
    /// bridge, so a realm profile edit that loses to durable metadata is
    /// currently silent (tracked follow-up: thread the realm store and resolve
    /// declarations via `MobDefinition::resolve_profile`).
    ///
    /// Never fails the resume: metadata read faults are skipped at debug.
    async fn log_unmasked_resume_divergence(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        base_profile: Option<&meerkat_mob::Profile>,
        session_id: &meerkat_core::types::SessionId,
    ) {
        let Some(profile) = base_profile else {
            return;
        };
        let mask = profile.resume_override_mask();
        if mask.model && mask.provider {
            return;
        }
        {
            let logged = self
                .resume_divergence_logged
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if logged.contains(identity.as_str()) {
                return;
            }
        }
        let Some(service) = self.session_service.as_ref() else {
            return;
        };
        let metadata = match service.load_persisted_session_metadata(session_id).await {
            Ok(Some(view)) => view.session_metadata,
            Ok(None) => None,
            Err(error) => {
                tracing::debug!(
                    identity = %identity,
                    session_id = %session_id,
                    %error,
                    "resume-divergence check skipped: durable metadata read failed"
                );
                None
            }
        };
        let Some(metadata) = metadata else {
            return;
        };
        // The declaration the profile (plus a draft model pin) would apply if
        // it were masked — mirrors the candidate side of meerkat-mob's
        // `effective_resumed_session_llm_identity`.
        let declared_model = draft.model.as_ref().unwrap_or(&profile.model);
        let divergence = unmasked_resume_divergence(
            &mask,
            declared_model,
            profile.provider,
            &metadata.model,
            metadata.provider,
        );
        if divergence.model || divergence.provider {
            // One line, BOTH pairs: the production cutover incident's first symptom
            // was a (model, provider) pair mismatch, and a model-only line
            // hid the half that mattered.
            tracing::info!(
                identity = %identity,
                profile = %spec.profile.as_str(),
                session_id = %session_id,
                restored_model = %metadata.model,
                restored_provider = %metadata.provider.as_str(),
                profile_model = %declared_model,
                profile_provider = ?profile.provider.map(|provider| provider.as_str()),
                model_unmasked_divergent = divergence.model,
                provider_unmasked_divergent = divergence.provider,
                "resume restored an LLM identity (model, provider) that differs from the \
                 profile declaration; durable metadata wins for the unmasked fields (no \
                 resume_overrides mask covers them)"
            );
            self.resume_divergence_logged
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(identity.as_str().to_string());
        }
    }

    async fn resolve_runtime_session_id(
        &self,
        runtime_id: &AgentRuntimeId,
        member_id: &MobAgentIdentity,
        missing_message: &'static str,
        deadline: &ActorAdmissionDeadline,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        // `resolve_bridge_session_id` reads the machine-state watch
        // projection; it never enters the actor queue, so there is nothing to
        // bound here.
        if let Some(session_id) = self.handle.resolve_bridge_session_id(member_id).await {
            self.remember_runtime_session(runtime_id, &session_id).await;
            return Ok(session_id);
        }

        // `get_member` faults (machine command/transport errors) must not be
        // laundered into "member absent"; surface them to the caller.
        if member_id.as_str() != runtime_id.as_str()
            && deadline
                .bound(
                    "resolve_session.get_member",
                    member_id,
                    self.handle.get_member(member_id),
                )
                .await?
                .map_err(|err| BridgeError::Mob(err.to_string()))?
                .is_some()
            && let Some(session_id) = self.runtime_session_id(runtime_id).await
        {
            return Ok(session_id);
        }

        Err(BridgeError::Mob(missing_message.to_string()))
    }

    async fn repair_member_for_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        member_id: &MobAgentIdentity,
        member_entry_before_delivery: Option<(meerkat_mob::ProfileName, BTreeMap<String, String>)>,
    ) -> Result<(), BridgeError> {
        // Resume-repair first: the recorded durable session IS the
        // conversation. `MobHandle::respawn` retires and spawns FRESH — it
        // rotates the bridge session and abandons the transcript (the production
        // `identity_alias_respawn_rotation` data-loss class). When we know
        // the durable session and the member's role, rebuild the member ONTO
        // that session instead. Every resume refusal fails the delivery typed,
        // Core's typed Absent included: whether the identity wakes on a fresh
        // session is the caller's decision, never this bridge's. A resume
        // source that no durable authority knows is refused BEFORE the
        // retire, as the resume collision arm does: the member then holds the
        // session's only live copy and its queued inputs, and a refusal
        // disposes nothing. The legacy respawn below remains only for the
        // case where we lack the material for a resume spec.
        // Fidelity note: like the legacy path, the rebuilt spec carries
        // role + labels only — deliver-time repair cannot re-run the host
        // customizer (that rebuild belongs to the runtime's materialize
        // seam).
        if let (Some(session_id), Some((role, labels))) = (
            self.runtime_session_id(runtime_id).await,
            member_entry_before_delivery.clone(),
        ) {
            // Precondition FIRST: the retire below destroys the member. When
            // no durable authority knows the session, the member holds its
            // only live copy and the resume after the retire could only answer
            // Absent, so refuse typed now, with nothing disposed; the caller
            // decides whether the identity wakes on a fresh session. When an
            // authority cannot answer, absence is unknown: refuse retryable,
            // also before the retire.
            if let Err(refusal) = delivery_repair_retire_gate(
                self.probe_resume_source(&session_id).await,
                &session_id,
            ) {
                tracing::warn!(
                    runtime_id = %runtime_id,
                    member_id = %member_id,
                    session_id = %session_id,
                    error = %refusal,
                    "delivery repair refused before its retire; nothing was disposed, and the \
                     caller decides"
                );
                return Err(refusal);
            }
            // The repair below starts with a destructive retire: capture the
            // wedged member's queued inputs FIRST so the healed successor can
            // re-admit them instead of losing them with the disposal (production
            // run 33758a41).
            let capture = self
                .capture_pending_member_ingress(&session_id)
                .await
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            self.log_pending_ingress_before_repair_disposal(member_id, &session_id, &capture);
            match self
                .resume_repair_member(
                    runtime_id,
                    member_id,
                    role.clone(),
                    labels.clone(),
                    &session_id,
                )
                .await
            {
                Ok(()) => {
                    self.readmit_carried_inputs(member_id, &session_id, capture)
                        .await;
                    return Ok(());
                }
                Err(err) => {
                    if !capture.carryable.is_empty() {
                        tracing::error!(
                            runtime_id = %runtime_id,
                            member_id = %member_id,
                            session_id = %session_id,
                            lost = capture.carryable.len(),
                            "delivery repair failed after its retire; the captured \
                             queued inputs are lost with the disposed member"
                        );
                    }
                    return Err(err);
                }
            }
        }
        match self.handle.respawn(member_id.clone(), None).await {
            Ok(_) => Ok(()),
            Err(respawn_err) => match classify_member_repair_respawn_failure(&respawn_err) {
                MemberRepairRespawnFailure::DegradedTopologyRestore { failed_peer_ids } => {
                    tracing::warn!(
                        runtime_id = %runtime_id,
                        member_id = %member_id,
                        failed_peer_count = failed_peer_ids.len(),
                        failed_peer_ids = ?failed_peer_ids,
                        "identity bridge respawn restored member with isolated peer edges; continuing delivery"
                    );
                    // meerkat-mob raises this only after the member/session is live; only peer edges are incomplete.
                    Ok(())
                }
                MemberRepairRespawnFailure::RecoverableCleanup => {
                    // A `get_member` fault must not be read as "member absent"
                    // (that would trigger a spurious re-spawn); fail the
                    // delivery repair instead.
                    if self
                        .handle
                        .get_member(member_id)
                        .await
                        .map_err(|err| BridgeError::Mob(err.to_string()))?
                        .is_none()
                        && let Some((role, labels)) = member_entry_before_delivery
                    {
                        self.handle
                            .ensure_member(fresh_member_spec_from_pre_delivery_entry(
                                member_id, role, labels,
                            ))
                            .await
                            .map_err(|e| BridgeError::Mob(e.to_string()))?;
                    }
                    Ok(())
                }
                MemberRepairRespawnFailure::Fatal(message) => Err(BridgeError::Mob(message)),
            },
        }
    }

    /// Rebuild a wedged member ONTO its recorded durable session
    /// (`MemberLaunchMode::Resume`): retire the stale roster entry if present
    /// (tolerating recoverable cleanup and member-absent), then spawn with the
    /// resume launch mode so the transcript survives the repair. The repaired
    /// member keeps the SAME bridge session id, so no continuity rebind is
    /// needed and the durable alias never rotates.
    async fn resume_repair_member(
        &self,
        runtime_id: &AgentRuntimeId,
        member_id: &MobAgentIdentity,
        role: meerkat_mob::ProfileName,
        labels: BTreeMap<String, String>,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        if let Err(err) = self.retire_session_owned_member_to_absence(member_id).await {
            return Err(BridgeError::Mob(format!(
                "repair retire before resume: {err}"
            )));
        }
        self.forget_runtime_member(runtime_id).await;

        let mut spec = SpawnMemberSpec::new(role, member_id.clone());
        spec.continuity_intent = repair_continuity_intent(&labels);
        if !labels.is_empty() {
            spec = spec.with_labels(labels);
        }
        spec.launch_mode = MemberLaunchMode::Resume {
            // 0.8.25: Resume gained `resume_from_role`; None = no migration authority.
            resume_from_role: None,
            bridge_session_id: session_id.clone(),
        };

        match self.spawn_member_spec(spec).await {
            Ok(()) => {
                self.remember_runtime_member(runtime_id, member_id).await;
                self.remember_runtime_session(runtime_id, session_id).await;
                tracing::info!(
                    runtime_id = %runtime_id,
                    member_id = %member_id,
                    session_id = %session_id,
                    "delivery repair resumed the member onto its durable session \
                     (transcript preserved, no session rotation)"
                );
                Ok(())
            }
            // Every refusal is typed, Core's typed Absent included
            // (`Unavailable { reason: Absent }`): this bridge spawns no fresh
            // session in its place, the caller decides.
            Err(err) => {
                let kind = classify_resume_error(&err);
                tracing::error!(
                    runtime_id = %runtime_id,
                    member_id = %member_id,
                    session_id = %session_id,
                    kind = ?kind,
                    error = %err,
                    "delivery repair resume rejected; the delivery fails typed and this \
                     bridge spawns no fresh session in its place"
                );
                Err(BridgeError::ResumeRejected {
                    kind,
                    session_id: session_id.clone(),
                    detail: format!("delivery repair resume: {err}"),
                })
            }
        }
    }

    /// Ops-owner bridge session for external (peer-only) member operations.
    ///
    /// meerkat 0.7.1 fails external member provisioning closed unless the
    /// spawn carries a generated owner binding (owner bridge session + ops
    /// registry; see `MultiBackendProvisioner::provision_member`). Prefer the
    /// mob's machine-bound owner bridge-session authority when it exists;
    /// otherwise mint one stable session id for this bridge — the runtime
    /// adapter creates local session resources for it on demand, exactly like
    /// meerkat's own external smoke supervisors.
    fn external_owner_bridge_session_id(&self) -> meerkat_core::types::SessionId {
        if let Some(authority) = self.handle.owner_bridge_session_lifecycle_authority() {
            return authority.bridge_session_id;
        }
        self.generated_external_owner_session
            .get_or_init(meerkat_core::types::SessionId::new)
            .clone()
    }

    /// Spawn a mob member from a fully-built spec, attaching the generated
    /// owner context that meerkat 0.7.1 requires for external (peer-only)
    /// bindings. Session-backed specs spawn through the plain path.
    async fn spawn_member_spec(
        &self,
        spawn_spec: SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        if spawn_spec_requires_generated_owner_context(&spawn_spec) {
            let owner_session_id = self.external_owner_bridge_session_id();
            Box::pin(
                self.handle
                    .spawn_spec_with_generated_owner_context(spawn_spec, owner_session_id),
            )
            .await
            .map(|_| ())
        } else {
            Box::pin(self.handle.spawn_spec(spawn_spec))
                .await
                .map(|_| ())
        }
    }
}

/// Project meerkat 0.7's tri-state peer connectivity into an inspect-level
/// reachable count, when the tri-state resolves one.
///
/// Only a fully resolved probe ([`WirePeerConnectivity::Known`] with no
/// unknown peers) contributes a live count. A partially resolved Known probe,
/// the not-applicable / probe-timed-out arms, and an uncomputed projection
/// return `None` so the caller falls back to the machine-owned wiring degree
/// (`wired_to.len()`) instead of projecting 0 — a freshly wired member has
/// peers regardless of whether a live probe resolved, and the sibling console
/// alias surface computes the same wire field from `wired_to`; the two
/// surfaces must agree.
fn peer_reachable_count_from_connectivity(
    connectivity: Option<&meerkat_contracts::WirePeerConnectivity>,
) -> Option<usize> {
    match connectivity {
        Some(meerkat_contracts::WirePeerConnectivity::Known { snapshot })
            if snapshot.unknown_peer_count == 0 =>
        {
            Some(snapshot.reachable_peer_count)
        }
        Some(meerkat_contracts::WirePeerConnectivity::Known { .. }) => None,
        Some(
            meerkat_contracts::WirePeerConnectivity::NotApplicable
            | meerkat_contracts::WirePeerConnectivity::ProbeTimedOut,
        )
        | None => None,
    }
}

/// External (peer-only) member provisioning on meerkat 0.7.1 requires a
/// machine-minted owner context; plain `spawn_spec` fails closed by design.
pub(crate) fn spawn_spec_requires_generated_owner_context(spawn_spec: &SpawnMemberSpec) -> bool {
    matches!(
        spawn_spec.binding,
        Some(meerkat_mob::RuntimeBinding::External { .. })
    )
}

fn spec_uses_external_binding(spec: &DurableAgentSpec) -> bool {
    matches!(spec.backend, Some(meerkat_mob::MobBackendKind::External))
        || matches!(
            spec.binding.as_ref(),
            Some(meerkat_contracts::WireRuntimeBinding::External { .. })
        )
}

/// The roster member id of a durable identity: the comms-safe encoding of the
/// identity itself (see [`member_id_for_spawn_spec`]).
fn member_id_for_identity(identity: &AgentIdentity) -> MobAgentIdentity {
    crate::member_comms_id::mob_member_id(identity.as_str())
}

/// The public runtime id of the incarnation Core committed for `identity`.
///
/// NOT `runtime_id.to_string()`: that Display emits `{identity}:{generation}`,
/// which only carried the public `rt:` marker while the roster identity WAS
/// the runtime alias. Post-lowering it drops the marker silently.
fn public_runtime_id(
    identity: &AgentIdentity,
    runtime_id: &meerkat_mob::ids::AgentRuntimeId,
) -> Result<AgentRuntimeId, BridgeError> {
    AgentRuntimeId::parse(&crate::member_comms_id::public_runtime_alias(runtime_id)).map_err(
        |error| {
            BridgeError::Mob(format!(
                "the committed roster entry for {identity} carries an unusable runtime id \
                 {runtime_id}: {error}"
            ))
        },
    )
}

fn member_id_for_spawn_spec(spec: &DurableAgentSpec) -> MobAgentIdentity {
    // The roster id is the comms-safe encoding of the DURABLE identity, for
    // every binding. `MemberCommsName` is fail-closed on identifier-unsafe
    // names (no `:`), so the encoding is required; Meta's public alias space
    // is unchanged.
    //
    // It must be the durable identity and not a runtime id. Meerkat defines
    // `AgentRuntimeId` as a stable identity plus a monotonically increasing
    // generation, where different generations are successive incarnations of
    // the SAME member, and `SpawnMemberSpec.identity` is the roster identity
    // whose per-identity generation counter Meerkat itself owns
    // (`MobActor::mint_spawn_generation`). Lowering a runtime id into that
    // slot nested one identity inside another and made the roster id
    // per-incarnation: durable mob state keyed on it belonged to a single
    // binding, so it could not survive a respawn or a restart, and a resumed
    // event log replayed a previous incarnation's roster against a freshly
    // minted generation.
    member_id_for_identity(&spec.identity)
}

/// Default OpenAI prompt-cache routing key for `identity`.
///
/// `prompt_cache_key` is a routing hint only: it steers a request onto a
/// cache-warm backend and has no effect on the completion. Meerkat sends none
/// by default, so without this every identity competes in one anonymous
/// routing pool.
///
/// The key is a total function of the durable identity id and nothing else —
/// no uuid, timestamp, generation, runtime id, session id or other
/// process-local value — so it reproduces byte-for-byte across restarts,
/// respawns and revivals. Per-identity (not per-profile) is the correct
/// bucket: two identities sharing a profile still carry different
/// transcripts, and therefore different cacheable prefixes.
fn identity_prompt_cache_key(identity: &AgentIdentity) -> String {
    format!("meta:{}", identity.as_str())
}

/// Fill every unset knob on `target` from `defaults`.
///
/// Explicit knobs always win. The tag half delegates to meerkat's own
/// `ProviderTag::merge_missing_from`, so a provider-family conflict is a typed
/// fault rather than a silent union of unrelated provider bags. Without the
/// merge, a draft that sets one cache knob would replace the profile's
/// declaration wholesale and silently drop knobs declared in the mob
/// definition.
fn merge_provider_params_missing_from(
    target: &mut ProviderParamsOverride,
    defaults: &ProviderParamsOverride,
) -> Result<(), BridgeError> {
    fn fill<T: Clone>(target: &mut Option<T>, default: &Option<T>) {
        if target.is_none()
            && let Some(value) = default
        {
            *target = Some(value.clone());
        }
    }

    fill(&mut target.temperature, &defaults.temperature);
    fill(&mut target.top_p, &defaults.top_p);
    fill(&mut target.max_output_tokens, &defaults.max_output_tokens);
    fill(&mut target.reasoning, &defaults.reasoning);
    fill(
        &mut target.thinking_budget_tokens,
        &defaults.thinking_budget_tokens,
    );
    match (target.provider_tag.as_mut(), defaults.provider_tag.as_ref()) {
        (Some(tag), Some(default)) => tag
            .merge_missing_from(default)
            .map_err(|error| BridgeError::InvalidInput(format!("provider params: {error}")))?,
        (None, Some(default)) => target.provider_tag = Some(default.clone()),
        _ => {}
    }
    Ok(())
}

/// Whether this spawn's effective model is served by OpenAI.
///
/// `prompt_cache_key` lives on the OpenAI provider tag, and meerkat rejects a
/// tag from the wrong provider family at its merge seam
/// (`ProviderParamsMergeError::ProviderTagMismatch`), which would fail the
/// turn for an Anthropic- or Gemini-backed identity. The default is injected
/// only when the provider is provably OpenAI.
fn spawn_is_openai_backed(
    draft: &AgentBuildDraft,
    base_profile: Option<&meerkat_mob::Profile>,
) -> bool {
    // A draft model override replaces the profile's model and meerkat
    // re-infers the owner from the new id (the pinned branch below clears
    // `provider` for exactly that reason), so the new id decides.
    if let Some(model) = draft.model.as_deref() {
        return matches!(
            meerkat_models::infer_provider(model),
            Some(meerkat_core::Provider::OpenAI)
        );
    }
    match base_profile {
        Some(profile) => match profile.provider {
            Some(provider) => matches!(provider, meerkat_core::Provider::OpenAI),
            None => matches!(
                meerkat_models::infer_provider(&profile.model),
                Some(meerkat_core::Provider::OpenAI)
            ),
        },
        None => false,
    }
}

/// Give `params` the identity's default prompt-cache routing key when the
/// caller supplied none.
///
/// A caller-supplied key is never replaced, and a tag belonging to another
/// provider family is left untouched — that tag is the caller's explicit
/// provider identity, not a slot to overwrite.
fn ensure_default_prompt_cache_key(params: &mut ProviderParamsOverride, identity: &AgentIdentity) {
    match params.provider_tag.as_mut() {
        Some(ProviderTag::OpenAi(tag)) => {
            if tag.prompt_cache_key.is_none() {
                tag.prompt_cache_key = Some(identity_prompt_cache_key(identity));
            }
        }
        Some(_) => {}
        None => {
            params.provider_tag = Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_key: Some(identity_prompt_cache_key(identity)),
                ..Default::default()
            }));
        }
    }
}

/// Land the draft's provider params, plus the per-identity prompt-cache
/// default, on `spawn_spec`.
///
/// `SpawnMemberSpec` has no field-scoped provider-params seam: the only
/// carrier meerkat reads is `override_profile.provider_params` (mob
/// `build.rs`, `config.provider_params = profile.provider_params.clone()`).
/// A declared override therefore requires a profile snapshot.
///
/// A spawn that declares nothing is deliberately left on the field-scoped
/// path: meerkat persists the snapshot as `effective_profile_override` and
/// reuses it on internal revival, so minting one for every OpenAI identity
/// just to carry a routing hint would freeze definition drift fleet-wide.
fn apply_provider_params(
    spawn_spec: &mut SpawnMemberSpec,
    spec: &DurableAgentSpec,
    draft: &AgentBuildDraft,
    base_profile: Option<&meerkat_mob::Profile>,
) -> Result<(), BridgeError> {
    let declared = draft.provider_params.clone();
    if declared.is_none() && spawn_spec.override_profile.is_none() {
        return Ok(());
    }

    let Some(mut profile) = spawn_spec
        .override_profile
        .take()
        .or_else(|| base_profile.cloned())
    else {
        // Declared params with no inline profile to carry them (realm-ref
        // binding). Fail closed: dropping them silently is the failure mode
        // that reads downstream as "caching just doesn't work".
        return Err(BridgeError::InvalidInput(format!(
            "identity {} declares provider_params but profile '{}' resolves to no inline \
             definition profile to carry them; declare provider_params on the realm profile \
             instead",
            spec.identity.as_str(),
            spec.profile.as_str(),
        )));
    };

    let mut params = match declared {
        Some(mut declared) => {
            if let Some(profile_params) = profile.provider_params.as_ref() {
                merge_provider_params_missing_from(&mut declared, profile_params)?;
            }
            // The declaration must reach RESUMED sessions too: unmasked,
            // meerkat restores provider_params from durable session metadata
            // and the draft's value is inert on every identity that already
            // has a session.
            if !profile
                .resume_overrides
                .contains(&ResumeOverrideField::ProviderParams)
            {
                profile
                    .resume_overrides
                    .push(ResumeOverrideField::ProviderParams);
            }
            declared
        }
        // No declaration, but a snapshot is already in flight for another
        // reason (pinned-provider model override): the routing-hint default
        // rides it at no extra cost. The key is deterministic, so the value
        // persisted into session metadata on create restores identically on
        // resume without a mask entry.
        None => profile.provider_params.clone().unwrap_or_default(),
    };

    if spawn_is_openai_backed(draft, base_profile) {
        ensure_default_prompt_cache_key(&mut params, &spec.identity);
    }

    profile.provider_params = (!params.is_empty()).then_some(params);
    spawn_spec.override_profile = Some(profile);
    Ok(())
}

/// Build a `SpawnMemberSpec` from identity-first types, wiring draft fields.
///
/// `base_profile` is the resolved definition profile for `spec.profile`; it is
/// needed when the draft carries a model override (meerkat 0.7 removed
/// `SpawnMemberSpec::model_override` in favor of the typed `override_profile`
/// owner, so a model override is expressed as the role profile with the model
/// swapped) and when the draft carries provider params, which meerkat reads
/// only off a profile.
pub(crate) fn build_spawn_spec(
    spec: &DurableAgentSpec,
    draft: &AgentBuildDraft,
    base_profile: Option<&meerkat_mob::Profile>,
) -> Result<SpawnMemberSpec, BridgeError> {
    let mid = member_id_for_spawn_spec(spec);
    // A DurableAgentSpec is the trusted host roster's identity declaration.
    // This ingress creates that identity, not an agent-requested descendant.
    // Fresh creation is therefore an explicit Root fact. Resume and reset
    // still go through native transitions that preserve predecessor proof.
    let mut spawn_spec = SpawnMemberSpec::host_root(spec.profile.clone(), mid);
    // Every identity-first spawn carries the durable-identity marker, keyed
    // by the identity: the create, the held create, both resume branches and
    // the reset successor all build here. Core's durable identity member read
    // lists exactly the members whose spawn carried it, which is how the boot
    // roster compile finds a member the roster dropped while Meta was down.
    spawn_spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
        continuity_key: spec.identity.as_str().to_string(),
    };

    if let Some(message) = spec.initial_message.as_ref() {
        spawn_spec = spawn_spec.with_initial_message(message.clone());
    }
    if let Some(runtime_mode) = spec.runtime_mode_override {
        spawn_spec = spawn_spec.with_runtime_mode(runtime_mode);
    }
    spawn_spec.backend = spec.backend;
    if let Some(binding) = spec.binding.clone() {
        spawn_spec.binding = runtime_binding_from_wire(binding);
    }
    if let Some(placement) = spec.placement.as_ref() {
        spawn_spec.placement = Some(meerkat_mob::machines::mob_machine::HostId::from(
            placement.0.clone(),
        ));
    }
    if let Some(ref ctx) = draft.app_context {
        spawn_spec = spawn_spec.with_context(ctx.clone());
    }
    let mut labels = draft.labels.clone();
    labels.insert(
        "agent_identity".to_string(),
        spec.identity.as_str().to_string(),
    );
    labels.insert(
        "profile_name".to_string(),
        spec.profile.as_str().to_string(),
    );
    if !labels.is_empty() {
        spawn_spec = spawn_spec.with_labels(labels);
    }
    if !draft.additional_instructions.is_empty() {
        spawn_spec = spawn_spec.with_additional_instructions(draft.additional_instructions.clone());
    }
    if let Some(model) = draft.model.as_ref() {
        match base_profile {
            // A pinned provider (or self-hosted server binding) belongs to the
            // base profile's ORIGINAL model id, and meerkat applies
            // `model_override` without re-inferring the provider. Keep the
            // whole-profile snapshot for pinned profiles only — accepting the
            // definition-drift freeze `model_override` was built to end.
            //
            // The snapshot's provider is the DRAFT model's catalog owner, not
            // None: on resume the profile's resume-override mask applies
            // model and provider as a pair, and a None provider falls back to
            // the durable one — minting exactly the invalid (model, provider)
            // pair the production cutover rejected typed. Catalog-unknown ids keep
            // None (definition `[models.<id>]` / config-entry resolution
            // downstream, durable-wins on resume).
            Some(base) if base.provider.is_some() || base.self_hosted_server_id.is_some() => {
                let mut profile = base.clone();
                profile.model = model.clone();
                profile.provider = meerkat_models::canonical().infer_provider(model);
                profile.self_hosted_server_id = None;
                spawn_spec.override_profile = Some(profile);
            }
            // meerkat 0.7.29 (ask 29, Bug G′): field-scoped seam. The model
            // pin is reapplied over the CURRENT definition profile on every
            // materialization (cold restore, revival, respawn), so definition
            // drift in tools/skills/peer posture keeps reaching reprofiled
            // members. Also covers realm-ref bindings, which the old
            // whole-profile path had to skip with a warning.
            _ => {
                spawn_spec.model_override = Some(model.clone());
            }
        }
    }
    if let Some(system_prompt) = draft.system_prompt.as_ref() {
        spawn_spec.system_prompt_override =
            Some(SpawnSystemPromptOverride::Replace(system_prompt.clone()));
    }
    if let Some(dispatcher) = draft.local_external_tools.dispatcher() {
        spawn_spec.external_tools = Some(dispatcher);
    }
    spawn_spec.compaction_curator_override = draft.compaction_curator.curator();

    // Runs after the model block: it composes with whatever snapshot that
    // block installed instead of clobbering it.
    apply_provider_params(&mut spawn_spec, spec, draft, base_profile)?;

    Ok(spawn_spec)
}

/// Build the spawn spec for a RESUME of `session_id`.
///
/// Differs from a fresh spawn in two deliberate ways:
/// - `launch_mode = Resume`, so meerkat loads the persisted session
///   (conversation history intact) instead of creating a new one.
/// - `system_prompt_override` is cleared: RESUME AUTHORS NOTHING (meerkat
///   0.8.11 prompt contract, 2026-07-29 ruling). `Message::System` is an
///   ordinary ordered authored transcript message; prompt policy
///   materializes only when creating an empty transcript, and the only way
///   to change durable instructions mid-thread is a caller EXPLICITLY
///   authoring a new System message as part of a turn
///   (`StartTurnRequest.system_messages`). Re-sending assembled
///   configuration on every activation would append a byte-duplicate System
///   per boot — the prompt-refresh depth leak reborn — so the +0-on-neutral-
///   resume invariant is achieved STRUCTURALLY by the absence of an append
///   command here, not by an unchanged-value comparison anywhere. Dynamic
///   regenerated projections belong in the transient turn-context seam (or
///   tool reads), never in boot-time prompt authoring.
pub(crate) fn build_resume_spawn_spec(
    spec: &DurableAgentSpec,
    draft: &AgentBuildDraft,
    base_profile: Option<&meerkat_mob::Profile>,
    session_id: &meerkat_core::types::SessionId,
    resume_from_role: Option<meerkat_mob::ProfileName>,
) -> Result<SpawnMemberSpec, BridgeError> {
    let mut spawn_spec = build_spawn_spec(spec, draft, base_profile)?;
    spawn_spec.launch_mode = MemberLaunchMode::Resume {
        // 0.8.25: Resume gained `resume_from_role`. `None` means NO migration
        // authority and is the default for every resume.
        //
        // Pass-through only. The value is the caller's previous role for this
        // exact identity or, without one, the host's declaration in the
        // activation's boot payload ([`RoleMigrationDeclaration`]); this
        // builder never synthesizes it, and the bridge never retries a resume
        // that Meerkat refused. Meerkat re-verifies the declared predecessor
        // against durable state and refuses on mismatch, and ignores the
        // declaration entirely once the roles already agree - so a stale
        // declaration is inert rather than a repeated restamp.
        resume_from_role,
        bridge_session_id: session_id.clone(),
    };
    spawn_spec.system_prompt_override = None;
    Ok(spawn_spec)
}

/// Build a boot's declaration map.
///
/// Exact identities only. The map is keyed by [`AgentIdentity`], so a
/// declaration can never match by prefix, suffix or case: an identity the host
/// did not name exactly gets no authority.
pub(crate) fn role_migration_declaration_map(
    declarations: impl IntoIterator<Item = RoleMigrationDeclaration>,
) -> HashMap<AgentIdentity, meerkat_mob::ProfileName> {
    declarations
        .into_iter()
        .map(|declaration| (declaration.identity, declaration.from_role))
        .collect()
}

/// The migration declared for this exact identity, if any.
pub(crate) fn declared_role_migration_in(
    declarations: &HashMap<AgentIdentity, meerkat_mob::ProfileName>,
    identity: &AgentIdentity,
) -> Option<meerkat_mob::ProfileName> {
    declarations.get(identity).cloned()
}

fn runtime_binding_from_wire(
    binding: meerkat_contracts::WireRuntimeBinding,
) -> Option<meerkat_mob::RuntimeBinding> {
    match binding {
        meerkat_contracts::WireRuntimeBinding::Session => {
            Some(meerkat_mob::RuntimeBinding::Session)
        }
        meerkat_contracts::WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity.resolve().ok()?;
            Some(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

#[async_trait]
impl SessionBridge for MobSessionBridge {
    fn observe_actor_loop_health(&self, health: Arc<ActorLoopHealth>) {
        *self
            .actor_loop_health
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(health);
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        let deadline = self.admission_deadline();
        let outcome = deadline
            .bound(
                "reload.reload_member_registration",
                &mid,
                self.handle.reload_member_registration(&mid),
            )
            .await?
            .map_err(|error| {
                classify_reload_mob_error(
                    "reload.reload_member_registration",
                    &mid,
                    error,
                    &deadline,
                )
            })?;
        Ok(Some(bridge_member_reload(outcome)))
    }

    async fn release_member_run_starts_for_published_tools(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<(), BridgeError> {
        match self
            .handle
            .release_member_run_starts(
                member_id,
                meerkat_mob::HostRunStartHoldReason::ToolsNotPublished,
            )
            .await
        {
            // Not a member of this mob: nothing holds it.
            Ok(()) | Err(meerkat_mob::MobError::MemberNotFound(_)) => Ok(()),
            Err(error) => Err(BridgeError::Mob(format!(
                "releasing the ToolsNotPublished run-start hold of {member_id} failed: {error}"
            ))),
        }
    }

    async fn member_durability(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<super::types::MemberDurability> {
        let machine = crate::mob_handle_runtime::observed_runtime_adapter(
            self.session_service.as_ref()?.as_ref(),
        )?;
        member_durability_from_machine(machine.durability_reload_required(session_id).await)
    }

    async fn member_registration(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_runtime::RuntimeSessionRegistrationWitness> {
        let machine = crate::mob_handle_runtime::observed_runtime_adapter(
            self.session_service.as_ref()?.as_ref(),
        )?;
        machine
            .current_session_registration_witness(session_id)
            .await
    }

    async fn raw_member_alias_exists(&self, alias: &str) -> Result<bool, BridgeError> {
        let members = self.handle.list_members_including_retiring().await;
        let authoritative_members = self.runtime_members.read().await;
        Ok(members.iter().any(|member| {
            crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()) == alias
                && !authoritative_members
                    .values()
                    .any(|owned| owned == member.agent_identity.as_str())
                && crate::member_comms_id::durable_identity_label(&member.labels) != Some(alias)
        }))
    }

    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        match self.committed_boundary_recoverer.as_ref() {
            Some(recoverer) => recoverer.recover_committed_boundary(session_id).await,
            // No heal authority composed in: keep the pre-incident contract
            // honest rather than pretending the head was checked.
            None => Ok(CommittedBoundaryRepair::Unsupported),
        }
    }

    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        // The successor occupies the SAME roster row, so this is respawn - one
        // authority transition that snapshots the stable identity, mints the
        // successor generation, terminally retires the old row, and spawns the
        // exact successor. It is deliberately not retire + create: splitting it
        // reopens the compensation races a single transition avoids.
        let roster_id = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());

        // REPROFILE IS PART OF THE RESET CONTRACT, and respawn cannot deliver it.
        //
        // `adopt_current_roster_spec_for_reset` means the rebuilt session uses
        // the CURRENT roster spec, so a reset after the host reprofiles a member
        // is supposed to come back under the new profile. Meerkat's respawn
        // promises the same profile, labels, wiring and mode as the row it
        // replaces. So when the requested spec has drifted from the committed
        // row, this transition would silently rebuild under the OLD profile and
        // report success - a reset that did not reset.
        //
        // ONE authoritative transition, carrying the successor's spec. Meerkat
        // owns predecessor retirement, successor generation and fence minting,
        // fresh session creation, and topology restoration; the spec's identity
        // is the already-encoded roster key and is consumed exactly once.
        //
        // This replaces a plain `respawn` plus a refusal for any profile change:
        // respawn preserves the predecessor's profile, so reprofiling through it
        // was impossible and had to be refused rather than silently kept.
        let mut successor = build_spawn_spec(spec, draft, None).map_err(|error| {
            BridgeError::Mob(format!(
                "lowering the successor spec for destructive reset of {identity}: {error}"
            ))
        })?;
        successor.identity = roster_id.clone();
        // Required by the operation: a successor always receives a newly minted
        // session, so Resume and Fork are rejected upstream. Parent auto-wiring
        // is likewise rejected - existing topology is restored, not re-derived.
        successor.launch_mode = meerkat_mob::launch::MemberLaunchMode::Fresh;
        successor.auto_wire_parent = false;

        let receipt = self
            .handle
            .respawn_with_successor_spec(successor)
            .await
            .map_err(|error| {
                respawn_bridge_error(
                    &format!("atomic successor respawn for destructive reset of {identity}"),
                    &error,
                )
            })?;
        let _ = &receipt;

        // Read what the machine actually committed rather than predicting it:
        // the successor generation is MobMachine's to mint.
        let agent_runtime_id = self.committed_runtime_id(identity, &roster_id).await?;
        let session_id = self
            .handle
            .resolve_bridge_session_id(&roster_id)
            .await
            .ok_or_else(|| {
                BridgeError::Mob(format!(
                    "committed successor for {identity} has no bridge session yet; reset cannot \
                     commit continuity against an unbound successor"
                ))
            })?;

        // Both mappings, so nothing downstream has to derive an incarnation.
        self.remember_runtime_member(&agent_runtime_id, &roster_id)
            .await;
        self.remember_runtime_session(&agent_runtime_id, &session_id)
            .await;
        Ok(MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id),
        })
    }

    async fn capture_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<QueuedMemberInputs, BridgeError> {
        // The delivery repair's own capture (production run 33758a41): the
        // same classes are carried, and the rest is named before the member
        // goes.
        let capture = self
            .capture_pending_member_ingress(session_id)
            .await
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.log_pending_ingress_before_repair_disposal(
            &member_id_for_identity(identity),
            session_id,
            &capture,
        );
        Ok(QueuedMemberInputs {
            capture: Some(capture),
        })
    }

    async fn readmit_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        inputs: QueuedMemberInputs,
    ) {
        if let Some(capture) = inputs.capture {
            self.readmit_carried_inputs(&member_id_for_identity(identity), session_id, capture)
                .await;
        }
    }

    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let mid = member_id_for_spawn_spec(spec);
        let mut spawn_spec =
            build_spawn_spec(spec, draft, self.base_profile_for_spec(spec).as_ref())?;
        self.apply_compaction_floor(identity, spec, &mut spawn_spec)?;

        self.spawn_member_spec(spawn_spec)
            .await
            .map_err(|error| spawn_bridge_error("create_session", &error))?;
        let agent_runtime_id = self.committed_runtime_id(identity, &mid).await?;
        self.remember_runtime_member(&agent_runtime_id, &mid).await;

        // A peer-only member is bound to no session; a session-backed member
        // must report the one Core committed.
        let session_id = if spec_uses_external_binding(spec) {
            self.handle.resolve_bridge_session_id(&mid).await
        } else {
            Some(
                self.resolve_runtime_session_id(
                    &agent_runtime_id,
                    &mid,
                    "member spawned but has no session ID",
                    &self.admission_deadline(),
                )
                .await?,
            )
        };
        if let Some(session_id) = session_id.as_ref() {
            self.remember_runtime_session(&agent_runtime_id, session_id)
                .await;
        }
        Ok(MemberRuntimeBinding {
            agent_runtime_id,
            session_id,
        })
    }

    async fn create_session_held(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        pass: &super::held_bootstrap::HeldCreatePass,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let mid = member_id_for_spawn_spec(spec);
        let mut spawn_spec =
            build_spawn_spec(spec, draft, self.base_profile_for_spec(spec).as_ref())?;
        self.apply_compaction_floor(identity, spec, &mut spawn_spec)?;
        if spawn_spec_requires_generated_owner_context(&spawn_spec) {
            return Err(BridgeError::Mob(format!(
                "the externally bound member of {identity} cannot be held before its publication"
            )));
        }
        // The spawn's own wait is bounded by the pass's setup deadline. A
        // member not held by then is still owned by Core: its settlement is
        // awaited here, so the create never fails while Core may still
        // construct or clean up the member.
        let held = match Box::pin(self.handle.spawn_held_until(spawn_spec, pass.deadline()))
            .await
            .map_err(|error| spawn_bridge_error("create_session_held", &error))?
        {
            meerkat_mob::HeldSpawnStart::Held(held) => held,
            meerkat_mob::HeldSpawnStart::Unsettled(settlement) => {
                return Err(BridgeError::Mob(match settlement.settled().await {
                    Ok(()) => format!(
                        "the held member of {identity} was not held before its setup deadline"
                    ),
                    Err(error) => format!(
                        "the held member of {identity} was not held before its setup deadline; \
                         its spawn settled with: {error}"
                    ),
                }));
            }
        };
        pass.arrive(
            identity,
            held.finished_surface().clone(),
            held.controller_route_report().clone(),
        );
        match pass.wait_decision().await {
            super::held_bootstrap::HeldDecision::Activate => {
                #[cfg(test)]
                {
                    super::held_bootstrap::held_activation_park(
                        identity,
                        super::held_bootstrap::HeldActivationPark::BeforeFirstPoll,
                    )
                    .await;
                }
                // The pass chose `Activate`; Core accepts or expires this
                // member's own activation when it is first polled, against
                // the same deadline, and either outcome is kept as is.
                Box::pin(held.activate())
                    .await
                    .map_err(|error| spawn_bridge_error("create_session_held", &error))?;
                #[cfg(test)]
                {
                    super::held_bootstrap::held_activation_park(
                        identity,
                        super::held_bootstrap::HeldActivationPark::AfterNativeActivation,
                    )
                    .await;
                }
            }
            super::held_bootstrap::HeldDecision::Abort => {
                return Err(BridgeError::Mob(match held.abort().await {
                    Ok(()) => format!("the held member of {identity} was aborted by its producer"),
                    Err(error) => format!(
                        "the held member of {identity} was aborted by its producer; its abort \
                         did not confirm cleanup: {error}"
                    ),
                }));
            }
        }
        // A held member is always session-backed: an externally bound one is
        // refused above.
        let agent_runtime_id = self.committed_runtime_id(identity, &mid).await?;
        self.remember_runtime_member(&agent_runtime_id, &mid).await;
        let session_id = self
            .resolve_runtime_session_id(
                &agent_runtime_id,
                &mid,
                "member spawned but has no session ID",
                &self.admission_deadline(),
            )
            .await?;
        self.remember_runtime_session(&agent_runtime_id, &session_id)
            .await;
        Ok(MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id),
        })
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        // The caller decides the migration (see `declared_role_migration`);
        // the bridge applies none of its own.
        let resume_from_role = resume_from_role.cloned();
        self.log_unmasked_resume_divergence(
            identity,
            spec,
            draft,
            self.base_profile_for_spec(spec).as_ref(),
            session_id,
        )
        .await;
        if spec_uses_external_binding(spec) {
            let mut spawn_spec = build_resume_spawn_spec(
                spec,
                draft,
                self.base_profile_for_spec(spec).as_ref(),
                session_id,
                resume_from_role,
            )?;
            self.apply_compaction_floor(identity, spec, &mut spawn_spec)?;
            let mid = member_id_for_spawn_spec(spec);
            self.spawn_member_spec(spawn_spec).await.map_err(|error| {
                resume_rejected(identity, session_id, &error, "external-binding resume")
            })?;
            let agent_runtime_id = self
                .attach_committed_member(identity, &mid, session_id)
                .await?;
            return Ok(ResumeSessionOutcome::Resumed {
                session_id: session_id.clone(),
                agent_runtime_id,
            });
        }

        // MemberLaunchMode::Resume loads the existing session from the session
        // store (conversation history intact).
        let mut spawn_spec = build_resume_spawn_spec(
            spec,
            draft,
            self.base_profile_for_spec(spec).as_ref(),
            session_id,
            resume_from_role,
        )?;
        self.apply_compaction_floor(identity, spec, &mut spawn_spec)?;

        let mid = member_id_for_spawn_spec(spec);
        let declared_migration = DeclaredRoleMigration::of(&spawn_spec);

        match self.spawn_member_spec(spawn_spec.clone()).await {
            Ok(()) => {
                let agent_runtime_id = self
                    .attach_committed_member(identity, &mid, session_id)
                    .await?;
                Ok(ResumeSessionOutcome::Resumed {
                    session_id: session_id.clone(),
                    agent_runtime_id,
                })
            }
            Err(error) if is_member_already_exists_error(&error) => {
                // HOST ADOPTION FIRST, from one identity intent read, before
                // any attach, retire or convergence wait. Meta writes no Core
                // identity intent row, so a row Core holds for this identity
                // is the host's: the host adopted the identity and owns its
                // materialization, a Present intent naming this very session
                // included. Only an identity with no row reaches the legacy
                // path below. A row this read cannot classify says nothing
                // about who manages the identity, so it refuses retryable.
                resume_collision_intent_gate(
                    read_identity_intent(&self.handle, identity).await,
                    identity,
                    session_id,
                )?;
                // A DECLARED ROLE MIGRATION the mob's own restore did not take.
                // The mob actor's explicit resume restores every member under
                // its durable role before this bridge runs, and meerkat lets
                // neither that restore nor its spawn customizer change the
                // profile. So on the boot path the migration this Spawn carries
                // meets an occupant still on the old role. Every branch below
                // either applies the migration (retire the occupant to absence,
                // then resume the same session with `resume_from_role`, the
                // sequence meerkat's own refusal names: "stop or retire its
                // current runtime before restamping") or refuses typed. None
                // adopts the occupant as it is.
                let occupant_missing_migration = match self
                    .occupant_misses_declared_role(declared_migration.as_ref(), &mid)
                    .await
                {
                    Ok(observed) => observed,
                    Err(probe_error) => {
                        return Err(resume_rejected(
                            identity,
                            session_id,
                            &probe_error,
                            "declared role migration occupant probe",
                        ));
                    }
                };
                // The occupant runs neither the target nor the declared
                // predecessor: the declaration does not describe this member,
                // and Meerkat would refuse the restamp anyway. Refuse BEFORE
                // any destructive step rather than retire a member the resume
                // cannot replace.
                if let (Some(migration), Some(observed)) = (
                    declared_migration.as_ref(),
                    occupant_missing_migration.as_ref(),
                ) && *observed != migration.from_role
                {
                    return Err(role_migration_not_applied(
                        identity,
                        session_id,
                        migration,
                        Some(observed),
                        "the restored member's role is not the declared predecessor role",
                    ));
                }
                // NO INTENT ROW: the legacy authority must prove EXACT custody
                // of the occupant before any destructive step. Without that
                // proof the collision refuses, so a realm with no intent rows
                // can boot while an ambiguous collision still cannot destroy
                // anything.
                //
                // Custody means the occupant BELONGS to this identity, not
                // that it is the same session. A stale predecessor incarnation
                // is exactly what the preconditioned retire exists to clear,
                // so requiring session equality here was inverted: equality
                // would mean the occupant is the member we want and must be
                // attached, not destroyed.
                let occupant_is_ours =
                    crate::member_comms_id::live_member_names_identity_or_incarnation(
                        crate::member_comms_id::runtime_alias_str(mid.as_str()).as_ref(),
                        identity.as_str(),
                        None,
                    );
                // If the occupant is ours AND already bound to the very
                // session being resumed, it is not a stale predecessor - it IS
                // the member. Retiring it and re-resuming the same session is
                // what produced "session already has a different operation":
                // the destructive path was aimed at the thing it was trying to
                // recover. ...unless a declared migration has not been taken:
                // then this occupant is the member under the wrong role, and
                // the preconditioned retire below is how the migration is
                // applied.
                if occupant_is_ours
                    && occupant_missing_migration.is_none()
                    && self.handle.resolve_bridge_session_id(&mid).await.as_ref()
                        == Some(session_id)
                {
                    let agent_runtime_id = self
                        .attach_committed_member(identity, &mid, session_id)
                        .await?;
                    // NOT Resumed. Custody and bookkeeping are reconciled, but
                    // this identity has no authoritative registration, and the
                    // continuity adapter will refuse a head-canonical session
                    // whose owner was never registered. Only the caller holds
                    // the record needed to register.
                    return Ok(ResumeSessionOutcome::AttachedPendingRegistration {
                        session_id: session_id.clone(),
                        agent_runtime_id,
                    });
                }
                if !occupant_is_ours {
                    return Err(resume_rejected(
                        identity,
                        session_id,
                        &meerkat_mob::MobError::Internal(format!(
                            "roster collision on {session_id} with no identity intent row, and \
                             the legacy authority cannot prove the occupant {mid} belongs to \
                             {identity} (retryable: refusing to retire an occupant whose \
                             custody is unproven)"
                        )),
                        "legacy custody unproven",
                    ));
                }
                // The occupant is ours. Whether it is STALE is a separate
                // question the roster answers: a collision with
                // no roster member came from MobMachine's in-flight spawn
                // guard, and a build in progress for this very identity is
                // convergence to await, not debris to clear (a 2026-09-22 production incident
                // item 3: the inert retire and the retry that collided with
                // the just-finished build marked a healthy identity Broken).
                match classify_colliding_occupant(self.handle.get_member(&mid).await) {
                    CollidingOccupant::InFlight => {
                        let outcome = self
                            .await_in_flight_spawn_and_attach(
                                &mid,
                                identity,
                                session_id,
                                &spawn_spec,
                            )
                            .await?;
                        self.require_declared_role(
                            declared_migration.as_ref(),
                            &mid,
                            identity,
                            session_id,
                        )
                        .await?;
                        return Ok(outcome);
                    }
                    // Committed, and already bound to the very session being
                    // resumed: this IS the member, not a stale predecessor.
                    // The production deployment had a 245 ms window between the collision and the
                    // spawn completing; a resume that classifies inside that
                    // window sees the roster row and must adopt it exactly as
                    // the in-flight path does. Retiring it would destroy a
                    // healthy member to re-resume the session it already runs
                    // (the resume-source precondition below checks the durable
                    // source, not this binding, so it cannot guard this).
                    // A declared migration the occupant has not taken is
                    // the exception: the retire below applies it.
                    CollidingOccupant::Committed
                        if occupant_missing_migration.is_none()
                            && self.handle.resolve_bridge_session_id(&mid).await.as_ref()
                                == Some(session_id) =>
                    {
                        tracing::info!(
                            identity = %identity,
                            member_id = %mid,
                            session_id = %session_id,
                            "resume_session collided with a committed member already bound to \
                             the resumed session; adopted rather than retired"
                        );
                        let agent_runtime_id = self
                            .attach_committed_member(identity, &mid, session_id)
                            .await?;
                        return Ok(ResumeSessionOutcome::AttachedPendingRegistration {
                            session_id: session_id.clone(),
                            agent_runtime_id,
                        });
                    }
                    // Committed and bound to a different session or to none:
                    // the preconditioned retire path below decides.
                    CollidingOccupant::Committed => {}
                    CollidingOccupant::Unobservable(probe_error) => {
                        return Err(resume_rejected(
                            identity,
                            session_id,
                            &meerkat_mob::MobError::Internal(format!(
                                "roster collision on {session_id}, but the roster could not be \
                                 read to tell a committed occupant from a spawn still in \
                                 custody ({probe_error}); refusing to retire (retryable)"
                            )),
                            "collision occupant probe",
                        ));
                    }
                }
                // Genuine roster collision: an in-process restart where the
                // previous member actor hasn't fully terminated, or a Broken
                // roster entry left by an earlier rejected resume. Retire the
                // collision and retry the RESUME — never a fresh spawn; the
                // durable session must stay bound to the identity.
                if let (Some(migration), Some(observed)) = (
                    declared_migration.as_ref(),
                    occupant_missing_migration.as_ref(),
                ) {
                    tracing::info!(
                        identity = %identity,
                        session_id = %session_id,
                        from_role = %migration.from_role,
                        to_role = %migration.to_role,
                        observed_role = %observed,
                        "applying a declared role migration: retiring the member the mob \
                         restored under its durable role, then resuming the same session under \
                         the declared role"
                    );
                } else {
                    tracing::warn!(
                        identity = %identity,
                        session_id = %session_id,
                        error = %error,
                        "resume_session hit a roster collision; retiring the stale member and retrying resume"
                    );
                }
                // Preconditions FIRST (production run 33758a41): the retire below is
                // destructive — on the ephemeral runtime-store shape it takes
                // the stale member's in-memory state and queued inputs with
                // it. Before destroying anything, prove the session this
                // retry will resume from actually exists; a CONFIRMED-absent
                // resume source means the stale member holds the only live
                // state and retiring it destroys the session outright, and a
                // probe that cannot answer refuses retryable as well.
                collision_retire_gate(
                    self.probe_resume_source(session_id).await,
                    identity,
                    session_id,
                )?;
                let capture = match self.capture_pending_member_ingress(session_id).await {
                    Ok(capture) => capture,
                    Err(error) => {
                        return Err(resume_rejected(
                            identity,
                            session_id,
                            &error,
                            "collision retire deferred: queued inputs unobservable",
                        ));
                    }
                };
                self.log_pending_ingress_before_repair_disposal(&mid, session_id, &capture);
                if let Err(err) = self.retire_session_owned_member_to_absence(&mid).await {
                    return Err(resume_rejected(
                        identity,
                        session_id,
                        &err,
                        "collision retire before resume retry",
                    ));
                }
                // meerkat 0.7.29 (ask 32): retire disposition is machine-
                // authorized and incarnation-scoped — a retire that matches
                // no committed identity is inert for this retry, so the
                // 0.7.34 drain-poll between retire and respawn is gone.
                self.forget_member_bindings(&mid).await;
                if let Err(error) = self.spawn_member_spec(spawn_spec).await {
                    if !capture.carryable.is_empty() {
                        tracing::error!(
                            identity = %identity,
                            session_id = %session_id,
                            lost = capture.carryable.len(),
                            "the collision retire already destroyed the member's queued \
                             inputs and the resume retry failed; the captured inputs are \
                             lost with it"
                        );
                    }
                    self.verify_durable_session_after_rejected_resume(identity, session_id)
                        .await;
                    return Err(resume_rejected(
                        identity,
                        session_id,
                        &error,
                        "resume retry after collision",
                    ));
                }
                // Re-admit before reading the binding back: the carried inputs
                // must not ride on a roster read that could still fail.
                self.readmit_carried_inputs(&mid, session_id, capture).await;
                let agent_runtime_id = self
                    .attach_committed_member(identity, &mid, session_id)
                    .await?;
                self.require_declared_role(declared_migration.as_ref(), &mid, identity, session_id)
                    .await?;
                Ok(ResumeSessionOutcome::Resumed {
                    session_id: session_id.clone(),
                    agent_runtime_id,
                })
            }
            // Any other resume failure is refused typed, never answered with a
            // fresh spawn: rebinding the identity to a fresh empty session would
            // abandon a durable transcript (the downstream app's restart-loss
            // bug). Core's typed Absent is refused the same way: there is no
            // transcript to keep, and the caller decides whether the identity
            // wakes on a fresh session in its place.
            Err(error) => {
                if durable_snapshot_is_typed_absent(&error) {
                    // Core refused in spawn preparation and committed no
                    // member, so a runtime binding this bridge still keeps
                    // for the member names an incarnation that is gone.
                    self.forget_member_bindings(&mid).await;
                } else {
                    self.verify_durable_session_after_rejected_resume(identity, session_id)
                        .await;
                }
                Err(resume_rejected(
                    identity,
                    session_id,
                    &error,
                    "resume spawn",
                ))
            }
        }
    }

    /// A pure lookup: no durable read, no divergence detection, no inference.
    /// An identity the host did not name gets `None`, which carries zero
    /// migration authority.
    fn declared_role_migration(
        &self,
        identity: &AgentIdentity,
    ) -> Option<meerkat_mob::ProfileName> {
        declared_role_migration_in(&self.role_migration_declarations, identity)
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        let member_id = member_id_for_identity(identity);
        let read_failed = |error: meerkat_mob::MobError| {
            BridgeError::Mob(format!("reading the live member of {identity}: {error}"))
        };
        // The incarnation (the roster, through the actor) and the session (the
        // published machine state) are two reads, and a respawn can land
        // between them. Read the incarnation again after the session and keep
        // the pair only when it did not move: retry once, then refuse typed
        // rather than hand out a torn pair.
        for _ in 0..2 {
            let Some(before) = self
                .handle
                .get_member(&member_id)
                .await
                .map_err(read_failed)?
            else {
                return Ok(None);
            };
            let session_id = self.handle.resolve_bridge_session_id(&member_id).await;
            let Some(after) = self
                .handle
                .get_member(&member_id)
                .await
                .map_err(read_failed)?
            else {
                return Ok(None);
            };
            if after.agent_runtime_id == before.agent_runtime_id {
                return Ok(Some(MemberRuntimeBinding {
                    agent_runtime_id: public_runtime_id(identity, &after.agent_runtime_id)?,
                    session_id,
                }));
            }
        }
        Err(BridgeError::LiveMemberMoved {
            identity: identity.clone(),
        })
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        match self
            .handle
            .capture_member_delivery_scope(&member_id_for_identity(identity))
            .await
        {
            Ok(scope) => Ok(Some(scope)),
            Err(meerkat_mob::MobError::MemberNotFound(_)) => Ok(None),
            Err(error) => Err(BridgeError::Mob(format!(
                "capturing the delivery scope of {identity}: {error}"
            ))),
        }
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        // Host adoption first, before any other Core call, so a refusal
        // ensures nothing: a Core intent row for the identity is the host's.
        ensure_member_live_intent_gate(
            read_identity_intent(&self.handle, identity).await,
            identity,
        )?;
        let member_id = member_id_for_identity(identity);
        match self.handle.ensure_member_live(&member_id).await {
            // Core answers MemberNotFound both when it holds no member and for
            // a member bound to no session, which has nothing to revive. The
            // read below tells them apart.
            Ok(()) | Err(meerkat_mob::MobError::MemberNotFound(_)) => {}
            Err(error) => {
                return Err(classify_reload_mob_error(
                    "ensure_member_live",
                    &member_id,
                    error,
                    &self.admission_deadline(),
                ));
            }
        }
        let Some(binding) = self.live_member(identity).await? else {
            return Ok(None);
        };
        self.remember_runtime_member(&binding.agent_runtime_id, &member_id)
            .await;
        if let Some(session_id) = binding.session_id.as_ref() {
            self.remember_runtime_session(&binding.agent_runtime_id, session_id)
                .await;
        }
        Ok(Some(binding))
    }

    async fn identity_convergence_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        meerkat_mob::identity::IdentityStoredObservation<
            meerkat_mob::identity::IdentityConvergenceStatus,
        >,
        BridgeError,
    > {
        self.handle
            .identity_convergence_status(&member_id_for_identity(identity))
            .await
            .map_err(|error| {
                BridgeError::Mob(format!(
                    "reading the convergence status of {identity}: {error}"
                ))
            })
    }

    async fn identity_intent_present(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<bool>, BridgeError> {
        match self
            .handle
            .identity_intent(&member_id_for_identity(identity))
            .await
        {
            Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(record)) => {
                Ok(Some(matches!(
                    record.intent,
                    meerkat_mob::identity::IdentityIntent::Present { .. }
                )))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(BridgeError::Mob(format!(
                "reading the identity intent of {identity}: {error}"
            ))),
        }
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.handle
            .member_binding_history()
            .await
            .map_err(|error| BridgeError::Mob(format!("reading Core's member history: {error}")))
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.handle
            .last_member_binding(&member_id_for_identity(identity))
            .await
            .map_err(|error| {
                BridgeError::Mob(format!(
                    "reading the last member binding of {identity}: {error}"
                ))
            })
    }

    async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(self
            .member_binding_history()
            .await?
            .identity_management(&member_id_for_identity(identity)))
    }

    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        self.handle
            .durable_identity_members()
            .await
            .map_err(|error| {
                BridgeError::Mob(format!("reading Core's durable identity members: {error}"))
            })
    }

    async fn identity_labelled_members(
        &self,
    ) -> Result<Vec<meerkat_mob::AgentIdentity>, BridgeError> {
        Ok(self
            .handle
            .list_all_members()
            .await
            .into_iter()
            .filter(|entry| {
                crate::member_comms_id::durable_identity_label(&entry.labels).is_some_and(
                    |identity| {
                        crate::member_comms_id::live_member_is_identity(
                            entry.agent_identity.as_str(),
                            identity,
                        )
                    },
                )
            })
            .map(|entry| entry.agent_identity)
            .collect())
    }

    async fn broken_members(
        &self,
    ) -> Result<BTreeMap<meerkat_mob::AgentIdentity, String>, BridgeError> {
        Ok(crate::mob_handle_runtime::broken_members_of(&self.handle)
            .await
            .into_iter()
            .collect())
    }

    async fn durable_session_held(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        let Some(service) = self.session_service.as_ref() else {
            return Err(BridgeError::Unsupported {
                operation: "durable_session_held",
            });
        };
        service
            .load_retained_session_metadata(session_id)
            .await
            .map(|metadata| metadata.is_some())
            .map_err(|error| {
                BridgeError::Mob(format!(
                    "reading Core's retained metadata of session {session_id}: {error}"
                ))
            })
    }

    async fn session_outlives_member(
        &self,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        // A session service that persists no session (Core's ephemeral one)
        // keeps each session only in its live member.
        match self.session_service.as_ref() {
            Some(service) => Ok(service.supports_persistent_sessions()),
            None => Err(BridgeError::Unsupported {
                operation: "session_outlives_member",
            }),
        }
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        // Admission-only: no turn is in flight, so the receipt's completion is
        // an immediate Ok and propagating the resolution result here drops
        // nothing. Identical to the pre-receipt behaviour.
        let receipt = self
            .deliver_admitted_inner(
                runtime_id,
                delivery,
                BridgeSubmitMode::AdmissionOnly,
                None,
                ReceiptOutput::Terminal,
            )
            .await
            .map_err(BridgeError::from)?;
        receipt.wait().await.map_err(BridgeError::from)
    }

    async fn capture_member_delivery_scope(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<meerkat_mob::MemberDeliveryScope, ScopedDeliveryError> {
        let mid = self
            .member_id_for_runtime_id(runtime_id)
            .await
            .map_err(|error| ScopedDeliveryError::Rejected {
                detail: error.to_string(),
            })?;
        self.handle
            .capture_member_delivery_scope(&mid)
            .await
            .map_err(|error| match error {
                // A member with no session binding (peer-only or remotely
                // hosted) cannot be scope-bound at all.
                meerkat_mob::MobError::DeliveryScopeUnavailable { agent_identity, .. } => {
                    ScopedDeliveryError::Unsupported {
                        detail: format!(
                            "member '{agent_identity}': {}",
                            super::runtime::NO_NATIVE_SESSION_BINDING
                        ),
                    }
                }
                error => ScopedDeliveryError::Rejected {
                    detail: error.to_string(),
                },
            })
    }

    async fn submit_at_scope(
        &self,
        runtime_id: &AgentRuntimeId,
        scope: &meerkat_mob::MemberDeliveryScope,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        delivery_identity: &meerkat_mob::MobDeliveryIdentity,
    ) -> Result<meerkat_mob::WorkDeliveryReceipt, ScopedDeliveryError> {
        let mid = self
            .member_id_for_runtime_id(runtime_id)
            .await
            .map_err(|error| ScopedDeliveryError::StaleScope {
                mismatch: ScopeMismatch::RuntimeId,
                detail: error.to_string(),
            })?;
        if scope.agent_identity() != &mid {
            return Err(ScopedDeliveryError::StaleScope {
                mismatch: ScopeMismatch::Member,
                detail: format!(
                    "scope names member '{}', the identity is bound to member '{mid}'",
                    scope.agent_identity()
                ),
            });
        }
        // Exact content only: no system prompt, no injected context. The
        // correlation id rides as the interaction id, which meerkat requires
        // to agree with the delivery identity.
        let spec = internal_bridge_work_spec(
            content,
            None,
            &[],
            Some(delivery_identity.correlation_id.as_str()),
        );
        let deadline = self.admission_deadline();
        let submitted = deadline
            .bound(
                "deliver.submit_work_at_scope",
                &mid,
                self.handle
                    .submit_work_with_mode_and_delivery_identity_bounded(
                        scope,
                        spec,
                        handling_mode,
                        delivery_identity.clone(),
                        deadline.deadline.into_std(),
                    ),
            )
            .await
            .map_err(classify_scoped_bound_error)?;
        submitted.map_err(classify_scoped_submit_mob_error)
    }

    async fn recover_at_scope(
        &self,
        scope: &meerkat_mob::MemberDeliveryScope,
        delivery_identity: &meerkat_mob::MobDeliveryIdentity,
        deadline: std::time::Instant,
    ) -> Result<meerkat_mob::ScopedWorkRecovery, ScopedDeliveryError> {
        let spec =
            meerkat_mob::BoundedResultSpec::new("turn_output", super::types::TURN_OUTPUT_MAX_BYTES)
                .map_err(|error| ScopedDeliveryError::Rejected {
                    detail: error.to_string(),
                })?;
        self.handle
            .recover_bounded_work_at_scope(scope, delivery_identity, &spec, deadline)
            .await
            .map_err(|error| ScopedDeliveryError::Rejected {
                detail: error.to_string(),
            })
    }

    async fn retire_at_scope(
        &self,
        runtime_id: &AgentRuntimeId,
        scope: &meerkat_mob::MemberDeliveryScope,
    ) -> Result<(), ScopedRetireError> {
        self.handle
            .retire_at_scope(scope)
            .await
            .map_err(classify_scoped_retire_mob_error)?;
        // meerkat answered its terminal barrier: the scope's incarnation is
        // retired (or no incarnation is current and none is retiring).
        self.forget_runtime_member(runtime_id).await;
        Ok(())
    }

    async fn deliver_host_human_input(
        &self,
        runtime_id: &AgentRuntimeId,
        expected_session: &meerkat_core::SessionId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::SessionId, BridgeError> {
        let receipt = self
            .deliver_admitted_inner(
                runtime_id,
                delivery,
                BridgeSubmitMode::AdmissionOnly,
                Some(expected_session),
                ReceiptOutput::Terminal,
            )
            .await
            .map_err(BridgeError::from)?;
        receipt.wait().await.map_err(BridgeError::from)
    }

    /// Completion-bearing override: identical admission, returning the turn's
    /// receipt UNAWAITED so the caller can release its locks first.
    ///
    /// The trait's `deliver_awaiting_commit_*` default composes this with
    /// `wait()`, so there is no second admission path to drift.
    async fn begin_awaiting_commit(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        let mut delivery = BridgeDelivery::new(content.clone(), handling_mode);
        delivery.system_prompt = system_prompt.map(ToString::to_string);
        delivery.injected_context = injected_context.to_vec();
        delivery.interaction_id = interaction_id.map(ToString::to_string);
        self.deliver_admitted_inner(
            runtime_id,
            delivery,
            BridgeSubmitMode::CompletionBearing,
            None,
            ReceiptOutput::Terminal,
        )
        .await
    }

    fn tracks_turn_output(&self) -> bool {
        true
    }

    async fn governed_controller(
        &self,
        _runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
        let governed = |error| BridgeAdmissionError::GovernedInput(error);
        let source = self.controller_pin_source.as_ref().ok_or_else(|| {
            governed(GovernedInputError::Unsupported(
                "no controller pin source is installed for this bridge".to_string(),
            ))
        })?;
        let actor = source.live_actor(session_id).await.ok_or_else(|| {
            governed(GovernedInputError::Unsupported(
                "the member session has no live actor".to_string(),
            ))
        })?;
        // Retained per exact actor: a running actor answers the pin only
        // after its turn, so it is asked once per actor incarnation, and
        // again after recovery or replacement.
        if let Some(pin) = self
            .controller_pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .filter(|(pinned, _)| pinned == &actor)
            .map(|(_, pin)| pin.clone())
        {
            return Ok(pin);
        }
        let pin = source
            .pin_actor(&actor)
            .await
            .map_err(|reason| governed(GovernedInputError::Unsupported(reason)))?;
        self.controller_pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.clone(), (actor, pin.clone()));
        Ok(pin)
    }

    async fn attach_live_mcp_server(
        &self,
        session_id: &meerkat_core::types::SessionId,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        match self.controller_pin_source.as_ref() {
            Some(source) => {
                source
                    .try_attach_live_mcp_server(session_id, attachment)
                    .await
            }
            None => Err(meerkat_core::mcp_attachment::LiveMcpAttachError::Unsupported),
        }
    }

    async fn begin_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.governed_delivery_through(
            runtime_id,
            session_id,
            input,
            custody,
            GovernedDoor::External,
        )
        .await
    }

    async fn begin_internal_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.governed_delivery_through(
            runtime_id,
            session_id,
            input,
            custody,
            GovernedDoor::TrustedHostInternal,
        )
        .await
    }

    async fn begin_delivery_with_output(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.deliver_admitted_inner(
            runtime_id,
            delivery,
            BridgeSubmitMode::CompletionBearing,
            None,
            ReceiptOutput::TurnOutput,
        )
        .await
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        let store = self.session_store.as_ref().ok_or_else(|| {
            BridgeError::InvalidInput(
                "checkpoint requires a session store but none was configured".to_string(),
            )
        })?;

        let session = store
            .load(session_id)
            .await
            .map_err(|e| BridgeError::Mob(format!("failed to load session for checkpoint: {e}")))?
            .ok_or_else(|| {
                BridgeError::Mob(format!(
                    "session {session_id} not found in store for checkpoint"
                ))
            })?;

        let data = serde_json::to_vec(&session)
            .map_err(|e| BridgeError::Mob(format!("failed to serialize session: {e}")))?;

        Ok(SessionSnapshot { data })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        self.retire_session_owned_member_to_absence(&mid)
            .await
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.forget_runtime_member(runtime_id).await;
        Ok(())
    }

    async fn retire_reset_superseded_member(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        retire_reset_superseded(&mid, session_id, || self.handle.retire(mid.clone())).await?;

        if self
            .handle
            .list_all_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == mid)
        {
            return Err(BridgeError::Mob(format!(
                "reset retire reported success but retained roster anchor {mid}"
            )));
        }

        self.forget_runtime_member(runtime_id).await;
        self.unregister_session_runtime_state(session_id).await
    }

    async fn wire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        let member_a = self.member_id_for_runtime_id(a).await?;
        let member_b = self.member_id_for_runtime_id(b).await?;
        self.handle
            .wire(
                meerkat_mob::AgentIdentity::from(member_a.as_str()),
                member_b,
            )
            .await
            .map_err(|e| BridgeError::Mob(e.to_string()))
    }

    async fn wire_peers_batch(
        &self,
        edges: &[(AgentRuntimeId, AgentRuntimeId)],
    ) -> Result<(), BridgeError> {
        let mut member_edges = Vec::with_capacity(edges.len());
        for (a, b) in edges {
            let member_a = self.member_id_for_runtime_id(a).await?;
            let member_b = self.member_id_for_runtime_id(b).await?;
            member_edges.push((
                meerkat_mob::AgentIdentity::from(member_a.as_str()),
                meerkat_mob::AgentIdentity::from(member_b.as_str()),
            ));
        }
        match self.handle.wire_members_batch(member_edges.clone()).await {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .to_string()
                    .contains("does not support legacy external (peer-only) members") =>
            {
                // Meerkat's dense batch operation is intentionally local-only.
                // Its ordinary wire command is the generated reciprocal-trust
                // authority for local↔peer-only edges, so retry the normalized
                // edge set through that existing operation instead of
                // fabricating an external-peer side channel.
                for (member_a, member_b) in member_edges {
                    self.handle
                        .wire(member_a, member_b)
                        .await
                        .map_err(|error| BridgeError::Mob(error.to_string()))?;
                }
                Ok(())
            }
            Err(error) => Err(BridgeError::Mob(error.to_string())),
        }
    }

    async fn current_member_wires(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.member_wires(true).await
    }

    async fn current_member_wires_any_half(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.member_wires(false).await
    }

    async fn unwire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        let member_a = self.member_id_for_runtime_id(a).await?;
        let member_b = self.member_id_for_runtime_id(b).await?;
        match self
            .handle
            .unwire(
                meerkat_mob::AgentIdentity::from(member_a.as_str()),
                member_b,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let message = err.to_string();
                if message.contains("peer not found") || message.contains("not wired") {
                    Ok(())
                } else {
                    Err(BridgeError::Mob(message))
                }
            }
        }
    }

    async fn inspect_member(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        let deadline = self.admission_deadline();
        let snap = deadline
            .observe_member_status(&mid, || self.handle.member_status(&mid))
            .await?;
        let peer_reachable_count =
            match peer_reachable_count_from_connectivity(snap.peer_connectivity.as_ref()) {
                Some(count) => count,
                None => self
                    .handle
                    .get_member(&mid)
                    .await
                    .ok()
                    .flatten()
                    .map(|entry| entry.wired_to.len())
                    .unwrap_or(0),
            };
        Ok(MemberInspection {
            output_preview: snap.output_preview.clone(),
            is_final: snap.is_final,
            peer_reachable_count,
            preview_unavailable: snap.preview_unavailable,
        })
    }
}

impl MobSessionBridge {
    /// One governed delivery through `door`: the member lookup, the
    /// exact-session check, the custody hand-off and the definitive wait for
    /// native admission are the same for both doors; only the mob entry
    /// differs.
    async fn governed_delivery_through(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
        door: GovernedDoor,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        // `custody` settles `NotAdmitted` on any answer before submission.
        let governed = |error| BridgeAdmissionError::GovernedInput(error);
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        let spec =
            meerkat_mob::BoundedResultSpec::new("turn_output", super::types::TURN_OUTPUT_MAX_BYTES)
                .map_err(|error| BridgeAdmissionError::Mob(error.to_string()))?;
        let deadline = self.admission_deadline();
        // Bounded and before anything is submitted: a stalled or terminated
        // actor is refused here, definitely before admission.
        let entry = deadline
            .bound(
                "deliver.governed.get_member",
                &mid,
                self.handle.get_member(&mid),
            )
            .await?
            .map_err(|error| BridgeAdmissionError::Mob(error.to_string()))?
            .ok_or_else(|| governed(GovernedInputError::BindingChanged))?;
        // The input was bound for exactly this session: never retargeted.
        if entry.bridge_session_id() != Some(session_id) {
            return Err(governed(GovernedInputError::BindingChanged));
        }
        // Once submitted, native admission owns the custody's settlement, and
        // its answer is awaited to its definitive acknowledgement or error: no
        // observation deadline drops it, since native admission can still
        // accept after a dropped waiter. An error is carried as the mob's own
        // answer; the identity runtime acts on the custody settlement, never
        // on the error's kind.
        let Some(custody) = custody.submit() else {
            return Err(governed(GovernedInputError::AdmissionUnresolved {
                detail: "the native admission custody was already handed off".to_string(),
            }));
        };
        let turn = match door {
            GovernedDoor::External => {
                self.handle
                    .start_governed_input_with_custody(&mid, input, custody)
                    .await
            }
            GovernedDoor::TrustedHostInternal => {
                self.handle
                    .start_internal_governed_input_with_custody(&mid, input, custody)
                    .await
            }
        }
        .map_err(|error| governed(GovernedInputError::Mob(Box::new(error))))?;
        let admission = turn.admission().map(|admission| GovernedAdmission {
            session_id: admission.session_id,
            input_id: admission.input_id,
            deduplicated: admission.deduplicated,
        });
        // Carried, not propagated: the admitted turn is never abandoned on a
        // resolution failure.
        let session_result = self
            .resolve_runtime_session_id(
                runtime_id,
                &mid,
                "member has no bridge session after governed deliver",
                &deadline,
            )
            .await;
        Ok(GovernedDelivery {
            receipt: BridgeTurnReceipt::with_output(
                session_result,
                project_member_turn_output(turn, spec),
            ),
            admission,
        })
    }

    /// One delivery attempt's actor deadline, carrying the probe verdict when
    /// a probe is wired so an open stall fails the attempt fast.
    fn admission_deadline(&self) -> ActorAdmissionDeadline {
        let health = self
            .actor_loop_health
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        ActorAdmissionDeadline::with_health(self.actor_admission_budget, health)
    }

    /// The installed probe verdict, for operator reads.
    pub fn actor_loop_health(&self) -> Option<Arc<ActorLoopHealth>> {
        self.actor_loop_health
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Shared admission body for both the ingress-only and completion-bearing
    /// deliveries. The [`BridgeSubmitMode`] selects only which meerkat submit
    /// verb is used; every other step - budget, stale-state repair and retry,
    /// session resolution - is common by construction so the two lanes cannot
    /// drift.
    async fn deliver_admitted_inner(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
        mode: BridgeSubmitMode,
        host_human_session: Option<&meerkat_core::SessionId>,
        output: ReceiptOutput,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        // Validated before anything is submitted, so an invalid projection can
        // never cost an admitted turn.
        let output_spec = match output {
            ReceiptOutput::Terminal => None,
            ReceiptOutput::TurnOutput => Some(
                meerkat_mob::BoundedResultSpec::new(
                    "turn_output",
                    super::types::TURN_OUTPUT_MAX_BYTES,
                )
                .map_err(|error| BridgeAdmissionError::Mob(error.to_string()))?,
            ),
        };
        let content = &delivery.content;
        let handling_mode = delivery.handling_mode;
        let system_prompt = delivery.system_prompt.as_deref();
        let injected_context = delivery.injected_context.as_slice();
        let interaction_id = delivery.interaction_id.as_deref();
        let delivery_identity = delivery.delivery_identity.as_ref();
        let skill_references = delivery.skill_references.as_slice();
        if !skill_references.is_empty() && host_human_session.is_none() {
            return Err(BridgeAdmissionError::InvalidInput(
                "selected skills are carried only on the local host-human lane".to_string(),
            ));
        }
        let mid = self.member_id_for_runtime_id(runtime_id).await?;
        // One admission budget for the whole attempt, shared by every actor
        // round trip below: the serialized hops must not each cost a budget.
        let mut deadline = self.admission_deadline();
        // Best-effort repair material: a faulted or timed-out lookup degrades
        // to "no pre-delivery entry" (the delivery itself will surface the
        // fault; a blocked actor hits the same deadline again below).
        let member_entry_before_delivery = deadline
            .bound(
                "deliver.get_member.pre_delivery",
                &mid,
                self.handle.get_member(&mid),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .map(|entry| (entry.role, entry.labels));
        if content_input_has_images(content) {
            let member_entry = deadline
                .bound(
                    "deliver.get_member.image_capability",
                    &mid,
                    self.handle.get_member(&mid),
                )
                .await?
                .map_err(|err| BridgeError::Mob(err.to_string()))?
                .ok_or_else(|| {
                    BridgeError::Mob("member not found while checking image capability".to_string())
                })?;
            let caps = deadline
                .bound(
                    "deliver.model_capabilities",
                    &mid,
                    model_capabilities_for_member_with_config(
                        &self.handle,
                        self.session_service.as_ref(),
                        self.member_model_config.as_deref(),
                        &member_entry.agent_identity,
                    ),
                )
                .await?;
            if !caps.image_input {
                return Err(BridgeAdmissionError::InvalidInput(
                    "target member model cannot accept image input".to_string(),
                ));
            }
        }

        // Bound BY the match, not pre-initialised: every arm either yields the
        // handle or returns, so there is no state in which a turn was admitted
        // and `pending_turn` does not hold its handle.
        let pending_turn: Option<meerkat_mob::WorkTurnHandle> = match submit_internal_bridge_work(
            &self.handle,
            &mid,
            InternalBridgeWork {
                content,
                system_prompt,
                injected_context,
                interaction_id,
                delivery_identity,
                host_human_session,
                skill_references,
            },
            handling_mode,
            &deadline,
            mode,
        )
        .await
        {
            Ok(turn) => turn,
            // A blocked actor is not stale runtime state: keep the typed
            // timeout instead of routing it into member repair (which cannot
            // unblock an actor) or laundering it into `Mob(String)` below.
            // The same holds for the probe's open-stall / terminated refusals.
            Err(
                err @ (BridgeError::ActorAdmissionTimeout { .. }
                | BridgeError::ActorLoopStalled { .. }
                | BridgeError::ActorTerminated { .. }),
            ) => return Err(err.into()),
            // A durability-degraded member needs a registration-authorized
            // cold reload, which only the identity runtime can run (it owns
            // the lifecycle lock and the continuity record). Hand it up typed:
            // never repair (destructive, cannot clear the degradation), never
            // `Mob(String)`.
            Err(err @ BridgeError::ReloadRequired { .. }) => return Err(err.into()),
            // Per-member backpressure: the member is healthy, its lane is full.
            // Retryable by the caller; never repair, never reload.
            Err(err @ BridgeError::AdmissionBacklogFull { .. }) => return Err(err.into()),
            // meerkat refused the carriers for the member's actual runtime
            // mode before admission. Nothing is stale and nothing was
            // delivered: hand it up typed so a tracked delivery can fall back
            // to the ingress lane; never repair, never `Mob(String)`.
            Err(err @ BridgeError::UnsupportedForMode { .. }) => return Err(err.into()),
            // A closed mob, one whose Shutdown is still draining, or a member
            // closed while its identity converges refused the delivery before
            // admission. Nothing is stale and nothing was delivered: hand it
            // up typed; never repair, never `Mob(String)`.
            Err(
                err @ (BridgeError::MobAdmissionClosed { .. }
                | BridgeError::MobShutdownInProgress { .. }
                | BridgeError::IdentityConvergenceAdmissionClosed { .. }),
            ) => return Err(err.into()),
            Err(err) if host_human_session.is_none() && is_repairable_bridge_delivery(&err) => {
                tracing::warn!(
                    runtime_id = %runtime_id,
                    error = %err,
                    "identity bridge delivery found stale runtime state; repairing member before retry"
                );
                Box::pin(self.repair_member_for_delivery(
                    runtime_id,
                    &mid,
                    member_entry_before_delivery,
                ))
                .await?;
                // Repair is a distinct multi-step recovery with its own
                // (pre-existing, unbounded) cost; the retry is a new admission
                // attempt and gets a fresh budget.
                deadline = self.admission_deadline();
                // The retry's handle is kept exactly like the first attempt's:
                // dropping it here would abandon an admitted, running turn on
                // the repair path specifically.
                submit_internal_bridge_work(
                    &self.handle,
                    &mid,
                    InternalBridgeWork {
                        content,
                        system_prompt,
                        injected_context,
                        interaction_id,
                        delivery_identity,
                        host_human_session,
                        skill_references,
                    },
                    handling_mode,
                    &deadline,
                    mode,
                )
                .await?
            }
            Err(err @ BridgeError::HostHumanInput(_)) => return Err(err.into()),
            Err(err) => return Err(BridgeAdmissionError::Mob(err.to_string())),
        };

        // Resolution runs under the admission deadline, and its outcome is
        // CARRIED, not propagated. The handle is moved into the receipt without
        // the resolution result ever being inspected here, so there is no path
        // on which a resolution failure returns early and abandons a turn that
        // is already running. That invariant used to depend on statement order;
        // now it is structural.
        let session_result = self
            .resolve_runtime_session_id(
                runtime_id,
                &mid,
                "member has no bridge session after deliver",
                &deadline,
            )
            .await;

        match (pending_turn, output_spec) {
            // The LLM turn and its own output. Awaited by the CALLER, after it
            // has released whatever lock it holds.
            (Some(turn), Some(spec)) => Ok(BridgeTurnReceipt::with_output(
                session_result,
                project_turn_output(turn, spec),
            )),
            (pending_turn, _) => Ok(BridgeTurnReceipt::new(session_result, async move {
                match pending_turn {
                    // The LLM turn. Awaited by the CALLER, after it has
                    // released whatever lock it holds - never here, never
                    // under a lock.
                    Some(turn) => turn.wait().await.map(|_| ()).map_err(|err| err.to_string()),
                    // Admission-only: nothing to wait for.
                    None => Ok::<(), String>(()),
                }
            })),
        }
    }
}

/// An exported bridge is itself a bridge: handing it to a caller-built
/// identity runtime carries the mob's public owner with it (D67), so the
/// caller never receives the ownerless inner bridge. Every trait method is
/// forwarded, including the governed delivery's custody guard, which passes
/// through unchanged (a default body here would drop it or silently diverge
/// from the inner bridge); `public_session_bridge_forwards_every_method`
/// enforces it.
#[async_trait]
impl SessionBridge for super::PublicSessionBridge {
    async fn raw_member_alias_exists(&self, alias: &str) -> Result<bool, BridgeError> {
        self.bridge.raw_member_alias_exists(alias).await
    }

    fn observe_actor_loop_health(&self, health: Arc<ActorLoopHealth>) {
        self.bridge.observe_actor_loop_health(health);
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        self.bridge.reload_member_registration(runtime_id).await
    }

    async fn release_member_run_starts_for_published_tools(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<(), BridgeError> {
        self.bridge
            .release_member_run_starts_for_published_tools(member_id)
            .await
    }

    async fn member_durability(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<super::types::MemberDurability> {
        self.bridge.member_durability(session_id).await
    }

    async fn member_registration(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_runtime::RuntimeSessionRegistrationWitness> {
        self.bridge.member_registration(session_id).await
    }

    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.bridge.create_session(identity, spec, draft).await
    }

    async fn create_session_held(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        pass: &super::held_bootstrap::HeldCreatePass,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.bridge
            .create_session_held(identity, spec, draft, pass)
            .await
    }

    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.bridge
            .reset_member_to_successor(identity, spec, draft)
            .await
    }

    async fn capture_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<QueuedMemberInputs, BridgeError> {
        self.bridge
            .capture_queued_member_inputs(identity, session_id)
            .await
    }

    async fn readmit_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        inputs: QueuedMemberInputs,
    ) {
        self.bridge
            .readmit_queued_member_inputs(identity, session_id, inputs)
            .await;
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        self.bridge
            .resume_session(identity, spec, draft, session_id, resume_from_role)
            .await
    }

    fn declared_role_migration(
        &self,
        identity: &AgentIdentity,
    ) -> Option<meerkat_mob::ProfileName> {
        self.bridge.declared_role_migration(identity)
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.bridge.live_member(identity).await
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.bridge.live_member_scope(identity).await
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.bridge.ensure_member_live(identity).await
    }

    async fn identity_convergence_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        meerkat_mob::identity::IdentityStoredObservation<
            meerkat_mob::identity::IdentityConvergenceStatus,
        >,
        BridgeError,
    > {
        self.bridge.identity_convergence_status(identity).await
    }

    async fn identity_intent_present(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<bool>, BridgeError> {
        self.bridge.identity_intent_present(identity).await
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.bridge.member_binding_history().await
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.bridge.last_member_binding(identity).await
    }

    async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        self.bridge.identity_management(identity).await
    }

    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        self.bridge.durable_identity_members().await
    }

    async fn identity_labelled_members(
        &self,
    ) -> Result<Vec<meerkat_mob::AgentIdentity>, BridgeError> {
        self.bridge.identity_labelled_members().await
    }

    async fn broken_members(
        &self,
    ) -> Result<BTreeMap<meerkat_mob::AgentIdentity, String>, BridgeError> {
        self.bridge.broken_members().await
    }

    async fn durable_session_held(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        self.bridge.durable_session_held(session_id).await
    }

    async fn session_outlives_member(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        self.bridge.session_outlives_member(session_id).await
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge.deliver_admitted(runtime_id, delivery).await
    }

    async fn deliver_host_human_input(
        &self,
        runtime_id: &AgentRuntimeId,
        expected_session: &meerkat_core::SessionId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::SessionId, BridgeError> {
        self.bridge
            .deliver_host_human_input(runtime_id, expected_session, delivery)
            .await
    }

    async fn capture_member_delivery_scope(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<meerkat_mob::MemberDeliveryScope, ScopedDeliveryError> {
        self.bridge.capture_member_delivery_scope(runtime_id).await
    }

    async fn submit_at_scope(
        &self,
        runtime_id: &AgentRuntimeId,
        scope: &meerkat_mob::MemberDeliveryScope,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        delivery_identity: &meerkat_mob::MobDeliveryIdentity,
    ) -> Result<meerkat_mob::WorkDeliveryReceipt, ScopedDeliveryError> {
        self.bridge
            .submit_at_scope(runtime_id, scope, content, handling_mode, delivery_identity)
            .await
    }

    async fn recover_at_scope(
        &self,
        scope: &meerkat_mob::MemberDeliveryScope,
        delivery_identity: &meerkat_mob::MobDeliveryIdentity,
        deadline: std::time::Instant,
    ) -> Result<meerkat_mob::ScopedWorkRecovery, ScopedDeliveryError> {
        self.bridge
            .recover_at_scope(scope, delivery_identity, deadline)
            .await
    }

    async fn retire_at_scope(
        &self,
        runtime_id: &AgentRuntimeId,
        scope: &meerkat_mob::MemberDeliveryScope,
    ) -> Result<(), ScopedRetireError> {
        self.bridge.retire_at_scope(runtime_id, scope).await
    }

    async fn deliver(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge.deliver(runtime_id, content).await
    }

    async fn deliver_with_mode(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge
            .deliver_with_mode(runtime_id, content, handling_mode)
            .await
    }

    async fn deliver_with_mode_and_context(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge
            .deliver_with_mode_and_context(
                runtime_id,
                content,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
    }

    async fn deliver_awaiting_commit_with_mode_context_and_system_prompt(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge
            .deliver_awaiting_commit_with_mode_context_and_system_prompt(
                runtime_id,
                content,
                system_prompt,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
    }

    async fn begin_awaiting_commit(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.bridge
            .begin_awaiting_commit(
                runtime_id,
                content,
                system_prompt,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
    }

    fn tracks_turn_output(&self) -> bool {
        self.bridge.tracks_turn_output()
    }

    async fn begin_delivery_with_output(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.bridge
            .begin_delivery_with_output(runtime_id, delivery)
            .await
    }

    async fn begin_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.bridge
            .begin_governed_delivery(runtime_id, session_id, input, custody)
            .await
    }

    async fn begin_internal_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.bridge
            .begin_internal_governed_delivery(runtime_id, session_id, input, custody)
            .await
    }

    async fn governed_controller(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
        self.bridge
            .governed_controller(runtime_id, session_id)
            .await
    }

    async fn attach_live_mcp_server(
        &self,
        session_id: &meerkat_core::types::SessionId,
        attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    > {
        self.bridge
            .attach_live_mcp_server(session_id, attachment)
            .await
    }

    async fn deliver_with_mode_context_and_system_prompt(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.bridge
            .deliver_with_mode_context_and_system_prompt(
                runtime_id,
                content,
                system_prompt,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
    }

    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        self.bridge.checkpoint_session(runtime_id, session_id).await
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.bridge.retire_member(runtime_id).await
    }

    async fn retire_reset_superseded_member(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.bridge
            .retire_reset_superseded_member(runtime_id, session_id)
            .await
    }

    async fn wire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.bridge.wire_peer(a, b).await
    }

    async fn wire_peers_batch(
        &self,
        edges: &[(AgentRuntimeId, AgentRuntimeId)],
    ) -> Result<(), BridgeError> {
        self.bridge.wire_peers_batch(edges).await
    }

    async fn current_member_wires(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.bridge.current_member_wires().await
    }

    async fn current_member_wires_any_half(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.bridge.current_member_wires_any_half().await
    }

    async fn unwire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.bridge.unwire_peer(a, b).await
    }

    async fn inspect_member(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        self.bridge.inspect_member(runtime_id).await
    }

    async fn register_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
        identity: &AgentIdentity,
        generation: ContinuityGeneration,
        checkpoint_version: CheckpointVersion,
        fencing_token: FencingToken,
    ) -> Result<CheckpointVersion, BridgeError> {
        self.bridge
            .register_session_runtime_state(
                session_id,
                identity,
                generation,
                checkpoint_version,
                fencing_token,
            )
            .await
    }

    async fn suspend_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.bridge.suspend_session_runtime_state(session_id).await
    }

    async fn unregister_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.bridge
            .unregister_session_runtime_state(session_id)
            .await
    }

    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        self.bridge.recover_committed_boundary(session_id).await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    fn probe_input_header() -> meerkat_runtime::InputHeader {
        meerkat_runtime::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::External {
                source_name: "carry-classification-probe".to_string(),
            },
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        }
    }

    /// What a repair may carry across a member disposal, asserted at the owner.
    ///
    /// The integration-level version of this could only observe prompt TEXT,
    /// and the recording double there flattens the whole transcript - so a flow
    /// step legitimately sitting in the successor's history was
    /// indistinguishable from one re-admitted as a new input. That made the
    /// assertion unable to fail for the right reason. This asserts the decision
    /// itself, where it is made.
    ///
    /// The property that matters: user-originated work crosses the repair, and a
    /// flow-owned occurrence NEVER does, because its correlation belongs to the
    /// flow engine which observes its own step's terminal and owns the retry.
    #[test]
    fn repair_carries_user_input_and_never_a_flow_owned_occurrence() {
        // Carried: the three user-originated classes. Asserted individually,
        // not as a group - they share one match arm today, and a test that only
        // covered Prompt would not notice Peer or ExternalEvent being moved out
        // of it.
        for (label, input) in [
            (
                "prompt",
                meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new("p", None)),
            ),
            (
                "peer",
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: probe_input_header(),
                    directed_interaction_id: None,
                    convention: None,
                    content: meerkat_core::ContentInput::Text("peer".to_string()),
                    payload: None,
                    handling_mode: None,
                    sender_taint: None,
                    objective_id: None,
                    system_prompts: Vec::new(),
                    injected_context: Vec::new(),
                    in_reply_to: None,
                }),
            ),
            (
                "external-event",
                meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
                    header: probe_input_header(),
                    event_type: "probe".to_string(),
                    payload: serde_json::Value::Null,
                    blocks: None,
                    handling_mode: meerkat_core::types::HandlingMode::default(),
                    render_metadata: None,
                    objective_id: None,
                }),
            ),
        ] {
            match super::classify_pending_input_for_carry(Some(input)) {
                super::PendingInputCarry::Carry(_) => {}
                super::PendingInputCarry::Uncarryable { class, reason } => panic!(
                    "{label} is user-originated work and must cross the repair, but it was \
                     classified {class}: {reason}"
                ),
            }
        }

        // NOT carried, and this is the invariant the integration test could not
        // observe.
        let flow_step = meerkat_runtime::Input::FlowStep(meerkat_runtime::FlowStepInput {
            header: probe_input_header(),
            step_id: "probe-step".to_string(),
            content: meerkat_core::ContentInput::Text("flow".to_string()),
            directed_interaction_id: None,
            turn_metadata: None,
        });
        match super::classify_pending_input_for_carry(Some(flow_step)) {
            super::PendingInputCarry::Uncarryable { class, reason } => {
                assert_eq!(
                    class, "flow-step",
                    "a flow step must be named as such, so the disposal it precedes is reviewable"
                );
                assert!(
                    reason.contains("owned by the flow engine"),
                    "the reason must say WHO owns the correlation, not merely that it cannot be \
                     carried: {reason}"
                );
            }
            super::PendingInputCarry::Carry(_) => panic!(
                "a flow-owned occurrence must NEVER cross a repair: its correlation belongs to \
                 the flow engine, which owns the retry"
            ),
        }

        // A pending input the runtime kept no payload for is named too, rather
        // than vanishing into the disposal.
        match super::classify_pending_input_for_carry(None) {
            super::PendingInputCarry::Uncarryable { class, .. } => {
                assert_eq!(class, "payload-unavailable");
            }
            super::PendingInputCarry::Carry(_) => {
                panic!("a pending input with no retained payload cannot be carried")
            }
        }

        // NOTE: Continuation and Operation are deliberately not constructed
        // here - their payloads need OperationId/OpEvent/ContinuationKind
        // fixtures that would dominate this test. Their lane
        // ("runtime-internal") is therefore unasserted, and said so rather than
        // implied by the cases above.
    }

    /// The ordering property, proven by direct poll rather than by timing.
    ///
    /// Session resolution must COMPLETE before the completion future is polled
    /// even once. A wall-clock version of this test was written first, and a
    /// negative mutation proved it inert - it passed with the awaits swapped.
    /// This one cannot: swapping them makes the completion future observe an
    /// empty trace on its first poll and panic there.
    #[test]
    fn session_resolution_completes_before_the_completion_future_is_polled() {
        use std::sync::Mutex;
        use std::task::{Context, Poll, Waker};

        let trace: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));

        /// Records its first poll, asserts resolution already happened, then
        /// parks until released.
        struct GatedCompletion {
            trace: Arc<Mutex<Vec<&'static str>>>,
            released: Arc<std::sync::atomic::AtomicBool>,
            polled: bool,
        }
        impl std::future::Future for GatedCompletion {
            type Output = Result<(), BridgeError>;
            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                if !self.polled {
                    assert_eq!(
                        self.trace.lock().expect("trace mutex poisoned").as_slice(),
                        ["SessionResolved"],
                        "the completion future was polled before session resolution finished - \
                         the admission budget would be spent on the turn"
                    );
                    self.trace
                        .lock()
                        .expect("trace mutex poisoned")
                        .push("CompletionPolled");
                    self.polled = true;
                }
                if self.released.load(std::sync::atomic::Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }
        }

        let session = meerkat_core::types::SessionId::new();
        let expected = session.clone();
        let resolve_trace = Arc::clone(&trace);
        let resolve = async move {
            resolve_trace
                .lock()
                .expect("trace mutex poisoned")
                .push("SessionResolved");
            Ok::<_, BridgeError>(session)
        };
        let completion = GatedCompletion {
            trace: Arc::clone(&trace),
            released: Arc::clone(&released),
            polled: false,
        };

        // Resolution happens in `begin`, BEFORE the receipt exists, so the
        // ordering is now structural: a receipt cannot be constructed without a
        // resolution outcome in hand. What still needs proving is that `wait`
        // parks on the completion rather than returning on the carried result.
        let resolved = futures::executor::block_on(resolve).expect("resolution");
        let combined =
            super::BridgeTurnReceipt::new(Ok::<_, BridgeError>(resolved), completion).wait();
        let mut combined = Box::pin(combined);
        let mut cx = Context::from_waker(Waker::noop());

        assert!(
            combined.as_mut().poll(&mut cx).is_pending(),
            "must park on the completion, not return at resolution"
        );
        assert_eq!(
            trace.lock().expect("trace mutex poisoned").as_slice(),
            ["SessionResolved", "CompletionPolled"],
            "exact ordering: resolve fully, THEN poll the completion"
        );

        released.store(true, std::sync::atomic::Ordering::SeqCst);
        match combined.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(got)) => assert_eq!(got, expected, "must return the resolved session"),
            other => panic!("expected the resolved session once released, got {other:?}"),
        }
    }

    /// P1: a resolution failure AFTER admission must not drop the turn.
    ///
    /// Proven by direct poll, not by timing. The resolution error is
    /// deliberately admission-shaped AND repairable by
    /// `is_repairable_bridge_delivery_error`, which is the dangerous case: an
    /// early return here would abandon an admitted, running turn and hand the
    /// delivery path an error it would repair and resubmit, running the
    /// member's turn a second time.
    ///
    /// Two things are asserted that an early return cannot satisfy: the
    /// completion future is polled at all, and the combined future parks on it
    /// rather than returning at resolution.
    #[test]
    fn a_post_admission_resolution_failure_still_awaits_the_turn_and_is_not_repairable() {
        use std::sync::Mutex;
        use std::task::{Context, Poll, Waker};

        let polls: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));

        struct CountedCompletion {
            polls: Arc<Mutex<usize>>,
            released: Arc<std::sync::atomic::AtomicBool>,
        }
        impl std::future::Future for CountedCompletion {
            type Output = Result<(), BridgeError>;
            fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                *self.polls.lock().expect("poll counter mutex poisoned") += 1;
                if self.released.load(std::sync::atomic::Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }
        }

        // Admission-shaped AND accepted by the repair classifier.
        let resolve_detail = "missing bridge session snapshot for member rt-agent-alpha-0";
        assert!(
            is_repairable_bridge_delivery_error(resolve_detail),
            "the test is only meaningful if the resolution error is one the \
             delivery path WOULD repair and resubmit"
        );
        let completion = CountedCompletion {
            polls: Arc::clone(&polls),
            released: Arc::clone(&released),
        };

        let combined = super::BridgeTurnReceipt::new(
            Err(BridgeError::Mob(resolve_detail.to_string())),
            completion,
        )
        .wait();
        let mut combined = Box::pin(combined);
        let mut cx = Context::from_waker(Waker::noop());

        assert!(
            combined.as_mut().poll(&mut cx).is_pending(),
            "resolution failed, but the turn is still running: the combined \
             future must park on the completion, not return and drop it"
        );
        assert_eq!(
            *polls.lock().expect("poll counter mutex poisoned"),
            1,
            "the completion future was never polled - the admitted turn was dropped"
        );

        released.store(true, std::sync::atomic::Ordering::SeqCst);
        match combined.as_mut().poll(&mut cx) {
            Poll::Ready(Err(BridgeTurnError::PostAdmissionResolutionFailed(detail))) => {
                assert!(
                    detail.contains("missing bridge session snapshot"),
                    "the resolution detail must survive verbatim: {detail}"
                );
                // The protection is the VARIANT, not the wording: this error is
                // produced after admission and is never handed to the delivery
                // path's substring classifier. Assert the variant is not one of
                // the admission-shaped ones it acts on.
                let rendered = BridgeTurnError::PostAdmissionResolutionFailed(detail).to_string();
                assert!(
                    rendered.contains("do not retry"),
                    "the rendering must tell an operator this is terminal: {rendered}"
                );
            }
            other => panic!(
                "a turn that SUCCEEDED after a failed resolution must surface as \
                 PostAdmissionResolutionFailed - anything admission-shaped could be \
                 repaired and resubmitted, running the turn twice. Got: {other:?}"
            ),
        }
    }

    /// When BOTH fail, the turn's failure is the outcome and leads, and the
    /// resolution detail is carried rather than silently dropped.
    #[tokio::test]
    async fn both_failing_leads_with_the_turn_and_still_carries_the_resolution_detail() {
        let failed = super::BridgeTurnReceipt::new(
            Err(BridgeError::Mob("resolution detail marker".to_string())),
            async { Err::<(), _>(BridgeError::Mob("turn failure marker".to_string())) },
        )
        .wait()
        .await;
        match failed {
            Err(BridgeTurnError::CompletionFailed(detail)) => {
                assert!(
                    detail.contains("turn failure marker"),
                    "the turn's own failure must lead: {detail}"
                );
                assert!(
                    detail.contains("resolution detail marker"),
                    "the resolution failure must not be silently dropped: {detail}"
                );
            }
            other => panic!(
                "a turn that RAN and failed is a completion failure regardless of \
                 what resolution did. Got: {other:?}"
            ),
        }
    }

    /// A completion failure must be typed `CompletionFailed`, never anything
    /// admission-shaped that the repair-and-retry classifier could act on.
    #[tokio::test]
    async fn a_completion_failure_is_typed_and_never_admission_shaped() {
        let session = meerkat_core::types::SessionId::new();
        let failed = super::BridgeTurnReceipt::new(Ok::<_, BridgeError>(session), async {
            Err::<(), _>(BridgeError::Mob(
                "missing bridge session snapshot - completion sentinel".to_string(),
            ))
        })
        .wait()
        .await;
        match failed {
            Err(BridgeTurnError::CompletionFailed(detail)) => assert!(
                detail.contains("missing bridge session snapshot"),
                "the detail must survive so operators can see what failed: {detail}"
            ),
            other => panic!(
                "a completion failure whose text is ACCEPTED by \
                 is_repairable_bridge_delivery_error must still surface as CompletionFailed, \
                 or the delivery path could repair and resubmit a turn that already ran. \
                 Got: {other:?}"
            ),
        }
    }

    /// 8a threading pin: every optional carrier on the internal deliver
    /// surface reaches the WorkSpec unchanged - in particular the per-turn
    /// System message (meerkat 0.8.11 `WorkSpec::system_prompt`), threaded
    /// exactly like `injected_context`.
    #[test]
    fn internal_bridge_work_spec_threads_every_carrier() {
        let content = meerkat_core::ContentInput::Text("turn content".to_string());
        let injected = vec![meerkat_core::ContentInput::Text(
            "ambient recall".to_string(),
        )];
        let interaction = uuid::Uuid::new_v4();

        let spec = super::internal_bridge_work_spec(
            &content,
            Some("per-turn system message"),
            &injected,
            Some(&interaction.to_string()),
        );
        assert_eq!(
            spec.system_prompt.as_deref(),
            Some("per-turn system message")
        );
        assert_eq!(spec.injected_context, injected);
        assert_eq!(
            spec.interaction_id,
            Some(meerkat_core::interaction::InteractionId(interaction))
        );
        assert!(matches!(spec.origin, meerkat_mob::WorkOrigin::Internal));

        let bare = super::internal_bridge_work_spec(&content, None, &[], None);
        assert_eq!(bare.system_prompt, None, "absent carrier stays absent");
        assert!(bare.injected_context.is_empty());
        assert_eq!(bare.interaction_id, None);
    }

    use async_trait::async_trait;
    use meerkat_core::agent::AgentToolDispatcher;
    use meerkat_core::lifecycle::run_primitive::{OpenAiPromptCacheOptions, ReasoningEffort};
    use meerkat_core::model_profile::capabilities::{OpenAiPromptCacheMode, OpenAiPromptCacheTtl};
    use meerkat_core::types::ToolCallView;
    use meerkat_core::{ToolDef, error::ToolError, ops::ToolDispatchOutcome};
    use meerkat_mob::{MobRespawnError, MobRuntimeMode};

    use super::*;
    use crate::identity_first::{AgentAddressability, LocalExternalToolOverlay};

    struct EmptyDispatcher;

    struct TestCompactionCurator;

    #[async_trait]
    impl meerkat_core::CompactionCurator for TestCompactionCurator {
        async fn curate_summary(
            &self,
            _window: meerkat_core::CompactionWindow<'_>,
        ) -> Result<meerkat_core::CuratedCompactionSummary, meerkat_core::CompactionCuratorError>
        {
            meerkat_core::CuratedCompactionSummary::new("meta curated summary")
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for EmptyDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        async fn dispatch(
            &self,
            _call: ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::ExecutionFailed {
                message: "not implemented".to_string(),
            })
        }
    }

    /// Resume-divergence tripwire semantics: unmasked + declared + different
    /// flags the field; a mask or an absent declaration silences it.
    #[test]
    fn unmasked_resume_divergence_flags_only_unmasked_declared_differences() {
        let unmasked = meerkat_core::service::ResumeOverrideMask::default();
        let divergence = unmasked_resume_divergence(
            &unmasked,
            "claude-opus-4-8",
            Some(meerkat_core::Provider::Anthropic),
            "claude-sonnet-4-5",
            meerkat_core::Provider::OpenAI,
        );
        assert!(divergence.model, "unmasked differing model must be flagged");
        assert!(
            divergence.provider,
            "unmasked differing declared provider must be flagged"
        );

        let masked = meerkat_core::service::ResumeOverrideMask {
            model: true,
            provider: true,
            ..Default::default()
        };
        let divergence = unmasked_resume_divergence(
            &masked,
            "claude-opus-4-8",
            Some(meerkat_core::Provider::Anthropic),
            "claude-sonnet-4-5",
            meerkat_core::Provider::OpenAI,
        );
        assert!(
            !divergence.model && !divergence.provider,
            "a mask covering the field silences the tripwire (the profile wins anyway)"
        );

        let divergence = unmasked_resume_divergence(
            &unmasked,
            "claude-opus-4-8",
            None,
            "claude-opus-4-8",
            meerkat_core::Provider::OpenAI,
        );
        assert!(!divergence.model, "an identical model is not a divergence");
        assert!(
            !divergence.provider,
            "an undeclared provider states no intent to diverge from"
        );
    }

    fn durable_spec() -> DurableAgentSpec {
        DurableAgentSpec {
            identity: AgentIdentity::parse("agent:alpha").expect("identity"),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: Default::default(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: Some(meerkat_core::ContentInput::Text("hello".to_string())),
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        }
    }

    /// Regression: meerkat 0.7.1 made `member_status` peer connectivity a
    /// tri-state. Only the `Known` arm carries a live count; the
    /// not-applicable / probe-timed-out arms must defer to the machine-owned
    /// wiring degree (`wired_to.len()`) instead of projecting 0, or
    /// `peer_reachable_count` reads 0 for freshly role-wired members and
    /// topology verification breaks.
    #[test]
    fn peer_reachable_count_tri_state_defers_to_wiring_when_unresolved() {
        use meerkat_contracts::{WirePeerConnectivity, WirePeerConnectivitySnapshot};

        let known = WirePeerConnectivity::Known {
            snapshot: WirePeerConnectivitySnapshot {
                reachable_peer_count: 3,
                unknown_peer_count: 0,
                unreachable_peers: Vec::new(),
            },
        };
        assert_eq!(
            peer_reachable_count_from_connectivity(Some(&known)),
            Some(3),
            "a resolved probe owns the count"
        );
        let structurally_wired_but_unresolved = WirePeerConnectivity::Known {
            snapshot: WirePeerConnectivitySnapshot {
                reachable_peer_count: 0,
                unknown_peer_count: 3,
                unreachable_peers: Vec::new(),
            },
        };
        assert_eq!(
            peer_reachable_count_from_connectivity(Some(&structurally_wired_but_unresolved)),
            None,
            "a partially resolved probe must defer to the machine-owned wiring degree"
        );
        assert_eq!(
            peer_reachable_count_from_connectivity(Some(&WirePeerConnectivity::NotApplicable)),
            None,
            "not-applicable must defer to the wiring fallback"
        );
        assert_eq!(
            peer_reachable_count_from_connectivity(Some(&WirePeerConnectivity::ProbeTimedOut)),
            None,
            "probe timeout must defer to the wiring fallback"
        );
        assert_eq!(peer_reachable_count_from_connectivity(None), None);
    }

    #[test]
    fn build_spawn_spec_maps_identity_first_overrides() {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("team".to_string(), "ops".to_string());
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: Some("gpt-test".to_string()),
            system_prompt: Some("system override".to_string()),
            additional_instructions: vec!["stay focused".to_string()],
            labels,
            app_context: Some(serde_json::json!({"ticket": 7})),
            external_tools: Vec::new(),
            local_external_tools: LocalExternalToolOverlay::new(Arc::new(EmptyDispatcher)),
            provider_params: None,
        };

        let base_profile: meerkat_mob::Profile =
            serde_json::from_value(serde_json::json!({"model": "base-model"}))
                .expect("minimal profile");
        let spawn =
            build_spawn_spec(&durable_spec(), &draft, Some(&base_profile)).expect("spawn spec");

        // meerkat 0.7.29 (ask 29): an unpinned base profile takes the
        // field-scoped seam — the model pin follows definition drift instead
        // of freezing the whole profile (Bug G′).
        assert_eq!(spawn.model_override.as_deref(), Some("gpt-test"));
        assert!(
            spawn.override_profile.is_none(),
            "unpinned profiles must not freeze the whole profile for a model-only override"
        );
        assert_eq!(
            spawn.system_prompt_override,
            Some(SpawnSystemPromptOverride::Replace(
                "system override".to_string()
            ))
        );
        assert!(spawn.external_tools.is_some());
        assert_eq!(spawn.runtime_mode, Some(MobRuntimeMode::TurnDriven));
        assert_eq!(
            spawn.initial_message,
            Some(meerkat_core::ContentInput::Text("hello".to_string()))
        );
        assert_eq!(
            spawn
                .labels
                .as_ref()
                .and_then(|labels| labels.get("team"))
                .map(String::as_str),
            Some("ops")
        );
        assert_eq!(
            spawn
                .labels
                .as_ref()
                .and_then(|labels| labels.get("agent_identity"))
                .map(String::as_str),
            Some("agent:alpha")
        );
        assert_eq!(
            spawn
                .labels
                .as_ref()
                .and_then(|labels| labels.get("profile_name"))
                .map(String::as_str),
            Some("worker"),
            "identity-first spawn labels must carry the adopted profile so the \
             SDK build callback sees the roster profile, not a checkpoint default"
        );
        assert_eq!(
            spawn.role_name.as_str(),
            "worker",
            "SpawnMemberSpec role remains the authoritative mob profile"
        );
    }

    #[test]
    fn build_spawn_spec_preserves_exact_remote_placement() {
        let mut spec = durable_spec();
        spec.placement = Some(meerkat_contracts::WireHostRef(
            "12D3KooWExactRemoteHost".to_string(),
        ));
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: LocalExternalToolOverlay::default(),
            provider_params: None,
        };

        let spawn = build_spawn_spec(&spec, &draft, None).expect("spawn spec");
        assert_eq!(
            spawn
                .placement
                .as_ref()
                .map(meerkat_mob::machines::mob_machine::HostId::as_str),
            Some("12D3KooWExactRemoteHost"),
            "remote placement must reach Meerkat unchanged and never become local None"
        );
    }

    #[test]
    fn fresh_and_resume_spawn_specs_carry_the_exact_host_curator_arc() {
        let session_id = meerkat_core::types::SessionId::new();
        let curator: Arc<dyn meerkat_core::CompactionCurator> = Arc::new(TestCompactionCurator);
        let draft = AgentBuildDraft {
            compaction_curator: crate::identity_first::CompactionCuratorOverlay::new(Arc::clone(
                &curator,
            )),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };

        let fresh = build_spawn_spec(&durable_spec(), &draft, None).expect("fresh spawn spec");
        assert!(
            fresh
                .compaction_curator_override
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, &curator))
        );

        let resumed = build_resume_spawn_spec(&durable_spec(), &draft, None, &session_id, None)
            .expect("resume spawn spec");
        assert!(
            resumed
                .compaction_curator_override
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, &curator))
        );
    }

    /// Every identity-first spawn spec carries the durable-identity marker
    /// keyed by the identity: the fresh spec (create, held create and the
    /// reset successor build through it) and both resume branches (session
    /// and external binding). Core lists a member as a durable identity only
    /// when its spawn carried the marker.
    #[test]
    fn identity_first_spawn_specs_carry_the_durable_identity_marker() {
        let session_id = meerkat_core::types::SessionId::new();
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };
        let spec = durable_spec();
        let marker = SpawnContinuityIntent::DurableIdentity {
            continuity_key: spec.identity.as_str().to_string(),
        };

        let fresh = build_spawn_spec(&spec, &draft, None).expect("fresh spawn spec");
        assert_eq!(fresh.continuity_intent, marker, "fresh spawn");

        let resumed = build_resume_spawn_spec(&spec, &draft, None, &session_id, None)
            .expect("resume spawn spec");
        assert_eq!(resumed.continuity_intent, marker, "session resume");

        let mut external = spec;
        external.backend = Some(meerkat_mob::MobBackendKind::External);
        assert!(spec_uses_external_binding(&external));
        let external_resume = build_resume_spawn_spec(&external, &draft, None, &session_id, None)
            .expect("external-binding resume spawn spec");
        assert_eq!(
            external_resume.continuity_intent, marker,
            "external-binding resume"
        );
    }

    /// Only the identity-first builder marks a durable identity. The spawn
    /// shapes Meta builds for other members (classic and RPC spawns, helpers,
    /// delegates: `SpawnMemberSpec::new` and `host_root` straight) keep
    /// Core's `Ephemeral` intent, so a placement host's materialize preflight
    /// asks them for durable sessions only on a resume launch.
    #[test]
    fn only_the_identity_first_builder_marks_a_durable_identity() {
        let member = MobAgentIdentity::from("classic-member");
        assert_eq!(
            SpawnMemberSpec::new("worker", member.clone()).continuity_intent,
            SpawnContinuityIntent::Ephemeral
        );
        assert_eq!(
            SpawnMemberSpec::host_root("worker", member).continuity_intent,
            SpawnContinuityIntent::Ephemeral
        );
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };
        let identity_first =
            build_spawn_spec(&durable_spec(), &draft, None).expect("identity-first spawn spec");
        assert!(matches!(
            identity_first.continuity_intent,
            SpawnContinuityIntent::DurableIdentity { .. }
        ));
    }

    /// A placement host's refusal for a realm backend that is not persistent
    /// surfaces typed from the bridge rejection cause, also through meerkat's
    /// shared failure wrapper and on the respawn path; any other bridge
    /// rejection keeps the untyped mob text.
    #[test]
    fn a_non_persistent_realm_refusal_surfaces_typed() {
        use meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause;
        let refused = || meerkat_mob::MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::RealmBackendUnavailable,
            reason: "materialize rejected: durable sessions required but the realm backend \
                     is not persistent"
                .to_string(),
        };
        assert!(matches!(
            spawn_bridge_error("create_session", &refused()),
            BridgeError::RealmBackendUnavailable { .. }
        ));
        assert!(matches!(
            spawn_bridge_error(
                "create_session",
                &meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(refused()))
            ),
            BridgeError::RealmBackendUnavailable { .. }
        ));
        assert!(matches!(
            respawn_bridge_error(
                "successor respawn",
                &meerkat_mob::MobRespawnError::Mob(refused())
            ),
            BridgeError::RealmBackendUnavailable { .. }
        ));
        let stale = meerkat_mob::MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::StaleFence,
            reason: "stale fence".to_string(),
        };
        assert!(matches!(
            spawn_bridge_error("create_session", &stale),
            BridgeError::Mob(_)
        ));
    }

    /// meerkat's refusal of a durable profile on a mob whose member sessions
    /// do not outlive the process surfaces typed, with the refused profile,
    /// on every door that spawns: create, through meerkat's shared failure
    /// wrappers, the respawn of a reset and the resume of a wake. Any other
    /// failure keeps its own mapping.
    #[test]
    fn a_durable_profile_refusal_surfaces_typed() -> Result<(), Box<dyn std::error::Error>> {
        let refused = || meerkat_mob::MobError::NonDurableService {
            member_id: meerkat_mob::AgentIdentity::from("worker"),
            profile: meerkat_mob::ProfileName::from("durable-worker"),
        };
        let identity = AgentIdentity::parse("durable:worker")?;
        let session_id = meerkat_core::types::SessionId::new();
        let doors = [
            spawn_bridge_error("create_session", &refused()),
            spawn_bridge_error(
                "create_session",
                &meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(refused())),
            ),
            spawn_bridge_error(
                "create_session_held",
                &meerkat_mob::MobError::SharedRetirementFailure(Arc::new(refused())),
            ),
            respawn_bridge_error(
                "successor respawn",
                &meerkat_mob::MobRespawnError::Mob(refused()),
            ),
            resume_rejected(&identity, &session_id, &refused(), "resume"),
        ];
        for mapped in doors {
            let BridgeError::NonDurableService { profile, detail } = &mapped else {
                return Err(
                    format!("expected the typed durable-profile refusal, got {mapped:?}").into(),
                );
            };
            assert_eq!(profile.as_str(), "durable-worker");
            assert!(detail.contains("durable-worker"), "{detail}");
            assert!(
                mapped
                    .to_string()
                    .contains("member sessions do not outlive the process"),
                "{mapped}"
            );
        }
        assert!(matches!(
            non_durable_service_refusal(&meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(
                refused()
            ))),
            Some((meerkat_mob::MobError::NonDurableService { .. }, _))
        ));
        let other = meerkat_mob::MobError::Internal("unrelated".to_string());
        assert!(non_durable_service_refusal(&other).is_none());
        assert!(matches!(
            spawn_bridge_error("create_session", &other),
            BridgeError::Mob(_)
        ));
        Ok(())
    }

    /// The host's declaration is what reaches Meerkat, unchanged, and absence
    /// is what reaches it by default.
    ///
    /// That is the entire carrier contract: Meta must not lose the value and
    /// must not invent one. Meerkat owns the decision - it re-verifies the
    /// declared predecessor against durable state and refuses on mismatch.
    #[test]
    fn declared_predecessor_role_rides_the_resume_launch_mode() {
        let session_id = meerkat_core::types::SessionId::new();
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };

        let undeclared = build_resume_spawn_spec(&durable_spec(), &draft, None, &session_id, None)
            .expect("resume spawn spec");
        match &undeclared.launch_mode {
            MemberLaunchMode::Resume {
                resume_from_role, ..
            } => assert!(
                resume_from_role.is_none(),
                "an undeclared resume must carry no migration authority"
            ),
            _ => panic!("expected a Resume launch mode"),
        }

        let declared = build_resume_spawn_spec(
            &durable_spec(),
            &draft,
            None,
            &session_id,
            Some(meerkat_mob::ProfileName::from("domain")),
        )
        .expect("resume spawn spec");
        match &declared.launch_mode {
            MemberLaunchMode::Resume {
                resume_from_role, ..
            } => {
                let role = resume_from_role
                    .as_ref()
                    .expect("the declared predecessor role must survive the pass-through");
                assert_eq!(
                    *role, "domain",
                    "the declared predecessor role must reach Meerkat verbatim"
                );
            }
            _ => panic!("expected a Resume launch mode"),
        }
    }

    /// The Python SDK and this parser must agree on the wire key names.
    ///
    /// Both sides are pinned to ONE committed fixture, so renaming `from_role`
    /// on either side goes red here or in its Python twin, instead of both
    /// staying green while a host arms nothing.
    #[test]
    fn the_committed_wire_fixture_deserializes_into_declarations() {
        const FIXTURE: &str = include_str!("../../tests/fixtures/role_migrations_init_params.json");
        let params: serde_json::Value =
            serde_json::from_str(FIXTURE).expect("fixture is valid JSON");
        let declarations: Vec<RoleMigrationDeclaration> =
            serde_json::from_value(params["role_migrations"].clone())
                .expect("the fixture's role_migrations must deserialize");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].identity.as_str(), "domain:automation");
        assert_eq!(declarations[0].from_role, "domain");
    }

    /// A conflicting pair is refused; an identical repeat is not.
    ///
    /// Refusing the repeat would be the failure mode of treating every
    /// irregularity as fatal: it is unambiguous, so it costs nothing to accept.
    #[test]
    fn conflicting_declarations_are_reported_and_identical_repeats_are_not() {
        let identity = AgentIdentity::parse("agent:alice").expect("identity");
        let declare = |role: &str| RoleMigrationDeclaration {
            identity: identity.clone(),
            from_role: meerkat_mob::ProfileName::from(role),
        };

        let conflicting = [declare("domain"), declare("other")];
        let (found, first, second) =
            super::super::types::conflicting_role_migration_declaration(&conflicting)
                .expect("a conflicting pair must be reported");
        assert_eq!(found, &identity);
        assert_eq!(*first, "domain");
        assert_eq!(*second, "other");

        assert!(
            super::super::types::conflicting_role_migration_declaration(&[
                declare("domain"),
                declare("domain")
            ])
            .is_none(),
            "an identical repeat is unambiguous and must not fail the boot"
        );
    }

    /// Exact identity only, in both directions: a declaration arms the
    /// identity the host named and nothing adjacent to it.
    ///
    /// Prefix and case tolerance here would silently widen migration authority
    /// from one member to a family of them, which is the failure the typed
    /// refusal exists to prevent.
    #[test]
    fn a_declaration_arms_only_the_exact_identity() {
        let declared = AgentIdentity::parse("agent:alice").expect("identity");
        let map = role_migration_declaration_map([RoleMigrationDeclaration {
            identity: declared.clone(),
            from_role: meerkat_mob::ProfileName::from("domain"),
        }]);

        let armed = declared_role_migration_in(&map, &declared)
            .expect("the exact declared identity must be armed");
        assert_eq!(armed, "domain");

        for adjacent in [
            "agent:alice-2",
            "agent:alic",
            "agent:Alice",
            "alice",
            "agent:alice:child",
            "AGENT:ALICE",
        ] {
            let identity = AgentIdentity::parse(adjacent).expect("identity");
            assert!(
                declared_role_migration_in(&map, &identity).is_none(),
                "'{adjacent}' must carry no migration authority from a declaration for \
                 '{declared}'"
            );
        }
    }

    /// Meta does NOT clear a declaration once its migration has run, and
    /// this test exists to say so on purpose rather than by omission.
    ///
    /// The lookup is non-consuming: a second resume inside the same boot, for
    /// example a rematerialization, gets the same declaration handed to Meerkat
    /// again. That is safe only because Meerkat returns early when the stored
    /// role already equals the requested role, so the repeat is inert. Safety
    /// therefore rests on Meerkat's early return, NOT on Meta withholding the
    /// value, and whoever changes either side needs to know which half they are
    /// standing on.
    ///
    /// Measured in production, a downstream app activation-89: the same declaration was
    /// armed on two consecutive boots of one activation; the first performed the
    /// restamp and the second did nothing. A host-side gate cannot catch a wrong
    /// repeat, because after a successful migration the durable record agrees
    /// with the roster and such a gate reports "no migration needed" while
    /// passing an activation that still declares one.
    #[test]
    fn a_declaration_is_not_consumed_by_being_looked_up() {
        let identity = AgentIdentity::parse("domain:automation").expect("identity");
        let map = role_migration_declaration_map([RoleMigrationDeclaration {
            identity: identity.clone(),
            from_role: meerkat_mob::ProfileName::from("domain"),
        }]);

        for attempt in 1..=3 {
            let armed = declared_role_migration_in(&map, &identity)
                .unwrap_or_else(|| panic!("lookup {attempt} must still see the declaration"));
            assert_eq!(
                armed, "domain",
                "lookup {attempt} must return the same predecessor role"
            );
        }
    }

    /// The exact boot-payload shape the host sends, plus the failure that
    /// matters: a malformed identity refuses at ingress rather than arming a
    /// migration for something that is not an identity.
    #[test]
    fn role_migration_declaration_parses_from_the_boot_payload_shape() {
        let declaration: RoleMigrationDeclaration =
            serde_json::from_str(r#"{"identity": "domain:automation", "from_role": "domain"}"#)
                .expect("declaration parses");
        assert_eq!(declaration.identity.as_str(), "domain:automation");
        assert_eq!(declaration.from_role, "domain");

        serde_json::from_str::<RoleMigrationDeclaration>(
            r#"{"identity": "has a space", "from_role": "domain"}"#,
        )
        .expect_err("a malformed identity must refuse at ingress");
    }

    /// A base profile that pins a provider (or self-hosted binding) keeps the
    /// legacy whole-profile snapshot: upstream `model_override` does not
    /// re-infer the provider, and the pin belongs to the original model id.
    #[test]
    fn build_spawn_spec_keeps_profile_snapshot_for_pinned_provider() {
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: Some("gpt-test".to_string()),
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: LocalExternalToolOverlay::new(Arc::new(EmptyDispatcher)),
            provider_params: None,
        };
        let base_profile: meerkat_mob::Profile = serde_json::from_value(
            serde_json::json!({"model": "base-model", "provider": "openai"}),
        )
        .expect("pinned profile");

        let spawn =
            build_spawn_spec(&durable_spec(), &draft, Some(&base_profile)).expect("spawn spec");

        assert!(spawn.model_override.is_none());
        let profile = spawn
            .override_profile
            .as_ref()
            .expect("pinned profile keeps the snapshot path");
        assert_eq!(profile.model.as_str(), "gpt-test");
        assert!(
            profile.provider.is_none(),
            "a catalog-unknown pin carries no provider (config-entry resolution downstream)"
        );
    }

    /// A draft model pin on a pinned-provider base must carry the DRAFT
    /// model's catalog owner in the snapshot, applied with the model as a
    /// pair. Clearing it to None (the pre-incident shape) let resume fall back to
    /// the durable provider under the pinned model — the exact invalid
    /// (model, provider) pair the incident rejected typed.
    #[test]
    fn build_spawn_spec_derives_pair_provider_for_catalog_model_pin() {
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: Some("claude-opus-4-8".to_string()),
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: LocalExternalToolOverlay::new(Arc::new(EmptyDispatcher)),
            provider_params: None,
        };
        // Post-auto-mark shape: the definition profile carries its model's
        // owner and the pair mask.
        let base_profile: meerkat_mob::Profile = serde_json::from_value(serde_json::json!({
            "model": "gpt-5.5",
            "provider": "openai",
            "resume_overrides": ["model", "provider"],
        }))
        .expect("pinned profile");

        let spawn =
            build_spawn_spec(&durable_spec(), &draft, Some(&base_profile)).expect("spawn spec");

        let profile = spawn
            .override_profile
            .as_ref()
            .expect("pinned profile keeps the snapshot path");
        assert_eq!(profile.model.as_str(), "claude-opus-4-8");
        assert_eq!(
            profile.provider,
            Some(meerkat_core::Provider::Anthropic),
            "the pin's provider must be the DRAFT model's catalog owner, applied as a pair"
        );
        assert!(
            profile
                .resume_overrides
                .contains(&ResumeOverrideField::Model)
                && profile
                    .resume_overrides
                    .contains(&ResumeOverrideField::Provider),
            "the base profile's pair mask must ride the snapshot so both fields apply on resume"
        );
    }

    // -----------------------------------------------------------------
    // Provider params / prompt-cache plumbing
    // -----------------------------------------------------------------

    fn profile_with_provider(provider: &str) -> meerkat_mob::Profile {
        serde_json::from_value(serde_json::json!({
            "model": "base-model",
            "provider": provider,
        }))
        .expect("profile")
    }

    fn draft_with_provider_params(params: Option<ProviderParamsOverride>) -> AgentBuildDraft {
        AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: params,
        }
    }

    fn spawn_params(spawn: &SpawnMemberSpec) -> ProviderParamsOverride {
        spawn
            .override_profile
            .as_ref()
            .expect("provider params require a profile snapshot")
            .provider_params
            .clone()
            .expect("profile carries provider params")
    }

    fn spawn_openai_tag(spawn: &SpawnMemberSpec) -> OpenAiProviderTag {
        match spawn_params(spawn).provider_tag {
            Some(ProviderTag::OpenAi(tag)) => tag,
            other => panic!("expected an OpenAI provider tag, got {other:?}"),
        }
    }

    fn implicit_30m() -> OpenAiPromptCacheOptions {
        OpenAiPromptCacheOptions {
            mode: Some(OpenAiPromptCacheMode::Implicit),
            ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
        }
    }

    /// A declared prompt-cache policy reaches the profile meerkat reads
    /// (`config.provider_params = profile.provider_params`), and masks
    /// `provider_params` for resume — unmasked, durable session metadata wins
    /// and the declaration is inert on every identity that already has a
    /// session.
    #[test]
    fn build_spawn_spec_lands_declared_prompt_cache_options() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_options: Some(implicit_30m()),
                ..Default::default()
            })),
            ..Default::default()
        }));

        let spawn = build_spawn_spec(
            &durable_spec(),
            &draft,
            Some(&profile_with_provider("openai")),
        )
        .expect("spawn spec");

        assert_eq!(
            spawn_openai_tag(&spawn).prompt_cache_options,
            Some(implicit_30m())
        );
        assert!(
            spawn
                .override_profile
                .as_ref()
                .expect("profile snapshot")
                .resume_overrides
                .contains(&ResumeOverrideField::ProviderParams),
            "an explicit declaration must win over durable metadata on resume"
        );
    }

    /// A draft that sets one knob must not wipe the knobs the mob definition
    /// declared on the same profile.
    #[test]
    fn build_spawn_spec_merges_draft_params_over_profile_declaration() {
        let mut base_profile = profile_with_provider("openai");
        base_profile.provider_params = Some(ProviderParamsOverride {
            thinking_budget_tokens: Some(8192),
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(ReasoningEffort::High),
                ..Default::default()
            })),
            ..Default::default()
        });
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_options: Some(implicit_30m()),
                ..Default::default()
            })),
            ..Default::default()
        }));

        let spawn =
            build_spawn_spec(&durable_spec(), &draft, Some(&base_profile)).expect("spawn spec");

        assert_eq!(
            spawn_params(&spawn).thinking_budget_tokens,
            Some(8192),
            "profile-declared knobs survive a draft that sets an unrelated knob"
        );
        let tag = spawn_openai_tag(&spawn);
        assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(tag.prompt_cache_options, Some(implicit_30m()));
    }

    /// The profile snapshots Meta builds for a spawn (a pinned-provider
    /// model override, or declared provider params) are copies of the
    /// definition profile, so a durable worker profile stays durable in
    /// them: Meerkat reads `durable` off the snapshot it is handed.
    #[test]
    fn build_spawn_spec_snapshots_keep_a_durable_profile_durable() {
        let durable: meerkat_mob::Profile = serde_json::from_value(serde_json::json!({
            "model": "base-model",
            "provider": "openai",
            "durable": true,
        }))
        .expect("durable profile");
        assert!(durable.durable);

        let model_pin = AgentBuildDraft {
            model: Some("gpt-test".to_string()),
            ..draft_with_provider_params(None)
        };
        let pinned =
            build_spawn_spec(&durable_spec(), &model_pin, Some(&durable)).expect("spawn spec");
        assert!(
            pinned
                .override_profile
                .as_ref()
                .expect("a pinned provider keeps the snapshot")
                .durable
        );

        let params = draft_with_provider_params(Some(ProviderParamsOverride {
            thinking_budget_tokens: Some(1024),
            ..Default::default()
        }));
        let declared =
            build_spawn_spec(&durable_spec(), &params, Some(&durable)).expect("spawn spec");
        assert!(
            declared
                .override_profile
                .as_ref()
                .expect("declared params ride a snapshot")
                .durable
        );
    }

    /// Meerkat sends no `prompt_cache_key`; the identity supplies the default
    /// routing bucket.
    #[test]
    fn build_spawn_spec_defaults_prompt_cache_key_from_identity() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_options: Some(implicit_30m()),
                ..Default::default()
            })),
            ..Default::default()
        }));

        let spawn = build_spawn_spec(
            &durable_spec(),
            &draft,
            Some(&profile_with_provider("openai")),
        )
        .expect("spawn spec");

        assert_eq!(
            spawn_openai_tag(&spawn).prompt_cache_key.as_deref(),
            Some("meta:agent:alpha")
        );
    }

    /// The key is a routing hint the caller owns: an explicit one is never
    /// replaced.
    #[test]
    fn build_spawn_spec_keeps_caller_supplied_prompt_cache_key() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_key: Some("tenant-a:shared-prefix".to_string()),
                ..Default::default()
            })),
            ..Default::default()
        }));

        let spawn = build_spawn_spec(
            &durable_spec(),
            &draft,
            Some(&profile_with_provider("openai")),
        )
        .expect("spawn spec");

        assert_eq!(
            spawn_openai_tag(&spawn).prompt_cache_key.as_deref(),
            Some("tenant-a:shared-prefix")
        );
    }

    /// An unstable key is worse than none: it would move the identity to a
    /// cold backend on every restart. Two independent builds for the same
    /// identity must agree; the key is a function of the identity alone.
    #[test]
    fn build_spawn_spec_prompt_cache_key_is_stable_across_builds() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_options: Some(implicit_30m()),
                ..Default::default()
            })),
            ..Default::default()
        }));
        let profile = profile_with_provider("openai");

        let first =
            build_spawn_spec(&durable_spec(), &draft, Some(&profile)).expect("first spawn spec");
        let second =
            build_spawn_spec(&durable_spec(), &draft, Some(&profile)).expect("second spawn spec");

        assert_eq!(
            spawn_openai_tag(&first).prompt_cache_key,
            spawn_openai_tag(&second).prompt_cache_key
        );
        assert_eq!(
            spawn_openai_tag(&first).prompt_cache_key.as_deref(),
            Some("meta:agent:alpha")
        );
    }

    /// `prompt_cache_key` lives on the OpenAI tag. Fabricating one for an
    /// Anthropic identity is a typed provider-family fault at meerkat's merge
    /// seam (`ProviderTagMismatch`), which would fail the turn.
    #[test]
    fn build_spawn_spec_skips_prompt_cache_key_for_non_openai_identity() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            temperature: Some(0.2),
            ..Default::default()
        }));

        let spawn = build_spawn_spec(
            &durable_spec(),
            &draft,
            Some(&profile_with_provider("anthropic")),
        )
        .expect("spawn spec");

        let params = spawn_params(&spawn);
        assert_eq!(params.temperature, Some(0.2));
        assert!(
            params.provider_tag.is_none(),
            "no OpenAI tag may be fabricated for an Anthropic-backed identity"
        );
    }

    /// Backward compatibility: a draft that declares nothing keeps the
    /// field-scoped path byte-for-byte as before. No profile snapshot is
    /// minted, so no identity is moved onto the definition-drift freeze just
    /// because this field now exists.
    #[test]
    fn build_spawn_spec_without_provider_params_keeps_field_scoped_path() {
        let draft = draft_with_provider_params(None);

        let spawn = build_spawn_spec(
            &durable_spec(),
            &draft,
            Some(&profile_with_provider("openai")),
        )
        .expect("spawn spec");

        assert!(
            spawn.override_profile.is_none(),
            "an undeclared draft must not mint a profile snapshot"
        );
        assert!(spawn.model_override.is_none());
    }

    /// Declared params with no inline definition profile to carry them (a
    /// realm-ref binding) fail closed. Dropping them silently is the failure
    /// mode that reads downstream as "caching just doesn't work".
    #[test]
    fn build_spawn_spec_rejects_declared_provider_params_without_inline_profile() {
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            temperature: Some(0.2),
            ..Default::default()
        }));

        let error = build_spawn_spec(&durable_spec(), &draft, None)
            .expect_err("no inline profile can carry provider params");

        match error {
            BridgeError::InvalidInput(detail) => {
                assert!(detail.contains("provider_params"), "{detail}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    /// A provider-family conflict between the draft and the profile is a
    /// typed fault, never a silent union of unrelated provider bags.
    #[test]
    fn build_spawn_spec_rejects_provider_tag_family_conflict() {
        let mut base_profile = profile_with_provider("openai");
        base_profile.provider_params = Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Anthropic(Default::default())),
            ..Default::default()
        });
        let draft = draft_with_provider_params(Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_options: Some(implicit_30m()),
                ..Default::default()
            })),
            ..Default::default()
        }));

        let error = build_spawn_spec(&durable_spec(), &draft, Some(&base_profile))
            .expect_err("provider families must not be unioned");

        match error {
            BridgeError::InvalidInput(detail) => {
                assert!(detail.contains("provider params"), "{detail}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    /// Realm-ref bindings (no inline base profile) now take the field-scoped
    /// seam instead of skipping the model override with a warning.
    #[test]
    fn build_spawn_spec_model_override_works_without_base_profile() {
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: Some("gpt-test".to_string()),
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: LocalExternalToolOverlay::new(Arc::new(EmptyDispatcher)),
            provider_params: None,
        };

        let spawn = build_spawn_spec(&durable_spec(), &draft, None).expect("spawn spec");

        assert_eq!(spawn.model_override.as_deref(), Some("gpt-test"));
        assert!(spawn.override_profile.is_none());
    }

    #[test]
    fn build_spawn_spec_keeps_profile_fallback_policy_under_model_override() {
        let mut base = profile_with_provider("openai");
        base.model_fallback = Some(meerkat_core::config::ModelFallbackConfig {
            enabled: Some(false),
            ..Default::default()
        });
        let mut draft = draft_with_provider_params(None);
        draft.model = Some("gpt-5.5".to_string());
        let spawn = build_spawn_spec(&durable_spec(), &draft, Some(&base)).expect("spawn");
        assert_eq!(
            spawn
                .override_profile
                .expect("pinned profile")
                .model_fallback,
            base.model_fallback
        );
    }

    /// Why `member_id_for_runtime_id` and the wire projection return errors
    /// instead of deriving an id when the runtime_members mapping is missing.
    ///
    /// This covers the HAZARD rather than the full missing-map path, which needs
    /// a live bridge and therefore a whole mob. What it pins down is the pair of
    /// facts that made the old fallback unsafe, and either one changing should
    /// break this test rather than silently re-permit derivation.
    #[test]
    fn a_stable_roster_id_would_parse_as_a_fake_runtime_id() {
        // After the stable-identity lowering, a roster member id decodes to a
        // BARE durable identity.
        let roster_id = crate::member_comms_id::mob_member_id_str("agent:alpha").into_owned();
        let decoded = crate::member_comms_id::runtime_alias_str(&roster_id).into_owned();
        assert_eq!(
            decoded, "agent:alpha",
            "a stable roster id must decode to the bare durable identity"
        );

        // And `AgentRuntimeId` accepts it. That is the whole danger: the old
        // fallback fed exactly this string to `parse`, which succeeded and
        // minted a runtime id naming no incarnation, so a missing mapping
        // produced a plausible-looking lie instead of an error.
        let parsed = AgentRuntimeId::parse(&decoded);
        assert!(
            parsed.is_ok(),
            "if AgentRuntimeId ever rejects a bare identity this test's premise is stale, but \
             today it accepts it, which is why derivation had to be removed rather than \
             guarded: {parsed:?}"
        );
        assert!(
            crate::member_comms_id::durable_identity_from_runtime_alias(&decoded).is_none(),
            "the accepted string is not a generated runtime alias at all, so nothing \
             downstream could have detected the manufactured id"
        );
    }

    #[test]
    fn fresh_fallback_collision_classifier_matches_member_already_exists() {
        let error = meerkat_mob::MobError::MemberAlreadyExists(meerkat_mob::AgentIdentity::from(
            "rt-agent-alpha-0",
        ));

        assert!(
            is_member_already_exists_error(&error),
            "fresh fallback must retry recreate-over-running-member collisions"
        );
    }

    #[test]
    fn build_spawn_spec_maps_remote_runtime_binding() {
        let mut spec = durable_spec();
        spec.backend = Some(meerkat_mob::MobBackendKind::External);
        spec.binding = Some(
            serde_json::from_value(serde_json::json!({
                "kind": "external",
                "address": "tcp://127.0.0.1:4777",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
                }
            }))
            .expect("wire binding"),
        );
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };

        let spawn = build_spawn_spec(&spec, &draft, None).expect("spawn spec");

        // meerkat 0.7: MemberCommsName is fail-closed (no `:` in member-id
        // components), so the roster id is the comms-safe encoding of the
        // public identity `agent:alpha` (see crate::member_comms_id).
        assert_eq!(
            spawn.identity.as_str(),
            crate::member_comms_id::mob_member_id_str("agent:alpha").as_ref()
        );
        assert_eq!(spawn.backend, Some(meerkat_mob::MobBackendKind::External));
        assert!(
            matches!(
                spawn.binding,
                Some(meerkat_mob::RuntimeBinding::External { .. })
            ),
            "expected external runtime binding, got {:?}",
            spawn.binding
        );
        if let Some(meerkat_mob::RuntimeBinding::External {
            address, pubkey, ..
        }) = spawn.binding
        {
            assert_eq!(address.as_str(), "tcp://127.0.0.1:4777");
            assert_eq!(pubkey, [7; 32]);
        }
    }

    #[test]
    fn external_binding_spawn_specs_require_generated_owner_context() {
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: None,
            additional_instructions: Vec::new(),
            labels: Default::default(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };

        // Session-backed members keep the plain spawn path.
        let session_spawn = build_spawn_spec(&durable_spec(), &draft, None).expect("spawn spec");
        assert!(
            !spawn_spec_requires_generated_owner_context(&session_spawn),
            "session-backed spawns must not require a generated owner context"
        );

        // meerkat 0.7.1 MultiBackendProvisioner::provision_member fails
        // external (peer-only) members closed without a generated owner
        // binding; the bridge must route them through
        // spawn_spec_with_generated_owner_context.
        let mut spec = durable_spec();
        spec.backend = Some(meerkat_mob::MobBackendKind::External);
        spec.binding = Some(
            serde_json::from_value(serde_json::json!({
                "kind": "external",
                "address": "tcp://127.0.0.1:4777",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
                }
            }))
            .expect("wire binding"),
        );
        let external_spawn = build_spawn_spec(&spec, &draft, None).expect("spawn spec");
        assert!(
            spawn_spec_requires_generated_owner_context(&external_spawn),
            "external peer-only spawns must carry a generated owner binding on meerkat 0.7.1"
        );
    }

    #[test]
    fn bridge_delivery_repair_covers_missing_bridge_session_snapshot() {
        let error = "session bridge mob error: member rt:review:singleton:0 failed to restore session 019e5fc2-dad4-77e2-abbe-a8a66bc15f66: missing bridge session snapshot for '019e5fc2-dad4-77e2-abbe-a8a66bc15f66'";

        assert!(
            is_repairable_bridge_delivery_error(error),
            "stale bridge-session bindings should be repaired before retrying delivery"
        );
        assert!(
            is_repairable_bridge_delivery_error("missing event injector capability for member"),
            "existing stale event-injector repair path must remain covered"
        );
        assert!(
            is_repairable_bridge_delivery_error(
                "mob member rt:us-president:0 missing required capability interaction_event_injector: autonomous member dispatch"
            ),
            "newer autonomous member dispatch wording should repair and retry instead of dropping the event"
        );
        assert!(
            is_repairable_bridge_delivery_error(
                "previous member cleanup ambiguous for member rt:deep-investigator:singleton:0"
            ),
            "ambiguous Meerkat respawn cleanup should trigger bridge repair instead of failing delivery"
        );
        assert!(
            !is_repairable_bridge_delivery_error("model provider returned rate limit"),
            "ordinary turn failures must not trigger member repair"
        );
    }

    #[test]
    fn bridge_delivery_repair_classifies_topology_restore_failure_as_degraded() {
        let identity = meerkat_mob::AgentIdentity::from("rt:review:singleton:0");
        let receipt = meerkat_mob::MemberRespawnReceipt::new(
            identity.clone(),
            meerkat_mob::AgentRuntimeId::new(identity, meerkat_mob::ids::Generation::INITIAL),
            meerkat_mob::FenceToken::new(1),
            meerkat_mob::FenceToken::new(2),
        );
        let err = MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids: vec![meerkat_mob::RespawnTopologyPeerId::from(
                "initiative:broken",
            )],
        };

        assert_eq!(
            classify_member_repair_respawn_failure(&err),
            MemberRepairRespawnFailure::DegradedTopologyRestore {
                failed_peer_ids: vec!["initiative:broken".to_string()]
            },
            "failed peer edges should degrade bridge repair instead of bricking delivery"
        );
        assert!(
            matches!(
                classify_member_repair_respawn_failure(&MobRespawnError::NoRuntimeControl {
                    identity: meerkat_mob::AgentIdentity::from("rt:review:singleton:0"),
                }),
                MemberRepairRespawnFailure::Fatal(_)
            ),
            "ordinary respawn failures must still fail bridge repair"
        );
    }

    /// RESUME AUTHORS NOTHING (meerkat 0.8.11 prompt contract): even when
    /// the draft carries an explicit customizer prompt, the resume spawn
    /// spec must NOT re-send it. `Message::System` is ordinary ordered
    /// authored transcript content; re-sending assembled configuration at
    /// every activation would append a byte-duplicate System per boot (the
    /// prompt-refresh depth leak, reborn). The +0-on-neutral-resume
    /// invariant is structural — no append command exists on this path —
    /// and mid-thread instruction changes are a caller EXPLICITLY authoring
    /// a System message via `StartTurnRequest.system_messages`, exactly
    /// once, as part of a turn.
    #[test]
    fn resume_spawn_spec_authors_nothing() {
        let draft = AgentBuildDraft {
            compaction_curator: Default::default(),
            model: None,
            system_prompt: Some("explicit customizer prompt".to_string()),
            additional_instructions: Vec::new(),
            labels: std::collections::BTreeMap::new(),
            app_context: None,
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
        };
        let session_id = meerkat_core::types::SessionId::new();

        let spawn = build_resume_spawn_spec(&durable_spec(), &draft, None, &session_id, None)
            .expect("resume spawn spec");

        assert_eq!(
            spawn.system_prompt_override, None,
            "resume must not author or re-send prompt configuration; explicit System \
             authoring via a turn is the only mid-thread instruction change"
        );
        match &spawn.launch_mode {
            MemberLaunchMode::Resume {
                bridge_session_id, ..
            } => {
                assert_eq!(bridge_session_id, &session_id);
            }
            other => panic!("expected Resume launch mode, got {other:?}"),
        }
    }

    /// Core's typed Absent is a TYPED variant match: only
    /// `SessionUnavailableForResume { reason: Absent }` reads as absent, and
    /// no error WORDING can impersonate it. Archived-but-intact and every
    /// other refusal carry a transcript.
    #[test]
    fn typed_absent_is_a_variant_match_never_wording() {
        let session_id = meerkat_core::types::SessionId::new();
        assert!(durable_snapshot_is_typed_absent(
            &meerkat_mob::MobError::SessionUnavailableForResume {
                session_id: session_id.clone(),
                reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                runtime_state: None,
                verdict: None,
            }
        ));
        // Archived-but-intact carries a transcript: not absent.
        assert!(!durable_snapshot_is_typed_absent(
            &meerkat_mob::MobError::SessionUnavailableForResume {
                session_id,
                reason: meerkat_mob::error::SessionResumeUnavailableReason::ArchivedNotRevivable,
                runtime_state: Some("archived".to_string()),
                verdict: None,
            }
        ));
        // The wording attack the substring classifier was vulnerable to: an
        // arbitrary error whose MESSAGE contains the magic prose must not
        // read as absent.
        let impersonator = meerkat_mob::MobError::Internal(
            "missing durable session snapshot for '019e5fc2-dad4-77e2-abbe-a8a66bc15f66'"
                .to_string(),
        );
        assert!(
            impersonator
                .to_string()
                .contains("missing durable session snapshot")
        );
        assert!(!durable_snapshot_is_typed_absent(&impersonator));
        // And should the impersonator surface through the legacy respawn
        // ladder instead, it must classify Fatal — no wording reaches a
        // recovery arm there either.
        assert!(
            matches!(
                classify_member_repair_respawn_failure(&MobRespawnError::Mob(impersonator)),
                MemberRepairRespawnFailure::Fatal(_)
            ),
            "an Internal impersonator must stay Fatal in the respawn ladder"
        );
    }

    /// The rebuild the recoverable-cleanup arm of the legacy respawn ladder
    /// spawns when the cleanup left no member: the SAME identity, the
    /// pre-delivery role and labels, `Fresh` launch mode.
    #[test]
    fn pre_delivery_rebuild_spec_keeps_the_identity_role_and_labels() {
        let member_id = MobAgentIdentity::from("rt-agent-alpha-0");

        let mut labels = std::collections::BTreeMap::new();
        labels.insert("agent_identity".to_string(), "agent:alpha".to_string());
        let spec = fresh_member_spec_from_pre_delivery_entry(
            &member_id,
            meerkat_mob::ProfileName::from("worker"),
            labels.clone(),
        );
        assert_eq!(
            spec.identity.as_str(),
            member_id.as_str(),
            "the rebuild must keep the SAME identity"
        );
        assert_eq!(spec.role_name.as_str(), "worker");
        assert_eq!(spec.labels, Some(labels));
        assert_eq!(
            spec.continuity_intent,
            SpawnContinuityIntent::DurableIdentity {
                continuity_key: "agent:alpha".to_string(),
            },
            "the rebuilt member keeps the durable-identity marker of the member it replaces"
        );
        assert!(
            matches!(spec.launch_mode, MemberLaunchMode::Fresh),
            "the rebuild takes a fresh session"
        );

        // No pre-delivery labels: the spec carries none.
        let bare = fresh_member_spec_from_pre_delivery_entry(
            &member_id,
            meerkat_mob::ProfileName::from("worker"),
            std::collections::BTreeMap::new(),
        );
        assert_eq!(bare.labels, None);
        assert_eq!(
            bare.continuity_intent,
            SpawnContinuityIntent::Ephemeral,
            "a member that was never labelled with an identity was never marked"
        );
    }

    #[test]
    fn resume_error_classification_reads_typed_errors_only() {
        use meerkat_core::service::DurableResumeHold;
        use meerkat_mob::error::SessionResumeUnavailableReason;
        let unavailable = |reason| meerkat_mob::MobError::SessionUnavailableForResume {
            session_id: meerkat_core::types::SessionId::new(),
            reason,
            runtime_state: None,
            verdict: None,
        };
        // Every typed reason Core gives is carried as is, Absent included:
        // the caller decides what Absent means for the identity, never the
        // bridge.
        for reason in [
            SessionResumeUnavailableReason::Absent,
            SessionResumeUnavailableReason::ArchivedNotRevivable,
            SessionResumeUnavailableReason::CommittedBoundaryUnprovable,
            SessionResumeUnavailableReason::AuthorityChangedDuringMaterialization,
            SessionResumeUnavailableReason::ContradictoryDurableAuthority,
        ] {
            assert_eq!(
                classify_resume_error(&unavailable(reason)),
                ResumeRejectionKind::Unavailable { reason }
            );
        }

        let restore_failed = |hold| meerkat_mob::MobError::MemberRestoreFailed {
            hold,
            member_id: meerkat_mob::ids::AgentIdentity::from("agent-alpha"),
            session_id: None,
            reason: "durable snapshot missing".to_string(),
        };
        assert_eq!(
            classify_resume_error(&restore_failed(None)),
            ResumeRejectionKind::MemberRestoreFailed
        );
        assert_eq!(
            classify_resume_error(&restore_failed(Some(
                DurableResumeHold::TailHeldForRecovery
            ))),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::TailHeldForRecovery
            }
        );

        // No error text is read: the transcript-continuity wording that used
        // to be matched classifies as any other untyped failure.
        let continuity = meerkat_mob::MobError::Internal(
            "session save rejected: incoming transcript is not a continuation of persisted \
             revision sha256:d57e07"
                .to_string(),
        );
        assert_eq!(
            classify_resume_error(&continuity),
            ResumeRejectionKind::Other
        );
        let other = meerkat_mob::MobError::WiringError("unrelated".to_string());
        assert_eq!(classify_resume_error(&other), ResumeRejectionKind::Other);
        let required = meerkat_mob::MobError::MemberRoleMigrationRequired {
            member_id: meerkat_mob::AgentIdentity::from("member-1"),
            stored_role: meerkat_mob::ProfileName::from("identity"),
            requested_role: meerkat_mob::ProfileName::from("identity-member"),
        };
        assert_eq!(
            classify_resume_error(&required),
            ResumeRejectionKind::RoleMigrationNotApplied
        );
        let rejected = meerkat_mob::MobError::MemberRoleMigrationRejected {
            member_id: meerkat_mob::AgentIdentity::from("member-1"),
            declared_predecessor_role: meerkat_mob::ProfileName::from("predecessor"),
            requested_role: meerkat_mob::ProfileName::from("identity-member"),
            reason: "durable predecessor role is 'identity', not the declared role".to_string(),
        };
        assert_eq!(
            classify_resume_error(&rejected),
            ResumeRejectionKind::RoleMigrationNotApplied
        );

        // meerkat-mob hands a joined observer (and the collision retire before
        // a resume retry) the SAME failure through its transparent shared
        // wrappers. The classification reads through them.
        let wrapped_archived = meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(
            unavailable(SessionResumeUnavailableReason::ArchivedNotRevivable),
        ));
        assert_eq!(
            classify_resume_error(&wrapped_archived),
            ResumeRejectionKind::Unavailable {
                reason: SessionResumeUnavailableReason::ArchivedNotRevivable
            }
        );
        let wrapped_restore_failed =
            meerkat_mob::MobError::SharedRetirementFailure(Arc::new(restore_failed(None)));
        assert_eq!(
            classify_resume_error(&wrapped_restore_failed),
            ResumeRejectionKind::MemberRestoreFailed
        );
        let wrapped_absent = meerkat_mob::MobError::SharedRetirementFailure(Arc::new(unavailable(
            SessionResumeUnavailableReason::Absent,
        )));
        assert_eq!(
            classify_resume_error(&wrapped_absent),
            ResumeRejectionKind::Unavailable {
                reason: SessionResumeUnavailableReason::Absent
            }
        );
        // The resume door carries the typed kind and names the session the
        // resume asked for.
        let identity = AgentIdentity::parse("domain:archived")
            .unwrap_or_else(|err| panic!("test identity must parse: {err}"));
        let session_id = meerkat_core::types::SessionId::new();
        let BridgeError::ResumeRejected {
            kind,
            session_id: refused,
            ..
        } = resume_rejected(
            &identity,
            &session_id,
            &wrapped_archived,
            "collision retire before resume retry",
        )
        else {
            panic!("a refused resume must be ResumeRejected");
        };
        assert_eq!(
            kind,
            ResumeRejectionKind::Unavailable {
                reason: SessionResumeUnavailableReason::ArchivedNotRevivable
            }
        );
        assert_eq!(refused, session_id);
    }

    /// One predicate decides Absent: `durable_snapshot_is_typed_absent`
    /// agrees with `classify_resume_error` on every error shape the resume
    /// door can see, bare or behind one or two shared wrappers, so a joined
    /// resume observer's wrapped Absent forgets the dead bindings exactly as a
    /// bare one does, and no wording reads as absent.
    #[test]
    fn the_absent_predicate_agrees_with_the_classifier_on_every_shape() {
        use meerkat_core::service::DurableResumeHold;
        use meerkat_mob::error::SessionResumeUnavailableReason;
        let unavailable = |reason| meerkat_mob::MobError::SessionUnavailableForResume {
            session_id: meerkat_core::types::SessionId::new(),
            reason,
            runtime_state: None,
            verdict: None,
        };
        let restore_failed = |hold| meerkat_mob::MobError::MemberRestoreFailed {
            hold,
            member_id: meerkat_mob::ids::AgentIdentity::from("agent-alpha"),
            session_id: None,
            reason: "durable snapshot missing".to_string(),
        };
        let retirement = |inner: meerkat_mob::MobError| {
            meerkat_mob::MobError::SharedRetirementFailure(Arc::new(inner))
        };
        let lifecycle = |inner: meerkat_mob::MobError| {
            meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(inner))
        };
        let mut shapes = Vec::new();
        for reason in [
            SessionResumeUnavailableReason::Absent,
            SessionResumeUnavailableReason::ArchivedNotRevivable,
            SessionResumeUnavailableReason::CommittedBoundaryUnprovable,
            SessionResumeUnavailableReason::AuthorityChangedDuringMaterialization,
            SessionResumeUnavailableReason::ContradictoryDurableAuthority,
        ] {
            shapes.push(unavailable(reason));
            shapes.push(retirement(unavailable(reason)));
            shapes.push(lifecycle(unavailable(reason)));
            shapes.push(lifecycle(retirement(unavailable(reason))));
        }
        shapes.push(restore_failed(None));
        shapes.push(restore_failed(Some(DurableResumeHold::TailHeldForRecovery)));
        shapes.push(retirement(restore_failed(None)));
        shapes.push(meerkat_mob::MobError::Internal(
            "missing durable session snapshot for '019e5fc2-dad4-77e2-abbe-a8a66bc15f66'"
                .to_string(),
        ));
        shapes.push(meerkat_mob::MobError::WiringError("unrelated".to_string()));

        let mut absent = 0;
        for shape in &shapes {
            let classified_absent = matches!(
                classify_resume_error(shape),
                ResumeRejectionKind::Unavailable {
                    reason: SessionResumeUnavailableReason::Absent
                }
            );
            assert_eq!(
                durable_snapshot_is_typed_absent(shape),
                classified_absent,
                "the two Absent reads disagree on {shape:?}"
            );
            absent += usize::from(classified_absent);
        }
        assert_eq!(
            absent, 4,
            "bare, wrapped once by either wrapper and wrapped twice: exactly the Absent shapes"
        );
    }

    /// The host-adoption decision of the resume collision arm and of
    /// `ensure_member_live`, over all six outcomes of the one identity intent
    /// read (r3 Q7): a valid row, Present or Absent, refuses host-adopted; no
    /// row takes the legacy path; an unsupported or malformed row or a failed
    /// read refuses retryable at both sites. Core cannot produce the last
    /// three on demand, so this table is what pins them; the Core-backed tests
    /// cover the real Present and Missing paths. The classifier is generic
    /// over the row, so the table instantiates it at Core's `IdentityIntent`
    /// to carry the real Present and Absent variants.
    #[test]
    fn the_host_adoption_decision_covers_every_intent_read_outcome()
    -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_mob::identity::{IdentityIntent, IdentityStoredObservation};

        enum Expected {
            HostAdopted,
            LegacyPath,
            Retryable(&'static str),
        }
        let identity = AgentIdentity::parse("domain:adopted")?;
        let session_id = meerkat_core::types::SessionId::new();
        let member = member_id_for_identity(&identity);
        let present: IdentityIntent = serde_json::from_value(serde_json::json!({
            "desired_presence": "present",
            "identity": member,
            "session": {
                "session_id": session_id,
                "lineage_id": format!("session:{session_id}"),
                "lineage_generation": 0,
                "authority_policy": "require_existing",
            },
            "member": {
                "material": {
                    "profile_name": "worker",
                    "profile": { "model": "test-model", "provider": "anthropic" },
                    "definition_extract": { "profile_names": ["worker"] },
                    "overlay": { "runtime_mode": "autonomous_host" },
                    "execution": { "execution": "controlling_session" },
                },
            },
        }))?;
        assert!(matches!(present, IdentityIntent::Present { .. }));
        let absent = IdentityIntent::Absent { identity: member };
        let evidence = "0".repeat(64);
        type Read = Result<IdentityStoredObservation<IdentityIntent>, meerkat_mob::MobError>;
        let rows: Vec<(&str, Read, Expected)> = vec![
            (
                "valid present",
                Ok(IdentityStoredObservation::Valid(present)),
                Expected::HostAdopted,
            ),
            (
                "valid absent",
                Ok(IdentityStoredObservation::Valid(absent)),
                Expected::HostAdopted,
            ),
            (
                "missing",
                Ok(IdentityStoredObservation::Missing),
                Expected::LegacyPath,
            ),
            (
                "unsupported",
                Ok(IdentityStoredObservation::Unsupported {
                    evidence_digest: evidence.clone(),
                    detail: "identity intent schema version 9".to_string(),
                }),
                Expected::Retryable("unsupported identity intent row"),
            ),
            (
                "malformed",
                Ok(IdentityStoredObservation::Malformed {
                    evidence_digest: evidence,
                    detail: "truncated row".to_string(),
                }),
                Expected::Retryable("malformed identity intent row"),
            ),
            (
                "read failed",
                Err(meerkat_mob::MobError::Internal(
                    "identity store unavailable".to_string(),
                )),
                Expected::Retryable("identity intent read failed"),
            ),
        ];
        for (label, read, expected) in rows {
            let decided = classify_identity_intent(read);
            let resumed = resume_collision_intent_gate(decided.clone(), &identity, &session_id);
            let ensured = ensure_member_live_intent_gate(decided, &identity);
            match expected {
                Expected::HostAdopted => {
                    for (site, outcome) in [("resume", &resumed), ("ensure", &ensured)] {
                        assert!(
                            matches!(
                                outcome,
                                Err(BridgeError::HostAdopted { identity: refused })
                                    if refused == &identity
                            ),
                            "{label} at {site} must refuse host-adopted: {outcome:?}"
                        );
                    }
                }
                Expected::LegacyPath => {
                    assert!(resumed.is_ok(), "{label} at resume: {resumed:?}");
                    assert!(ensured.is_ok(), "{label} at ensure: {ensured:?}");
                }
                Expected::Retryable(cause) => {
                    let Err(BridgeError::ResumeRejected {
                        kind: ResumeRejectionKind::Other,
                        session_id: refused,
                        detail,
                    }) = &resumed
                    else {
                        panic!("{label} at resume must refuse retryable: {resumed:?}");
                    };
                    assert_eq!(refused, &session_id, "{label}");
                    assert!(
                        detail.contains(cause) && detail.contains("retryable"),
                        "{label} at resume: {detail}"
                    );
                    let Err(BridgeError::Mob(detail)) = &ensured else {
                        panic!("{label} at ensure must refuse retryable: {ensured:?}");
                    };
                    assert!(
                        detail.contains(cause) && detail.contains("retryable"),
                        "{label} at ensure: {detail}"
                    );
                }
            }
        }
        Ok(())
    }

    /// A probe that cannot answer never leads to a destructive retire. Both
    /// repairs refuse an
    /// unanswered probe retryable, a confirmed absence still takes each arm's
    /// Absent refusal, and only a source some authority knows (or no
    /// authority to ask) lets the retire run. The gates run before the
    /// capture and the retire at both call sites, so a refusal disposes
    /// nothing.
    #[test]
    fn the_retire_gates_refuse_an_absent_or_unanswered_resume_source() {
        use super::{BridgeError, ResumeRejectionKind, ResumeSourceProbe};
        let session_id = meerkat_core::types::SessionId::new();
        let identity = super::AgentIdentity::parse("gate:worker").expect("identity");
        let unanswered = || ResumeSourceProbe::Unanswered {
            authority: "archive authority",
            error: "disk I/O error".to_string(),
        };

        // Delivery repair.
        assert!(
            super::delivery_repair_retire_gate(ResumeSourceProbe::NotAbsent, &session_id).is_ok()
        );
        match super::delivery_repair_retire_gate(ResumeSourceProbe::ConfirmedAbsent, &session_id) {
            Err(BridgeError::ResumeRejected {
                kind:
                    ResumeRejectionKind::Unavailable {
                        reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                    },
                session_id: refused,
                ..
            }) => assert_eq!(refused, session_id),
            other => panic!("a confirmed-absent source takes the Absent refusal, got {other:?}"),
        }
        match super::delivery_repair_retire_gate(unanswered(), &session_id) {
            Err(BridgeError::ResumeRejected {
                kind:
                    ResumeRejectionKind::SourceUnanswered {
                        authority: "archive authority",
                    },
                session_id: refused,
                detail,
            }) => {
                assert_eq!(refused, session_id);
                assert!(
                    detail.contains("archive authority")
                        && detail.contains("disk I/O error")
                        && detail.contains("nothing was disposed")
                        && detail.contains("retryable"),
                    "{detail}"
                );
            }
            other => panic!("an unanswered probe refuses retryable, got {other:?}"),
        }

        // Collision repair.
        assert!(
            super::collision_retire_gate(ResumeSourceProbe::NotAbsent, &identity, &session_id)
                .is_ok()
        );
        match super::collision_retire_gate(
            ResumeSourceProbe::ConfirmedAbsent,
            &identity,
            &session_id,
        ) {
            Err(BridgeError::ResumeRejected { detail, .. }) => assert!(
                detail.contains("is confirmed absent")
                    && detail.contains("only live copy of the session"),
                "{detail}"
            ),
            other => {
                panic!("a confirmed-absent source refuses the collision retire, got {other:?}")
            }
        }
        match super::collision_retire_gate(unanswered(), &identity, &session_id) {
            Err(BridgeError::ResumeRejected { kind, detail, .. }) => {
                assert_eq!(
                    kind,
                    ResumeRejectionKind::SourceUnanswered {
                        authority: "archive authority"
                    },
                    "an unanswered probe is its own retryable kind, not an absence"
                );
                assert!(
                    detail.contains("disk I/O error") && detail.contains("retryable"),
                    "{detail}"
                );
            }
            other => panic!("an unanswered probe refuses the collision retire, got {other:?}"),
        }
    }

    /// The probe classifies a failed archive authority read as `Unanswered`,
    /// never as `NotAbsent`, so the gates above refuse instead of failing
    /// open. An authority that answers without knowing the session confirms
    /// absence, one that knows it does not, and a bridge with no authority to
    /// ask keeps the pre-guard behavior.
    #[tokio::test]
    async fn a_faulting_resume_source_probe_is_unanswered_not_absent() {
        use super::ResumeSourceProbe;
        let dir = tempfile::tempdir().expect("fixture directory");
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"resume-source-probe\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("mob definition");
        let spec = crate::MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec");
        let runtime = crate::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime");
        let session_id = meerkat_core::types::SessionId::new();

        let faulting = super::MobSessionBridge::with_session_service(
            runtime.handle(),
            crate::mob_handle_runtime::archive_authority_probe(None),
        );
        match faulting.probe_resume_source(&session_id).await {
            ResumeSourceProbe::Unanswered { authority, error } => {
                assert_eq!(authority, "archive authority");
                assert!(!error.is_empty(), "the fault must carry its error");
            }
            other => panic!("a failed authority read must stay unanswered, got {other:?}"),
        }

        let absent = super::MobSessionBridge::with_session_service(
            runtime.handle(),
            crate::mob_handle_runtime::archive_authority_probe(Some(false)),
        );
        assert_eq!(
            absent.probe_resume_source(&session_id).await,
            ResumeSourceProbe::ConfirmedAbsent
        );

        let known = super::MobSessionBridge::with_session_service(
            runtime.handle(),
            crate::mob_handle_runtime::archive_authority_probe(Some(true)),
        );
        assert_eq!(
            known.probe_resume_source(&session_id).await,
            ResumeSourceProbe::NotAbsent
        );

        let unguarded = super::MobSessionBridge::new(runtime.handle());
        assert_eq!(
            unguarded.probe_resume_source(&session_id).await,
            ResumeSourceProbe::NotAbsent
        );
    }

    /// A provider-auth build refusal is read TYPED from meerkat's carriers
    /// (in-process `SessionError` structured data, placed/remote
    /// `MaterializeBuildRejected`), never from display text, and every spawn
    /// door (create, resume, reset successor) surfaces it as
    /// `BridgeError::ProviderAuthRejected` with the kind intact.
    #[test]
    fn provider_auth_build_refusal_is_typed_on_every_spawn_door()
    -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_contracts::wire::supervisor_bridge::{
            BridgeRejectionCause, MemberBuildRejection,
        };
        use meerkat_core::service::{SessionError, SessionProviderAuthFailure};

        let failure = SessionProviderAuthFailure {
            kind: meerkat_core::AuthErrorKind::MissingSecret,
            provider: meerkat_core::Provider::Anthropic,
            realm_id: None,
            binding_id: None,
        };
        let in_process = meerkat_mob::MobError::SessionError(SessionError::provider_auth_failure(
            failure.clone(),
        ));
        assert_eq!(provider_auth_rejection(&in_process), Some(failure.clone()));

        let remote = meerkat_mob::MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::MaterializeBuildRejected {
                cause: MemberBuildRejection::from_provider_auth_failure(&failure),
            },
            reason: "materialize rejected".to_string(),
        };
        assert_eq!(provider_auth_rejection(&remote), Some(failure.clone()));

        // A build failure that is NOT provider auth stays on the untyped path.
        let unresolvable = meerkat_mob::MobError::SessionError(
            SessionError::build_llm_identity_unresolvable("model unknown to registry"),
        );
        assert_eq!(provider_auth_rejection(&unresolvable), None);
        assert!(matches!(
            spawn_bridge_error("create_session", &unresolvable),
            BridgeError::Mob(_)
        ));

        let BridgeError::ProviderAuthRejected {
            failure: created,
            detail,
        } = spawn_bridge_error("create_session", &in_process)
        else {
            return Err("the create door must surface the typed refusal".into());
        };
        assert_eq!(created, failure);
        assert!(detail.starts_with("create_session: "), "{detail}");
        assert!(detail.contains("kind=missing_secret"), "{detail}");

        let identity = AgentIdentity::parse("domain:keyless")?;
        let session_id = meerkat_core::types::SessionId::new();
        assert!(matches!(
            resume_rejected(&identity, &session_id, &in_process, "resume"),
            BridgeError::ProviderAuthRejected { .. }
        ));
        // A joined observer receives the same failure through meerkat-mob's
        // transparent shared wrappers (every non-pending resume failure a
        // joined caller sees is wrapped this way); the typed refusal must
        // survive them on both the classifier and the resume door.
        let shared_lifecycle = meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(
            meerkat_mob::MobError::SessionError(SessionError::provider_auth_failure(
                failure.clone(),
            )),
        ));
        assert_eq!(
            provider_auth_rejection(&shared_lifecycle),
            Some(failure.clone())
        );
        assert!(matches!(
            resume_rejected(&identity, &session_id, &shared_lifecycle, "resume"),
            BridgeError::ProviderAuthRejected { .. }
        ));
        let shared_retirement = meerkat_mob::MobError::SharedRetirementFailure(Arc::new(
            meerkat_mob::MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::MaterializeBuildRejected {
                    cause: MemberBuildRejection::from_provider_auth_failure(&failure),
                },
                reason: "materialize rejected".to_string(),
            },
        ));
        assert_eq!(
            provider_auth_rejection(&shared_retirement),
            Some(failure.clone())
        );
        // A wrapped failure that is not provider auth stays untyped.
        let shared_other = meerkat_mob::MobError::SharedLifecycleFailure(Arc::new(
            meerkat_mob::MobError::SessionError(SessionError::build_llm_identity_unresolvable(
                "model unknown to registry",
            )),
        ));
        assert_eq!(provider_auth_rejection(&shared_other), None);

        // The admission seam must not flatten the refusal back into text: the
        // round trip through `BridgeAdmissionError` keeps the typed failure.
        let refused = BridgeError::ProviderAuthRejected {
            failure: failure.clone(),
            detail: "resume: refused".to_string(),
        };
        let admission = BridgeAdmissionError::from(refused);
        let BridgeAdmissionError::ProviderAuthRejected {
            failure: admitted,
            detail: admitted_detail,
        } = &admission
        else {
            return Err("the admission seam must carry the typed refusal".into());
        };
        assert_eq!(admitted, &failure);
        assert_eq!(admitted_detail, "resume: refused");
        let BridgeError::ProviderAuthRejected {
            failure: round_tripped,
            ..
        } = BridgeError::from(admission)
        else {
            return Err("the admission seam must round-trip the typed refusal".into());
        };
        assert_eq!(round_tripped, failure);
        // An ordinary resume failure still classifies as a resume rejection.
        let restore_failed = meerkat_mob::MobError::MemberRestoreFailed {
            hold: None,
            member_id: meerkat_mob::ids::AgentIdentity::from("agent-alpha"),
            session_id: None,
            reason: "durable snapshot missing".to_string(),
        };
        assert!(matches!(
            resume_rejected(&identity, &session_id, &restore_failed, "resume"),
            BridgeError::ResumeRejected {
                kind: ResumeRejectionKind::MemberRestoreFailed,
                ..
            }
        ));

        let respawn = meerkat_mob::MobRespawnError::SpawnAfterRetireWithCause {
            identity: meerkat_mob::ids::AgentIdentity::from("agent-alpha"),
            cause: meerkat_mob::MobError::SessionError(SessionError::provider_auth_failure(
                failure.clone(),
            )),
        };
        let BridgeError::ProviderAuthRejected { failure: reset, .. } =
            respawn_bridge_error("reset", &respawn)
        else {
            return Err("the reset door must surface the typed refusal".into());
        };
        assert_eq!(reset, failure);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Bounded actor admission
    // -----------------------------------------------------------------------

    #[test]
    fn admission_budget_defaults_when_unset_or_unparseable() {
        for raw in [None, Some(""), Some("   "), Some("soon"), Some("-5")] {
            assert_eq!(
                parse_bridge_actor_admission_budget(raw),
                BRIDGE_ACTOR_ADMISSION_BUDGET,
                "unset or unparseable {raw:?} must fall back to the default budget"
            );
        }
    }

    #[test]
    fn admission_budget_honours_configured_value_within_the_clamp() {
        assert_eq!(
            parse_bridge_actor_admission_budget(Some("30")),
            Duration::from_secs(30)
        );
        assert_eq!(
            parse_bridge_actor_admission_budget(Some(" 30 ")),
            Duration::from_secs(30)
        );
        // The floor stops `0` from rejecting every delivery; the ceiling stops
        // a mistyped value from restoring the unbounded hang.
        assert_eq!(
            parse_bridge_actor_admission_budget(Some("0")),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse_bridge_actor_admission_budget(Some("999999")),
            Duration::from_hours(1)
        );
    }

    fn inspect_observation_contention() -> meerkat_mob::MobError {
        meerkat_mob::MobError::LifecycleOperationAdmissionPending {
            intent: "member_status_observation".to_string(),
            stage: "observation_lane_saturated",
        }
    }

    #[tokio::test(start_paused = true)]
    async fn inspect_observation_waits_for_actual_snapshot_and_preserves_other_errors() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_secs(1));
        let member = MobAgentIdentity::from("original-member");
        let mut calls = 0;
        let actual = deadline
            .observe_member_status(&member, || {
                calls += 1;
                std::future::ready(if calls == 1 {
                    Err(inspect_observation_contention())
                } else {
                    Ok("actual owner snapshot")
                })
            })
            .await
            .expect("read succeeds after typed contention");
        assert_eq!(actual, "actual owner snapshot");
        assert_eq!(calls, 2);
        let mut calls = 0;
        let error = deadline
            .observe_member_status(&member, || {
                calls += 1;
                std::future::ready(Err::<(), _>(meerkat_mob::MobError::Internal(
                    "original owner failure".to_string(),
                )))
            })
            .await
            .expect_err("unrelated owner failure must remain failure");
        assert!(
            matches!(error, BridgeError::Mob(message) if message == "internal error: original owner failure")
        );
        assert_eq!(calls, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn inspect_observation_uses_bridge_budget_and_existing_health_refusal() {
        let start = tokio::time::Instant::now();
        let deadline = ActorAdmissionDeadline::new(Duration::from_millis(51));
        let member = MobAgentIdentity::from("original-member");
        let mut calls = 0;
        let error = deadline
            .observe_member_status(&member, || {
                calls += 1;
                std::future::ready(Err::<(), _>(inspect_observation_contention()))
            })
            .await
            .expect_err("contention cannot extend the bridge deadline");
        assert!(
            matches!(error, BridgeError::ActorAdmissionTimeout { operation: "inspect.member_status", identity, .. } if identity == member)
        );
        assert_eq!(start.elapsed(), Duration::from_millis(51));
        assert!(calls > 1);

        let health = ActorLoopHealth::shared();
        let deadline =
            ActorAdmissionDeadline::with_health(Duration::from_secs(10), Some(health.clone()));
        let mut calls = 0;
        let error = deadline
            .observe_member_status(&member, || {
                calls += 1;
                health.mark_stalled(73);
                std::future::ready(Err::<(), _>(inspect_observation_contention()))
            })
            .await
            .expect_err("opening stall must cancel pending observation retry");
        assert!(matches!(
            error,
            BridgeError::ActorLoopStalled {
                operation: "inspect.member_status",
                stall_id: 73,
                observation: ActorCallObservation::InFlight,
                ..
            }
        ));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn responsive_round_trip_passes_through_the_bound_unchanged() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");

        let value = deadline
            .bound("test.responsive", &member, async { 7_u32 })
            .await
            .expect("a ready round trip must pass through the bound untouched");

        assert_eq!(value, 7);
        // No added latency on the success path: the budget is still intact.
        assert!(
            deadline
                .deadline
                .saturating_duration_since(tokio::time::Instant::now())
                > Duration::from_secs(599)
        );
    }

    #[tokio::test]
    async fn stalled_actor_fails_typed_instead_of_hanging() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_millis(20));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");

        let error = deadline
            .bound(
                "deliver.submit_work",
                &member,
                std::future::pending::<Result<(), meerkat_mob::MobError>>(),
            )
            .await
            .expect_err("a mob actor that never replies must not hang the delivery");

        match error {
            BridgeError::ActorAdmissionTimeout {
                operation,
                identity,
                waited,
                command,
            } => {
                assert_eq!(operation, "deliver.submit_work");
                assert_eq!(identity.as_str(), "rt-agent-alpha-0");
                assert!(waited >= Duration::from_millis(20), "waited {waited:?}");
                assert_eq!(command, None);
            }
            other => panic!("expected a typed admission timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admission_timeout_names_the_operation_and_member_and_is_never_repairable() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_millis(5));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");

        let error = deadline
            .bound(
                "deliver.get_member",
                &member,
                std::future::pending::<Result<(), meerkat_mob::MobError>>(),
            )
            .await
            .expect_err("stalled actor");

        let rendered = error.to_string();
        assert!(rendered.contains("deliver.get_member"), "{rendered}");
        assert!(rendered.contains("rt-agent-alpha-0"), "{rendered}");
        // The delivery path classifies repairable errors by substring; a
        // blocked actor must never be routed into member repair.
        assert!(
            !is_repairable_bridge_delivery_error(&rendered),
            "a blocked actor is not stale runtime state: {rendered}"
        );
    }

    /// The production circuit breaker: while the probe has an OPEN stall, an
    /// admission round trip is refused immediately, typed, naming the
    /// stall, and never waits the admission budget.
    #[tokio::test]
    async fn open_stall_refuses_admission_fast_naming_the_stall() {
        let health = ActorLoopHealth::shared();
        health.mark_stalled(42);
        let deadline =
            ActorAdmissionDeadline::with_health(Duration::from_mins(10), Some(Arc::clone(&health)));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");

        let started = tokio::time::Instant::now();
        let error = deadline
            .bound(
                "deliver.submit_work",
                &member,
                std::future::pending::<Result<(), meerkat_mob::MobError>>(),
            )
            .await
            .expect_err("an open stall must refuse the round trip");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the refusal must not wait the budget: {:?}",
            started.elapsed()
        );
        match &error {
            BridgeError::ActorLoopStalled {
                operation,
                identity,
                stall_id,
                ..
            } => {
                assert_eq!(*operation, "deliver.submit_work");
                assert_eq!(identity.as_str(), "rt-agent-alpha-0");
                assert_eq!(*stall_id, 42, "the refusal must name the open stall");
            }
            other => panic!("expected ActorLoopStalled, got {other:?}"),
        }
        let rendered = error.to_string();
        assert!(rendered.contains("stall_id 42"), "{rendered}");
        assert!(
            rendered.contains("this actor call was not started"),
            "{rendered}"
        );
        assert!(
            !is_repairable_bridge_delivery_error(&rendered),
            "a stalled loop is not stale runtime state: {rendered}"
        );
        assert!(
            !is_reload_required_bridge_delivery_error(&rendered),
            "a stalled loop is not a reload-required member: {rendered}"
        );
        // Typed round trip through the admission-edge enum, never Mob(String).
        assert!(matches!(
            BridgeAdmissionError::from(error),
            BridgeAdmissionError::ActorLoopStalled { stall_id: 42, .. }
        ));
    }

    /// A stall that opens WHILE a round trip is already parked must end that
    /// wait the instant the probe pages, not at the admission budget.
    #[tokio::test]
    async fn stall_opening_mid_wait_cuts_the_wait_short() {
        let health = ActorLoopHealth::shared();
        let deadline =
            ActorAdmissionDeadline::with_health(Duration::from_mins(10), Some(Arc::clone(&health)));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");
        let pager = {
            let health = Arc::clone(&health);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                health.mark_stalled(7);
            })
        };

        let started = tokio::time::Instant::now();
        let error = deadline
            .bound(
                "deliver.get_member",
                &member,
                std::future::pending::<Result<(), meerkat_mob::MobError>>(),
            )
            .await
            .expect_err("the stall must cut the parked wait");
        pager.await.expect("pager task");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait must end when the stall opens: {:?}",
            started.elapsed()
        );
        assert!(
            matches!(
                error,
                BridgeError::ActorLoopStalled {
                    stall_id: 7,
                    observation: ActorCallObservation::InFlight,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("execution fate is not established")
        );
        assert!(!error.to_string().contains("nothing was queued"));
    }

    /// The correlated resolution reopens the delivery path: after
    /// `mark_recovered` a responsive round trip passes straight through.
    #[tokio::test]
    async fn recovered_loop_admits_again() {
        let health = ActorLoopHealth::shared();
        health.mark_stalled(3);
        health.mark_recovered(3);
        let deadline =
            ActorAdmissionDeadline::with_health(Duration::from_mins(10), Some(Arc::clone(&health)));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");
        let value = deadline
            .bound("deliver.submit_work", &member, std::future::ready(11))
            .await
            .expect("a live loop must admit");
        assert_eq!(value, 11);
    }

    /// A terminated loop is a DIFFERENT refusal from a stall: the operator
    /// must not wait for a recovery that cannot come.
    #[tokio::test]
    async fn terminated_loop_refuses_with_a_distinct_error() {
        let health = ActorLoopHealth::shared();
        health.mark_terminated(Some(1), "mob actor command channel closed");
        let deadline =
            ActorAdmissionDeadline::with_health(Duration::from_mins(10), Some(Arc::clone(&health)));
        let member = MobAgentIdentity::from("rt-agent-alpha-0");
        let error = deadline
            .bound(
                "deliver.submit_work",
                &member,
                std::future::pending::<Result<(), meerkat_mob::MobError>>(),
            )
            .await
            .expect_err("a terminated loop must refuse");
        match &error {
            BridgeError::ActorTerminated { detail, .. } => {
                assert!(detail.contains("command channel closed"), "{detail}");
            }
            other => panic!("expected ActorTerminated, got {other:?}"),
        }
        assert!(error.to_string().contains("process must restart"));
        assert!(!is_repairable_bridge_delivery_error(&error.to_string()));
    }

    /// The repair-blocked refusal meerkat 0.8.33 renders for a
    /// durability-degraded member classifies TYPED at the bridge boundary,
    /// never as `Mob(String)`, and is never routed into member repair.
    #[test]
    fn repair_blocked_text_classifies_as_typed_reload_required() {
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        // Exactly the 0.8.33 wording: `RuntimeDriverError::RecoveryRepairBlocked`
        // as composed by `mark_durability_reload_required`.
        let rendered = "Runtime recovery is repair-blocked: durable state may differ from the \
                        live runtime after `completed_boundary_commit`; registration-authorized \
                        cold reload is required: continuity save: HTTP 0";
        assert!(is_reload_required_bridge_delivery_error(rendered));
        assert!(
            !is_repairable_bridge_delivery_error(rendered),
            "repair-blocked must never route to repair_member_for_delivery"
        );
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let classified = classify_submit_mob_error(
            &member,
            meerkat_mob::MobError::Internal(rendered.to_string()),
            &deadline,
        );
        match &classified {
            BridgeError::ReloadRequired { identity, reason } => {
                assert_eq!(identity.as_str(), "rt-review-singleton-0");
                assert!(reason.contains("cold reload is required"), "{reason}");
            }

            other => panic!("expected ReloadRequired, got {other:?}"),
        }
        assert!(
            classified.to_string().contains("meta/reload_member"),
            "the operator path must be named: {classified}"
        );
        assert!(matches!(
            BridgeAdmissionError::from(classified),
            BridgeAdmissionError::ReloadRequired { .. }
        ));
        // The bare variant token also classifies (a future wrapper that prints
        // the Debug name instead of the Display text).
        assert!(is_reload_required_bridge_delivery_error(
            "admission refused: RecoveryRepairBlocked { .. }"
        ));
        // Anything else keeps the historical text form the substring
        // classifier reads.
        let other = classify_submit_mob_error(
            &member,
            meerkat_mob::MobError::Internal("missing event injector capability for member".into()),
            &deadline,
        );
        assert!(matches!(other, BridgeError::Mob(_)), "{other:?}");
        assert!(is_repairable_bridge_delivery_error(&other.to_string()));
    }

    #[test]
    fn maintenance_floor_is_single_build_scoped_and_old_owner_cannot_clear_replacement() {
        let floors = CompactionFloorRegistry::default();
        let identity = AgentIdentity::parse("worker:main").expect("identity");
        let low = std::num::NonZeroU64::new(256).expect("floor");
        let high = std::num::NonZeroU64::new(512).expect("replacement floor");
        floors.set_owned(&identity, "old-operation", low);
        assert_eq!(floors.for_build(&identity), Some(low));
        floors.finish_owned_build(&identity, "old-operation");
        assert_eq!(
            floors.get(&identity),
            Some(low),
            "pending cleanup remains observable"
        );
        assert_eq!(
            floors.for_build(&identity),
            None,
            "later incarnations cannot inherit the override"
        );
        floors.set_owned(&identity, "new-operation", high);
        floors.clear_owned(&identity, "old-operation");
        assert_eq!(floors.for_build(&identity), Some(high));
        floors.clear_owned(&identity, "new-operation");
        assert_eq!(floors.get(&identity), None);
    }

    /// meerkat 0.8.34 types the durability-degraded refusal; it must classify
    /// from the VARIANT (ahead of the text fallback) and carry meerkat's
    /// member id and reason verbatim.
    #[test]
    fn typed_member_reload_required_classifies_ahead_of_text() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let classified = classify_submit_mob_error(
            &member,
            meerkat_mob::MobError::MemberReloadRequired {
                member_id: MobAgentIdentity::from("rt-review-singleton-0"),
                reason: "durable commit could not be reconciled".to_string(),
            },
            &deadline,
        );
        match classified {
            BridgeError::ReloadRequired { identity, reason } => {
                assert_eq!(identity.as_str(), "rt-review-singleton-0");
                assert_eq!(reason, "durable commit could not be reconciled");
            }
            other => panic!("expected ReloadRequired, got {other:?}"),
        }
    }

    /// meerkat #1248: a member whose session lost its runtime attachment is
    /// refused typed. It classifies from the variant into the repairable
    /// class (resume repair onto the same durable session, queued inputs
    /// carried), whatever the rendering says.
    #[tokio::test]
    async fn typed_member_runtime_detached_is_repairable_from_the_variant() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let session_id = meerkat_core::types::SessionId::new();
        let classified = classify_submit_mob_error(
            &member,
            meerkat_mob::MobError::MemberRuntimeDetached {
                session_id: session_id.clone(),
                detachment: meerkat_mob::error::MemberRuntimeDetachment::UnregisterFailed,
            },
            &deadline,
        );
        match &classified {
            BridgeError::MemberRuntimeDetached {
                session_id: classified_session,
                ..
            } => assert_eq!(classified_session, &session_id),
            other => panic!("expected MemberRuntimeDetached, got {other:?}"),
        }
        assert!(is_repairable_bridge_delivery(&classified));
        assert!(
            !is_repairable_bridge_delivery_error(&classified.to_string()),
            "the class comes from the variant, not the text"
        );
        assert!(!is_reload_required_bridge_delivery_error(
            &classified.to_string()
        ));
    }

    #[test]
    fn actor_deadline_stages_survive_admission_round_trip_without_fate_claims() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_secs(1));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        for stage in ["actor_command_admission", "actor_command_reply"] {
            let upstream = meerkat_mob::MobError::ActorCommandTimedOut {
                command_kind: "SubmitWork",
                stage,
            };
            let expected = upstream.structured_data().expect("upstream timeout data");
            let error = classify_submit_mob_error(&member, upstream, &deadline);
            let admission = BridgeAdmissionError::from(error);
            for display in [
                admission.to_string(),
                BridgeError::from(admission).to_string(),
            ] {
                assert!(display.contains("SubmitWork") && display.contains(stage));
                assert!(display.contains("execution fate"));
                assert!(!display.contains("NOT executed") && !display.contains("never started"));
                assert!(!is_repairable_bridge_delivery_error(&display));
                assert!(!is_reload_required_bridge_delivery_error(&display));
            }
            let data = actor_timeout_data(
                "deliver.submit_work",
                Duration::ZERO,
                Some(ActorCommandTimeout {
                    command_kind: "SubmitWork",
                    stage,
                }),
            );
            assert_eq!(data, expected);
            assert!(data.get("executed").is_none());
            assert!(data.get("retryable").is_none());
        }
    }

    /// meerkat 0.8.34's reload refusal (store not healthy yet) is typed and
    /// tells the operator to retry later; the timeout names its stage and is
    /// not retryable without inspection. Neither is ever `Mob(String)`.
    #[test]
    fn reload_refused_and_timed_out_classify_typed() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_secs(1));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let session_id = meerkat_core::types::SessionId::new();
        let refused = classify_reload_mob_error(
            "reload.reload_member_registration",
            &member,
            meerkat_mob::MobError::MemberReloadRefused {
                session_id: session_id.clone(),
                reason: "durable resume authority unreadable: continuity save: HTTP 0".into(),
            },
            &deadline,
        );
        match &refused {
            BridgeError::ReloadRefused {
                identity,
                session_id: refused_session,
                reason,
            } => {
                assert_eq!(identity.as_str(), "rt-review-singleton-0");
                assert_eq!(refused_session, &session_id.to_string());
                assert!(reason.contains("HTTP 0"), "{reason}");
            }
            other => panic!("expected ReloadRefused, got {other:?}"),
        }
        let rendered = refused.to_string();
        assert!(
            rendered.contains("store is not healthy") && rendered.contains("retry later"),
            "{rendered}"
        );
        assert!(!is_repairable_bridge_delivery_error(&rendered));
        assert!(matches!(
            BridgeAdmissionError::from(refused),
            BridgeAdmissionError::ReloadRefused { .. }
        ));

        let timed_out = classify_reload_mob_error(
            "reload.reload_member_registration",
            &member,
            meerkat_mob::MobError::MemberReloadTimedOut {
                session_id,
                stage: "live_session_revival",
            },
            &deadline,
        );
        match &timed_out {
            BridgeError::ReloadTimedOut { stage, .. } => {
                assert_eq!(stage, "live_session_revival");
            }
            other => panic!("expected ReloadTimedOut, got {other:?}"),
        }
        let rendered = timed_out.to_string();
        assert!(
            rendered.contains("live_session_revival") && rendered.contains("inspect"),
            "{rendered}"
        );
        assert!(matches!(
            BridgeError::from(BridgeAdmissionError::from(timed_out)),
            BridgeError::ReloadTimedOut { .. }
        ));
        for stage in ["actor_command_admission", "actor_command_reply"] {
            let error = classify_reload_mob_error(
                "reload.reload_member_registration",
                &member,
                meerkat_mob::MobError::ActorCommandTimedOut {
                    command_kind: "ReloadMemberRegistration",
                    stage,
                },
                &deadline,
            );
            assert!(matches!(error, BridgeError::ActorAdmissionTimeout {
                operation: "reload.reload_member_registration",
                command: Some(ActorCommandTimeout { command_kind: "ReloadMemberRegistration", stage: actual }),
                ..
            } if actual == stage));
        }
        // The same timeout reached from `ensure_member_live` names that verb.
        let ensured = classify_reload_mob_error(
            "ensure_member_live",
            &member,
            meerkat_mob::MobError::ActorCommandTimedOut {
                command_kind: "EnsureMemberLive",
                stage: "actor_command_reply",
            },
            &deadline,
        );
        assert!(matches!(
            ensured,
            BridgeError::ActorAdmissionTimeout {
                operation: "ensure_member_live",
                ..
            }
        ));
    }

    /// meerkat 0.8.34's per-member lane backpressure is typed and RETRYABLE:
    /// never repair, never reload, never `Mob(String)`.
    #[test]
    fn member_admission_backlog_full_classifies_typed_and_retryable() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let classified = classify_submit_mob_error(
            &member,
            meerkat_mob::MobError::MemberAdmissionBacklogFull {
                member_id: MobAgentIdentity::from("rt-review-singleton-0"),
                depth: 256,
            },
            &deadline,
        );
        match &classified {
            BridgeError::AdmissionBacklogFull { identity, depth } => {
                assert_eq!(identity.as_str(), "rt-review-singleton-0");
                assert_eq!(*depth, 256);
            }
            other => panic!("expected AdmissionBacklogFull, got {other:?}"),
        }
        let rendered = classified.to_string();
        assert!(
            rendered.contains("retry once the lane drains"),
            "{rendered}"
        );
        assert!(
            !is_repairable_bridge_delivery_error(&rendered),
            "{rendered}"
        );
        assert!(
            !is_reload_required_bridge_delivery_error(&rendered),
            "{rendered}"
        );
        let admission = BridgeAdmissionError::from(classified);
        assert!(matches!(
            admission,
            BridgeAdmissionError::AdmissionBacklogFull { depth: 256, .. }
        ));
        assert!(matches!(
            BridgeError::from(admission),
            BridgeError::AdmissionBacklogFull { depth: 256, .. }
        ));
    }

    /// A closed mob (for every close reason) and a mob whose Shutdown is
    /// still draining refuse the submit before admission. Both are typed,
    /// keep meerkat's rendering exactly as the untyped mob error showed it,
    /// are never read by the text repair classifier, and survive the
    /// admission-error round trip.
    #[test]
    fn closed_and_shutting_down_mob_refusals_classify_typed_and_never_repair() {
        use meerkat_mob::MobAdmissionCloseReason as Reason;
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let mob_id = meerkat_mob::MobId::from("closing-mob");
        for reason in [
            Reason::IdleClose,
            Reason::LastOwnerReleased,
            Reason::StandDown,
            Reason::Shutdown,
            Reason::Destroyed,
            Reason::DurabilityFailStop,
            Reason::ActorLost,
            Reason::DispatchFailed,
        ] {
            let error = meerkat_mob::MobError::AdmissionClosed {
                mob_id: mob_id.clone(),
                reason,
            };
            let rendered = error.to_string();
            let classified = classify_submit_mob_error(&member, error, &deadline);
            match &classified {
                BridgeError::MobAdmissionClosed {
                    identity,
                    mob_id: closed,
                    reason: why,
                    detail,
                } => {
                    assert_eq!(identity, &member);
                    assert_eq!(closed, &mob_id);
                    assert_eq!(*why, reason);
                    assert_eq!(detail, &rendered);
                }
                other => panic!("expected MobAdmissionClosed, got {other:?}"),
            }
            assert_eq!(
                classified.to_string(),
                BridgeError::Mob(rendered.clone()).to_string()
            );
            assert!(
                !is_repairable_bridge_delivery(&classified),
                "{classified:?}"
            );
            let admission = BridgeAdmissionError::from(classified);
            assert!(
                matches!(
                    &admission,
                    BridgeAdmissionError::MobAdmissionClosed { reason: why, .. } if *why == reason
                ),
                "{admission:?}"
            );
            assert!(matches!(
                BridgeError::from(admission),
                BridgeError::MobAdmissionClosed { reason: why, .. } if why == reason
            ));
        }

        let error = meerkat_mob::MobError::ShutdownInProgress {
            mob_id: mob_id.clone(),
        };
        let rendered = error.to_string();
        let classified = classify_submit_mob_error(&member, error, &deadline);
        assert!(
            matches!(
                &classified,
                BridgeError::MobShutdownInProgress { identity, mob_id: draining, detail }
                    if identity == &member && draining == &mob_id && detail == &rendered
            ),
            "{classified:?}"
        );
        assert_eq!(
            classified.to_string(),
            BridgeError::Mob(rendered).to_string()
        );
        assert!(
            !is_repairable_bridge_delivery(&classified),
            "{classified:?}"
        );
        let admission = BridgeAdmissionError::from(classified);
        assert!(
            matches!(
                admission,
                BridgeAdmissionError::MobShutdownInProgress { .. }
            ),
            "{admission:?}"
        );
        assert!(matches!(
            BridgeError::from(admission),
            BridgeError::MobShutdownInProgress { .. }
        ));
    }

    /// A member closed while its identity converges refuses the submit before
    /// admission. The refusal is typed with the member meerkat named, keeps
    /// meerkat's rendering exactly as the untyped mob error showed it, is
    /// never read by the text repair classifier, and survives the
    /// admission-error round trip.
    #[test]
    fn identity_convergence_refusal_classifies_typed_and_never_repairs() {
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(10));
        let member = MobAgentIdentity::from("rt-review-singleton-0");
        let error = meerkat_mob::MobError::IdentityConvergenceAdmissionClosed {
            member_id: member.clone(),
        };
        let rendered = error.to_string();
        let classified = classify_submit_mob_error(&member, error, &deadline);
        assert!(
            matches!(
                &classified,
                BridgeError::IdentityConvergenceAdmissionClosed { identity, detail }
                    if identity == &member && detail == &rendered
            ),
            "{classified:?}"
        );
        assert_eq!(
            classified.to_string(),
            BridgeError::Mob(rendered.clone()).to_string()
        );
        assert!(
            !is_repairable_bridge_delivery(&classified),
            "{classified:?}"
        );
        let admission = BridgeAdmissionError::from(classified);
        assert!(
            matches!(
                &admission,
                BridgeAdmissionError::IdentityConvergenceAdmissionClosed { identity, detail }
                    if identity == &member && detail == &rendered
            ),
            "{admission:?}"
        );
        assert!(matches!(
            BridgeError::from(admission),
            BridgeError::IdentityConvergenceAdmissionClosed { identity, detail }
                if identity == member && detail == rendered
        ));
    }

    /// The mob primitive's outcome and the machine's durability verdict
    /// project into the identity-first wire vocabulary without loss.
    #[test]
    fn mob_reload_outcome_and_durability_project_into_wire_vocabulary() {
        let session_id = meerkat_core::types::SessionId::new();
        // `MemberReloadOutcome` is `#[non_exhaustive]` upstream, so a test
        // builds it through its serde form rather than a struct literal.
        let outcome: meerkat_mob::MemberReloadOutcome = serde_json::from_value(serde_json::json!({
            "disposition": "not_degraded",
            "session_id": session_id,
            "generation": 0,
        }))
        .expect("MemberReloadOutcome wire form");
        let reload = bridge_member_reload(outcome);
        assert_eq!(
            reload.disposition,
            crate::identity_first::MemberReloadDisposition::NotDegraded
        );
        assert_eq!(reload.session_id, session_id);

        assert_eq!(member_durability_from_machine(None), None);
        let degraded = member_durability_degraded(
            "completed_boundary_commit".to_string(),
            "continuity save: HTTP 0".to_string(),
        );
        assert_eq!(
            serde_json::to_value(&degraded).expect("serialize"),
            serde_json::json!({
                "reload_required": {
                    "operation": "completed_boundary_commit",
                    "reason": "continuity save: HTTP 0"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(crate::identity_first::MemberDurability::Ready).expect("ready"),
            serde_json::json!("ready")
        );
    }

    #[tokio::test]
    async fn serialized_round_trips_share_one_budget() {
        // The delivery path makes three or four serialized round trips; a
        // per-call timeout would cost a full budget each and make the
        // contained worst case worse than the hang. Once the shared deadline
        // is spent, every later hop must fail immediately.
        let budget = Duration::from_millis(50);
        let deadline = ActorAdmissionDeadline::new(budget);
        let member = MobAgentIdentity::from("rt-agent-alpha-0");

        let mut outcomes = Vec::new();
        // First hop consumes the whole budget.
        outcomes.push(
            deadline
                .bound(
                    "test.hop",
                    &member,
                    std::future::pending::<Result<(), meerkat_mob::MobError>>(),
                )
                .await,
        );

        let after_first = tokio::time::Instant::now();
        for _ in 0..2 {
            outcomes.push(
                deadline
                    .bound(
                        "test.hop",
                        &member,
                        std::future::pending::<Result<(), meerkat_mob::MobError>>(),
                    )
                    .await,
            );
        }
        let spent_after_first = after_first.elapsed();

        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Err(BridgeError::ActorAdmissionTimeout { .. }))),
            "every hop against a dead actor must fail typed"
        );
        assert!(
            spent_after_first < budget,
            "the deadline must not be re-armed per hop: two further hops spent \
             {spent_after_first:?} against a {budget:?} budget"
        );
    }

    /// Heal contract: `DurableTailRecoveryRefused` is a typed refusal only an
    /// external change clears — it must PARK as the terminal `Unprovable`
    /// verdict, not escape as a retryable bridge error that loops the
    /// reconcile repair pass forever (the downstream app's 9/17-Broken shape would
    /// then never reach an operator as a parked reason).
    #[test]
    fn heal_error_tier_recovery_refused_parks_terminal_unprovable() {
        let id = meerkat_core::SessionId::new();
        let verdict = map_committed_boundary_recovery_error(
            meerkat_core::SessionError::DurableTailRecoveryRefused { id: id.clone() },
        );
        match verdict {
            Ok(CommittedBoundaryRepair::Unprovable { reason }) => {
                assert!(
                    reason.contains(&id.to_string()),
                    "park reason must carry the session id for the operator: {reason}"
                );
                assert!(
                    reason.contains("refused"),
                    "park reason must state the refusal: {reason}"
                );
            }
            other => panic!("refused must park as Unprovable, got {other:?}"),
        }
    }

    /// Forked/unverifiable durable evidence is the same class: no retry can
    /// un-fork evidence, so it parks with the reason instead of retrying.
    #[test]
    fn heal_error_tier_quarantined_evidence_parks_terminal_unprovable() {
        let verdict = map_committed_boundary_recovery_error(
            meerkat_core::SessionError::DurableEvidenceQuarantined {
                id: meerkat_core::SessionId::new(),
            },
        );
        assert!(
            matches!(verdict, Ok(CommittedBoundaryRepair::Unprovable { .. })),
            "quarantined evidence must park as Unprovable, got {verdict:?}"
        );
    }

    /// Recovery takes this owner's hosting claim before it reads or writes,
    /// so a session another runtime owner hosts refuses it typed. That is a
    /// hold that clears when the other owner releases the session: the heal
    /// seam reports it as its own typed, retryable error naming the session,
    /// never the terminal `Unprovable` verdict and never the generic mob
    /// error text.
    #[test]
    fn heal_error_tier_served_elsewhere_is_a_typed_retryable_hold() {
        let id = meerkat_core::SessionId::new();
        let verdict =
            map_committed_boundary_recovery_error(meerkat_core::SessionError::ServedElsewhere {
                id: id.clone(),
            });
        match verdict {
            Err(BridgeError::ServedElsewhere { session_id }) => assert_eq!(session_id, id),
            other => panic!("served elsewhere must be the typed hosting hold, got {other:?}"),
        }
    }

    /// A hosting claim that cannot be taken clears once it can: the heal seam
    /// reports it as its own typed, retryable error naming the session.
    #[test]
    fn heal_error_tier_hosting_unavailable_is_a_typed_retryable_hold() {
        let id = meerkat_core::SessionId::new();
        let verdict =
            map_committed_boundary_recovery_error(meerkat_core::SessionError::HostingUnavailable {
                id: id.clone(),
            });
        match verdict {
            Err(BridgeError::HostingUnavailable { session_id }) => assert_eq!(session_id, id),
            other => {
                panic!("an unavailable hosting claim must be the typed hosting hold, got {other:?}")
            }
        }
    }

    /// The genuinely retryable tier stays in `Err`: a live session owning the
    /// head mid-turn (`Busy`) and a held tail awaiting the recovery commit
    /// itself (`DurableTailHeldForRecovery`) both clear on a later pass.
    #[test]
    fn heal_error_tier_busy_and_held_stay_retryable_errors() {
        for error in [
            meerkat_core::SessionError::Busy {
                id: meerkat_core::SessionId::new(),
            },
            meerkat_core::SessionError::DurableTailHeldForRecovery {
                id: meerkat_core::SessionId::new(),
            },
        ] {
            let verdict = map_committed_boundary_recovery_error(error);
            assert!(
                matches!(verdict, Err(BridgeError::Mob(_))),
                "retryable-tier errors must stay bridge errors, got {verdict:?}"
            );
        }
    }

    /// The audited-endpoint divergence is the one hold whose exit is the
    /// operator's sanctioned repair (downstream app 2026-09-22): the heal authority
    /// reports it as the typed repair verdict, not as the retryable error tier
    /// (a recovery retry every pass is the retry storm) and not as the
    /// operator-opaque `Unprovable` text.
    #[test]
    fn heal_error_tier_audited_endpoint_divergence_is_the_typed_repair_verdict() {
        let id = meerkat_core::SessionId::new();
        let verdict = map_committed_boundary_recovery_error(
            meerkat_core::SessionError::WholeBlobAuditedEndpointDivergence { id: id.clone() },
        );
        match verdict {
            Ok(CommittedBoundaryRepair::RepairRequired { session_id, detail }) => {
                assert_eq!(session_id, id);
                assert!(
                    detail.contains(&id.to_string()),
                    "the verdict must name the session for the operator: {detail}"
                );
            }
            other => panic!("divergence must be the typed repair verdict, got {other:?}"),
        }
    }

    /// The resume classifier reads meerkat's typed hold, never display text:
    /// the in-process `SessionError` carrier, the same carrier under
    /// meerkat-mob's shared retirement / lifecycle failure wrappers (a joined
    /// resume observer receives it this way), and meerkat's structured
    /// `durable_resume_hold` wire token. Sibling holds keep their own hold.
    #[test]
    fn resume_classifier_reads_the_audited_endpoint_hold_typed() {
        use meerkat_core::service::{DurableResumeHold, SessionError};
        use meerkat_mob::MobError;
        let id = meerkat_core::SessionId::new();
        let divergence = || SessionError::WholeBlobAuditedEndpointDivergence { id: id.clone() };

        assert_eq!(
            classify_resume_error(&MobError::SessionError(divergence())),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::AuditedEndpointDivergence
            }
        );
        assert_eq!(
            classify_resume_error(&MobError::SharedLifecycleFailure(Arc::new(
                MobError::SessionError(divergence())
            ))),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::AuditedEndpointDivergence
            }
        );
        assert_eq!(
            classify_resume_error(&MobError::SharedRetirementFailure(Arc::new(
                MobError::SharedLifecycleFailure(Arc::new(MobError::SessionError(divergence())))
            ))),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::AuditedEndpointDivergence
            }
        );
        assert_eq!(
            durable_resume_hold_of(&MobError::SessionError(divergence())),
            Some(DurableResumeHold::AuditedEndpointDivergence)
        );
        // The wire token is the same authority the meerkat RPC client uses to
        // re-type a hold across the transport.
        let wire = MobError::SessionError(divergence())
            .structured_data()
            .expect("meerkat projects the hold as structured data");
        assert_eq!(
            SessionError::durable_resume_hold_from_data(&wire),
            Some(DurableResumeHold::AuditedEndpointDivergence)
        );

        // meerkat 0.8.41: a Broken member's recorded restore failure carries
        // the hold typed; a restore failure that was not a hold keeps its
        // own class.
        assert_eq!(
            classify_resume_error(&MobError::MemberRestoreFailed {
                member_id: MobAgentIdentity::from("domain:security"),
                session_id: Some(id.clone()),
                reason: "recorded restore failure".to_string(),
                hold: Some(DurableResumeHold::AuditedEndpointDivergence),
            }),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::AuditedEndpointDivergence
            }
        );
        assert_eq!(
            classify_resume_error(&MobError::MemberRestoreFailed {
                member_id: MobAgentIdentity::from("domain:security"),
                session_id: Some(id.clone()),
                reason: "recorded restore failure".to_string(),
                hold: None,
            }),
            ResumeRejectionKind::MemberRestoreFailed
        );

        // Sibling holds are held under their own hold, not the repair one.
        assert_eq!(
            classify_resume_error(&MobError::SessionError(
                SessionError::DurableTailHeldForRecovery { id: id.clone() }
            )),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::TailHeldForRecovery
            }
        );
        assert_eq!(
            classify_resume_error(&MobError::SessionError(
                SessionError::DurableEvidenceQuarantined { id: id.clone() }
            )),
            ResumeRejectionKind::Held {
                hold: DurableResumeHold::EvidenceQuarantined
            }
        );
        assert_eq!(
            classify_resume_error(&MobError::SessionError(SessionError::Busy {
                id: id.clone()
            })),
            ResumeRejectionKind::Other
        );
    }

    /// The registration reload path reports the same typed fact as the
    /// resume path: a divergence is `SessionRepairRequired` naming the
    /// session, never the retryable "store not healthy yet" refusal.
    #[test]
    fn reload_classifier_reads_the_audited_endpoint_hold_typed() {
        use meerkat_core::service::SessionError;
        use meerkat_mob::MobError;
        let id = meerkat_core::SessionId::new();
        let member = MobAgentIdentity::from("domain:security");
        let deadline = ActorAdmissionDeadline::new(Duration::from_mins(1));
        let error = classify_reload_mob_error(
            "reload.reload_member_registration",
            &member,
            MobError::SharedLifecycleFailure(Arc::new(MobError::SessionError(
                SessionError::WholeBlobAuditedEndpointDivergence { id: id.clone() },
            ))),
            &deadline,
        );
        match error {
            BridgeError::SessionRepairRequired {
                identity,
                session_id,
                detail,
            } => {
                assert_eq!(identity, member);
                assert_eq!(session_id, id);
                assert!(detail.contains("audited endpoint"), "{detail}");
            }
            other => panic!("divergence on reload must park typed, got {other:?}"),
        }
        // A plain refusal keeps its retryable class.
        let refused = classify_reload_mob_error(
            "reload.reload_member_registration",
            &member,
            MobError::MemberReloadRefused {
                session_id: id,
                reason: "store still opening".to_string(),
            },
            &deadline,
        );
        assert!(
            matches!(refused, BridgeError::ReloadRefused { .. }),
            "{refused:?}"
        );
    }

    /// The occupant classifier is the one read that separates "stale" from
    /// "still building" before the destructive collision path. Each arm is
    /// pinned individually: a roster row is committed, an empty roster under a
    /// collision is an in-flight spawn, and a failed read is neither.
    #[test]
    fn colliding_occupant_classifier_separates_committed_in_flight_and_unreadable() {
        assert!(matches!(
            super::classify_colliding_occupant(Ok(Some(()))),
            super::CollidingOccupant::Committed
        ));
        assert!(matches!(
            super::classify_colliding_occupant::<()>(Ok(None)),
            super::CollidingOccupant::InFlight
        ));
        match super::classify_colliding_occupant::<()>(Err(meerkat_mob::MobError::Internal(
            "roster read failed".to_string(),
        ))) {
            super::CollidingOccupant::Unobservable(error) => {
                assert!(error.to_string().contains("roster read failed"));
            }
            other => panic!("a failed roster read must stay unobservable, got {other:?}"),
        }
    }
    /// A refused runtime acquisition is not an absent owner: the queue may
    /// hold member inputs, so repair must defer its disposal instead of
    /// capturing nothing and destroying them.
    #[tokio::test]
    async fn a_refused_runtime_acquisition_defers_repair_disposal() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let definition =
            meerkat_mob::MobDefinition::from_toml(
                "[mob]\nid = \"refused-acquisition-repair\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            )
                .expect("mob definition");
        let spec = crate::MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec");
        let runtime = crate::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime");
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let session_id = meerkat_core::types::SessionId::new();

        let refusing = super::MobSessionBridge::with_session_service(
            runtime.handle(),
            crate::mob_handle_runtime::acquisition_observation_probe(true, Arc::clone(&reads)),
        );
        assert!(
            refusing
                .capture_pending_member_ingress(&session_id)
                .await
                .is_err(),
            "a refused acquisition must defer disposal, not yield an empty capture"
        );

        // An owner that is genuinely absent still has nothing to carry.
        let absent = super::MobSessionBridge::with_session_service(
            runtime.handle(),
            crate::mob_handle_runtime::acquisition_observation_probe(false, reads),
        );
        assert!(
            absent
                .capture_pending_member_ingress(&session_id)
                .await
                .expect("an absent owner is not an error")
                .is_empty()
        );
    }

    /// The activation's declarations are read through the trait: the exact
    /// identity gets its declared predecessor role, any other identity none.
    #[tokio::test]
    async fn declared_role_migration_reads_the_activation_declarations() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"declared-role-migration\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("mob definition");
        let spec = crate::MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec");
        let runtime = crate::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime");
        let declared = AgentIdentity::parse("agent:declared").expect("identity");
        let bridge = super::MobSessionBridge::new(runtime.handle())
            .with_role_migration_declarations([RoleMigrationDeclaration {
                identity: declared.clone(),
                from_role: meerkat_mob::ProfileName::from("old-role"),
            }]);
        assert_eq!(
            SessionBridge::declared_role_migration(&bridge, &declared),
            Some(meerkat_mob::ProfileName::from("old-role"))
        );
        let other = AgentIdentity::parse("agent:other").expect("identity");
        assert_eq!(
            SessionBridge::declared_role_migration(&bridge, &other),
            None
        );
    }
}

#[cfg(test)]
mod scoped_classification_tests {
    use super::{
        ScopeMismatch, ScopedDeliveryError, ScopedRetireError, classify_scoped_retire_mob_error,
        classify_scoped_submit_mob_error,
    };

    /// A scope captured at another incarnation of the mob (one destroyed
    /// and created again under its id), version-2 or version-1, is a stale
    /// member binding for both the scoped submit and the scoped retire:
    /// final for the scope, never an uncertain delivery to recover and
    /// never the retirement's own failure.
    #[test]
    fn a_scope_from_another_mob_incarnation_is_a_stale_scope() {
        let mismatches = || {
            [Some(meerkat_core::service::MobIncarnationId::mint()), None]
                .into_iter()
                .map(|expected| meerkat_mob::MobError::IncarnationMismatch {
                    mob_id: meerkat_mob::MobId::from("recreated-mob"),
                    expected,
                    current: Some(meerkat_core::service::MobIncarnationId::mint()),
                })
        };
        for error in mismatches() {
            let submit = classify_scoped_submit_mob_error(error);
            assert!(
                matches!(
                    &submit,
                    ScopedDeliveryError::StaleScope {
                        mismatch: ScopeMismatch::MemberBinding,
                        ..
                    }
                ),
                "{submit:?}"
            );
            assert!(!submit.admission_possible(), "{submit:?}");
        }
        for error in mismatches() {
            let retire = classify_scoped_retire_mob_error(error);
            assert!(
                matches!(
                    &retire,
                    ScopedRetireError::StaleScope {
                        mismatch: ScopeMismatch::MemberBinding,
                        ..
                    }
                ),
                "{retire:?}"
            );
        }
    }

    /// A member that lost (or never had) a native session binding is
    /// Unsupported for scope-bound delivery, never a stale scope a caller
    /// could "re-capture", and never a fallback to the unscoped submit. A
    /// session that moved to another session is a stale member binding.
    #[test]
    fn an_unbound_member_is_unsupported_and_a_moved_session_is_stale() {
        let identity = meerkat_mob::ids::AgentIdentity::from("peer");
        let expected = meerkat_core::types::SessionId::new();
        let unbound = classify_scoped_submit_mob_error(meerkat_mob::MobError::StaleDeliveryScope {
            agent_identity: identity.clone(),
            expected_session: expected.clone(),
            actual_session: None,
        });
        assert!(
            matches!(&unbound, ScopedDeliveryError::Unsupported { detail }
                if detail.contains("no native session binding")),
            "{unbound:?}"
        );
        let unavailable =
            classify_scoped_submit_mob_error(meerkat_mob::MobError::DeliveryScopeUnavailable {
                agent_identity: identity.clone(),
                reason: "member has no session binding".to_string(),
            });
        assert!(
            matches!(&unavailable, ScopedDeliveryError::Unsupported { .. }),
            "{unavailable:?}"
        );
        let moved = classify_scoped_submit_mob_error(meerkat_mob::MobError::StaleDeliveryScope {
            agent_identity: identity,
            expected_session: expected,
            actual_session: Some(meerkat_core::types::SessionId::new()),
        });
        assert!(
            matches!(
                &moved,
                ScopedDeliveryError::StaleScope {
                    mismatch: ScopeMismatch::MemberBinding,
                    ..
                }
            ),
            "{moved:?}"
        );
        for refusal in [&unbound, &unavailable, &moved] {
            assert!(!refusal.admission_possible(), "{refusal:?}");
        }
    }

    /// A closed mob admits nothing, whatever closed it, and neither does a
    /// member closed for identity convergence: the refusal is definite, never
    /// an uncertain delivery a caller would have to recover.
    #[test]
    fn a_closed_admission_rejects_a_scoped_delivery() {
        use meerkat_mob::MobAdmissionCloseReason as Reason;
        let mob_id = meerkat_mob::MobId::from("closed-mob");
        let closed = [
            Reason::IdleClose,
            Reason::LastOwnerReleased,
            Reason::StandDown,
            Reason::Shutdown,
            Reason::Destroyed,
            Reason::DurabilityFailStop,
            Reason::ActorLost,
            Reason::DispatchFailed,
        ]
        .into_iter()
        .map(|reason| meerkat_mob::MobError::AdmissionClosed {
            mob_id: mob_id.clone(),
            reason,
        });
        let converging = meerkat_mob::MobError::IdentityConvergenceAdmissionClosed {
            member_id: meerkat_mob::ids::AgentIdentity::from("converging"),
        };
        for error in closed.chain(std::iter::once(converging)) {
            let detail = error.to_string();
            let refusal = classify_scoped_submit_mob_error(error);
            assert!(
                matches!(&refusal, ScopedDeliveryError::Rejected { detail: rejected }
                    if rejected == &detail),
                "{refusal:?}"
            );
            assert!(!refusal.admission_possible(), "{refusal:?}");
        }
    }
}
