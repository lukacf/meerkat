//! Edge topology reconciliation for distributed runtime nodes.

use std::collections::{BTreeMap, BTreeSet};

use futures::stream::{self, StreamExt};
use meerkat_mob::SpawnMemberSpec;
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::runtime::MobMemberListEntry;
use meerkat_mob::runtime::reconcile::ReconcileOptions;

use crate::runtime::RuntimeRoute;
use crate::unified_runtime::types::meerkat_reconcile_report_to_wire;

use super::edge_types::{DesiredPeerEdge, EdgeMemberView, EdgeReconcileFailure};
use super::types::{
    ErrorEvent, UnifiedRuntimeReconcileEdgesReport, UnifiedRuntimeReconcileError,
    UnifiedRuntimeReconcileReport, UnifiedRuntimeReconcileRoutingReport,
};
use super::{
    ROSTER_ROUTE_CHANNEL, ROSTER_ROUTE_PREFIX, ROSTER_ROUTE_SINK, ROSTER_ROUTE_TARGET_MODULE,
    UnifiedRuntime,
};

const EDGE_RECONCILE_CONCURRENCY: usize = 64;

/// Default [`super::edge_types::EdgeDiscovery`] policy derived from the mob
/// definition's declared `wiring` block. Upstream applies
/// `auto_wire_orchestrator`/`role_wiring` only at SPAWN time and only from
/// the non-orchestrator side (meerkat-mob `spawn_wiring_targets`), which
/// makes the result bring-up-order dependent: a lead ensured after its
/// workers — or any member resumed after a gateway restart — ends up with
/// `wired_to: []` and the declared crew never forms (downstream app field report,
/// 2026-07-09). This policy makes the declaration a RECONCILABLE desired
/// state: `reconcile_edges` wires whatever the definition implies over the
/// live roster, order-independently and restart-healingly. It is installed
/// automatically when the embedder supplies no custom policy and the
/// definition declares wiring; the reconciler only ever unwires edges it
/// wired itself, so host-made manual edges are never touched.
pub struct DefinitionWiringEdgeDiscovery {
    auto_wire_orchestrator: bool,
    orchestrator_profile: Option<String>,
    role_wiring: Vec<(String, String)>,
}

impl DefinitionWiringEdgeDiscovery {
    /// `None` when the definition declares no wiring (the reconciler then
    /// stays inert exactly as before).
    pub fn from_definition(definition: &meerkat_mob::MobDefinition) -> Option<Self> {
        let auto_wire_orchestrator =
            definition.wiring.auto_wire_orchestrator && definition.orchestrator.is_some();
        if !auto_wire_orchestrator && definition.wiring.role_wiring.is_empty() {
            return None;
        }
        Some(Self {
            auto_wire_orchestrator,
            orchestrator_profile: definition
                .orchestrator
                .as_ref()
                .map(|config| config.profile.to_string()),
            role_wiring: definition
                .wiring
                .role_wiring
                .iter()
                .map(|rule| (rule.a.to_string(), rule.b.to_string()))
                .collect(),
        })
    }

    pub(crate) fn desired_edges(&self, members: &[EdgeMemberView]) -> Vec<DesiredPeerEdge> {
        let mut edges = BTreeSet::new();
        if self.auto_wire_orchestrator
            && let Some(orchestrator_profile) = &self.orchestrator_profile
        {
            for orchestrator in members.iter().filter(|m| &m.role == orchestrator_profile) {
                for member in members.iter().filter(|m| &m.role != orchestrator_profile) {
                    if let Ok(edge) = DesiredPeerEdge::new(
                        orchestrator.agent_identity.clone(),
                        member.agent_identity.clone(),
                    ) {
                        edges.insert(edge);
                    }
                }
            }
        }
        for (a, b) in &self.role_wiring {
            for left in members.iter().filter(|m| &m.role == a) {
                for right in members.iter().filter(|m| &m.role == b) {
                    if let Ok(edge) = DesiredPeerEdge::new(
                        left.agent_identity.clone(),
                        right.agent_identity.clone(),
                    ) {
                        edges.insert(edge);
                    }
                }
            }
        }
        edges.into_iter().collect()
    }
}

