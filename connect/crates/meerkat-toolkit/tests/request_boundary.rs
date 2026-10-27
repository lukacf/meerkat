//! Request value tests, not evidence of a running setup agent or MobKit tool.

// Reuse the exact lower-crate data fixture. Its current owner refuses every
// operation; matching retained evidence is not native admission or permission.
#[path = "../src/authority/test_support.rs"]
mod native_fixture;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, NativeWorkTarget,
};
use meerkat_core::{
    auth::{PrincipalKind, PrincipalRef},
    authorization::WorkAuthorizationContext,
    connection::RealmId,
};
fn principal(value: &str) -> PrincipalRef {
    native_fixture::principal(PrincipalKind::Human, value)
}
fn dest(value: &str) -> NativeWorkTarget {
    native_fixture::target(value, &principal("person-a"))
}

use meerkat_toolkit::authority::*;
use meerkat_toolkit::request::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use toolkit_package::{ContentDigest, Id, Revision};

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}

struct Fixture {
    tenant: Id,
    deployment: Id,
    receiver: Id,
    destination: NativeWorkTarget,
    policy: Revision,
    realm: RealmId,
    native: WorkAuthorizationContext,
}

impl Fixture {
    fn new() -> Self {
        Self {
            tenant: id("tenant-a"),
            deployment: id("deployment-a"),
            receiver: id("original-agent"),
            destination: dest("original-agent"),
            policy: rev("policy-1"),
            realm: RealmId::parse("core-realm").unwrap(),
            native: native_fixture::current(&native_fixture::association(
                principal("person-a"),
                dest("original-agent"),
                "private-conversation",
                "original-event",
            )),
        }
    }

    fn context(&self, now_ms: u64) -> SetupUseContext<'_> {
        SetupUseContext {
            tenant: &self.tenant,
            deployment: &self.deployment,
            target: &self.destination,
            realm: &self.realm,
            authorization: &self.native,
            policy_revision: &self.policy,
            now_ms,
        }
    }

    fn authority(&self, requester: &str) -> SetupRequestEvidence {
        SetupRequestEvidence::new(
            native_fixture::association(
                principal(requester),
                self.destination.clone(),
                "private-conversation",
                "original-event",
            ),
            SetupProposalConstraints {
                tenant: self.tenant.clone(),
                deployment: self.deployment.clone(),
                policy_revision: self.policy.clone(),
                issued_at_ms: 100,
                expires_at_ms: 1000,
                ceiling: Ceiling {
                    actions: [ActionPermission {
                        action: Action::FindAndInstall,
                        resource: self.receiver.clone(),
                    }]
                    .into(),
                    services: Default::default(),
                    frontends: Default::default(),
                    output_audiences: [id(requester)].into(),
                },
            },
        )
        .unwrap()
    }

    fn host(&self) -> HostSetupRequest {
        HostSetupRequest {
            setup_helper: None,
            target: self.receiver.clone(),
            correlation: id("logical-call-1"),
            parent_operation: Some(id("business-task-1")),
            continuation: TaskContinuation { native_original: None,
                owner: id("runtime-owner"),
                task: id("business-task-1"),
                revision: rev("task-1"),
                delivery_key: id("setup-result-1"),
                audience: [id("person-a")].into(),
            },
            environment_revision: rev("environment-1"),
            limits: SetupLimits {
                deadline_ms: 900,
                max_workers: 3,
                max_effect_attempts: 5,
            },
        }
    }
}

fn request() -> FindAndInstallRequest {
    FindAndInstallRequest {
        request: "  I vaguely remember it was called 'Paper something'?\nUse the work account.  "
            .into(),
        user_context: vec![RequestContext::Text(
            "Correction: personal workspace, work email.\nKeep both constraints.".into(),
        )],
        caller_observations: vec!["I have no matching tools. I may be wrong.".into()],
    }
}

struct ReferenceOwner {
    allowed: AtomicBool,
    checks: AtomicUsize,
}

impl ReferenceOwner {
    fn new(allowed: bool) -> Self {
        Self {
            allowed: AtomicBool::new(allowed),
            checks: AtomicUsize::new(0),
        }
    }
}

