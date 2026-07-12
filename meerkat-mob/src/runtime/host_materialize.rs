//! Member-host materialization substrate (multi-host mobs phase 3,
//! DEC-P3H-1..10).
//!
//! Serves the build half of `MaterializeMember`: decompiles the digest-covered
//! [`PortableMemberSpec`] back into [`BuildAgentConfigParams`] and re-runs the
//! ONE compiler (`crate::build::build_agent_config`) — no second prompt/tool
//! assembly exists (plan §14.2/§15 R1, A1). The member's comms runtime is
//! materializer-built (durable per-session keypair under the daemon's identity
//! root, DEC-P3H-3) and injected into the factory through the
//! `session_comms_runtime_override` type-erasure seam; member-side supervisor
//! authority is HOST-SEEDED through
//! `meerkat_runtime::comms_drain::{bind,authorize}_supervisor_for_materialized_session`
//! (DEC-P3H-4 — no BindMember round trip).
//!
//! Budget carrier (ADJ-1): the digest-covered
//! `PortableSpawnOverlay.budget_limits` is the ONE owner; there is no
//! `budget_seed` sibling anywhere on this path.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;

use meerkat_contracts::wire::WireAuthBindingRef;
use meerkat_contracts::wire::supervisor_bridge::{
    BridgePeerIdentity, MaterializeLaunchMode, MaterializeLaunchOutcome,
};
use meerkat_contracts::wire::{
    PortableMcpDecl, PortableMemberSpec, PortableSkillSource, PortableSystemPrompt,
    WireResolvedToolAccessPolicy, WireSpawnContinuityIntent,
};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use meerkat_runtime::SessionServiceRuntimeExt as _;

use crate::MobRuntimeMode;
use crate::build::{BuildAgentConfigParams, BuildResumedAgentConfigParams};
use crate::definition::{MobDefinition, SkillSource};
use crate::profile::{Profile, ProfileBinding, ResumeOverrideField, ToolConfig};
use crate::runtime::SpawnSystemPromptOverride;
use crate::runtime::host_actor::{
    HostCapabilityFacts, ProviderPresenceProbe, ProviderPresenceProbeError,
};
use crate::runtime::member_upcall::MemberUpcallBindingStamp;
use crate::runtime::provisioner::{
    CommittedRuntimeSessionPublicationLease, MemberSessionDisposalArc,
    MemberSessionDisposalVerdict, PreparedServiceActorTransaction, RuntimeSessionState,
    RuntimeTurnFinalizationBoundaryLease,
};
use crate::runtime::session_service::MobSessionService;

// ---------------------------------------------------------------------------
// Tier-2 preflight probe (DEC-P3H-8) — widens the tier-1 presence probe
// ---------------------------------------------------------------------------

/// Tier-2 materialize preflight probe: the tier-1 provider presence facts
/// plus a per-binding presence walk of a NAMED realm chain (the W2.5 recipe —
/// zero network, zero OAuth, presence-level reads only; gotcha 13).
///
/// Implemented above this crate (the composing binary owns the effective
/// config chain and token store) and injected through
/// [`HostMemberSubstrate::preflight_probe`]. Every answer is a pure shell
/// fact; the machine composes the verdict (A11).
#[async_trait]
pub trait MaterializePreflightProbe: ProviderPresenceProbe {
    /// Whether the named binding (or, for `None`, the provider's default
    /// chain) is presence-resolvable on this host.
    async fn binding_resolvable(
        &self,
        binding: Option<&meerkat_core::AuthBindingRef>,
        provider: meerkat_core::Provider,
    ) -> Result<bool, ProviderPresenceProbeError>;
}

// ---------------------------------------------------------------------------
// Substrate bundle (DEC-P3H-2)
// ---------------------------------------------------------------------------

/// Member-build substrate for `MaterializeMember`/`ReleaseMember` serving.
/// `None` on `MobHostActorConfig.member_host` ⇒ those commands typed-reject
/// `Unavailable` (bind-only composition; fixtures that only test bind keep
/// working unchanged).
pub struct HostMemberSubstrate {
    pub session_service: Arc<dyn MobSessionService>,
    pub runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    /// Durable event projection for generation-floor capture on Resume.
    /// `None` only for explicitly ephemeral hosts.
    pub durable_event_log:
        Option<Arc<dyn meerkat_runtime::member_observation::DurableEventLogRead>>,
    /// The daemon's opened-backend durability fact.
    pub realm_backend_persistent: bool,
    /// Identity root for durable per-session member keypairs (DEC-P3H-3) —
    /// stable across restart, so the revived member pubkey never changes
    /// (A20: host-autonomous revival produces NO ack to refresh
    /// `member_peer_ids`).
    pub member_identity_root: std::path::PathBuf,
    /// Tier-2 preflight probe (extends the tier-1 `ProviderPresenceProbe`).
    pub preflight_probe: Arc<dyn MaterializePreflightProbe>,
}

// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------

/// Typed decompile failure: the spec decoded on the wire but cannot be
/// re-validated into domain build inputs on this host. Ordinary validation —
/// never laundered into `Internal`.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeDecompileError {
    #[error("portable output_schema is not a valid schema: {detail}")]
    OutputSchema { detail: String },
    #[error("portable spec context is not valid JSON: {detail}")]
    Context { detail: String },
    #[error("mob tool authority context projection failed to rehydrate: {detail}")]
    AuthorityContext { detail: String },
    #[error(
        "declared MCP stdio env key '{key}' for server '{server}' is absent from the host environment"
    )]
    McpEnvKeyMissing { server: String, key: String },
    #[error(
        "declared MCP HTTP header names for server '{server}' have no host-side value source \
         in this engine version (v1 compiles them empty)"
    )]
    McpHeaderNamesUnsupported { server: String },
    #[error("MCP connect timeout for server '{server}' exceeds the supported range")]
    McpTimeoutOutOfRange { server: String },
}

/// Typed materialize serving failure with its wire projection.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeServeError {
    #[error(transparent)]
    Decompile(#[from] MaterializeDecompileError),
    #[error("resume session '{session_id}' not found on the member host")]
    ResumeSessionNotFound { session_id: String },
    #[error(
        "recorded session '{session_id}' is terminal {state} and cannot be revived at the same tuple; the controller must advance the member generation"
    )]
    ResumeSessionNonRecoverable { session_id: String, state: String },
    #[error("resume session id '{session_id}' is not a valid session id: {detail}")]
    ResumeSessionIdInvalid { session_id: String, detail: String },
    #[error("member build compile failed: {0}")]
    Build(#[from] crate::error::MobError),
    #[error("member comms runtime construction failed: {detail}")]
    Comms { detail: String },
    #[error("host-seeded supervisor bind failed: {detail}")]
    SupervisorBind { detail: String },
    #[error("member session service failed: {0}")]
    SessionService(#[from] SessionError),
    #[error("session service returned '{created}' for admitted member session '{expected}'")]
    IdentityMismatch { expected: String, created: String },
    #[error("runtime session bindings preparation failed: {detail}")]
    Bindings { detail: String },
    #[error("member session executor attachment failed: {detail}")]
    ExecutorAttach { detail: String },
    #[error("member durable event floor read failed: {detail}")]
    EventFloor { detail: String },
    #[error(
        "unrecorded member session '{session_id}' could not be proven quiescent after '{original}': {cleanup}"
    )]
    UnrecordedSessionCleanup {
        session_id: String,
        original: String,
        cleanup: String,
    },
    #[error(
        "revived member keypair diverged from the recorded pubkey (recorded '{recorded}', derived '{derived}')"
    )]
    RevivedIdentityDiverged { recorded: String, derived: String },
}

impl MaterializeServeError {
    /// Whether continuing to mutate host authority in this actor would risk
    /// orphaning a runtime that no durable row can enumerate.
    pub fn requires_actor_fail_stop(&self) -> bool {
        matches!(self, Self::UnrecordedSessionCleanup { .. })
    }

    /// The wire rejection projection (cause + reason). `Internal` is reserved
    /// for genuine invariant/infrastructure faults; decode-adjacent
    /// validation failures map to `Unsupported` with the typed detail.
    pub fn wire_cause(
        &self,
    ) -> (
        meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause,
        String,
    ) {
        use meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause as Cause;
        match self {
            Self::Decompile(err) => (Cause::Unsupported, err.to_string()),
            Self::ResumeSessionNotFound { .. } | Self::ResumeSessionNonRecoverable { .. } => {
                (Cause::ResumeSessionNotFound, self.to_string())
            }
            Self::ResumeSessionIdInvalid { .. } => (Cause::Unsupported, self.to_string()),
            Self::Build(_)
            | Self::Comms { .. }
            | Self::SupervisorBind { .. }
            | Self::SessionService(_)
            | Self::IdentityMismatch { .. }
            | Self::Bindings { .. }
            | Self::ExecutorAttach { .. }
            | Self::EventFloor { .. }
            | Self::UnrecordedSessionCleanup { .. }
            | Self::RevivedIdentityDiverged { .. } => (Cause::Internal, self.to_string()),
        }
    }
}

fn generation_start_seq_after(latest_seq: Option<u64>) -> Result<u64, MaterializeServeError> {
    latest_seq.unwrap_or(0).checked_add(1).ok_or_else(|| {
        MaterializeServeError::EventFloor {
            detail: "resume event sequence space is exhausted at u64::MAX; no strictly later generation floor can be represented".to_string(),
        }
    })
}

fn combine_unregister_cleanup_result(
    session_id: &SessionId,
    context: &str,
    unregister_error: Option<meerkat_runtime::RuntimeDriverError>,
    fallback_result: Result<(), MaterializeServeError>,
) -> Result<(), MaterializeServeError> {
    match (unregister_error, fallback_result) {
        (None, result) => result,
        // Absence is idempotent only after the fallback fence independently
        // proved the machine registration, service actor, and sidecar absent.
        (Some(meerkat_runtime::RuntimeDriverError::NotFound { .. }), Ok(())) => Ok(()),
        (Some(unregister), Ok(())) => Err(MaterializeServeError::Bindings {
            detail: format!(
                "{context}: runtime unregister failed for session {session_id}: {unregister}"
            ),
        }),
        (Some(unregister), Err(fallback)) => Err(MaterializeServeError::Bindings {
            detail: format!(
                "{context}: runtime unregister failed for session {session_id}: {unregister}; fallback cleanup also failed: {fallback}"
            ),
        }),
    }
}

fn combine_prepared_session_unregister_result(
    session_id: &SessionId,
    original: MaterializeServeError,
    unregister_result: Result<(), meerkat_runtime::RuntimeDriverError>,
) -> MaterializeServeError {
    match unregister_result {
        Ok(()) => original,
        Err(cleanup) => MaterializeServeError::UnrecordedSessionCleanup {
            session_id: session_id.to_string(),
            original: original.to_string(),
            cleanup: format!("prepared runtime unregister failed: {cleanup}"),
        },
    }
}

/// Validate every deterministic conversion in a portable member spec without
/// consulting host-local secret values. A missing environment value is an
/// availability fact, not durable corruption; supplying inert placeholders
/// lets validation continue far enough to expose any later structural fault.
pub fn validate_portable_spec_structure(
    spec: &PortableMemberSpec,
) -> Result<(), MaterializeDecompileError> {
    decompile_portable_spec_with_env(spec, &|_| Some(String::new())).map(drop)
}

/// Decompile a [`PortableMemberSpec`] into owned inputs for the ONE compiler.
/// Total over the spec's fields (T21): every carried fact lands in exactly
/// one build destination; structurally-absent facts have nothing to map.
pub fn decompile_portable_spec(
    spec: &PortableMemberSpec,
) -> Result<DecompiledMemberBuild, MaterializeDecompileError> {
    decompile_portable_spec_with_env(spec, &|key| std::env::var(key).ok())
}

