//! Auth, principal, grant, and visibility wire contracts.
//!
//! The canonical types live in `meerkat-core`; wire surfaces re-export the
//! same shapes so transports cannot invent competing authority vocabulary.

pub use meerkat_core::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalId, PrincipalKind, PrincipalRef,
    VisibilityClass,
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
