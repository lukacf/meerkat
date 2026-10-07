//! Compatibility negotiation for the independently versioned security contract.
//!
//! Advertisements are claims, not evidence of enforcement or authenticated
//! identity. A trusted composition must bind them to the actual participating
//! owners and transport before use. A compatible contract is not an operation
//! permit and must never authorize entry, hydration or disclosure by itself.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Security wire version, independent of the Rust package release number.
///
/// Compatibility is exact. A newer minor version is not implicitly compatible:
/// even an additive security requirement may change the meaning of permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractVersion {
    pub major: u16,
    pub minor: u16,
}

impl ContractVersion {
    pub const V1: Self = Self { major: 1, minor: 0 };
}

/// The profile selected by the composition and retained with admitted work.
///
/// This profile requires mechanical permission checks at shared operation
/// boundaries across execution modes, including streaming, live/voice,
/// compaction, memory, comms and hosted tools. These declarations do not
/// advertise implemented support, semantic information confinement, a sandbox
/// or external durability. Once content enters model context, this profile does
/// not mechanically prevent the model from conveying its meaning elsewhere.
/// Unknown and parked profile values fail decoding; neither may fall back to
/// trusted embedded work. The old buffered profile is not an alias for local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementProfile {
    TrustedEmbedded,
    LocalGovernedV1,
}

/// Common security semantics, not application feature availability.
///
/// Application action/resource catalogs remain feature-owned. These identifiers
/// specify how those owners compose, and do not replace the capability registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecuritySemantic {
    QualifiedIdentity,
    RetainedWorkAssociation,
    AttenuatingDelegation,
    ConjunctiveDecisions,
    ExactOperationBinding,
    /// Coherent local owner publication and invalidation, not remote fencing.
    LocalGenerationOrdering,
    CurrentAudienceRelease,
    /// The actual source/resource owner decides access at its operation boundary.
    /// Source access does not establish restrictions on later model derivations.
    SourceAuthorityContracts,
    OperationLocalRefusal,
    /// Audit joins existing native commits or declares process-lifetime custody.
    NativeAudit,
    /// All model/tool/output paths, compaction, memory, comms and live/voice
    /// enforce the same contract. A slim smoke fixture is not profile support.
    SharedBoundaryEnforcement,
}

/// Required owner duties that cannot be dropped during a handoff.
///
/// An advertisement says only that the composition can realize a duty. Its
/// fulfillment for an exact operation still requires the owning authority and
/// evidence. Adding a duty requires a new conformance vector.
/// Each participant enforces its own protected operation or verifies exact
/// evidence from its canonical owner under a declared handoff. A composed
/// source-owner receipt need not be duplicated; a commissioned service does not
/// grant its caller direct read access to private service inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    CurrentResourceUse,
    CurrentRecipientRelease,
    RecordNativeAudit,
    RefuseAffectedOperation,
}

const LOCAL_GOVERNED_SEMANTICS: [SecuritySemantic; 11] = [
    SecuritySemantic::QualifiedIdentity,
    SecuritySemantic::RetainedWorkAssociation,
    SecuritySemantic::AttenuatingDelegation,
    SecuritySemantic::ConjunctiveDecisions,
    SecuritySemantic::ExactOperationBinding,
    SecuritySemantic::LocalGenerationOrdering,
    SecuritySemantic::CurrentAudienceRelease,
    SecuritySemantic::SourceAuthorityContracts,
    SecuritySemantic::OperationLocalRefusal,
    SecuritySemantic::NativeAudit,
    SecuritySemantic::SharedBoundaryEnforcement,
];

const LOCAL_GOVERNED_OBLIGATIONS: [ObligationKind; 4] = [
    ObligationKind::CurrentResourceUse,
    ObligationKind::CurrentRecipientRelease,
    ObligationKind::RecordNativeAudit,
    ObligationKind::RefuseAffectedOperation,
];

/// Untrusted wire description of the exact contract required for admitted work.
///
/// There are deliberately no serde defaults. Legacy absence is not consent to
/// downgrade. The governed baseline is added during negotiation even when a
/// sender supplies empty additional requirement sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractRequirements {
    pub version: ContractVersion,
    pub profile: EnforcementProfile,
    pub semantics: BTreeSet<SecuritySemantic>,
    pub obligations: BTreeSet<ObligationKind>,
}