// ---------------------------------------------------------------------------
// Spec decompile (the A1 one-compiler answer) — §4.2 field-for-field
// ---------------------------------------------------------------------------

/// System prompt as decompiled from the portable overlay. `Inherit` is
/// decode-unrepresentable (R3), so the compiler's Inherit fallback is
/// unreachable for materialized members by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecompiledSystemPrompt {
    Replace(String),
    Disable,
}

/// Owned build inputs recovered from a [`PortableMemberSpec`]. Feeds the ONE
/// compiler (`build_agent_config`) plus the post-compile stamps the spec does
/// not carry (comms runtime override, `host_prompt_sections`, keep_alive).
pub struct DecompiledMemberBuild {
    pub mob_id: crate::ids::MobId,
    pub profile_name: crate::ids::ProfileName,
    pub agent_identity: crate::ids::AgentIdentity,
    pub profile: Profile,
    /// Minimal definition shim carrying exactly the four extract facts the
    /// compiler reads: `models`, `image_generation_provider`, `skills`
    /// (inline-only) and the profile-name roster (default spawn-profile grant
    /// parity). Nothing else in the shim is ever read by the compiler.
    pub definition: MobDefinition,
    pub context: Option<serde_json::Value>,
    pub labels: Option<BTreeMap<String, String>>,
    pub additional_instructions: Option<Vec<String>>,
    pub system_prompt: DecompiledSystemPrompt,
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// Rehydrated UNSEALED context (serde cannot mint the seal) — visibility
    /// composition only; dispatch authority is controlling-side (R6).
    pub mob_tool_authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
    /// ADJ-1: the digest-covered overlay budget is the ONE budget carrier.
    pub budget_limits: Option<meerkat_core::BudgetLimits>,
    /// The member's own resolved policy — the O3 Inherit-resolution source
    /// for child spawns through the member-operator forwarding dispatcher.
    pub requester_resolved_policy: Option<WireResolvedToolAccessPolicy>,
}

fn wire_runtime_mode_to_domain(
    mode: meerkat_contracts::wire::WireMobRuntimeMode,
) -> MobRuntimeMode {
    match mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => MobRuntimeMode::TurnDriven,
    }
}

fn wire_resume_override_to_domain(
    field: meerkat_contracts::wire::WireMobResumeOverrideField,
) -> ResumeOverrideField {
    match field {
        meerkat_contracts::wire::WireMobResumeOverrideField::Model => ResumeOverrideField::Model,
        meerkat_contracts::wire::WireMobResumeOverrideField::Provider => {
            ResumeOverrideField::Provider
        }
        meerkat_contracts::wire::WireMobResumeOverrideField::ProviderParams => {
            ResumeOverrideField::ProviderParams
        }
    }
}

fn decompile_mcp_servers(
    decls: &BTreeMap<String, PortableMcpDecl>,
    env_lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Vec<meerkat_core::mcp_config::McpServerConfig>, MaterializeDecompileError> {
    let mut servers = Vec::with_capacity(decls.len());
    for (name, decl) in decls {
        match decl {
            PortableMcpDecl::Stdio {
                command,
                args,
                required_env_keys,
                connect_timeout_secs,
            } => {
                // Values never travel (A10): declared stdio env keys are
                // satisfied from the HOST's own process environment; a
                // missing key is a typed error (preflight verified presence,
                // this read is the value fetch).
                let mut env = HashMap::with_capacity(required_env_keys.len());
                for key in required_env_keys {
                    let value = env_lookup(key).ok_or_else(|| {
                        MaterializeDecompileError::McpEnvKeyMissing {
                            server: name.clone(),
                            key: key.clone(),
                        }
                    })?;
                    env.insert(key.clone(), value);
                }
                let mut server = meerkat_core::mcp_config::McpServerConfig::stdio(
                    name.clone(),
                    command.clone(),
                    args.clone(),
                    env,
                );
                if let Some(secs) = connect_timeout_secs {
                    server.connect_timeout_secs = Some(u32::try_from(*secs).map_err(|_| {
                        MaterializeDecompileError::McpTimeoutOutOfRange {
                            server: name.clone(),
                        }
                    })?);
                }
                servers.push(server);
            }
            PortableMcpDecl::Http {
                url,
                http_transport,
                required_header_names,
                connect_timeout_secs,
            } => {
                // v1 compiles header names empty (spec-compiler rule); a
                // non-empty set has no host-side value source — fail closed,
                // never strip.
                if !required_header_names.is_empty() {
                    return Err(MaterializeDecompileError::McpHeaderNamesUnsupported {
                        server: name.clone(),
                    });
                }
                servers.push(meerkat_core::mcp_config::McpServerConfig {
                    name: name.clone(),
                    transport: meerkat_core::mcp_config::McpTransportConfig::Http(
                        meerkat_core::mcp_config::McpHttpConfig {
                            url: url.clone(),
                            headers: HashMap::new(),
                            transport: *http_transport,
                        },
                    ),
                    connect_timeout_secs: connect_timeout_secs
                        .map(|seconds| {
                            u32::try_from(seconds).map_err(|_| {
                                MaterializeDecompileError::McpTimeoutOutOfRange {
                                    server: name.clone(),
                                }
                            })
                        })
                        .transpose()?,
                });
            }
        }
    }
    Ok(servers)
}

fn decompile_portable_spec_with_env(
    spec: &PortableMemberSpec,
    env_lookup: &impl Fn(&str) -> Option<String>,
) -> Result<DecompiledMemberBuild, MaterializeDecompileError> {
    let output_schema = spec
        .profile
        .output_schema
        .as_ref()
        .map(|envelope| {
            let value =
                envelope
                    .to_value()
                    .map_err(|err| MaterializeDecompileError::OutputSchema {
                        detail: err.to_string(),
                    })?;
            meerkat_core::MeerkatSchema::new(value).map_err(|err| {
                MaterializeDecompileError::OutputSchema {
                    detail: err.to_string(),
                }
            })
        })
        .transpose()?;

    let profile = Profile {
        model: spec.profile.model.clone(),
        // Provider is REQUIRED on the portable profile (R4): the member host
        // never re-infers a provider for the model id.
        provider: Some(spec.profile.provider),
        self_hosted_server_id: spec.profile.self_hosted_server_id.clone(),
        image_generation_provider: spec.profile.image_generation_provider,
        auto_compact_threshold: spec.profile.auto_compact_threshold,
        resume_overrides: spec
            .profile
            .resume_overrides
            .iter()
            .copied()
            .map(wire_resume_override_to_domain)
            .collect(),
        skills: spec.profile.skills.clone(),
        tools: ToolConfig {
            builtins: spec.profile.tools.builtins,
            shell: spec.profile.tools.shell,
            comms: spec.profile.tools.comms,
            memory: spec.profile.tools.memory,
            workgraph: spec.profile.tools.workgraph,
            mob: spec.profile.tools.mob,
            schedule: spec.profile.tools.schedule,
            image_generation: spec.profile.tools.image_generation,
            // The host-surface MCP allowlist and rust bundles are
            // structurally absent from the portable vocabulary (A4) —
            // nothing to map.
            mcp: Vec::new(),
            mcp_servers: decompile_mcp_servers(&spec.profile.tools.mcp_servers, env_lookup)?,
            rust_bundles: Vec::new(),
        },
        peer_description: spec.profile.peer_description.clone(),
        external_addressable: spec.profile.external_addressable,
        // Placement is not backend vocabulary; binding is machine-owned.
        backend: None,
        runtime_mode: wire_runtime_mode_to_domain(spec.profile.runtime_mode),
        max_inline_peer_notifications: spec.profile.max_inline_peer_notifications,
        output_schema,
        provider_params: spec.profile.provider_params.clone().map(Into::into),
    };

    // Minimal definition shim: exactly the extract facts the compiler reads.
    let mut definition = MobDefinition::explicit(spec.mob_id.as_str());
    definition.models = spec.definition_extract.models.clone();
    definition.image_generation_provider = spec.definition_extract.image_generation_provider;
    definition.skills = spec
        .definition_extract
        .skills
        .iter()
        .map(|(name, source)| {
            let PortableSkillSource::Inline { content } = source;
            // `Path` is unrepresentable on the wire — the compiler's fs read
            // is unreachable by construction for materialized members.
            (
                name.clone(),
                SkillSource::Inline {
                    content: content.clone(),
                },
            )
        })
        .collect();
    for name in &spec.definition_extract.profile_names {
        // The compiler reads only the KEYS of `definition.profiles` (the
        // default spawn-profile grant roster); the binding value is never
        // resolved on this path.
        definition.profiles.insert(
            crate::ids::ProfileName::from(name.as_str()),
            ProfileBinding::RealmRef {
                realm_profile: name.clone(),
            },
        );
    }

    let context = spec
        .overlay
        .context
        .as_ref()
        .map(|envelope| {
            envelope
                .to_value()
                .map_err(|err| MaterializeDecompileError::Context {
                    detail: err.to_string(),
                })
        })
        .transpose()?;

    let system_prompt = match &spec.overlay.system_prompt {
        PortableSystemPrompt::Set { text } => DecompiledSystemPrompt::Replace(text.clone()),
        PortableSystemPrompt::Disable => DecompiledSystemPrompt::Disable,
    };

    let tool_access_policy = spec.overlay.tool_access_policy.as_ref().map(|policy| {
        // `Inherit` is decode-unrepresentable (O3); the compiler's
        // `Inherit → None` trap is unreachable here.
        match policy {
            WireResolvedToolAccessPolicy::AllowList(names) => {
                meerkat_core::ops::ToolAccessPolicy::AllowList(names.iter().cloned().collect())
            }
            WireResolvedToolAccessPolicy::DenyList(names) => {
                meerkat_core::ops::ToolAccessPolicy::DenyList(names.iter().cloned().collect())
            }
        }
    });

    // Rehydrate through the serde lane: the wire projection carries exactly
    // the serde-visible facts; the private generation seal cannot be minted
    // by this conversion (the factory's `UnresolvedInherit`-style backstops
    // treat the context as unsealed input). Deliberately NOT a typed
    // `TryFrom`: the core type's fields are private and its only constructor
    // is cfg-gated to the generated-authority bridge, so deserialization is
    // the SINGLE unsealed mint — a public unsealed constructor would weaken
    // that invariant more than this one-off Value round-trip costs.
    let mob_tool_authority_context = spec
        .overlay
        .mob_tool_authority_context
        .as_ref()
        .map(|wire| {
            serde_json::to_value(wire)
                .and_then(serde_json::from_value::<meerkat_core::service::MobToolAuthorityContext>)
                .map_err(|err| MaterializeDecompileError::AuthorityContext {
                    detail: err.to_string(),
                })
        })
        .transpose()?;

    Ok(DecompiledMemberBuild {
        mob_id: crate::ids::MobId::from(spec.mob_id.as_str()),
        profile_name: crate::ids::ProfileName::from(spec.profile_name.as_str()),
        agent_identity: crate::ids::AgentIdentity::from(spec.agent_identity.as_str()),
        profile,
        definition,
        context,
        labels: spec.overlay.labels.clone(),
        additional_instructions: spec.overlay.additional_instructions.clone(),
        system_prompt,
        tool_access_policy,
        mob_tool_authority_context,
        auth_binding: spec
            .overlay
            .auth_binding
            .clone()
            .map(meerkat_core::AuthBindingRef::from),
        budget_limits: spec.overlay.budget_limits.clone(),
        requester_resolved_policy: spec.overlay.tool_access_policy.clone(),
    })
}

// ---------------------------------------------------------------------------
// Preflight observation assembly (DEC-P3H-8) — pure shell-read facts
// ---------------------------------------------------------------------------

/// Shell-read preflight facts plus the first offending concrete detail per
/// fact (the machine owns the verdict kind; the shell owns the reason
/// string).
pub struct MaterializePreflightObservations {
    pub model_resolvable: bool,
    pub binding_resolvable: bool,
    pub env_keys_present: bool,
    pub stdio_commands_present: bool,
    pub engine_protocol_supported: bool,
    pub durable_sessions_required: bool,
    pub realm_backend_persistent: bool,
    pub memory_required: bool,
    pub memory_capability: bool,
    pub first_missing_env_key: Option<String>,
    pub first_missing_stdio_server: Option<String>,
}

fn stdio_command_discoverable(command: &str) -> bool {
    let path = std::path::Path::new(command);
    if path.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
        return path.is_file();
    }
    let Some(search) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search).any(|dir| dir.join(command).is_file())
}

