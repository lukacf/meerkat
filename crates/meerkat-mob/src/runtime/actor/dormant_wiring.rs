//! Wiring with a member that is not materialized in this process.
//!
//! A seated local member that is Dormant (or Waking before its comms runtime
//! exists) resolves to [`WiringEndpoint::LocalDormant`]: its peer material is
//! MobMachine's durable endpoint. Wire, unwire and the idempotent repair
//! commit the machine edge and its durable event exactly as for a live
//! member. Trust is installed or removed only on a side that has a comms
//! runtime (a live member trusts the durable endpoint; a peer-only member's
//! overlay is pushed through its bridge); the dormant side's own trust is
//! realized by its wake's topology step, from machine state. No wiring verb
//! wakes a member, with one exception MobMachine decides: a new edge that
//! joins a Dormant member to a placed member wakes it (the placed member, in
//! another process, cannot use its in-process wake route). That wire's
//! transition carries the member's materialization, realized with every
//! other residency effect of the apply (see `wire_new_edge_with_placement`).
//!
//! The lifecycle notices such an edge change would exchange are MobMachine's
//! verdict (`ClassifyPeerNotice`): a member that is not materialized can
//! neither send nor receive them, they are suppressed, and the trust its wake
//! installs carries their content.
//!
//! An edge to an external peer is the member's own trust row and nothing
//! else. MobMachine's verdict on the wire or unwire says whether that row is
//! owed now: for a Dormant member it is not (no trust obligation is
//! emitted), so the machine edge and its durable event are committed alone,
//! and the member's wake installs the rows of the external edges the machine
//! holds then. A row that is owed while the member's wake has not yet built
//! its comms runtime waits for that wake (see
//! [`MobActor::external_wiring_awaits_wake`]).

use super::*;

/// The side of an edge with a comms runtime, trusting a counterpart that is
/// not materialized.
struct LiveSideTrust<'a> {
    comms: &'a Arc<dyn CoreCommsRuntime>,
    counterpart: &'a AgentIdentity,
    counterpart_spec: &'a TrustedPeerDescriptor,
}

/// A peer-only side's overlay toward a counterpart that is not materialized.
struct PeerOnlySideTrust<'a> {
    spec: &'a TrustedPeerDescriptor,
    binding: &'a crate::RuntimeBinding,
    counterpart_spec: &'a TrustedPeerDescriptor,
}

/// What an edge change with at least one member that is not materialized
/// realizes beyond the machine edge and its event.
enum DormantEdgeLane<'a> {
    /// Neither member is materialized: nothing to install now.
    Neither,
    /// One live member's trust toward the other's durable endpoint.
    Live(LiveSideTrust<'a>),
    /// One peer-only member's overlay toward the other's durable endpoint.
    PeerOnly(PeerOnlySideTrust<'a>),
}

fn dormant_edge_lane<'a>(
    local: &'a AgentIdentity,
    local_endpoint: &'a WiringEndpoint,
    peer: &'a AgentIdentity,
    peer_endpoint: &'a WiringEndpoint,
) -> Result<DormantEdgeLane<'a>, MobError> {
    match (local_endpoint, peer_endpoint) {
        (WiringEndpoint::LocalDormant { .. }, WiringEndpoint::LocalDormant { .. }) => {
            Ok(DormantEdgeLane::Neither)
        }
        (WiringEndpoint::Local { comms, .. }, WiringEndpoint::LocalDormant { spec, .. }) => {
            Ok(DormantEdgeLane::Live(LiveSideTrust {
                comms,
                counterpart: peer,
                counterpart_spec: spec,
            }))
        }
        (WiringEndpoint::LocalDormant { spec, .. }, WiringEndpoint::Local { comms, .. }) => {
            Ok(DormantEdgeLane::Live(LiveSideTrust {
                comms,
                counterpart: local,
                counterpart_spec: spec,
            }))
        }
        (
            WiringEndpoint::PeerOnly { spec, binding },
            WiringEndpoint::LocalDormant {
                spec: dormant_spec, ..
            },
        )
        | (
            WiringEndpoint::LocalDormant {
                spec: dormant_spec, ..
            },
            WiringEndpoint::PeerOnly { spec, binding },
        ) => Ok(DormantEdgeLane::PeerOnly(PeerOnlySideTrust {
            spec,
            binding,
            counterpart_spec: dormant_spec,
        })),
        _ => Err(MobError::Internal(format!(
            "the wiring lane of members that are not materialized was entered for '{local}' <-> '{peer}' without such a member"
        ))),
    }
}

