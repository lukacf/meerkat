//! One canonical local grant owner. Inputs are trusted host observations, not
//! authentication receipts. No decoded state, audit row or caller claim creates
//! a production authority. The feature host owns local publication and custody.

/// Canonical data declarations shared by both host instantiations. These types
/// carry no authority. Decoding supplies checked candidate data, not issuance,
/// authentication or recovery of an owner. The principal has no mutable API.
pub mod types {
    use meerkat_authorization_contracts::constraints::{ActionRef, ExecutionRestrictions};
    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_authorization_contracts::grant::GrantAuthorityIncarnation;
    use meerkat_authorization_contracts::work_association::{GrantTarget, OriginalWorkRef};
    use meerkat_core::approval::decision::{ApproverId, DecisionId, Sha256Digest};
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
    ///
    /// `child_approval` and `approval` are absent from the serialized form
    /// when `None`, so a record without them has exactly the bytes of a
    /// record that predates them.
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
        /// Every child of this grant must carry a person's decision for the
        /// rule's approver id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub child_approval: Option<ChildApprovalRule>,
        /// The terms of an approval-carried child: the decision it was
        /// issued under and the one use it may serve.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub approval: Option<GrantApproval>,
    }

    impl std::fmt::Debug for GrantRecord {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GrantRecord([protected])")
        }
    }

    /// A grant's rule for its children: each must carry a decision a person
    /// made for `approver_id`, attested on the decision's own event, or
    /// device-trusted only when `accepts_device_trust`. A service credential
    /// relaying a decision never meets it.
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ChildApprovalRule {
        pub approver_id: ApproverId,
        #[serde(default)]
        pub accepts_device_trust: GrantDeviceTrust,
    }

    impl std::fmt::Debug for ChildApprovalRule {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("ChildApprovalRule([protected])")
        }
    }

    /// Whether a [`ChildApprovalRule`] accepts a device-trusted person.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum GrantDeviceTrust {
        /// Only a person authenticated on the decision's event.
        #[default]
        Refused,
        /// A device-trusted person too.
        Accepted,
    }

    /// The tier of the decision an approval-carried child was issued under.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum GrantDecisionTier {
        /// An authenticated human acting for the decision's approver id.
        Human,
        /// The decision's named reviewer, through its verdict.
        Review,
    }

    /// How the person who decided was authenticated on the decision's event.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum GrantDecisionAssurance {
        /// The person was authenticated on the actual event.
        PersonAuthenticated,
        /// A delegated service credential authenticated the event.
        ServiceCredential,
        /// Only device or network trust was established.
        DeviceTrusted,
        /// No person attested it (a reviewer's verdict).
        Unattested,
    }

    /// The terms of an approval-carried child: the execution permit of the
    /// decision it is issued under (its id, approver id, tier and the
    /// decider's assurance, and the digest of the approved request), the
    /// one requester and the one work that may use it, the exact operation
    /// when bound, and how many uses it serves. The decision's evidence
    /// stays with the decision; the record names it by id.
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct GrantApproval {
        pub decision_id: DecisionId,
        pub approver_id: ApproverId,
        pub tier: GrantDecisionTier,
        pub assurance: GrantDecisionAssurance,
        pub request_digest: Sha256Digest,
        pub requester: GrantPrincipal,
        pub work: OriginalWorkRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub operation: Option<GrantOperationBinding>,
        pub uses: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub review_before_return: Option<ApproverId>,
    }

    impl std::fmt::Debug for GrantApproval {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GrantApproval([protected])")
        }
    }

    /// The exact operation an approval-carried child is bound to: its typed
    /// target, its action and the digest of its exact request. A use whose
    /// operation is not exactly this one is refused.
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct GrantOperationBinding {
        pub target: GrantTarget,
        pub action: ActionRef,
        pub request_digest: Sha256Digest,
    }

    impl std::fmt::Debug for GrantOperationBinding {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GrantOperationBinding([protected])")
        }
    }

    /// Exact projection used by the generated grant expressions: a copy of
    /// the value of an option the same expression proved present. A missing
    /// value is an invalid projection, never a request for a default.
    #[doc(hidden)]
    pub trait GrantValueProjection {
        type Value;
        fn get(&self, key: &str) -> Self::Value;
    }

    impl<T: Clone> GrantValueProjection for Option<T> {
        type Value = T;
        #[allow(
            clippy::panic,
            reason = "an impossible projection must not invent a grant value"
        )]
        fn get(&self, key: &str) -> T {
            match (key, self.as_ref()) {
                ("value", Some(value)) => value.clone(),
                ("value", None) => panic!("grant projection requires a present value"),
                _ => panic!("grant projection requires the value selector"),
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
        use meerkat_authorization_contracts::work_association::OriginalWorkRef;
        use meerkat_core::approval::decision::{DecisionId, Sha256Digest};
        pub use $crate::catalog::dsl::grant_authority::types::{
            ChildApprovalRule, GrantApproval, GrantDecisionAssurance, GrantDecisionTier,
            GrantDeviceTrust, GrantOperationBinding, GrantPrincipal, GrantRecord,
        };
        use $crate::catalog::dsl::grant_authority::types::GrantValueProjection as _;

        meerkat_machine_dsl::machine! {
            machine GrantAuthorityMachine {
                version: 3,
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
                    // Uses spent of each approval-carried child, from 0 at its
                    // issuance: a key exactly for each approval record.
                    #[redacted] spent: Map<EvidenceId, u64>,
                    // Every use ever spent, for the revision's accounting.
                    consumed: u64,
                    // The decision of each approval-carried child: one grant
                    // per decision.
                    #[redacted] carried_decisions: Set<DecisionId>,
                }
                init(Unconfigured) {
                    root = None,
                    namespace = None,
                    generation = 0,
                    incarnation = None,
                    revision = 0,
                    records = EmptyMap,
                    revoked = EmptySet,
                    spent = EmptyMap,
                    consumed = 0,
                    carried_decisions = EmptySet,
                }
                terminal []
                phase GrantAuthorityPhase { Unconfigured, Active }
                input GrantAuthorityInput {
                    Configure { #[redacted] root: GrantPrincipal, #[redacted] namespace: EvidenceId, generation: u64, #[redacted] incarnation: GrantAuthorityIncarnation },
                    IssueRoot { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord },
                    IssueChild { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord, #[redacted] derived: DerivedChildRestrictions, #[redacted] chain: Seq<GrantRecord>, now_ms: u64, #[redacted] terms_digest: Sha256Digest },
                    Revoke { #[redacted] actor: GrantPrincipal, #[redacted] record: GrantRecord },
                    ResolveUse { #[redacted] namespace: EvidenceId, generation: u64, #[redacted] incarnation: GrantAuthorityIncarnation, #[redacted] executor: GrantPrincipal, #[redacted] represented_subject: Option<GrantPrincipal>, #[redacted] requester: Option<GrantPrincipal>, #[redacted] work: Option<OriginalWorkRef>, #[redacted] operation: Option<GrantOperationBinding>, #[redacted] leaf: GrantRecord, #[redacted] chain: Seq<GrantRecord>, now_ms: u64 },
                    ConsumeUse { #[redacted] namespace: EvidenceId, generation: u64, #[redacted] incarnation: GrantAuthorityIncarnation, #[redacted] executor: GrantPrincipal, #[redacted] represented_subject: Option<GrantPrincipal>, #[redacted] requester: Option<GrantPrincipal>, #[redacted] work: Option<OriginalWorkRef>, #[redacted] operation: Option<GrantOperationBinding>, #[redacted] leaf: GrantRecord, #[redacted] chain: Seq<GrantRecord>, now_ms: u64, approver_current: bool },
                }
                effect GrantAuthorityEffect {
                    Configured,
                    Issued { #[redacted] record: GrantRecord },
                    Revoked { #[redacted] grant_id: EvidenceId },
                    UseResolved { #[redacted] leaf: GrantRecord },
                    UseConsumed { #[redacted] leaf: GrantRecord },
                }
                disposition Configured => local seam OwnerRealizationOnly,
                disposition Issued => local seam OwnerRealizationOnly,
                disposition Revoked => local seam OwnerRealizationOnly,
                disposition UseResolved => local seam SurfaceResultAlignment,
                disposition UseConsumed => local seam OwnerRealizationOnly,
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
                // A child of a grant under a rule carries a decision a person
                // made for the rule's approver id, authenticated on its event,
                // or device-trusted only where the rule accepts it; and only a
                // grant under a rule has approval-carried children.
                helper approval_rule_admits(parent: GrantRecord, record: GrantRecord) -> bool {
                    (record.approval == None || parent.child_approval != None)
                    && (parent.child_approval == None
                        || (record.approval != None
                            && record.approval.get("value").approver_id == parent.child_approval.get("value").approver_id
                            && record.approval.get("value").tier == GrantDecisionTier::Human
                            && (record.approval.get("value").assurance == GrantDecisionAssurance::PersonAuthenticated
                                || (record.approval.get("value").assurance == GrantDecisionAssurance::DeviceTrusted
                                    && parent.child_approval.get("value").accepts_device_trust == GrantDeviceTrust::Accepted))))
                }
                // An approval-carried child cannot be delegated further, ends
                // in a finite window and serves at least one use.
                helper approval_bounded(record: GrantRecord) -> bool {
                    record.approval == None
                    || (record.restrictions.delegation_depth.unresolved.len() == 0
                        && record.restrictions.delegation_depth.bound.is_data_variant(DepthBound::Remaining)
                        && record.restrictions.delegation_depth.remaining_edges == 0
                        && record.restrictions.lifetime.unresolved.len() == 0
                        && record.restrictions.lifetime.bound.is_data_variant(LifetimeBound::Window)
                        && record.approval.get("value").uses >= 1)
                }
                // A use of an approval-carried leaf: its one requester, its
                // one work, and its exact operation when one is bound. A use
                // that names no requester or no work, or no operation where
                // one is bound, never matches.
                helper approval_use_bound(leaf: GrantRecord, requester: Option<GrantPrincipal>, work: Option<OriginalWorkRef>, operation: Option<GrantOperationBinding>) -> bool {
                    leaf.approval == None
                    || (Some(leaf.approval.get("value").requester) == requester
                        && Some(leaf.approval.get("value").work) == work
                        && (leaf.approval.get("value").operation == None
                            || leaf.approval.get("value").operation == operation))
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
                        && self.records.keys().len() == 0 && self.revoked.len() == 0
                        && self.spent.keys().len() == 0 && self.consumed == 0
                        && self.carried_decisions.len() == 0)
                }
                invariant revision_accounts_for_retained_mutations {
                    self.revision >= self.records.keys().len()
                    && self.revision - self.records.keys().len() == self.revoked.len() + self.consumed
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
                // Spent uses are kept exactly for approval-carried records,
                // never past the uses each serves.
                invariant spent_only_for_approval_children {
                    for_all(id in self.spent.keys(),
                        self.records.contains_key(id)
                        && self.records.get_cloned(id).get("value").approval != None
                        && self.spent.get_cloned(id).get("value") <= self.records.get_cloned(id).get("value").approval.get("value").uses)
                    && for_all(id in self.records.keys(),
                        self.records.get_cloned(id).get("value").approval == None
                        || self.spent.contains_key(id))
                }
                invariant approval_children_are_bounded {
                    for_all(id in self.records.keys(),
                        approval_bounded(self.records.get_cloned(id).get("value")))
                }
                invariant approval_children_have_a_parent_rule {
                    for_all(id in self.records.keys(),
                        self.records.get_cloned(id).get("value").approval == None
                        || (self.records.get_cloned(id).get("value").parent != None
                            && self.records.contains_key(self.records.get_cloned(id).get("value").parent.get("value"))
                            && self.records.get_cloned(self.records.get_cloned(id).get("value").parent.get("value")).get("value").child_approval != None))
                }
                invariant carried_decisions_match_records {
                    for_all(decision in self.carried_decisions,
                        exists(id in self.records.keys(),
                            self.records.get_cloned(id).get("value").approval != None
                            && self.records.get_cloned(id).get("value").approval.get("value").decision_id == decision))
                    && for_all(id in self.records.keys(),
                        self.records.get_cloned(id).get("value").approval == None
                        || self.carried_decisions.contains(self.records.get_cloned(id).get("value").approval.get("value").decision_id))
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
                    // A root has no parent, so no rule to carry a decision for.
                    guard { record.approval == None }
                    guard { self.records.contains_key(record.id) == false && self.revision < u64::MAX }
                    guard { record.issued_revision == self.revision + 1 }
                    update { self.records.insert(record.id, record); self.revision += 1; }
                    to Active
                    emit Issued { record: record }
                }
                transition IssueChild {
                    on input IssueChild { actor, record, derived, chain, now_ms, terms_digest }
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
                            && child_rank(parent.restrictions, record.restrictions)
                            && approval_rule_admits(parent, record))
                    }
                    guard { approval_bounded(record) }
                    // The issued terms are exactly the approved request's.
                    guard { record.approval == None || terms_digest == record.approval.get("value").request_digest }
                    guard { record.approval == None || self.carried_decisions.contains(record.approval.get("value").decision_id) == false }
                    update {
                        self.records.insert(record.id, record);
                        self.revision += 1;
                        if record.approval != None {
                            self.spent.insert(record.id, 0);
                            self.carried_decisions.insert(record.approval.get("value").decision_id);
                        }
                    }
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
                    on input ResolveUse { namespace, generation, incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms }
                    guard { self.lifecycle_phase == Phase::Active && Some(namespace) == self.namespace && generation == self.generation && Some(incarnation) == self.incarnation }
                    guard { leaf.grantee == executor && leaf.represented_subject == represented_subject && chain.contains(leaf) }
                    guard { chain_links(chain, self.root, now_ms) }
                    guard { for_all(link in chain, self.records.get_cloned(link.id) == Some(link) && self.revoked.contains(link.id) == false) }
                    guard { approval_use_bound(leaf, requester, work, operation) }
                    guard { leaf.approval == None || self.spent.get_cloned(leaf.id).get("value") < leaf.approval.get("value").uses }
                    update {}
                    to Active
                    emit UseResolved { leaf: leaf }
                }
                // Spend one use of an approval-carried leaf, before its
                // operation enters: the same checks as a use, and the
                // approver of its decision still current. The revision
                // advances, so a decision prepared before the spend is stale.
                transition ConsumeUse {
                    on input ConsumeUse { namespace, generation, incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, approver_current }
                    guard { self.lifecycle_phase == Phase::Active && Some(namespace) == self.namespace && generation == self.generation && Some(incarnation) == self.incarnation }
                    guard { self.revision < u64::MAX }
                    guard { leaf.grantee == executor && leaf.represented_subject == represented_subject && chain.contains(leaf) }
                    guard { chain_links(chain, self.root, now_ms) }
                    guard { for_all(link in chain, self.records.get_cloned(link.id) == Some(link) && self.revoked.contains(link.id) == false) }
                    guard { leaf.approval != None && approval_use_bound(leaf, requester, work, operation) }
                    guard { self.spent.get_cloned(leaf.id).get("value") < leaf.approval.get("value").uses }
                    guard { approver_current }
                    update {
                        self.spent.insert(leaf.id, self.spent.get_cloned(leaf.id).get("value") + 1);
                        self.consumed += 1;
                        self.revision += 1;
                    }
                    to Active
                    emit UseConsumed { leaf: leaf }
                }
                // The approver of the leaf's decision is no longer current:
                // nothing is spent, and the use is refused.
                transition ConsumeUseRefusedApproverRevoked {
                    on input ConsumeUse { namespace, generation, incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, approver_current }
                    guard { self.lifecycle_phase == Phase::Active && Some(namespace) == self.namespace && generation == self.generation && Some(incarnation) == self.incarnation }
                    guard { leaf.grantee == executor && leaf.represented_subject == represented_subject && chain.contains(leaf) }
                    guard { chain_links(chain, self.root, now_ms) }
                    guard { for_all(link in chain, self.records.get_cloned(link.id) == Some(link) && self.revoked.contains(link.id) == false) }
                    guard { leaf.approval != None && approval_use_bound(leaf, requester, work, operation) }
                    guard { self.spent.get_cloned(leaf.id).get("value") < leaf.approval.get("value").uses }
                    guard { approver_current == false }
                    update {}
                    to Active
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
    let types = "meerkat_machine_schema::catalog::dsl::grant_authority::types";
    let decision = "meerkat_core::approval::decision";
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
        N::type_path("GrantPrincipal", format!("{types}::GrantPrincipal")),
        N::type_path_struct(
            "GrantRecord",
            format!("{types}::GrantRecord"),
            vec![
                F::named("id", "EvidenceId"),
                F::named("authority_incarnation", "GrantAuthorityIncarnation"),
                F::optional_named("parent", "EvidenceId"),
                F::named("issuer", "GrantPrincipal"),
                F::named("grantee", "GrantPrincipal"),
                F::optional_named("represented_subject", "GrantPrincipal"),
                F::named("issued_revision", "GrantNumber"),
                F::named("restrictions", "ExecutionRestrictions"),
                F::optional_named("child_approval", "ChildApprovalRule"),
                F::optional_named("approval", "GrantApproval"),
            ],
        ),
        N::type_path_struct(
            "ChildApprovalRule",
            format!("{types}::ChildApprovalRule"),
            vec![
                F::named("approver_id", "ApproverId"),
                F::named("accepts_device_trust", "GrantDeviceTrust"),
            ],
        ),
        N::type_path_struct(
            "GrantApproval",
            format!("{types}::GrantApproval"),
            vec![
                F::named("decision_id", "DecisionId"),
                F::named("approver_id", "ApproverId"),
                F::named("tier", "GrantDecisionTier"),
                F::named("assurance", "GrantDecisionAssurance"),
                F::named("request_digest", "Sha256Digest"),
                F::named("requester", "GrantPrincipal"),
                F::named("work", "OriginalWorkRef"),
                F::optional_named("operation", "GrantOperationBinding"),
                F::named("uses", "GrantNumber"),
                F::optional_named("review_before_return", "ApproverId"),
            ],
        ),
        // The bound operation and the work are compared whole, never read
        // into: opaque in the model.
        N::type_path(
            "GrantOperationBinding",
            format!("{types}::GrantOperationBinding"),
        ),
        N::type_path(
            "OriginalWorkRef",
            "meerkat_authorization_contracts::work_association::OriginalWorkRef",
        ),
        N::type_path("ApproverId", format!("{decision}::ApproverId")),
        N::type_path("DecisionId", format!("{decision}::DecisionId")),
        N::type_path("Sha256Digest", format!("{decision}::Sha256Digest")),
        N::type_path_enum(
            "GrantDecisionTier",
            format!("{types}::GrantDecisionTier"),
            &["Human", "Review"],
        ),
        N::type_path_enum(
            "GrantDecisionAssurance",
            format!("{types}::GrantDecisionAssurance"),
            &[
                "PersonAuthenticated",
                "ServiceCredential",
                "DeviceTrusted",
                "Unattested",
            ],
        ),
        N::type_path_enum(
            "GrantDeviceTrust",
            format!("{types}::GrantDeviceTrust"),
            &["Refused", "Accepted"],
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
        ("GrantProcessorBounds", "ProcessorRef"),
        ("GrantAudienceBounds", "AudienceRef"),
    ] {
        bindings.push(N::type_path(
            name,
            format!("{restrictions}::ExactRestriction<{restrictions}::{value}>"),
        ));
    }
    // Resource domains carry their own bound kinds (exact domains or the
    // owning authorities); the machine compares the value opaquely.
    bindings.push(N::type_path(
        "GrantResourceBounds",
        format!("{restrictions}::DomainRestriction"),
    ));
    super::machine_schema_metadata(bindings, vec![]).with_tlc_model(tlc_fixtures::model())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod present_record_projection_tests {
    use super::types::GrantValueProjection;
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
            child_approval: None,
            approval: None,
        }
    }

    #[test]
    fn present_record_projection_is_the_exact_record() {
        let value = Some(record());
        assert!(value.get("value") == record());
    }

    #[test]
    #[should_panic(expected = "grant projection requires a present value")]
    fn absent_record_projection_does_not_invent_a_default() {
        let value: Option<GrantRecord> = None;
        let _ = value.get("value");
    }

    #[test]
    #[should_panic(expected = "grant projection requires the value selector")]
    fn record_projection_rejects_an_unknown_selector() {
        let value = Some(record());
        let _ = value.get("other");
    }

    #[test]
    fn a_plain_record_serializes_without_the_approval_fields() {
        let encoded = serde_json::to_value(record()).expect("record encodes");
        let fields = encoded.as_object().expect("a record is an object");
        assert!(!fields.contains_key("child_approval") && !fields.contains_key("approval"));
        let decoded: GrantRecord = serde_json::from_value(encoded).expect("record decodes");
        assert!(decoded == record());
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