impl ContractRequirements {
    /// Required semantics of the complete host-trusted local profile.
    /// Constructing requirements does not advertise implemented support.
    #[must_use]
    pub fn local_governed_v1() -> Self {
        Self {
            version: ContractVersion::V1,
            profile: EnforcementProfile::LocalGovernedV1,
            semantics: LOCAL_GOVERNED_SEMANTICS.into_iter().collect(),
            obligations: LOCAL_GOVERNED_OBLIGATIONS.into_iter().collect(),
        }
    }

    fn with_profile_baseline(&self) -> Self {
        let mut requirements = self.clone();
        match requirements.profile {
            EnforcementProfile::TrustedEmbedded => {}
            EnforcementProfile::LocalGovernedV1 => {
                requirements.semantics.extend(LOCAL_GOVERNED_SEMANTICS);
                requirements.obligations.extend(LOCAL_GOVERNED_OBLIGATIONS);
            }
        }
        requirements
    }
}

/// Claimed support supplied by a participating composition.
///
/// This has no all-supported default. Hosts derive their advertisement from
/// wired and verified owners, not from the presence of these Rust enum variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractAdvertisement {
    pub contracts: Vec<ContractSupport>,
}

/// Support is scoped to one exact version/profile pair. It cannot be borrowed
/// from another profile or another version advertised by the same participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractSupport {
    pub version: ContractVersion,
    pub profile: EnforcementProfile,
    pub semantics: BTreeSet<SecuritySemantic>,
    pub obligations: BTreeSet<ObligationKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Participant {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NegotiationRefusal {
    #[error("security contract version {0:?} has no implemented semantics")]
    UnknownVersion(ContractVersion),
    #[error("{participant:?} does not support security contract {version:?}")]
    UnsupportedVersion {
        participant: Participant,
        version: ContractVersion,
    },
    #[error("{participant:?} does not support required profile {profile:?}")]
    UnsupportedProfile {
        participant: Participant,
        profile: EnforcementProfile,
    },
    #[error("{participant:?} advertised duplicate support for the required version/profile")]
    AmbiguousSupport { participant: Participant },
    #[error("{participant:?} does not support required semantic {semantic:?}")]
    UnsupportedSemantic {
        participant: Participant,
        semantic: SecuritySemantic,
    },
    #[error("{participant:?} cannot realize required obligation {obligation:?}")]
    UnsupportedObligation {
        participant: Participant,
        obligation: ObligationKind,
    },
}

/// A checked compatibility result, never an authorization permit.
///
/// It cannot be deserialized. Admitted work retains `requirements()` and must
/// negotiate against current owners on restore/handoff. Historical advertised
/// support is not proof that the current process enforces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibleContract {
    requirements: ContractRequirements,
}

impl CompatibleContract {
    pub fn requirements(&self) -> &ContractRequirements {
        &self.requirements
    }
}