impl ContextReferenceOwner for ReferenceOwner {
    fn verify_access(
        &self,
        reference: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.allowed.load(Ordering::SeqCst)
            && reference.owner == id("context-owner")
            && reference.id == id("context-a")
            && reference.revision == rev("context-2")
            && reference.content_digest == ContentDigest::of_bytes(b"exact supplied context")
            && authority.association().candidate().requester == principal("person-a")
        {
            Ok(())
        } else {
            Err(RequestError::ContextUnavailable)
        }
    }
}

#[test]
fn vague_request_and_corrections_survive_byte_for_byte_without_selected_roles() {
    let f = Fixture::new();
    let original = request();
    let validated = validate_request(
        original.clone(),
        f.host(),
        &f.authority("person-a"),
        &f.context(200),
        &ReferenceOwner::new(false),
    )
    .unwrap();
    assert_eq!(validated.request(), &original);
    assert_eq!(
        validated.evidence().association().candidate().requester,
        principal("person-a")
    );
    assert!(validated.constraints().ceiling.services.is_empty());
    assert!(validated.constraints().ceiling.frontends.is_empty());
    assert_eq!(validated.host().continuation.task, id("business-task-1"));
    assert!(!format!("{validated:?}").contains("Paper"));
    assert!(!format!("{original:?}").contains("personal workspace"));
}

#[test]
fn model_input_cannot_supply_host_authority_routing_or_a_package_choice_field() {
    let value = serde_json::to_value(request()).unwrap();
    for field in [
        "tenant",
        "authority",
        "target",
        "correlation",
        "continuation",
        "package",
    ] {
        let mut forged = value.clone();
        forged[field] = serde_json::json!("attacker-selected");
        assert!(serde_json::from_value::<FindAndInstallRequest>(forged).is_err());
    }
    let mut forged = value;
    forged["user_context"] = serde_json::json!([{
        "owner": "context-owner", "id": "context-a", "revision": "context-2",
        "content_digest": ContentDigest::of_bytes(b"exact supplied context"),
        "authorized": true
    }]);
    assert!(serde_json::from_value::<FindAndInstallRequest>(forged).is_err());
}

#[test]
fn correlation_is_exact_host_scope_and_changed_context_changes_input_digest() {
    let f = Fixture::new();
    let owner = ReferenceOwner::new(false);
    let authority = f.authority("person-a");
    let validate = |request, host| {
        validate_request(request, host, &authority, &f.context(200), &owner).unwrap()
    };
    let first = validate(request(), f.host());
    let replay = validate(request(), f.host());
    assert_eq!(first.input_digest(), replay.input_digest());
    assert_eq!(first.correlation_digest(), replay.correlation_digest());

    let mut changed = request();
    changed.user_context.reverse();
    changed.request.push(' ');
    let changed = validate(changed, f.host());
    assert_eq!(first.correlation_digest(), changed.correlation_digest());
    assert_ne!(first.input_digest(), changed.input_digest());

    let mut rerouted = f.host();
    rerouted.continuation.task = id("another-task");
    let rerouted = validate(request(), rerouted);
    assert_eq!(first.correlation_digest(), rerouted.correlation_digest());
    assert_ne!(first.input_digest(), rerouted.input_digest());

    let mut different_parent = f.host();
    different_parent.parent_operation = Some(id("different-parent"));
    let changed_parent = validate(request(), different_parent);
    assert_eq!(
        first.correlation_digest(),
        changed_parent.correlation_digest()
    );
    assert_ne!(first.input_digest(), changed_parent.input_digest());

    let mut foreign_host = f.host();
    foreign_host.continuation.audience = [id("person-b")].into();
    let foreign_evidence = f.authority("person-b");
    let foreign_native = native_fixture::current(foreign_evidence.association());
    let foreign = validate_request(
        request(),
        foreign_host,
        &foreign_evidence,
        &SetupUseContext {
            authorization: &foreign_native,
            ..f.context(200)
        },
        &owner,
    )
    .unwrap();
    assert_eq!(first.host().correlation, foreign.host().correlation);
    assert_ne!(first.correlation_digest(), foreign.correlation_digest());
    assert_ne!(first.input_digest(), foreign.input_digest());
}

