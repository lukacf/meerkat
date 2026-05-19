// @generated -- Generated comms trust authority handoff helpers.

use crate::comms::CommsTrustMutationAuthority;

/// Generated-only source contract.
///
/// Implement only for typed facts emitted from generated machine/composition
/// transitions. Production implementations live in generated modules owned by
/// the emitting machine/composition seam; handwritten runtime code consumes the
/// resulting handoff objects and must not implement this trait to mint trust
/// authority.
pub trait GeneratedMeerkatMachinePeerProjectionHandoff {
    fn peer_id(&self) -> &str;
    fn epoch(&self) -> u64;
}

/// Generated-only source contract.
///
/// Implement only for typed facts emitted from generated machine/composition
/// transitions. Production implementations live in generated modules owned by
/// the emitting machine/composition seam; handwritten runtime code consumes the
/// resulting handoff objects and must not implement this trait to mint trust
/// authority.
pub trait GeneratedMeerkatMachineSupervisorTrustHandoff {
    fn peer_id(&self) -> &str;
    fn epoch(&self) -> u64;
}

/// Generated-only source contract.
///
/// Implement only for typed facts emitted from generated machine/composition
/// transitions. Production implementations live in generated modules owned by
/// the emitting machine/composition seam; handwritten runtime code consumes the
/// resulting handoff objects and must not implement this trait to mint trust
/// authority.
pub trait GeneratedMobMachineMemberTrustHandoff {
    fn edge_a(&self) -> &str;
    fn edge_b(&self) -> &str;
    fn a_peer_id(&self) -> &str;
    fn b_peer_id(&self) -> &str;
    fn epoch(&self) -> u64;
}

/// Generated-only source contract.
///
/// Implement only for typed facts emitted from generated machine/composition
/// transitions. Production implementations live in generated modules owned by
/// the emitting machine/composition seam; handwritten runtime code consumes the
/// resulting handoff objects and must not implement this trait to mint trust
/// authority.
pub trait GeneratedMobMachineExternalPeerTrustHandoff {
    fn peer_id(&self) -> &str;
    fn epoch(&self) -> u64;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatMachinePeerProjectionHandoff {
    peer_id: String,
    epoch: u64,
}

impl MeerkatMachinePeerProjectionHandoff {
    pub fn from_generated_projection(
        source: &impl GeneratedMeerkatMachinePeerProjectionHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
        }
    }

