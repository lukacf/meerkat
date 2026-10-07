//! One canonical local grant owner. Inputs are trusted host observations, not
//! authentication receipts. No decoded state, audit row or caller claim creates
//! a production authority. The feature host owns local publication and custody.

/// Canonical data declarations shared by both host instantiations. These types
/// carry no authority. Decoding supplies checked candidate data, not issuance,
/// authentication or recovery of an owner. The principal has no mutable API.
pub mod types {
    use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_authorization_contracts::grant::GrantAuthorityIncarnation;
    use meerkat_core::auth::{PrincipalContractError, PrincipalRef};
    use serde::{Deserialize, Serialize};

    /// Syntax-validated immutable use of the existing principal domain.
    /// Construction proves qualification only, never authentication.
    #[derive(Clone, PartialEq, Eq, Serialize)]
    #[serde(transparent)]
    pub struct GrantPrincipal(PrincipalRef);

    impl GrantPrincipal {
        pub fn new(principal: PrincipalRef) -> Result<Self, PrincipalContractError> {
            principal.validate_qualified()?;
            Ok(Self(principal))
        }
        pub fn principal(&self) -> &PrincipalRef {
            &self.0
        }
    }

    impl<'de> Deserialize<'de> for GrantPrincipal {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Self::new(PrincipalRef::deserialize(deserializer)?).map_err(serde::de::Error::custom)
        }
    }

    impl std::fmt::Debug for GrantPrincipal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GrantPrincipal([protected])")
        }
    }

    /// Immutable after issuance. The generated owner binds every field;
    /// constructing this candidate does not issue it.
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct GrantRecord {
        pub id: EvidenceId,
        pub authority_incarnation: GrantAuthorityIncarnation,
        pub parent: Option<EvidenceId>,
        pub issuer: GrantPrincipal,
        pub grantee: GrantPrincipal,
        pub represented_subject: Option<GrantPrincipal>,
        pub issued_revision: u64,
        pub restrictions: ExecutionRestrictions,
    }

    impl std::fmt::Debug for GrantRecord {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GrantRecord([protected])")
        }
    }

    /// Exact projection used by the generated grant invariant expressions.
    /// Their keys come from the same immutable map being checked. A missing
    /// value is an invalid projection, never a request for a default record.
    #[doc(hidden)]
    pub trait GrantRecordValueProjection {
        fn get(&self, key: &str) -> &GrantRecord;
    }

    impl GrantRecordValueProjection for Option<GrantRecord> {
        #[allow(
            clippy::panic,
            reason = "an impossible invariant projection must not invent a grant record"
        )]
        fn get(&self, key: &str) -> &GrantRecord {
            match (key, self.as_ref()) {
                ("value", Some(record)) => record,
                ("value", None) => panic!("grant invariant projection requires a present record"),
                _ => panic!("grant invariant projection requires the value selector"),
            }
        }
    }
}

