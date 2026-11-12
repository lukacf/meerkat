//! Identity requests under the host's identity policy: one change policy
//! for every identity, the canonical rendering of a request and its
//! structured presentation.
//!
//! A host lets agents, itself or its console ask for changes of durable
//! identities: to create one, update one, archive or unarchive one, remove
//! one, or adopt one. An identity a request creates is declared with the
//! spec a host's roster declares an identity with (`DurableAgentSpec`),
//! binds its profile directly, and differs from a declared identity in
//! nothing: the requester is recorded, and grants no authority. The policy
//! comes from
//! `runtime_options.identity_creation`, or the Rust form a native host
//! composition returns (`GatewayHostComposition::identity_creation_policy`).
//! This module is the pure policy layer of that capability:
//!
//! - the policy: parsing and validation, refused typed (`PolicyRefusal`),
//!   and which source a launch uses (`resolve_launch_policy`);
//! - requests: normalization of a create, an update, an archive, an
//!   unarchive, a removal or an adoption against the
//!   policy, refused with the closed vocabulary (`IdentityRefusal`), and the
//!   approval plan of the normalized request, whose rules select by the
//!   change, the requester (kind, host surface, agent identity, profile and
//!   authority labels), and the target's identity, profile and authority
//!   labels;
//! - `render_v1`: the one canonical, complete plain-text rendering whose
//!   sha256 is the decision's subject digest, with the host's consequence
//!   lines in their own section, and its canonical parts;
//! - the structured card, parsed from the rendering.
//!
//! On it stands the interface every surface and every part shares:
//!
//! - the family (`family`): one typed request per operation of
//!   `meta/identity/*`, its typed answer, and the trait a surface calls
//!   (`IdentityFamily`);
//! - the typed outcome (`outcome`): `done`, `pending_decision`,
//!   `pending_preparation`, `refused {kind, field?, retryable, detail}`;
//! - the operation id (`operation`): the one function that makes a caller's
//!   request id Core's operation id, namespaced by the requester;
//! - the executor seam (`executor`): commit this normalized request under
//!   this consent, as this requester, in this mob, over Core's change path.
//!
//! The decisions held for a change (`decided`) stand on it: a change whose
//! outcome is `require_approval` is held as a durable decision, decided by
//! a person through the decisions surface, and carried out by the decision
//! kind's executor through the seam.
//!
//! The family's implementation over the host's rules, the host RPC methods
//! and the agent tools build on this module; none of them is here.

/// The wire tokens of a closed vocabulary, listed once with their variants.
/// It gives the type `ALL_TOKENS`, every token in declaration order, and
/// `listed_token`, whose match is exhaustive: a variant that is not listed
/// here with its token fails to compile, so the list cannot fall behind the
/// type.
macro_rules! closed_tokens {
    ($type:ty { $($variant:ident => $token:literal),+ $(,)? }) => {
        impl $type {
            /// Every wire token of the vocabulary, in declaration order.
            #[allow(
                dead_code,
                reason = "read by the rows that compare the SDKs' token files with this list"
            )]
            pub(crate) const ALL_TOKENS: &'static [&'static str] = &[$($token),+];

            /// The value's wire token.
            const fn listed_token(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $token),+
                }
            }
        }
    };
}

mod approval;
mod config;
mod consequences;
mod decided;
mod digest;
mod escape;
mod executor;
mod family;
mod gate;
mod operation;
mod outcome;
mod parts;
mod policy;
mod presentation;
mod refusal;
mod render;
mod request;
mod schedules;

#[cfg(test)]
mod interface_tests;
#[cfg(test)]
mod tests;