impl super::edge_types::EdgeDiscovery for DefinitionWiringEdgeDiscovery {
    fn discover_edges(
        &self,
        active_members: Vec<EdgeMemberView>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<DesiredPeerEdge>> + Send + '_>>
    {
        Box::pin(async move { self.desired_edges(&active_members) })
    }
}

impl UnifiedRuntime {
    pub async fn reconcile(
        &self,
        desired_specs: Vec<SpawnMemberSpec>,
    ) -> Result<UnifiedRuntimeReconcileReport, UnifiedRuntimeReconcileError> {
        if self.identity_runtime_ref().is_some() {
            return Err(UnifiedRuntimeReconcileError::Mob(
                crate::mob_handle_runtime::MobRuntimeError::InvalidConfig(
                    "whole-mob reconcile is unavailable with identity-first authority; use refresh_desired_topology or identity topology reconcile"
                        .to_string(),
                ),
            ));
        }
        // Member-id alias rule: decode every spec id first, so an encoded
        // roster id is not encoded twice into a phantom member, and refuse a
        // generated runtime alias, which only the identity authority owns, as
        // spawn does. The baseline then holds the public aliases.
        let mut decoded_specs = Vec::with_capacity(desired_specs.len());
        for mut spec in desired_specs {
            let alias =
                crate::member_comms_id::runtime_alias_str(spec.identity.as_str()).into_owned();
            if crate::member_comms_id::is_reserved_generated_alias(&alias) {
                return Err(UnifiedRuntimeReconcileError::Mob(
                    crate::mob_handle_runtime::MobRuntimeError::InvalidConfig(format!(
                        "generated member alias requires identity authority: {alias}"
                    )),
                ));
            }
            crate::member_comms_id::refuse_runtime_plane_member(
                &crate::member_comms_id::mob_member_id_str(&alias),
                spec.labels.as_ref(),
            )
            .map_err(|message| {
                UnifiedRuntimeReconcileError::Mob(
                    crate::mob_handle_runtime::MobRuntimeError::InvalidConfig(message),
                )
            })?;
            spec.identity = meerkat_mob::AgentIdentity::from(alias.as_str());
            decoded_specs.push(spec);
        }
        let desired_specs = decoded_specs;
        self.mob_runtime
            .set_baseline_member_specs(desired_specs.clone())
            .await;
        // Spec member ids arrive in the public alias space (identity-first
        // identities like `domain:billing` contain `:`); encode them into
        // comms-safe roster ids at the meta→meerkat-mob boundary (meerkat
        // 0.7 `MemberCommsName` is fail-closed). The projections below —
        // reconcile report, console identity registration, routing — decode
        // back to the alias space.
        let desired_specs: Vec<SpawnMemberSpec> = desired_specs
            .into_iter()
            .map(|mut spec| {
                spec.identity = crate::member_comms_id::mob_member_id(spec.identity.as_str());
                spec
            })
            .collect();
        // 1. Member reconcile (meerkat 0.6 native path)
        let mob_id = self.mob_handle().mob_id().to_string();
        let meerkat_report = self
            .mob_handle()
            .reconcile(desired_specs, ReconcileOptions { retire_stale: true })
            .await
            .map_err(|err| UnifiedRuntimeReconcileError::Mob(err.into()))?;
        let mob = meerkat_reconcile_report_to_wire(&mob_id, meerkat_report);
        // 2. Refresh active members. Roster ids are comms-safe encodings; the
        // agent-event ingest decodes them back to the alias space, so console
        // registration is keyed by the public alias. Register as a fallback
        // only: spawn/reserve paths register the durable console identity for
        // the same alias key, and a reconcile must never clobber that with
        // the alias self-mapping.
        let active_snapshots = self.mob_handle().list_members_including_retiring().await;
        for member in &active_snapshots {
            let alias = crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str())
                .into_owned();
            self.console_events
                .register_runtime_identity_fallback(alias.clone(), alias)
                .await;
        }
        let active_member_ids = active_snapshots
            .iter()
            .map(|m| {
                crate::member_comms_id::runtime_alias_str(m.agent_identity.as_str()).into_owned()
            })
            .collect::<Vec<_>>();
        // 3 + 4. Edge discovery + dynamic edge reconcile
        let edges = self.reconcile_edges_from_members(active_snapshots).await;
        // 5. Routing reconcile
        let routing = self.reconcile_routing_wiring(active_member_ids).await?;
        let report = UnifiedRuntimeReconcileReport {
            mob,
            edges,
            routing,
        };
        if let Some(hook) = &self.post_reconcile_hook {
            hook(report.clone()).await;
        }
        // Meerkat 0.6's `MobHandle::reconcile` returns Ok even on per-identity
        // failures (they're collected into `report.mob.failures`). Re-lift any
        // non-empty failures list into an `Err` so callers using `?` see the
        // same propagation behavior they had pre-cleanup, while still carrying
        // the full report for inspection via `PartialFailure`.
        if !report.mob.failures.is_empty() {
            return Err(UnifiedRuntimeReconcileError::PartialFailure(Box::new(
                report,
            )));
        }
        Ok(report)
    }

