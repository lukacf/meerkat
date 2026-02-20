//! Definition validation for mob definitions.
//!
//! Validates that all cross-references in a `MobDefinition` are consistent:
//! skill references resolve, MCP references exist, orchestrator profile exists,
//! wiring rules reference valid profiles, and profile names are valid identifiers.

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
    /// An MCP server referenced by a profile is not defined in the mcp section.
    MissingMcpRef,
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
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::MissingSkillRef => "missing_skill_ref",
            Self::MissingMcpRef => "missing_mcp_ref",
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
    for (name, profile) in &def.profiles {
        // Validate profile name
        if !is_valid_identifier(name.as_str()) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::InvalidProfileName,
                message: format!("profile name '{}' is not a valid identifier", name.as_str()),
                location: Some(format!("profiles.{}", name.as_str())),
                severity: DiagnosticSeverity::Error,
            });
        }

        // Check skill references
        for (i, skill_ref) in profile.skills.iter().enumerate() {
            if !def.skills.contains_key(skill_ref) {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::MissingSkillRef,
                    message: format!("skill '{}' is not defined", skill_ref),
                    location: Some(format!("profiles.{}.skills[{}]", name.as_str(), i)),
                    severity: DiagnosticSeverity::Error,
                });
            }
        }

        // Check MCP server references
        for (i, mcp_ref) in profile.tools.mcp.iter().enumerate() {
            if !def.mcp_servers.contains_key(mcp_ref) {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::MissingMcpRef,
                    message: format!("MCP server '{}' is not defined", mcp_ref),
                    location: Some(format!("profiles.{}.tools.mcp[{}]", name.as_str(), i)),
                    severity: DiagnosticSeverity::Error,
                });
            }
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
                location: Some(format!("wiring.role_wiring[{}].a", i)),
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
                location: Some(format!("wiring.role_wiring[{}].b", i)),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    let definition_uses_external_default = def.backend.default == MobBackendKind::External;
    let profile_uses_external = def
        .profiles
        .iter()
        .any(|(_, profile)| profile.backend == Some(MobBackendKind::External));
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
                })
            }
            Some(_) => {}
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
        BackendConfig, McpServerConfig, MobDefinition, OrchestratorConfig, RoleWiringRule,
        SkillSource, WiringRules,
    };
    use crate::ids::{MobId, ProfileName};
    use crate::profile::{Profile, ToolConfig};
    use std::collections::BTreeMap;

    fn base_profile() -> Profile {
        Profile {
            model: "claude-opus-4-6".to_string(),
            skills: vec![],
            tools: ToolConfig::default(),
            peer_description: "test".to_string(),
            external_addressable: false,
            backend: None,
        }
    }

    fn valid_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("lead"), {
            let mut p = base_profile();
            p.skills = vec!["skill-a".to_string()];
            p.tools.mcp = vec!["server-a".to_string()];
            p
        });
        profiles.insert(ProfileName::from("worker"), base_profile());

        let mut skills = BTreeMap::new();
        skills.insert(
            "skill-a".to_string(),
            SkillSource::Inline {
                content: "You are a leader.".to_string(),
            },
        );

        let mut mcp_servers = BTreeMap::new();
        mcp_servers.insert(
            "server-a".to_string(),
            McpServerConfig {
                command: vec!["node".to_string(), "server.js".to_string()],
                url: None,
                env: BTreeMap::new(),
            },
        );

        MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers,
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring: vec![RoleWiringRule {
                    a: ProfileName::from("lead"),
                    b: ProfileName::from("worker"),
                }],
            },
            skills,
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
        }
    }

    #[test]
    fn test_valid_definition_passes() {
        let diagnostics = validate_definition(&valid_definition());
        assert!(diagnostics.is_empty(), "unexpected: {:?}", diagnostics);
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
            .skills
            .push("nonexistent-skill".to_string());

        let diagnostics = validate_definition(&def);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::MissingSkillRef);
        assert!(diagnostics[0].message.contains("nonexistent-skill"));
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
    }

    #[test]
    fn test_missing_mcp_ref() {
        let mut def = valid_definition();
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .tools
            .mcp
            .push("nonexistent-mcp".to_string());

        let diagnostics = validate_definition(&def);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, DiagnosticCode::MissingMcpRef);
        assert!(diagnostics[0].message.contains("nonexistent-mcp"));
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
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
        def.profiles
            .insert(ProfileName::from("123-invalid"), base_profile());

        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::InvalidProfileName)
        );
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
            .skills
            .push("bad-skill".to_string());
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .unwrap()
            .tools
            .mcp
            .push("bad-mcp".to_string());

        let diagnostics = validate_definition(&def);
        assert!(diagnostics.len() >= 3);
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
            .backend = Some(MobBackendKind::External);
        def.backend.external = Some(crate::definition::ExternalBackendConfig {
            address_base: "   ".to_string(),
        });
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::InvalidExternalBackendConfig)
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
        let def = MobDefinition {
            id: MobId::from("minimal"),
            orchestrator: None,
            profiles: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
        };
        let diagnostics = validate_definition(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::EmptyProfiles),
            "minimal definition without profiles should fail validation"
        );
    }
}
