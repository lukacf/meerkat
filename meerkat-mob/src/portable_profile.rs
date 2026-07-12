//! Canonical portable-profile projection and rehydration.
//!
//! This neutral module is shared by live remote-spec compilation and durable
//! placed-carrier recovery. A carrier stores only whether an effective
//! override was present; the profile itself is rehydrated from the
//! digest-covered portable profile, so no second mutable profile copy exists.

use std::collections::{BTreeMap, HashMap};

use meerkat_contracts::wire::supervisor_bridge::WireOpaqueJson;
use meerkat_contracts::wire::{
    PortableMcpDecl, PortableProfile, PortableToolConfig, WireMobResumeOverrideField,
    WireMobRuntimeMode, WireNonPortableResourceKind,
};

use crate::profile::{Profile, ResumeOverrideField, ToolConfig};

pub(crate) fn project_portable_profile(
    profile: &Profile,
    runtime_mode: crate::MobRuntimeMode,
    models: &BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    agent_identity: &str,
    profile_name: &str,
    non_portable_disabled: Vec<WireNonPortableResourceKind>,
) -> Result<PortableProfile, String> {
    let provider = match profile.provider {
        Some(provider) => provider,
        None => models
            .get(&profile.model)
            .map(|custom| custom.provider)
            .or_else(|| meerkat_models::canonical().infer_provider(&profile.model))
            .ok_or_else(|| {
                format!(
                    "model '{}' has no resolvable provider for remote placement of '{agent_identity}' (set provider on profile '{profile_name}' or define models.{})",
                    profile.model, profile.model
                )
            })?,
    };

    let mut mcp_servers = BTreeMap::new();
    for server in &profile.tools.mcp_servers {
        let decl = match &server.transport {
            meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) => {
                if !stdio.env.is_empty() {
                    return Err(format!(
                        "MCP server '{}' carries stdio env values; secret-bearing values cannot enter a portable profile",
                        server.name
                    ));
                }
                PortableMcpDecl::Stdio {
                    command: stdio.command.clone(),
                    args: stdio.args.clone(),
                    required_env_keys: Vec::new(),
                    connect_timeout_secs: server.connect_timeout_secs.map(u64::from),
                }
            }
            meerkat_core::mcp_config::McpTransportConfig::Http(http) => {
                if !http.headers.is_empty() {
                    return Err(format!(
                        "MCP server '{}' carries HTTP header values; secret-bearing values cannot enter a portable profile",
                        server.name
                    ));
                }
                PortableMcpDecl::Http {
                    url: http.url.clone(),
                    http_transport: http.transport,
                    required_header_names: Vec::new(),
                    connect_timeout_secs: server.connect_timeout_secs.map(u64::from),
                }
            }
        };
        if mcp_servers.insert(server.name.clone(), decl).is_some() {
            return Err(format!(
                "profile '{profile_name}' declares duplicate MCP server '{}'",
                server.name
            ));
        }
    }

    Ok(PortableProfile {
        model: profile.model.clone(),
        provider,
        self_hosted_server_id: profile.self_hosted_server_id.clone(),
        image_generation_provider: profile.image_generation_provider,
        auto_compact_threshold: profile.auto_compact_threshold,
        resume_overrides: profile
            .resume_overrides
            .iter()
            .map(wire_resume_override_field)
            .collect(),
        skills: profile.skills.clone(),
        tools: PortableToolConfig {
            builtins: profile.tools.builtins,
            shell: profile.tools.shell,
            comms: profile.tools.comms,
            memory: profile.tools.memory,
            workgraph: profile.tools.workgraph,
            mob: profile.tools.mob,
            schedule: profile.tools.schedule,
            image_generation: profile.tools.image_generation,
            mcp_servers,
            non_portable_disabled,
        },
        peer_description: profile.peer_description.clone(),
        external_addressable: profile.external_addressable,
        runtime_mode: wire_runtime_mode(runtime_mode),
        max_inline_peer_notifications: profile.max_inline_peer_notifications,
        output_schema: profile
            .output_schema
            .as_ref()
            .map(|schema| WireOpaqueJson::from_value(schema.as_value())),
        provider_params: profile.provider_params.clone().map(Into::into),
    })
}

