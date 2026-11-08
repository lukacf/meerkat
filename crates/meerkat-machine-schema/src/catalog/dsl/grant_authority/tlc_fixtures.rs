//! Small, linked finite fixtures for this owner, not a second grant evaluator.
//! Non-lifetime/depth restrictions remain opaque and equal in this model.
//! Rust tests exercise the same fixture rows through the generated owner.
//!
//! Beside the plain chains, one root carries a child rule for the approver
//! id `approver_owner`, and one approval-carried child under it carries a
//! person-authenticated human decision for that id: one use, for the
//! requester `principal_a` and the work `work_1`, its terms digest
//! `digest_1`. The bound operation stays opaque, and the child binds none.

use crate::identity::{FieldId, NamedTypeId};
use crate::{MachineTlcModel, MachineTlcProfile, MachineTlcStateLimits, TlcValue as V};
use meerkat_authorization_contracts::constraints::{
    DelegationDepth, DepthBound, ExecutionRestrictions, LifetimeBound, LifetimeRestriction,
    UnresolvedConstraint,
};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use std::collections::BTreeMap;

/// What a fixture row carries beside a plain grant.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Carries {
    /// A plain grant.
    Nothing,
    /// A root whose children must carry a decision for `approver_owner`.
    ChildRule,
    /// A child carrying the decision `decision_1` for `approver_owner`.
    Approval,
}

#[derive(Clone)]
struct Row {
    issuer: &'static str,
    grantee: &'static str,
    child: bool,
    foreign: bool,
    carries: Carries,
    restrictions: ExecutionRestrictions,
}

#[allow(clippy::expect_used)]
fn attenuation() -> DerivedChildRestrictions {
    let mut parent = ExecutionRestrictions::unrestricted();
    parent.lifetime = LifetimeRestriction::window(0, 3);
    parent.delegation_depth = DelegationDepth::remaining(1);
    let mut requested = ExecutionRestrictions::unrestricted();
    requested.lifetime = LifetimeRestriction::window(1, 2);
    requested.delegation_depth = DelegationDepth::remaining(0);
    DerivedChildRestrictions::new(parent, requested).expect("finite fixture attenuation")
}

fn rows(deep: bool) -> Vec<Row> {
    let derived = attenuation();
    let mut rows = Vec::new();
    // Configure may choose either principal independently. Each choice has a
    // complete chain in this same domain, with one shared owner incarnation.
    for (issuer, grantee) in [
        ("principal_a", "principal_b"),
        ("principal_b", "principal_a"),
    ] {
        rows.push(Row {
            issuer,
            grantee,
            child: false,
            foreign: false,
            carries: Carries::Nothing,
            restrictions: derived.parent().clone(),
        });
        rows.push(Row {
            issuer: grantee,
            grantee: issuer,
            child: true,
            foreign: false,
            carries: Carries::Nothing,
            restrictions: derived.effective().clone(),
        });
    }
    // The approval chain, under the root `principal_a` configures.
    rows.push(Row {
        issuer: "principal_a",
        grantee: "principal_b",
        child: false,
        foreign: false,
        carries: Carries::ChildRule,
        restrictions: derived.parent().clone(),
    });
    rows.push(Row {
        issuer: "principal_b",
        grantee: "principal_a",
        child: true,
        foreign: false,
        carries: Carries::Approval,
        restrictions: derived.effective().clone(),
    });
    if deep {
        let mut unresolved = rows[0].clone();
        unresolved.restrictions.lifetime =
            LifetimeRestriction::unresolved(UnresolvedConstraint::Unknown);
        rows.push(unresolved);
        let mut foreign = rows[0].clone();
        foreign.foreign = true;
        rows.push(foreign);
    }
    rows
}

