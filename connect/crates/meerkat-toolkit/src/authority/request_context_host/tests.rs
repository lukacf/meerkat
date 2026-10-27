use super::*;
use crate::authority::{
    Action, ActionPermission, Ceiling, RequestAuthorityClaims, RequestAuthorityUseContext,
    RequestDelegationProposal,
};
use hmac::Mac;
use meerkat_core::auth::{GrantScope, PrincipalId, PrincipalKind, PrincipalRef};
use meerkat_core::request_context::{
    RequestContextId, RequestContextPartitionId, RequestContextRevision, RequestOriginId,
};
use serde_json::Value;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use toolkit_package::{Revision, canonical_json};

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn destination(receiver: &str) -> RequestContextDestination {
    RequestContextDestination {
        realm_id: RealmId::parse("core-realm").unwrap(),
        receiver: PrincipalRef::new(PrincipalKind::SharedAgent, receiver).unwrap(),
        scope: GrantScope::application("work-owner", "exact-work").unwrap(),
    }
}
fn claims() -> RequestAuthorityClaims {
    RequestAuthorityClaims {
        issuer: PrincipalId::new("native-issuer").unwrap(),
        record_id: RequestContextId::new("ingress/original").unwrap(),
        revision: RequestContextRevision::new(1).unwrap(),
        tenant: id("native-tenant"),
        deployment: id("native-deployment"),
        requester: PrincipalRef::new(PrincipalKind::Human, "person:original").unwrap(),
        origin: RequestOriginId::new("ingress/origin:original").unwrap(),
        destination: RequestAuthorityDestination::new(destination("agent:parent")).unwrap(),
        partition: RequestContextPartitionId::new("ingress/partition:original").unwrap(),
        parent: None,
        policy_revision: Revision::new("policy-1").unwrap(),
        issued_at_ms: 100,
        expires_at_ms: 1000,
        ceiling: Ceiling {
            actions: [ActionPermission {
                action: Action::Delegate,
                resource: id("child-work"),
            }]
            .into(),
            services: Default::default(),
            frontends: Default::default(),
            output_audiences: [id("original-person")].into(),
        },
        hops: 0,
        max_hops: 3,
    }
}
fn issuer() -> Arc<AuthorityIssuer> {
    Arc::new(AuthorityIssuer::new(id("native-issuer"), [47; 32]))
}
fn child(authority: &AuthorityIssuer, root: &SealedRequestAuthority) -> SealedRequestAuthority {
    let native = authority
        .verify_request_integrity(root, &root.reference().unwrap())
        .unwrap();
    let parent_destination = native.claims().destination.to_core();
    authority
        .delegate_request(
            &native,
            &RequestAuthorityUseContext {
                tenant: &native.claims().tenant,
                deployment: &native.claims().deployment,
                destination: &parent_destination,
                policy_revision: &native.claims().policy_revision,
                now_ms: 200,
            },
            RequestDelegationProposal {
                record_id: RequestContextId::new("work-owner/child").unwrap(),
                revision: RequestContextRevision::new(1).unwrap(),
                destination: destination("agent:child"),
                partition: RequestContextPartitionId::new("work-owner/partition:child").unwrap(),
                delegation_resource: id("child-work"),
                ceiling: native.claims().ceiling.clone(),
                expires_at_ms: 900,
            },
        )
        .unwrap()
}

// Test double only. These preloaded values are not a store, a transaction or
// evidence of durable/atomic publication. Faults deliberately violate the port
// contract so the adapter's acceptance boundary is independently exercised.
struct Owner {
    issuer: Arc<AuthorityIssuer>,
    records: Mutex<Vec<(RequestContextRef, SealedRequestAuthority)>>,
    reads: Mutex<Vec<RequestContextRef>>,
    publications: Mutex<Vec<(RequestContextRef, RequestContextDestination)>>,
    read_error: Mutex<Option<RequestAuthorityOwnerError>>,
    publication: Mutex<Result<RequestContextRef, RequestAuthorityOwnerError>>,
    now_ms: AtomicU64,
    operation_allowed: AtomicBool,
}