#[test]
fn context_reference_requires_exact_owner_access_and_is_rechecked_after_revocation() {
    let f = Fixture::new();
    let authority = f.authority("person-a");
    let owner = ReferenceOwner::new(true);
    let mut input = request();
    input
        .user_context
        .push(RequestContext::Reference(ContextReference {
            owner: id("context-owner"),
            id: id("context-a"),
            revision: rev("context-2"),
            content_digest: ContentDigest::of_bytes(b"exact supplied context"),
        }));
    let validated =
        validate_request(input.clone(), f.host(), &authority, &f.context(200), &owner).unwrap();
    assert_eq!(owner.checks.load(Ordering::SeqCst), 1);
    if let RequestContext::Reference(reference) = &mut input.user_context[1] {
        reference.content_digest = ContentDigest::of_bytes(b"changed supplied context");
    }
    assert!(matches!(
        validate_request(input, f.host(), &authority, &f.context(200), &owner),
        Err(RequestError::ContextUnavailable)
    ));
    owner.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        validated.revalidate(&authority, &f.context(201), &owner),
        Err(RequestError::ContextUnavailable)
    ));
}

#[test]
fn cached_request_cannot_outlive_deadline_policy_receiver_or_original_authority() {
    let f = Fixture::new();
    let authority = f.authority("person-a");
    let owner = ReferenceOwner::new(false);
    let validated =
        validate_request(request(), f.host(), &authority, &f.context(200), &owner).unwrap();
    assert!(matches!(
        validated.revalidate(&authority, &f.context(900), &owner),
        Err(RequestError::InvalidLimits)
    ));
    assert!(matches!(
        validated.revalidate(&f.authority("person-b"), &f.context(200), &owner),
        Err(RequestError::ChangedAuthority)
    ));
    let mut changed = Fixture::new();
    changed.policy = rev("policy-2");
    assert!(matches!(
        validated.revalidate(&authority, &changed.context(200), &owner),
        Err(RequestError::Authority(SetupContextError::StalePolicy))
    ));
    changed.policy = f.policy.clone();
    changed.destination = dest("another-agent");
    assert!(matches!(
        validated.revalidate(&authority, &changed.context(200), &owner),
        Err(RequestError::Authority(SetupContextError::WrongContext))
    ));
}

#[test]
fn oversized_input_is_rejected_without_truncation_or_effect_authorization() {
    let f = Fixture::new();
    let authority = f.authority("person-a");
    let owner = ReferenceOwner::new(false);
    let mut input = request();
    input.request = "x".repeat(1024 * 1024 + 1);
    assert!(matches!(
        validate_request(input, f.host(), &authority, &f.context(200), &owner),
        Err(RequestError::InputLimit)
    ));
    let mut host = f.host();
    host.limits.deadline_ms = 1001;
    assert!(matches!(
        validate_request(request(), host, &authority, &f.context(200), &owner),
        Err(RequestError::InvalidLimits)
    ));
    let mut host = f.host();
    host.target = id("not-authorized");
    assert!(matches!(
        validate_request(request(), host, &authority, &f.context(200), &owner),
        Err(RequestError::Authority(SetupContextError::Denied))
    ));
}

fn replace_inline_field(input: &mut FindAndInstallRequest, field: usize, value: String) {
    match field {
        0 => input.request = value,
        1 => input.user_context = vec![RequestContext::Text(value)],
        2 => input.caller_observations = vec![value],
        _ => panic!("unknown inline field"),
    }
}

#[test]
fn each_oversized_inline_field_is_rejected_before_reference_access() {
    let f = Fixture::new();
    let authority = f.authority("person-a");
    let owner = ReferenceOwner::new(true);
    for field in 0..3 {
        let mut input = request();
        // Far beyond the accepted size, but still one item. Spaces also verify
        // that oversized empty requests do not reach the blank-content check.
        replace_inline_field(&mut input, field, " ".repeat(16 * 1024 * 1024));
        input
            .user_context
            .push(RequestContext::Reference(ContextReference {
                owner: id("context-owner"),
                id: id("context-a"),
                revision: rev("context-2"),
                content_digest: ContentDigest::of_bytes(b"exact supplied context"),
            }));
        assert!(matches!(
            validate_request(input, f.host(), &authority, &f.context(200), &owner),
            Err(RequestError::InputLimit)
        ));
    }
    assert_eq!(owner.checks.load(Ordering::SeqCst), 0);
}

#[test]
fn aggregate_inline_bytes_are_limited_even_when_each_field_fits() {
    let f = Fixture::new();
    let mut input = request();
    for field in 0..3 {
        replace_inline_field(&mut input, field, "x".repeat(400 * 1024));
    }
    assert!(matches!(
        validate_request(
            input,
            f.host(),
            &f.authority("person-a"),
            &f.context(200),
            &ReferenceOwner::new(false),
        ),
        Err(RequestError::InputLimit)
    ));
}