    /// Reconcile dynamic peer edges using fresh roster state.
    ///
    /// Refreshes the roster, runs edge discovery if configured, diffs
    /// desired vs managed edges, and calls wire/unwire as needed. On a
    /// candidate launch it is a dry run: nothing is wired, unwired or
    /// pruned, and the report names what would be.
    pub async fn reconcile_edges(&self) -> UnifiedRuntimeReconcileEdgesReport {
        let report = self.topology_runtime_handle().reconcile().await;
        if let Some(alert) = reconcile_incomplete_alert(&report) {
            self.fire_error(alert);
            log_reconcile_failures(&report.failures);
        } else if !report.skipped_missing_members.is_empty() {
            // INFO, not WARN: lazy identity bootstrap leaves an edge pending
            // until both endpoints are members, so a boot skips such edges by
            // design and a warning would fire on every boot.
            tracing::info!(
                skipped = report.skipped_missing_members.len(),
                "edge reconcile left edges pending until both endpoints are members; \
                 nothing failed"
            );
        }
        report
    }

    pub(super) async fn reconcile_edges_from_members(
        &self,
        active_members: Vec<MobMemberListEntry>,
    ) -> UnifiedRuntimeReconcileEdgesReport {
        // Member reconcile and public edge reconcile must share the same
        // admission lock as operator topology mutation. Otherwise an ordinary
        // reconcile can observe and overwrite a half-applied intent journal.
        let _admission = self.topology_controller.mutation_guard().await;
        // A candidate launch's reconcile is a dry run: it rewires nothing and
        // touches no topology journal.
        let dry_run = !self.mob_runtime.speaks_for_composition();
        if !dry_run && let Err(error) = self.topology_controller.prepare_pending_recovery().await {
            tracing::error!(error = %error, "failed to prepare topology recovery rollback");
            return crate::topology_control::recovery_failure_report(error);
        }
        let report = reconcile_edges_over_members(
            &self.mob_handle(),
            self.edge_discovery.as_deref(),
            &self.managed_dynamic_edges,
            Some(&self.topology_controller),
            active_members,
            dry_run,
        )
        .await;
        if !dry_run
            && let Err(error) = self
                .topology_controller
                .finalize_recovered_pending(report.is_complete())
                .await
        {
            tracing::error!(error = %error, "failed to finalize recovered topology operation");
        }
        report
    }

    pub(super) async fn reconcile_routing_wiring(
        &self,
        mut active_members: Vec<String>,
    ) -> Result<UnifiedRuntimeReconcileRoutingReport, UnifiedRuntimeReconcileError> {
        active_members.sort();
        active_members.dedup();

        let mut rt = self.module_runtime.lock().await;
        let router_module_loaded = rt
            .loaded_modules()
            .iter()
            .any(|module_id| module_id == "router");
        let mut added_route_keys = Vec::new();
        let mut removed_route_keys = Vec::new();

        if router_module_loaded {
            let managed_routes: Vec<RuntimeRoute> = rt
                .list_runtime_routes()
                .into_iter()
                .filter(|route| route.route_key.starts_with(ROSTER_ROUTE_PREFIX))
                .collect();
            let active_member_set = active_members.iter().cloned().collect::<BTreeSet<_>>();
            for route in &managed_routes {
                if !active_member_set.contains(&route.recipient) {
                    rt.delete_runtime_route(&route.route_key)
                        .map_err(UnifiedRuntimeReconcileError::RouteMutation)?;
                    removed_route_keys.push(route.route_key.clone());
                }
            }

            let existing_managed_recipients = managed_routes
                .into_iter()
                .map(|route| route.recipient)
                .collect::<BTreeSet<_>>();
            for member_id in &active_members {
                if existing_managed_recipients.contains(member_id) {
                    continue;
                }
                let route_key = format!("{ROSTER_ROUTE_PREFIX}{member_id}");
                rt.add_runtime_route(RuntimeRoute {
                    route_key: route_key.clone(),
                    recipient: member_id.clone(),
                    channel: Some(ROSTER_ROUTE_CHANNEL.to_string()),
                    sink: ROSTER_ROUTE_SINK.to_string(),
                    target_module: ROSTER_ROUTE_TARGET_MODULE.to_string(),
                    retry_max: None,
                    backoff_ms: None,
                    rate_limit_per_minute: None,
                })
                .map_err(UnifiedRuntimeReconcileError::RouteMutation)?;
                added_route_keys.push(route_key);
            }
        }

        added_route_keys.sort();
        removed_route_keys.sort();

        Ok(UnifiedRuntimeReconcileRoutingReport {
            router_module_loaded,
            active_members,
            added_route_keys,
            removed_route_keys,
        })
    }
}