impl MobActor {
    /// MobMachine's verdict on the lifecycle notices two members exchange
    /// when the edge between them changes and one of them is not
    /// materialized: each direction must be `Suppressed`. A member that is
    /// not materialized can neither send nor receive a notice, so any other
    /// verdict is refused typed rather than dropped.
    pub(super) fn require_suppressed_lifecycle_notices(
        &mut self,
        a: &AgentIdentity,
        b: &AgentIdentity,
    ) -> Result<(), MobError> {
        for (recipient, sender) in [(a, b), (b, a)] {
            let disposition =
                self.classify_peer_notice(recipient, sender, mob_dsl::PeerInputClass::Lifecycle)?;
            if !matches!(disposition, mob_dsl::PeerNoticeDisposition::Suppressed) {
                return Err(Self::unmaterialized_notice_drift(
                    recipient,
                    sender,
                    &format!("{disposition:?}"),
                ));
            }
        }
        Ok(())
    }

    /// MobMachine's verdict on one lifecycle notice from `sender` to
    /// `recipient`, realized through the notice consumer: true only for
    /// `Deliver`. A notice that is not delivered is skipped because of that
    /// verdict, never because of the recipient's endpoint.
    pub(super) fn lifecycle_notice_delivers(
        &mut self,
        recipient: &AgentIdentity,
        sender: &AgentIdentity,
    ) -> Result<bool, MobError> {
        self.peer_notice_delivers(recipient, sender, mob_dsl::PeerInputClass::Lifecycle)
    }

    /// MobMachine's verdict on one notice of `class` from `sender` to
    /// `recipient`: true only for `Deliver`. A kickoff status notice is
    /// content: it is delivered like a message, and its delivery wakes a
    /// recipient that is not materialized through its wake route.
    pub(super) fn peer_notice_delivers(
        &mut self,
        recipient: &AgentIdentity,
        sender: &AgentIdentity,
        class: mob_dsl::PeerInputClass,
    ) -> Result<bool, MobError> {
        Ok(matches!(
            self.realize_peer_notice_disposition(recipient, sender, class)?,
            super::member_wake::PeerNoticeRealization::Deliver
        ))
    }

    /// A lifecycle notice whose MobMachine verdict needs a comms runtime that
    /// a member that is not materialized does not have: the verdict and the
    /// member's materialization disagree.
    pub(super) fn unmaterialized_notice_drift(
        recipient: &AgentIdentity,
        sender: &AgentIdentity,
        verdict: &str,
    ) -> MobError {
        MobError::Internal(format!(
            "MobMachine's verdict {verdict} on the lifecycle notice from '{sender}' to '{recipient}' needs a comms runtime that a member that is not materialized does not have"
        ))
    }

