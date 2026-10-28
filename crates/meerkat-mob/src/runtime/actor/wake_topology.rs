//! A woken member's topology step, a chunk of its edges at a time.
//!
//! The step realizes the woken member's side of every edge MobMachine holds
//! for it (`MobActor::member_wake_ready`). It takes the edges a chunk at a
//! time, and a chunk's edges to members that are not materialized (an
//! orchestrator woken among seated, Dormant workers, say) are realized as
//! one batch:
//! - one prepared machine step applies each edge's `WireMembers` (an edge
//!   MobMachine holds answers a trust repair), then each edge's
//!   `AuthorizeMemberTrustWiring`, with one base-checked commit and one
//!   publish. Neither arm changes the machine state, so the topology epoch
//!   does not move and every authorization carries the live epoch;
//! - after that commit each edge's repair authority is derived from its
//!   authorization against the live machine, before any trust is installed,
//!   and the woken member then trusts each other member's durable endpoint;
//! - one more prepared step reports the edges whose realization MobMachine
//!   does not hold yet, so an edge whose trust failed is owed a repair
//!   (`ResolveWiringEdgeRealization`). A realized edge that owes no repair
//!   is no input there (its realization changes nothing), so a wake whose
//!   edges all realize takes no such step.
//!
//! A chunk's other edges (to a live, placed or peer-only member, or to an
//! external peer) take the single-edge wire path, one at a time, as before.
//! An edge MobMachine no longer holds (an unwire or a retirement since the
//! step planned it) is not restored. The turn yields to the actor between
//! chunks.
//!
//! Each prepared step copies and compares the whole machine state, so its
//! cost grows with the mob, and the step takes a number of chunks that does
//! not grow with the edges (at most [`WAKE_TOPOLOGY_CHUNKS`]). The step is
//! then linear in the edges: a fixed number of whole-state copies per wake
//! and map reads per edge, where a wire per edge copied the whole state
//! several times per edge.

use super::member_wake::MemberWiringEdge;
use super::*;

/// The context the machine inputs and the logs of the batched step name.
const WAKE_TOPOLOGY: &str = "member_wake_topology_batch";

/// The most chunks a woken member's topology step takes.
const WAKE_TOPOLOGY_CHUNKS: usize = 4;

/// The fewest edges a chunk takes: a topology this small is one chunk.
const WAKE_TOPOLOGY_MIN_CHUNK: usize = 1024;

/// One edge of a woken member's topology step: its wire target, the peer
/// key a failure is reported under, and the edge as MobMachine keys its owed
/// repairs.
pub(super) type WakeTopologyEdge = (
    super::super::handle::PeerTarget,
    RespawnTopologyPeerId,
    MemberWiringEdge,
);

/// How a chunk takes one of its edges.
enum WakeEdgeRoute {
    /// To a Dormant local member with its durable endpoint: the batch.
    Batched(AgentIdentity),
    /// The single-edge wire path.
    Single,
    /// MobMachine no longer holds the edge: nothing to restore.
    Unwired,
}

/// One edge the batch realizes: the woken member trusts `peer_spec`, the
/// durable endpoint of `peer`.
struct BatchedWakeEdge {
    item: WakeTopologyEdge,
    edge: mob_dsl::WiringEdge,
    peer: AgentIdentity,
    peer_spec: TrustedPeerDescriptor,
}

impl MobActor {
    /// The edges each chunk of a woken member's topology step over `edges`
    /// edges takes: enough that the step takes at most
    /// [`WAKE_TOPOLOGY_CHUNKS`] chunks, and at least
    /// [`WAKE_TOPOLOGY_MIN_CHUNK`].
    pub(super) fn wake_topology_chunk_size(&self, edges: usize) -> usize {
        #[cfg(test)]
        if let Some(size) = WAKE_TOPOLOGY_CHUNKS_FOR_TEST
            .lock()
            .ok()
            .and_then(|chunks| chunks.get(&self.definition.id).copied())
        {
            return size.max(1);
        }
        edges
            .div_ceil(WAKE_TOPOLOGY_CHUNKS)
            .max(WAKE_TOPOLOGY_MIN_CHUNK)
    }

    /// Whether MobMachine no longer holds `edge`, a member edge an unwire or
    /// a retirement removed since the topology step planned it: it is not
    /// restored.
    pub(super) fn wake_edge_unwired(&self, edge: &MemberWiringEdge) -> bool {
        matches!(
            edge,
            MemberWiringEdge::Member(edge)
                if !self.dsl_authority.state().wiring_edges.contains(edge)
        )
    }

