// @generated — protocol helpers for `mob_member_trust_wiring`
// Composition: adaptive_mob_bundle, Producer: control_mob, Effect: MemberTrustWiringRequested
// Closure policy: PublicationOnly
// Liveness: generated authority publication is consumed by the owning runtime; no source-machine feedback is declared

use crate::machines::mob_machine::{
    AgentIdentity, MemberPeerEndpoint, MobMachineAuthority, MobMachineEffect, MobMachineTransition,
    MobPhase, PeerId, WiringEdge,
};

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

pub(crate) fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!("__meerkat_mob_generated_authority_bridge_token_is_valid_v1_mob_member_trust_wiring_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[derive(Debug, Clone)]
pub struct MobTopologyFreshnessAuthority {
    topology_epoch: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    prepared_batch: Option<PreparedBatchTopologyFreshness>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedBatchBaseTopology {
    lifecycle_phase: MobPhase,
    topology_epoch: u64,
    wiring_edges: std::collections::BTreeSet<WiringEdge>,
    member_peer_ids: std::collections::BTreeMap<AgentIdentity, PeerId>,
    member_peer_endpoints: std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,
}

impl PreparedBatchBaseTopology {
    fn from_authority(authority: &MobMachineAuthority) -> Self {
        let state = authority.state();
        Self {
            lifecycle_phase: state.lifecycle_phase,
            topology_epoch: state.topology_epoch,
            wiring_edges: state.wiring_edges.clone(),
            member_peer_ids: state.member_peer_ids.clone(),
            member_peer_endpoints: state.member_peer_endpoints.clone(),
        }
    }

    fn validate_live_authority(&self, authority: &MobMachineAuthority) -> Result<(), String> {
        let state = authority.state();
        let live_epoch = state.topology_epoch;
        if state.lifecycle_phase != self.lifecycle_phase || live_epoch != self.topology_epoch {
            return Err(format!(
                "stale generated MobMachine prepared trust batch at base epoch {} (current {live_epoch})",
                self.topology_epoch
            ));
        }
        if state.wiring_edges != self.wiring_edges
            || state.member_peer_ids != self.member_peer_ids
            || state.member_peer_endpoints != self.member_peer_endpoints
        {
            return Err(format!(
                "stale generated MobMachine prepared trust batch base topology changed at epoch {}",
                self.topology_epoch
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PreparedBatchTopologyFreshness {
    base: PreparedBatchBaseTopology,
    obligations: std::collections::BTreeSet<PreparedBatchMemberTrustObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PreparedBatchMemberTrustObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    a_endpoint: MemberPeerEndpoint,
    b_endpoint: MemberPeerEndpoint,
    epoch: u64,
}

impl PreparedBatchMemberTrustObligation {
    fn from_effect(effect: &MobMachineEffect) -> Option<Self> {
        match effect {
            MobMachineEffect::MemberTrustWiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                a_endpoint,
                b_endpoint,
                epoch,
            } => Some(Self {
                edge: edge.clone(),
                a_peer_id: a_peer_id.clone(),
                b_peer_id: b_peer_id.clone(),
                a_endpoint: a_endpoint.clone(),
                b_endpoint: b_endpoint.clone(),
                epoch: *epoch,
            }),
            _ => None,
        }
    }

    fn from_obligation(obligation: &MobMemberTrustWiringObligation) -> Self {
        Self {
            edge: obligation.edge.clone(),
            a_peer_id: obligation.a_peer_id.clone(),
            b_peer_id: obligation.b_peer_id.clone(),
            a_endpoint: obligation.a_endpoint.clone(),
            b_endpoint: obligation.b_endpoint.clone(),
            epoch: obligation.epoch,
        }
    }
}

fn validate_live_member_trust_obligation(
    authority: &MobMachineAuthority,
    obligation: &MobMemberTrustWiringObligation,
) -> Result<(), String> {
    let state = authority.state();
    if state.lifecycle_phase != MobPhase::Running {
        return Err(
            "generated MobMachine member trust authority requires live Running phase".to_string(),
        );
    }
    if state.topology_epoch != obligation.epoch {
        return Err(format!(
            "stale generated MobMachine trust obligation at epoch {} (current {})",
            obligation.epoch, state.topology_epoch
        ));
    }
    if !state.wiring_edges.contains(&obligation.edge) {
        return Err(format!(
            "generated MobMachine member trust edge {:?} is not live",
            obligation.edge
        ));
    }
    if state.member_peer_ids.get(&obligation.edge.a) != Some(&obligation.a_peer_id) {
        return Err(format!(
            "generated MobMachine member trust peer id for {:?} is stale",
            obligation.edge.a
        ));
    }
    if state.member_peer_ids.get(&obligation.edge.b) != Some(&obligation.b_peer_id) {
        return Err(format!(
            "generated MobMachine member trust peer id for {:?} is stale",
            obligation.edge.b
        ));
    }
    if state.member_peer_endpoints.get(&obligation.edge.a) != Some(&obligation.a_endpoint) {
        return Err(format!(
            "generated MobMachine member trust endpoint for {:?} is stale",
            obligation.edge.a
        ));
    }
    if state.member_peer_endpoints.get(&obligation.edge.b) != Some(&obligation.b_endpoint) {
        return Err(format!(
            "generated MobMachine member trust endpoint for {:?} is stale",
            obligation.edge.b
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct MobTopologyPreparedBatchAuthority {
    source_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    prepared_base: PreparedBatchBaseTopology,
}

impl MobTopologyPreparedBatchAuthority {
    pub(crate) fn from_live_authority(authority: &MobMachineAuthority) -> Self {
        let prepared_base = PreparedBatchBaseTopology::from_authority(authority);
        let source_owner_token = authority.generated_authority_owner_token();
        Self {
            source_owner_token,
            prepared_base,
        }
    }

    pub(crate) fn freshness_for_prepared_transitions<'a>(
        &self,
        live_authority: &MobMachineAuthority,
        transitions: impl IntoIterator<Item = &'a MobMachineTransition>,
    ) -> Result<MobTopologyFreshnessAuthority, String> {
        let live_owner_token = live_authority.generated_authority_owner_token();
        if !std::sync::Arc::ptr_eq(&self.source_owner_token, &live_owner_token) {
            return Err("generated MobMachine prepared trust batch was minted by a different live authority owner".to_string());
        }
        self.prepared_base.validate_live_authority(live_authority)?;
        let mut obligation_epochs = std::collections::BTreeSet::new();
        let mut obligations = std::collections::BTreeSet::new();
        for transition in transitions {
            for effect in transition.effects() {
                if let Some(obligation) = PreparedBatchMemberTrustObligation::from_effect(effect) {
                    let epoch = obligation.epoch;
                    if !obligation_epochs.insert(epoch) {
                        return Err(format!(
                            "prepared MobMachine member trust batch carried duplicate obligation epoch {epoch}"
                        ));
                    }
                    obligations.insert(obligation);
                }
            }
        }
        if obligations.is_empty() {
            return Err(
                "prepared MobMachine batch did not carry member trust wiring obligations"
                    .to_string(),
            );
        }
        Ok(MobTopologyFreshnessAuthority {
            topology_epoch: None,
            source_owner_token: Some(std::sync::Arc::clone(&self.source_owner_token)),
            prepared_batch: Some(PreparedBatchTopologyFreshness {
                base: self.prepared_base.clone(),
                obligations,
            }),
        })
    }
}

impl MobTopologyFreshnessAuthority {
    pub(crate) fn from_live_topology_epoch(
        topology_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            topology_epoch: Some(topology_epoch),
            source_owner_token: Some(source_owner_token),
            prepared_batch: None,
        }
    }

    pub(crate) fn from_live_member_trust_authority(authority: &MobMachineAuthority) -> Self {
        let _ = authority.state();
        let source_owner_token = authority.generated_authority_owner_token();
        Self {
            topology_epoch: None,
            source_owner_token: Some(source_owner_token),
            prepared_batch: None,
        }
    }

    fn missing() -> Self {
        Self {
            topology_epoch: None,
            source_owner_token: None,
            prepared_batch: None,
        }
    }

    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {
        self.source_owner_token.as_ref().map(std::sync::Arc::clone)
    }

    fn validate_topology_epoch(
        &self,
        expected_epoch: u64,
        allow_next_epoch: bool,
    ) -> Result<(), String> {
        if self.prepared_batch.is_some() {
            return Err("generated MobMachine prepared trust freshness requires exact obligation validation".to_string());
        }
        let Some(topology_epoch) = &self.topology_epoch else {
            return Err("generated MobMachine topology freshness authority is absent".to_string());
        };
        let current_epoch = topology_epoch.load(std::sync::atomic::Ordering::Acquire);
        debug_assert!(self.prepared_batch.is_none());
        let matches_current = current_epoch == expected_epoch;
        let matches_next = allow_next_epoch && current_epoch.checked_add(1) == Some(expected_epoch);
        if matches_current || matches_next {
            Ok(())
        } else {
            Err(format!(
                "stale generated MobMachine trust obligation at epoch {expected_epoch} (current {current_epoch})"
            ))
        }
    }

    fn validate_live_authority_owner(
        &self,
        live_authority: &MobMachineAuthority,
    ) -> Result<(), String> {
        let Some(source_owner_token) = &self.source_owner_token else {
            return Err(
                "generated MobMachine member trust freshness owner authority is absent".to_string(),
            );
        };
        let live_owner_token = live_authority.generated_authority_owner_token();
        if std::sync::Arc::ptr_eq(source_owner_token, &live_owner_token) {
            Ok(())
        } else {
            Err("generated MobMachine member trust freshness was minted by a different live authority owner".to_string())
        }
    }

    fn validate_member_trust_obligation(
        &self,
        obligation: &MobMemberTrustWiringObligation,
        allow_next_epoch: bool,
        prepared_live_authority: Option<&MobMachineAuthority>,
    ) -> Result<(), String> {
        if let Some(prepared_batch) = &self.prepared_batch {
            let Some(prepared_live_authority) = prepared_live_authority else {
                return Err("generated MobMachine prepared trust freshness requires live authority validation".to_string());
            };
            self.validate_live_authority_owner(prepared_live_authority)?;
            prepared_batch
                .base
                .validate_live_authority(prepared_live_authority)?;
            let expected = PreparedBatchMemberTrustObligation::from_obligation(obligation);
            if prepared_batch.obligations.contains(&expected) {
                return Ok(());
            }
            return Err(format!(
                "generated MobMachine prepared trust batch does not contain exact obligation for edge {:?} at epoch {} (base {})",
                obligation.edge, obligation.epoch, prepared_batch.base.topology_epoch
            ));
        }
        let Some(prepared_live_authority) = prepared_live_authority else {
            return Err(
                "generated MobMachine member trust freshness requires live authority validation"
                    .to_string(),
            );
        };
        self.validate_live_authority_owner(prepared_live_authority)?;
        let _ = allow_next_epoch;
        validate_live_member_trust_obligation(prepared_live_authority, obligation)
    }
}

#[derive(Debug, Clone)]
pub struct MobMemberTrustWiringObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    a_endpoint: MemberPeerEndpoint,
    b_endpoint: MemberPeerEndpoint,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
}

impl MobMemberTrustWiringObligation {
    pub fn edge(&self) -> &WiringEdge {
        &self.edge
    }

    pub fn a_peer_id(&self) -> &PeerId {
        &self.a_peer_id
    }

    pub fn b_peer_id(&self) -> &PeerId {
        &self.b_peer_id
    }

    pub fn a_endpoint(&self) -> &MemberPeerEndpoint {
        &self.a_endpoint
    }

    pub fn b_endpoint(&self) -> &MemberPeerEndpoint {
        &self.b_endpoint
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

fn trusted_peer_descriptor_from_member_endpoint(
    endpoint: &crate::machines::mob_machine::MemberPeerEndpoint,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.as_str(),
        endpoint.signing_key.0,
        endpoint.address.0.as_str(),
    )
}

fn trusted_peer_descriptor_for_request(
    obligation: &MobMemberTrustWiringObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    let a_matches = obligation.a_endpoint.peer_id.0 == peer_id;
    let b_matches = obligation.b_endpoint.peer_id.0 == peer_id;
    match (a_matches, b_matches) {
        (true, false) => trusted_peer_descriptor_from_member_endpoint(&obligation.a_endpoint),
        (false, true) => trusted_peer_descriptor_from_member_endpoint(&obligation.b_endpoint),
        (false, false) => Err(format!(
            "MobMachine member trust obligation does not carry requested peer {peer_id:?}"
        )),
        (true, true) => Err(format!(
            "MobMachine member trust obligation has ambiguous endpoint descriptors for peer {peer_id:?}"
        )),
    }
}

impl MobMemberTrustWiringObligation {
    #[allow(unsafe_code)]
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
        prepared_live_authority: Option<&MobMachineAuthority>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PublicAdd) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        self.mob_topology_freshness_authority
            .validate_member_trust_obligation(self, false, prepared_live_authority)?;
        if self.a_peer_id.0 != peer_id && self.b_peer_id.0 != peer_id {
            return Err(format!(
                "MobMachine member trust obligation does not carry requested peer {peer_id:?}"
            ));
        }
        let claim_key = format!("{operation:?}:{peer_id}");
        let mut claims = self.comms_trust_authority_claims.lock().map_err(|_| {
            "generated comms trust authority source claims were poisoned".to_string()
        })?;
        if !claims.insert(claim_key) {
            return Err(format!(
                "generated comms trust authority source already minted {operation:?} for peer {peer_id:?}"
            ));
        }
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!("__meerkat_core_mob_generated_comms_trust_authority_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
            fn core_generated_comms_trust_authority_build(
                token: &'static (dyn std::any::Any + Send + Sync),
                source_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                source_epoch: u64,
                source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
                trust_row_owner_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
                peer_id: String,
                trust_store_peer_id: Option<String>,
                peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
            ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String>;
        }
        match operation {
            Operation::PublicAdd => {
                let peer_descriptor = peer_descriptor.ok_or_else(|| format!("generated comms trust add for peer {peer_id:?} requires a trusted peer descriptor"))?;
                let expected_descriptor = trusted_peer_descriptor_for_request(self, peer_id)?;
                if expected_descriptor.peer_id != peer_descriptor.peer_id
                    || expected_descriptor.address != peer_descriptor.address
                    || expected_descriptor.pubkey != peer_descriptor.pubkey
                {
                    return Err(format!(
                        "generated comms trust descriptor for peer {peer_id:?} does not match requested mutation descriptor"
                    ));
                }
                let trust_store_peer_id = if self.a_peer_id.0 == peer_id { self.b_peer_id.0.as_str() } else if self.b_peer_id.0 == peer_id { self.a_peer_id.0.as_str() } else { return Err(format!("MobMachine member trust obligation does not carry requested peer {peer_id:?}")); }.to_string();
                let generated_peer_id = peer_descriptor.peer_id.to_string();
                #[allow(unsafe_code)]
                unsafe {
                    core_generated_comms_trust_authority_build(
                        generated_authority_bridge_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                        self.epoch,
                        self.mob_topology_freshness_authority.source_owner_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
                        generated_peer_id,
                        Some(trust_store_peer_id),
                        Some(peer_descriptor),
                    )
                }
            }
            _ => unreachable!("operation checked above"),
        }
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobMemberTrustWiringObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
) -> Vec<MobMemberTrustWiringObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberTrustWiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                a_endpoint,
                b_endpoint,
                epoch,
            } => Some(MobMemberTrustWiringObligation {
                edge: edge.clone(),
                a_peer_id: a_peer_id.clone(),
                b_peer_id: b_peer_id.clone(),
                a_endpoint: a_endpoint.clone(),
                b_endpoint: b_endpoint.clone(),
                epoch: *epoch,
                comms_trust_authority_claims: Default::default(),
                mob_topology_freshness_authority: mob_topology_freshness_authority.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn validate_expected_peer(
    context: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{context} peer id {actual:?} does not match expected mutation peer id {expected:?}"
        ))
    }
}

fn peer_id_for_identity<'a>(
    obligation: &'a MobMemberTrustWiringObligation,
    identity: &str,
) -> Option<&'a str> {
    if obligation.edge.a.0 == identity {
        Some(obligation.a_peer_id.0.as_str())
    } else if obligation.edge.b.0 == identity {
        Some(obligation.b_peer_id.0.as_str())
    } else {
        None
    }
}

fn required_peer_id_for_identity<'a>(
    obligation: &'a MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<&'a str, String> {
    let Some(actual) = peer_id_for_identity(obligation, identity) else {
        return Err(format!(
            "MobMachine member trust obligation does not cover identity {identity:?}"
        ));
    };
    validate_expected_peer("MobMachineMemberTrust", actual, expected_peer_id)?;
    Ok(actual)
}

pub fn wiring_authority_for_identity(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        peer_id,
        Some(trusted_peer_descriptor_for_request(obligation, peer_id)?),
        None,
    )
}

pub fn repair_authority_for_identity(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        peer_id,
        Some(trusted_peer_descriptor_for_request(obligation, peer_id)?),
        None,
    )
}

pub fn wiring_authority_for_identity_with_live_authority(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
    live_authority: &MobMachineAuthority,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        peer_id,
        Some(trusted_peer_descriptor_for_request(obligation, peer_id)?),
        Some(live_authority),
    )
}

pub fn repair_authority_for_identity_with_live_authority(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
    live_authority: &MobMachineAuthority,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        peer_id,
        Some(trusted_peer_descriptor_for_request(obligation, peer_id)?),
        Some(live_authority),
    )
}