    /// Wire a new edge one of whose members is not materialized (see the
    /// module docs). Every effect before the durable event is rolled back on
    /// a failure, the machine edge included.
    pub(super) async fn wire_new_edge_with_dormant_endpoint(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        local: &AgentIdentity,
        peer: &AgentIdentity,
        local_endpoint: &WiringEndpoint,
        peer_endpoint: &WiringEndpoint,
    ) -> Result<(), MobError> {
        let lane = dormant_edge_lane(local, local_endpoint, peer, peer_endpoint)?;
        let local_spec = local_endpoint.spec();
        let peer_spec = peer_endpoint.spec();
        // The notices are a pure verdict: refused before anything moves.
        self.require_suppressed_lifecycle_notices(local, peer)?;
        let authority = self.apply_wire_members_idempotent(edge)?;
        let dsl_added = authority.dsl_added();
        let handoff = authority.member_handoff()?;

        let mut live_trust_created = false;
        match &lane {
            DormantEdgeLane::Neither => {}
            DormantEdgeLane::Live(side) => {
                let key = Self::trusted_peer_removal_key(side.counterpart_spec);
                let trust_authority = handoff
                    .require_peer_id_for(side.counterpart, &key)
                    .and_then(|()| {
                        handoff.add_authority_for(side.counterpart, &key, &self.dsl_authority)
                    });
                let added = match trust_authority {
                    Ok(trust_authority) => self
                        .apply_trusted_peer_add_report(
                            side.comms.as_ref(),
                            side.counterpart_spec.clone(),
                            trust_authority,
                        )
                        .await
                        .map_err(MobError::from),
                    Err(error) => Err(error),
                };
                match added {
                    Ok(created) => live_trust_created = created,
                    Err(error) => {
                        self.rollback_peer_only_wire(
                            edge,
                            dsl_added,
                            WiringSides::empty(),
                            local,
                            peer,
                            local_spec,
                            peer_spec,
                        )
                        .await;
                        return Err(error);
                    }
                }
            }
            DormantEdgeLane::PeerOnly(side) => {
                if let Err(error) = self
                    .wire_peer_only_recipient(
                        side.spec,
                        Some(side.binding),
                        side.counterpart_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await
                {
                    self.rollback_peer_only_wire(
                        edge,
                        dsl_added,
                        WiringSides::empty(),
                        local,
                        peer,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(error);
                }
            }
        }

        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersWired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(error) => {
                match &lane {
                    DormantEdgeLane::Live(side) if live_trust_created => {
                        let key = Self::trusted_peer_removal_key(side.counterpart_spec);
                        self.rollback_peer_only_trust(
                            edge,
                            side.comms.as_ref(),
                            side.counterpart,
                            &key,
                            "wire_unmaterialized_member_event_rollback_trust_authority",
                        )
                        .await;
                    }
                    DormantEdgeLane::PeerOnly(side) => {
                        if let Err(cleanup) = self
                            .unwire_peer_only_recipient(
                                side.spec,
                                None,
                                side.counterpart_spec,
                                std::time::Duration::from_secs(10),
                            )
                            .await
                        {
                            tracing::warn!(
                                mob_id = %self.definition.id,
                                error = %cleanup,
                                "wire rollback: failed to unwire the peer-only recipient"
                            );
                        }
                    }
                    _ => {}
                }
                self.rollback_peer_only_wire(
                    edge,
                    dsl_added,
                    WiringSides::empty(),
                    local,
                    peer,
                    local_spec,
                    peer_spec,
                )
                .await;
                return Err(MobError::from(error));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    /// Unwire an edge one of whose members is not materialized (see the
    /// module docs). Idempotent absence is a no-op only when the live side's
    /// trust agrees. A failure before the durable event restores the machine
    /// edge (and the live side's trust, when it was removed).
    pub(super) async fn unwire_edge_with_dormant_endpoint(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        local: &AgentIdentity,
        peer: &AgentIdentity,
        local_endpoint: &WiringEndpoint,
        peer_endpoint: &WiringEndpoint,
        dsl_has_edge: bool,
    ) -> Result<(), MobError> {
        let lane = dormant_edge_lane(local, local_endpoint, peer, peer_endpoint)?;
        if !dsl_has_edge {
            if let DormantEdgeLane::Live(side) = &lane {
                let key = Self::trusted_peer_removal_key(side.counterpart_spec);
                let stale = side
                    .comms
                    .peers()
                    .await
                    .iter()
                    .any(|trusted| trusted.peer_id.to_string() == key);
                if stale {
                    return Err(MobError::WiringError(format!(
                        "unwire for '{local}' <-> '{peer}' requires MobMachine wiring authority"
                    )));
                }
            }
            return Ok(());
        }
        // The notices are a pure verdict: refused before anything moves.
        self.require_suppressed_lifecycle_notices(local, peer)?;
        self.cancel_peer_deliveries_for_edge(local, peer, "members are unwiring")
            .await?;
        let unwire_handoff = self.apply_unwire_members_idempotent(edge)?;

        let mut live_trust_removed = false;
        let removed = match &lane {
            DormantEdgeLane::Neither => Ok(()),
            DormantEdgeLane::Live(side) => {
                let key = Self::trusted_peer_removal_key(side.counterpart_spec);
                let removal = unwire_handoff
                    .require_peer_id_for(side.counterpart, &key)
                    .and_then(|()| unwire_handoff.unwiring_authority_for(side.counterpart, &key));
                match removal {
                    Ok(authority) => {
                        let result = self
                            .apply_trusted_peer_remove(side.comms.as_ref(), key, authority)
                            .await
                            .map(|_| ())
                            .map_err(MobError::from);
                        live_trust_removed = result.is_ok();
                        result
                    }
                    Err(error) => Err(error),
                }
            }
            DormantEdgeLane::PeerOnly(side) => {
                self.unwire_peer_only_recipient(
                    side.spec,
                    Some(side.binding),
                    side.counterpart_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            }
        };
        let appended = match removed {
            Ok(()) => {
                let event = NewMobEvent {
                    mob_id: self.definition.id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MembersUnwired {
                        a: AgentIdentity::from(edge.a.0.as_str()),
                        b: AgentIdentity::from(edge.b.0.as_str()),
                    },
                };
                self.events.append(event).await.map_err(MobError::from)
            }
            Err(error) => Err(error),
        };
        match appended {
            Ok(stored) => {
                self.roster.write().await.apply_event(&stored);
                Ok(())
            }
            Err(error) => {
                self.restore_unmaterialized_member_edge(edge, &lane, live_trust_removed)
                    .await;
                Err(error)
            }
        }
    }

    /// Roll back a failed unwire of an edge with a member that is not
    /// materialized: the machine edge is wired again and the live side's
    /// trust reinstalled when it was removed. Best effort: each failure is
    /// logged, as the other unwire rollbacks do.
    async fn restore_unmaterialized_member_edge(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        lane: &DormantEdgeLane<'_>,
        live_trust_removed: bool,
    ) {
        let authority = match self.apply_wire_members_idempotent(edge) {
            Ok(authority) => authority,
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "unwire rollback: failed to restore the MobMachine wire"
                );
                return;
            }
        };
        let DormantEdgeLane::Live(side) = lane else {
            return;
        };
        if !live_trust_removed {
            return;
        }
        let key = Self::trusted_peer_removal_key(side.counterpart_spec);
        let restored = match authority.member_handoff().and_then(|handoff| {
            handoff
                .require_peer_id_for(side.counterpart, &key)
                .and_then(|()| {
                    handoff.add_authority_for(side.counterpart, &key, &self.dsl_authority)
                })
        }) {
            Ok(trust_authority) => self
                .apply_trusted_peer_add_report(
                    side.comms.as_ref(),
                    side.counterpart_spec.clone(),
                    trust_authority,
                )
                .await
                .map(|_| ())
                .map_err(MobError::from),
            Err(error) => Err(error),
        };
        if let Err(error) = restored {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "unwire rollback: failed to reinstall the live side's trust"
            );
        }
    }

    /// Whether `command` is an external-peer wire or unwire that waits for
    /// its local member's wake (a deferral predicate of
    /// `resume_member_control_is_pending`): the member's wake attempt is
    /// open, so its comms runtime is the wake's to build, and MobMachine's
    /// verdict owes the member a trust row change. The command runs once the
    /// attempt closed, on the member as the wake left it: Live, the row
    /// changes there; Dormant again, the verdict owes no row; Broken, the
    /// verb refuses typed. Every other command runs now.
    pub(super) fn external_wiring_awaits_wake(&self, command: &MobCommand) -> bool {
        use super::super::handle::PeerTarget;
        let (local, input) = match command {
            MobCommand::Wire { local, target, .. } => {
                let spec = match target {
                    PeerTarget::External(descriptor) => descriptor.clone(),
                    PeerTarget::ExternalBinding(binding) => {
                        match Self::trusted_peer_descriptor_from_external_binding(binding.clone()) {
                            Ok(descriptor) => descriptor,
                            // The verb refuses the binding itself.
                            Err(_) => return false,
                        }
                    }
                    PeerTarget::Local(_) | PeerTarget::ExternalName(_) => return false,
                };
                let edge = Self::external_peer_edge(local, &spec);
                let key = Self::external_peer_key_for_edge(&edge);
                (
                    local,
                    mob_dsl::MobMachineInput::WireExternalPeer { key, edge },
                )
            }
            MobCommand::Unwire { local, target, .. } => {
                let name = match target {
                    PeerTarget::Local(id) => {
                        if self
                            .dsl_authority
                            .state()
                            .identity_to_runtime
                            .contains_key(&mob_dsl::AgentIdentity::from_domain(id))
                        {
                            return false;
                        }
                        id.as_str().to_string()
                    }
                    PeerTarget::ExternalName(name) => name.as_str().to_string(),
                    PeerTarget::External(descriptor) => descriptor.name.as_str().to_string(),
                    PeerTarget::ExternalBinding(binding) => binding.name.clone(),
                };
                let Ok(peer_name) = meerkat_core::comms::PeerName::new(name) else {
                    return false;
                };
                // An edge the machine does not hold owes no row change.
                let Some(edge) = self.external_peer_edge_for_name(local, &peer_name) else {
                    return false;
                };
                let key = Self::external_peer_key_for_edge(&edge);
                (
                    local,
                    mob_dsl::MobMachineInput::UnwireExternalPeer { key, edge },
                )
            }
            _ => return false,
        };
        if !self
            .dsl_authority
            .state()
            .member_wake_attempts
            .contains_key(&mob_dsl::AgentIdentity::from_domain(local))
        {
            return false;
        }
        let Ok(prepared) =
            self.prepare_dsl_input_transition(input.clone(), "external_wiring_wake_probe")
        else {
            // The verb answers the machine's refusal itself.
            return false;
        };
        let effects = prepared.transition.effects();
        match &input {
            mob_dsl::MobMachineInput::WireExternalPeer { edge, .. } => {
                Self::external_peer_trust_row_owed(effects, edge)
            }
            mob_dsl::MobMachineInput::UnwireExternalPeer { edge, .. } => {
                Self::external_peer_trust_removal_owed(effects, edge)
            }
            _ => false,
        }
    }

    /// Whether MobMachine's external-peer wire verdict owes the local
    /// member's runtime a trust row for `edge` now: a new edge's wiring
    /// obligation, or an existing edge's repair. A member that is not
    /// materialized is owed none.
    pub(super) fn external_peer_trust_row_owed(
        effects: &[mob_dsl::MobMachineEffect],
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> bool {
        effects.iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::ExternalPeerTrustWiringRequested { edge: owed, .. }
                    | mob_dsl::MobMachineEffect::ExternalPeerTrustRepairRequested {
                        edge: owed,
                        ..
                    } if owed == edge
            )
        })
    }

