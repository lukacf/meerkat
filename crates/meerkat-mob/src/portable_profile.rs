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

/// The provider a profile's build resolves to: its declared provider, else the
/// mob's own entry for its model, else the model catalog. `None` when none of
/// them names one.
pub(crate) fn resolved_profile_provider(
    profile: &Profile,
    models: &BTreeMap<String, meerkat_core::config::CustomModelConfig>,
) -> Option<meerkat_core::Provider> {
    profile.provider.or_else(|| {
        models
            .get(&profile.model)
            .map(|custom| custom.provider)
            .or_else(|| meerkat_models::canonical().infer_provider(&profile.model))
    })
}

pub(crate) fn project_portable_profile(
    profile: &Profile,
    runtime_mode: crate::MobRuntimeMode,
    models: &BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    agent_identity: &str,
    profile_name: &str,
    non_portable_disabled: Vec<WireNonPortableResourceKind>,
) -> Result<PortableProfile, String> {
    let provider = resolved_profile_provider(profile, models).ok_or_else(|| {
        format!(
            "model '{}' has no resolvable provider for remote placement of '{agent_identity}' (set provider on profile '{profile_name}' or define models.{})",
            profile.model, profile.model
        )
    })?;

    let mut mcp_servers = BTreeMap::new();
    for server in &profile.tools.mcp_servers {
        // The access ceiling travels exactly. An invalid one is refused,
        // never stripped: dropping it would widen the server on the host.
        server
            .validate_access()
            .map_err(|error| error.to_string())?;
        // A connector binding is host configuration with no exact portable
        // form; dropping it would change what the server's calls carry.
        if server.connector.is_some() {
            return Err(format!(
                "MCP server '{}' has a connector binding that a portable profile cannot carry",
                server.name
            ));
        }
        // An account declaration is host configuration too; dropping it
        // would change how review states the server's operations.
        if server.account.is_some() {
            return Err(format!(
                "MCP server '{}' has an account declaration that a portable profile cannot carry",
                server.name
            ));
        }
        let decl = match &server.transport {
            meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) => {
                if !stdio.env.is_empty() {
                    return Err(format!(
                        "MCP server '{}' carries stdio env values; secret-bearing values cannot enter a portable profile",
                        server.name
                    ));
                }
                // A working directory is a path on this host: a portable
                // profile has no exact form for it, and dropping it would
                // change where the server runs.
                if stdio.cwd.is_some() {
                    return Err(format!(
                        "MCP server '{}' has a working directory that a portable profile cannot carry",
                        server.name
                    ));
                }
                PortableMcpDecl::Stdio {
                    command: stdio.command.clone(),
                    args: stdio.args.clone(),
                    required_env_keys: Vec::new(),
                    connect_timeout_secs: server.connect_timeout_secs.map(u64::from),
                    tool_names: server.tool_names.clone(),
                    access: server.access.clone(),
                }
            }
            meerkat_core::mcp_config::McpTransportConfig::Http(http) => {
                if !http.headers.is_empty() {
                    return Err(format!(
                        "MCP server '{}' carries HTTP header values; secret-bearing values cannot enter a portable profile",
                        server.name
                    ));
                }
                // A Discover or unverified account selection is host
                // configuration on the placement host; a portable profile
                // cannot carry it, and dropping it would change the mode.
                if http.oauth_account_selection.is_some() {
                    return Err(format!(
                        "MCP server '{}' has an OAuth account selection that a portable profile cannot carry",
                        server.name
                    ));
                }
                PortableMcpDecl::Http {
                    url: http.url.clone(),
                    http_transport: http.transport,
                    oauth_account: http.oauth_account.clone(),
                    required_header_names: Vec::new(),
                    connect_timeout_secs: server.connect_timeout_secs.map(u64::from),
                    tool_names: server.tool_names.clone(),
                    access: server.access.clone(),
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
        model_fallback: profile.model_fallback.clone(),
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
            read_only: profile.tools.read_only,
            deny: profile.tools.deny.clone(),
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
        durable: profile.durable,
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
                tool_names,
                access,
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
                server.tool_names = tool_names.clone();
                server.access = access.clone();
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
                oauth_account,
                required_header_names,
                connect_timeout_secs,
                tool_names,
                access,
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
                            oauth_account: oauth_account.clone(),
                            oauth_account_selection: None,
                        },
                    ),
                    tool_names: tool_names.clone(),
                    access: access.clone(),
                    // Portable HTTP servers never carry a connector binding.
                    connector: None,
                    account: None,
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
        // A carried ceiling must be valid; it is refused, never stripped.
        server
            .validate_access()
            .map_err(|error| error.to_string())?;
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
        model_fallback: portable.model_fallback.clone(),
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
            read_only: portable.tools.read_only,
            deny: portable.tools.deny.clone(),
            mcp: Vec::new(),
            mcp_servers,
            rust_bundles: Vec::new(),
            bundles: Vec::new(),
        },
        peer_description: portable.peer_description.clone(),
        external_addressable: portable.external_addressable,
        backend: None,
        runtime_mode: domain_runtime_mode(portable.runtime_mode),
        max_inline_peer_notifications: portable.max_inline_peer_notifications,
        output_schema,
        provider_params: portable.provider_params.clone().map(Into::into),
        durable: portable.durable,
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
        stdio
            .tool_names
            .insert("raw_stdio".into(), "home_stdio".into());
        let mut sse = meerkat_core::mcp_config::McpServerConfig::sse(
            "events",
            "https://mcp.example.invalid/events",
            HashMap::new(),
        );
        sse.connect_timeout_secs = Some(23);
        let mut selected = meerkat_core::mcp_config::McpServerConfig::streamable_http(
            "account-tools",
            "https://mcp.example.invalid/mcp",
            HashMap::new(),
        );
        selected
            .tool_names
            .insert("search".into(), "home_search".into());
        if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut selected.transport {
            http.oauth_account = Some("provider-subject-42".into());
        }
        let profile = Profile {
            model_fallback: Some(meerkat_core::config::ModelFallbackConfig {
                enabled: Some(false),
                ..Default::default()
            }),
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
                read_only: false,
                deny: vec!["mob_wire".to_string()],
                mcp: Vec::new(),
                mcp_servers: vec![stdio, sse, selected.clone()],
                rust_bundles: Vec::new(),
                bundles: Vec::new(),
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
            durable: true,
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
        assert!(portable.durable, "the durable worker flag is carried");
        assert_eq!(
            portable.tools.mcp_servers.get("events"),
            Some(&PortableMcpDecl::Http {
                url: "https://mcp.example.invalid/events".to_string(),
                http_transport: Some(meerkat_core::mcp_config::McpHttpTransport::Sse),
                oauth_account: None,
                required_header_names: Vec::new(),
                connect_timeout_secs: Some(23),
                tool_names: BTreeMap::new(),
                access: None,
            })
        );
        let rehydrated = rehydrate_portable_profile(&portable).expect("reverse projection");
        assert_eq!(rehydrated.provider, Some(meerkat_core::Provider::Anthropic));
        assert!(rehydrated.durable, "the durable worker flag rehydrates");
        for expected in &profile.tools.mcp_servers {
            assert!(rehydrated.tools.mcp_servers.contains(expected));
        }
        assert_eq!(
            rehydrated.tools.mcp_servers.len(),
            profile.tools.mcp_servers.len()
        );
        assert_eq!(portable.tools.deny, profile.tools.deny);
        assert_eq!(rehydrated.tools.deny, profile.tools.deny);
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
        assert_eq!(
            portable.tools.mcp_servers.get("account-tools"),
            Some(&PortableMcpDecl::Http {
                url: "https://mcp.example.invalid/mcp".into(),
                http_transport: None,
                oauth_account: Some("provider-subject-42".into()),
                required_header_names: Vec::new(),
                connect_timeout_secs: None,
                tool_names: selected.tool_names.clone(),
                access: None,
            })
        );
        assert_eq!(
            rehydrated
                .tools
                .mcp_servers
                .iter()
                .find(|server| server.name == "account-tools"),
            Some(&selected)
        );
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
                model_fallback: None,
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
                durable: false,
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
                oauth_account: None,
                required_header_names: vec!["authorization".to_string()],
                connect_timeout_secs: None,
                tool_names: BTreeMap::new(),
                access: None,
            },
        );
        assert!(rehydrate_portable_profile(&portable).is_err());
    }

    #[test]
    fn mcp_account_selection_fails_closed_in_portable_projection() {
        for selection in [
            meerkat_core::mcp_config::McpOAuthAccountSelection::Discover,
            meerkat_core::mcp_config::McpOAuthAccountSelection::Unverified,
        ] {
            let mut server = meerkat_core::mcp_config::McpServerConfig::streamable_http(
                "selected",
                "https://mcp.example.invalid/mcp",
                HashMap::new(),
            );
            if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut server.transport
            {
                http.oauth_account_selection = Some(selection);
            }
            let mut tools = ToolConfig::default();
            tools.mcp_servers.push(server);
            let error = project_portable_profile(
                &Profile {
                    model_fallback: None,
                    model: "override-model".to_string(),
                    provider: Some(meerkat_core::Provider::Anthropic),
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: Vec::new(),
                    tools,
                    peer_description: String::new(),
                    external_addressable: false,
                    backend: None,
                    runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                    durable: false,
                },
                crate::MobRuntimeMode::AutonomousHost,
                &BTreeMap::new(),
                "worker",
                "review",
                Vec::new(),
            )
            .expect_err("a portable profile never strips an account selection");
            assert!(error.contains("OAuth account selection"), "{error}");
        }
    }

    fn access_profile(servers: Vec<meerkat_core::mcp_config::McpServerConfig>) -> Profile {
        let tools = ToolConfig {
            mcp_servers: servers,
            ..ToolConfig::default()
        };
        Profile {
            model_fallback: None,
            model: "override-model".to_string(),
            provider: Some(meerkat_core::Provider::Anthropic),
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools,
            peer_description: String::new(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        }
    }

    fn project(profile: &Profile) -> Result<PortableProfile, String> {
        project_portable_profile(
            profile,
            crate::MobRuntimeMode::AutonomousHost,
            &BTreeMap::new(),
            "worker",
            "review",
            Vec::new(),
        )
    }

    /// An exact access ceiling travels through the portable profile on both
    /// transports with its raw tool-name mappings, and still refuses an
    /// operation outside it after rehydration. No ceiling stays no ceiling,
    /// and its absence is not serialized.
    #[test]
    fn mcp_access_ceiling_round_trips_exactly() {
        let mut stdio = meerkat_core::mcp_config::McpServerConfig::stdio(
            "ceiled-stdio",
            "/bin/true",
            Vec::new(),
            HashMap::new(),
        );
        stdio.tool_names = BTreeMap::from([("read".to_string(), "docs_read".to_string())]);
        stdio.access = Some(meerkat_core::McpServerAccess::allowing(["read", "list"]));
        let mut http = meerkat_core::mcp_config::McpServerConfig::streamable_http(
            "ceiled-http",
            "https://mcp.example.invalid/mcp",
            HashMap::new(),
        );
        http.access = Some(meerkat_core::McpServerAccess::allowing(["search"]));
        let open = meerkat_core::mcp_config::McpServerConfig::streamable_http(
            "open-http",
            "https://mcp.example.invalid/open",
            HashMap::new(),
        );
        let profile = access_profile(vec![stdio.clone(), http.clone(), open.clone()]);

        let portable = project(&profile).expect("a valid ceiling projects");
        assert!(matches!(
            portable.tools.mcp_servers.get("ceiled-stdio"),
            Some(PortableMcpDecl::Stdio { access, tool_names, .. })
                if access == &stdio.access && tool_names == &stdio.tool_names
        ));
        assert!(matches!(
            portable.tools.mcp_servers.get("ceiled-http"),
            Some(PortableMcpDecl::Http { access, .. }) if access == &http.access
        ));
        let open_json =
            serde_json::to_value(portable.tools.mcp_servers.get("open-http")).expect("serialize");
        assert!(
            open_json.get("access").is_none(),
            "no ceiling is omitted, keeping the digest-covered bytes: {open_json}"
        );

        let rehydrated = rehydrate_portable_profile(&portable).expect("rehydrate");
        for expected in [&stdio, &http, &open] {
            let server = rehydrated
                .tools
                .mcp_servers
                .iter()
                .find(|server| server.name == expected.name)
                .expect("server rehydrated");
            assert_eq!(server, expected);
        }
        let ceiled = rehydrated
            .tools
            .mcp_servers
            .iter()
            .find(|server| server.name == "ceiled-stdio")
            .expect("ceiled server");
        assert!(ceiled.permits_operation("read"));
        assert!(ceiled.permits_operation("list"));
        assert!(
            !ceiled.permits_operation("write"),
            "an operation outside the ceiling stays refused"
        );
        assert_eq!(project(&rehydrated).expect("re-project"), portable);
    }

    /// An invalid ceiling is refused on both sides, never stripped to none.
    #[test]
    fn invalid_mcp_access_ceiling_is_refused_not_stripped() {
        let empty = meerkat_core::McpServerAccess::allowing(Vec::<String>::new());
        let mut server = meerkat_core::mcp_config::McpServerConfig::streamable_http(
            "ceiled-http",
            "https://mcp.example.invalid/mcp",
            HashMap::new(),
        );
        server.access = Some(empty.clone());
        let error = project(&access_profile(vec![server])).expect_err("invalid ceiling");
        assert!(error.contains("invalid access policy"), "{error}");

        let mut portable = project(&access_profile(Vec::new())).expect("projection");
        portable.tools.mcp_servers.insert(
            "carried".to_string(),
            PortableMcpDecl::Stdio {
                command: "/bin/true".to_string(),
                args: Vec::new(),
                required_env_keys: Vec::new(),
                connect_timeout_secs: None,
                tool_names: BTreeMap::new(),
                access: Some(empty),
            },
        );
        let error = rehydrate_portable_profile(&portable).expect_err("invalid carried ceiling");
        assert!(error.contains("invalid access policy"), "{error}");
    }

    /// A connector-bound server is refused with or without a valid ceiling:
    /// the binding has no portable form and is never dropped. The portable
    /// declaration has no connector field, so the host decompile input cannot
    /// name one either.
    #[test]
    fn connector_bound_mcp_server_is_refused_in_portable_projection() {
        for access in [
            None,
            Some(meerkat_core::McpServerAccess::allowing(["read"])),
        ] {
            let mut server = meerkat_core::mcp_config::McpServerConfig::stdio(
                "connector-stdio",
                "/bin/true",
                Vec::new(),
                HashMap::new(),
            );
            server.access = access;
            server.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
                realm_id: "realm".to_string(),
                slot_id: "slot".to_string(),
                issuer: "https://issuer.example.invalid".to_string(),
                client: "client".to_string(),
                resource: "https://api.example.invalid".to_string(),
                strategy_id: "strategy".to_string(),
                account: "account-subject".to_string(),
                operations: BTreeMap::from([(
                    "read".to_string(),
                    meerkat_core::mcp_config::McpConnectorOperation {
                        required_scopes: Default::default(),
                        destination_origin: "https://api.example.invalid".to_string(),
                        read_only: true,
                    },
                )]),
            });
            let error = project(&access_profile(vec![server])).expect_err("connector-bound");
            assert!(error.contains("connector binding"), "{error}");
        }
    }

    /// A server whose host declares the account its operations act with is
    /// refused: the declaration is host configuration and is never dropped,
    /// which would change how review states its operations.
    #[test]
    fn an_account_declaration_is_refused_in_portable_projection() {
        let mut server = meerkat_core::mcp_config::McpServerConfig::stdio(
            "declared",
            "/bin/true",
            Vec::new(),
            HashMap::new(),
        );
        server.account = Some(meerkat_core::McpAccountDeclaration::NoAccount);
        let error = project(&access_profile(vec![server])).expect_err("declared");
        assert!(error.contains("account declaration"), "{error}");
    }

    /// A stdio server with a working directory is refused: the directory is
    /// a path on this host with no portable form, and dropping it would
    /// change where the server runs. (Fails-old: the projection dropped it.)
    #[test]
    fn a_stdio_working_directory_is_refused_in_portable_projection() {
        let placed = meerkat_core::mcp_config::McpServerConfig::stdio(
            "placed",
            "/opt/placed/bin/server",
            Vec::new(),
            HashMap::new(),
        );
        let mut with_cwd = placed.clone();
        if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut with_cwd.transport
        {
            stdio.cwd = Some("/opt/placed".to_owned());
        }
        let error = project(&access_profile(vec![with_cwd])).expect_err("a working directory");
        assert!(error.contains("working directory"), "{error}");
        assert!(
            project(&access_profile(vec![placed])).is_ok(),
            "without one the server projects as before"
        );
    }

    /// The declaration rejects fields it does not know, so a reader that
    /// predates a field refuses it instead of silently widening.
    #[test]
    fn portable_mcp_declaration_rejects_unknown_fields() {
        let error = serde_json::from_value::<PortableMcpDecl>(serde_json::json!({
            "transport": "stdio",
            "command": "/bin/true",
            "access": {"allowed_operations": ["read"]},
            "future_field": true,
        }))
        .expect_err("unknown field");
        assert!(error.to_string().contains("future_field"), "{error}");
    }
}
