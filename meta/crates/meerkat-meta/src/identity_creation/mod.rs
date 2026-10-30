//! Created identities: the host's creation policy, its templates, the
//! canonical rendering of a request and its structured presentation.
//!
//! A host lets agents, itself or its console create durable identities from
//! templates it declares, through `runtime_options.identity_creation` or
//! the Rust form a native host composition returns
//! (`GatewayHostComposition::identity_creation_policy`). This module is the
//! pure policy layer of that capability:
//!
//! - the policy: parsing and validation, refused typed (`PolicyRefusal`),
//!   and which source a launch uses (`resolve_launch_policy`);
//! - templates: the binding an identity seals and its digest, the tool
//!   policy a template derives, and its preamble;
//! - requests: normalization of a create, update or remove against the
//!   policy, refused with the closed vocabulary (`IdentityRefusal`), and the
//!   approval plan of the normalized request;
//! - `render_v1`: the one canonical, complete plain-text rendering whose
//!   sha256 is the decision's subject digest, with the host's consequence
//!   lines in their own section, and its canonical parts;
//! - the structured card, parsed from the rendering.
//!
//! The executor over the durable decision owner, the decision ingresses,
//! the host RPC family, preparation delivery, the per-call tool bound and
//! the agent tools build on this module; none of them is here.

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
mod template;

#[cfg(test)]
mod tests;

pub use approval::{ApprovalPlan, PlanRequest, PlannedOutcome, plan_approval};
pub use config::{
    ApprovalConfig, ApprovalKind, ApprovalOutcomeConfig, ApprovalRuleConfig, ApproverConfig,
    AuthorityFieldConfig, ChangeClass, ConsoleConfig, CreatorKind, CreatorRuleConfig,
    CreatorSelectorConfig, DEFAULT_NAME_PATTERN, ExcessMode, IdentityCreationConfig, KickoffRule,
    LimitsConfig, MAX_DECISION_TIMEOUT_MS, MaxBytesConfig, ModelsConfig, NamingConfig, PeerOutcome,
    PeerRelation, PeerRuleConfig, PeersConfig, PreparationMode, ProfilesConfig, ReservedKeysConfig,
    SelectorConfig, SlugConfig, TemplateConfig, TimeoutsConfig, ToolBoundsConfig,
};
pub use consequences::{
    CONSEQUENCES_CALLBACK, ConsequenceHook, ConsequenceLine, HookRefusal,
    consequences_callback_params, parse_consequences_callback_result,
};
pub use digest::Sha256Digest;
pub use parts::{CanonicalPart, canonical_parts, part_ranges, shows_every_part};
pub use policy::{
    ApprovalPolicy, ApprovalRule, ApproverRule, AuthorityFieldRule, CORE_BATCH_MAX, CREATION_TOOLS,
    ConsoleCreation, CreationLimits, CreatorRule, CreatorSelector, DEFAULT_PEER_RULE_ID,
    IdentityCreationPolicy, Limit, META_RESERVED_LABEL_KEYS, ModelChoice, NamingPolicy,
    OutcomeRule, PeerPolicy, PeerRule, ProfileChoice, RuntimeOptionPolicy, Selector, SlugRefusal,
    Template, TextLimits, Timeouts, resolve_launch_policy,
};
pub use presentation::{CardBlock, CardEntry, CardError, CardValue, StructuredCard, ValueSegment};
pub use refusal::{
    IdentityRefusal, IdentityRefusalKind, LimitKind, NameTakenBy, PolicyRefusal, PolicyRefusalKind,
    RequestProblem, ValidationRefusal,
};
pub use render::{
    HOST_CONSEQUENCE_MAX_BYTES, HOST_CONSEQUENCES_MAX_BYTES, RENDERER, RENDERING_MAX_BYTES,
    Rendering, labels, render_v1,
};
pub use request::{
    CONSOLE_SURFACE, Consequence, ConsequenceEffect, CreateIdentitySpec, CreatedEntryFacts,
    CreatedSpec, CreatorOrigin, EdgeBasis, IdentityRequest, NormalizedRequest, PeerFacts,
    PeerVerdict, RemoveIdentitySpec, RequestAction, RequestContext, RequestEnvelope, Requester,
    RequesterFacts, SourceEvidence, UpdateIdentitySpec, UpdateSetSpec, UpdatedSpec,
    creator_templates, evaluate_peer, normalize_create, normalize_remove, normalize_update,
    template_for,
};
pub use template::{
    CREATOR_INSTRUCTIONS_HEADING, CreatorBound, MobProfiles, Preamble, PreambleInputs,
    ProfileFacts, TemplateBinding, callback_definition_digest, check_build_callbacks,
    compose_member_instructions, effective_tools, native_tool_policy, resolve_binding,
    sealed_upper_bound,
};