#[allow(clippy::expect_used)]
fn record(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Record(
        fields
            .into_iter()
            .map(|(name, value)| (FieldId::parse(name).expect("fixture field"), value))
            .collect(),
    )
}
fn text(value: &str) -> V {
    V::String(value.into())
}
fn bound(tag: &str, fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    record(std::iter::once(("tag", text(tag))).chain(fields))
}
fn unresolved(values: &std::collections::BTreeSet<UnresolvedConstraint>) -> V {
    V::Set(
        values
            .iter()
            .map(|value| {
                text(match value {
                    UnresolvedConstraint::Absent => "Absent",
                    UnresolvedConstraint::Unknown => "Unknown",
                    UnresolvedConstraint::Unavailable => "Unavailable",
                })
            })
            .collect(),
    )
}
fn restrictions(value: &ExecutionRestrictions) -> V {
    let lifetime = match value.lifetime.bound() {
        LifetimeBound::Unrestricted => bound("Unrestricted", []),
        LifetimeBound::Empty => bound("Empty", []),
        LifetimeBound::Window {
            not_before_ms,
            expires_at_ms,
        } => bound(
            "Window",
            [
                ("not_before_ms", V::U64(not_before_ms)),
                ("expires_at_ms", V::U64(expires_at_ms)),
            ],
        ),
    };
    let depth = match value.delegation_depth.bound() {
        DepthBound::Unrestricted => bound("Unrestricted", []),
        DepthBound::Remaining(edges) => bound("Remaining", [("edges", V::U64(u64::from(edges)))]),
    };
    record([
        ("actions", text("unrestricted_actions")),
        ("resource_domains", text("unrestricted_resources")),
        ("processors", text("unrestricted_processors")),
        ("audiences", text("unrestricted_audiences")),
        (
            "lifetime",
            record([
                ("bound", lifetime),
                ("unresolved", unresolved(value.lifetime.unresolved_facts())),
                ("not_before_ms", V::U64(value.lifetime.not_before_ms)),
                ("expires_at_ms", V::U64(value.lifetime.expires_at_ms)),
            ]),
        ),
        (
            "delegation_depth",
            record([
                ("bound", depth),
                (
                    "unresolved",
                    unresolved(value.delegation_depth.unresolved_facts()),
                ),
                (
                    "remaining_edges",
                    V::U64(value.delegation_depth.remaining_edges),
                ),
            ]),
        ),
    ])
}
fn child_rule_value() -> V {
    record([
        ("approver_id", text("approver_owner")),
        ("accepts_device_trust", text("Refused")),
    ])
}
fn approval_value() -> V {
    record([
        ("decision_id", text("decision_1")),
        ("approver_id", text("approver_owner")),
        ("tier", text("Human")),
        ("assurance", text("PersonAuthenticated")),
        ("request_digest", text("digest_1")),
        ("requester", text("principal_a")),
        ("work", text("work_1")),
        ("operation", V::None),
        ("uses", V::U64(1)),
        ("review_before_return", V::None),
    ])
}
fn row_value(row: &Row) -> V {
    record([
        (
            "id",
            text(if row.child {
                "grant_child"
            } else {
                "grant_root"
            }),
        ),
        (
            "authority_incarnation",
            text(if row.foreign {
                "foreign_incarnation"
            } else {
                "owner_incarnation"
            }),
        ),
        (
            "parent",
            if row.child {
                V::Some(Box::new(text("grant_root")))
            } else {
                V::None
            },
        ),
        ("issuer", text(row.issuer)),
        ("grantee", text(row.grantee)),
        ("represented_subject", V::None),
        ("issued_revision", V::U64(if row.child { 2 } else { 1 })),
        ("restrictions", restrictions(&row.restrictions)),
        (
            "child_approval",
            if row.carries == Carries::ChildRule {
                V::Some(Box::new(child_rule_value()))
            } else {
                V::None
            },
        ),
        (
            "approval",
            if row.carries == Carries::Approval {
                V::Some(Box::new(approval_value()))
            } else {
                V::None
            },
        ),
    ])
}
#[allow(clippy::expect_used)]
fn profile(deep: bool) -> MachineTlcProfile {
    let derived = attenuation();
    let domains = [
        ("EvidenceId", vec![text("grant_root"), text("grant_child")]),
        (
            "GrantPrincipal",
            vec![text("principal_a"), text("principal_b")],
        ),
        ("GrantAuthorityIncarnation", vec![text("owner_incarnation")]),
        ("GrantRecord", rows(deep).iter().map(row_value).collect()),
        (
            "DerivedChildRestrictions",
            vec![record([
                ("parent", restrictions(derived.parent())),
                ("requested", restrictions(derived.requested())),
                ("effective", restrictions(derived.effective())),
            ])],
        ),
        ("DecisionId", vec![text("decision_1")]),
        ("Sha256Digest", vec![text("digest_1")]),
        ("OriginalWorkRef", vec![text("work_1")]),
        ("GrantOperationBinding", vec![text("operation_1")]),
    ];
    MachineTlcProfile {
        named_values: domains
            .into_iter()
            .map(|(name, values)| (NamedTypeId::parse(name).expect("fixture type"), values))
            .collect::<BTreeMap<_, _>>(),
    }
}