/// Assemble the tier-2 preflight observations. Every field is a pure fact,
/// never a pre-decision (A11/RMAT read discipline); the generated
/// `ResolveMaterializePreflight` arms compose the verdict.
pub async fn assemble_preflight_observations(
    spec: &PortableMemberSpec,
    launch: &MaterializeLaunchMode,
    protocol_version: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion,
    substrate: &HostMemberSubstrate,
    capability_facts: HostCapabilityFacts,
) -> Result<MaterializePreflightObservations, ProviderPresenceProbeError> {
    let provider = spec.profile.provider;
    let catalog = meerkat_models::canonical();
    // "The (model, provider) pair names a client this binary can construct":
    // catalog hit OR mob-scoped custom-model hit OR a pinned provider that
    // constructs without host config (self-hosted needs its alias binding).
    let provider_constructible = !matches!(provider, meerkat_core::Provider::SelfHosted)
        || spec.profile.self_hosted_server_id.is_some();
    let model_resolvable = catalog.entry_for(provider, &spec.profile.model).is_some()
        || spec
            .definition_extract
            .models
            .contains_key(&spec.profile.model)
        || provider_constructible;

    let binding_ref = spec
        .overlay
        .auth_binding
        .clone()
        .map(meerkat_core::AuthBindingRef::from);
    let binding_resolvable = substrate
        .preflight_probe
        .binding_resolvable(binding_ref.as_ref(), provider)
        .await?;

    let mut first_missing_env_key = None;
    for key in &spec.required_env_keys {
        if std::env::var_os(key).is_none() {
            first_missing_env_key = Some(key.clone());
            break;
        }
    }

    let mut first_missing_stdio_server = None;
    for (server, decl) in &spec.profile.tools.mcp_servers {
        if let PortableMcpDecl::Stdio { command, .. } = decl
            && !stdio_command_discoverable(command)
        {
            first_missing_stdio_server = Some(server.clone());
            break;
        }
    }

    Ok(MaterializePreflightObservations {
        model_resolvable,
        binding_resolvable,
        env_keys_present: first_missing_env_key.is_none(),
        stdio_commands_present: first_missing_stdio_server.is_none(),
        first_missing_env_key,
        first_missing_stdio_server,
        engine_protocol_supported: protocol_version.supports_multi_host(),
        durable_sessions_required: matches!(launch, MaterializeLaunchMode::Resume { .. })
            || matches!(
                spec.overlay.continuity_intent,
                WireSpawnContinuityIntent::DurableIdentity { .. }
            ),
        realm_backend_persistent: substrate.realm_backend_persistent,
        memory_required: spec.profile.tools.memory,
        memory_capability: capability_facts.memory_store,
    })
}

// ---------------------------------------------------------------------------
// The materializer (owner-task inline, DEC-P3H-1)
// ---------------------------------------------------------------------------

/// One live materialized member runtime plus the Ed25519 keypair the
/// acceptor demux signs acks with for this identity. The keypair is the SAME
/// durable per-session key material the runtime loaded from the member
/// identity dir (DEC-P3H-3); pubkey equality is verified at build, so the
/// two can never name different identities.
#[derive(Clone)]
pub struct LiveMemberRuntime {
    pub runtime: Arc<meerkat_comms::CommsRuntime>,
    pub ack_keypair: Arc<meerkat_comms::Keypair>,
}

/// Outcome of one successful member build — the ack material plus the
/// concrete runtime handle the actor registers post-commit (DEC-P3H-7).
pub struct MaterializedBuildOutcome {
    pub session_id: SessionId,
    /// Stable residency transaction acquired before any old incarnation is
    /// quiesced and retained until the host actor durably records this build.
    pub residency_update: meerkat_runtime::meerkat_machine::MemberResidencyUpdate,
    /// Exact B+M publication transaction. The attachment remains
    /// non-admitting until the host actor commits both its durable row and
    /// residency, then consumes this lease with
    /// `commit_serving_with_residency`.
    pub(super) runtime_publication: Option<CommittedRuntimeSessionPublicationLease>,
    pub member_runtime: Arc<meerkat_comms::CommsRuntime>,
    /// Acceptor-demux ack keypair for the member identity (registry
    /// registration material, DEC-P3H-7).
    pub ack_keypair: Arc<meerkat_comms::Keypair>,
    pub member_pubkey: String,
    pub member_peer_id: String,
    pub launch_outcome: MaterializeLaunchOutcome,
    pub resolved_auth_binding: Option<WireAuthBindingRef>,
    /// First durable event seq belonging to this materialization generation.
    pub generation_start_seq: u64,
}

/// Machine-admitted authority facts for one materialization attempt.
/// Keeping the tuple intact makes it harder for callers to transpose a
/// generation/fence or host/binding-generation pair.
pub struct MaterializeServingContext<'a> {
    pub generation: u64,
    pub fence_token: u64,
    pub host_id: &'a str,
    pub host_binding_generation: u64,
    pub supervisor: &'a BridgePeerIdentity,
    pub epoch: u64,
}

pub struct RevivedMemberOutcome {
    pub member: LiveMemberRuntime,
    /// `Some` when revival repaired or rebuilt the runtime residency and the
    /// host actor must atomically publish the recovered incarnation+journal.
    pub residency_update: Option<meerkat_runtime::meerkat_machine::MemberResidencyUpdate>,
    /// Present only when revival rebuilt the runtime. An already-serving
    /// idempotent replay has no unpublished attachment to commit.
    pub(super) runtime_publication: Option<CommittedRuntimeSessionPublicationLease>,
}

/// Builds (and revives) member sessions on the daemon's substrate. One
/// instance per actor; every method runs inline on the single owner task, so
/// the internal live-runtime index needs no interior mutability.
pub struct HostMemberMaterializer {
    substrate: HostMemberSubstrate,
    disposal: MemberSessionDisposalArc,
    /// Shell capability index of materializer-owned concrete comms runtimes,
    /// keyed by member session id. Never lifecycle truth (the authority's
    /// materialized rows are); used for replay ensure and release teardown.
    live_runtimes: HashMap<SessionId, LiveMemberRuntime>,
    /// Atomically sampled upcall authority tuple for each mounted dispatcher.
    upcall_binding_stamps: HashMap<SessionId, Arc<MemberUpcallBindingStamp>>,
    /// Executor-residency sidecars (ADJ-23), shared with the disposal arc so
    /// release/dispose clears exactly the entries the materializer
    /// registered. Transport-only owner context — never lifecycle truth.
    runtime_sessions: Arc<tokio::sync::RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

const fn member_runtime_is_healthy(
    materializer_runtime: bool,
    executor_resident: bool,
    machine_serving: bool,
    service_live: bool,
) -> bool {
    materializer_runtime && executor_resident && machine_serving && service_live
}

impl HostMemberMaterializer {
    pub fn new(substrate: HostMemberSubstrate) -> Self {
        let runtime_sessions = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let disposal = MemberSessionDisposalArc::with_runtime_sessions(
            Arc::clone(&substrate.session_service),
            Some(Arc::clone(&substrate.runtime_adapter)),
            Arc::clone(&runtime_sessions),
        );
        Self {
            substrate,
            disposal,
            live_runtimes: HashMap::new(),
            upcall_binding_stamps: HashMap::new(),
            runtime_sessions,
        }
    }

    /// Acquire B only after prior-incarnation quiescence, then re-prove that
    /// all process carriers are still vacant while B excludes actor creation.
    /// This closes the quiesce -> prepare gap without ever using a broad
    /// SessionId cleanup against a possible replacement.
    async fn acquire_vacant_materialization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeTurnFinalizationBoundaryLease, MaterializeServeError> {
        let boundary = RuntimeTurnFinalizationBoundaryLease::acquire(
            &self.substrate.session_service,
            session_id,
        )
        .await
        .map_err(MaterializeServeError::Build)?;
        let machine_registered = self
            .substrate
            .runtime_adapter
            .contains_session(session_id)
            .await;
        let sidecar_registered = self.runtime_sessions.read().await.contains_key(session_id);
        let actor_registered = self
            .substrate
            .session_service
            .live_session_actor_registered(session_id)
            .await
            .map_err(MaterializeServeError::SessionService)?;
        if machine_registered || sidecar_registered || actor_registered {
            return Err(MaterializeServeError::Bindings {
                detail: format!(
                    "session {session_id} acquired a replacement process carrier before exact host materialization could claim B"
                ),
            });
        }
        Ok(boundary)
    }

    /// ADJ-23 — hold a materialized session EXECUTOR-ATTACHED: the SAME
    /// residency every local session-backed member has. `RegisterSession`
    /// happened at binding preparation; this attaches the production
    /// `MobSessionRuntimeExecutor` so (a) external Message-class peer ingress
    /// classifies between turns (an idle attached session, exactly like a
    /// local member), and (b) queued `DeliverMemberInput` is executed by the
    /// runtime loop's wake. No bespoke turn runner exists on the member host.
    async fn attach_member_executor(
        &self,
        transaction: PreparedServiceActorTransaction,
    ) -> Result<CommittedRuntimeSessionPublicationLease, MaterializeServeError> {
        transaction
            .prepare_host_runtime_publication_owned(
                Arc::clone(&self.substrate.runtime_adapter),
                Arc::clone(&self.runtime_sessions),
            )
            .await
            .map_err(|error| MaterializeServeError::ExecutorAttach {
                detail: error.to_string(),
            })
    }

    /// Commit the controller-owned member incarnation into the local
    /// MeerkatMachine before executor attachment. Local session resources are
    /// prepared first for factory construction, but only this exact tuple is
    /// authoritative for terminal-publication fencing on a remote host.
    async fn prepare_member_runtime_placement(
        &self,
        session_id: &SessionId,
        agent_identity: &crate::ids::AgentIdentity,
        generation: u64,
        fence_token: u64,
    ) -> Result<(), MaterializeServeError> {
        let agent_runtime_id = crate::ids::AgentRuntimeId::new(
            agent_identity.clone(),
            crate::ids::Generation::new(generation),
        );
        self.substrate
            .runtime_adapter
            .prepare_runtime_placement_binding(
                session_id.clone(),
                meerkat_runtime::identifiers::LogicalRuntimeId::new(agent_runtime_id.to_string()),
                fence_token,
                generation,
            )
            .await
            .map_err(|error| MaterializeServeError::Bindings {
                detail: error.to_string(),
            })
    }

    pub fn substrate(&self) -> &HostMemberSubstrate {
        &self.substrate
    }

    pub fn live_runtime(&self, session_id: &SessionId) -> Option<&LiveMemberRuntime> {
        self.live_runtimes.get(session_id)
    }

