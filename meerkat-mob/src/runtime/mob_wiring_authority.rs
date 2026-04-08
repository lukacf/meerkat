use crate::MeerkatId;
use crate::error::MobError;
use crate::roster::MemberState;
use meerkat_core::comms::TrustedPeerSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalWirePlan {
    ReconcileExisting,
    EstablishNew,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalWirePlan {
    NoOp,
    EstablishOrUpdate {
        previous_spec: Option<TrustedPeerSpec>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalUnwirePlan {
    NoOp,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalUnwirePlan {
    NoOp,
    Remove { spec: TrustedPeerSpec },
}

pub(super) struct ExternalWireInput<'a> {
    pub(super) local: &'a MeerkatId,
    pub(super) local_state: MemberState,
    pub(super) external_name: &'a MeerkatId,
    pub(super) local_comms_name: &'a str,
    pub(super) already_wired: bool,
    pub(super) collides_with_local_member: bool,
    pub(super) stored_spec: Option<&'a TrustedPeerSpec>,
    pub(super) new_spec: &'a TrustedPeerSpec,
}

pub(super) struct MobWiringAuthority;

impl MobWiringAuthority {
    pub(super) fn plan_local_wire(
        a: &MeerkatId,
        a_state: MemberState,
        a_has_b_edge: bool,
        b: &MeerkatId,
        b_state: MemberState,
        b_has_a_edge: bool,
    ) -> Result<LocalWirePlan, MobError> {
        if a == b {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{a}')"
            )));
        }
        if a_state != MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{a}', found state {a_state:?}",
            )));
        }
        if b_state != MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{b}', found state {b_state:?}",
            )));
        }
        if a_has_b_edge && b_has_a_edge {
            Ok(LocalWirePlan::ReconcileExisting)
        } else {
            Ok(LocalWirePlan::EstablishNew)
        }
    }

    pub(super) fn plan_external_wire(
        input: ExternalWireInput<'_>,
    ) -> Result<ExternalWirePlan, MobError> {
        if input.local == input.external_name || input.new_spec.name == input.local_comms_name {
            return Err(MobError::WiringError(format!(
                "wire requires distinct peers (got '{}')",
                input.local_comms_name
            )));
        }
        if input.collides_with_local_member {
            return Err(MobError::WiringError(format!(
                "external peer '{}' collides with a local roster member; use PeerTarget::Local instead",
                input.new_spec.name
            )));
        }
        if input.local_state != MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{}', found state {:?}",
                input.local, input.local_state
            )));
        }
        if input.already_wired && input.stored_spec == Some(input.new_spec) {
            return Ok(ExternalWirePlan::NoOp);
        }
        Ok(ExternalWirePlan::EstablishOrUpdate {
            previous_spec: input.stored_spec.cloned(),
        })
    }

    pub(super) fn plan_local_unwire(
        a: &MeerkatId,
        b: &MeerkatId,
        a_has_b_edge: bool,
        b_has_a_edge: bool,
    ) -> Result<LocalUnwirePlan, MobError> {
        if a == b {
            return Err(MobError::WiringError(format!(
                "unwire requires distinct peers (got '{a}')"
            )));
        }
        if !a_has_b_edge && !b_has_a_edge {
            Ok(LocalUnwirePlan::NoOp)
        } else {
            Ok(LocalUnwirePlan::Remove)
        }
    }

    pub(super) fn plan_external_unwire(
        local: &MeerkatId,
        peer_name: &MeerkatId,
        already_wired: bool,
        stored_spec: Option<&TrustedPeerSpec>,
        collides_with_local_member: bool,
        spec_hint: Option<&TrustedPeerSpec>,
    ) -> Result<ExternalUnwirePlan, MobError> {
        if local == peer_name {
            return Err(MobError::WiringError(format!(
                "unwire requires distinct peers (got '{local}')"
            )));
        }
        if collides_with_local_member {
            return Err(MobError::WiringError(format!(
                "peer '{peer_name}' is a local roster member; use local unwire semantics instead",
            )));
        }
        if !already_wired && stored_spec.is_none() && spec_hint.is_none() {
            return Ok(ExternalUnwirePlan::NoOp);
        }
        let spec = match (spec_hint, stored_spec) {
            (Some(hint), Some(stored)) if hint != stored => {
                return Err(MobError::WiringError(format!(
                    "external peer spec mismatch for '{local}' -> '{peer_name}'",
                )));
            }
            (Some(hint), _) => hint.clone(),
            (None, Some(stored)) => stored.clone(),
            (None, None) => {
                return Err(MobError::WiringError(format!(
                    "external unwire requires stored peer spec for '{local}' -> '{peer_name}'",
                )));
            }
        };
        Ok(ExternalUnwirePlan::Remove { spec })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec(name: &str) -> TrustedPeerSpec {
        TrustedPeerSpec {
            name: name.into(),
            peer_id: "peer-1".into(),
            address: "tcp://127.0.0.1:1".into(),
        }
    }

    #[test]
    fn existing_symmetric_local_edge_reconciles() {
        let plan = MobWiringAuthority::plan_local_wire(
            &MeerkatId::from("a"),
            MemberState::Active,
            true,
            &MeerkatId::from("b"),
            MemberState::Active,
            true,
        )
        .unwrap();
        assert_eq!(plan, LocalWirePlan::ReconcileExisting);
    }

    #[test]
    fn identical_external_spec_is_noop() {
        let existing = spec("peer-x");
        let plan = MobWiringAuthority::plan_external_wire(ExternalWireInput {
            local: &MeerkatId::from("local"),
            local_state: MemberState::Active,
            external_name: &MeerkatId::from("peer-x"),
            local_comms_name: "mob/profile/local",
            already_wired: true,
            collides_with_local_member: false,
            stored_spec: Some(&existing),
            new_spec: &existing,
        })
        .unwrap();
        assert_eq!(plan, ExternalWirePlan::NoOp);
    }

    #[test]
    fn local_unwire_without_edges_is_noop() {
        let plan = MobWiringAuthority::plan_local_unwire(
            &MeerkatId::from("a"),
            &MeerkatId::from("b"),
            false,
            false,
        )
        .unwrap();
        assert_eq!(plan, LocalUnwirePlan::NoOp);
    }
}