/// Check both sides against the exact retained requirements, without fallback.
///
/// This pure function neither authenticates the peer nor inspects its owners.
/// The caller must obtain those claims from its declared trusted composition
/// and authenticated peer protocol. Changing a work item's profile requires
/// separately authorized administration at its owner, not renegotiation here.
pub fn negotiate(
    required: &ContractRequirements,
    local: &ContractAdvertisement,
    remote: &ContractAdvertisement,
) -> Result<CompatibleContract, NegotiationRefusal> {
    if required.version != ContractVersion::V1 {
        return Err(NegotiationRefusal::UnknownVersion(required.version));
    }
    let required = required.with_profile_baseline();
    for (participant, advertised) in [(Participant::Local, local), (Participant::Remote, remote)] {
        if !advertised
            .contracts
            .iter()
            .any(|offer| offer.version == required.version)
        {
            return Err(NegotiationRefusal::UnsupportedVersion {
                participant,
                version: required.version,
            });
        }
        let mut offers = advertised
            .contracts
            .iter()
            .filter(|offer| offer.version == required.version && offer.profile == required.profile);
        let support = offers
            .next()
            .ok_or(NegotiationRefusal::UnsupportedProfile {
                participant,
                profile: required.profile,
            })?;
        if offers.next().is_some() {
            return Err(NegotiationRefusal::AmbiguousSupport { participant });
        }
        for semantic in &required.semantics {
            if !support.semantics.contains(semantic) {
                return Err(NegotiationRefusal::UnsupportedSemantic {
                    participant,
                    semantic: *semantic,
                });
            }
        }
        for obligation in &required.obligations {
            if !support.obligations.contains(obligation) {
                return Err(NegotiationRefusal::UnsupportedObligation {
                    participant,
                    obligation: *obligation,
                });
            }
        }
    }
    Ok(CompatibleContract {
        requirements: required,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn fixture_advertisement() -> ContractAdvertisement {
        ContractAdvertisement {
            contracts: vec![ContractSupport {
                version: ContractVersion::V1,
                profile: EnforcementProfile::LocalGovernedV1,
                semantics: LOCAL_GOVERNED_SEMANTICS.into_iter().collect(),
                obligations: LOCAL_GOVERNED_OBLIGATIONS.into_iter().collect(),
            }],
        }
    }

    #[test]
    fn downgrade_cannot_be_negotiated_for_retained_governed_work() {
        let required = ContractRequirements::local_governed_v1();
        let local = fixture_advertisement();
        let mut remote = local.clone();
        remote.contracts[0].profile = EnforcementProfile::TrustedEmbedded;
        assert_eq!(
            negotiate(&required, &local, &remote),
            Err(NegotiationRefusal::UnsupportedProfile {
                participant: Participant::Remote,
                profile: EnforcementProfile::LocalGovernedV1,
            })
        );
    }

    #[test]
    fn empty_wire_requirements_cannot_remove_governed_baseline() {
        let required = ContractRequirements {
            version: ContractVersion::V1,
            profile: EnforcementProfile::LocalGovernedV1,
            semantics: BTreeSet::new(),
            obligations: BTreeSet::new(),
        };
        let local = fixture_advertisement();
        let mut remote = local.clone();
        remote.contracts[0]
            .semantics
            .remove(&SecuritySemantic::NativeAudit);
        assert_eq!(
            negotiate(&required, &local, &remote),
            Err(NegotiationRefusal::UnsupportedSemantic {
                participant: Participant::Remote,
                semantic: SecuritySemantic::NativeAudit,
            })
        );
        let checked = negotiate(&required, &local, &local).expect("fixture supports baseline");
        assert_eq!(
            checked.requirements(),
            &ContractRequirements::local_governed_v1()
        );
    }

    #[test]
    fn each_participant_must_implement_its_own_duty_or_validate_owner_evidence() {
        let required = ContractRequirements::local_governed_v1();
        let complete = fixture_advertisement();
        let mut incapable = complete.clone();
        incapable.contracts[0]
            .obligations
            .remove(&ObligationKind::RecordNativeAudit);
        for (participant, local, remote) in [
            (Participant::Local, &incapable, &complete),
            (Participant::Remote, &complete, &incapable),
        ] {
            assert_eq!(
                negotiate(&required, local, remote),
                Err(NegotiationRefusal::UnsupportedObligation {
                    participant,
                    obligation: ObligationKind::RecordNativeAudit,
                })
            );
        }
    }

    #[test]
    fn claimed_support_for_a_future_minor_version_does_not_implement_it() {
        let version = ContractVersion { major: 1, minor: 1 };
        let mut required = ContractRequirements::local_governed_v1();
        required.version = version;
        let mut claimed = fixture_advertisement();
        claimed.contracts[0].version = version;
        assert_eq!(
            negotiate(&required, &claimed, &claimed),
            Err(NegotiationRefusal::UnknownVersion(version))
        );
    }

    #[test]
    fn support_from_different_profiles_is_not_combined() {
        let required = ContractRequirements::local_governed_v1();
        let local = fixture_advertisement();
        let mut remote = local.clone();
        let mut other_profile = remote.contracts[0].clone();
        other_profile.profile = EnforcementProfile::TrustedEmbedded;
        remote.contracts[0]
            .obligations
            .remove(&ObligationKind::RecordNativeAudit);
        remote.contracts.push(other_profile);
        assert_eq!(
            negotiate(&required, &local, &remote),
            Err(NegotiationRefusal::UnsupportedObligation {
                participant: Participant::Remote,
                obligation: ObligationKind::RecordNativeAudit,
            })
        );
    }

    #[test]
    fn duplicate_offers_cannot_create_implicit_rule_order() {
        let required = ContractRequirements::local_governed_v1();
        let local = fixture_advertisement();
        let mut remote = local.clone();
        remote.contracts.push(remote.contracts[0].clone());
        assert_eq!(
            negotiate(&required, &local, &remote),
            Err(NegotiationRefusal::AmbiguousSupport {
                participant: Participant::Remote,
            })
        );
    }

    #[test]
    fn unknown_or_omitted_security_fields_are_not_silently_ignored() {
        let required = ContractRequirements::local_governed_v1();
        let wire = serde_json::to_value(&required).expect("fixture serializes");
        for field in ["version", "profile", "semantics", "obligations"] {
            let mut missing = wire.clone();
            missing.as_object_mut().expect("object").remove(field);
            assert!(serde_json::from_value::<ContractRequirements>(missing).is_err());
        }
        let mut unknown_field = wire.clone();
        unknown_field["allow_fallback"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ContractRequirements>(unknown_field).is_err());
        for (field, value) in [
            ("profile", serde_json::json!("governed_live")),
            ("semantics", serde_json::json!(["future_semantic"])),
            ("obligations", serde_json::json!(["future_obligation"])),
        ] {
            let mut unknown = wire.clone();
            unknown[field] = value;
            assert!(serde_json::from_value::<ContractRequirements>(unknown).is_err());
        }
    }

    #[test]
    fn buffered_and_future_contracts_are_not_local_profile_aliases() {
        let required = ContractRequirements::local_governed_v1();
        let wire = serde_json::to_value(&required).expect("requirements");
        assert_eq!(wire["profile"], "local_governed_v1");
        for profile in ["governed_buffered_v1", "local_governed_v2", "governed_live"] {
            let mut changed = wire.clone();
            changed["profile"] = serde_json::json!(profile);
            assert!(serde_json::from_value::<ContractRequirements>(changed).is_err());
        }
        for (field, value) in [
            ("semantics", "conservative_dependencies"),
            ("obligations", "preserve_dependencies"),
            ("obligations", "confine_secrets"),
            ("semantics", "rollback_resistance"),
            ("semantics", "participating_authority_fence"),
            ("obligations", "durable_entry_receipt"),
            ("obligations", "durable_settlement_receipt"),
        ] {
            let mut changed = wire.clone();
            changed[field] = serde_json::json!([value]);
            assert!(serde_json::from_value::<ContractRequirements>(changed).is_err());
        }
    }

    #[test]
    fn local_support_requires_all_execution_paths_and_operation_local_refusal() {
        let required = ContractRequirements::local_governed_v1();
        let complete = fixture_advertisement();
        for semantic in [
            SecuritySemantic::SharedBoundaryEnforcement,
            SecuritySemantic::OperationLocalRefusal,
            SecuritySemantic::LocalGenerationOrdering,
            SecuritySemantic::SourceAuthorityContracts,
        ] {
            let mut partial = complete.clone();
            partial.contracts[0].semantics.remove(&semantic);
            assert_eq!(
                negotiate(&required, &complete, &partial),
                Err(NegotiationRefusal::UnsupportedSemantic {
                    participant: Participant::Remote,
                    semantic,
                })
            );
        }
        let mut turns_off_turn = complete.clone();
        turns_off_turn.contracts[0]
            .obligations
            .remove(&ObligationKind::RefuseAffectedOperation);
        assert_eq!(
            negotiate(&required, &complete, &turns_off_turn),
            Err(NegotiationRefusal::UnsupportedObligation {
                participant: Participant::Remote,
                obligation: ObligationKind::RefuseAffectedOperation,
            })
        );
    }

    #[test]
    fn support_cannot_be_borrowed_from_a_different_version() {
        let required = ContractRequirements::local_governed_v1();
        let complete = fixture_advertisement();
        let mut split = complete.clone();
        let mut future = split.contracts[0].clone();
        future.version = ContractVersion { major: 1, minor: 1 };
        split.contracts[0]
            .obligations
            .remove(&ObligationKind::CurrentResourceUse);
        split.contracts.push(future);
        assert_eq!(
            negotiate(&required, &complete, &split),
            Err(NegotiationRefusal::UnsupportedObligation {
                participant: Participant::Remote,
                obligation: ObligationKind::CurrentResourceUse,
            })
        );
    }

    #[test]
    fn nested_advertisement_fields_cannot_strip_a_security_requirement() {
        let wire = serde_json::to_value(fixture_advertisement()).expect("advertisement");
        for field in ["version", "profile", "semantics", "obligations"] {
            let mut changed = wire.clone();
            changed["contracts"][0]
                .as_object_mut()
                .expect("support object")
                .remove(field);
            assert!(serde_json::from_value::<ContractAdvertisement>(changed).is_err());
        }
        let mut changed = wire;
        changed["contracts"][0]["live_is_unchecked"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ContractAdvertisement>(changed).is_err());
    }
}
