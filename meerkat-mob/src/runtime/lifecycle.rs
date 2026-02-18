use super::*;

impl MobRuntime {
    pub(super) async fn resolve_step_targets(
        &self,
        mob_id: &MobId,
        spec: &MobSpec,
        role: &str,
        meerkat_selector: &str,
        activation_payload: Value,
    ) -> MobResult<Vec<DispatchTarget>> {
        let role_id = RoleId::from(role);
        let selector = if meerkat_selector == "*" {
            None
        } else {
            Some(MeerkatId::from(meerkat_selector))
        };
        let mut out = self
            .discover_dispatch_targets(mob_id, &role_id, selector.as_ref())
            .await?;

        if !out.is_empty() {
            return Ok(out);
        }

        let Some(role_spec) = spec.roles.get(role) else {
            return Ok(Vec::new());
        };
        if matches!(role_spec.spawn_strategy, crate::model::SpawnStrategy::Eager) {
            return Ok(Vec::new());
        }

        let identities = self
            .resolve_role_meerkats(mob_id, spec, role, role_spec, activation_payload)
            .await?;
        let spawn_lock = self.spawn_lock(mob_id).await;
        let _spawn_guard = spawn_lock.lock().await;
        for identity in identities {
            if selector
                .as_ref()
                .is_some_and(|selected| identity.meerkat_id != *selected)
            {
                continue;
            }
            let key = format!("{}/{}", role, identity.meerkat_id);
            let already_exists = {
                let managed = self.managed_meerkats.read().await;
                managed
                    .get(mob_id)
                    .and_then(|map| map.get(&key))
                    .is_some()
            };
            if already_exists {
                continue;
            }
            self.spawn_meerkat(mob_id, spec, role, &identity).await?;
        }

        out = self
            .discover_dispatch_targets(mob_id, &role_id, selector.as_ref())
            .await?;
        Ok(out)
    }

    async fn discover_dispatch_targets(
        &self,
        mob_id: &MobId,
        role_id: &RoleId,
        selector: Option<&MeerkatId>,
    ) -> MobResult<Vec<DispatchTarget>> {
        let supervisor = self.supervisor_runtime(mob_id).await?;
        let supervisor_name = self.supervisor_name_from_mob(mob_id);
        let peers = supervisor.peers().await;

        let mut dedup = BTreeMap::<String, DispatchTarget>::new();
        for peer in peers {
            if peer.name.as_ref() == supervisor_name {
                continue;
            }
            if peer.meta.labels.get("mob_id") != Some(&mob_id.to_string()) {
                continue;
            }
            let Some(peer_role) = peer.meta.labels.get("role") else {
                continue;
            };
            if peer_role != role_id.as_ref() {
                continue;
            }
            let Some(peer_meerkat_id) = peer.meta.labels.get("meerkat_id") else {
                continue;
            };
            let resolved_meerkat_id = MeerkatId::from(peer_meerkat_id.as_str());
            if selector.is_some_and(|selected| resolved_meerkat_id != *selected) {
                continue;
            }

            dedup.insert(
                peer.name.to_string(),
                DispatchTarget {
                    role: RoleId::from(peer_role.as_str()),
                    meerkat_id: resolved_meerkat_id,
                    comms_name: peer.name.to_string(),
                },
            );
        }

        Ok(dedup.into_values().collect())
    }

    pub(super) async fn resolve_role_meerkats(
        &self,
        mob_id: &MobId,
        spec: &MobSpec,
        role_name: &str,
        role: &RoleSpec,
        activation_payload: Value,
    ) -> MobResult<Vec<MeerkatIdentity>> {
        match role.cardinality {
            crate::model::CardinalityKind::Singleton => Ok(vec![MeerkatIdentity {
                meerkat_id: MeerkatId::from("singleton"),
                role: RoleId::from(role_name),
                labels: role.labels.clone(),
                attributes: BTreeMap::new(),
            }]),
            crate::model::CardinalityKind::PerKey | crate::model::CardinalityKind::PerMeerkat => {
                let resolver_id = role
                    .resolver
                    .as_ref()
                    .ok_or_else(|| MobError::ResolverNotFound {
                        resolver_id: "<missing>".to_string(),
                    })?;

                let resolver = self.resolver_registry.get(resolver_id).ok_or_else(|| {
                    MobError::ResolverNotFound {
                        resolver_id: resolver_id.clone(),
                    }
                })?;

                let resolver_spec = spec.resolvers.get(resolver_id).cloned();
                let context = ResolverContext {
                    mob_id: mob_id.to_string(),
                    role: role_name.to_string(),
                    resolver_id: resolver_id.clone(),
                    resolver_spec,
                    spec: Some(Arc::new(spec.clone())),
                    activation_payload,
                };

                let mut resolved = resolver.list_meerkats(&context).await?;
                for identity in &mut resolved {
                    if identity.role.is_empty() {
                        identity.role = RoleId::from(role_name);
                    }
                    if identity.labels.is_empty() {
                        identity.labels = role.labels.clone();
                    }
                }
                Ok(resolved)
            }
        }
    }