#[macro_export]
macro_rules! grant_authority_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        use meerkat_authorization_contracts::constraints::{DepthBound, ExecutionRestrictions, LifetimeBound};
        use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
        use meerkat_authorization_contracts::evidence::EvidenceId;
        use meerkat_authorization_contracts::grant::GrantAuthorityIncarnation;
        pub use $crate::catalog::dsl::grant_authority::types::{GrantPrincipal, GrantRecord};
        use $crate::catalog::dsl::grant_authority::types::GrantRecordValueProjection as _;

        meerkat_machine_dsl::machine! {
            machine GrantAuthorityMachine {
                version: 2,
                rust: $rust_crate / $rust_module,

                state {
                    lifecycle_phase: GrantAuthorityPhase,
                    #[redacted] root: Option<GrantPrincipal>,
                    #[redacted] namespace: Option<EvidenceId>,
                    generation: u64,
                    #[redacted] incarnation: Option<GrantAuthorityIncarnation>,
                    revision: u64,
                    #[redacted] records: Map<EvidenceId, GrantRecord>,
                    #[redacted] revoked: Set<EvidenceId>,
                }
                init(Unconfigured) {
                    root = None,
                    namespace = None,
                    generation = 0,
                    incarnation = None,
                    revision = 0,
                    records = EmptyMap,
                    revoked = EmptySet,
                }
                terminal []
                phase GrantAuthorityPhase { Unconfigured, Active }
                input GrantAuthorityInput {
                    Configure { #[redacted] root: GrantPrincipal, #[redacted] namespace: EvidenceId, generation: u64, #[redacted] incarnation: GrantAuthorityIncarnation },
                    IssueRoot { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord },
                    IssueChild { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord, #[redacted] derived: DerivedChildRestrictions, #[redacted] chain: Seq<GrantRecord>, now_ms: u64 },
                    Revoke { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord },
                    ResolveUse { #[redacted] namespace: EvidenceId, generation: u64, #[redacted] incarnation: GrantAuthorityIncarnation, #[redacted] executor: GrantPrincipal, #[redacted] represented_subject: Option<GrantPrincipal>, #[redacted] leaf: GrantRecord, #[redacted] chain: Seq<GrantRecord>, now_ms: u64 },
                }
                effect GrantAuthorityEffect {
                    Configured,
                    Issued { #[redacted] record: GrantRecord },
                    Revoked { #[redacted] grant_id: EvidenceId },
                    UseResolved { #[redacted] leaf: GrantRecord },
                }
                disposition Configured => local seam OwnerRealizationOnly,
                disposition Issued => local seam OwnerRealizationOnly,
                disposition Revoked => local seam OwnerRealizationOnly,
                disposition UseResolved => local seam SurfaceResultAlignment,

                helper lifetime_current(restrictions: ExecutionRestrictions, now_ms: u64) -> bool {
                    restrictions.lifetime.unresolved.len() == 0
                    && restrictions.delegation_depth.unresolved.len() == 0
                    && (restrictions.lifetime.bound.is_unit_variant(LifetimeBound::Unrestricted)
                        || (restrictions.lifetime.bound.is_data_variant(LifetimeBound::Window)
                            && restrictions.lifetime.not_before_ms <= now_ms
                            && now_ms < restrictions.lifetime.expires_at_ms))
                }
                helper child_rank(parent: ExecutionRestrictions, child: ExecutionRestrictions) -> bool {
                    parent.delegation_depth.bound.is_unit_variant(DepthBound::Unrestricted)
                    || (parent.delegation_depth.bound.is_data_variant(DepthBound::Remaining)
                        && child.delegation_depth.bound.is_data_variant(DepthBound::Remaining)
                        && child.delegation_depth.remaining_edges < parent.delegation_depth.remaining_edges)
                }
                // Every supplied link is compared with its actual retained row.
                // Parent closure plus strictly increasing issued revision makes
                // omission, cycles and substitution unable to authorize use.
                helper chain_links(chain: Seq<GrantRecord>, root: Option<GrantPrincipal>, now_ms: u64) -> bool {
                    chain.len() > 0 && chain.len() <= 64
                    && for_all(link in chain,
                        lifetime_current(link.restrictions, now_ms)
                        && (if link.parent == None {
                            Some(link.issuer) == root
                        } else {
                            exists(parent in chain,
                                link.parent == Some(parent.id)
                                && link.issuer == parent.grantee
                                && link.represented_subject == parent.represented_subject
                                && parent.issued_revision < link.issued_revision
                                && child_rank(parent.restrictions, link.restrictions))
                        }))
                }
                invariant configured_identity_is_present {
                    self.lifecycle_phase == Phase::Unconfigured
                    || (self.root != None && self.namespace != None && self.generation > 0 && self.incarnation != None)
                }
                // In-place mutation can unwind between field updates. Cold
                // recovery must reject an incomplete configuration or a row
                // insertion/revocation without its corresponding revision.
                invariant unconfigured_state_is_empty {
                    self.lifecycle_phase != Phase::Unconfigured
                    || (self.root == None && self.namespace == None && self.generation == 0
                        && self.incarnation == None && self.revision == 0
                        && self.records.keys().len() == 0 && self.revoked.len() == 0)
                }
                invariant revision_accounts_for_retained_mutations {
                    self.revision >= self.records.keys().len()
                    && self.revision - self.records.keys().len() == self.revoked.len()
                }
                invariant issued_records_have_exact_identity_and_revision {
                    for_all(id in self.records.keys(),
                        self.records.get_cloned(id).get("value").id == id
                        && self.records.get_cloned(id).get("value").issued_revision > 0
                        && self.records.get_cloned(id).get("value").issued_revision <= self.revision)
                }
                invariant issued_records_belong_to_this_incarnation {
                    for_all(id in self.records.keys(),
                        Some(self.records.get_cloned(id).get("value").authority_incarnation) == self.incarnation)
                }
                invariant revoked_records_remain_present {
                    for_all(id in self.revoked, self.records.contains_key(id))
                }
                transition Configure {
                    on input Configure { root, namespace, generation, incarnation }
                    guard { self.lifecycle_phase == Phase::Unconfigured && generation > 0 }
                    update { self.root = Some(root); self.namespace = Some(namespace); self.generation = generation; self.incarnation = Some(incarnation); }
                    to Active
                    emit Configured
                }
                transition IssueRoot {
                    on input IssueRoot { actor, record }
                    guard { self.lifecycle_phase == Phase::Active && Some(actor) == self.root }
                    guard { Some(record.authority_incarnation) == self.incarnation }
                    guard { record.issuer == actor && record.parent == None }
                    guard { self.records.contains_key(record.id) == false && self.revision < u64::MAX }
                    guard { record.issued_revision == self.revision + 1 }
                    update { self.records.insert(record.id, record); self.revision += 1; }
                    to Active
                    emit Issued { record: record }
                }
                transition IssueChild {
                    on input IssueChild { actor, record, derived, chain, now_ms }
                    guard { self.lifecycle_phase == Phase::Active && self.revision < u64::MAX }
                    guard { Some(record.authority_incarnation) == self.incarnation }
                    guard { self.records.contains_key(record.id) == false && record.issued_revision == self.revision + 1 }
                    guard { 64 > chain.len() && chain_links(chain, self.root, now_ms) }
                    guard { for_all(link in chain, self.records.get_cloned(link.id) == Some(link) && self.revoked.contains(link.id) == false) }
                    guard {
                        record.issuer == actor && record.restrictions == derived.effective
                        && exists(parent in chain,
                            record.parent == Some(parent.id) && parent.grantee == actor
                            && record.represented_subject == parent.represented_subject
                            && derived.parent == parent.restrictions
                            && child_rank(parent.restrictions, record.restrictions))
                    }
                    update { self.records.insert(record.id, record); self.revision += 1; }
                    to Active
                    emit Issued { record: record }
                }
                transition RevokeNew {
                    on input Revoke { actor, record }
                    guard { self.lifecycle_phase == Phase::Active && self.revision < u64::MAX }
                    guard { self.records.get_cloned(record.id) == Some(record) }
                    guard { Some(actor) == self.root || actor == record.issuer }
                    guard { self.revoked.contains(record.id) == false }
                    update { self.revoked.insert(record.id); self.revision += 1; }
                    to Active
                    emit Revoked { grant_id: record.id }
                }
                transition RevokeAlready {
                    on input Revoke { actor, record }
                    guard { self.lifecycle_phase == Phase::Active && self.records.get_cloned(record.id) == Some(record) }
                    guard { Some(actor) == self.root || actor == record.issuer }
                    guard { self.revoked.contains(record.id) }
                    update {}
                    to Active
                    emit Revoked { grant_id: record.id }
                }
                transition ResolveUse {
                    on input ResolveUse { namespace, generation, incarnation, executor, represented_subject, leaf, chain, now_ms }
                    guard { self.lifecycle_phase == Phase::Active && Some(namespace) == self.namespace && generation == self.generation && Some(incarnation) == self.incarnation }
                    guard { leaf.grantee == executor && leaf.represented_subject == represented_subject && chain.contains(leaf) }
                    guard { chain_links(chain, self.root, now_ms) }
                    guard { for_all(link in chain, self.records.get_cloned(link.id) == Some(link) && self.revoked.contains(link.id) == false) }
                    update {}
                    to Active
                    emit UseResolved { leaf: leaf }
                }
            }
        }
    };
}

crate::grant_authority_catalog_machine_dsl!("self", "catalog::dsl::grant_authority");

mod tlc_fixtures;

/// Structural descriptions are an overapproximation, not a claim that arbitrary
/// model records can construct the private checked Rust projections. Native
/// tests enumerate raw lifetime/depth bounds and verify the derived scalar
/// relation, exact wire preservation and attenuation constructor. A future TLC
/// receipt must state the finite relation used; arbitrary Cartesian samples do
/// not prove the restriction algebra or authentication.
pub fn schema_metadata() -> super::MachineSchemaMetadata {
    use crate::{
        NamedTypeBinding as N, TypePathEnumPayloadField as E, TypePathEnumStructuralVariant as V,
        TypePathStructField as F,
    };
    let restrictions = "meerkat_authorization_contracts::constraints";
    let mut bindings = vec![
        N::u64("GrantNumber"),
        N::type_path(
            "GrantAuthorityIncarnation",
            "meerkat_authorization_contracts::grant::GrantAuthorityIncarnation",
        ),
        N::type_path(
            "EvidenceId",
            "meerkat_authorization_contracts::evidence::EvidenceId",
        ),
        N::type_path(
            "GrantPrincipal",
            "meerkat_machine_schema::catalog::dsl::grant_authority::types::GrantPrincipal",
        ),
        N::type_path_struct(
            "GrantRecord",
            "meerkat_machine_schema::catalog::dsl::grant_authority::types::GrantRecord",
            vec![
                F::named("id", "EvidenceId"),
                F::named("authority_incarnation", "GrantAuthorityIncarnation"),
                F::optional_named("parent", "EvidenceId"),
                F::named("issuer", "GrantPrincipal"),
                F::named("grantee", "GrantPrincipal"),
                F::optional_named("represented_subject", "GrantPrincipal"),
                F::named("issued_revision", "GrantNumber"),
                F::named("restrictions", "ExecutionRestrictions"),
            ],
        ),
        N::type_path_struct(
            "ExecutionRestrictions",
            format!("{restrictions}::ExecutionRestrictions"),
            vec![
                F::named("actions", "GrantActionBounds"),
                F::named("resource_domains", "GrantResourceBounds"),
                F::named("processors", "GrantProcessorBounds"),
                F::named("audiences", "GrantAudienceBounds"),
                F::named("lifetime", "GrantLifetime"),
                F::named("delegation_depth", "GrantDepth"),
            ],
        ),
        N::type_path_struct(
            "DerivedChildRestrictions",
            "meerkat_authorization_contracts::derived_child::DerivedChildRestrictions",
            vec![
                F::named("parent", "ExecutionRestrictions"),
                F::named("requested", "ExecutionRestrictions"),
                F::named("effective", "ExecutionRestrictions"),
            ],
        ),
        N::type_path_struct(
            "GrantLifetime",
            format!("{restrictions}::LifetimeRestriction"),
            vec![
                F::named("bound", "LifetimeBound"),
                F::named("unresolved", "GrantUnresolved"),
                F::named("not_before_ms", "GrantNumber"),
                F::named("expires_at_ms", "GrantNumber"),
            ],
        ),
        N::type_path_struct(
            "GrantDepth",
            format!("{restrictions}::DelegationDepth"),
            vec![
                F::named("bound", "DepthBound"),
                F::named("unresolved", "GrantUnresolved"),
                F::named("remaining_edges", "GrantNumber"),
            ],
        ),
        N::type_path_field_presence_set(
            "GrantUnresolved",
            format!("std::collections::BTreeSet<{restrictions}::UnresolvedConstraint>"),
            &["Absent", "Unknown", "Unavailable"],
        ),
        N::type_path_enum_with_structural_variants(
            "LifetimeBound",
            format!("{restrictions}::LifetimeBound"),
            &["Unrestricted", "Empty"],
            vec![V::with_fields(
                "Window",
                vec![
                    E::named("not_before_ms", "GrantNumber"),
                    E::named("expires_at_ms", "GrantNumber"),
                ],
            )],
        ),
        N::type_path_enum_with_structural_variants(
            "DepthBound",
            format!("{restrictions}::DepthBound"),
            &["Unrestricted"],
            vec![V::with_fields(
                "Remaining",
                vec![E::named("edges", "GrantNumber")],
            )],
        ),
    ];
    for (name, value) in [
        ("GrantActionBounds", "ActionRef"),
        ("GrantResourceBounds", "ResourceDomain"),
        ("GrantProcessorBounds", "ProcessorRef"),
        ("GrantAudienceBounds", "AudienceRef"),
    ] {
        bindings.push(N::type_path(
            name,
            format!("{restrictions}::ExactRestriction<{restrictions}::{value}>"),
        ));
    }
    super::machine_schema_metadata(bindings, vec![]).with_tlc_model(tlc_fixtures::model())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod present_record_projection_tests {
    use super::types::GrantRecordValueProjection;
    use super::*;
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    fn record() -> GrantRecord {
        let root = GrantPrincipal::new(
            PrincipalRef::in_domain(
                PrincipalKind::ServiceAccount,
                "root",
                TrustDomainId::new("grant-projection-test").expect("domain"),
            )
            .expect("qualified principal"),
        )
        .expect("grant principal");
        GrantRecord {
            id: EvidenceId::new("present").expect("id"),
            authority_incarnation: GrantAuthorityIncarnation::from_uuid(
                uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001")
                    .expect("version four UUID"),
            )
            .expect("incarnation"),
            parent: None,
            issuer: root.clone(),
            grantee: root,
            represented_subject: None,
            issued_revision: 1,
            restrictions: ExecutionRestrictions::unrestricted(),
        }
    }

    #[test]
    fn present_record_projection_borrows_the_exact_record() {
        let value = Some(record());
        assert!(std::ptr::eq(
            value.get("value"),
            value.as_ref().expect("present record")
        ));
    }

    #[test]
    #[should_panic(expected = "grant invariant projection requires a present record")]
    fn absent_record_projection_does_not_invent_a_default() {
        let value: Option<GrantRecord> = None;
        let _ = value.get("value");
    }

    #[test]
    #[should_panic(expected = "grant invariant projection requires the value selector")]
    fn record_projection_rejects_an_unknown_selector() {
        let value = Some(record());
        let _ = value.get("other");
    }

    #[test]
    fn generated_recovery_checks_exact_present_record_fields() {
        let record = record();
        let mut owner = GrantAuthorityMachineAuthority::new();
        owner
            .apply(GrantAuthorityInput::Configure {
                root: record.issuer.clone(),
                namespace: EvidenceId::new("namespace").expect("namespace"),
                generation: 1,
                incarnation: record.authority_incarnation,
            })
            .expect("configure actual generated owner");
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: record.issuer.clone(),
                record: record.clone(),
            })
            .expect("issue exact record");
        let state = owner.state().clone();
        let recovered = GrantAuthorityMachineAuthority::recover_from_state(state.clone())
            .expect("unchanged issued record is valid");
        assert_eq!(recovered.state(), &state);

        let mut wrong_key = state.clone();
        let issued = wrong_key.records.remove(&record.id).expect("issued row");
        wrong_key
            .records
            .insert(EvidenceId::new("other-key").expect("different key"), issued);
        assert!(GrantAuthorityMachineAuthority::recover_from_state(wrong_key).is_err());

        for invalid_revision in [0, state.revision + 1] {
            let mut wrong_revision = state.clone();
            wrong_revision
                .records
                .get_mut(&record.id)
                .expect("issued row")
                .issued_revision = invalid_revision;
            assert!(GrantAuthorityMachineAuthority::recover_from_state(wrong_revision).is_err());
        }

        let mut wrong_incarnation = state;
        wrong_incarnation
            .records
            .get_mut(&record.id)
            .expect("issued row")
            .authority_incarnation = GrantAuthorityIncarnation::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002")
                .expect("different version four UUID"),
        )
        .expect("different incarnation");
        assert!(GrantAuthorityMachineAuthority::recover_from_state(wrong_incarnation).is_err());
    }
}
