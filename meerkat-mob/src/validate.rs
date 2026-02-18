use std::collections::BTreeSet;

use regex::Regex;

use crate::definition::MobDefinition;
use crate::error::{Diagnostic, MobError};

pub fn validate(definition: &MobDefinition) -> Result<(), MobError> {
    let mut diagnostics = Vec::new();

    let ident = Regex::new(r"^[A-Za-z_][A-Za-z0-9_\-]{0,127}$").expect("valid regex");
    for profile_name in definition.profiles.keys() {
        if !ident.is_match(profile_name.as_str()) {
            diagnostics.push(Diagnostic {
                code: "invalid-profile-name".to_string(),
                message: format!("profile name '{profile_name}' is not valid"),
                location: Some(format!("profiles.{profile_name}")),
            });
        }
    }

    if let Some(orchestrator) = &definition.orchestrator
        && !definition.profiles.contains_key(&orchestrator.profile)
    {
        diagnostics.push(Diagnostic {
            code: "orchestrator-profile-missing".to_string(),
            message: format!("orchestrator profile '{}' does not exist", orchestrator.profile),
            location: Some("orchestrator.profile".to_string()),
        });
    }

    let known_profiles = definition.profiles.keys().cloned().collect::<BTreeSet<_>>();
    for pair in &definition.wiring.role_wiring {
        if !known_profiles.contains(&pair.a) || !known_profiles.contains(&pair.b) {
            diagnostics.push(Diagnostic {
                code: "wiring-profile-missing".to_string(),
                message: format!("wire pair '{}' <-> '{}' references unknown profile", pair.a, pair.b),
                location: Some("wiring.role_wiring".to_string()),
            });
        }
    }

    for (name, source) in &definition.skills {
        if source.is_empty() {
            diagnostics.push(Diagnostic {
                code: "skill-empty".to_string(),
                message: format!("skill '{name}' has no content"),
                location: Some(format!("skills.{name}")),
            });
        }
    }

    for (profile_name, profile) in &definition.profiles {
        for skill in &profile.skills {
            if !definition.skills.contains_key(skill) {
                diagnostics.push(Diagnostic {
                    code: "missing-skill".to_string(),
                    message: format!("profile '{profile_name}' references missing skill '{skill}'"),
                    location: Some(format!("profiles.{profile_name}.skills")),
                });
            }
        }
        for server in &profile.tools.mcp {
            if !definition.mcp_servers.contains_key(server) {
                diagnostics.push(Diagnostic {
                    code: "missing-mcp-server".to_string(),
                    message: format!("profile '{profile_name}' references missing mcp server '{server}'"),
                    location: Some(format!("profiles.{profile_name}.tools.mcp")),
                });
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(MobError::DefinitionError { diagnostics });
    }

    Ok(())
}

trait SkillSourceExt {
    fn is_empty(&self) -> bool;
}

impl SkillSourceExt for crate::definition::SkillSource {
    fn is_empty(&self) -> bool {
        match self {
            crate::definition::SkillSource::Inline(value) => value.trim().is_empty(),
            crate::definition::SkillSource::File { file } => file.trim().is_empty(),
        }
    }
}
