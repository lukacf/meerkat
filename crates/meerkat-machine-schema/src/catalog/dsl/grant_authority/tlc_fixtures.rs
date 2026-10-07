//! Small, linked finite fixtures for this owner, not a second grant evaluator.
//! Non-lifetime/depth restrictions remain opaque and equal in this model.
//! Rust tests exercise the same fixture rows through the generated owner.

use crate::identity::{FieldId, NamedTypeId};
use crate::{MachineTlcModel, MachineTlcProfile, MachineTlcStateLimits, TlcValue as V};
use meerkat_authorization_contracts::constraints::{
    DelegationDepth, DepthBound, ExecutionRestrictions, LifetimeBound, LifetimeRestriction,
    UnresolvedConstraint,
};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use std::collections::BTreeMap;

#[derive(Clone)]
struct Row {
    issuer: &'static str,
    grantee: &'static str,
    child: bool,
    foreign: bool,
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
            restrictions: derived.parent().clone(),
        });
        rows.push(Row {
            issuer: grantee,
            grantee: issuer,
            child: true,
            foreign: false,
            restrictions: derived.effective().clone(),
        });
    }
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
    fn resolve(leaf: &GrantRecord, chain: Vec<GrantRecord>, now_ms: u64) -> GrantAuthorityInput {
        GrantAuthorityInput::ResolveUse {
            namespace: EvidenceId::new("grant_root").expect("namespace"),
            generation: 1,
            incarnation: incarnation(false),
            executor: leaf.grantee.clone(),
            represented_subject: None,
            leaf: leaf.clone(),
            chain,
            now_ms,
        }
    }
    #[test]
    fn every_configured_fixture_root_reaches_all_six_generated_transitions() {
        for deep in [false, true] {
            let rows = rows(deep);
            for pair in rows[..4].chunks_exact(2) {
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
                    .apply(GrantAuthorityInput::IssueChild {
                        actor: child.issuer.clone(),
                        record: child.clone(),
                        derived: attenuation(),
                        chain: vec![root.clone()],
                        now_ms: 1,
                    })
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
    #[test]
    fn finite_fixture_negatives_refuse_foreign_unresolved_and_expired_use() {
        let rows = rows(true);
        let mut owner = configured(&rows[0]);
        let foreign = actual(&rows[5]);
        assert!(
            owner
                .apply(GrantAuthorityInput::IssueRoot {
                    actor: foreign.issuer.clone(),
                    record: foreign
                })
                .is_err()
        );
        let unresolved = actual(&rows[4]);
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
            .apply(GrantAuthorityInput::IssueChild {
                actor: child.issuer.clone(),
                record: child.clone(),
                derived: attenuation(),
                chain: vec![root.clone()],
                now_ms: 1,
            })
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
