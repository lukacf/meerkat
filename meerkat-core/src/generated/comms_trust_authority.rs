// @generated -- Generated comms trust authority handoff helpers.

use crate::comms::CommsTrustMutationAuthority;

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
    pub fn from_generated_projection(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
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
    pub fn from_generated_supervisor_publish(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
            operation: MeerkatMachineSupervisorTrustOperation::Publish,
        }
    }

    pub fn from_generated_supervisor_revoke(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
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
        edge_a: impl Into<String>,
        edge_b: impl Into<String>,
        a_peer_id: impl Into<String>,
        b_peer_id: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self {
            edge_a: edge_a.into(),
            edge_b: edge_b.into(),
            a_peer_id: a_peer_id.into(),
            b_peer_id: b_peer_id.into(),
            epoch,
            operation: MobMachineMemberTrustOperation::Wiring,
        }
    }

    pub fn from_generated_member_unwiring(
        edge_a: impl Into<String>,
        edge_b: impl Into<String>,
        a_peer_id: impl Into<String>,
        b_peer_id: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self {
            edge_a: edge_a.into(),
            edge_b: edge_b.into(),
            a_peer_id: a_peer_id.into(),
            b_peer_id: b_peer_id.into(),
            epoch,
            operation: MobMachineMemberTrustOperation::Unwiring,
        }
    }

    pub fn from_generated_member_repair(
        edge_a: impl Into<String>,
        edge_b: impl Into<String>,
        a_peer_id: impl Into<String>,
        b_peer_id: impl Into<String>,
        epoch: u64,
    ) -> Self {
        Self {
            edge_a: edge_a.into(),
            edge_b: edge_b.into(),
            a_peer_id: a_peer_id.into(),
            b_peer_id: b_peer_id.into(),
            epoch,
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
    pub fn from_generated_external_peer_wiring(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
            operation: MobMachineExternalPeerTrustOperation::Wiring,
        }
    }

    pub fn from_generated_external_peer_unwiring(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
            operation: MobMachineExternalPeerTrustOperation::Unwiring,
        }
    }

    pub fn from_generated_external_peer_repair(peer_id: impl Into<String>, epoch: u64) -> Self {
        Self {
            peer_id: peer_id.into(),
            epoch,
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
