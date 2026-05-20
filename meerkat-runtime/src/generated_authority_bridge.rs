use std::any::Any;

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

pub(crate) fn generated_authority_bridge_token() -> &'static (dyn Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ForgedGeneratedAuthorityBridgeToken;

    static FORGED_GENERATED_AUTHORITY_BRIDGE_TOKEN: ForgedGeneratedAuthorityBridgeToken =
        ForgedGeneratedAuthorityBridgeToken;

    #[test]
    fn core_runtime_comms_bridge_rejects_forged_token() {
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!(
                "__meerkat_core_runtime_generated_comms_trust_authority_build_v1_",
                env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
            )]
            fn core_runtime_generated_comms_trust_authority_build(
                token: &'static (dyn Any + Send + Sync),
                source_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                source_epoch: u64,
                trust_row_owner_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
                peer_id: String,
                trust_store_peer_id: Option<String>,
                peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
            ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String>;
        }

        #[allow(unsafe_code)]
        let result = unsafe {
            core_runtime_generated_comms_trust_authority_build(
                &FORGED_GENERATED_AUTHORITY_BRIDGE_TOKEN,
                meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                7,
                meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicRemove,
                "peer-a".to_string(),
                Some("trust-store".to_string()),
                None,
            )
        };

        assert!(matches!(
            result,
            Err(message) if message.contains("canonical runtime bridge token")
        ));
    }
}
