//! Identity requests under the host's identity policy: one change policy
//! for every identity, the canonical rendering of a request and its
//! structured presentation.
//!
//! A host lets agents, itself or its console ask for changes of durable
//! identities: to create one, update one or remove one. An identity a
//! request creates is declared with the spec a host's roster declares an
//! identity with (`DurableAgentSpec`), binds its profile directly, and
//! differs from a declared identity in nothing: the requester is recorded,
//! and grants no authority. The policy comes from
//! `runtime_options.identity_creation`, or the Rust form a native host
//! composition returns (`GatewayHostComposition::identity_creation_policy`).
//! This module is the pure policy layer of that capability:
//!
//! - the policy: parsing and validation, refused typed (`PolicyRefusal`),
//!   and which source a launch uses (`resolve_launch_policy`);
//! - requests: normalization of a create, update or remove against the
//!   policy, refused with the closed vocabulary (`IdentityRefusal`), and the
//!   approval plan of the normalized request, whose rules select by the
//!   change, the requester's kind, and the target's profile and authority
//!   labels;
//! - `render_v1`: the one canonical, complete plain-text rendering whose
//!   sha256 is the decision's subject digest, with the host's consequence
//!   lines in their own section, and its canonical parts;
//! - the structured card, parsed from the rendering.
//!
//! The executor over the durable decision owner, the decision ingresses,
//! the host RPC family, preparation delivery and the agent tools build on
//! this module; none of them is here.

mod approval;
mod config;
mod consequences;
mod digest;
mod escape;
mod parts;
mod policy;
mod presentation;
mod refusal;
mod render;
mod request;

#[cfg(test)]
mod tests;

pub use approval::{ApprovalPlan, PlanRequest, PlanTarget, PlannedOutcome, plan_approval};
pub use config::{
    ApprovalConfig, ApprovalKind, ApprovalOutcomeConfig, ApprovalRuleConfig, ApproverConfig,
    AuthorityFieldConfig, ChangeClass, IdentityCreationConfig, MAX_DECISION_TIMEOUT_MS,
    PeerOutcome, PeerRuleConfig, PeersConfig, PreparationMode, RequesterKind, ReservedKeysConfig,
    SelectorConfig, TimeoutsConfig,
};
pub use consequences::{
    CONSEQUENCES_CALLBACK, ConsequenceHook, ConsequenceLine, HookRefusal,
    consequences_callback_params, parse_consequences_callback_result,
};
pub use digest::Sha256Digest;
pub use parts::{CanonicalPart, canonical_parts, part_ranges, shows_every_part};
pub use policy::{
    ApprovalPolicy, ApprovalRule, ApproverRule, AuthorityFieldRule, CORE_BATCH_MAX,
    DEFAULT_PEER_RULE_ID, IdentityCreationPolicy, META_RESERVED_LABEL_KEYS, OutcomeRule,
    PeerPolicy, PeerRule, RuntimeOptionPolicy, Selector, Timeouts, resolve_launch_policy,
};
pub use presentation::{CardBlock, CardEntry, CardError, CardValue, StructuredCard, ValueSegment};
pub use refusal::{
    IdentityRefusal, IdentityRefusalKind, NameTakenBy, PolicyRefusal, PolicyRefusalKind,
    RequestProblem, ValidationRefusal,
};
pub use render::{
    HOST_CONSEQUENCE_MAX_BYTES, HOST_CONSEQUENCES_MAX_BYTES, RENDERER, RENDERING_MAX_BYTES,
    Rendering, labels, render_v1,
};
pub use request::{
    CONSOLE_SURFACE, Consequence, ConsequenceEffect, CreateIdentitySpec, CreatedSpec, EdgeBasis,
    IdentityEntryFacts, IdentityRequest, NormalizedRequest, PeerFacts, PeerVerdict, ProfileChange,
    REQUEST_BODY_MAX_BYTES, RemoveIdentitySpec, RequestAction, RequestContext, RequestEnvelope,
    Requester, RequesterFacts, SourceEvidence, UpdateIdentitySpec, UpdateSetSpec, UpdatedSpec,
    evaluate_peer, normalize_create, normalize_remove, normalize_update,
};