    /// Whether MobMachine's external-peer unwire verdict owes the local
    /// member's runtime the removal of its trust row for `edge`.
    pub(super) fn external_peer_trust_removal_owed(
        effects: &[mob_dsl::MobMachineEffect],
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> bool {
        effects.iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::ExternalPeerTrustUnwiringRequested { edge: owed, .. }
                    if owed == edge
            )
        })
    }

    /// Commit an external-peer wire whose verdict owes the local member no
    /// trust row (see the module docs): a new edge commits once its
    /// `ExternalPeerWired` event is durable; a wire of an edge the machine
    /// already holds changes and publishes nothing.
    pub(super) async fn commit_external_wire_without_trust_row(
        &mut self,
        prepared: PreparedDslTransition,
        local: AgentIdentity,
        spec: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        if !Self::effects_include_wiring_graph_change(prepared.transition.effects()) {
            return self.commit_prepared_dsl_transition(prepared);
        }
        let events = self.events.clone();
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerWired { local, spec },
        };
        let stored = self
            .commit_prepared_dsl_transition_after(prepared, move || async move {
                events.append(event).await.map_err(MobError::from)
            })
            .await?;
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    /// Commit an external-peer unwire whose verdict owes the local member no
    /// trust row removal (see the module docs): the edge leaves the graph
    /// once its `ExternalPeerUnwired` event is durable.
    pub(super) async fn commit_external_unwire_without_trust_row(
        &mut self,
        prepared: PreparedDslTransition,
        local: AgentIdentity,
        peer_name: meerkat_core::comms::PeerName,
    ) -> Result<(), MobError> {
        if !Self::effects_include_wiring_graph_change(prepared.transition.effects()) {
            return Err(MobError::WiringError(format!(
                "external unwire for '{local}' -> '{peer_name}' was not authorized by MobMachine"
            )));
        }
        let events = self.events.clone();
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerUnwired { local, peer_name },
        };
        let stored = self
            .commit_prepared_dsl_transition_after(prepared, move || async move {
                events.append(event).await.map_err(MobError::from)
            })
            .await?;
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }
}