    /// Refresh the private supervisor authority on an already-live member
    /// after its host binding is rebound. Returns `false` when this
    /// materializer has no live runtime for the recorded session; the durable
    /// row still carries the new endpoint and boot/replay revival will seed it.
    pub async fn refresh_live_supervisor(
        &mut self,
        recorded_session_id: &str,
        supervisor: &TrustedPeerDescriptor,
        epoch: u64,
        host_id: &str,
        host_binding_generation: u64,
    ) -> Result<bool, MaterializeServeError> {
        let session_id = SessionId::parse(recorded_session_id).map_err(|error| {
            MaterializeServeError::ResumeSessionIdInvalid {
                session_id: recorded_session_id.to_string(),
                detail: error.to_string(),
            }
        })?;
        let Some(live) = self.live_runtimes.get(&session_id).cloned() else {
            return Ok(false);
        };
        meerkat_runtime::comms_drain::authorize_supervisor_for_materialized_session(
            self.substrate.runtime_adapter.as_ref(),
            &session_id,
            live.runtime.as_ref() as &dyn CoreCommsRuntime,
            supervisor,
            epoch,
        )
        .await
        .map_err(|error| MaterializeServeError::SupervisorBind {
            detail: error.to_string(),
        })?;
        if let Some(binding_stamp) = self.upcall_binding_stamps.get(&session_id) {
            let current = binding_stamp.snapshot();
            binding_stamp.update(
                current.generation,
                current.fence_token,
                host_id,
                host_binding_generation,
                current.member_session_id,
            );
        }
        Ok(true)
    }

