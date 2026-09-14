//! Shared generated request model and native committed-store realization.

pub mod dsl;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod store;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LiveInputAdmissionValidation {
    input_digest: [u8; 32],
}

impl LiveInputAdmissionValidation {
    pub(crate) fn validate(
        &self,
        input: &crate::input::Input,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        use sha2::{Digest, Sha256};
        let encoded = serde_json::to_vec(input)
            .map_err(|error| crate::traits::RuntimeDriverError::Internal(error.to_string()))?;
        let digest: [u8; 32] = Sha256::digest(encoded).into();
        if digest != self.input_digest {
            return Err(crate::traits::RuntimeDriverError::ValidationFailed {
                reason: "Live admission handoff does not bind this exact input".into(),
            });
        }
        Ok(())
    }
}