/// Deterministically rebuild the domain profile represented by a portable
/// profile. The rebuilt profile has an explicit provider and no local-only
/// backend, MCP allowlist, Rust bundle, or secret map.
/// Forward-projecting the result must reproduce the input exactly.
pub(crate) fn rehydrate_portable_profile(portable: &PortableProfile) -> Result<Profile, String> {
    if !portable.tools.non_portable_disabled.is_empty() {
        return Err("portable profile contains non-portable-disabled residue".to_string());
    }

    let mut mcp_servers = Vec::with_capacity(portable.tools.mcp_servers.len());
    for (name, declaration) in &portable.tools.mcp_servers {
        let server = match declaration {
            PortableMcpDecl::Stdio {
                command,
                args,
                required_env_keys,
                connect_timeout_secs,
            } => {
                if !required_env_keys.is_empty() {
                    return Err(format!(
                        "portable MCP server '{name}' requires env keys unsupported by carrier schema v1"
                    ));
                }
                let mut server = meerkat_core::mcp_config::McpServerConfig::stdio(
                    name.clone(),
                    command.clone(),
                    args.clone(),
                    HashMap::new(),
                );
                server.connect_timeout_secs = connect_timeout_secs
                    .map(|seconds| {
                        u32::try_from(seconds).map_err(|_| {
                            format!("portable MCP server '{name}' timeout exceeds u32")
                        })
                    })
                    .transpose()?;
                server
            }
            PortableMcpDecl::Http {
                url,
                http_transport,
                required_header_names,
                connect_timeout_secs,
            } => {
                if !required_header_names.is_empty() {
                    return Err(format!(
                        "portable MCP server '{name}' requires headers unsupported by carrier schema v1"
                    ));
                }
                meerkat_core::mcp_config::McpServerConfig {
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
                                format!("portable MCP server '{name}' timeout exceeds u32")
                            })
                        })
                        .transpose()?,
                }
            }
        };
        mcp_servers.push(server);
    }

    let output_schema = portable
        .output_schema
        .as_ref()
        .map(|schema| {
            schema
                .to_value()
                .map_err(|error| format!("portable output schema is invalid JSON: {error}"))
                .and_then(|value| {
                    meerkat_core::MeerkatSchema::new(value)
                        .map_err(|error| format!("portable output schema is invalid: {error}"))
                })
        })
        .transpose()?;

    Ok(Profile {
        model: portable.model.clone(),
        provider: Some(portable.provider),
        self_hosted_server_id: portable.self_hosted_server_id.clone(),
        image_generation_provider: portable.image_generation_provider,
        auto_compact_threshold: portable.auto_compact_threshold,
        resume_overrides: portable
            .resume_overrides
            .iter()
            .map(domain_resume_override_field)
            .collect(),
        skills: portable.skills.clone(),
        tools: ToolConfig {
            builtins: portable.tools.builtins,
            shell: portable.tools.shell,
            comms: portable.tools.comms,
            memory: portable.tools.memory,
            workgraph: portable.tools.workgraph,
            mob: portable.tools.mob,
            schedule: portable.tools.schedule,
            image_generation: portable.tools.image_generation,
            mcp: Vec::new(),
            mcp_servers,
            rust_bundles: Vec::new(),
        },
        peer_description: portable.peer_description.clone(),
        external_addressable: portable.external_addressable,
        backend: None,
        runtime_mode: domain_runtime_mode(portable.runtime_mode),
        max_inline_peer_notifications: portable.max_inline_peer_notifications,
        output_schema,
        provider_params: portable.provider_params.clone().map(Into::into),
    })
}

pub(crate) fn wire_runtime_mode(mode: crate::MobRuntimeMode) -> WireMobRuntimeMode {
    match mode {
        crate::MobRuntimeMode::AutonomousHost => WireMobRuntimeMode::AutonomousHost,
        crate::MobRuntimeMode::TurnDriven => WireMobRuntimeMode::TurnDriven,
    }
}

fn domain_runtime_mode(mode: WireMobRuntimeMode) -> crate::MobRuntimeMode {
    match mode {
        WireMobRuntimeMode::AutonomousHost => crate::MobRuntimeMode::AutonomousHost,
        WireMobRuntimeMode::TurnDriven => crate::MobRuntimeMode::TurnDriven,
    }
}

fn wire_resume_override_field(field: &ResumeOverrideField) -> WireMobResumeOverrideField {
    match field {
        ResumeOverrideField::Model => WireMobResumeOverrideField::Model,
        ResumeOverrideField::Provider => WireMobResumeOverrideField::Provider,
        ResumeOverrideField::ProviderParams => WireMobResumeOverrideField::ProviderParams,
    }
}