    /// Realize the edges of `chunk` that the batch takes (see the module
    /// docs), with every outcome reported to MobMachine and each failed
    /// edge's peer added to `failed`. Returns the chunk's edges that take
    /// the single-edge wire path, in order.
    pub(super) async fn realize_wake_topology_chunk(
        &mut self,
        woken: &AgentIdentity,
        chunk: Vec<WakeTopologyEdge>,
        failed: &mut Vec<RespawnTopologyPeerId>,
    ) -> VecDeque<WakeTopologyEdge> {
        let mut single = VecDeque::new();
        let mut outcomes = Vec::new();
        let mut candidates = Vec::new();
        {
            let state = self.dsl_authority.state();
            let woken_ready = Self::wake_edge_end_ready(state, woken);
            for item in chunk {
                #[cfg(test)]
                if super::member_wake_turn::take_wake_edge_failure_for_test(woken) {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %woken,
                        peer = %item.1,
                        "test seam: a woken member's wiring edge fails; its repair is owed"
                    );
                    let (_, peer_key, edge) = item;
                    failed.push(peer_key);
                    outcomes.push((edge, false));
                    continue;
                }
                match Self::wake_edge_route(state, woken_ready, &item) {
                    WakeEdgeRoute::Batched(peer) => candidates.push((item, peer)),
                    WakeEdgeRoute::Single => single.push_back(item),
                    WakeEdgeRoute::Unwired => tracing::debug!(
                        mob_id = %self.definition.id,
                        agent_identity = %woken,
                        peer = %item.1,
                        "a woken member's edge MobMachine no longer holds is not restored"
                    ),
                }
            }
        }
        if !candidates.is_empty() {
            let unbatched = self
                .realize_batched_wake_edges(woken, candidates, failed, &mut outcomes)
                .await;
            single.extend(unbatched);
        }
        self.report_wake_edge_realizations(outcomes);
        single
    }

    /// Whether `identity` can be an end of a batched edge: its endpoint is
    /// registered and it is an active member (neither retiring nor broken).
    fn wake_edge_end_ready(state: &mob_dsl::MobMachineState, identity: &AgentIdentity) -> bool {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        state.member_peer_ids.contains_key(&dsl_identity)
            && state.member_peer_endpoints.contains_key(&dsl_identity)
            && MobMemberLifecycleProjection::is_active_machine_lifecycle(
                &state.member_lifecycle_for_identity(&dsl_identity),
            )
    }

    /// How a chunk takes `item`, from MobMachine's facts: an edge to a
    /// Dormant local member with its durable endpoint is batched when the
    /// woken member's own end is ready; any other edge takes the
    /// single-edge path.
    fn wake_edge_route(
        state: &mob_dsl::MobMachineState,
        woken_ready: bool,
        (target, _, edge): &WakeTopologyEdge,
    ) -> WakeEdgeRoute {
        let MemberWiringEdge::Member(edge) = edge else {
            return WakeEdgeRoute::Single;
        };
        if !state.wiring_edges.contains(edge) {
            return WakeEdgeRoute::Unwired;
        }
        let super::super::handle::PeerTarget::Local(peer) = target else {
            return WakeEdgeRoute::Single;
        };
        if woken_ready
            && !super::super::member_runtime_is_host_owned(state, peer)
            && super::super::boot_plan::domain_member_is_dormant(state, peer)
            && Self::wake_edge_end_ready(state, peer)
        {
            WakeEdgeRoute::Batched(peer.clone())
        } else {
            WakeEdgeRoute::Single
        }
    }

    /// Realize `candidates` as one batch (see the module docs), recording
    /// each edge's outcome in `outcomes` and each failed edge's peer in
    /// `failed`. Returns the candidates left to the single-edge path: an
    /// edge whose counterpart's endpoint the batch cannot read, or every
    /// candidate when the woken member has no local comms runtime or the
    /// machine step is refused (nothing of the batch is committed then).
    async fn realize_batched_wake_edges(
        &mut self,
        woken: &AgentIdentity,
        candidates: Vec<(WakeTopologyEdge, AgentIdentity)>,
        failed: &mut Vec<RespawnTopologyPeerId>,
        outcomes: &mut Vec<(MemberWiringEdge, bool)>,
    ) -> Vec<WakeTopologyEdge> {
        // The woken member's own end, resolved once, as every wire resolves
        // it: its comms runtime.
        let woken_entry = self.roster.read().await.get(woken).cloned();
        let woken_comms = match woken_entry {
            Some(entry) => match self.resolve_wiring_endpoint(&entry, WAKE_TOPOLOGY).await {
                Ok(WiringEndpoint::Local { comms, .. }) => Some(comms),
                _ => None,
            },
            None => None,
        };
        let Some(woken_comms) = woken_comms else {
            return candidates.into_iter().map(|(item, _)| item).collect();
        };

        // Each counterpart's durable endpoint, read from MobMachine.
        let mut unbatched = Vec::new();
        let mut batched = Vec::with_capacity(candidates.len());
        {
            let roster = self.roster.read().await;
            for (item, peer) in candidates {
                let session_backed = roster
                    .get(&peer)
                    .is_some_and(|entry| matches!(entry.member_ref, MemberRef::Session { .. }));
                let peer_spec = if session_backed {
                    self.machine_member_peer_spec_for(&peer, WAKE_TOPOLOGY)
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let edge = match &item.2 {
                    MemberWiringEdge::Member(edge) => Some(edge.clone()),
                    MemberWiringEdge::External(_) => None,
                };
                match (peer_spec, edge) {
                    (Some(peer_spec), Some(edge)) => batched.push(BatchedWakeEdge {
                        item,
                        edge,
                        peer,
                        peer_spec,
                    }),
                    _ => unbatched.push(item),
                }
            }
        }
        if batched.is_empty() {
            return unbatched;
        }

        // One prepared step: every edge's WireMembers, then every edge's
        // trust authorization, over the topology the wires leave.
        let inputs = batched
            .iter()
            .map(|batched| mob_dsl::MobMachineInput::WireMembers {
                edge: batched.edge.clone(),
            })
            .chain(batched.iter().map(|batched| {
                mob_dsl::MobMachineInput::AuthorizeMemberTrustWiring {
                    edge: batched.edge.clone(),
                    a_identity: batched.edge.a.clone(),
                    b_identity: batched.edge.b.clone(),
                }
            }))
            .collect::<Vec<_>>();
        let mut authority = self.dsl_authority.prepare_authority();
        let mut wired = match authority.apply_batch(inputs) {
            Ok(transitions) if transitions.len() == batched.len() * 2 => transitions,
            Ok(_) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %woken,
                    edges = batched.len(),
                    "MobMachine answered a woken member's batched edges with a transition count \
                     that does not match them; each takes the single-edge path"
                );
                unbatched.extend(batched.into_iter().map(|batched| batched.item));
                return unbatched;
            }
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %woken,
                    edges = batched.len(),
                    %error,
                    "MobMachine refused a woken member's batched edges; each takes the single-edge path"
                );
                unbatched.extend(batched.into_iter().map(|batched| batched.item));
                return unbatched;
            }
        };
        let authorized = wired.split_off(batched.len());
        // Every wire must be a trust repair of an edge MobMachine holds. Any
        // other answer refuses the batch, and nothing of it is committed.
        let refusal = batched.iter().zip(&wired).find_map(|(batched, wire)| {
            match Self::wire_members_disposition_from_effects(
                wire.effects(),
                &batched.edge,
                WAKE_TOPOLOGY,
            ) {
                Ok(false) => None,
                Ok(true) => Some(MobError::WiringError(format!(
                    "{WAKE_TOPOLOGY} added edge {:?} that MobMachine held when it was planned",
                    batched.edge
                ))),
                Err(error) => Some(error),
            }
        });
        if let Some(error) = refusal {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %woken,
                edges = batched.len(),
                %error,
                "a woken member's batched edges were refused; each takes the single-edge path"
            );
            unbatched.extend(batched.into_iter().map(|batched| batched.item));
            return unbatched;
        }
        let phase_changed = wired
            .iter()
            .chain(&authorized)
            .any(|transition| transition.from_phase != transition.to_phase);
        let effects = wired
            .into_iter()
            .flat_map(mob_dsl::MobMachineTransition::into_effects)
            .chain(
                authorized
                    .iter()
                    .flat_map(|transition| transition.effects().iter().cloned()),
            )
            .collect();
        if let Err(error) = self.commit_prepared_dsl_input(PreparedDslInput {
            authority,
            effects,
            phase_changed,
        }) {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %woken,
                edges = batched.len(),
                %error,
                "a woken member's batched edges could not be committed; each takes the single-edge path"
            );
            unbatched.extend(batched.into_iter().map(|batched| batched.item));
            return unbatched;
        }
        #[cfg(test)]
        record_wake_topology_batch_for_test(&self.definition.id, batched.len());

        // Each edge's repair authority, derived from its authorization
        // against the live machine once the step committed, before any trust
        // is installed.
        let mut installs = Vec::with_capacity(batched.len());
        for (batched, authorized) in batched.into_iter().zip(&authorized) {
            let key = Self::trusted_peer_removal_key(&batched.peer_spec);
            let authority = self
                .member_trust_wiring_handoff_from_transition(
                    authorized,
                    &batched.edge,
                    WAKE_TOPOLOGY,
                    MemberTrustOperation::Repair,
                )
                .and_then(|handoff| {
                    handoff.require_peer_id_for(&batched.peer, &key)?;
                    handoff.repair_authority_for(&batched.peer, &key, &self.dsl_authority)
                });
            match authority {
                Ok(authority) => installs.push((batched, authority)),
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %woken,
                        peer = %batched.peer,
                        %error,
                        "a woken member's wiring edge has no trust authority; its repair is owed"
                    );
                    let (_, peer_key, edge) = batched.item;
                    failed.push(peer_key);
                    outcomes.push((edge, false));
                }
            }
        }
        for (batched, authority) in installs {
            let (_, peer_key, edge) = batched.item;
            match self
                .apply_trusted_peer_add_report(woken_comms.as_ref(), batched.peer_spec, authority)
                .await
            {
                Ok(_) => outcomes.push((edge, true)),
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %woken,
                        peer = %batched.peer,
                        %error,
                        "a woken member could not trust a member that is not materialized; \
                         its repair is owed"
                    );
                    failed.push(peer_key);
                    outcomes.push((edge, false));
                }
            }
        }
        unbatched
    }

    /// Report the realization of a chunk's edges to MobMachine in one
    /// prepared step: a realized edge clears its owed repair, and a failed
    /// one is owed a repair while it is wired between seated members
    /// (MobMachine answers `OwedEdgeRepairsRecorded`, which arms its repair
    /// timer). A realized edge that owes no repair is left out: its
    /// realization changes nothing in MobMachine. No step is taken when
    /// nothing is left.
    fn report_wake_edge_realizations(&mut self, outcomes: Vec<(MemberWiringEdge, bool)>) {
        let outcomes = {
            let state = self.dsl_authority.state();
            outcomes
                .into_iter()
                .filter(|(edge, realized)| {
                    !realized
                        || match edge {
                            MemberWiringEdge::Member(edge) => {
                                state.owed_edge_repairs.contains(edge)
                            }
                            MemberWiringEdge::External(edge) => {
                                state.owed_external_edge_repairs.contains(edge)
                            }
                        }
                })
                .collect::<Vec<_>>()
        };
        if outcomes.is_empty() {
            return;
        }
        let mut authority = self.dsl_authority.prepare_authority();
        let mut effects = Vec::new();
        let mut phase_changed = false;
        for (edge, realized) in outcomes {
            let signal = match &edge {
                MemberWiringEdge::Member(edge) => {
                    mob_dsl::MobMachineSignal::ResolveWiringEdgeRealization {
                        edge: edge.clone(),
                        realized,
                    }
                }
                MemberWiringEdge::External(edge) => {
                    mob_dsl::MobMachineSignal::ResolveExternalEdgeRealization {
                        edge: edge.clone(),
                        realized,
                    }
                }
            };
            match authority.apply_signal(signal) {
                Ok(transition) => {
                    phase_changed |= transition.from_phase != transition.to_phase;
                    effects.extend(transition.into_effects());
                }
                Err(error) => tracing::error!(
                    mob_id = %self.definition.id,
                    ?edge,
                    realized,
                    %error,
                    "MobMachine refused an edge realization outcome"
                ),
            }
        }
        if let Err(error) = self.commit_prepared_dsl_input(PreparedDslInput {
            authority,
            effects,
            phase_changed,
        }) {
            tracing::error!(
                mob_id = %self.definition.id,
                %error,
                "the realization outcomes of a woken member's edges could not be committed"
            );
        }
    }
}

