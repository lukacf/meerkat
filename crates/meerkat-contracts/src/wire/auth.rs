//! Auth, principal, grant, and visibility wire contracts.
//!
//! The canonical types live in `meerkat-core`; wire surfaces re-export the
//! same shapes so transports cannot invent competing authority vocabulary.

pub use meerkat_core::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalId, PrincipalKind,
    PrincipalQualification, PrincipalRef, TrustDomainId, VisibilityClass,
};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn human(id: &str) -> PrincipalRef {
        PrincipalRef::new(PrincipalKind::Human, id).expect("valid human principal")
    }

    // Captured and verified against the pre-qualification 0.8.49 types.
    const LEGACY_AUTH_WIRE: &str =
        include_str!("../../tests/fixtures/auth_principals_v0_8_49.json");

    #[test]
    fn legacy_principal_wire_bytes_roundtrip() {
        type LegacyAuth = (Vec<PrincipalRef>, Vec<AuthGrant>, Vec<VisibilityClass>);
        let decoded: LegacyAuth = serde_json::from_str(LEGACY_AUTH_WIRE).unwrap();
        for principal in &decoded.0 {
            assert!(PrincipalId::new(principal.id.as_str()).is_ok());
            assert_eq!(principal.qualification, PrincipalQualification::Unqualified);
        }
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            LEGACY_AUTH_WIRE.trim_end()
        );
    }

    #[test]
    fn legacy_unqualified_ids_are_not_silently_migrated() {
        // Released deserialization accepted these even though new() rejects
        // them. Keep that legacy wire behavior until an explicit migration.
        for raw in [
            r#"{"kind":"human","id":""}"#,
            r#"{"kind":"human","id":" \t "}"#,
            r#"{"kind":"human","id":"human:\nlegacy"}"#,
        ] {
            let principal: PrincipalRef = serde_json::from_str(raw).unwrap();
            assert!(PrincipalId::new(principal.id.as_str()).is_err());
            assert_eq!(principal.qualification, PrincipalQualification::Unqualified);
            assert_eq!(serde_json::to_string(&principal).unwrap(), raw);
        }
    }

    #[test]
    fn qualified_principal_wire_roundtrip_retains_exact_domain() {
        let principal = PrincipalRef::in_domain(
            PrincipalKind::Human,
            "human:alice",
            TrustDomainId::new("authority:one").unwrap(),
        )
        .unwrap();
        let encoded = serde_json::to_value(&principal).unwrap();
        assert_eq!(
            encoded,
            json!({
                "kind": "human",
                "id": "human:alice",
                "qualification": {
                    "kind": "qualified",
                    "trust_domain_id": "authority:one"
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<PrincipalRef>(encoded).unwrap(),
            principal
        );
        let grant = AuthGrant {
            principal: principal.clone(),
            scope: GrantScope::Session {
                session_id: "session:one".to_owned(),
            },
            actions: BTreeSet::from([GrantAction::Observe]),
            acting_on_behalf_of: None,
        };
        let visibility = VisibilityClass::Private { principal };
        let bytes = serde_json::to_vec(&(&grant, &visibility)).unwrap();
        assert_eq!(
            serde_json::from_slice::<(AuthGrant, VisibilityClass)>(&bytes).unwrap(),
            (grant, visibility)
        );
    }

    #[test]
    fn principal_grant_and_visibility_wire_roundtrip() {
        let principal = human("human:alice");
        let scope = GrantScope::application("client.project", "repo-a").expect("valid scope");
        let grant = AuthGrant {
            principal: principal.clone(),
            scope: scope.clone(),
            actions: BTreeSet::from([GrantAction::Observe, GrantAction::ReplayEvents]),
            acting_on_behalf_of: None,
        };
        let visibility = VisibilityClass::Scoped { scope };

        let encoded = serde_json::to_value((&principal, &grant, &visibility)).unwrap();
        assert_eq!(encoded[0]["kind"], "human");
        assert_eq!(encoded[1]["actions"][0], "observe");
        assert_eq!(encoded[2]["visibility"], "scoped");

        let decoded: (PrincipalRef, AuthGrant, VisibilityClass) =
            serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, (principal, grant, visibility));
    }

    #[test]
    fn acting_on_behalf_of_wire_shape_is_explicit() {
        let actor = PrincipalRef::new(PrincipalKind::PersonalAgent, "agent:bob").unwrap();
        let subject = human("human:bob");
        let relationship = ActingOnBehalfOf::new(actor, subject);

        let encoded = serde_json::to_value(&relationship).unwrap();
        assert_eq!(encoded["actor"]["kind"], "personal_agent");
        assert_eq!(encoded["subject"]["kind"], "human");
        assert_eq!(encoded["subject"]["id"], "human:bob");
        let decoded: ActingOnBehalfOf = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, relationship);
    }

    #[test]
    fn metadata_like_json_is_not_a_visibility_contract() {
        let metadata_like = json!({
            "labels": {
                "principal": "human:alice",
                "visibility": "client.project:repo-a"
            }
        });

        assert!(serde_json::from_value::<VisibilityClass>(metadata_like).is_err());
    }
}