fn domain_resume_override_field(field: &WireMobResumeOverrideField) -> ResumeOverrideField {
    match field {
        WireMobResumeOverrideField::Model => ResumeOverrideField::Model,
        WireMobResumeOverrideField::Provider => ResumeOverrideField::Provider,
        WireMobResumeOverrideField::ProviderParams => ResumeOverrideField::ProviderParams,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn resolved_override_round_trips_through_portable_profile_without_catalog_inference() {
        let mut stdio = meerkat_core::mcp_config::McpServerConfig::stdio(
            "tools",
            "tool-server",
            vec!["--stdio".to_string()],
            HashMap::new(),
        );
        stdio.connect_timeout_secs = Some(7);
        let mut sse = meerkat_core::mcp_config::McpServerConfig::sse(
            "events",
            "https://mcp.example.invalid/events",
            HashMap::new(),
        );
        sse.connect_timeout_secs = Some(23);
        let profile = Profile {
            model: "override-model".to_string(),
            provider: Some(meerkat_core::Provider::Anthropic),
            self_hosted_server_id: None,
            image_generation_provider: Some(meerkat_core::Provider::OpenAI),
            auto_compact_threshold: std::num::NonZeroU64::new(1024),
            resume_overrides: vec![
                ResumeOverrideField::Model,
                ResumeOverrideField::ProviderParams,
            ],
            skills: vec!["review".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                workgraph: false,
                mob: true,
                schedule: false,
                image_generation: true,
                mcp: Vec::new(),
                mcp_servers: vec![stdio, sse],
                rust_bundles: Vec::new(),
            },
            peer_description: "review worker".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: Some(4),
            output_schema: Some(
                meerkat_core::MeerkatSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } }
                }))
                .expect("valid schema"),
            ),
            provider_params: None,
        };
        let models = BTreeMap::new();
        let portable = project_portable_profile(
            &profile,
            profile.runtime_mode,
            &models,
            "worker",
            "review",
            Vec::new(),
        )
        .expect("forward projection");
        assert_eq!(
            portable.tools.mcp_servers.get("events"),
            Some(&PortableMcpDecl::Http {
                url: "https://mcp.example.invalid/events".to_string(),
                http_transport: Some(meerkat_core::mcp_config::McpHttpTransport::Sse),
                required_header_names: Vec::new(),
                connect_timeout_secs: Some(23),
            })
        );
        let rehydrated = rehydrate_portable_profile(&portable).expect("reverse projection");
        assert_eq!(rehydrated.provider, Some(meerkat_core::Provider::Anthropic));
        assert_eq!(rehydrated.backend, None);
        assert!(rehydrated.tools.mcp.is_empty());
        assert!(rehydrated.tools.rust_bundles.is_empty());
        let rehydrated_sse = rehydrated
            .tools
            .mcp_servers
            .iter()
            .find(|server| server.name == "events")
            .expect("SSE declaration rehydrated");
        assert_eq!(
            rehydrated_sse.transport_kind(),
            meerkat_core::mcp_config::McpTransportKind::Sse
        );
        assert_eq!(rehydrated_sse.connect_timeout_secs, Some(23));
        let round_tripped = project_portable_profile(
            &rehydrated,
            rehydrated.runtime_mode,
            &models,
            "worker",
            "review",
            Vec::new(),
        )
        .expect("round-trip projection");
        assert_eq!(round_tripped, portable);
    }

    #[test]
    fn future_required_secret_names_fail_closed_in_v1_reverse_projection() {
        let mut portable = project_portable_profile(
            &Profile {
                model: "override-model".to_string(),
                provider: Some(meerkat_core::Provider::Anthropic),
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: String::new(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
            crate::MobRuntimeMode::AutonomousHost,
            &BTreeMap::new(),
            "worker",
            "review",
            Vec::new(),
        )
        .expect("projection");
        portable.tools.mcp_servers.insert(
            "future".to_string(),
            PortableMcpDecl::Http {
                url: "https://example.invalid/mcp".to_string(),
                http_transport: None,
                required_header_names: vec!["authorization".to_string()],
                connect_timeout_secs: None,
            },
        );
        assert!(rehydrate_portable_profile(&portable).is_err());
    }
}
