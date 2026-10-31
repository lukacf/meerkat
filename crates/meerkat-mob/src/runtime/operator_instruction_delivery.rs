//! Operator-instruction delivery: the shell around MobMachine's
//! operator-instruction region. Not realized yet: every verb refuses.

use meerkat_core::{InstructionRevision, InstructionSlot};

use super::handle::MobHandle;
use crate::error::MobError;
use crate::operator_instruction::{
    OperatorInstructionError, OperatorInstructionReceipt, OperatorInstructionStatusPage,
    OperatorInstructionStatusQuery, OperatorInstructionWrite,
};

/// The default bound on concurrent dormant (store-only) deliveries.
pub(crate) const DEFAULT_DORMANT_DELIVERY_CONCURRENCY: usize = 16;

fn not_realized() -> OperatorInstructionError {
    OperatorInstructionError::Mob(MobError::Internal(
        "this mob does not realize operator instructions yet".to_string(),
    ))
}

impl MobHandle {
    /// Write one operator instruction (not realized yet).
    ///
    /// # Errors
    ///
    /// Always, for now.
    pub async fn set_operator_instruction(
        &self,
        write: OperatorInstructionWrite,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        let _ = write;
        Err(not_realized())
    }

    /// Revoke one operator instruction (not realized yet).
    ///
    /// # Errors
    ///
    /// Always, for now.
    pub async fn revoke_operator_instruction(
        &self,
        slot: InstructionSlot,
        revision: InstructionRevision,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        let _ = (slot, revision);
        Err(not_realized())
    }

    /// Read the mob's operator-instruction status (not realized yet).
    ///
    /// # Errors
    ///
    /// Always, for now.
    pub async fn operator_instruction_status(
        &self,
        query: OperatorInstructionStatusQuery,
    ) -> Result<OperatorInstructionStatusPage, OperatorInstructionError> {
        let _ = query;
        Err(not_realized())
    }
}
