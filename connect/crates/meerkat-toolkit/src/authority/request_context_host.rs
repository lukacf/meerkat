//! Mechanical comparison at trusted native ingress. This is not an ingress
//! authenticator or a replacement for NativeWorkAuthorizationHost. The runtime
//! authenticates the actual final Input and association through its native host.

use super::{SetupContextError, SetupRequestEvidence};
use meerkat_runtime::input_authority::NativeIngressContext;

pub fn check_ingress_correspondence(
    evidence: &SetupRequestEvidence,
    ingress: &NativeIngressContext,
) -> Result<(), SetupContextError> {
    let association = evidence.association().candidate();
    if &association.requester != ingress.requester()
        || &association.ingress_actor != ingress.ingress_actor()
        || &association.ingress_namespace.realm != ingress.realm()
    {
        return Err(SetupContextError::WrongContext);
    }
    // A retry may have fresh authentication while retaining its original
    // historical observation. The native replay/admission owner checks that
    // distinction and binds this process-only ingress to the final Input.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Ceiling, SetupProposalConstraints, test_support as fixture};
    use meerkat_core::auth::PrincipalKind;
    use meerkat_runtime::input::{Input, PromptInput};
    use toolkit_package::{Id, Revision};

    #[test]
    fn fresh_retry_authentication_preserves_original_evidence_but_not_foreign_requester() {
        let requester = fixture::principal(PrincipalKind::Human, "person");
        let association = fixture::association(
            requester.clone(),
            fixture::target("agent", &requester),
            "work",
            "event",
        );
        let evidence = SetupRequestEvidence::new(
            association.clone(),
            SetupProposalConstraints {
                tenant: Id::new("tenant").unwrap(),
                deployment: Id::new("deployment").unwrap(),
                policy_revision: Revision::new("1").unwrap(),
                issued_at_ms: 1,
                expires_at_ms: 2,
                ceiling: Ceiling {
                    actions: Default::default(),
                    services: Default::default(),
                    frontends: Default::default(),
                    output_audiences: Default::default(),
                },
            },
        )
        .unwrap();
        let mut prompt = PromptInput::new("same submitted content", None);
        prompt.header.authority_association = Some(association.clone());
        let input = Input::Prompt(prompt);
        let fresh = fixture::event("fresh-authentication");
        assert!(fresh != association.candidate().original_authentication);
        for (actual_requester, expected) in [
            (requester, Ok(())),
            (
                fixture::principal(PrincipalKind::Human, "another-person"),
                Err(SetupContextError::WrongContext),
            ),
        ] {
            let ingress = NativeIngressContext::from_trusted_ingress(
                &input,
                actual_requester,
                association.candidate().ingress_actor.clone(),
                association.candidate().ingress_namespace.realm.clone(),
                fresh.clone(),
            )
            .unwrap();
            assert_eq!(check_ingress_correspondence(&evidence, &ingress), expected);
        }
        // Passing this comparison is not a native replay verdict or admission.
        assert_eq!(evidence.association(), &association);
    }
}
