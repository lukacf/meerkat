// @generated — protocol helpers for `comms_trust_reconcile`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: CommsTrustReconcileRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!("__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_comms_trust_reconcile_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[derive(Clone)]
pub struct PeerProjectionFreshnessAuthority {
    authority: Option<
        std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>,
    >,
    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for PeerProjectionFreshnessAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerProjectionFreshnessAuthority")
            .field("present", &self.authority.is_some())
            .field("owner_present", &self.source_owner_token.is_some())
            .finish()
    }
}

impl PeerProjectionFreshnessAuthority {
    pub fn from_authority(
        authority: std::sync::Arc<
            std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>,
        >,
    ) -> Self {
        let source_owner_token = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generated_authority_owner_token();
        Self {
            authority: Some(authority),
            source_owner_token: Some(source_owner_token),
        }
    }

    fn missing() -> Self {
        Self {
            authority: None,
            source_owner_token: None,
        }
    }

    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {
        self.source_owner_token.as_ref().map(std::sync::Arc::clone)
    }

    fn validate_peer_projection_epoch(&self, expected_epoch: u64) -> Result<(), String> {
        let Some(authority) = &self.authority else {
            return Err("generated peer projection freshness authority is absent".to_string());
        };
        let guard = authority.lock().map_err(|_| {
            "generated peer projection freshness authority was poisoned".to_string()
        })?;
        let current_epoch = guard.state().peer_projection_epoch;
        if current_epoch == expected_epoch {
            Ok(())
        } else {
            Err(format!(
                "stale generated peer projection trust obligation at epoch {expected_epoch} (current {current_epoch})"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommsTrustReconcileObligation {
    local_endpoint: Option<PeerEndpoint>,
    peer_projection_epoch: u64,
    direct_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    mob_overlay_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    peer_projection_freshness_authority: PeerProjectionFreshnessAuthority,
}

impl CommsTrustReconcileObligation {
    pub fn local_endpoint(&self) -> &Option<PeerEndpoint> {
        &self.local_endpoint
    }

    pub fn peer_projection_epoch(&self) -> u64 {
        self.peer_projection_epoch
    }

    pub fn direct_peer_endpoints(&self) -> &std::collections::BTreeSet<PeerEndpoint> {
        &self.direct_peer_endpoints
    }

    pub fn mob_overlay_peer_endpoints(&self) -> &std::collections::BTreeSet<PeerEndpoint> {
        &self.mob_overlay_peer_endpoints
    }
}

fn trusted_peer_descriptor_from_peer_endpoint(
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.as_str(),
        endpoint.signing_key.0,
        endpoint.address.0.as_str(),
    )
}

fn trusted_peer_descriptor_for_request(
    obligation: &CommsTrustReconcileObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    let mut matches = obligation
        .direct_peer_endpoints
        .iter()
        .chain(obligation.mob_overlay_peer_endpoints.iter())
        .filter(|endpoint| endpoint.peer_id.0 == peer_id);
    let Some(endpoint) = matches.next() else {
        return Err(format!(
            "MeerkatMachine peer projection did not request trust for peer {peer_id:?}"
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "MeerkatMachine peer projection has ambiguous endpoint descriptors for peer {peer_id:?}"
        ));
    }
    trusted_peer_descriptor_from_peer_endpoint(endpoint)
}

impl CommsTrustReconcileObligation {
    #[allow(unsafe_code)]
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PublicAdd | Operation::PublicRemove) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        self.peer_projection_freshness_authority
            .validate_peer_projection_epoch(self.peer_projection_epoch)?;
        match operation {
            Operation::PublicAdd => {
                let requested = self
                    .direct_peer_endpoints
                    .iter()
                    .chain(self.mob_overlay_peer_endpoints.iter())
                    .any(|endpoint| endpoint.peer_id.0 == peer_id);
                if !requested {
                    return Err(format!(
                        "MeerkatMachine peer projection did not request trust for peer {peer_id:?}"
                    ));
                }
            }
            Operation::PublicRemove => {
                let still_requested = self
                    .direct_peer_endpoints
                    .iter()
                    .chain(self.mob_overlay_peer_endpoints.iter())
                    .any(|endpoint| endpoint.peer_id.0 == peer_id);
                if still_requested {
                    return Err(format!(
                        "MeerkatMachine peer projection still requests trust for peer {peer_id:?}"
                    ));
                }
            }
            _ => unreachable!("operation checked above"),
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
            #[link_name = concat!("__meerkat_core_runtime_generated_comms_trust_authority_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
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
                if expected_descriptor != peer_descriptor {
                    return Err(format!(
                        "generated comms trust descriptor for peer {peer_id:?} does not match requested mutation descriptor"
                    ));
                }
                let trust_store_peer_id = self.local_endpoint.as_ref().ok_or_else(|| "generated MeerkatMachine trust obligation did not carry local trust-store endpoint".to_string())?.peer_id.0.as_str().to_string();
                let generated_peer_id = peer_descriptor.peer_id.to_string();
                #[allow(unsafe_code)]
                unsafe {
                    core_generated_comms_trust_authority_build(
                        generated_authority_bridge_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                        self.peer_projection_epoch,
                        self.peer_projection_freshness_authority.source_owner_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
                        generated_peer_id,
                        Some(trust_store_peer_id),
                        Some(peer_descriptor),
                    )
                }
            }
            Operation::PublicRemove => {
                if peer_descriptor.is_some() {
                    return Err(format!(
                        "generated comms trust remove for peer {peer_id:?} must not carry a trusted peer descriptor"
                    ));
                }
                let trust_store_peer_id = self.local_endpoint.as_ref().ok_or_else(|| "generated MeerkatMachine trust obligation did not carry local trust-store endpoint".to_string())?.peer_id.0.as_str().to_string();
                #[allow(unsafe_code)]
                unsafe {
                    core_generated_comms_trust_authority_build(
                        generated_authority_bridge_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                        self.peer_projection_epoch,
                        self.peer_projection_freshness_authority.source_owner_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicRemove,
                        peer_id.to_string(),
                        Some(trust_store_peer_id),
                        None,
                    )
                }
            }
            _ => unreachable!("operation checked above"),
        }
    }
}

pub fn extract_obligations(
    transition: &MeerkatMachineTransition,
) -> Vec<CommsTrustReconcileObligation> {
    extract_obligations_with_freshness(transition, PeerProjectionFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MeerkatMachineTransition,
    peer_projection_freshness_authority: PeerProjectionFreshnessAuthority,
) -> Vec<CommsTrustReconcileObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::CommsTrustReconcileRequested {
                local_endpoint,
                peer_projection_epoch,
                direct_peer_endpoints,
                mob_overlay_peer_endpoints,
            } => Some(CommsTrustReconcileObligation {
                local_endpoint: local_endpoint.clone(),
                peer_projection_epoch: *peer_projection_epoch,
                direct_peer_endpoints: direct_peer_endpoints.clone(),
                mob_overlay_peer_endpoints: mob_overlay_peer_endpoints.clone(),
                comms_trust_authority_claims: Default::default(),
                peer_projection_freshness_authority: peer_projection_freshness_authority.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub fn effective_peers(
    obligation: &CommsTrustReconcileObligation,
) -> std::collections::BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint> {
    obligation
        .direct_peer_endpoints
        .iter()
        .chain(obligation.mob_overlay_peer_endpoints.iter())
        .cloned()
        .collect()
}

pub fn peer_projection_epoch(obligation: &CommsTrustReconcileObligation) -> u64 {
    obligation.peer_projection_epoch
}

pub fn authority_for_endpoint(
    obligation: &CommsTrustReconcileObligation,
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    if !effective_peers(obligation).contains(endpoint) {
        return Err(format!(
            "MeerkatMachine peer projection did not request trust for peer '{}'",
            endpoint.peer_id.0
        ));
    }
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        endpoint.peer_id.0.as_str(),
        Some(trusted_peer_descriptor_from_peer_endpoint(endpoint)?),
    )
}

pub fn removal_authority_for_peer_id(
    obligation: &CommsTrustReconcileObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    if effective_peers(obligation)
        .iter()
        .any(|endpoint| endpoint.peer_id.0 == peer_id)
    {
        return Err(format!(
            "MeerkatMachine peer projection still requests trust for peer '{peer_id}'"
        ));
    }
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicRemove,
        peer_id,
        None,
    )
}