#[test]
fn exact_encoded_limit_preserves_strings_and_rejects_one_more_byte() {
    const LIMIT: usize = 1024 * 1024;
    let f = Fixture::new();
    let authority = f.authority("person-a");
    let owner = ReferenceOwner::new(false);
    for field in 0..3 {
        // Exercise plain bytes, multi-byte UTF-8, and JSON escape expansion in
        // each inline location. Canonical key ordering cannot change the size.
        for unit in ["a", "é🐾", "\"\\\0\n"] {
            let mut input = request();
            replace_inline_field(&mut input, field, String::new());
            let available = LIMIT - serde_json::to_vec(&input).unwrap().len();
            let encoded_unit = serde_json::to_vec(unit).unwrap().len() - 2;
            let mut value = unit.repeat(available / encoded_unit);
            value.push_str(&"x".repeat(available % encoded_unit));
            replace_inline_field(&mut input, field, value.clone());
            assert_eq!(serde_json::to_vec(&input).unwrap().len(), LIMIT);
            let validated =
                validate_request(input.clone(), f.host(), &authority, &f.context(200), &owner)
                    .unwrap();
            assert_eq!(validated.request(), &input);
            value.push('x');
            replace_inline_field(&mut input, field, value);
            assert_eq!(serde_json::to_vec(&input).unwrap().len(), LIMIT + 1);
            assert!(matches!(
                validate_request(input, f.host(), &authority, &f.context(200), &owner),
                Err(RequestError::InputLimit)
            ));
        }
    }
}

#[test]
fn continuation_selects_a_nonempty_audience_within_the_original_ceiling() {
    let f = Fixture::new();
    let authority = f.authority("person-a");
    for audience in [
        std::collections::BTreeSet::new(),
        [id("person-b")].into(),
        [id("person-a"), id("person-b")].into(),
    ] {
        let mut host = f.host();
        host.continuation.audience = audience;
        assert!(matches!(
            validate_request(
                request(),
                host,
                &authority,
                &f.context(200),
                &ReferenceOwner::new(false)
            ),
            Err(RequestError::InvalidAudience)
        ));
    }
    let result = validate_request(
        request(),
        f.host(),
        &authority,
        &f.context(200),
        &ReferenceOwner::new(false),
    )
    .unwrap();
    assert_eq!(result.host().continuation.audience, [id("person-a")].into());
}

#[test]
fn broad_native_identity_survives_and_same_id_other_kind_changes_exact_payload() {
    let f = Fixture::new();
    let base = f.authority("person-a");
    let mut association = base.association().candidate().clone();
    let broad = format!("person:{}", "λ".repeat(180));
    association.requester = principal(&broad);
    association.original_work = native_fixture::original(&format!("origin:{}", "δ".repeat(100)));
    association.contributing_work = vec![association.original_work.clone()];
    assert!(Id::new(&broad).is_err());
    let evidence = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(association.clone()).unwrap(),
        base.constraints().clone(),
    )
    .unwrap();
    let native = native_fixture::current(evidence.association());
    let first = validate_request(
        request(),
        f.host(),
        &evidence,
        &SetupUseContext {
            authorization: &native,
            ..f.context(200)
        },
        &ReferenceOwner::new(false),
    )
    .unwrap();
    assert_eq!(
        first.evidence().association().candidate().requester,
        association.requester
    );
    assert_eq!(first.original_work(), &association.original_work);
    association.requester.kind = PrincipalKind::ServiceAccount;
    let other = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(association).unwrap(),
        base.constraints().clone(),
    )
    .unwrap();
    let other_native = native_fixture::current(other.association());
    let second = validate_request(
        request(),
        f.host(),
        &other,
        &SetupUseContext {
            authorization: &other_native,
            ..f.context(200)
        },
        &ReferenceOwner::new(false),
    )
    .unwrap();
    assert_eq!(first.host().correlation, second.host().correlation);
    assert_ne!(first.correlation_digest(), second.correlation_digest());
    assert_ne!(first.input_digest(), second.input_digest());
    assert!(matches!(
        first.revalidate(&other, &f.context(200), &ReferenceOwner::new(false)),
        Err(RequestError::ChangedAuthority)
    ));
}
