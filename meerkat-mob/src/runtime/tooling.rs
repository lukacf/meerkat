use super::*;

impl MobRuntime {
    pub(super) async fn compile_role_session_request(
        &self,
        input: CompileRoleSessionInput<'_>,
    ) -> MobResult<CreateSessionRequest> {
        let CompileRoleSessionInput {
            spec,
            role_name,
            role,
            labels,
            comms_name,
            namespace,
            session,
        } = input;

        let role_tools = self.resolve_role_tooling(spec, role).await?;
        let peer_meta = labels
            .iter()
            .fold(PeerMeta::default(), |meta, (key, value)| {
                meta.with_label(key.clone(), value.clone())
            })
            .with_description(format!(
                "Mob meerkat '{}' role '{}'",
                labels
                    .get("meerkat_id")
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string()),
                role_name
            ));

        Ok(CreateSessionRequest {
            model: role
                .model
                .clone()
                .unwrap_or_else(|| self.default_model.clone()),
            prompt: String::new(),
            system_prompt: Some(role.prompt.clone()),
            max_tokens: None,
            event_tx: None,
            host_mode: true,
            skill_references: None,
            build: Some(SessionBuildOptions {
                comms_name: Some(comms_name.to_string()),
                peer_meta: Some(peer_meta),
                resume_session: Some(session),
                external_tools: role_tools.external_tools,
                override_builtins: Some(role_tools.enable_builtins),
                override_shell: Some(role_tools.enable_shell),
                override_subagents: Some(role_tools.enable_subagents),
                override_memory: Some(role_tools.enable_memory),
                preload_skills: Some(
                    role.preload_skills
                        .iter()
                        .map(|value| meerkat_core::skills::SkillId(value.clone()))
                        .collect(),
                ),
                realm_id: Some(namespace),
                ..SessionBuildOptions::default()
            }),
        })
    }

    pub(super) async fn resolve_role_tooling(
        &self,
        spec: &MobSpec,
        role: &RoleSpec,
    ) -> MobResult<RoleTooling> {
        let mut enable_builtins = false;
        let mut enable_shell = false;
        let mut enable_subagents = false;
        let mut enable_memory = false;

        let mut external_dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();

        for bundle_id in &role.tool_bundles {
            let bundle = spec.tool_bundles.get(bundle_id).ok_or_else(|| {
                MobError::ToolBundle(format!("unknown tool bundle '{bundle_id}'"))
            })?;

            match bundle {
                crate::model::ToolBundleSpec::Builtin {
                    enable_builtins: b,
                    enable_shell: s,
                    enable_subagents: sa,
                    enable_memory: m,
                } => {
                    enable_builtins |= *b;
                    enable_shell |= *s;
                    enable_subagents |= *sa;
                    enable_memory |= *m;
                }
                crate::model::ToolBundleSpec::Mcp {
                    servers,
                    unavailable,
                } => match self.build_mcp_dispatcher(servers).await {
                    Ok(dispatcher) => external_dispatchers.push(dispatcher),
                    Err(err) => {
                        if matches!(unavailable, UnavailablePolicy::FailActivation) {
                            return Err(err);
                        }
                        self.emit_event(
                            self.event(
                                spec.mob_id.clone(),
                                MobEventCategory::Supervisor,
                                MobEventKind::Warning,
                            )
                            .payload(json!({"warning": err.to_string()}))
                            .build(),
                        )
                        .await?;
                    }
                },
                crate::model::ToolBundleSpec::RustBundle {
                    bundle_id,
                    unavailable,
                } => match self.rust_bundles.get(bundle_id).await {
                    Some(dispatcher) => external_dispatchers.push(dispatcher),
                    None => {
                        if matches!(unavailable, UnavailablePolicy::FailActivation) {
                            return Err(MobError::ToolBundle(format!(
                                "missing rust tool bundle '{bundle_id}'"
                            )));
                        }
                        self.emit_event(
                            self.event(
                                spec.mob_id.clone(),
                                MobEventCategory::Supervisor,
                                MobEventKind::Warning,
                            )
                            .payload(json!({"warning": format!("missing rust tool bundle '{bundle_id}'")}))
                            .build(),
                        )
                        .await?;
                    }
                },
            }
        }

        let external_tools = if external_dispatchers.is_empty() {
            None
        } else if external_dispatchers.len() == 1 {
            external_dispatchers.into_iter().next()
        } else {
            let mut builder = ToolGatewayBuilder::new();
            for dispatcher in external_dispatchers {
                builder = builder.add_dispatcher(dispatcher);
            }
            let gateway = builder
                .build()
                .map_err(|err| MobError::ToolBundle(err.to_string()))?;
            Some(Arc::new(gateway) as Arc<dyn AgentToolDispatcher>)
        };

        Ok(RoleTooling {
            enable_builtins,
            enable_shell,
            enable_subagents,
            enable_memory,
            external_tools,
        })
    }

    pub(super) async fn build_mcp_dispatcher(
        &self,
        server_names: &[String],
    ) -> MobResult<Arc<dyn AgentToolDispatcher>> {
        let servers = meerkat_core::McpConfig::load_with_scopes_from_roots(
            self.context_root.as_deref(),
            self.user_config_root.as_deref(),
        )
        .await
        .map_err(|err| MobError::ToolBundle(err.to_string()))?;

        let mut server_by_name = HashMap::new();
        for item in servers {
            server_by_name.insert(item.server.name.clone(), item.server);
        }

        let mut router = meerkat::McpRouter::new();
        for name in server_names {
            let server = server_by_name.get(name).cloned().ok_or_else(|| {
                MobError::ToolBundle(format!("MCP server '{name}' not found in realm config"))
            })?;
            router
                .add_server(server)
                .await
                .map_err(|err| MobError::ToolBundle(err.to_string()))?;
        }

        Ok(Arc::new(router))
    }
}
