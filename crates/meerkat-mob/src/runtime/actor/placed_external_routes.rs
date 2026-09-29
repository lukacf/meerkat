//! External-peer edges on placed members.
//!
//! The graph fact is the ordinary external edge (`WireExternalPeer` /
//! `UnwireExternalPeer`, `ExternalPeerWired` / `ExternalPeerUnwired`). What is
//! different for a placed member is where its trust row lives: on its host.
//! That row is realized exactly like a placed member-member route:
//!
//! * MobMachine owns the INSTALL ledger (`pending_external_route_installs`
//!   and the Record/Resolve/Rollback trio). A failed install never unwinds
//!   the committed edge; it stays pending and every retry lane drains it.
//! * Remove is synchronous pre-unwire authority: the host must ACK the
//!   removal while the edge still exists, and only then does the unwire
//!   commit.
//! * The bridge carrier is the existing V4 `InstallPeerTrust` /
//!   `RemovePeerTrust`: the host arm installs any validated peer descriptor
//!   on the materialized member, so no protocol change is needed.

use super::*;

impl MobActor {
    /// The host-scoped route operation for `edge`, when its local member is
    /// placed. A local member's edge has no host lane.
    pub(super) fn external_route_obligation_for_edge(
        &self,
        edge: &mob_dsl::ExternalPeerEdge,
        kind: mob_dsl::RouteObligationKind,
    ) -> Option<mob_dsl::ExternalRouteObligation> {
        self.dsl_authority
            .state()
            .member_placement
            .get(&edge.local)
            .map(|host| mob_dsl::ExternalRouteObligation {
                edge: edge.clone(),
                host: host.clone(),
                kind,
            })
    }

