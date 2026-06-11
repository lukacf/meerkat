//! Definition validation for mob definitions.
//!
//! Validates that all cross-references in a `MobDefinition` are consistent:
//! skill references resolve, orchestrator profile exists, wiring rules
//! reference valid profiles, and profile names are valid identifiers.

use crate::MobBackendKind;
use crate::definition::MobDefinition;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Diagnostic code for validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// A skill referenced by a profile is not defined in the skills section.
    MissingSkillRef,
    /// The orchestrator profile is not defined.
    MissingOrchestratorProfile,
    /// A profile name is not a valid identifier.
    InvalidProfileName,
    /// A wiring rule references a profile that does not exist.
    InvalidWiringProfile,
    /// Definition has no spawnable profiles.
    EmptyProfiles,
    /// External backend selected but config missing.
    MissingExternalBackendConfig,
    /// External backend config is invalid.
    InvalidExternalBackendConfig,
    /// A flow has cyclic dependencies.
    FlowCycleDetected,
    /// A flow dependency points to an unknown step.
    FlowUnknownStep,
    /// A flow step references an unknown role.
    FlowUnknownRole,
    /// Flow depth exceeds hard limit.
    FlowDepthExceeded,
    /// Topology references an unknown role.
    TopologyUnknownRole,
    /// Quorum policy is invalid.
    QuorumInvalid,
    /// Branch group has fewer than two steps.
    BranchGroupEmpty,
    /// Branch step is missing a condition.
    BranchStepMissingCondition,
    /// Branch steps in same group do not share dependency set.
    BranchStepConflictingDeps,
    /// `depends_on_mode = any` used without branch dependencies.
    BranchJoinWithoutBranch,
    /// Definition uses a reserved flow-system identifier.
    ReservedSystemIdentifier,
    /// Inline peer notification threshold is outside supported range.
    InvalidInlinePeerNotificationThreshold,
    /// A profile model is neither catalogued, custom-defined (`[models.<id>]`),
    /// nor provider-annotated.
    UnknownModel,
    /// A `[models.<id>]` entry declares a provider that cannot own a custom
    /// model (self-hosted or unknown).
    InvalidCustomModel,
    /// An `image_generation_provider` has no image-generation capability.
    InvalidImageGenerationProvider,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::MissingSkillRef => "missing_skill_ref",
            Self::MissingOrchestratorProfile => "missing_orchestrator_profile",
            Self::InvalidProfileName => "invalid_profile_name",
            Self::InvalidWiringProfile => "invalid_wiring_profile",
            Self::EmptyProfiles => "empty_profiles",
            Self::MissingExternalBackendConfig => "missing_external_backend_config",
            Self::InvalidExternalBackendConfig => "invalid_external_backend_config",
            Self::FlowCycleDetected => "flow_cycle_detected",
            Self::FlowUnknownStep => "flow_unknown_step",
            Self::FlowUnknownRole => "flow_unknown_role",
            Self::FlowDepthExceeded => "flow_depth_exceeded",
            Self::TopologyUnknownRole => "topology_unknown_role",
            Self::QuorumInvalid => "quorum_invalid",
            Self::BranchGroupEmpty => "branch_group_empty",
            Self::BranchStepMissingCondition => "branch_step_missing_condition",
            Self::BranchStepConflictingDeps => "branch_step_conflicting_deps",
            Self::BranchJoinWithoutBranch => "branch_join_without_branch",
            Self::ReservedSystemIdentifier => "reserved_system_identifier",
            Self::InvalidInlinePeerNotificationThreshold => {
                "invalid_inline_peer_notification_threshold"
            }
            Self::UnknownModel => "unknown_model",
            Self::InvalidCustomModel => "invalid_custom_model",
            Self::InvalidImageGenerationProvider => "invalid_image_generation_provider",
        };
        f.write_str(s)
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

/// A validation diagnostic with code, message, and optional location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Machine-readable diagnostic code.
    pub code: DiagnosticCode,
    /// Human-readable description.
    pub message: String,
    /// Optional path-like location (e.g. "profiles.worker.skills[0]").
    pub location: Option<String>,
    /// Severity of this diagnostic.
    pub severity: DiagnosticSeverity,
}

