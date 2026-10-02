//! Admission-specific policy input: no fabricated operation, run or transcript.
use super::*;
use meerkat_core::ControllerModelFacts;

struct AccountRules {
    routes: Vec<ControllerModelFacts>,
    restrictions: ExecutionRestrictions,
    empty: bool,
    calls: AtomicU64,
}
impl OperationPolicyOwner for AccountRules {
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        panic!("admission must not manufacture a prepared operation");
    }
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let candidate = association.candidate();
        if candidate.requester != who("requester")
            || candidate.logical_executor != who("executor")
            || candidate.represented_subject != Some(who("subject"))
            || !self.routes.iter().any(|route| {
                route.selection() == facts.selection()
                    && route.endpoint() == facts.endpoint()
                    && route.wire_model() == facts.wire_model()
            })
        {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: if self.empty {
                vec![]
            } else {
                vec![tuple("infer", "models")]
            },
            restrictions: self.restrictions.clone(),
        })
    }
}
fn plain(account: &str, model_name: &str, endpoint: &str, wire: &str) -> ControllerModelFacts {
    let credential = AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
        realm: RealmId::parse("fixture").expect("realm"),
        account: meerkat_core::CredentialAccountId::parse(account).expect("account"),
    });
    ControllerModelFacts::new(
        ControllerModelSelection::new(
            SessionLlmIdentity {
                model: model_name.into(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            credential,
            "fixture-profile".into(),
            "fixture-backend".into(),
        ),
        endpoint.into(),
        wire.into(),
    )
}
fn claim(fixture: &Fixture, facts: &ControllerModelFacts) -> InputAuthorityAssociation {
    let mut candidate = fixture.association().candidate().clone();
    candidate.controller_model = Some(facts.selection().clone());
    InputAuthorityAssociation::new(candidate).expect("data claim")
}
fn policy(
    fixture: &Fixture,
    rules: AccountRules,
) -> (GrantBackedWorkPolicy, Arc<Admitted>, Arc<AccountRules>) {
    let admitted = Arc::new(Admitted {
        original: fixture.association(),
        requester_allowed: AtomicBool::new(false),
        ordinary_allowed: AtomicBool::new(false),
        controller_allowed: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
    });
    let rules = Arc::new(rules);
    (
        GrantBackedWorkPolicy::new(fixture.grants.clone(), admitted.clone(), rules.clone()),
        admitted,
        rules,
    )
}
fn rules(routes: Vec<ControllerModelFacts>) -> AccountRules {
    AccountRules {
        routes,
        restrictions: ExecutionRestrictions::unrestricted(),
        empty: false,
        calls: AtomicU64::new(0),
    }
}

#[test]
fn admission_checks_correlated_account_route_without_inventing_running_work() {
    let fixture = Fixture::new();
    let a = plain(
        "account-a",
        "model-a",
        "https://a.invalid/responses",
        "wire-a",
    );
    let b = plain(
        "account-b",
        "model-b",
        "https://b.invalid/responses",
        "wire-b",
    );
    let (policy, admitted, owner) = policy(&fixture, rules(vec![a.clone(), b.clone()]));
    for facts in [&a, &b] {
        policy
            .validate_controller_admission(&claim(&fixture, facts), facts)
            .expect("actual correlated pair");
    }
    for facts in [
        plain(
            "account-a",
            "model-b",
            "https://b.invalid/responses",
            "wire-b",
        ),
        plain(
            "account-b",
            "model-a",
            "https://a.invalid/responses",
            "wire-a",
        ),
        plain(
            "account-a",
            "model-a",
            "https://other.invalid/responses",
            "wire-a",
        ),
        plain(
            "account-a",
            "model-a",
            "https://a.invalid/responses",
            "wrong-wire-model",
        ),
    ] {
        assert!(
            policy
                .validate_controller_admission(&claim(&fixture, &facts), &facts)
                .is_err()
        );
    }
    assert_eq!(owner.calls.load(Ordering::SeqCst), 6);
    assert!(
        admitted.calls.lock().expect("calls").is_empty(),
        "no fabricated running-work lookup"
    );
}

#[test]
fn admission_does_not_infer_actual_caller_or_subject_from_held_account() {
    let fixture = Fixture::new();
    let facts = plain(
        "account-a",
        "model-a",
        "https://a.invalid/responses",
        "wire-a",
    );
    let (policy, _, _) = policy(&fixture, rules(vec![facts.clone()]));
    let original = claim(&fixture, &facts);
    policy
        .validate_controller_admission(&original, &facts)
        .expect("control");
    for dimension in 0..3 {
        let mut candidate = original.candidate().clone();
        match dimension {
            0 => candidate.requester = who("other-requester"),
            1 => candidate.represented_subject = None,
            _ => candidate.logical_executor = who("other-executor"),
        }
        let changed = InputAuthorityAssociation::new(candidate).expect("claims are not permission");
        assert!(
            policy
                .validate_controller_admission(&changed, &facts)
                .is_err()
        );
    }
}

#[test]
fn admission_requires_actual_lineage_exact_controller_ceiling_and_work_lifetime() {
    let fixture = Fixture::new();
    let facts = plain(
        "account-a",
        "model-a",
        "https://a.invalid/responses",
        "wire-a",
    );
    let (policy, _, _) = policy(&fixture, rules(vec![facts.clone()]));
    let original = claim(&fixture, &facts);
    policy
        .validate_controller_admission(&original, &facts)
        .expect("control");
    for dimension in 0..3 {
        let mut candidate = original.candidate().clone();
        match dimension {
            0 => candidate.controller_grant_lineage[0].issued_revision += 1,
            1 => candidate.controller_ceiling.actions = ExactRestriction::exact([action("write")]),
            _ => candidate.controller_ceiling.lifetime = LifetimeRestriction::window(0, 1_000),
        }
        let changed =
            InputAuthorityAssociation::new(candidate).expect("well-formed historical claims");
        assert!(
            policy
                .validate_controller_admission(&changed, &facts)
                .is_err()
        );
    }
    for dimension in 0..3 {
        let mut owner = rules(vec![facts.clone()]);
        match dimension {
            0 => owner.restrictions.lifetime = LifetimeRestriction::window(0, 1_000),
            1 => {
                owner.restrictions.actions =
                    ExactRestriction::unresolved(UnresolvedConstraint::Unavailable);
            }
            _ => owner.empty = true,
        }
        let (policy, _, _) = self::policy(&fixture, owner);
        assert!(
            policy
                .validate_controller_admission(&original, &facts)
                .is_err()
        );
    }
    // Ordinary grant expiry does not shorten the separately admitted controller.
    fixture.clock.0.store(301, Ordering::SeqCst);
    policy
        .validate_controller_admission(&original, &facts)
        .expect("ordinary operation expiry is separate");
}

#[test]
fn operation_only_owner_cannot_implicitly_enable_controller_admission() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let facts = ControllerModelFacts::new(
        selection(),
        "https://fixture.invalid/v1/responses".into(),
        "controller-wire-model".into(),
    );
    assert!(
        policy
            .validate_controller_admission(&association, &facts)
            .is_err(),
        "dedicated owner contract is mandatory"
    );
}

#[test]
fn admission_clock_unavailable_is_not_a_policy_deny_or_stale_observation() {
    let fixture = Fixture::new();
    let facts = plain(
        "account-a",
        "model-a",
        "https://a.invalid/responses",
        "wire-a",
    );
    let (policy, _, _) = policy(&fixture, rules(vec![facts.clone()]));
    let original = claim(&fixture, &facts);
    policy
        .validate_controller_admission(&original, &facts)
        .expect("live control");
    fixture.clock.1.store(true, Ordering::SeqCst);
    assert!(matches!(
        policy.validate_controller_admission(&original, &facts),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    fixture.clock.1.store(false, Ordering::SeqCst);
    policy
        .validate_controller_admission(&original, &facts)
        .expect("same owner recovers");
}