    /// Submit `RecordExternalRouteInstall` and require the machine's exact
    /// `ExternalRouteInstallRequested` handoff.
    fn record_external_route_install_obligation(
        &mut self,
        obligation: &mob_dsl::ExternalRouteObligation,
        context: &str,
    ) -> Result<(), MobError> {
        if obligation.kind != mob_dsl::RouteObligationKind::Install {
            return Err(MobError::Internal(
                "pending external route ledger accepts Install obligations only".to_string(),
            ));
        }
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::RecordExternalRouteInstall {
                obligation: obligation.clone(),
            },
            context,
        )?;
        if Self::effects_request_external_route(&effects, obligation) {
            return Ok(());
        }
        Err(MobError::Internal(format!(
            "MobMachine accepted RecordExternalRouteInstall but emitted no ExternalRouteInstallRequested effect for host '{}'",
            obligation.host.as_str()
        )))
    }

    /// Generated authority for an exact synchronous Remove while the external
    /// edge is still wired. Opens no volatile ledger row.
    fn authorize_external_route_removal_before_unwire(
        &mut self,
        obligation: &mob_dsl::ExternalRouteObligation,
    ) -> Result<(), MobError> {
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::AuthorizeExternalRouteRemovalBeforeUnwire {
                obligation: obligation.clone(),
            },
            "authorize_external_route_removal_before_unwire",
        )?;
        if Self::effects_request_external_route(&effects, obligation) {
            return Ok(());
        }
        Err(MobError::Internal(format!(
            "MobMachine authorized pre-unwire external route removal but emitted no exact handoff for host '{}'",
            obligation.host.as_str()
        )))
    }

    fn effects_request_external_route(
        effects: &[mob_dsl::MobMachineEffect],
        obligation: &mob_dsl::ExternalRouteObligation,
    ) -> bool {
        effects.iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::ExternalRouteInstallRequested { obligation: requested }
                    if requested == obligation
            )
        })
    }

    fn resolve_external_route_install_obligation(
        &mut self,
        obligation: &mob_dsl::ExternalRouteObligation,
        context: &str,
    ) -> Result<(), MobError> {
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::ResolveExternalRouteInstall {
                obligation: obligation.clone(),
            },
            context,
        )
    }

    fn rollback_external_route_install_obligation(
        &mut self,
        obligation: &mob_dsl::ExternalRouteObligation,
        context: &str,
    ) -> Result<(), MobError> {
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::RollbackExternalRouteInstall {
                obligation: obligation.clone(),
            },
            context,
        )
    }

    /// Roll back every pending Install for an external edge that is about to
    /// leave the graph (unwire or retirement): it could never re-validate.
    pub(super) fn rollback_superseded_external_installs(
        &mut self,
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> Result<(), MobError> {
        let stale: Vec<mob_dsl::ExternalRouteObligation> = self
            .dsl_authority
            .state()
            .pending_external_route_installs
            .iter()
            .filter(|obligation| &obligation.edge == edge)
            .cloned()
            .collect();
        for obligation in stale {
            self.rollback_external_route_install_obligation(
                &obligation,
                "external_unwire_supersedes_pending_install",
            )?;
        }
        Ok(())
    }

    /// Realize one host-scoped external route operation: install (or remove)
    /// the edge's external descriptor on the placed local member through its
    /// host. A stale Install (the member is no longer placed there, or the
    /// edge left the graph) is superseded, not realizable.
    async fn realize_external_route(
        &mut self,
        obligation: &mob_dsl::ExternalRouteObligation,
    ) -> Result<(), MobError> {
        let premise_holds = {
            let state = self.dsl_authority.state();
            state.member_placement.get(&obligation.edge.local) == Some(&obligation.host)
                && state.external_peer_edges.contains(&obligation.edge)
        };
        if !premise_holds {
            return match obligation.kind {
                mob_dsl::RouteObligationKind::Install => self
                    .rollback_external_route_install_obligation(
                        obligation,
                        "external_route_install_stale_premise",
                    ),
                mob_dsl::RouteObligationKind::Remove => Err(MobError::Internal(format!(
                    "authorized pre-unwire external route removal for host '{}' lost its placed member or edge",
                    obligation.host.as_str()
                ))),
            };
        }
        let host_peer =
            self.bound_host_peer_descriptor(&obligation.host, "realize_external_route")?;
        let authority = self.supervisor_bridge.authority().await;
        let binding_generation = self.current_host_binding_generation(&obligation.host)?;
        let supervisor_spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(&host_peer)
            .await?;
        let peer_spec =
            Self::trusted_peer_descriptor_from_machine_endpoint(&obligation.edge.endpoint)?;
        let payload = crate::runtime::bridge_protocol::BridgePeerTrustPayload {
            supervisor: supervisor_spec.into(),
            epoch: authority.epoch,
            binding_generation,
            protocol_version: crate::runtime::bridge_protocol::BridgeProtocolVersion::V4,
            mob_id: self.definition.id.to_string(),
            agent_identity: obligation.edge.local.0.clone(),
            peer: peer_spec.into(),
        };
        let command = match obligation.kind {
            mob_dsl::RouteObligationKind::Install => {
                crate::runtime::bridge_protocol::BridgeCommand::InstallPeerTrust(payload)
            }
            mob_dsl::RouteObligationKind::Remove => {
                crate::runtime::bridge_protocol::BridgeCommand::RemovePeerTrust(payload)
            }
        };
        self.send_route_install_command(&host_peer, &command)
            .await?;
        if obligation.kind == mob_dsl::RouteObligationKind::Install {
            self.resolve_external_route_install_obligation(
                obligation,
                "external_route_install_confirmed",
            )?;
        }
        Ok(())
    }

    /// Record and realize the Install for an external edge whose local member
    /// is placed. A failure leaves the obligation pending (observable in
    /// `route_installs()`); it never unwinds the committed edge.
    pub(super) async fn fold_external_route_install_after_wire(
        &mut self,
        edge: &mob_dsl::ExternalPeerEdge,
    ) {
        let Some(obligation) =
            self.external_route_obligation_for_edge(edge, mob_dsl::RouteObligationKind::Install)
        else {
            return;
        };
        if let Err(error) = self
            .record_external_route_install_obligation(&obligation, "external_wire_route_install")
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                host = %obligation.host.as_str(),
                %error,
                "external route install not recorded; the host drain re-derives it"
            );
            return;
        }
        if let Err(error) = self.realize_external_route(&obligation).await {
            tracing::warn!(
                mob_id = %self.definition.id,
                host = %obligation.host.as_str(),
                %error,
                "external route install left pending; next trigger retries"
            );
        }
    }

    /// Re-derive the Install ledger from durable facts (optionally for one
    /// host). Machine admission rejects are skips: the host drain re-derives
    /// them once the host binds.
    pub(super) fn record_derived_external_route_install_obligations(
        &mut self,
        host_filter: Option<&mob_dsl::HostId>,
    ) {
        let derived = crate::runtime::derive_external_install_obligations(
            self.dsl_authority.state(),
            host_filter,
        );
        for obligation in derived {
            if let Err(error) = self.record_external_route_install_obligation(
                &obligation,
                "external_route_install_re_derive",
            ) {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    host = %obligation.host.as_str(),
                    %error,
                    "external route-install re-derive skipped by machine admission"
                );
            }
        }
    }

    /// Realize every pending external Install (optionally for one host).
    /// Per-install failures stay pending and do not abort the drain.
    pub(super) async fn realize_pending_external_route_installs(
        &mut self,
        host_filter: Option<&mob_dsl::HostId>,
    ) -> Result<(), MobError> {
        let pending: Vec<mob_dsl::ExternalRouteObligation> = self
            .dsl_authority
            .state()
            .pending_external_route_installs
            .iter()
            .filter(|obligation| host_filter.is_none_or(|filter| &obligation.host == filter))
            .cloned()
            .collect();
        if let Some(invalid) = pending
            .iter()
            .find(|obligation| obligation.kind != mob_dsl::RouteObligationKind::Install)
        {
            return Err(MobError::Internal(format!(
                "MobMachine invariant violation: pending external route ledger contains a non-Install obligation for host '{}'",
                invalid.host.as_str()
            )));
        }
        for obligation in pending {
            if let Err(error) = self.realize_external_route(&obligation).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    host = %obligation.host.as_str(),
                    %error,
                    "external route-install drain left obligation pending; next trigger retries"
                );
            }
        }
        Ok(())
    }

    /// After a placed member is re-materialized (revival) its fresh runtime
    /// holds no trust rows: reinstall every external edge of the identity.
    pub(super) async fn drive_external_routes_for_identity(&mut self, identity: &AgentIdentity) {
        let edges = self.machine_external_peer_edges_for(identity);
        for edge in edges {
            self.fold_external_route_install_after_wire(&edge).await;
        }
    }

    /// Wire a placed member to an external peer: the same admission, machine
    /// edge, durable `ExternalPeerWired` event and roster fold as a local
    /// member, with the trust row realized on the member's host.
    pub(super) async fn wire_placed_member_external_peer(
        &mut self,
        local: AgentIdentity,
        spec: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        TrustedPeerDescriptor::validate_pubkey_for_peer_id(spec.peer_id, &spec.pubkey).map_err(
            |error| MobError::WiringError(format!("external peer descriptor is invalid: {error}")),
        )?;
        let external_identity = AgentIdentity::from(spec.name.as_str());
        if local == external_identity {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{local}')"
            )));
        }
        let edge = Self::external_peer_edge(&local, &spec);
        let key = Self::external_peer_key_for_edge(&edge);
        self.probe_command_admission(
            mob_dsl::MobMachineInput::WireExternalPeer {
                key: key.clone(),
                edge: edge.clone(),
            },
            MobState::Running,
            "wire_placed_external_peer_command_admission",
        )?;
        if self.roster.read().await.get(&local).is_none() {
            return Err(MobError::MemberNotFound(local));
        }
        // The host route must be recordable before the edge commits: a
        // committed edge whose install the machine refuses (host not Bound,
        // carrier binding inactive) would leave nothing pending and a
        // route_installs() that falsely reads complete. Refuse typed instead.
        if self
            .ensure_placed_carrier_binding_active(&local, "wire_placed_external_peer")?
            .is_none()
        {
            return Err(MobError::Internal(format!(
                "placed external wire for '{local}' found no machine placement"
            )));
        }
        // The host keys trust rows by peer id: a second edge to the same peer
        // id under another name would share (and on unwire, drop) one row.
        if let Some(existing) = self
            .machine_external_peer_edges_for(&local)
            .into_iter()
            .find(|existing| {
                existing.endpoint.peer_id == edge.endpoint.peer_id
                    && existing.endpoint.name != edge.endpoint.name
            })
        {
            return Err(MobError::WiringError(format!(
                "placed member '{local}' is already wired to external peer id '{}' as '{}'; one peer id maps to one host trust row",
                edge.endpoint.peer_id.0, existing.endpoint.name.0
            )));
        }
        let authority = self.apply_wire_external_peer_idempotent(&key, &edge)?;
        if !authority.is_repair() {
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::ExternalPeerWired {
                    local: local.clone(),
                    spec: spec.clone(),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(append_error) => {
                    self.rollback_external_wire_dsl(&key, &edge, authority.dsl_added())
                        .await;
                    return Err(MobError::from(append_error));
                }
            };
            self.roster.write().await.apply_event(&stored);
        }
        self.fold_external_route_install_after_wire(&edge).await;
        Ok(())
    }

    /// Unwire a placed member from an external peer. The host must ACK the
    /// exact removal while the edge still exists; a rejected or lost removal
    /// leaves the edge wired and returns the typed error.
    pub(super) async fn unwire_placed_member_external_peer(
        &mut self,
        local: AgentIdentity,
        peer_name: meerkat_core::comms::PeerName,
    ) -> Result<(), MobError> {
        if self.roster.read().await.get(&local).is_none() {
            return Err(MobError::MemberNotFound(local));
        }
        let Some(edge) = self.external_peer_edge_for_name(&local, &peer_name) else {
            // Idempotent: an absent edge is already unwired.
            return Ok(());
        };
        let key = Self::external_peer_key_for_edge(&edge);
        if self.confirmed_revoked_placed_host(&local).is_none() {
            let removal = self
                .external_route_obligation_for_edge(&edge, mob_dsl::RouteObligationKind::Remove)
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "placed external unwire for '{local}' found no machine placement"
                    ))
                })?;
            self.authorize_external_route_removal_before_unwire(&removal)?;
            if let Err(error) = self.realize_external_route(&removal).await {
                // A rejected or timed-out removal leaves the edge wired, and
                // the host may already have dropped the row: reinstall it so
                // route_installs() never reads complete while a wired edge
                // lacks host trust.
                self.fold_external_route_install_after_wire(&edge).await;
                return Err(error);
            }
        }
        // Otherwise an exact revoke tombstone proves the host (and its trust
        // store) is gone: there is no row to remove and no ACK to await, as
        // for member-member routes on a confirmed-revoked host.
        let committed = self
            .rollback_superseded_external_installs(&edge)
            .and_then(|()| {
                self.apply_unwire_external_peer_idempotent(&key, &edge)?
                    .map(|_| ())
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "external unwire for '{local}' -> '{peer_name}' was not authorized by MobMachine"
                        ))
                    })
            });
        if let Err(error) = committed {
            // The host row is gone but the edge is still wired: reinstall it.
            self.fold_external_route_install_after_wire(&edge).await;
            return Err(error);
        }
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerUnwired {
                local: local.clone(),
                peer_name: peer_name.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_error) => {
                // The durable graph still holds the edge: restore the machine
                // edge and reinstall the host row so the unwire is a no-op.
                match self.apply_wire_external_peer_idempotent(&key, &edge) {
                    Ok(_) => self.fold_external_route_install_after_wire(&edge).await,
                    Err(rollback_error) => tracing::warn!(
                        mob_id = %self.definition.id,
                        local = %local,
                        error = %rollback_error,
                        "failed to restore the placed external edge after event append failure"
                    ),
                }
                return Err(MobError::from(append_error));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }
}
