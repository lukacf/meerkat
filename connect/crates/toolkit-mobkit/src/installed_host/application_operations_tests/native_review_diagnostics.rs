//! Component controls reuse real issued source/grant fixtures. A sentinel
//! owner checks forwarding only; it is never installed in a native runtime.
use super::*;
use crate::private_originals::review_owner::observe_configuration;
use std::sync::Mutex;

#[tokio::test]
async fn native_review_observer_preserves_configuration_and_real_owner_results() {
    let f = Fixture::new().await;
    let retained = f.source.retained_resume_check(f.application.clone());
    let configuration = f.source.configuration(
        ScopedInvocation::from_policy(f.application.clone()),
        f.application.clone(),
        Some(retained),
    );
    let grants = configuration.grants.clone();
    let ingress = configuration.ingress.clone();
    let inner = configuration.invocation_owner.clone();
    let operation = configuration.operation_owner.clone();
    let retained = configuration.retained_resume.clone().unwrap();
    let require_scoped_destination = configuration.require_scoped_destination;
    assert!(configuration.configured_mcp_attachment.is_none());
    let observed = observe_configuration(
        configuration,
        f.application.originals.review_observations().clone(),
    );
    assert!(Arc::ptr_eq(&observed.grants, &grants));
    assert!(Arc::ptr_eq(&observed.ingress, &ingress));
    assert!(Arc::ptr_eq(&observed.operation_owner, &operation));
    assert!(Arc::ptr_eq(
        observed.retained_resume.as_ref().unwrap(),
        &retained
    ));
    assert_eq!(
        observed.require_scoped_destination,
        require_scoped_destination
    );
    assert!(observed.configured_mcp_attachment.is_none());
    let binding = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    let expected = inner.review_mandate(&f.association, &binding, now).unwrap();
    assert!(expected.is_some());
    assert_eq!(
        observed
            .invocation_owner
            .review_mandate(&f.association, &binding, now)
            .unwrap(),
        expected
    );
    let expected = inner
        .authorize_admitted_work(&f.association, &binding, LocalPolicyPurpose::Operation, now)
        .unwrap();
    let actual = observed
        .invocation_owner
        .authorize_admitted_work(&f.association, &binding, LocalPolicyPurpose::Operation, now)
        .unwrap();
    assert_eq!(actual.restrictions, expected.restrictions);
    assert_eq!(actual.expires_at_ms, expected.expires_at_ms);
    // This is the actual production configuration entry, outside Core's owner.
    let actual = f.application.originals.configuration(f.application.clone());
    assert_eq!(
        actual
            .invocation_owner
            .review_mandate(&f.association, &binding, now)
            .unwrap(),
        inner.review_mandate(&f.association, &binding, now).unwrap()
    );
}

struct OnceOwner {
    result: Mutex<Option<Result<Option<String>, OperationAuthorizationError>>>,
    calls: Mutex<Vec<(usize, usize, u64)>>,
}
impl AdmittedWorkPolicyOwner for OnceOwner {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        panic!("passive observation must not ask for admission")
    }
    fn review_mandate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        self.calls.lock().unwrap().push((
            std::ptr::from_ref(association) as usize,
            std::ptr::from_ref(binding) as usize,
            now_ms,
        ));
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("one native owner callback")
    }
}

#[tokio::test]
async fn native_review_observer_calls_exact_owner_once_and_preserves_every_answer() {
    let f = Fixture::new().await;
    let binding = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    let mut answers = vec![
        Ok(None),
        Ok(Some(String::new())),
        Ok(Some("private-\u{00e5}".into())),
        Ok(Some("x".repeat(4097))),
    ];
    for error in [
        OperationAuthorizationError::Unavailable,
        OperationAuthorizationError::ObservationUnavailable(
            meerkat_core::authorization::OperationObservationError,
        ),
        OperationRefused::new(OperationRefusalKind::Denied).into(),
        OperationRefused::new(OperationRefusalKind::MalformedFacts).into(),
        OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
    ] {
        answers.push(Err(error));
    }
    for answer in answers {
        let expected = answer.clone();
        let pointer = answer
            .as_ref()
            .ok()
            .and_then(|value| value.as_ref())
            .map(|value| value.as_ptr());
        let owner = Arc::new(OnceOwner {
            result: Mutex::new(Some(answer)),
            calls: Mutex::new(Vec::new()),
        });
        let mut configuration = f.source.configuration(
            ScopedInvocation::from_policy(f.application.clone()),
            f.application.clone(),
            None,
        );
        // Test-only sentinel at the exact public Core field the observer wraps.
        configuration.invocation_owner = owner.clone();
        let observed = observe_configuration(
            configuration,
            f.application.originals.review_observations().clone(),
        );
        let actual = observed
            .invocation_owner
            .review_mandate(&f.association, &binding, 123);
        assert_eq!(actual, expected);
        assert_eq!(
            actual
                .as_ref()
                .ok()
                .and_then(|value| value.as_ref())
                .map(|value| value.as_ptr()),
            pointer
        );
        assert_eq!(
            *owner.calls.lock().unwrap(),
            vec![(
                std::ptr::from_ref(&f.association) as usize,
                std::ptr::from_ref(&binding) as usize,
                123
            )]
        );
        assert!(owner.result.lock().unwrap().is_none());
    }
}