    /// Whether this materializer still owns a serving runtime incarnation.
    ///
    /// Health requires the materializer's exact concrete comms runtime, its
    /// executor-residency sidecar, and the runtime adapter's canonical serving
    /// witness, plus the live session-service actor that actually executes
    /// turns. The machine-local witness jointly requires an ingress-
    /// admissible lifecycle, an Active executor registration with live runtime-
    /// loop channels, and a live mob-owned drain for the exact comms runtime.
    /// Registry membership alone is insufficient because `Idle`, `Stopped`, and
    /// `Retired` registrations can survive after the executor incarnation stops
    /// serving. Likewise, a durable session snapshot without its live service
    /// actor is resumable state, not a currently serving member.
    pub async fn session_live(&self, session_id: &SessionId) -> bool {
        let Some(live) = self.live_runtimes.get(session_id) else {
            return false;
        };
        let comms_runtime: Arc<dyn CoreCommsRuntime> = live.runtime.clone();
        let sidecar = self.runtime_sessions.read().await.get(session_id).cloned();
        let machine_witness = self
            .substrate
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await;
        let executor_resident = sidecar.as_ref().is_some_and(|state| {
            machine_witness.as_ref().is_some_and(|witness| {
                state.witness() == witness && state.attachment_is_active(witness)
            })
        });
        let service_live = match self
            .substrate
            .session_service
            .live_session_actor_registered(session_id)
            .await
        {
            Ok(live) => live,
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    error = %error,
                    "mob host: failed to read live member session-service witness"
                );
                false
            }
        };
        let machine_serving = match self
            .substrate
            .runtime_adapter
            .materialized_member_runtime_is_serving(session_id, &comms_runtime)
            .await
        {
            Ok(serving) => serving,
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    error = %error,
                    "mob host: failed to read materialized member serving witness"
                );
                false
            }
        };
        member_runtime_is_healthy(true, executor_resident, machine_serving, service_live)
    }

    /// Quiesce one non-serving materializer incarnation before rebuilding the
    /// same SessionId. Runtime-stop cleanup is scheduled outside the runtime
    /// loop so unregister can join that loop; this method waits for all three
    /// old process carriers (machine registration, live service actor, and
    /// executor sidecar) to disappear before a replacement can be created.
    async fn quiesce_nonserving_incarnation(
        &mut self,
        session_id: &SessionId,
    ) -> Result<(), MaterializeServeError> {
        let old_state = self.runtime_sessions.read().await.get(session_id).cloned();
        let unregister_error = if let Some(old_state) = old_state.as_ref() {
            match self
                .substrate
                .runtime_adapter
                .unregister_executor_attachment_if_current(old_state.witness())
                .await
            {
                Ok(true) => None,
                Ok(false)
                    if self
                        .substrate
                        .runtime_adapter
                        .current_executor_attachment_witness(session_id)
                        .await
                        .is_none() =>
                {
                    None
                }
                Ok(false) => {
                    return Err(MaterializeServeError::Bindings {
                        detail: format!(
                            "non-serving incarnation {session_id} changed executor attachment before quiescence; refusing to touch the replacement"
                        ),
                    });
                }
                Err(error) => Some(error),
            }
        } else {
            let machine_registered = self
                .substrate
                .runtime_adapter
                .contains_session(session_id)
                .await;
            let service_live = self
                .substrate
                .session_service
                .live_session_actor_registered(session_id)
                .await
                .map_err(MaterializeServeError::SessionService)?;
            if machine_registered || service_live {
                return Err(MaterializeServeError::Bindings {
                    detail: format!(
                        "non-serving incarnation {session_id} has process state without its exact host attachment sidecar; explicit recovery is required"
                    ),
                });
            }
            None
        };

        let fallback_result = async {
            // Exact attachment unregister owns actor + sidecar cleanup. There
            // is deliberately no SessionId-only fallback: if that exact hook
            // cannot prove quiescence, a delayed broad discard could erase the
            // next incarnation after this method returns.
            const CLEANUP_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
            tokio::time::timeout(CLEANUP_GRACE, async {
                loop {
                    let runtime_registered = self
                        .substrate
                        .runtime_adapter
                        .contains_session(session_id)
                        .await;
                    let service_live = self
                        .substrate
                        .session_service
                        .live_session_actor_registered(session_id)
                        .await
                        .map_err(MaterializeServeError::SessionService)?;
                    let executor_resident =
                        self.runtime_sessions.read().await.contains_key(session_id);
                    if !runtime_registered && !service_live && !executor_resident {
                        return Ok::<(), MaterializeServeError>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| MaterializeServeError::Bindings {
                detail: format!(
                    "non-serving incarnation {session_id} did not quiesce within {}s",
                    CLEANUP_GRACE.as_secs()
                ),
            })??;

            self.live_runtimes.remove(session_id);
            self.upcall_binding_stamps.remove(session_id);
            Ok(())
        }
        .await;
        combine_unregister_cleanup_result(
            session_id,
            "non-serving incarnation quiescence",
            unregister_error,
            fallback_result,
        )
    }

    /// Full member-session disposal through the extracted provisioner arc
    /// (DEC-P3H-5): outer quiesce → ownership-discriminated archive →
    /// runtime unregister. The comms-runtime identity claim releases via the
    /// dropped handle's RAII.
    pub async fn dispose(
        &mut self,
        session_id: &SessionId,
    ) -> Result<MemberSessionDisposalVerdict, SessionError> {
        let verdict = self.disposal.dispose(session_id).await?;
        self.live_runtimes.remove(session_id);
        self.upcall_binding_stamps.remove(session_id);
        Ok(verdict)
    }

    /// Runtime-retire-only disposal (non-persistent backend arm, DEC-P3H-6):
    /// runtime retire + binding release; no durable archive exists to write.
    pub async fn release_runtime_only(
        &mut self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.disposal.release_runtime_only(session_id).await?;
        self.live_runtimes.remove(session_id);
        self.upcall_binding_stamps.remove(session_id);
        Ok(())
    }

    /// Close one live session incarnation and wait for its durable event
    /// projection to consume every queued envelope before a higher
    /// materialization generation reuses the same SessionId.
    async fn quiesce_generation_cutover(
        &mut self,
        session_id: &SessionId,
        projection_witness_required: bool,
    ) -> Result<(), MaterializeServeError> {
        // Reuse the same three-carrier quiescence fence as exact replay.
        // Runtime unregister can make the old loop schedule detached cleanup;
        // rebuilding immediately after a second, uncoordinated discard would
        // let that delayed task ABA-delete the higher-generation service
        // actor. Any terminal event from the old incarnation remains part of
        // the OLD generation and is drained below before the replacement floor
        // is captured.
        self.quiesce_nonserving_incarnation(session_id).await?;

        const PROJECTION_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
        let witnessed = tokio::time::timeout(
            PROJECTION_DRAIN_GRACE,
            self.substrate
                .session_service
                .await_event_projection_drain(session_id),
        )
        .await
        .map_err(|_| MaterializeServeError::EventFloor {
            detail: format!(
                "generation cutover event projection did not drain within {}s",
                PROJECTION_DRAIN_GRACE.as_secs()
            ),
        })?
        .map_err(|error| MaterializeServeError::EventFloor {
            detail: format!("generation cutover event projection failed: {error}"),
        })?;
        if projection_witness_required && !witnessed {
            return Err(MaterializeServeError::EventFloor {
                detail: "generation cutover lost the live projection drain witness".to_string(),
            });
        }
        Ok(())
    }

    /// Force one unsuccessfully-published incarnation inert while preserving
    /// the already-durable session document for discovery and explicit resume.
    async fn quiesce_unrecorded_session(&mut self, session_id: &SessionId) -> Result<(), String> {
        self.quiesce_nonserving_incarnation(session_id)
            .await
            .map_err(|error| {
                format!("failed to quiesce session while preserving its durable snapshot: {error}")
            })
    }

    /// Forget only the materializer-local handles after the exact retained
    /// publication transaction has quiesced (or fail-closed while attempting
    /// to quiesce) its machine attachment, service actor, and sidecar under B.
    /// This method deliberately performs no SessionId-wide lifecycle action.
    pub fn forget_runtime_after_exact_publication_abort(&mut self, session_id: &SessionId) {
        self.live_runtimes.remove(session_id);
        self.upcall_binding_stamps.remove(session_id);
    }

    /// Quiesce a superseded live incarnation while its durable row and
    /// snapshot still own recovery. Archival is forbidden until the replacing
    /// row and exact runtime residency have both committed.
    pub async fn quiesce_before_superseding_record(
        &mut self,
        session_id: &SessionId,
    ) -> Result<(), String> {
        self.quiesce_unrecorded_session(session_id).await
    }

    /// Dispose a superseded snapshot only after a different-session
    /// replacement has committed its durable row and runtime residency.
    pub async fn dispose_after_superseding_commit(
        &mut self,
        session_id: &SessionId,
    ) -> Result<(), String> {
        let first_disposal_error = self
            .dispose(session_id)
            .await
            .err()
            .map(|error| error.to_string());

        if let Err(error) = self.quiesce_nonserving_incarnation(session_id).await {
            return Err(match first_disposal_error {
                Some(disposal) => format!(
                    "initial superseded-session disposal failed ({disposal}); forced runtime quiescence also failed ({error})"
                ),
                None => format!(
                    "forced runtime quiescence failed after superseded-session disposal: {error}"
                ),
            });
        }

        if let Some(first_error) = first_disposal_error
            && let Err(retry_error) = self.dispose(session_id).await
        {
            return Err(format!(
                "initial superseded-session disposal failed ({first_error}); runtime quiesced, but archive retry failed ({retry_error})"
            ));
        }
        Ok(())
    }

    async fn preserve_snapshot_after_revival_failure(
        &mut self,
        session_id: &SessionId,
        original: MaterializeServeError,
    ) -> MaterializeServeError {
        match self.quiesce_unrecorded_session(session_id).await {
            Ok(()) => original,
            Err(cleanup) => MaterializeServeError::UnrecordedSessionCleanup {
                session_id: session_id.to_string(),
                original: original.to_string(),
                cleanup,
            },
        }
    }

    async fn rollback_prepared_session_after_failure(
        &self,
        prepared: &mut meerkat_runtime::PreparedSessionMaterialization,
        original: MaterializeServeError,
    ) -> MaterializeServeError {
        let session_id = prepared.session_id().clone();
        let rollback_result = prepared
            .rollback_now_under_turn_finalization_boundary()
            .await;
        combine_prepared_session_unregister_result(
            &session_id,
            original,
            rollback_result.map(|_| ()),
        )
    }

    /// Build one member session from the admitted payload facts (§4.3).
    /// Machine admission + preflight have ALREADY run; this is shell effect
    /// work between preflight-admit and `RecordMaterializedMember`.
    pub async fn materialize(
        &mut self,
        spec: &PortableMemberSpec,
        launch: &MaterializeLaunchMode,
        context: MaterializeServingContext<'_>,
    ) -> Result<MaterializedBuildOutcome, MaterializeServeError> {
        let MaterializeServingContext {
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            supervisor,
            epoch,
        } = context;
        let decompiled = decompile_portable_spec(spec)?;
        // Step 2 — session id + launch resolution (the provisioner template).
        let (
            session_id,
            resumed_session,
            launch_outcome,
            generation_start_seq,
            residency_update,
            mut materialization_boundary,
        ) = match launch {
            MaterializeLaunchMode::Fresh {} => {
                let id = SessionId::new();
                let boundary = self.acquire_vacant_materialization_boundary(&id).await?;
                let residency_update = self
                    .substrate
                    .runtime_adapter
                    .begin_member_residency_update(id.clone())
                    .await;
                (
                    id,
                    None,
                    MaterializeLaunchOutcome::Fresh,
                    1,
                    residency_update,
                    Some(boundary),
                )
            }
            MaterializeLaunchMode::Resume { session_id } => {
                let id = SessionId::parse(session_id).map_err(|err| {
                    MaterializeServeError::ResumeSessionIdInvalid {
                        session_id: session_id.clone(),
                        detail: err.to_string(),
                    }
                })?;
                // Acquire the stable residency slot BEFORE quiescing the old
                // runtime. A delayed G1 effect either completes before this
                // point or observes the final G2/vacant state afterward; it
                // can never validate G1 and then mutate the replacement.
                let residency_update = self
                    .substrate
                    .runtime_adapter
                    .begin_member_residency_update(id.clone())
                    .await;
                // Higher-generation same-session Resume is a real cutover,
                // never an in-place live rebind. Quiesce every old producer,
                // await the durable projection drain, then capture the floor
                // and rebuild from the persisted snapshot. Exact-tuple replay
                // is answered by the actor before entering this materializer.
                // The materializer-owned runtime map is the local live-owner
                // fact. Check it first so no richer durable/live authority read
                // is needed while the old turn is intentionally parked. A
                // known live runtime must proceed directly to machine-
                // authorized quiescence so unregister can interrupt that turn.
                let was_live = if self.live_runtimes.contains_key(&id) {
                    true
                } else {
                    self.substrate
                        .session_service
                        .live_session_actor_registered(&id)
                        .await
                        .map_err(MaterializeServeError::SessionService)?
                };
                self.quiesce_generation_cutover(&id, was_live).await?;
                let boundary = self.acquire_vacant_materialization_boundary(&id).await?;
                let log = self.substrate.durable_event_log.as_ref().ok_or_else(|| {
                    MaterializeServeError::EventFloor {
                        detail: "resume requires a durable event projection".to_string(),
                    }
                })?;
                let latest_seq = log.latest_seq(&id).await.map_err(|error| {
                    MaterializeServeError::EventFloor {
                        detail: error.to_string(),
                    }
                })?;
                let generation_start_seq = generation_start_seq_after(latest_seq)?;
                match self
                    .substrate
                    .session_service
                    .load_persisted_session(&id)
                    .await?
                {
                    Some(session) => (
                        id,
                        Some(session),
                        MaterializeLaunchOutcome::ResumedFromSnapshot,
                        generation_start_seq,
                        residency_update,
                        Some(boundary),
                    ),
                    // Never a silent Fresh (§19.F row 1).
                    None => {
                        return Err(MaterializeServeError::ResumeSessionNotFound {
                            session_id: session_id.clone(),
                        });
                    }
                }
            }
        };

        let mut config = self
            .compile_member_config(&decompiled, &session_id, resumed_session)
            .await?;

        // Step 2 tail — runtime bindings for the (possibly pre-created) id.
        let mut prepared = self
            .substrate
            .runtime_adapter
            .prepare_local_session_materialization(session_id.clone())
            .await
            .map_err(|err| MaterializeServeError::Bindings {
                detail: err.to_string(),
            })?;
        let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        if let Err(error) =
            crate::runtime::provisioner::install_prepared_mob_session_executor_handles(
                Arc::clone(&self.substrate.session_service),
                Arc::clone(&self.substrate.runtime_adapter),
                &prepared,
                actor_witness_slot.clone(),
            )
            .await
        {
            let rollback_error = prepared
                .rollback_now_under_turn_finalization_boundary()
                .await
                .err();
            return Err(MaterializeServeError::Bindings {
                detail: rollback_error.map_or_else(
                    || format!("failed to install prepared session handles: {error}"),
                    |rollback| {
                        format!(
                            "failed to install prepared session handles: {error}; exact rollback failed: {rollback}"
                        )
                    },
                ),
            });
        }
        let bindings = prepared.bindings_clone();
        let claim_handle = Arc::clone(bindings.session_claim_handle());

        // Step 3 — member comms runtime (DEC-P3H-3): durable per-session
        // keypair under the daemon identity root ⇒ pubkey stable across
        // restart revival (A20).
        let live = self
            .build_member_comms_runtime(&config, &session_id, claim_handle)
            .await;
        let live = match live {
            Ok(live) => live,
            Err(error) => {
                return Err(self
                    .rollback_prepared_session_after_failure(&mut prepared, error)
                    .await);
            }
        };
        let member_runtime = Arc::clone(&live.runtime);

        // The host-seeded supervisor bind below publishes a trust edge into
        // this runtime, and the runtime validates that write against ITS
        // installed generated trust owner (fail-closed). Install the
        // session's generated peer-comms authority NOW — the factory's own
        // install at create_session re-installs the same generated owner
        // idempotently.
        if let Err(detail) = bindings.install_peer_comms_on(member_runtime.as_ref()) {
            let original = MaterializeServeError::Comms { detail };
            return Err(self
                .rollback_prepared_session_after_failure(&mut prepared, original)
                .await);
        }
        config.runtime_build_mode =
            meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);

        // Step 4 — host-seeded supervisor authority (DEC-P3H-4): the SAME
        // staged machine seams the member drain's BindMember arm uses, with
        // the host-authority-adjudicated payload facts. No BindMember round
        // trip ever happens.
        let supervisor_desc = supervisor.clone().into_trusted_peer_descriptor();
        if let Err(error) = meerkat_runtime::comms_drain::bind_supervisor_for_materialized_session(
            self.substrate.runtime_adapter.as_ref(),
            &session_id,
            member_runtime.as_ref() as &dyn CoreCommsRuntime,
            &supervisor_desc,
            epoch,
        )
        .await
        {
            let original = MaterializeServeError::SupervisorBind {
                detail: error.to_string(),
            };
            return Err(self
                .rollback_prepared_session_after_failure(&mut prepared, original)
                .await);
        }

        self.mount_member_operator_tools(
            &mut config,
            &decompiled,
            &session_id,
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            &supervisor_desc,
            &member_runtime,
        );

        // Inject the pre-built runtime through the type-erasure seam; the
        // factory verifies the comms name and skips config-mode construction.
        config.session_comms_runtime_override = Some(
            meerkat::encode_session_comms_runtime_override_for_service(Arc::clone(&member_runtime)),
        );

        // Step 5 — create. Kickoff arrives later via DeliverMemberInput
        // (§15.3): Defer + Discard are pinned by `to_create_session_request`.
        let req = crate::build::to_create_session_request(
            &config,
            meerkat_core::types::ContentInput::Text(String::new()),
        );
        let boundary = materialization_boundary.take().ok_or_else(|| {
            MaterializeServeError::Bindings {
                detail: format!(
                    "materialization for session {session_id} lost its exact service boundary before actor creation"
                ),
            }
        })?;
        let actor_transaction = PreparedServiceActorTransaction::new(
            session_id.clone(),
            Arc::clone(&self.substrate.session_service),
            prepared,
            boundary,
            actor_witness_slot,
        )
        .map_err(MaterializeServeError::Build)?;
        let (created, actor_transaction) = actor_transaction
            .create_owned(req, None)
            .await
            .map_err(MaterializeServeError::Build)?;
        if created.session_id != session_id {
            let original = MaterializeServeError::IdentityMismatch {
                expected: session_id.to_string(),
                created: created.session_id.to_string(),
            };
            drop(config);
            drop(member_runtime);
            drop(live);
            if let Err(cleanup) = actor_transaction.abort().await {
                return Err(MaterializeServeError::UnrecordedSessionCleanup {
                    session_id: session_id.to_string(),
                    original: original.to_string(),
                    cleanup: format!(
                        "exact actor/materialization transaction cleanup failed: {cleanup}"
                    ),
                });
            }
            return Err(original);
        }

        // Keep Prepared+B as the single volatile-cleanup owner while every
        // fallible placement, drain, and auth read completes. The durable
        // session document is not compensation-owned. Executor attachment is
        // deliberately the final build operation; it returns a retained B+M
        // publication lease instead of exposing a serving runtime early.
        let pre_attach = async {
            self.prepare_member_runtime_placement(
                &session_id,
                &decompiled.agent_identity,
                generation,
                fence_token,
            )
            .await?;
            self.substrate
                .runtime_adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    Arc::clone(&member_runtime) as Arc<dyn CoreCommsRuntime>,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(spec.mob_id.clone()),
                )
                .await
                .map_err(|error| MaterializeServeError::Comms {
                    detail: format!("member peer-ingress drain spawn failed: {error}"),
                })?;

            // Step 6 — ack material. `Some(ref)` iff a configured
            // lease-bearing binding resolved, else `None` (env_default).
            self.read_resolved_auth_binding(&session_id).await
        }
        .await;
        let resolved_auth_binding = match pre_attach {
            Ok(resolved) => resolved,
            Err(original) => {
                if let Err(cleanup) = actor_transaction.abort().await {
                    return Err(MaterializeServeError::UnrecordedSessionCleanup {
                        session_id: session_id.to_string(),
                        original: original.to_string(),
                        cleanup: format!(
                            "exact actor/materialization transaction cleanup failed: {cleanup}"
                        ),
                    });
                }
                return Err(original);
            }
        };
        let runtime_publication = match self.attach_member_executor(actor_transaction).await {
            Ok(publication) => publication,
            Err(original) => return Err(original),
        };
        let member_pubkey = member_runtime.public_key().to_pubkey_string();
        let member_peer_id = member_runtime.public_key().to_peer_id().as_str();
        let ack_keypair = Arc::clone(&live.ack_keypair);
        self.live_runtimes.insert(session_id.clone(), live);

        Ok(MaterializedBuildOutcome {
            session_id,
            residency_update,
            runtime_publication: Some(runtime_publication),
            member_runtime,
            ack_keypair,
            member_pubkey,
            member_peer_id,
            launch_outcome,
            resolved_auth_binding,
            generation_start_seq,
        })
    }

    /// Ensure a replayed/recovered member is actually alive (ADJ-9): when the
    /// recorded session is dead and a substrate exists, recompose it from the
    /// durable spec row, then let the caller ack the RECORDED response
    /// verbatim. Boot revival (A20) rides the same path with the recovered
    /// binding facts — zero bridge traffic.
    #[allow(clippy::too_many_arguments)] // one exact persisted incarnation's revival facts
    pub async fn revive_from_row(
        &mut self,
        spec: &PortableMemberSpec,
        recorded_session_id: &str,
        recorded_member_pubkey: &str,
        generation: u64,
        fence_token: u64,
        host_id: &str,
        host_binding_generation: u64,
        supervisor: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> Result<RevivedMemberOutcome, MaterializeServeError> {
        let decompiled = decompile_portable_spec(spec)?;
        self.revive_decompiled_from_row(
            spec,
            decompiled,
            recorded_session_id,
            recorded_member_pubkey,
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            supervisor,
            epoch,
        )
        .await
    }

    /// Boot-revival variant whose durable spec was fully decompiled before
    /// the host descriptor/registry publication boundary. This prevents a
    /// deterministic durable-row conversion error from being discovered only
    /// after the daemon has become public.
    #[allow(clippy::too_many_arguments)] // one exact persisted incarnation's revival facts
    pub async fn revive_prepared_from_row(
        &mut self,
        spec: &PortableMemberSpec,
        decompiled: DecompiledMemberBuild,
        recorded_session_id: &str,
        recorded_member_pubkey: &str,
        generation: u64,
        fence_token: u64,
        host_id: &str,
        host_binding_generation: u64,
        supervisor: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> Result<RevivedMemberOutcome, MaterializeServeError> {
        self.revive_decompiled_from_row(
            spec,
            decompiled,
            recorded_session_id,
            recorded_member_pubkey,
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            supervisor,
            epoch,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)] // one exact persisted incarnation's revival facts
    async fn revive_decompiled_from_row(
        &mut self,
        spec: &PortableMemberSpec,
        decompiled: DecompiledMemberBuild,
        recorded_session_id: &str,
        recorded_member_pubkey: &str,
        generation: u64,
        fence_token: u64,
        host_id: &str,
        host_binding_generation: u64,
        supervisor: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> Result<RevivedMemberOutcome, MaterializeServeError> {
        let session_id = SessionId::parse(recorded_session_id).map_err(|err| {
            MaterializeServeError::ResumeSessionIdInvalid {
                session_id: recorded_session_id.to_string(),
                detail: err.to_string(),
            }
        })?;
        if self.session_live(&session_id).await {
            // Already serving under this materializer — idempotent. Reuse the
            // same canonical witness as HostStatus/replay admission so a
            // stopped, draining, or task-dead incarnation can never bypass the
            // repair path merely because its service actor or handles remain.
            if let Some(live) = self.live_runtimes.get(&session_id) {
                if let Some(stamp) = self.upcall_binding_stamps.get(&session_id) {
                    stamp.update(
                        generation,
                        fence_token,
                        host_id,
                        host_binding_generation,
                        session_id.to_string(),
                    );
                }
                let expected =
                    meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                        mob_id: spec.mob_id.clone(),
                        agent_identity: spec.agent_identity.clone(),
                        host_id: host_id.to_string(),
                        binding_generation: host_binding_generation,
                        member_session_id: session_id.to_string(),
                        generation,
                        fence_token,
                    };
                if self
                    .substrate
                    .runtime_adapter
                    .member_incarnation(&session_id)
                    .as_ref()
                    == Some(&expected)
                {
                    return Ok(RevivedMemberOutcome {
                        member: live.clone(),
                        residency_update: None,
                        runtime_publication: None,
                    });
                }
                // A residency repair cannot safely publish from this sampled
                // live state: between the sample and the actor's later commit,
                // attachment A could be replaced by B. Rebuild through the
                // quiesce -> B -> pending exact attachment transaction below
                // so the repair carries a non-clone A/B witness all the way to
                // publication instead of binding A's incarnation to B's M.
            }
        }
        if let Ok(
            state @ (meerkat_runtime::RuntimeState::Retired
            | meerkat_runtime::RuntimeState::Destroyed),
        ) = self
            .substrate
            .runtime_adapter
            .runtime_state(&session_id)
            .await
        {
            return Err(MaterializeServeError::ResumeSessionNonRecoverable {
                session_id: recorded_session_id.to_string(),
                state: state.to_string(),
            });
        }
        // A dead SERVICE session can leave this materializer's runtime
        // residue alive: the machine session registration (executor-attached,
        // so a fresh create's LLM hydration would be guard-rejected), the
        // runtime loop, the comms drain, and the session-identity claim (a
        // second runtime for the same session id fails typed "session
        // identity already active"). Recomposition needs the SAME clean slate
        // a daemon restart produces: unregister the machine session (tearing
        // down its loop + drain and releasing the identity claim through the
        // dropped runtime's RAII) and drop this materializer's handles —
        // never a runtime RETIRE, whose terminal phase would fail-close the
        // rebuild's hydration. Durable session data is untouched, and the
        // durable per-session keypair file keeps the member pubkey stable
        // across the rebuild (A20).
        // The canonical serving witness above already failed. Quiesce
        // unconditionally: even if adapter registration and the executor
        // sidecar disappeared just before a lingering service actor, that
        // third carrier must be discarded before same-SessionId rebuild. A
        // preflight residue predicate would create an ABA window between its
        // last observation and `create_session`.
        let residency_update = self
            .substrate
            .runtime_adapter
            .begin_member_residency_update(session_id.clone())
            .await;
        self.quiesce_nonserving_incarnation(&session_id).await?;
        let mut materialization_boundary = Some(
            self.acquire_vacant_materialization_boundary(&session_id)
                .await?,
        );
        // Read the recovery snapshot only after the prior incarnation is
        // quiescent and B excludes same-SessionId actor replacement. Reading
        // earlier can capture a snapshot while the old actor is still
        // committing its final turn, then recreate from stale durable state.
        let session = self
            .substrate
            .session_service
            .load_persisted_session(&session_id)
            .await?;
        let Some(session) = session else {
            if self
                .substrate
                .session_service
                .session_known_to_archive_authority(&session_id)
                .await?
            {
                // The public resume read intentionally hides archived rows and
                // runtime-Retired authority. Preserve that absorbing contract
                // even after the in-process runtime registration disappears:
                // this is a terminal same-tuple result, not a transient missing
                // snapshot that should be retried forever.
                return Err(MaterializeServeError::ResumeSessionNonRecoverable {
                    session_id: recorded_session_id.to_string(),
                    state: "Archived/Retired".to_string(),
                });
            }
            return Err(MaterializeServeError::ResumeSessionNotFound {
                session_id: recorded_session_id.to_string(),
            });
        };

        let mut config = self
            .compile_member_config(&decompiled, &session_id, Some(session))
            .await?;
        let mut prepared = match self
            .substrate
            .runtime_adapter
            .prepare_local_session_materialization(session_id.clone())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let original = MaterializeServeError::Bindings {
                    detail: error.to_string(),
                };
                drop(materialization_boundary.take());
                return Err(self
                    .preserve_snapshot_after_revival_failure(&session_id, original)
                    .await);
            }
        };
        let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        if let Err(error) =
            crate::runtime::provisioner::install_prepared_mob_session_executor_handles(
                Arc::clone(&self.substrate.session_service),
                Arc::clone(&self.substrate.runtime_adapter),
                &prepared,
                actor_witness_slot.clone(),
            )
            .await
        {
            let rollback_error = prepared
                .rollback_now_under_turn_finalization_boundary()
                .await
                .err();
            let original = MaterializeServeError::Bindings {
                detail: rollback_error.map_or_else(
                    || format!("failed to install prepared session handles: {error}"),
                    |rollback| {
                        format!(
                            "failed to install prepared session handles: {error}; exact rollback failed: {rollback}"
                        )
                    },
                ),
            };
            drop(materialization_boundary.take());
            return Err(self
                .preserve_snapshot_after_revival_failure(&session_id, original)
                .await);
        }
        let bindings = prepared.bindings_clone();
        let claim_handle = Arc::clone(bindings.session_claim_handle());

        let live = match self
            .build_member_comms_runtime(&config, &session_id, claim_handle)
            .await
        {
            Ok(live) => live,
            Err(error) => {
                let error = self
                    .rollback_prepared_session_after_failure(&mut prepared, error)
                    .await;
                drop(materialization_boundary.take());
                return Err(self
                    .preserve_snapshot_after_revival_failure(&session_id, error)
                    .await);
            }
        };
        let member_runtime = Arc::clone(&live.runtime);
        // Same identity root ⇒ SAME member pubkey; divergence is durable
        // store corruption and fails closed.
        let derived = member_runtime.public_key().to_pubkey_string();
        if derived != recorded_member_pubkey {
            let original = MaterializeServeError::RevivedIdentityDiverged {
                recorded: recorded_member_pubkey.to_string(),
                derived,
            };
            drop(member_runtime);
            drop(live);
            let original = self
                .rollback_prepared_session_after_failure(&mut prepared, original)
                .await;
            drop(materialization_boundary.take());
            return Err(self
                .preserve_snapshot_after_revival_failure(&session_id, original)
                .await);
        }

        // Same install-before-bind discipline as `materialize`: the
        // supervisor bind's trust publication validates against the target
        // runtime's installed generated trust owner.
        if let Err(detail) = bindings.install_peer_comms_on(member_runtime.as_ref()) {
            drop(member_runtime);
            drop(live);
            let original = self
                .rollback_prepared_session_after_failure(
                    &mut prepared,
                    MaterializeServeError::Comms { detail },
                )
                .await;
            drop(materialization_boundary.take());
            return Err(self
                .preserve_snapshot_after_revival_failure(&session_id, original)
                .await);
        }
        config.runtime_build_mode =
            meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);

        if let Err(error) = meerkat_runtime::comms_drain::bind_supervisor_for_materialized_session(
            self.substrate.runtime_adapter.as_ref(),
            &session_id,
            member_runtime.as_ref() as &dyn CoreCommsRuntime,
            supervisor,
            epoch,
        )
        .await
        {
            drop(config);
            drop(member_runtime);
            drop(live);
            let original = self
                .rollback_prepared_session_after_failure(
                    &mut prepared,
                    MaterializeServeError::SupervisorBind {
                        detail: error.to_string(),
                    },
                )
                .await;
            drop(materialization_boundary.take());
            return Err(self
                .preserve_snapshot_after_revival_failure(&session_id, original)
                .await);
        }

        self.mount_member_operator_tools(
            &mut config,
            &decompiled,
            &session_id,
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            supervisor,
            &member_runtime,
        );
        config.session_comms_runtime_override = Some(
            meerkat::encode_session_comms_runtime_override_for_service(Arc::clone(&member_runtime)),
        );
        let req = crate::build::to_create_session_request(
            &config,
            meerkat_core::types::ContentInput::Text(String::new()),
        );
        let boundary = materialization_boundary.take().ok_or_else(|| {
            MaterializeServeError::Bindings {
                detail: format!(
                    "revival for session {session_id} lost its exact service boundary before actor creation"
                ),
            }
        })?;
        let actor_transaction = PreparedServiceActorTransaction::new(
            session_id.clone(),
            Arc::clone(&self.substrate.session_service),
            prepared,
            boundary,
            actor_witness_slot,
        )
        .map_err(MaterializeServeError::Build)?;
        let (created, actor_transaction) = actor_transaction
            .create_owned(req, None)
            .await
            .map_err(MaterializeServeError::Build)?;
        if created.session_id != session_id {
            let original = MaterializeServeError::IdentityMismatch {
                expected: session_id.to_string(),
                created: created.session_id.to_string(),
            };
            drop(config);
            drop(member_runtime);
            drop(live);
            if let Err(cleanup) = actor_transaction.abort().await {
                return Err(MaterializeServeError::UnrecordedSessionCleanup {
                    session_id: session_id.to_string(),
                    original: original.to_string(),
                    cleanup: format!(
                        "exact actor/materialization transaction cleanup failed: {cleanup}"
                    ),
                });
            }
            return Err(original);
        }

        // Finish every fallible placement/drain step while Prepared+B still
        // owns exact volatile cleanup. The final attachment commit returns the
        // B+M lease that the replay/boot actor carries through residency
        // publication; the durable document remains resume-owned throughout.
        let pre_attach = async {
            self.prepare_member_runtime_placement(
                &session_id,
                &decompiled.agent_identity,
                generation,
                fence_token,
            )
            .await?;
            self.substrate
                .runtime_adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    Arc::clone(&member_runtime) as Arc<dyn CoreCommsRuntime>,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(spec.mob_id.clone()),
                )
                .await
                .map_err(|error| MaterializeServeError::Comms {
                    detail: format!("revived member peer-ingress drain spawn failed: {error}"),
                })?;
            Ok::<(), MaterializeServeError>(())
        }
        .await;
        if let Err(error) = pre_attach {
            drop(config);
            drop(member_runtime);
            drop(live);
            if let Err(cleanup) = actor_transaction.abort().await {
                return Err(MaterializeServeError::UnrecordedSessionCleanup {
                    session_id: session_id.to_string(),
                    original: error.to_string(),
                    cleanup: format!(
                        "exact actor/materialization transaction cleanup failed: {cleanup}"
                    ),
                });
            }
            return Err(error);
        }
        let mut runtime_publication = match self.attach_member_executor(actor_transaction).await {
            Ok(publication) => publication,
            Err(original) => return Err(original),
        };
        // A recovered driver normalizes interrupted nonterminal inputs back to
        // queued state. They still belong to the dead predecessor attachment:
        // replacement B must never execute them. Fence the complete recovered
        // request set while B is still non-serving and M is retained. This is
        // deliberately silent (no runless terminal outbox), so the controller's
        // existing typed timeout remains the externally visible outcome.
        let abandoned_predecessor_inputs = runtime_publication
            .abandon_recovered_predecessor_inputs()
            .await
            .map_err(|error| MaterializeServeError::ExecutorAttach {
                detail: error.to_string(),
            })?;
        if abandoned_predecessor_inputs != 0 {
            tracing::info!(
                %session_id,
                abandoned_predecessor_inputs,
                "mob host revival abandoned requests owned by the interrupted attachment"
            );
        }
        self.live_runtimes.insert(session_id, live.clone());
        Ok(RevivedMemberOutcome {
            member: live,
            residency_update: Some(residency_update),
            runtime_publication: Some(runtime_publication),
        })
    }

    /// Run the ONE compiler over the decompiled spec + apply the post-compile
    /// stamps the spec does not carry (§4.2 tail).
    async fn compile_member_config(
        &self,
        decompiled: &DecompiledMemberBuild,
        session_id: &SessionId,
        resumed_session: Option<meerkat_core::Session>,
    ) -> Result<meerkat::AgentBuildConfig, MaterializeServeError> {
        let base = BuildAgentConfigParams {
            mob_id: &decompiled.mob_id,
            profile_name: &decompiled.profile_name,
            agent_identity: &decompiled.agent_identity,
            profile: &decompiled.profile,
            definition: &decompiled.definition,
            // The member-operator FORWARDING dispatcher (D-X2) binds to the
            // member's OWN comms runtime, which is constructed after this
            // compile — `mount_member_operator_tools` sets
            // `config.external_tools` once the runtime exists.
            external_tools: None,
            context: decompiled.context.clone(),
            labels: decompiled.labels.clone(),
            additional_instructions: decompiled.additional_instructions.clone(),
            // shell_env never travels (A2): shell subprocess env is
            // host-local process env; nothing to inject per-member.
            shell_env: None,
            mob_tool_authority_context: decompiled.mob_tool_authority_context.clone(),
            // Inherited tool filters are hard-deny superseded for remote
            // spawns (ADJ-6) — structurally absent from the spec.
            inherited_tool_filter: None,
            tool_access_policy: decompiled.tool_access_policy.clone(),
            system_prompt_override: match &decompiled.system_prompt {
                DecompiledSystemPrompt::Replace(text) => {
                    Some(SpawnSystemPromptOverride::Replace(text.clone()))
                }
                DecompiledSystemPrompt::Disable => None,
            },
        };

        let mut config = match resumed_session {
            Some(session) => {
                crate::build::build_resumed_agent_config(BuildResumedAgentConfigParams {
                    base,
                    expected_session_id: session_id,
                    resumed_session: session,
                })
                .await?
            }
            None => {
                let mut config = crate::build::build_agent_config(base).await?;
                // Fresh builds pin the admitted session id through a
                // pre-created empty session (the provisioner template).
                config.resume_session = Some(meerkat_core::Session::with_id(session_id.clone()));
                config
            }
        };

        if matches!(decompiled.system_prompt, DecompiledSystemPrompt::Disable)
            && config
                .resume_session
                .as_ref()
                .is_none_or(|s| s.messages().is_empty())
        {
            // The compiler has no Disable arm; the overlay's resolved
            // Disable is applied post-compile (R3: the controlling host
            // resolved inheritance; this host never guesses).
            config.system_prompt = meerkat::SystemPromptOverride::Disable;
        }

        // Post-compile stamps (spec-adjacent, not spec-carried):
        // ADJ-17 / A1 — spec-built sessions are byte-pinned; the factory's
        // skill-inventory extra section is suppressed.
        config.host_prompt_sections = meerkat_core::service::HostPromptSections::SpecPinned;
        // ADJ-1 — the digest-covered overlay budget is the ONE carrier.
        config.budget_limits = decompiled.budget_limits.clone();
        // ADJ-23 — executor-attached residency: EVERY materialized member
        // stays resident on this host regardless of runtime mode. A parked
        // TurnDriven session would sit phase-Idle where external
        // Message-class ingress is machine-refused and queued deliveries
        // have no turn runner — the daemon (not a controlling-side caller)
        // is the only party that can hold the session attached. The
        // overlay's runtime_mode still steers agent behavior.
        config.keep_alive = true;
        // Realm-scoped auth binding resolves on THIS host at factory build.
        config.auth_binding = decompiled.auth_binding.clone();

        Ok(config)
    }

    /// Mount the member-operator forwarding dispatcher onto a compiled config
    /// once the member's concrete comms runtime exists (D-X2 seam): it
    /// replaces the controlling host's in-process
    /// `MobOperatorToolDispatcher` composition, which requires a `MobHandle`
    /// that does not exist on this host. Dispatcher internals are the upcall
    /// lane's deliverable; the mount call is the materializer's.
    #[allow(clippy::too_many_arguments)] // one compiled member's authority/mount facts
    fn mount_member_operator_tools(
        &mut self,
        config: &mut meerkat::AgentBuildConfig,
        decompiled: &DecompiledMemberBuild,
        session_id: &SessionId,
        generation: u64,
        fence_token: u64,
        host_id: &str,
        host_binding_generation: u64,
        supervisor: &TrustedPeerDescriptor,
        member_runtime: &Arc<meerkat_comms::CommsRuntime>,
    ) {
        if !decompiled.profile.tools.mob {
            return;
        }
        let supervisor_route = meerkat_core::comms::PeerRoute::with_display_name(
            supervisor.peer_id,
            supervisor.name.clone(),
        );
        let binding_stamp = self
            .upcall_binding_stamps
            .entry(session_id.clone())
            .or_insert_with(|| {
                Arc::new(MemberUpcallBindingStamp::new(
                    generation,
                    fence_token,
                    host_id,
                    host_binding_generation,
                    session_id.to_string(),
                ))
            });
        binding_stamp.update(
            generation,
            fence_token,
            host_id,
            host_binding_generation,
            session_id.to_string(),
        );
        let binding_stamp = Arc::clone(binding_stamp);
        let dispatcher = crate::runtime::member_upcall::MemberUpcallToolDispatcher::new(
            decompiled.agent_identity.clone(),
            binding_stamp,
            supervisor_route,
            Arc::clone(member_runtime),
            decompiled.requester_resolved_policy.clone(),
        );
        config.external_tools =
            Some(Arc::new(dispatcher) as Arc<dyn meerkat_core::AgentToolDispatcher>);
    }

    /// Build the member's session-scoped comms runtime (DEC-P3H-3): the
    /// factory's own Inproc recipe with the daemon's durable identity root
    /// and the runtime-binding claim handle (§19.L6 — the member host's
    /// machine registry claims each materialized session at construction).
    async fn build_member_comms_runtime(
        &self,
        config: &meerkat::AgentBuildConfig,
        session_id: &SessionId,
        claim_handle: Arc<dyn meerkat_core::handles::SessionClaimHandle>,
    ) -> Result<LiveMemberRuntime, MaterializeServeError> {
        let comms_name =
            config
                .comms_name
                .as_deref()
                .ok_or_else(|| MaterializeServeError::Comms {
                    detail: "compiled member config carries no comms name".to_string(),
                })?;
        let namespace = config
            .realm_id
            .as_ref()
            .map(|realm| realm.as_str().to_string());
        let silent_intents = Arc::new(
            config
                .silent_comms_intents
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<String>>(),
        );
        let runtime = meerkat_comms::CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            comms_name,
            namespace,
            self.substrate.member_identity_root.clone(),
            session_id,
            silent_intents,
            claim_handle,
        )
        .await
        .map_err(|err| MaterializeServeError::Comms {
            detail: err.to_string(),
        })?;
        runtime.require_peer_comms_machine_authority();
        if let Some(meta) = config.peer_meta.clone() {
            runtime.set_peer_meta(meta);
        }
        // Acceptor-demux ack keypair: the durable per-session key FILE the
        // runtime just loaded/persisted is the identity owner; this second
        // read observes the same fact for registry registration
        // (DEC-P3H-7). Divergence from the runtime's own key is identity
        // material corruption and fails closed.
        let identity_dir = self
            .substrate
            .member_identity_root
            .join(session_id.to_string());
        let ack_keypair = meerkat_comms::Keypair::load_or_generate(&identity_dir)
            .await
            .map_err(|err| MaterializeServeError::Comms {
                detail: format!("member ack keypair load failed: {err}"),
            })?;
        if ack_keypair.public_key() != runtime.public_key() {
            return Err(MaterializeServeError::Comms {
                detail: format!(
                    "member ack keypair for session '{session_id}' does not match the \
                     runtime identity (durable key material diverged)"
                ),
            });
        }
        Ok(LiveMemberRuntime {
            runtime: Arc::new(runtime),
            ack_keypair: Arc::new(ack_keypair),
        })
    }

    /// Read the factory's persisted binding stamp back from session metadata
    /// (gotcha 12): `Some(configured ref)` or `None` for env_default.
    async fn read_resolved_auth_binding(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<WireAuthBindingRef>, MaterializeServeError> {
        let session = self
            .substrate
            .session_service
            .load_persisted_session(session_id)
            .await?
            .ok_or_else(|| {
                MaterializeServeError::SessionService(SessionError::NotFound {
                    id: session_id.clone(),
                })
            })?;
        Ok(session
            .session_metadata()
            .and_then(|metadata| metadata.auth_binding)
            .filter(|binding| {
                matches!(
                    binding.origin,
                    meerkat_core::connection::BindingOrigin::Configured
                )
            })
            .map(WireAuthBindingRef::from))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::wire::{
        PortableDefinitionExtract, PortableProfile, PortableSpawnOverlay, PortableToolConfig,
        WireMobToolAuthorityContext,
    };
    use std::collections::BTreeSet;

    #[test]
    fn resume_generation_floor_rejects_sequence_exhaustion_without_aliasing_max() {
        assert_eq!(generation_start_seq_after(None).unwrap(), 1);
        assert_eq!(
            generation_start_seq_after(Some(u64::MAX - 1)).unwrap(),
            u64::MAX
        );
        let error = generation_start_seq_after(Some(u64::MAX))
            .expect_err("MAX has no strictly later generation floor");
        assert!(matches!(error, MaterializeServeError::EventFloor { .. }));
        assert!(error.to_string().contains("sequence space is exhausted"));
    }

    #[test]
    fn unregister_cleanup_result_composition_preserves_every_failure() {
        let session_id = SessionId::new();

        assert!(
            combine_unregister_cleanup_result(&session_id, "test cleanup", None, Ok(())).is_ok()
        );
        let fallback_only = combine_unregister_cleanup_result(
            &session_id,
            "test cleanup",
            None,
            Err(MaterializeServeError::Comms {
                detail: "fallback-only failure".to_string(),
            }),
        )
        .expect_err("a fallback failure must remain visible");
        assert!(matches!(
            fallback_only,
            MaterializeServeError::Comms { ref detail }
                if detail == "fallback-only failure"
        ));

        let missing_runtime = meerkat_runtime::RuntimeDriverError::NotFound {
            runtime_id: meerkat_runtime::identifiers::LogicalRuntimeId::new("missing-runtime"),
        };
        assert!(
            combine_unregister_cleanup_result(
                &session_id,
                "test cleanup",
                Some(missing_runtime.clone()),
                Ok(()),
            )
            .is_ok(),
            "NotFound is benign only after the fallback proves every carrier absent"
        );
        let missing_with_failed_fallback = combine_unregister_cleanup_result(
            &session_id,
            "test cleanup",
            Some(missing_runtime),
            Err(MaterializeServeError::Comms {
                detail: "absence proof failed".to_string(),
            }),
        )
        .expect_err("NotFound cannot hide a failed fallback absence proof");
        let missing_with_failed_fallback = missing_with_failed_fallback.to_string();
        assert!(missing_with_failed_fallback.contains("Runtime not found: missing-runtime"));
        assert!(missing_with_failed_fallback.contains("absence proof failed"));

        let unregister_only = combine_unregister_cleanup_result(
            &session_id,
            "test cleanup",
            Some(meerkat_runtime::RuntimeDriverError::Internal(
                "unregister-only failure".to_string(),
            )),
            Ok(()),
        )
        .expect_err("a non-NotFound unregister failure must remain visible");
        let unregister_only = unregister_only.to_string();
        assert!(unregister_only.contains("test cleanup"));
        assert!(unregister_only.contains("unregister-only failure"));

        let dual_failure = combine_unregister_cleanup_result(
            &session_id,
            "test cleanup",
            Some(meerkat_runtime::RuntimeDriverError::Internal(
                "primary unregister failure".to_string(),
            )),
            Err(MaterializeServeError::Comms {
                detail: "secondary fallback failure".to_string(),
            }),
        )
        .expect_err("both cleanup failures must remain visible");
        let dual_failure = dual_failure.to_string();
        assert!(dual_failure.contains("primary unregister failure"));
        assert!(dual_failure.contains("secondary fallback failure"));
    }

    #[test]
    fn prepared_session_unregister_failure_retains_original_error() {
        let session_id = SessionId::new();
        let combined = combine_prepared_session_unregister_result(
            &session_id,
            MaterializeServeError::Comms {
                detail: "original comms failure".to_string(),
            },
            Err(meerkat_runtime::RuntimeDriverError::Internal(
                "cleanup unregister failure".to_string(),
            )),
        );

        match combined {
            MaterializeServeError::UnrecordedSessionCleanup {
                session_id: recorded_session_id,
                original,
                cleanup,
            } => {
                assert_eq!(recorded_session_id, session_id.to_string());
                assert!(original.contains("original comms failure"));
                assert!(cleanup.contains("cleanup unregister failure"));
            }
            other => panic!("expected composed unrecorded-session cleanup, got {other:?}"),
        }
    }

    #[test]
    fn host_health_requires_runtime_executor_and_admissibly_live_machine() {
        assert!(member_runtime_is_healthy(true, true, true, true));
        assert!(!member_runtime_is_healthy(false, true, true, true));
        assert!(!member_runtime_is_healthy(true, false, true, true));
        assert!(!member_runtime_is_healthy(true, true, false, true));
        assert!(!member_runtime_is_healthy(true, true, true, false));
    }

    fn sample_spec() -> PortableMemberSpec {
        PortableMemberSpec {
            mob_id: "mob-1".to_string(),
            profile_name: "worker".to_string(),
            agent_identity: "worker-1".to_string(),
            profile: PortableProfile {
                model: "claude-opus-4-8".to_string(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                image_generation_provider: Some(meerkat_core::Provider::Gemini),
                auto_compact_threshold: std::num::NonZeroU64::new(60_000),
                resume_overrides: vec![meerkat_contracts::wire::WireMobResumeOverrideField::Model],
                skills: vec!["review".to_string()],
                tools: PortableToolConfig {
                    builtins: true,
                    shell: false,
                    comms: true,
                    memory: false,
                    workgraph: false,
                    mob: true,
                    schedule: false,
                    image_generation: false,
                    mcp_servers: BTreeMap::new(),
                    non_portable_disabled: Vec::new(),
                },
                peer_description: "reviews things".to_string(),
                external_addressable: false,
                runtime_mode: meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: Some(3),
                output_schema: None,
                provider_params: None,
            },
            definition_extract: PortableDefinitionExtract {
                models: BTreeMap::new(),
                image_generation_provider: None,
                skills: BTreeMap::from([(
                    "review".to_string(),
                    PortableSkillSource::Inline {
                        content: "# Review skill".to_string(),
                    },
                )]),
                profile_names: vec!["worker".to_string()],
            },
            overlay: PortableSpawnOverlay {
                context: None,
                labels: Some(BTreeMap::from([("team".to_string(), "alpha".to_string())])),
                additional_instructions: Some(vec!["Be terse.".to_string()]),
                system_prompt: PortableSystemPrompt::Set {
                    text: "You are worker-1.".to_string(),
                },
                tool_access_policy: Some(WireResolvedToolAccessPolicy::AllowList(vec![
                    "member_status".to_string(),
                ])),
                mob_tool_authority_context: Some(WireMobToolAuthorityContext {
                    principal_token: "principal-token-1".to_string(),
                    can_create_mobs: false,
                    can_mutate_profiles: false,
                    can_run_adaptive_packs: false,
                    managed_mob_scope: BTreeSet::from(["mob-1".to_string()]),
                    spawn_profile_scope: BTreeMap::from([(
                        "mob-1".to_string(),
                        BTreeSet::from(["worker".to_string()]),
                    )]),
                    caller_provenance: None,
                    audit_invocation_id: Some("audit-1".to_string()),
                }),
                auth_binding: None,
                budget_limits: Some(meerkat_core::BudgetLimits {
                    max_tokens: Some(100_000),
                    max_duration: None,
                    max_tool_calls: Some(50),
                }),
                runtime_mode: meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven,
                continuity_intent: WireSpawnContinuityIntent::Ephemeral,
            },
            required_env_keys: Vec::new(),
        }
    }

    // T21 — decompile totality over the contracts fixture: every carried
    // field lands in its build destination.
    #[test]
    fn decompile_maps_every_portable_field() {
        let spec = sample_spec();
        let decompiled = decompile_portable_spec(&spec).expect("decompile");
        assert_eq!(decompiled.mob_id.as_str(), spec.mob_id);
        assert_eq!(decompiled.profile_name.as_str(), spec.profile_name);
        assert_eq!(decompiled.agent_identity.as_str(), spec.agent_identity);
        assert_eq!(decompiled.profile.model, spec.profile.model);
        assert_eq!(decompiled.profile.provider, Some(spec.profile.provider));
        assert_eq!(
            decompiled.profile.image_generation_provider,
            spec.profile.image_generation_provider
        );
        assert_eq!(
            decompiled.profile.auto_compact_threshold,
            spec.profile.auto_compact_threshold
        );
        assert_eq!(decompiled.profile.skills, spec.profile.skills);
        assert_eq!(
            decompiled.profile.tools.builtins,
            spec.profile.tools.builtins
        );
        assert_eq!(decompiled.profile.tools.mob, spec.profile.tools.mob);
        assert!(decompiled.profile.tools.mcp.is_empty());
        assert!(decompiled.profile.tools.rust_bundles.is_empty());
        assert_eq!(
            decompiled.profile.peer_description,
            spec.profile.peer_description
        );
        assert_eq!(decompiled.profile.backend, None);
        assert_eq!(
            decompiled.profile.max_inline_peer_notifications,
            spec.profile.max_inline_peer_notifications
        );
        assert_eq!(
            decompiled.definition.models.len(),
            spec.definition_extract.models.len()
        );
        assert_eq!(
            decompiled.definition.skills.len(),
            spec.definition_extract.skills.len()
        );
        for name in &spec.definition_extract.profile_names {
            assert!(
                decompiled
                    .definition
                    .profiles
                    .contains_key(&crate::ids::ProfileName::from(name.as_str()))
            );
        }
        assert_eq!(decompiled.labels, spec.overlay.labels);
        assert_eq!(
            decompiled.additional_instructions,
            spec.overlay.additional_instructions
        );
        assert_eq!(decompiled.budget_limits, spec.overlay.budget_limits);
        assert!(decompiled.mob_tool_authority_context.is_some());
        assert_eq!(decompiled.auth_binding, None);
    }

    // T21 — the resolved policy mapping never produces Inherit.
    #[test]
    fn decompiled_tool_access_policy_is_never_inherit() {
        let spec = sample_spec();
        let decompiled = decompile_portable_spec(&spec).expect("decompile");
        match decompiled.tool_access_policy {
            Some(meerkat_core::ops::ToolAccessPolicy::AllowList(names)) => {
                assert_eq!(names.len(), 1);
                assert!(names.contains("member_status"));
            }
            other => panic!("expected AllowList, got {other:?}"),
        }
    }

    // T21 — system prompt mapping: Set → Replace, Disable → Disable.
    #[test]
    fn decompiled_system_prompt_maps_set_and_disable() {
        let mut spec = sample_spec();
        let decompiled = decompile_portable_spec(&spec).expect("decompile");
        assert!(matches!(
            decompiled.system_prompt,
            DecompiledSystemPrompt::Replace(ref text) if text == "You are worker-1."
        ));
        spec.overlay.system_prompt = PortableSystemPrompt::Disable;
        let decompiled = decompile_portable_spec(&spec).expect("decompile");
        assert!(matches!(
            decompiled.system_prompt,
            DecompiledSystemPrompt::Disable
        ));
    }

    #[test]
    fn decompile_http_header_names_fail_closed() {
        let mut spec = sample_spec();
        spec.profile.tools.mcp_servers.insert(
            "remote".to_string(),
            PortableMcpDecl::Http {
                url: "https://mcp.example".to_string(),
                http_transport: None,
                required_header_names: vec!["authorization".to_string()],
                connect_timeout_secs: None,
            },
        );
        assert!(matches!(
            decompile_portable_spec(&spec),
            Err(MaterializeDecompileError::McpHeaderNamesUnsupported { .. })
        ));
    }

    #[test]
    fn decompile_preserves_sse_transport_and_nondefault_timeout() {
        let mut spec = sample_spec();
        spec.profile.tools.mcp_servers.insert(
            "remote".to_string(),
            PortableMcpDecl::Http {
                url: "https://mcp.example/events".to_string(),
                http_transport: Some(meerkat_core::mcp_config::McpHttpTransport::Sse),
                required_header_names: Vec::new(),
                connect_timeout_secs: Some(23),
            },
        );

        let decompiled = decompile_portable_spec(&spec).expect("SSE declaration decompiles");
        let server = decompiled
            .profile
            .tools
            .mcp_servers
            .iter()
            .find(|server| server.name == "remote")
            .expect("decompiled remote MCP server");
        assert_eq!(
            server.transport_kind(),
            meerkat_core::mcp_config::McpTransportKind::Sse
        );
        assert_eq!(server.connect_timeout_secs, Some(23));
    }

    #[test]
    fn stdio_command_path_walk_finds_absolute_and_path_entries() {
        assert!(!stdio_command_discoverable("/definitely/not/a/real/binary"));
        assert!(!stdio_command_discoverable("definitely-not-a-real-binary"));
    }
}