/// Definition-driven, wire-only edge reconcile for surfaces that hold only a
/// [`crate::MobRuntime`] (the console RPC dispatch): derives the desired
/// edges from the definition's declared `wiring` block over the live roster
/// and wires whatever is missing. The transient managed set means this NEVER
/// unwires — host-made and upstream-made edges are untouched.
pub async fn reconcile_definition_edges(
    runtime: &crate::MobRuntime,
) -> UnifiedRuntimeReconcileEdgesReport {
    reconcile_definition_edges_with_topology(runtime, None).await
}

/// Definition-driven reconciliation composed with the optional durable
/// topology overlay. Console routers use this form so a suppression tombstone
/// cannot be undone by an old `reconcile_edges` call.
pub async fn reconcile_definition_edges_with_topology(
    runtime: &crate::MobRuntime,
    topology: Option<&crate::topology_control::TopologyController>,
) -> UnifiedRuntimeReconcileEdgesReport {
    let _admission = match topology {
        Some(controller) => Some(controller.mutation_guard().await),
        None => None,
    };
    // A candidate launch's reconcile is a dry run: it rewires nothing and
    // touches no topology journal.
    let dry_run = !runtime.speaks_for_composition();
    if !dry_run
        && let Some(controller) = topology
        && let Err(error) = controller.prepare_pending_recovery().await
    {
        tracing::error!(error = %error, "failed to prepare topology recovery rollback");
        return crate::topology_control::recovery_failure_report(error);
    }
    let handle = runtime.handle();
    let policy = DefinitionWiringEdgeDiscovery::from_definition(handle.definition());
    let active_members = handle.list_members_including_retiring().await;
    let managed = tokio::sync::RwLock::new(BTreeSet::new());
    let report = reconcile_edges_over_members(
        &handle,
        policy
            .as_ref()
            .map(|value| value as &dyn super::edge_types::EdgeDiscovery),
        &managed,
        topology,
        active_members,
        dry_run,
    )
    .await;
    if !dry_run
        && let Some(controller) = topology
        && let Err(error) = controller
            .finalize_recovered_pending(report.is_complete())
            .await
    {
        tracing::error!(error = %error, "failed to finalize recovered topology operation");
    }
    report
}

/// The `meta/reconcile_edges` result: the report, and `"applied": false`
/// when it was a candidate launch's dry run (nothing was wired, unwired or
/// pruned; the report names what would be). A launch that applied it answers
/// the report alone, as before.
pub(crate) fn reconcile_edges_result(
    report: &UnifiedRuntimeReconcileEdgesReport,
    applied: bool,
) -> serde_json::Value {
    let mut result = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
    if !applied && let Some(object) = result.as_object_mut() {
        object.insert("applied".to_string(), serde_json::Value::Bool(false));
    }
    result
}