    pub fn authority_for(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        validate_expected_peer(
            "MeerkatMachinePeerProjection",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(
            CommsTrustMutationAuthority::from_meerkat_machine_peer_projection(
                self.peer_id.clone(),
                self.epoch,
            ),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatMachineSupervisorTrustHandoff {
    peer_id: String,
    epoch: u64,
    operation: MeerkatMachineSupervisorTrustOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeerkatMachineSupervisorTrustOperation {
    Publish,
    Revoke,
}

impl MeerkatMachineSupervisorTrustHandoff {
    pub fn from_generated_supervisor_publish(
        source: &impl GeneratedMeerkatMachineSupervisorTrustHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
            operation: MeerkatMachineSupervisorTrustOperation::Publish,
        }
    }

    pub fn from_generated_supervisor_revoke(
        source: &impl GeneratedMeerkatMachineSupervisorTrustHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
            operation: MeerkatMachineSupervisorTrustOperation::Revoke,
        }
    }

    pub fn publish_authority_for(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MeerkatMachineSupervisorTrustOperation::Publish {
            return Err(
                "MeerkatMachine supervisor handoff cannot publish trust for revoke operation"
                    .to_string(),
            );
        }
        validate_expected_peer(
            "MeerkatMachineSupervisorPublish",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(
            CommsTrustMutationAuthority::from_meerkat_machine_supervisor_publish(
                self.peer_id.clone(),
                self.epoch,
            ),
        )
    }

    pub fn revoke_authority_for(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MeerkatMachineSupervisorTrustOperation::Revoke {
            return Err(
                "MeerkatMachine supervisor handoff cannot revoke trust for publish operation"
                    .to_string(),
            );
        }
        validate_expected_peer(
            "MeerkatMachineSupervisorRevoke",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(
            CommsTrustMutationAuthority::from_meerkat_machine_supervisor_revoke(
                self.peer_id.clone(),
                self.epoch,
            ),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobMachineMemberTrustHandoff {
    edge_a: String,
    edge_b: String,
    a_peer_id: String,
    b_peer_id: String,
    epoch: u64,
    operation: MobMachineMemberTrustOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobMachineMemberTrustOperation {
    Wiring,
    Unwiring,
    Repair,
}

impl MobMachineMemberTrustHandoff {
    pub fn from_generated_member_wiring(
        source: &impl GeneratedMobMachineMemberTrustHandoff,
    ) -> Self {
        Self {
            edge_a: source.edge_a().to_string(),
            edge_b: source.edge_b().to_string(),
            a_peer_id: source.a_peer_id().to_string(),
            b_peer_id: source.b_peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineMemberTrustOperation::Wiring,
        }
    }

    pub fn from_generated_member_unwiring(
        source: &impl GeneratedMobMachineMemberTrustHandoff,
    ) -> Self {
        Self {
            edge_a: source.edge_a().to_string(),
            edge_b: source.edge_b().to_string(),
            a_peer_id: source.a_peer_id().to_string(),
            b_peer_id: source.b_peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineMemberTrustOperation::Unwiring,
        }
    }

    pub fn from_generated_member_repair(
        source: &impl GeneratedMobMachineMemberTrustHandoff,
    ) -> Self {
        Self {
            edge_a: source.edge_a().to_string(),
            edge_b: source.edge_b().to_string(),
            a_peer_id: source.a_peer_id().to_string(),
            b_peer_id: source.b_peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineMemberTrustOperation::Repair,
        }
    }

    pub fn peer_id_for_identity(&self, identity: &str) -> Option<&str> {
        if self.edge_a == identity {
            Some(self.a_peer_id.as_str())
        } else if self.edge_b == identity {
            Some(self.b_peer_id.as_str())
        } else {
            None
        }
    }

    fn required_peer_id_for_identity(
        &self,
        identity: &str,
        expected_peer_id: &str,
    ) -> Result<&str, String> {
        let Some(actual) = self.peer_id_for_identity(identity) else {
            return Err(format!(
                "MobMachine member trust handoff does not cover identity {identity:?}"
            ));
        };
        validate_expected_peer("MobMachineMemberTrust", actual, expected_peer_id)?;
        Ok(actual)
    }

    pub fn wiring_authority_for_identity(
        &self,
        identity: &str,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineMemberTrustOperation::Wiring {
            return Err(
                "MobMachine member trust handoff cannot wire trust for non-wiring operation"
                    .to_string(),
            );
        }
        let peer_id = self.required_peer_id_for_identity(identity, expected_peer_id)?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_wiring(
            peer_id.to_owned(),
            self.epoch,
        ))
    }

    pub fn unwiring_authority_for_identity(
        &self,
        identity: &str,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineMemberTrustOperation::Unwiring {
            return Err(
                "MobMachine member trust handoff cannot unwire trust for non-unwiring operation"
                    .to_string(),
            );
        }
        let peer_id = self.required_peer_id_for_identity(identity, expected_peer_id)?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_unwiring(
            peer_id.to_owned(),
            self.epoch,
        ))
    }

    pub fn repair_authority_for_identity(
        &self,
        identity: &str,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineMemberTrustOperation::Repair {
            return Err(
                "MobMachine member trust handoff cannot repair trust for non-repair operation"
                    .to_string(),
            );
        }
        let peer_id = self.required_peer_id_for_identity(identity, expected_peer_id)?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_repair(
            peer_id.to_owned(),
            self.epoch,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobMachineExternalPeerTrustHandoff {
    peer_id: String,
    epoch: u64,
    operation: MobMachineExternalPeerTrustOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobMachineExternalPeerTrustOperation {
    Wiring,
    Unwiring,
    Repair,
}

impl MobMachineExternalPeerTrustHandoff {
    pub fn peer_id(&self) -> &str {
        self.peer_id.as_str()
    }

    pub fn from_generated_external_peer_wiring(
        source: &impl GeneratedMobMachineExternalPeerTrustHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineExternalPeerTrustOperation::Wiring,
        }
    }

    pub fn from_generated_external_peer_unwiring(
        source: &impl GeneratedMobMachineExternalPeerTrustHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineExternalPeerTrustOperation::Unwiring,
        }
    }

    pub fn from_generated_external_peer_repair(
        source: &impl GeneratedMobMachineExternalPeerTrustHandoff,
    ) -> Self {
        Self {
            peer_id: source.peer_id().to_string(),
            epoch: source.epoch(),
            operation: MobMachineExternalPeerTrustOperation::Repair,
        }
    }

    pub fn authority_for_wiring(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineExternalPeerTrustOperation::Wiring {
            return Err(
                "MobMachine external peer trust handoff cannot wire trust for non-wiring operation"
                    .to_string(),
            );
        }
        validate_expected_peer(
            "MobMachineExternalPeerWiring",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_wiring(
            self.peer_id.clone(),
            self.epoch,
        ))
    }

    pub fn authority_for_unwiring(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineExternalPeerTrustOperation::Unwiring {
            return Err(
                "MobMachine external peer trust handoff cannot unwire trust for non-unwiring operation"
                    .to_string(),
            );
        }
        validate_expected_peer(
            "MobMachineExternalPeerUnwiring",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_unwiring(
            self.peer_id.clone(),
            self.epoch,
        ))
    }

    pub fn authority_for_repair(
        &self,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, String> {
        if self.operation != MobMachineExternalPeerTrustOperation::Repair {
            return Err(
                "MobMachine external peer trust handoff cannot repair trust for non-repair operation"
                    .to_string(),
            );
        }
        validate_expected_peer(
            "MobMachineExternalPeerRepair",
            &self.peer_id,
            expected_peer_id,
        )?;
        Ok(CommsTrustMutationAuthority::from_mob_machine_peer_repair(
            self.peer_id.clone(),
            self.epoch,
        ))
    }
}