impl Owner {
    fn record(&self, reference: &RequestContextRef) -> Option<SealedRequestAuthority> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key == reference)
            .map(|(_, sealed)| sealed.clone())
    }
    fn return_child(&self, child: SealedRequestAuthority) {
        let reference = child.reference().unwrap();
        self.records
            .lock()
            .unwrap()
            .push((reference.clone(), child));
        *self.publication.lock().unwrap() = Ok(reference);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ApplicationRequestAuthorityOwner for Owner {
    async fn load_exact(
        &self,
        reference: &RequestContextRef,
    ) -> Result<SealedRequestAuthority, RequestAuthorityOwnerError> {
        self.reads.lock().unwrap().push(reference.clone());
        if let Some(error) = *self.read_error.lock().unwrap() {
            return Err(error);
        }
        self.record(reference)
            .ok_or(RequestAuthorityOwnerError::Unavailable)
    }

    async fn publish_narrower(
        &self,
        parent: &RequestContextRef,
        destination: &RequestContextDestination,
    ) -> Result<RequestContextRef, RequestAuthorityOwnerError> {
        self.publications
            .lock()
            .unwrap()
            .push((parent.clone(), destination.clone()));
        if !self.operation_allowed.load(Ordering::SeqCst) {
            return Err(RequestAuthorityOwnerError::DelegationDenied);
        }
        let sealed = self
            .record(parent)
            .ok_or(RequestAuthorityOwnerError::Unavailable)?;
        let native = self
            .issuer
            .verify_request_integrity(&sealed, parent)
            .map_err(|_| RequestAuthorityOwnerError::InvalidRecord)?;
        let parent_destination = native.claims().destination.to_core();
        native
            .require(
                Action::Delegate,
                &id("child-work"),
                &RequestAuthorityUseContext {
                    tenant: &id("native-tenant"),
                    deployment: &id("native-deployment"),
                    destination: &parent_destination,
                    policy_revision: &Revision::new("policy-1").unwrap(),
                    now_ms: self.now_ms.load(Ordering::SeqCst),
                },
            )
            .map_err(|_| RequestAuthorityOwnerError::DelegationDenied)?;
        self.publication.lock().unwrap().clone()
    }

    // This original publication fixture has no authenticated ingress/work view.
    fn authenticated_request(
        &self,
    ) -> Result<(&RequestContextRef, &RequestContextDestination), RequestContextUseError> {
        Err(RequestContextUseError::Unavailable)
    }

    fn with_current_use<'a>(
        &'a self,
        _context: &'a VerifiedRequestAuthority,
        _operation: RequestContextUse,
        _action: RequestContextUseCallback<'a>,
    ) -> RequestContextUseFuture<'a> {
        Box::pin(async { Err(RequestContextUseError::Unavailable) })
    }
}

struct Fixture {
    root: SealedRequestAuthority,
    child: SealedRequestAuthority,
    owner: Arc<Owner>,
    host: ToolkitRequestContextHost,
}
impl Fixture {
    fn new() -> Self {
        let issuer = issuer();
        let root = issuer.issue_request_root(claims()).unwrap();
        let child = child(&issuer, &root);
        let owner = Arc::new(Owner {
            issuer: issuer.clone(),
            records: Mutex::new(vec![
                (root.reference().unwrap(), root.clone()),
                (child.reference().unwrap(), child.clone()),
            ]),
            reads: Mutex::new(Vec::new()),
            publications: Mutex::new(Vec::new()),
            read_error: Mutex::new(None),
            publication: Mutex::new(Ok(child.reference().unwrap())),
            now_ms: AtomicU64::new(200),
            operation_allowed: AtomicBool::new(true),
        });
        let host = ToolkitRequestContextHost::new(
            owner.clone(),
            issuer,
            id("native-tenant"),
            id("native-deployment"),
            RealmId::parse("core-realm").unwrap(),
        );
        Self {
            root,
            child,
            owner,
            host,
        }
    }
    async fn parent(&self) -> VerifiedRequestContext {
        VerifiedRequestContext::verify(
            &self.host,
            &self.root.reference().unwrap(),
            &destination("agent:parent"),
        )
        .await
        .unwrap()
    }
}