/// Test override of the chunk size, per mob.
#[cfg(test)]
static WAKE_TOPOLOGY_CHUNKS_FOR_TEST: std::sync::Mutex<BTreeMap<MobId, usize>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Make the woken members of `mob_id` take `size` edges per topology chunk.
#[cfg(test)]
pub(in crate::runtime) fn set_wake_topology_chunk_for_test(mob_id: &MobId, size: usize) {
    if let Ok(mut chunks) = WAKE_TOPOLOGY_CHUNKS_FOR_TEST.lock() {
        chunks.insert(mob_id.clone(), size);
    }
}

/// The edge count of every batch the woken members of a mob committed, in
/// order: the structural witness that a chunk's edges are one machine step.
#[cfg(test)]
static WAKE_TOPOLOGY_BATCHES_FOR_TEST: std::sync::Mutex<BTreeMap<MobId, Vec<usize>>> =
    std::sync::Mutex::new(BTreeMap::new());

#[cfg(test)]
fn record_wake_topology_batch_for_test(mob_id: &MobId, edges: usize) {
    if let Ok(mut batches) = WAKE_TOPOLOGY_BATCHES_FOR_TEST.lock() {
        batches.entry(mob_id.clone()).or_default().push(edges);
    }
}

/// Test read: the edge count of every batch the woken members of `mob_id`
/// committed, in order.
#[cfg(test)]
pub(in crate::runtime) fn wake_topology_batches_for_test(mob_id: &MobId) -> Vec<usize> {
    WAKE_TOPOLOGY_BATCHES_FOR_TEST
        .lock()
        .ok()
        .and_then(|batches| batches.get(mob_id).cloned())
        .unwrap_or_default()
}