/// The reconcile body, shared by [`UnifiedRuntime::reconcile_edges`] and the
/// console surface's definition-driven reconcile (which passes a transient
/// empty managed set — wire-only convergence: it wires missing desired edges
/// and never unwires anything it did not itself manage).
///
/// With `dry_run` (a candidate launch) nothing is wired, unwired or pruned:
/// the report names what the reconcile would wire, unwire and prune, and the
/// census is logged.
pub(crate) async fn reconcile_edges_over_members(
    mob_handle: &meerkat_mob::runtime::MobHandle,
    edge_discovery: Option<&dyn super::edge_types::EdgeDiscovery>,
    managed_dynamic_edges: &tokio::sync::RwLock<BTreeSet<(String, String)>>,
    topology: Option<&crate::topology_control::TopologyController>,
    active_members: Vec<MobMemberListEntry>,
    dry_run: bool,
) -> UnifiedRuntimeReconcileEdgesReport {
    // Everything in this function speaks the public alias space: roster
    // ids are comms-safe encodings (meerkat 0.7 `MemberCommsName`), so
    // decode snapshots on entry and encode only at the wire/unwire calls.
    let alias_of =
        |id: &str| -> String { crate::member_comms_id::runtime_alias_str(id).into_owned() };

    let active_ids: BTreeSet<String> = active_members
        .iter()
        .map(|m| alias_of(m.agent_identity.as_str()))
        .collect();

    // Build current wiring map from snapshots
    let mut current_edges: BTreeSet<(String, String)> = BTreeSet::new();
    for member in &active_members {
        for peer in &member.wired_to {
            let mut a = alias_of(member.agent_identity.as_str());
            let mut b = alias_of(peer.as_str());
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            current_edges.insert((a, b));
        }
    }

    // Project to EdgeMemberView for the policy closure — it only needs
    // identity/role/labels/wired_to, not meerkat's private runtime fields.
    let member_views: Vec<EdgeMemberView> = active_members
        .into_iter()
        .map(|m| EdgeMemberView {
            agent_identity: alias_of(m.agent_identity.as_str()),
            role: m.role.to_string(),
            wired_to: m
                .wired_to
                .iter()
                .map(|peer| alias_of(peer.as_str()))
                .collect(),
            labels: m.labels,
        })
        .collect();

    // Run edge discovery
    let discovered = match edge_discovery {
        Some(edge_discovery) => edge_discovery.discover_edges(member_views).await,
        None => Vec::new(),
    };
    let raw_desired = match topology {
        Some(topology) => topology.compose_declared(discovered).await,
        None => discovered,
    };

    // Deduplicate and defensively validate (DesiredPeerEdge enforces
    // invariants at construction, but we still canonicalize the key set)
    let desired: BTreeSet<(String, String)> = raw_desired
        .iter()
        .map(|e| {
            let (a, b) = e.endpoints();
            (a.to_string(), b.to_string())
        })
        .collect();

    let mut report = UnifiedRuntimeReconcileEdgesReport {
        desired_edges: raw_desired,
        ..Default::default()
    };

    let managed_snapshot = managed_dynamic_edges.read().await.clone();
    let mut to_wire = Vec::new();

    // Classify desired edges
    for (a, b) in &desired {
        if crate::member_comms_id::is_reserved_generated_alias(a)
            || crate::member_comms_id::is_reserved_generated_alias(b)
        {
            if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
                report.failures.push(EdgeReconcileFailure {
                    edge,
                    operation: "wire".to_string(),
                    error: "generated aliases require IdentityRuntime topology authority"
                        .to_string(),
                });
            }
            continue;
        }
        // Skip if either endpoint is missing from the active roster
        if !active_ids.contains(a) || !active_ids.contains(b) {
            if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
                report.skipped_missing_members.push(edge);
            }
            continue;
        }
        let key = (a.clone(), b.clone());
        if managed_snapshot.contains(&key) {
            // Managed by us — check if the actual edge still exists in the
            // mob graph. If an out-of-band unwire() removed it, re-wire.
            if current_edges.contains(&key) {
                if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
                    report.retained_edges.push(edge);
                }
            } else {
                to_wire.push((a.clone(), b.clone(), "wire (heal)"));
            }
        } else if current_edges.contains(&key) {
            // Exists but not managed by us (static or external) — don't claim
            if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
                report.preexisting_edges.push(edge);
            }
        } else {
            to_wire.push((a.clone(), b.clone(), "wire"));
        }
    }

    // Unwire managed edges that are no longer desired
    let mut stale_pruned = Vec::new();
    let mut to_unwire = Vec::new();
    for (a, b) in managed_snapshot
        .iter()
        .filter(|key| !desired.contains(*key))
        .cloned()
    {
        let key = (a.clone(), b.clone());
        if crate::member_comms_id::is_reserved_generated_alias(&a)
            || crate::member_comms_id::is_reserved_generated_alias(&b)
        {
            if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
                report.failures.push(EdgeReconcileFailure {
                    edge,
                    operation: "unwire".to_string(),
                    error: "generated aliases require IdentityRuntime topology authority"
                        .to_string(),
                });
            }
            continue;
        }
        // If either endpoint is gone, just prune from managed set
        if !active_ids.contains(&a) || !active_ids.contains(&b) {
            stale_pruned.push((a, b));
            continue;
        }
        // If the edge is already gone from the mob graph (out-of-band
        // unwire/reset), just drop ownership — don't attempt unwire.
        if !current_edges.contains(&key) {
            stale_pruned.push((a, b));
            continue;
        }
        to_unwire.push((a, b));
    }

    // A candidate launch rewires no member: report what the reconcile would
    // change, in the report's own shape, and apply none of it.
    if dry_run {
        let logical = |a: &String, b: &String| DesiredPeerEdge::new(a.clone(), b.clone()).ok();
        report.wired_edges = to_wire
            .iter()
            .filter_map(|(a, b, _)| logical(a, b))
            .collect();
        report.unwired_edges = to_unwire
            .iter()
            .filter_map(|(a, b)| logical(a, b))
            .collect();
        report.pruned_stale_managed_edges = stale_pruned
            .iter()
            .filter_map(|(a, b)| logical(a, b))
            .collect();
        let joined = |edges: &[DesiredPeerEdge]| {
            edges
                .iter()
                .map(|edge| {
                    let (a, b) = edge.endpoints();
                    format!("{a}<->{b}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        if report.wired_edges.is_empty() && report.unwired_edges.is_empty() {
            tracing::info!(
                retained = report.retained_edges.len(),
                skipped = report.skipped_missing_members.len(),
                "candidate launch peer-edge census: the edge reconcile would wire and unwire no \
                 edge"
            );
        } else {
            tracing::warn!(
                would_wire = report.wired_edges.len(),
                would_unwire = report.unwired_edges.len(),
                wire = %joined(&report.wired_edges),
                unwire = %joined(&report.unwired_edges),
                "candidate launch peer-edge census: the edge reconcile applies nothing; the \
                 authoritative launch would wire and unwire these edges"
            );
        }
        return report;
    }

    let handle = mob_handle.clone();
    let wire_operations = to_wire
        .iter()
        .map(|(a, b, operation)| ((a.clone(), b.clone()), (*operation).to_string()))
        .collect::<BTreeMap<_, _>>();
    let mut wire_successes = Vec::new();
    let mut wire_failures = Vec::new();
    if !to_wire.is_empty() {
        let batch_edges = to_wire
            .iter()
            .map(|(a, b, _)| {
                (
                    crate::member_comms_id::mob_member_id(a.as_str()),
                    crate::member_comms_id::mob_member_id(b.as_str()),
                )
            })
            .collect::<Vec<_>>();
        // The batch report carries roster ids; decode back to aliases and
        // re-canonicalize (alias-space ordering can differ from roster-id
        // ordering) so keys line up with `to_wire`.
        let alias_edge_key = |a: &AgentIdentity, b: &AgentIdentity| -> (String, String) {
            let mut a = crate::member_comms_id::runtime_alias_str(a.as_str()).into_owned();
            let mut b = crate::member_comms_id::runtime_alias_str(b.as_str()).into_owned();
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            (a, b)
        };
        match handle.wire_members_batch(batch_edges).await {
            Ok(batch_report) => {
                let mut seen = BTreeSet::new();
                for edge in batch_report.wired {
                    let key = alias_edge_key(&edge.a, &edge.b);
                    seen.insert(key.clone());
                    wire_successes.push((key.0, key.1, true));
                }
                for edge in batch_report.already_wired {
                    let key = alias_edge_key(&edge.a, &edge.b);
                    seen.insert(key.clone());
                    wire_successes.push((key.0, key.1, false));
                }
                for (a, b, operation) in to_wire {
                    let key = if a <= b { (a, b) } else { (b, a) };
                    if !seen.contains(&key) {
                        wire_failures.push((
                            key.0,
                            key.1,
                            operation.to_string(),
                            "wire_members_batch omitted edge from report".to_string(),
                        ));
                    }
                }
            }
            Err(err) => {
                let error = err.to_string();
                for (a, b, operation) in to_wire {
                    wire_failures.push((a, b, operation.to_string(), error.clone()));
                }
            }
        }
    }

    let handle = mob_handle.clone();
    let unwire_results = stream::iter(to_unwire.into_iter().map(|(a, b)| {
        let handle = handle.clone();
        async move {
            let result = handle
                .unwire(
                    crate::member_comms_id::mob_member_id(a.as_str()),
                    crate::member_comms_id::mob_member_id(b.as_str()),
                )
                .await
                .map_err(|err| format!("{err}"));
            (a, b, result)
        }
    }))
    .buffer_unordered(EDGE_RECONCILE_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut managed_edges = managed_dynamic_edges.write().await;
    for (a, b, newly_wired) in wire_successes {
        managed_edges.insert((a.clone(), b.clone()));
        if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
            if newly_wired {
                report.wired_edges.push(edge);
            } else {
                report.retained_edges.push(edge);
            }
        }
    }
    for (a, b, operation, error) in wire_failures {
        if let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone()) {
            report.failures.push(EdgeReconcileFailure {
                edge,
                operation: wire_operations.get(&(a, b)).cloned().unwrap_or(operation),
                error,
            });
        }
    }
    for (a, b) in stale_pruned {
        managed_edges.remove(&(a.clone(), b.clone()));
        if let Ok(edge) = DesiredPeerEdge::new(a, b) {
            report.pruned_stale_managed_edges.push(edge);
        }
    }
    for (a, b, result) in unwire_results {
        match result {
            Ok(()) => {
                managed_edges.remove(&(a.clone(), b.clone()));
                if let Ok(edge) = DesiredPeerEdge::new(a, b) {
                    report.unwired_edges.push(edge);
                }
            }
            Err(error) => {
                if let Ok(edge) = DesiredPeerEdge::new(a, b) {
                    report.failures.push(EdgeReconcileFailure {
                        edge,
                        operation: "unwire".into(),
                        error,
                    });
                }
            }
        }
    }

    report
}

/// The alert an edge reconcile pages: only when an edge operation failed,
/// naming the first [`LOGGED_RECONCILE_FAILURES`] failed operations beside
/// the counts, with `skipped` counting the edges skipped beside the
/// failures. An edge skipped because an endpoint is not a member yet failed
/// nothing: lazy identity bootstrap leaves it pending until both endpoints
/// materialize.
fn reconcile_incomplete_alert(report: &UnifiedRuntimeReconcileEdgesReport) -> Option<ErrorEvent> {
    if report.failures.is_empty() {
        return None;
    }
    Some(ErrorEvent::ReconcileIncomplete {
        failures: report.failures.len(),
        skipped: report.skipped_missing_members.len(),
        failed: report
            .failures
            .iter()
            .take(LOGGED_RECONCILE_FAILURES)
            .cloned()
            .collect(),
    })
}

/// How many failed edge operations an incomplete reconcile names, in its
/// alert and in a log line each; one more line counts the rest.
const LOGGED_RECONCILE_FAILURES: usize = 5;

/// Log the failed edge operations behind a `ReconcileIncomplete` alert, which
/// names the same first ones: one ERROR line per failure with its operation,
/// edge endpoints and error, for the first [`LOGGED_RECONCILE_FAILURES`], then
/// one line counting the rest. They use the alert line's target, so a filter
/// that shows the alert shows them too.
fn log_reconcile_failures(failures: &[EdgeReconcileFailure]) {
    for failure in failures.iter().take(LOGGED_RECONCILE_FAILURES) {
        let (edge_a, edge_b) = failure.edge.endpoints();
        tracing::error!(
            target: "meerkat_meta::unified_runtime",
            operation = failure.operation.as_str(),
            edge_a,
            edge_b,
            error = failure.error.as_str(),
            "edge reconcile operation failed"
        );
    }
    let more = failures.len().saturating_sub(LOGGED_RECONCILE_FAILURES);
    if more != 0 {
        tracing::error!(
            target: "meerkat_meta::unified_runtime",
            more,
            "edge reconcile operation failed: +{more} more"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod reconcile_incomplete_alert_tests {
    use super::*;

    fn edge(a: &str, b: &str) -> DesiredPeerEdge {
        DesiredPeerEdge::new(a, b).expect("desired peer edge")
    }

    #[test]
    fn a_failed_edge_pages_with_the_skipped_count_beside_it() {
        let report = UnifiedRuntimeReconcileEdgesReport {
            skipped_missing_members: vec![edge("a", "b")],
            failures: vec![EdgeReconcileFailure {
                edge: edge("c", "d"),
                operation: "wire".to_string(),
                error: "refused".to_string(),
            }],
            ..Default::default()
        };
        assert_eq!(
            reconcile_incomplete_alert(&report),
            Some(ErrorEvent::ReconcileIncomplete {
                failures: 1,
                skipped: 1,
                failed: report.failures.clone(),
            })
        );
    }

    #[test]
    fn skipped_edges_alone_do_not_page() {
        let report = UnifiedRuntimeReconcileEdgesReport {
            skipped_missing_members: vec![edge("a", "b"), edge("a", "c")],
            ..Default::default()
        };
        assert!(!report.is_complete(), "the topology is still incomplete");
        assert_eq!(reconcile_incomplete_alert(&report), None);
        assert_eq!(
            reconcile_incomplete_alert(&UnifiedRuntimeReconcileEdgesReport::default()),
            None
        );
    }

    /// An early return replaces the whole report with one synthetic failure
    /// (here the recovery preparation's): the failure's own line names its
    /// operation, edge and error, as the alert does.
    #[test]
    fn a_synthetic_early_return_logs_its_operation_edge_and_error() {
        let report = crate::topology_control::recovery_failure_report(
            crate::topology_control::TopologyControlError::InvalidRequest(
                "invalid persisted topology identity".to_string(),
            ),
        );
        assert_eq!(report.failures.len(), 1, "one synthetic failure");
        let ((), logged) =
            super::super::tests::capture_tracing(|| log_reconcile_failures(&report.failures));
        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(lines.len(), 1, "one line for the one failure: {logged}");
        for expected in [
            "ERROR meerkat_meta::unified_runtime: edge reconcile operation failed",
            "operation=\"prepare_recovery\"",
            "edge_a=\"recovery-journal\"",
            "edge_b=\"topology-control\"",
            "error=\"invalid topology request: invalid persisted topology identity\"",
        ] {
            assert!(
                lines[0].contains(expected),
                "{expected} is missing: {logged}"
            );
        }
    }

    /// A line each for the first five failures and one line counting the
    /// rest: a reconcile whose every edge failed does not flood the log.
    #[test]
    fn failures_past_the_first_five_are_counted_on_one_line() {
        let failures: Vec<EdgeReconcileFailure> = (0..7)
            .map(|index| EdgeReconcileFailure {
                edge: edge("lead", &format!("worker-{index}")),
                operation: "wire".to_string(),
                error: "refused".to_string(),
            })
            .collect();
        let ((), logged) =
            super::super::tests::capture_tracing(|| log_reconcile_failures(&failures));
        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(lines.len(), 6, "five failures and the count: {logged}");
        for (index, line) in lines[..5].iter().enumerate() {
            assert!(
                line.contains(&format!("edge_b=\"worker-{index}\"")),
                "failure {index} is logged in order: {logged}"
            );
        }
        assert!(
            lines[5].contains("edge reconcile operation failed: +2 more")
                && lines[5].contains("more=2"),
            "the rest are counted: {logged}"
        );
    }

    /// An alert recorded before it named its failed operations reads with
    /// none.
    #[test]
    fn an_alert_recorded_with_counts_only_reads_with_no_failed_operations() {
        let recorded: ErrorEvent = serde_json::from_value(serde_json::json!({
            "category": "reconcile_incomplete",
            "failures": 2,
            "skipped": 1,
        }))
        .expect("a counts-only alert reads");
        assert_eq!(
            recorded,
            ErrorEvent::ReconcileIncomplete {
                failures: 2,
                skipped: 1,
                failed: Vec::new(),
            }
        );
    }

    /// The alert itself names the failed edge operations, the first five in
    /// order, each with its operation, edge and error, beside the counts: an
    /// `on_error` receiver needs no log line to say what failed.
    #[test]
    fn a_failed_edge_alert_names_each_failed_operation() {
        let report = UnifiedRuntimeReconcileEdgesReport {
            skipped_missing_members: vec![edge("a", "b")],
            failures: (0..7)
                .map(|index| EdgeReconcileFailure {
                    edge: edge("lead", &format!("worker-{index}")),
                    operation: "wire".to_string(),
                    error: format!("refused {index}"),
                })
                .collect(),
            ..Default::default()
        };
        let alert = reconcile_incomplete_alert(&report).expect("a failed edge pages");
        let wire = serde_json::to_value(&alert).expect("the alert serializes");
        assert_eq!(wire["category"], "reconcile_incomplete", "{wire}");
        assert_eq!(wire["failures"], 7, "{wire}");
        assert_eq!(wire["skipped"], 1, "{wire}");
        assert!(
            wire["failed"].is_array(),
            "the alert names the failed operations: {wire}"
        );
        let failed = wire["failed"].as_array().expect("the failed operations");
        assert_eq!(failed.len(), 5, "the first five: {wire}");
        for (index, failure) in failed.iter().enumerate() {
            assert_eq!(
                *failure,
                serde_json::to_value(&report.failures[index]).expect("a failure serializes"),
                "failure {index} in order: {wire}"
            );
        }
        let shown = alert.to_string();
        assert!(
            shown.contains("wire") && shown.contains("worker-0") && shown.contains("refused 0"),
            "the alert's line names the first failure: {shown}"
        );
    }
}