// A different configured core host may produce a valid core projection. The
// Toolkit host must reopen its own native evidence, not trust that provenance.
struct OtherCoreHost(RequestContextFacts);
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RequestContextHost for OtherCoreHost {
    async fn verify_reference(
        &self,
        _reference: &RequestContextRef,
        _destination: &RequestContextDestination,
    ) -> Result<RequestContextFacts, RequestContextError> {
        Ok(self.0.clone())
    }
    async fn narrow_for(
        &self,
        _parent: &VerifiedRequestContext,
        _destination: &RequestContextDestination,
    ) -> Result<RequestContextRef, RequestContextError> {
        Err(RequestContextError::DelegationDenied)
    }
}

// Fault injection: authenticate a malformed owner result with the fixture key.
// This signing code exists only in tests, to distinguish native authenticity
// from correct child lineage. Normal fixture children use delegate_request.
fn signed_fault(
    sealed: &SealedRequestAuthority,
    pointer: &str,
    replacement: Value,
) -> SealedRequestAuthority {
    let mut value = serde_json::to_value(sealed).unwrap();
    *value.pointer_mut(pointer).unwrap() = replacement;
    let bytes = canonical_json(&value["claims"]).unwrap();
    let mut mac = issuer().mac().unwrap();
    for field in [
        b"meerkat-toolkit.request-authority.v2".as_slice(),
        &2_u16.to_be_bytes(),
        bytes.as_slice(),
    ] {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field);
    }
    value["signature"] = serde_json::json!(hex::encode(mac.finalize().into_bytes()));
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn exact_reference_and_destination_are_forwarded_and_core_child_is_verified() {
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    assert_eq!(
        fixture.owner.reads.lock().unwrap().as_slice(),
        &[fixture.root.reference().unwrap()]
    );
    let result = parent
        .narrow_for(&fixture.host, &destination("agent:child"))
        .await
        .unwrap();
    assert_eq!(result.reference(), &fixture.child.reference().unwrap());
    assert_eq!(result.destination(), &destination("agent:child"));
    assert_eq!(result.facts().requester, parent.facts().requester);
    assert_eq!(result.facts().origin, parent.facts().origin);
    assert_eq!(result.facts().parent.as_ref(), Some(parent.reference()));
    assert_ne!(result.facts().partition, parent.facts().partition);
    assert_eq!(
        fixture.owner.publications.lock().unwrap().as_slice(),
        &[(parent.reference().clone(), destination("agent:child"))]
    );
    // Original verify, parent reopen, child acceptance, core wrapper reverify.
    assert_eq!(
        fixture.owner.reads.lock().unwrap().as_slice(),
        &[
            parent.reference().clone(),
            parent.reference().clone(),
            result.reference().clone(),
            result.reference().clone(),
        ]
    );
}