pub(super) fn model() -> MachineTlcModel {
    MachineTlcModel {
        ci: profile(false),
        deep: profile(true),
        ci_limits: MachineTlcStateLimits {
            step_limit: 6,
            seq_limit: 2,
            set_limit: 2,
            map_limit: 2,
        },
        require_ci_transition_coverage: true,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::super::*;
    use super::*;
    use meerkat_authorization_contracts::work_association::OriginalWorkRef;
    use meerkat_core::approval::decision::{ApproverId, DecisionId, Sha256Digest};
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    fn principal(name: &str) -> GrantPrincipal {
        GrantPrincipal::new(
            PrincipalRef::in_domain(
                PrincipalKind::ServiceAccount,
                name,
                TrustDomainId::new("grant-model-fixture").expect("domain"),
            )
            .expect("principal"),
        )
        .expect("qualification")
    }
    fn incarnation(foreign: bool) -> GrantAuthorityIncarnation {
        let id = if foreign {
            "00000000-0000-4000-8000-000000000002"
        } else {
            "00000000-0000-4000-8000-000000000001"
        };
        GrantAuthorityIncarnation::from_uuid(uuid::Uuid::parse_str(id).expect("uuid")).expect("v4")
    }
    fn approver() -> ApproverId {
        ApproverId::new("approver_owner").expect("approver id")
    }
    fn decision() -> DecisionId {
        DecisionId::derive(
            &meerkat_core::approval::decision::DecisionKind::new("grant.child").expect("kind"),
            &meerkat_core::approval::decision::DecisionSubjectKey::new("fixture").expect("key"),
        )
    }
    fn digest() -> Sha256Digest {
        Sha256Digest::of(b"fixture terms")
    }
    fn work() -> OriginalWorkRef {
        OriginalWorkRef {
            authority: principal("principal_b").principal().clone(),
            work: EvidenceId::new("work_1").expect("work"),
        }
    }
    fn approval() -> GrantApproval {
        GrantApproval {
            decision_id: decision(),
            approver_id: approver(),
            tier: GrantDecisionTier::Human,
            assurance: GrantDecisionAssurance::PersonAuthenticated,
            request_digest: digest(),
            requester: principal("principal_a"),
            work: work(),
            operation: None,
            uses: 1,
            review_before_return: None,
        }
    }
    fn actual(row: &Row) -> GrantRecord {
        GrantRecord {
            id: EvidenceId::new(if row.child {
                "grant_child"
            } else {
                "grant_root"
            })
            .expect("id"),
            authority_incarnation: incarnation(row.foreign),
            parent: row
                .child
                .then(|| EvidenceId::new("grant_root").expect("parent")),
            issuer: principal(row.issuer),
            grantee: principal(row.grantee),
            represented_subject: None,
            issued_revision: if row.child { 2 } else { 1 },
            restrictions: row.restrictions.clone(),
            child_approval: (row.carries == Carries::ChildRule).then(|| ChildApprovalRule {
                approver_id: approver(),
                accepts_device_trust: GrantDeviceTrust::Refused,
            }),
            approval: (row.carries == Carries::Approval).then(approval),
        }
    }
    fn configured(row: &Row) -> GrantAuthorityMachineAuthority {
        let mut owner = GrantAuthorityMachineAuthority::new();
        owner
            .apply(GrantAuthorityInput::Configure {
                root: principal(row.issuer),
                namespace: EvidenceId::new("grant_root").expect("namespace"),
                generation: 1,
                incarnation: incarnation(false),
            })
            .expect("Configure");
        owner
    }
    fn issue_child(
        actor: &GrantPrincipal,
        record: &GrantRecord,
        chain: Vec<GrantRecord>,
    ) -> GrantAuthorityInput {
        GrantAuthorityInput::IssueChild {
            actor: actor.clone(),
            record: record.clone(),
            derived: attenuation(),
            chain,
            now_ms: 1,
            terms_digest: digest(),
        }
    }
    fn resolve(leaf: &GrantRecord, chain: Vec<GrantRecord>, now_ms: u64) -> GrantAuthorityInput {
        GrantAuthorityInput::ResolveUse {
            namespace: EvidenceId::new("grant_root").expect("namespace"),
            generation: 1,
            incarnation: incarnation(false),
            executor: leaf.grantee.clone(),
            represented_subject: None,
            requester: Some(principal("principal_a")),
            work: Some(work()),
            operation: None,
            leaf: leaf.clone(),
            chain,
            now_ms,
        }
    }
    fn consume(
        leaf: &GrantRecord,
        chain: Vec<GrantRecord>,
        approver_current: bool,
    ) -> GrantAuthorityInput {
        GrantAuthorityInput::ConsumeUse {
            namespace: EvidenceId::new("grant_root").expect("namespace"),
            generation: 1,
            incarnation: incarnation(false),
            executor: leaf.grantee.clone(),
            represented_subject: None,
            requester: Some(principal("principal_a")),
            work: Some(work()),
            operation: None,
            leaf: leaf.clone(),
            chain,
            now_ms: 1,
            approver_current,
        }
    }
    fn plain_rows(rows: &[Row]) -> Vec<Row> {
        rows.iter()
            .filter(|row| row.carries == Carries::Nothing && !row.foreign)
            .filter(|row| row.restrictions.lifetime.unresolved_facts().is_empty())
            .cloned()
            .collect()
    }
    fn approval_rows(rows: &[Row]) -> (Row, Row) {
        let rule = rows
            .iter()
            .find(|row| row.carries == Carries::ChildRule)
            .expect("rule root row")
            .clone();
        let child = rows
            .iter()
            .find(|row| row.carries == Carries::Approval)
            .expect("approval child row")
            .clone();
        (rule, child)
    }
    #[test]
    fn every_configured_fixture_root_reaches_the_plain_issuance_revocation_and_use_transitions() {
        for deep in [false, true] {
            let rows = plain_rows(&rows(deep));
            for pair in rows.chunks_exact(2) {
                let root = actual(&pair[0]);
                let child = actual(&pair[1]);
                let mut owner = configured(&pair[0]);
                owner
                    .apply(GrantAuthorityInput::IssueRoot {
                        actor: root.issuer.clone(),
                        record: root.clone(),
                    })
                    .expect("IssueRoot");
                owner
                    .apply(issue_child(&child.issuer, &child, vec![root.clone()]))
                    .expect("IssueChild");
                let before_use = owner.state().clone();
                let use_result = owner
                    .apply(resolve(&child, vec![root.clone(), child.clone()], 1))
                    .expect("ResolveUse");
                assert!(
                    use_result
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, GrantAuthorityEffect::UseResolved { .. }))
                );
                assert_eq!(owner.state(), &before_use);
                owner
                    .apply(GrantAuthorityInput::Revoke {
                        actor: root.issuer.clone(),
                        record: root.clone(),
                    })
                    .expect("RevokeNew");
                let after_revoke = owner.state().clone();
                owner
                    .apply(GrantAuthorityInput::Revoke {
                        actor: root.issuer.clone(),
                        record: root.clone(),
                    })
                    .expect("RevokeAlready");
                assert_eq!(owner.state(), &after_revoke);
                assert_eq!(owner.state().revision, 3);
                assert!(
                    owner
                        .apply(resolve(&child, vec![root, child.clone()], 1))
                        .is_err()
                );
            }
        }
    }
    /// The custody end, reached from Active and from Uncertain: every later
    /// change and use is refused, the retained facts stay, and a second
    /// release changes nothing.
    #[test]
    fn custody_end_refuses_every_later_change_and_use() {
        let rows = rows(false);
        let root = actual(&rows[0]);
        let child = actual(&rows[1]);
        for uncertain_first in [false, true] {
            let mut owner = configured(&rows[0]);
            owner
                .apply(GrantAuthorityInput::IssueRoot {
                    actor: root.issuer.clone(),
                    record: root.clone(),
                })
                .expect("IssueRoot");
            if uncertain_first {
                let uncertain = owner
                    .apply(GrantAuthorityInput::RecordUncertainAppend)
                    .expect("RecordUncertainAppend");
                assert!(matches!(
                    uncertain.effects(),
                    [GrantAuthorityEffect::CustodyUncertain]
                ));
                assert_eq!(
                    owner.state().lifecycle_phase,
                    GrantAuthorityPhase::Uncertain
                );
                assert!(
                    owner
                        .apply(GrantAuthorityInput::RecordUncertainAppend)
                        .is_err()
                );
            }
            let retained = owner.state().clone();
            let released = owner
                .apply(GrantAuthorityInput::ReleaseCustody)
                .expect("ReleaseCustody");
            assert!(matches!(
                released.effects(),
                [GrantAuthorityEffect::CustodyReleased]
            ));
            assert_eq!(owner.state().lifecycle_phase, GrantAuthorityPhase::Released);
            assert_eq!(owner.state().records, retained.records);
            assert_eq!(owner.state().revision, retained.revision);
            let after_release = owner.state().clone();
            for refused in [
                GrantAuthorityInput::IssueChild {
                    actor: child.issuer.clone(),
                    record: child.clone(),
                    derived: attenuation(),
                    chain: vec![root.clone()],
                    now_ms: 1,
                },
                GrantAuthorityInput::Revoke {
                    actor: root.issuer.clone(),
                    record: root.clone(),
                },
                resolve(&root, vec![root.clone()], 1),
                GrantAuthorityInput::RecordUncertainAppend,
            ] {
                assert!(owner.apply(refused).is_err());
            }
            let again = owner
                .apply(GrantAuthorityInput::ReleaseCustody)
                .expect("ReleaseCustodyAgain");
            assert!(again.effects().is_empty());
            assert_eq!(owner.state(), &after_release);
        }
    }
    #[test]
    fn the_approval_fixture_reaches_the_consume_transitions() {
        for deep in [false, true] {
            let (rule_row, child_row) = approval_rows(&rows(deep));
            let root = actual(&rule_row);
            let child = actual(&child_row);
            let mut owner = configured(&rule_row);
            owner
                .apply(GrantAuthorityInput::IssueRoot {
                    actor: root.issuer.clone(),
                    record: root.clone(),
                })
                .expect("IssueRoot with a child rule");
            owner
                .apply(issue_child(&child.issuer, &child, vec![root.clone()]))
                .expect("IssueChild of an approval child");
            assert_eq!(owner.state().spent.get(&child.id), Some(&0));
            assert!(owner.state().carried_decisions.contains(&decision()));
            let chain = vec![root.clone(), child.clone()];
            owner
                .apply(resolve(&child, chain.clone(), 1))
                .expect("ResolveUse of an unspent approval child");
            let refused = owner
                .apply(consume(&child, chain.clone(), false))
                .expect("ConsumeUseRefusedApproverRevoked");
            assert!(refused.effects().is_empty());
            assert_eq!(owner.state().spent.get(&child.id), Some(&0));
            let revision = owner.state().revision;
            let consumed = owner
                .apply(consume(&child, chain.clone(), true))
                .expect("ConsumeUse");
            assert!(
                consumed
                    .effects()
                    .iter()
                    .any(|effect| matches!(effect, GrantAuthorityEffect::UseConsumed { .. }))
            );
            assert_eq!(owner.state().spent.get(&child.id), Some(&1));
            assert_eq!(owner.state().consumed, 1);
            assert_eq!(owner.state().revision, revision + 1);
            assert!(owner.apply(resolve(&child, chain.clone(), 1)).is_err());
            assert!(owner.apply(consume(&child, chain, true)).is_err());
        }
    }
    #[test]
    fn a_plain_child_under_a_rule_and_an_approval_child_without_one_are_refused() {
        let rows = rows(false);
        let (rule_row, child_row) = approval_rows(&rows);
        let rule_root = actual(&rule_row);
        let approval_child = actual(&child_row);
        let plain = plain_rows(&rows);
        // A plain child under the rule root.
        let mut owner = configured(&rule_row);
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: rule_root.issuer.clone(),
                record: rule_root.clone(),
            })
            .expect("rule root");
        let plain_child = actual(&plain[1]);
        assert!(
            owner
                .apply(issue_child(
                    &plain_child.issuer,
                    &plain_child,
                    vec![rule_root]
                ))
                .is_err()
        );
        // The approval child under a plain root.
        let plain_root = actual(&plain[0]);
        let mut owner = configured(&plain[0]);
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: plain_root.issuer.clone(),
                record: plain_root.clone(),
            })
            .expect("plain root");
        assert!(
            owner
                .apply(issue_child(
                    &approval_child.issuer,
                    &approval_child,
                    vec![plain_root]
                ))
                .is_err()
        );
    }
    #[test]
    fn an_approval_leaf_never_resolves_without_its_requester_and_work() {
        let (rule_row, child_row) = approval_rows(&rows(false));
        let root = actual(&rule_row);
        let child = actual(&child_row);
        let mut owner = configured(&rule_row);
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: root.issuer.clone(),
                record: root.clone(),
            })
            .expect("rule root");
        owner
            .apply(issue_child(&child.issuer, &child, vec![root.clone()]))
            .expect("approval child");
        let chain = vec![root, child.clone()];
        let other_work = OriginalWorkRef {
            authority: principal("principal_b").principal().clone(),
            work: EvidenceId::new("work_2").expect("work"),
        };
        for (requester, work) in [
            (None, Some(work())),
            (Some(principal("principal_a")), None),
            (Some(principal("principal_b")), Some(work())),
            (Some(principal("principal_a")), Some(other_work)),
        ] {
            let resolve_use = GrantAuthorityInput::ResolveUse {
                namespace: EvidenceId::new("grant_root").expect("namespace"),
                generation: 1,
                incarnation: incarnation(false),
                executor: child.grantee.clone(),
                represented_subject: None,
                requester: requester.clone(),
                work: work.clone(),
                operation: None,
                leaf: child.clone(),
                chain: chain.clone(),
                now_ms: 1,
            };
            assert!(owner.apply(resolve_use).is_err());
            let consume_use = GrantAuthorityInput::ConsumeUse {
                namespace: EvidenceId::new("grant_root").expect("namespace"),
                generation: 1,
                incarnation: incarnation(false),
                executor: child.grantee.clone(),
                represented_subject: None,
                requester,
                work,
                operation: None,
                leaf: child.clone(),
                chain: chain.clone(),
                now_ms: 1,
                approver_current: true,
            };
            assert!(owner.apply(consume_use).is_err());
        }
        assert_eq!(owner.state().spent.get(&child.id), Some(&0));
    }
    #[test]
    fn finite_fixture_negatives_refuse_foreign_unresolved_and_expired_use() {
        let rows = rows(true);
        let unresolved_row = rows
            .iter()
            .find(|row| !row.restrictions.lifetime.unresolved_facts().is_empty())
            .expect("unresolved row")
            .clone();
        let foreign_row = rows
            .iter()
            .find(|row| row.foreign)
            .expect("foreign row")
            .clone();
        let mut owner = configured(&rows[0]);
        let foreign = actual(&foreign_row);
        assert!(
            owner
                .apply(GrantAuthorityInput::IssueRoot {
                    actor: foreign.issuer.clone(),
                    record: foreign
                })
                .is_err()
        );
        let unresolved = actual(&unresolved_row);
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: unresolved.issuer.clone(),
                record: unresolved.clone(),
            })
            .expect("unresolved issuance is candidate data");
        assert!(
            owner
                .apply(resolve(&unresolved, vec![unresolved.clone()], 1))
                .is_err()
        );
        let mut owner = configured(&rows[0]);
        let root = actual(&rows[0]);
        let child = actual(&rows[1]);
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor: root.issuer.clone(),
                record: root.clone(),
            })
            .expect("root");
        owner
            .apply(issue_child(&child.issuer, &child, vec![root.clone()]))
            .expect("child");
        for now_ms in [0, 2] {
            assert!(
                owner
                    .apply(resolve(&child, vec![root.clone(), child.clone()], now_ms))
                    .is_err()
            );
        }
        owner
            .apply(resolve(&child, vec![root, child.clone()], 1))
            .expect("inside exact window");
    }
}