pub use approval::{ApprovalPlan, PlanRequest, PlanTarget, PlannedOutcome, plan_approval};
pub use config::{
    ApprovalConfig, ApprovalKind, ApprovalOutcomeConfig, ApprovalRuleConfig, ApproverConfig,
    AuthorityFieldConfig, ChangeClass, IdentityCreationConfig, MAX_DECISION_TIMEOUT_MS,
    PeerOutcome, PeerRuleConfig, PeersConfig, PreparationConfig, PreparationMode, RequesterKind,
    RequesterSelectorConfig, ReservedKeysConfig, SelectorConfig, TimeoutsConfig,
};
pub use consequences::{
    CONSEQUENCES_CALLBACK, ConsequenceHook, ConsequenceLine, HookRefusal, HostConsequences,
    consequences_callback_params, host_consequences_of, parse_consequences_callback_result,
};
pub use decided::{
    HELD_CHANGE_FORMAT, HeldChangeHost, HeldIdentityChange, HeldTarget, IDENTITY_DECISION_KINDS,
    IdentityDecisionNotice, IdentityDecisionNotices, IdentityDecisions, KeyedSendNotices,
    NoRequesterNotices, REQUESTER_TRUST_DOMAIN_PREFIX, SUBJECT_WIDENED_REFUSAL,
};
pub use digest::Sha256Digest;
pub use executor::{
    AGENT_SPECS_NOT_GIVEN, AgentAdmission, AgentSpecs, ChangeCommit, ChangeCommitted,
    ChangeConsent, ChangeRequester, CoreChangeExecutor, ExecutorError, IdentityChangeExecutor,
    InstructionStep, committed_outcome, consent_of_plan, refusal_of_core, refusal_of_error,
};
pub use family::{
    AgentEntered, AgentPage, AgentProvenance, AgentReadiness, AgentRecord, AgentRef, AgentSet,
    AgentSpec, AgentSummary, ChangePlan, ChangesHold, ChangesHoldRequest, ConsentSummary,
    CreateAgents, DirectoryPage, DirectoryPeer, DirectoryProfile, DirectoryVerdict, GetAgent,
    IdentityChangeCall, IdentityChangeSpec, IdentityFamily, IdentityRead, IdentityReadAnswer,
    ListAgents, OperationRef, OperationState, ReadDirectory, UpdateAgent, WithdrawRequest,
    Withdrawn,
};
pub(crate) use gate::IdentityGate;
pub use gate::{
    FIRST_MESSAGE_IN_GOVERNED_REALM, IdentityFamilyOptions, IdentityFamilySlot, IdentityHostHooks,
    PeerVisibility,
};
pub use operation::{OperationId, REQUEST_ID_MAX_BYTES, ToolCallOrigin};
pub use outcome::{
    AgentChangeState, AgentOutcome, AgentSeat, IdentityOutcome, InstructionNextSessionReason,
    InstructionOutcome, InstructionsChanged,
};
pub use parts::{CanonicalPart, canonical_parts, part_ranges, shows_every_part};
pub use policy::{
    ApprovalPolicy, ApprovalRule, ApproverRule, AuthorityFieldRule, CORE_BATCH_MAX,
    DEFAULT_PEER_RULE_ID, IdentityCreationPolicy, META_RESERVED_LABEL_KEYS, OutcomeRule,
    PeerPolicy, PeerRule, PreparationRule, RequesterSelector, RuntimeOptionPolicy, Selector,
    Timeouts, resolve_launch_policy,
};
pub use presentation::{CardBlock, CardEntry, CardError, CardValue, StructuredCard, ValueSegment};
pub use refusal::{
    IdentityRefusal, IdentityRefusalKind, NameTakenBy, NotAdoptableReason, PolicyRefusal,
    PolicyRefusalKind, RequestProblem, ValidationRefusal,
};
pub use render::{
    AgentTools, HOST_CONSEQUENCE_MAX_BYTES, HOST_CONSEQUENCES_MAX_BYTES, ProfileTools,
    REALM_PROFILE_UNREADABLE, RENDERER, RENDERING_MAX_BYTES, Rendering, ToolConfigurations, labels,
    render_held_v1, render_v1,
};
pub use request::{
    AdoptIdentitySpec, ArchiveIdentitySpec, CONSOLE_SURFACE, Consequence, ConsequenceEffect,
    CreateIdentitySpec, CreatedSpec, EdgeBasis, IdentityEntryFacts, IdentityRequest,
    NormalizedRequest, PeerFacts, PeerVerdict, ProfileChange, REQUEST_BODY_MAX_BYTES,
    RemoveIdentitySpec, RequestAction, RequestContext, RequestEnvelope, Requester, RequesterFacts,
    SourceEvidence, UnarchiveIdentitySpec, UpdateIdentitySpec, UpdateSetSpec, UpdatedSpec,
    core_member_name, evaluate_peer, identity_name_of_member, normalize_adopt, normalize_archive,
    normalize_create, normalize_remove, normalize_unarchive, normalize_update,
};
pub use schedules::{AgentSchedules, ScheduleEffect, ScheduleRef, SchedulesFollowed};
