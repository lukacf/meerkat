//! Trusted application compiler input to the existing native policy factory.
//!
//! Applications select members, exact tool actions and consequences. Native
//! code validates the contract and owns its canonical policy representation.
//! Compiling this artifact neither installs tools nor grants their execution.

use meerkat_core::{
    CompiledApplicationToolPolicy, CompiledApplicationToolPolicyError, CompiledMemberToolGrants,
    CompiledPolicySourceProvenance, PolicyId, PolicyProviderId, PolicyRevision,
};
use serde::{Deserialize, Serialize};

/// Pre-digest input from the trusted application compiler. The policy digest,
/// schema version and default-deny flag are produced by the native constructor.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationToolPolicyInput {
    pub provider_id: PolicyProviderId,
    pub policy_id: PolicyId,
    pub revision: PolicyRevision,
    pub source: CompiledPolicySourceProvenance,
    pub members: Vec<CompiledMemberToolGrants>,
}

impl ApplicationToolPolicyInput {
    /// Canonical ordering does not infer, widen or deduplicate policy. Duplicate
    /// members or grants, malformed identities and invalid revisions are still
    /// refused by the actual native constructor.
    pub fn compile(mut self) -> Result<Vec<u8>, CompiledApplicationToolPolicyError> {
        self.members
            .sort_by(|left, right| left.member_identity.cmp(&right.member_identity));
        for member in &mut self.members {
            member
                .grants
                .sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
        }
        CompiledApplicationToolPolicy::new(
            self.provider_id,
            self.policy_id,
            self.revision,
            self.source,
            self.members,
        )?
        .canonical_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        CompiledMemberToolAction, CompiledMemberToolGrant, CompiledToolConsequence, PolicyDigest,
    };

    fn input() -> ApplicationToolPolicyInput {
        ApplicationToolPolicyInput {
            provider_id: PolicyProviderId::new("application").unwrap(),
            policy_id: PolicyId::new("installed-tools").unwrap(),
            revision: PolicyRevision(1),
            source: CompiledPolicySourceProvenance {
                source_id: "trusted-application-policy".into(),
                source_digest: PolicyDigest::from_canonical_bytes(b"application-owned-input"),
            },
            members: vec![
                CompiledMemberToolGrants {
                    member_identity: "writer".into(),
                    grants: vec![
                        CompiledMemberToolGrant {
                            tool_name: "write".into(),
                            action: CompiledMemberToolAction::Invoke,
                            consequence: CompiledToolConsequence::R2,
                        },
                        CompiledMemberToolGrant {
                            tool_name: "read".into(),
                            action: CompiledMemberToolAction::Invoke,
                            consequence: CompiledToolConsequence::R0,
                        },
                    ],
                },
                CompiledMemberToolGrants {
                    member_identity: "observer".into(),
                    grants: vec![],
                },
            ],
        }
    }

    #[test]
    fn output_is_accepted_by_native_parser_and_preserves_explicit_policy() {
        let selected = input();
        let bytes = selected.clone().compile().unwrap();
        let native = CompiledApplicationToolPolicy::parse_canonical_json(&bytes).unwrap();
        assert_eq!(native.provider_id, selected.provider_id);
        assert_eq!(native.policy_id, selected.policy_id);
        assert_eq!(native.source, selected.source);
        assert_eq!(native.revision, selected.revision);
        assert!(native.default_deny);
        assert_eq!(native.members[0].member_identity, "observer");
        assert!(native.members[0].grants.is_empty());
        assert_eq!(native.members[1].grants[0].tool_name, "read");
        assert_eq!(
            native.members[1].grants[0].consequence,
            CompiledToolConsequence::R0
        );
        assert_eq!(
            native.members[1].grants[1].consequence,
            CompiledToolConsequence::R2
        );
        assert_eq!(native.canonical_json().unwrap(), bytes);
        let mut reordered = selected;
        reordered.members.reverse();
        for member in &mut reordered.members {
            member.grants.reverse();
        }
        assert_eq!(reordered.compile().unwrap(), bytes);
    }

    #[test]
    fn duplicate_selections_and_invalid_native_values_are_refused() {
        let mut duplicate_member = input();
        duplicate_member
            .members
            .push(duplicate_member.members[0].clone());
        assert!(matches!(
            duplicate_member.compile(),
            Err(CompiledApplicationToolPolicyError::InvalidContract(_))
        ));
        let mut duplicate_grant = input();
        let grant = duplicate_grant.members[0].grants[0].clone();
        duplicate_grant.members[0].grants.push(grant);
        assert!(matches!(
            duplicate_grant.compile(),
            Err(CompiledApplicationToolPolicyError::InvalidContract(_))
        ));
        let mut invalid_revision = input();
        invalid_revision.revision = PolicyRevision(0);
        assert!(matches!(
            invalid_revision.compile(),
            Err(CompiledApplicationToolPolicyError::InvalidContract(_))
        ));
        let mut invalid_member = input();
        invalid_member.members[0].member_identity = " writer".into();
        assert!(matches!(
            invalid_member.compile(),
            Err(CompiledApplicationToolPolicyError::InvalidContract(_))
        ));
    }

    #[test]
    fn wire_input_cannot_override_native_generated_fields() {
        for (field, value) in [
            ("default_deny", serde_json::json!(false)),
            ("schema_version", serde_json::json!(999)),
            ("policy_digest", serde_json::json!("forged")),
        ] {
            let mut value_input = serde_json::to_value(input()).unwrap();
            value_input
                .as_object_mut()
                .unwrap()
                .insert(field.into(), value);
            assert!(serde_json::from_value::<ApplicationToolPolicyInput>(value_input).is_err());
        }
    }
}