/// Validate a mob definition for consistency.
///
/// Returns an empty `Vec` if the definition is valid.
pub fn validate_definition(def: &MobDefinition) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if def.profiles.is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::EmptyProfiles,
            message: "mob definition must define at least one profile".to_string(),
            location: Some("profiles".to_string()),
            severity: DiagnosticSeverity::Error,
        });
    }

    // Check orchestrator profile exists
    if let Some(orch) = &def.orchestrator
        && !def.profiles.contains_key(&orch.profile)
    {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::MissingOrchestratorProfile,
            message: format!(
                "orchestrator profile '{}' is not defined",
                orch.profile.as_str()
            ),
            location: Some("mob.orchestrator".to_string()),
            severity: DiagnosticSeverity::Error,
        });
    }

    // Check profile names are valid identifiers and cross-references
    for (name, binding) in &def.profiles {
        // Validate profile name
        if !is_valid_identifier(name.as_str()) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidProfileName,
                message: format!("profile name '{}' is not a valid identifier", name.as_str()),
                location: Some(format!("profiles.{}", name.as_str())),
                severity: DiagnosticSeverity::Error,
            });
        }
        if crate::ids::AgentIdentity::from(name.as_str()).is_system_reserved() {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::ReservedSystemIdentifier,
                message: format!(
                    "profile name '{}' uses reserved flow-system identifier namespace",
                    name.as_str()
                ),
                location: Some(format!("profiles.{}", name.as_str())),
                severity: DiagnosticSeverity::Error,
            });
        }

        // RealmRef bindings are validated at resolution time; skip inner checks
        let Some(profile) = binding.as_inline() else {
            continue;
        };

        // Check skill references
        for (i, skill_ref) in profile.skills.iter().enumerate() {
            if !def.skills.contains_key(skill_ref) {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::MissingSkillRef,
                    message: format!("skill '{skill_ref}' is not defined"),
                    location: Some(format!("profiles.{}.skills[{}]", name.as_str(), i)),
                    severity: DiagnosticSeverity::Error,
                });
            }
        }

        // profile.tools.mcp is a free allowlist of host MCP source IDs (loaded
        // by the host's McpRouterAdapter from .rkat/mcp.toml). The mob
        // definition has no opinion on which sources exist; mismatched names
        // simply produce no tools at compose time. No mob-level validation.

        // Fail-fast model resolution: a profile model must be catalogued,
        // custom-defined in this definition's `[models.<id>]` tables, or
        // provider-annotated. Anything else would brick the member at first
        // delivery with "Cannot infer provider from model".
        let model_resolvable = profile.provider.is_some()
            || def.models.contains_key(&profile.model)
            || meerkat_models::catalog()
                .iter()
                .any(|entry| entry.id == profile.model);
        if !model_resolvable {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnknownModel,
                message: format!(
                    "profiles.{} model '{}' is neither catalogued, defined under [models.{}], \
                     nor provider-annotated (set `provider = \"...\"` or define the model)",
                    name.as_str(),
                    profile.model,
                    profile.model
                ),
                location: Some(format!("profiles.{}.model", name.as_str())),
                severity: DiagnosticSeverity::Error,
            });
        }

        if let Some(provider) = profile.image_generation_provider
            && meerkat_models::default_image_generation_model(provider).is_none()
        {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidImageGenerationProvider,
                message: format!(
                    "profiles.{} image_generation_provider '{}' has no image-generation \
                     capability",
                    name.as_str(),
                    provider.as_str()
                ),
                location: Some(format!(
                    "profiles.{}.image_generation_provider",
                    name.as_str()
                )),
                severity: DiagnosticSeverity::Error,
            });
        }

        if let Some(threshold) = profile.max_inline_peer_notifications
            && threshold < -1
        {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidInlinePeerNotificationThreshold,
                message: format!(
                    "profiles.{} max_inline_peer_notifications={} is invalid (allowed: -1, 0, or >0)",
                    name.as_str(),
                    threshold
                ),
                location: Some(format!(
                    "profiles.{}.max_inline_peer_notifications",
                    name.as_str()
                )),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    // Mob-level image generation default must point at a provider that can
    // actually generate images.
    if let Some(provider) = def.image_generation_provider
        && meerkat_models::default_image_generation_model(provider).is_none()
    {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::InvalidImageGenerationProvider,
            message: format!(
                "image_generation_provider '{}' has no image-generation capability",
                provider.as_str()
            ),
            location: Some("image_generation_provider".to_string()),
            severity: DiagnosticSeverity::Error,
        });
    }

    // `[models.<id>]` entries must declare a concrete API provider; the same
    // rule the model registry enforces at member build time, surfaced
    // fail-fast at mob load.
    for (model_id, model) in &def.models {
        if matches!(
            model.provider,
            meerkat_core::Provider::SelfHosted | meerkat_core::Provider::Other
        ) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidCustomModel,
                message: format!(
                    "models.{model_id}: provider must be a concrete API provider \
                     (anthropic, openai, gemini); self-hosted models belong in the host \
                     config's [self_hosted.models]"
                ),
                location: Some(format!("models.{model_id}.provider")),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    // Check wiring rules reference valid profiles
    for (i, rule) in def.wiring.role_wiring.iter().enumerate() {
        if !def.profiles.contains_key(&rule.a) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidWiringProfile,
                message: format!(
                    "wiring rule references non-existent profile '{}'",
                    rule.a.as_str()
                ),
                location: Some(format!("wiring.role_wiring[{i}].a")),
                severity: DiagnosticSeverity::Error,
            });
        }
        if !def.profiles.contains_key(&rule.b) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidWiringProfile,
                message: format!(
                    "wiring rule references non-existent profile '{}'",
                    rule.b.as_str()
                ),
                location: Some(format!("wiring.role_wiring[{i}].b")),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    let definition_uses_external_default = def.backend.default == MobBackendKind::External;
    let profile_uses_external = def
        .profiles
        .values()
        .filter_map(|b| b.as_inline())
        .any(|profile| profile.backend == Some(MobBackendKind::External));
    if definition_uses_external_default || profile_uses_external {
        match &def.backend.external {
            None => diagnostics.push(Diagnostic {
                code: DiagnosticCode::MissingExternalBackendConfig,
                message: "external backend selected but backend.external config is missing"
                    .to_string(),
                location: Some("backend.external".to_string()),
                severity: DiagnosticSeverity::Error,
            }),
            Some(external) if external.address_base.trim().is_empty() => {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::InvalidExternalBackendConfig,
                    message: "backend.external.address_base must not be empty".to_string(),
                    location: Some("backend.external.address_base".to_string()),
                    severity: DiagnosticSeverity::Error,
                });
            }
            Some(external) => {
                if let Some(supervisor_bridge) = &external.supervisor_bridge {
                    let mut bind_socket = None;
                    if let Some(bind_address) = supervisor_bridge.bind_address.as_deref() {
                        match bind_address.parse::<std::net::SocketAddr>() {
                            Ok(socket) => bind_socket = Some(socket),
                            Err(error) => diagnostics.push(Diagnostic {
                                code: DiagnosticCode::InvalidExternalBackendConfig,
                                message: format!(
                                    "backend.external.supervisor_bridge.bind_address must be a socket address: {error}"
                                ),
                                location: Some(
                                    "backend.external.supervisor_bridge.bind_address".to_string(),
                                ),
                                severity: DiagnosticSeverity::Error,
                            }),
                        }
                    }
                    if bind_socket.is_some_and(|socket| socket.ip().is_unspecified())
                        && supervisor_bridge.advertised_address.is_none()
                    {
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::InvalidExternalBackendConfig,
                            message: "backend.external.supervisor_bridge.advertised_address is required when bind_address uses an unspecified interface".to_string(),
                            location: Some(
                                "backend.external.supervisor_bridge.advertised_address"
                                    .to_string(),
                            ),
                            severity: DiagnosticSeverity::Error,
                        });
                    }
                    if let Some(advertised_address) =
                        supervisor_bridge.advertised_address.as_deref()
                    {
                        match meerkat_core::comms::PeerAddress::parse(advertised_address) {
                            Ok(address)
                                if address.transport()
                                    == meerkat_core::comms::PeerTransport::Tcp => {}
                            Ok(address) => diagnostics.push(Diagnostic {
                                code: DiagnosticCode::InvalidExternalBackendConfig,
                                message: format!(
                                    "backend.external.supervisor_bridge.advertised_address must use tcp transport, got '{}'",
                                    address.transport()
                                ),
                                location: Some(
                                    "backend.external.supervisor_bridge.advertised_address"
                                        .to_string(),
                                ),
                                severity: DiagnosticSeverity::Error,
                            }),
                            Err(error) => diagnostics.push(Diagnostic {
                                code: DiagnosticCode::InvalidExternalBackendConfig,
                                message: format!(
                                    "backend.external.supervisor_bridge.advertised_address must be a typed peer address: {error}"
                                ),
                                location: Some(
                                    "backend.external.supervisor_bridge.advertised_address"
                                        .to_string(),
                                ),
                                severity: DiagnosticSeverity::Error,
                            }),
                        }
                    }
                }
            }
        }
    }

    diagnostics
}