    pub(super) async fn spawn_meerkat(
        &self,
        mob_id: &MobId,
        spec: &MobSpec,
        role_name: &str,
        identity: &MeerkatIdentity,
    ) -> MobResult<()> {
        let role = spec
            .roles
            .get(role_name)
            .ok_or_else(|| MobError::SpecValidation(format!("unknown role '{role_name}'")))?;

        let namespace = self.namespace_for_mob(mob_id);
        let comms_name = format!(
            "{}/{}/{}/{}",
            self.realm_id, mob_id, role_name, identity.meerkat_id
        );

        let mut labels = identity.labels.clone();
        labels.insert("mob_id".to_string(), mob_id.to_string());
        labels.insert("role".to_string(), role_name.to_string());
        labels.insert("meerkat_id".to_string(), identity.meerkat_id.to_string());

        let request = self
            .compile_role_session_request(CompileRoleSessionInput {
                mob_id,
                spec,
                role_name,
                role,
                labels: &labels,
                comms_name: &comms_name,
                namespace,
            })
            .await?;

        let created = self.session_service.create_session(request).await?;
        let session_id = created.session_id;
        let Some(runtime) = self.session_service.comms_runtime(&session_id).await else {
            return Err(MobError::Comms(format!(
                "session '{}' comms runtime unavailable",
                session_id
            )));
        };
        let _ = runtime.inbox_notify();
        {
            let mut managed = self.managed_meerkats.write().await;
            let mob_map = managed.entry(mob_id.clone()).or_default();
            let key = format!("{role_name}/{}", identity.meerkat_id);
            mob_map.insert(
                key,
                ManagedMeerkat {
                    role: RoleId::from(role_name),
                    meerkat_id: identity.meerkat_id.clone(),
                    session_id: session_id.clone(),
                    comms_name: comms_name.clone(),
                    last_activity_at: Utc::now(),
                    status: MeerkatInstanceStatus::Running,
                },
            );
        }
        self.refresh_peer_trust(mob_id, spec).await?;

        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Lifecycle,
                MobEventKind::MeerkatSpawned,
            )
            .meerkat_id(identity.meerkat_id.clone())
            .payload(json!({"role": role_name, "comms_name": comms_name}))
            .build(),
        )
        .await?;

        Ok(())
    }

    pub(super) async fn refresh_peer_trust(&self, mob_id: &MobId, spec: &MobSpec) -> MobResult<()> {
        let supervisor = self.supervisor_runtime(mob_id).await?;
        let supervisor_name = self.supervisor_name_from_mob(mob_id);
        let supervisor_peer_id = CoreCommsRuntimeTrait::public_key(&*supervisor)
            .ok_or_else(|| MobError::Comms("supervisor public key unavailable".to_string()))?;

        let entries: Vec<ManagedMeerkat> = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(mob_id)
                .map(|map| map.values().cloned().collect())
                .unwrap_or_default()
        };

        let mut live = Vec::with_capacity(entries.len());
        for instance in &entries {
            let runtime = self
                .session_service
                .comms_runtime(&instance.session_id)
                .await
                .ok_or_else(|| {
                    MobError::Comms(format!(
                        "missing comms runtime for session {}",
                        instance.session_id
                    ))
                })?;
            let peer_id = CoreCommsRuntimeTrait::public_key(&*runtime)
                .ok_or_else(|| MobError::Comms("peer public key unavailable".to_string()))?;
            live.push((instance.clone(), runtime, peer_id));
        }

        let mut supervisor_peers = Vec::with_capacity(live.len());
        for (instance, _, peer_id) in &live {
            supervisor_peers.push(
                TrustedPeerSpec::new(
                    instance.comms_name.clone(),
                    peer_id.clone(),
                    format!("inproc://{}", instance.comms_name),
                )
                .map_err(MobError::Comms)?,
            );
        }
        supervisor
            .replace_trusted_peers(supervisor_peers)
            .await
            .map_err(|err| MobError::Comms(err.to_string()))?;

        for (source, source_runtime, _) in &live {
            let mut desired = vec![
                TrustedPeerSpec::new(
                    supervisor_name.clone(),
                    supervisor_peer_id.clone(),
                    format!("inproc://{supervisor_name}"),
                )
                .map_err(MobError::Comms)?,
            ];

            for (target, _, target_peer_id) in &live {
                if source.comms_name == target.comms_name {
                    continue;
                }

                if matches!(spec.topology.ad_hoc.mode, PolicyMode::Strict)
                    && !role_pair_allowed(&spec.topology.ad_hoc, &source.role, &target.role)
                {
                    self.emit_event(
                        self.event(
                            mob_id.clone(),
                            MobEventCategory::Topology,
                            MobEventKind::TopologyViolation,
                        )
                        .meerkat_id(source.meerkat_id.clone())
                        .payload(json!({
                            "domain": "ad_hoc",
                            "from_role": source.role.clone(),
                            "to_role": target.role.clone(),
                            "from": source.comms_name.clone(),
                            "to": target.comms_name.clone(),
                            "message": "blocked trusted-peer edge by strict ad_hoc policy",
                        }))
                        .build(),
                    )
                    .await?;
                    continue;
                }

                desired.push(
                    TrustedPeerSpec::new(
                        target.comms_name.clone(),
                        target_peer_id.clone(),
                        format!("inproc://{}", target.comms_name),
                    )
                    .map_err(MobError::Comms)?,
                );
            }

            source_runtime
                .replace_trusted_peers(desired)
                .await
                .map_err(|err| MobError::Comms(err.to_string()))?;
        }

        Ok(())
    }

    pub(super) async fn retire_meerkat(&self, mob_id: &MobId, key: &str) -> MobResult<()> {
        let instance = {
            let managed = self.managed_meerkats.read().await;
            let Some(map) = managed.get(mob_id) else {
                return Ok(());
            };
            map.get(key).cloned()
        };

        if let Some(instance) = instance {
            self.session_service.archive(&instance.session_id).await?;
            {
                let mut managed = self.managed_meerkats.write().await;
                if let Some(map) = managed.get_mut(mob_id) {
                    map.remove(key);
                }
            }
            if let Some(spec) = self.spec_store.get_spec(mob_id.as_ref()).await? {
                self.refresh_peer_trust(mob_id, &spec).await?;
            }

            self.emit_event(
                self.event(
                    mob_id.clone(),
                    MobEventCategory::Lifecycle,
                    MobEventKind::MeerkatRetired,
                )
                .meerkat_id(instance.meerkat_id)
                .payload(json!({"key": key}))
                .build(),
            )
            .await?;
        }

        Ok(())
    }

    pub(super) async fn supervisor_runtime(&self, mob_id: &MobId) -> MobResult<Arc<CommsRuntime>> {
        if let Some(runtime) = self.supervisors.read().await.get(mob_id).cloned() {
            return Ok(runtime);
        }

        let namespace = self.namespace_for_mob(mob_id);
        let name = self.supervisor_name_from_mob(mob_id);

        let runtime = CommsRuntime::inproc_only_scoped(&name, Some(namespace))
            .map_err(|err| MobError::Comms(err.to_string()))?;
        runtime.set_peer_meta(
            PeerMeta::default()
                .with_description("Mob supervisor runtime")
                .with_label("mob_id", mob_id.as_ref())
                .with_label("role", "supervisor"),
        );

        let runtime = Arc::new(runtime);
        self.supervisors
            .write()
            .await
            .insert(mob_id.clone(), runtime.clone());
        Ok(runtime)
    }

    pub(super) fn namespace_for_mob(&self, mob_id: &MobId) -> String {
        format!("{}/{}", self.realm_id, mob_id)
    }

    pub(super) fn supervisor_name_from_mob(&self, mob_id: &MobId) -> String {
        format!("{}/{}/supervisor", self.realm_id, mob_id)
    }

    pub(super) async fn spawn_lock(&self, mob_id: &MobId) -> Arc<Mutex<()>> {
        if let Some(lock) = self.spawn_locks.read().await.get(mob_id).cloned() {
            return lock;
        }
        let mut locks = self.spawn_locks.write().await;
        locks
            .entry(mob_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