#[tokio::test]
async fn exact_tenant_deployment_and_realm_bindings_are_independent() {
    for (tenant, deployment, realm) in [
        ("other-tenant", "native-deployment", "core-realm"),
        ("native-tenant", "other-deployment", "core-realm"),
        ("native-tenant", "native-deployment", "native-tenant"),
    ] {
        let fixture = Fixture::new();
        let host = ToolkitRequestContextHost::new(
            fixture.owner.clone(),
            issuer(),
            id(tenant),
            id(deployment),
            RealmId::parse(realm).unwrap(),
        );
        assert_eq!(
            VerifiedRequestContext::verify(
                &host,
                &fixture.root.reference().unwrap(),
                &destination("agent:parent")
            )
            .await
            .unwrap_err(),
            RequestContextError::WrongDestination
        );
        assert!(fixture.owner.publications.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn receiver_kind_scope_and_realm_must_match_and_malformed_destination_is_rejected() {
    let mut wrong_kind = destination("agent:parent");
    wrong_kind.receiver.kind = PrincipalKind::RuntimeHost;
    let mut wrong_scope = destination("agent:parent");
    wrong_scope.scope = GrantScope::application("work-owner", "another-work").unwrap();
    let mut wrong_realm = destination("agent:parent");
    wrong_realm.realm_id = RealmId::parse("another-realm").unwrap();
    for expected in [
        destination("agent:other"),
        wrong_kind,
        wrong_scope,
        wrong_realm,
    ] {
        let fixture = Fixture::new();
        assert_eq!(
            fixture
                .host
                .verify_reference(&fixture.root.reference().unwrap(), &expected)
                .await
                .unwrap_err(),
            RequestContextError::WrongDestination
        );
    }
    let fixture = Fixture::new();
    let mut malformed = destination("agent:parent");
    malformed.scope = GrantScope::Application {
        namespace: String::new(),
        id: "work".into(),
    };
    assert_eq!(
        fixture
            .host
            .verify_reference(&fixture.root.reference().unwrap(), &malformed)
            .await
            .unwrap_err(),
        RequestContextError::MalformedDestination
    );
    assert!(fixture.owner.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn missing_mismatched_or_unavailable_canonical_seal_is_never_reconstructed() {
    let fixture = Fixture::new();
    let reference = fixture.root.reference().unwrap();
    fixture.owner.records.lock().unwrap().clear();
    assert_eq!(
        fixture
            .host
            .verify_reference(&reference, &destination("agent:parent"))
            .await
            .unwrap_err(),
        RequestContextError::Unavailable
    );
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .push((reference.clone(), fixture.child.clone()));
    assert_eq!(
        fixture
            .host
            .verify_reference(&reference, &destination("agent:parent"))
            .await
            .unwrap_err(),
        RequestContextError::IntegrityMismatch
    );
    *fixture.owner.read_error.lock().unwrap() = Some(RequestAuthorityOwnerError::Unavailable);
    assert_eq!(
        fixture
            .host
            .verify_reference(&reference, &destination("agent:parent"))
            .await
            .unwrap_err(),
        RequestContextError::Unavailable
    );
    *fixture.owner.read_error.lock().unwrap() = Some(RequestAuthorityOwnerError::InvalidRecord);
    assert_eq!(
        fixture
            .host
            .verify_reference(&reference, &destination("agent:parent"))
            .await
            .unwrap_err(),
        RequestContextError::IntegrityMismatch
    );
    assert!(fixture.owner.publications.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reference_digest_cannot_replace_native_signature_or_issuer() {
    for authority in [
        AuthorityIssuer::new(id("native-issuer"), [99; 32]),
        AuthorityIssuer::new(id("another-issuer"), [47; 32]),
    ] {
        let fixture = Fixture::new();
        let mut claims = claims();
        claims.issuer = PrincipalId::new(authority.id.as_str()).unwrap();
        let foreign = authority.issue_request_root(claims).unwrap();
        let reference = foreign.reference().unwrap();
        fixture
            .owner
            .records
            .lock()
            .unwrap()
            .push((reference.clone(), foreign));
        assert_eq!(
            fixture
                .host
                .verify_reference(&reference, &destination("agent:parent"))
                .await
                .unwrap_err(),
            RequestContextError::IntegrityMismatch
        );
    }
}

#[tokio::test]
async fn substituted_core_facts_are_rejected_before_publication() {
    for field in ["requester", "origin", "partition", "parent"] {
        let fixture = Fixture::new();
        let parent = fixture.parent().await;
        let mut facts = parent.facts().clone();
        match field {
            "requester" => {
                facts.requester =
                    PrincipalRef::new(PrincipalKind::Human, "substituted-person").unwrap()
            }
            "origin" => facts.origin = RequestOriginId::new("substituted-origin").unwrap(),
            "partition" => {
                facts.partition = RequestContextPartitionId::new("substituted-partition").unwrap()
            }
            "parent" => facts.parent = Some(fixture.child.reference().unwrap()),
            _ => unreachable!(),
        }
        let other = VerifiedRequestContext::verify(
            &OtherCoreHost(facts),
            parent.reference(),
            parent.destination(),
        )
        .await
        .unwrap();
        assert_eq!(
            other
                .narrow_for(&fixture.host, &destination("agent:child"))
                .await
                .unwrap_err(),
            RequestContextError::IntegrityMismatch,
            "{field}"
        );
        assert!(fixture.owner.publications.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn parent_is_reopened_after_verification_and_foreign_destination_is_not_a_permit() {
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .retain(|(key, _)| key != parent.reference());
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::Unavailable
    );
    assert!(fixture.owner.publications.lock().unwrap().is_empty());

    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    let other = VerifiedRequestContext::verify(
        &OtherCoreHost(parent.facts().clone()),
        parent.reference(),
        &destination("another-parent"),
    )
    .await
    .unwrap();
    assert_eq!(
        other
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::WrongDestination
    );
    let mut foreign_child = destination("agent:child");
    foreign_child.realm_id = RealmId::parse("another-realm").unwrap();
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &foreign_child)
            .await
            .unwrap_err(),
        RequestContextError::WrongDestination
    );
    assert!(fixture.owner.publications.lock().unwrap().is_empty());
}

#[tokio::test]
async fn core_verified_parent_from_a_different_native_key_cannot_publish() {
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    let foreign = AuthorityIssuer::new(id("native-issuer"), [99; 32])
        .issue_request_root(claims())
        .unwrap();
    let reference = foreign.reference().unwrap();
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .push((reference.clone(), foreign));
    let other = VerifiedRequestContext::verify(
        &OtherCoreHost(parent.facts().clone()),
        &reference,
        parent.destination(),
    )
    .await
    .unwrap();
    assert_eq!(
        other
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::IntegrityMismatch
    );
    assert!(fixture.owner.publications.lock().unwrap().is_empty());
}

#[tokio::test]
async fn publication_errors_propagate_without_returning_a_child() {
    for (owner_error, expected) in [
        (
            RequestAuthorityOwnerError::Unavailable,
            RequestContextError::Unavailable,
        ),
        (
            RequestAuthorityOwnerError::DelegationDenied,
            RequestContextError::DelegationDenied,
        ),
        (
            RequestAuthorityOwnerError::InvalidRecord,
            RequestContextError::IntegrityMismatch,
        ),
    ] {
        let fixture = Fixture::new();
        let parent = fixture.parent().await;
        *fixture.owner.publication.lock().unwrap() = Err(owner_error);
        assert_eq!(
            parent
                .narrow_for(&fixture.host, &destination("agent:child"))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(fixture.owner.publications.lock().unwrap().len(), 1);
        assert_eq!(fixture.owner.reads.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn publication_return_requires_exact_available_child_seal_and_destination() {
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .retain(|(key, _)| key == parent.reference());
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::Unavailable
    );

    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    *fixture.owner.publication.lock().unwrap() = Ok(parent.reference().clone());
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::WrongDestination
    );
    assert_eq!(
        parent
            .narrow_for(&fixture.host, parent.destination())
            .await
            .unwrap_err(),
        RequestContextError::InvalidLineage
    );

    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    let child_reference = fixture.child.reference().unwrap();
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .retain(|(key, _)| key != &child_reference);
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .push((child_reference, fixture.root.clone()));
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::IntegrityMismatch
    );
}

#[tokio::test]
async fn returned_authentic_child_must_bind_the_same_native_host_and_exact_destination() {
    for (pointer, replacement) in [
        ("/claims/tenant", serde_json::json!("other-tenant")),
        ("/claims/deployment", serde_json::json!("other-deployment")),
        (
            "/claims/destination/realm_id",
            serde_json::json!("other-realm"),
        ),
        (
            "/claims/destination/receiver/id",
            serde_json::json!("other-agent"),
        ),
        (
            "/claims/destination/receiver/kind",
            serde_json::json!("runtime_host"),
        ),
        (
            "/claims/destination/scope/id",
            serde_json::json!("other-work"),
        ),
    ] {
        let fixture = Fixture::new();
        let parent = fixture.parent().await;
        fixture
            .owner
            .return_child(signed_fault(&fixture.child, pointer, replacement));
        assert_eq!(
            parent
                .narrow_for(&fixture.host, &destination("agent:child"))
                .await
                .unwrap_err(),
            RequestContextError::WrongDestination,
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn authentic_child_must_preserve_requester_origin_exact_parent_and_native_bounds() {
    for (pointer, replacement) in [
        ("/claims/requester/id", serde_json::json!("other-person")),
        (
            "/claims/requester/kind",
            serde_json::json!("service_account"),
        ),
        ("/claims/origin", serde_json::json!("other-origin")),
        ("/claims/policy_revision", serde_json::json!("policy-2")),
        ("/claims/hops", serde_json::json!(2)),
        ("/claims/max_hops", serde_json::json!(4)),
        ("/claims/issued_at_ms", serde_json::json!(99)),
        ("/claims/expires_at_ms", serde_json::json!(1001)),
        (
            "/claims/ceiling/output_audiences",
            serde_json::json!(["another-person"]),
        ),
    ] {
        let fixture = Fixture::new();
        let parent = fixture.parent().await;
        fixture
            .owner
            .return_child(signed_fault(&fixture.child, pointer, replacement));
        assert_eq!(
            parent
                .narrow_for(&fixture.host, &destination("agent:child"))
                .await
                .unwrap_err(),
            RequestContextError::InvalidLineage,
            "{pointer}"
        );
    }
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    let mut other_claims = claims();
    other_claims.record_id = RequestContextId::new("ingress/other").unwrap();
    let other_parent = issuer().issue_request_root(other_claims).unwrap();
    fixture.owner.return_child(signed_fault(
        &fixture.child,
        "/claims/parent",
        serde_json::to_value(other_parent.reference().unwrap()).unwrap(),
    ));
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::InvalidLineage
    );
}

#[tokio::test]
async fn expired_authentic_read_succeeds_but_current_owner_denies_publication() {
    let fixture = Fixture::new();
    fixture.owner.now_ms.store(1000, Ordering::SeqCst);
    let parent = fixture.parent().await;
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::DelegationDenied
    );
    assert_eq!(fixture.owner.publications.lock().unwrap().len(), 1);
    assert_eq!(fixture.owner.reads.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn current_operation_revocation_overrides_unchanged_authentic_policy_revision() {
    let fixture = Fixture::new();
    let parent = fixture.parent().await;
    fixture
        .owner
        .operation_allowed
        .store(false, Ordering::SeqCst);
    assert_eq!(
        parent
            .narrow_for(&fixture.host, &destination("agent:child"))
            .await
            .unwrap_err(),
        RequestContextError::DelegationDenied
    );
    fixture
        .host
        .verify_reference(parent.reference(), parent.destination())
        .await
        .unwrap();
    assert_eq!(fixture.owner.publications.lock().unwrap().len(), 1);
}

#[test]
fn owner_diagnostics_are_closed_and_redacted() {
    for error in [
        RequestAuthorityOwnerError::Unavailable,
        RequestAuthorityOwnerError::DelegationDenied,
        RequestAuthorityOwnerError::InvalidRecord,
    ] {
        let diagnostic = format!("{error:?}: {error}");
        for private in [
            "native-tenant",
            "native-deployment",
            "person:original",
            "ingress/original",
        ] {
            assert!(!diagnostic.contains(private));
        }
    }
}

#[path = "current_use_tests.rs"]
mod current_use_tests;
