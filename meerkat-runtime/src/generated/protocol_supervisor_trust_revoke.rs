// @generated — protocol helpers for `supervisor_trust_revoke`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: RevokeSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!("__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_supervisor_trust_revoke_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[derive(Debug, Clone)]
pub struct SupervisorTrustRevokeObligation {
    local_endpoint: Option<PeerEndpoint>,
    peer_id: String,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl SupervisorTrustRevokeObligation {
    pub fn local_endpoint(&self) -> &Option<PeerEndpoint> {
        &self.local_endpoint
    }

    pub fn peer_id(&self) -> &String {
        &self.peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl SupervisorTrustRevokeObligation {
    #[allow(unsafe_code)]
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PrivateRemove) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
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
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorRevoke,
                        self.epoch,
                        None,
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
) -> Vec<SupervisorTrustRevokeObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::RevokeSupervisorTrustEdge {
                local_endpoint,
                peer_id,
                epoch,
            } => Some(SupervisorTrustRevokeObligation {
                local_endpoint: local_endpoint.clone(),
                peer_id: peer_id.clone(),
                epoch: *epoch,
                comms_trust_authority_claims: Default::default(),
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

pub fn revoke_authority_for_peer(
    obligation: &SupervisorTrustRevokeObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorRevoke",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,
        obligation.peer_id.as_str(),
        None,
    )
}