/// Split diagnostics into `(errors, warnings)`.
pub fn partition_diagnostics(
    diagnostics: impl IntoIterator<Item = Diagnostic>,
) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    diagnostics
        .into_iter()
        .partition(|diag| diag.severity == DiagnosticSeverity::Error)
}

/// Check if a string is a valid identifier (alphanumeric, hyphens, underscores).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must start with a letter or underscore
    let first = s.chars().next().unwrap_or(' ');
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    // Rest must be alphanumeric, hyphen, or underscore
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        BackendConfig, MobDefinition, OrchestratorConfig, RoleWiringRule, SkillSource, WiringRules,
    };
    use crate::ids::{MobId, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use std::collections::BTreeMap;

    fn base_profile() -> Profile {
        Profile {
            model: "claude-opus-4-8".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig::default(),
            peer_description: "test".to_string(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }
    }

    fn valid_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            ProfileBinding::Inline({
                let mut p = base_profile();
                p.skills = vec!["skill-a".to_string()];
                p.tools.mcp = vec!["server-a".to_string()];
                Box::new(p)
            }),
        );
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Box::new(base_profile())),
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "skill-a".to_string(),
            SkillSource::Inline {
                content: "You are a leader.".to_string(),
            },
        );

        let mut definition = MobDefinition::explicit("test-mob");
        definition.orchestrator = Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        });
        definition.profiles = profiles;
        definition.wiring = WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: vec![RoleWiringRule {
                a: ProfileName::from("lead"),
                b: ProfileName::from("worker"),
            }],
        };
        definition.skills = skills;
        definition
    }

    #[test]
    fn test_valid_definition_passes() {
        let diagnostics = validate_definition(&valid_definition());
        assert!(diagnostics.is_empty(), "unexpected: {diagnostics:?}");
    }

    #[test]
    fn test_empty_profiles_is_invalid() {
        let mut def = valid_definition();
        def.profiles.clear();
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::EmptyProfiles),
            "empty profile map must be rejected"
        );
    }

    #[test]
    fn test_missing_skill_ref() {
        let mut def = valid_definition();
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .as_inline_mut()
            .unwrap()
            .skills
            .push("nonexistent-skill".to_string());

        let diagnostics = validate_definition(&def);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::MissingSkillRef);
        assert!(diagnostics[0].message.contains("nonexistent-skill"));
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
    }

    #[test]
    fn test_unknown_mcp_source_does_not_error() {
        // profile.tools.mcp is a free allowlist of host MCP source IDs;
        // unknown names produce no tools at compose time but are not errors
        // at validation time (the mob has no opinion on host MCP catalog).
        let mut def = valid_definition();
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .as_inline_mut()
            .unwrap()
            .tools
            .mcp
            .push("nonexistent-mcp".to_string());

        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code != DiagnosticCode::MissingSkillRef
                    && !d.message.contains("nonexistent-mcp")),
            "validate_definition must not flag unknown MCP source IDs: {diagnostics:?}"
        );
    }

    #[test]
    fn test_missing_orchestrator_profile() {
        let mut def = valid_definition();
        def.orchestrator = Some(OrchestratorConfig {
            profile: ProfileName::from("nonexistent"),
        });

        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::MissingOrchestratorProfile)
        );
    }

    #[test]
    fn test_invalid_profile_name() {
        let mut def = valid_definition();
        def.profiles.insert(
            ProfileName::from("123-invalid"),
            ProfileBinding::Inline(Box::new(base_profile())),
        );

        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::InvalidProfileName)
        );
    }

    #[test]
    fn test_reserved_system_profile_name_rejected() {
        let mut def = valid_definition();
        def.profiles.insert(
            crate::runtime::flow_system_member_id().as_str().into(),
            ProfileBinding::Inline(Box::new(base_profile())),
        );

        let diagnostics = validate_definition(&def);
        assert!(diagnostics.iter().any(|d| {
            d.code == DiagnosticCode::ReservedSystemIdentifier
                && d.message.contains("reserved flow-system identifier")
        }));
    }

    #[test]
    fn test_invalid_wiring_profile() {
        let mut def = valid_definition();
        def.wiring.role_wiring.push(RoleWiringRule {
            a: ProfileName::from("nonexistent-a"),
            b: ProfileName::from("nonexistent-b"),
        });

        let diagnostics = validate_definition(&def);
        let wiring_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::InvalidWiringProfile)
            .collect();
        assert_eq!(wiring_diags.len(), 2);
    }

    #[test]
    fn test_multiple_errors() {
        let mut def = valid_definition();
        def.orchestrator = Some(OrchestratorConfig {
            profile: ProfileName::from("gone"),
        });
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .as_inline_mut()
            .unwrap()
            .skills
            .push("bad-skill".to_string());
        // profile.tools.mcp entries no longer trigger validation diagnostics —
        // they are a free allowlist of host MCP source IDs.
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .as_inline_mut()
            .unwrap()
            .tools
            .mcp
            .push("unknown-source".to_string());

        let diagnostics = validate_definition(&def);
        assert!(diagnostics.len() >= 2);
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("unknown-source")),
            "free MCP source allowlist must not produce diagnostics"
        );
    }

    #[test]
    fn test_external_backend_requires_external_config() {
        let mut def = valid_definition();
        def.backend.default = MobBackendKind::External;
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::MissingExternalBackendConfig)
        );
    }

    #[test]
    fn test_external_backend_rejects_empty_address_base() {
        let mut def = valid_definition();
        def.profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile exists")
            .as_inline_mut()
            .unwrap()
            .backend = Some(MobBackendKind::External);
        def.backend.external = Some(crate::definition::ExternalBackendConfig {
            address_base: "   ".to_string(),
            supervisor_bridge: None,
        });
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::InvalidExternalBackendConfig)
        );
    }

    #[test]
    fn test_external_backend_requires_advertised_supervisor_address_for_unspecified_bind() {
        let mut def = valid_definition();
        def.backend.default = MobBackendKind::External;
        def.backend.external = Some(crate::definition::ExternalBackendConfig {
            address_base: "https://backend.example.invalid/mesh".to_string(),
            supervisor_bridge: Some(crate::definition::SupervisorBridgeEndpointConfig {
                bind_address: Some("0.0.0.0:42000".to_string()),
                advertised_address: None,
            }),
        });
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics.iter().any(|d| {
                d.code == DiagnosticCode::InvalidExternalBackendConfig
                    && d.location.as_deref()
                        == Some("backend.external.supervisor_bridge.advertised_address")
            }),
            "unspecified bind address must not become advertised route truth"
        );
    }

    #[test]
    fn test_parse_and_validate_rejects_missing_external_backend_config() {
        let toml = r#"
[mob]
id = "mob-ext"

[backend]
default = "external"

[profiles.worker]
model = "claude-sonnet-4-5"
"#;
        let def = MobDefinition::from_toml(toml).expect("parse toml");
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::MissingExternalBackendConfig)
        );
    }

    #[test]
    fn test_invalid_inline_peer_notification_threshold_is_rejected() {
        let mut def = valid_definition();
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile")
            .as_inline_mut()
            .unwrap()
            .max_inline_peer_notifications = Some(-2);

        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics.iter().any(|d| {
                d.code == DiagnosticCode::InvalidInlinePeerNotificationThreshold
                    && d.location.as_deref() == Some("profiles.lead.max_inline_peer_notifications")
            }),
            "expected invalid inline threshold diagnostic"
        );
    }

    #[test]
    fn test_unknown_model_is_rejected_at_load() {
        let mut def = valid_definition();
        def.profiles.insert(
            ProfileName::from("custom"),
            ProfileBinding::Inline({
                let mut p = base_profile();
                p.model = "uncatalogued-model".to_string();
                Box::new(p)
            }),
        );
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics.iter().any(|d| {
                d.code == DiagnosticCode::UnknownModel
                    && d.severity == DiagnosticSeverity::Error
                    && d.location.as_deref() == Some("profiles.custom.model")
            }),
            "a model that is neither catalogued, custom-defined, nor provider-annotated \
             must fail at mob load: {diagnostics:?}"
        );
    }

    #[test]
    fn test_provider_annotated_model_passes_load_validation() {
        let mut def = valid_definition();
        def.profiles.insert(
            ProfileName::from("custom"),
            ProfileBinding::Inline({
                let mut p = base_profile();
                p.model = "uncatalogued-model".to_string();
                p.provider = Some(meerkat_core::Provider::Anthropic);
                Box::new(p)
            }),
        );
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code != DiagnosticCode::UnknownModel),
            "provider-annotated uncatalogued models are admissible: {diagnostics:?}"
        );
    }

    #[test]
    fn test_custom_defined_model_passes_load_validation() {
        let mut def = valid_definition();
        def.models.insert(
            "uncatalogued-model".to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: meerkat_core::Provider::OpenAI,
                display_name: None,
                context_window: None,
                max_output_tokens: None,
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        def.profiles.insert(
            ProfileName::from("custom"),
            ProfileBinding::Inline({
                let mut p = base_profile();
                p.model = "uncatalogued-model".to_string();
                Box::new(p)
            }),
        );
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code != DiagnosticCode::UnknownModel),
            "[models.<id>]-defined models are admissible: {diagnostics:?}"
        );
    }

    #[test]
    fn test_custom_model_with_non_concrete_provider_is_rejected() {
        let mut def = valid_definition();
        def.models.insert(
            "weird-model".to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: meerkat_core::Provider::SelfHosted,
                display_name: None,
                context_window: None,
                max_output_tokens: None,
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics.iter().any(|d| {
                d.code == DiagnosticCode::InvalidCustomModel
                    && d.location.as_deref() == Some("models.weird-model.provider")
            }),
            "self-hosted providers in [models.<id>] must be rejected: {diagnostics:?}"
        );
    }

    #[test]
    fn test_image_generation_provider_without_capability_is_rejected() {
        let mut def = valid_definition();
        def.image_generation_provider = Some(meerkat_core::Provider::Anthropic);
        def.profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile")
            .as_inline_mut()
            .unwrap()
            .image_generation_provider = Some(meerkat_core::Provider::Anthropic);

        let diagnostics = validate_definition(&def);
        let hits: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::InvalidImageGenerationProvider)
            .collect();
        assert_eq!(
            hits.len(),
            2,
            "both mob-level and profile-level Anthropic image providers must be rejected: {diagnostics:?}"
        );

        def.image_generation_provider = Some(meerkat_core::Provider::Gemini);
        def.profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile")
            .as_inline_mut()
            .unwrap()
            .image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code != DiagnosticCode::InvalidImageGenerationProvider),
            "image-capable providers must pass: {diagnostics:?}"
        );
    }

    #[test]
    fn test_valid_identifier_patterns() {
        assert!(is_valid_identifier("worker"));
        assert!(is_valid_identifier("lead_agent"));
        assert!(is_valid_identifier("agent-1"));
        assert!(is_valid_identifier("_private"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("123bad"));
        assert!(!is_valid_identifier("-start"));
        assert!(!is_valid_identifier("has space"));
    }

    #[test]
    fn test_diagnostic_serde_roundtrip() {
        let diag = Diagnostic {
            code: DiagnosticCode::MissingSkillRef,
            message: "skill 'foo' not found".to_string(),
            location: Some("profiles.worker.skills[0]".to_string()),
            severity: DiagnosticSeverity::Error,
        };
        let json = serde_json::to_string(&diag).unwrap();
        let parsed: Diagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, diag);
    }

    #[test]
    fn test_partition_diagnostics() {
        let diagnostics = vec![
            Diagnostic {
                code: DiagnosticCode::MissingSkillRef,
                message: "missing".to_string(),
                location: None,
                severity: DiagnosticSeverity::Error,
            },
            Diagnostic {
                code: DiagnosticCode::BranchJoinWithoutBranch,
                message: "warn".to_string(),
                location: None,
                severity: DiagnosticSeverity::Warning,
            },
        ];
        let (errors, warnings) = partition_diagnostics(diagnostics);
        assert_eq!(errors.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert_eq!(errors[0].severity, DiagnosticSeverity::Error);
        assert_eq!(warnings[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_minimal_valid_definition() {
        let def = MobDefinition::explicit("minimal");
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::EmptyProfiles),
            "minimal definition without profiles should fail validation"
        );
    }
}
