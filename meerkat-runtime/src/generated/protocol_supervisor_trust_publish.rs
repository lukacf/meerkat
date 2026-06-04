// @generated — protocol helpers for `supervisor_trust_publish`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: PublishSupervisorTrustEdge
// Closure policy: PublicationOnly
// Liveness: generated supervisor trust authority publication is consumed under comms transport liveness

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

pub(crate) fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!("__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_supervisor_trust_publish_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[derive(Clone)]
pub struct SupervisorTrustFreshnessAuthority {
    authority: Option<
        std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>,
    >,
    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for SupervisorTrustFreshnessAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupervisorTrustFreshnessAuthority")
            .field("present", &self.authority.is_some())
            .field("owner_present", &self.source_owner_token.is_some())
            .finish()
    }
}

#[allow(dead_code)]
impl SupervisorTrustFreshnessAuthority {
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

    fn validate_pending_publish(
        &self,
        expected_peer_id: &str,
        expected_name: &str,
        expected_address: &str,
        expected_signing_public_key: Option<&str>,
        expected_epoch: u64,
        expected_local_endpoint: &Option<PeerEndpoint>,
    ) -> Result<(), String> {
        let Some(authority) = &self.authority else {
            return Err("generated supervisor trust freshness authority is absent".to_string());
        };
        let guard = authority.lock().map_err(|_| {
            "generated supervisor trust freshness authority was poisoned".to_string()
        })?;
        let state = guard.state();
        if state.supervisor_publish_pending_peer_id.as_deref() == Some(expected_peer_id)
            && state.supervisor_publish_pending_name.as_deref() == Some(expected_name)
            && state.supervisor_publish_pending_address.as_deref() == Some(expected_address)
            && state
                .supervisor_publish_pending_signing_public_key
                .as_deref()
                == expected_signing_public_key
            && state.supervisor_publish_pending_epoch == Some(expected_epoch)
            && state.local_endpoint.as_ref() == expected_local_endpoint.as_ref()
        {
            Ok(())
        } else {
            Err(format!(
                "stale generated supervisor trust publish obligation for peer {expected_peer_id:?} at epoch {expected_epoch}"
            ))
        }
    }

    fn validate_pending_revoke(
        &self,
        expected_peer_id: &str,
        expected_epoch: u64,
        expected_local_endpoint: &Option<PeerEndpoint>,
    ) -> Result<(), String> {
        let Some(authority) = &self.authority else {
            return Err("generated supervisor trust freshness authority is absent".to_string());
        };
        let guard = authority.lock().map_err(|_| {
            "generated supervisor trust freshness authority was poisoned".to_string()
        })?;
        let state = guard.state();
        if state.supervisor_revoke_pending_peer_id.as_deref() == Some(expected_peer_id)
            && state.supervisor_revoke_pending_epoch == Some(expected_epoch)
            && state.local_endpoint.as_ref() == expected_local_endpoint.as_ref()
        {
            Ok(())
        } else {
            Err(format!(
                "stale generated supervisor trust revoke obligation for peer {expected_peer_id:?} at epoch {expected_epoch}"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorTrustPublishObligation {
    local_endpoint: Option<PeerEndpoint>,
    peer_id: String,
    name: String,
    address: String,
    signing_public_key: Option<String>,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    supervisor_trust_freshness_authority: SupervisorTrustFreshnessAuthority,
}

impl SupervisorTrustPublishObligation {
    pub fn local_endpoint(&self) -> &Option<PeerEndpoint> {
        &self.local_endpoint
    }

    pub fn peer_id(&self) -> &String {
        &self.peer_id
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn address(&self) -> &String {
        &self.address
    }

    pub fn signing_public_key(&self) -> &Option<String> {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

fn trusted_peer_descriptor_for_request(
    obligation: &SupervisorTrustPublishObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    if obligation.peer_id != peer_id {
        return Err(format!(
            "MeerkatMachine supervisor trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
            obligation.peer_id
        ));
    }
    let signing_public_key = obligation.signing_public_key.as_ref().ok_or_else(|| {
        "generated supervisor trust publish obligation omitted signing public key".to_string()
    })?;
    let pubkey = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)?;
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        obligation.name.clone(),
        obligation.peer_id.clone(),
        pubkey,
        obligation.address.clone(),
    )
}

impl SupervisorTrustPublishObligation {
    #[allow(unsafe_code)]
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PrivateAdd | Operation::PrivateRemove) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        self.supervisor_trust_freshness_authority
            .validate_pending_publish(
                self.peer_id.as_str(),
                self.name.as_str(),
                self.address.as_str(),
                self.signing_public_key.as_deref(),
                self.epoch,
                &self.local_endpoint,
            )?;
        if self.peer_id != peer_id {
            return Err(format!(
                "MeerkatMachine supervisor trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
                self.peer_id
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
            Operation::PrivateAdd => {
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
                let trust_store_peer_id = self.local_endpoint.as_ref().ok_or_else(|| "generated MeerkatMachine trust obligation did not carry local trust-store endpoint".to_string())?.peer_id.0.as_str().to_string();
                let generated_peer_id = peer_descriptor.peer_id.to_string();
                #[allow(unsafe_code)]
                unsafe {
                    core_generated_comms_trust_authority_build(
                        generated_authority_bridge_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                        self.epoch,
                        self.supervisor_trust_freshness_authority.source_owner_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateAdd,
                        generated_peer_id,
                        Some(trust_store_peer_id),
                        Some(peer_descriptor),
                    )
                }
            }
            Operation::PrivateRemove => {
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
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                        self.epoch,
                        self.supervisor_trust_freshness_authority.source_owner_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,
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
) -> Vec<SupervisorTrustPublishObligation> {
    extract_obligations_with_freshness(transition, SupervisorTrustFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MeerkatMachineTransition,
    supervisor_trust_freshness_authority: SupervisorTrustFreshnessAuthority,
) -> Vec<SupervisorTrustPublishObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::PublishSupervisorTrustEdge {
                local_endpoint,
                peer_id,
                name,
                address,
                signing_public_key,
                epoch,
            } => Some(SupervisorTrustPublishObligation {
                local_endpoint: local_endpoint.clone(),
                peer_id: peer_id.clone(),
                name: name.clone(),
                address: address.clone(),
                signing_public_key: signing_public_key.clone(),
                epoch: *epoch,
                comms_trust_authority_claims: Default::default(),
                supervisor_trust_freshness_authority: supervisor_trust_freshness_authority.clone(),
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

pub fn publish_authority_for_peer(
    obligation: &SupervisorTrustPublishObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorPublish",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateAdd,
        expected_peer_id,
        Some(trusted_peer_descriptor_for_request(
            obligation,
            expected_peer_id,
        )?),
    )
}

pub fn cleanup_authority_for_peer(
    obligation: &SupervisorTrustPublishObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorPublishCleanup",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,
        obligation.peer_id.as_str(),
        None,
    )
}
