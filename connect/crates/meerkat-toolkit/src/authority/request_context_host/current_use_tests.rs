#![allow(clippy::unwrap_used)]

use super::*;
use meerkat_core::SessionId;
use meerkat_core::session_context_binding::{SessionContextBinding, SessionContextPartition};
use std::sync::atomic::AtomicUsize;

// An explicit fixture view of the SAME application authority owner. It shares
// original native records but fixes one separately selected request/destination
// and one exact resource. This is not real paired host ingress or durable custody.
struct CurrentOwner {
    inner: Arc<Owner>,
    reference: RequestContextRef,
    destination: RequestContextDestination,
    session_id: SessionId,
    revoked: AtomicBool,
    cancelled: AtomicBool,
    work_fence: AtomicU64,
    uses: AtomicUsize,
}
impl CurrentOwner {
    fn new(fixture: &Fixture, sealed: &SealedRequestAuthority) -> Arc<Self> {
        Arc::new(Self {
            inner: fixture.owner.clone(),
            reference: sealed.reference().unwrap(),
            destination: fixture
                .owner
                .issuer
                .verify_request_integrity(sealed, &sealed.reference().unwrap())
                .unwrap()
                .claims()
                .destination
                .to_core(),
            session_id: SessionId::new(),
            revoked: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            work_fence: AtomicU64::new(1),
            uses: AtomicUsize::new(0),
        })
    }
    fn check_current(
        &self,
        context: &VerifiedRequestAuthority,
    ) -> Result<(), RequestContextUseError> {
        if context
            .reference()
            .map_err(|_| RequestContextUseError::WrongContext)?
            != self.reference
            || context.claims().destination.to_core() != self.destination
        {
            return Err(RequestContextUseError::WrongContext);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(RequestContextUseError::Cancelled);
        }
        if self.revoked.load(Ordering::SeqCst) || self.work_fence.load(Ordering::SeqCst) != 1 {
            return Err(RequestContextUseError::Denied);
        }
        context
            .revalidate(&RequestAuthorityUseContext {
                tenant: &id("native-tenant"),
                deployment: &id("native-deployment"),
                destination: &self.destination,
                policy_revision: &Revision::new("policy-1").unwrap(),
                now_ms: self.inner.now_ms.load(Ordering::SeqCst),
            })
            .map_err(|_| RequestContextUseError::Denied)
    }
    fn operation(&self, context: &VerifiedRequestContext) -> RequestContextUse {
        RequestContextUse::ReadSession {
            session_id: self.session_id.clone(),
            binding: SessionContextBinding::Scoped(
                SessionContextPartition::new(
                    context.reference().issuer().clone(),
                    context.destination().realm_id.clone(),
                    context.facts().partition.clone(),
                )
                .unwrap(),
            ),
        }
    }
}
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ApplicationRequestAuthorityOwner for CurrentOwner {
    async fn load_exact(
        &self,
        reference: &RequestContextRef,
    ) -> Result<SealedRequestAuthority, RequestAuthorityOwnerError> {
        self.inner.load_exact(reference).await
    }
    async fn publish_narrower(
        &self,
        _parent: &RequestContextRef,
        _destination: &RequestContextDestination,
    ) -> Result<RequestContextRef, RequestAuthorityOwnerError> {
        Err(RequestAuthorityOwnerError::DelegationDenied)
    }
    fn authenticated_request(
        &self,
    ) -> Result<(&RequestContextRef, &RequestContextDestination), RequestContextUseError> {
        Ok((&self.reference, &self.destination))
    }
    fn with_current_use<'a>(
        &'a self,
        context: &'a VerifiedRequestAuthority,
        operation: RequestContextUse,
        action: RequestContextUseCallback<'a>,
    ) -> RequestContextUseFuture<'a> {
        Box::pin(async move {
            self.uses.fetch_add(1, Ordering::SeqCst);
            self.check_current(context)?;
            let RequestContextUse::ReadSession {
                session_id,
                binding,
            } = operation
            else {
                return Err(RequestContextUseError::Unavailable);
            };
            let expected = SessionContextBinding::Scoped(
                SessionContextPartition::new(
                    context.claims().issuer.clone(),
                    self.destination.realm_id.clone(),
                    context.claims().partition.clone(),
                )
                .map_err(|_| RequestContextUseError::WrongContext)?,
            );
            if session_id != self.session_id || binding != expected {
                return Err(RequestContextUseError::Denied);
            }
            // Exact fixture resource policy, not coercion to an unrelated
            // Toolkit Action. A production owner supplies its real policy.
            action().await?;
            self.check_current(context)
        })
    }
}
fn current_host(fixture: &Fixture, owner: Arc<CurrentOwner>) -> ToolkitRequestContextHost {
    ToolkitRequestContextHost::new(
        owner,
        fixture.owner.issuer.clone(),
        id("native-tenant"),
        id("native-deployment"),
        RealmId::parse("core-realm").unwrap(),
    )
}

#[tokio::test]
async fn global_integrity_owner_cannot_upgrade_a_resolvable_seal_to_current_custody() {
    let fixture = Fixture::new();
    let _verified = fixture.parent().await;
    let reads = fixture.owner.reads.lock().unwrap().len();
    let result = fixture
        .host
        .bind_current_execution(
            &fixture.root.reference().unwrap(),
            &destination("agent:parent"),
        )
        .await;
    assert!(matches!(result, Err(RequestContextUseError::Unavailable)));
    assert_eq!(fixture.owner.reads.lock().unwrap().len(), reads);
}

#[tokio::test]
async fn current_host_checks_exact_authenticated_request_and_destination_before_lookup() {
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    assert!(matches!(
        host.bind_current_execution(
            &fixture.child.reference().unwrap(),
            &destination("agent:child")
        )
        .await,
        Err(RequestContextUseError::WrongContext)
    ));
    assert!(matches!(
        host.bind_current_execution(&owner.reference, &destination("agent:other"))
            .await,
        Err(RequestContextUseError::WrongContext)
    ));
    assert!(fixture.owner.reads.lock().unwrap().is_empty());
    let execution = host
        .bind_current_execution(&owner.reference, &owner.destination)
        .await
        .unwrap();
    assert_eq!(execution.verified().reference(), &owner.reference);
    assert_eq!(
        fixture.owner.reads.lock().unwrap().as_slice(),
        std::slice::from_ref(&owner.reference)
    );
}

#[tokio::test]
async fn current_native_use_reopens_evidence_and_rechecks_expiry_revocation_cancellation_and_fence()
{
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    let execution = host
        .bind_current_execution(&owner.reference, &owner.destination)
        .await
        .unwrap();
    let entries = AtomicUsize::new(0);
    let action = || {
        entries.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(41_u32) })
            as meerkat_core::request_context::RequestContextUseValueFuture<'_, u32>
    };
    assert_eq!(
        execution
            .with_current_use(owner.operation(execution.verified()), Box::new(action))
            .await
            .unwrap(),
        41
    );
    for state in 0..4 {
        owner.revoked.store(state == 0, Ordering::SeqCst);
        owner.cancelled.store(state == 1, Ordering::SeqCst);
        owner
            .work_fence
            .store(if state == 2 { 2 } else { 1 }, Ordering::SeqCst);
        owner
            .inner
            .now_ms
            .store(if state == 3 { 1000 } else { 200 }, Ordering::SeqCst);
        let result = execution
            .with_current_use(owner.operation(execution.verified()), Box::new(action))
            .await;
        assert_eq!(
            result,
            Err(if state == 1 {
                RequestContextUseError::Cancelled
            } else {
                RequestContextUseError::Denied
            })
        );
    }
    assert_eq!(entries.load(Ordering::SeqCst), 1);
    assert_eq!(owner.uses.load(Ordering::SeqCst), 5);
    assert_eq!(
        owner.inner.reads.lock().unwrap().len(),
        6,
        "one bind plus exact reopen on every use"
    );
}

#[tokio::test]
async fn adapter_reopens_own_native_evidence_and_refuses_substituted_core_facts() {
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    let original = fixture.parent().await;
    for field in 0..4 {
        let mut facts = original.facts().clone();
        match field {
            0 => facts.requester = PrincipalRef::new(PrincipalKind::Human, "person:other").unwrap(),
            1 => facts.origin = RequestOriginId::new("origin:other").unwrap(),
            2 => facts.partition = RequestContextPartitionId::new("partition:other").unwrap(),
            _ => facts.parent = Some(fixture.child.reference().unwrap()),
        }
        let substituted = VerifiedRequestContext::verify(
            &OtherCoreHost(facts),
            &owner.reference,
            &owner.destination,
        )
        .await
        .unwrap();
        let execution = RequestExecutionContext::new(
            substituted,
            Arc::new(ToolkitCurrentRequestCustody { host: host.clone() }),
        )
        .unwrap();
        let entered = AtomicBool::new(false);
        let result = execution
            .with_current_use(
                owner.operation(execution.verified()),
                Box::new(|| {
                    entered.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                }),
            )
            .await;
        assert_eq!(result, Err(RequestContextUseError::WrongContext));
        assert!(!entered.load(Ordering::SeqCst));
    }
    assert_eq!(owner.uses.load(Ordering::SeqCst), 0);
}

struct TypedSecret(Arc<AtomicUsize>);
impl Drop for TypedSecret {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn revocation_after_read_discards_typed_value_and_does_not_claim_no_read() {
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    let execution = host
        .bind_current_execution(&owner.reference, &owner.destination)
        .await
        .unwrap();
    let entries = AtomicUsize::new(0);
    let drops = Arc::new(AtomicUsize::new(0));
    let result = execution
        .with_current_use(
            owner.operation(execution.verified()),
            Box::new(|| {
                entries.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    owner.revoked.store(true, Ordering::SeqCst);
                    Ok(TypedSecret(drops.clone()))
                })
            }),
        )
        .await;
    assert!(matches!(result, Err(RequestContextUseError::Denied)));
    assert_eq!(
        entries.load(Ordering::SeqCst),
        1,
        "read entered before later revocation"
    );
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "value never released to caller"
    );
}

#[tokio::test]
async fn same_partition_requests_keep_independent_current_claims_and_unsupported_uses_refuse() {
    let fixture = Fixture::new();
    let mut sibling_claims = claims();
    sibling_claims.record_id = RequestContextId::new("ingress/sibling").unwrap();
    sibling_claims.origin = RequestOriginId::new("ingress/origin:sibling").unwrap();
    let sibling = fixture
        .owner
        .issuer
        .issue_request_root(sibling_claims)
        .unwrap();
    fixture
        .owner
        .records
        .lock()
        .unwrap()
        .push((sibling.reference().unwrap(), sibling.clone()));
    let first = CurrentOwner::new(&fixture, &fixture.root);
    let second = CurrentOwner::new(&fixture, &sibling);
    let first_host = current_host(&fixture, first.clone());
    let second_host = current_host(&fixture, second.clone());
    let first_execution = first_host
        .bind_current_execution(&first.reference, &first.destination)
        .await
        .unwrap();
    let second_execution = second_host
        .bind_current_execution(&second.reference, &second.destination)
        .await
        .unwrap();
    assert_eq!(
        first_execution.verified().facts().partition,
        second_execution.verified().facts().partition
    );
    first.revoked.store(true, Ordering::SeqCst);
    let (left, right) = tokio::join!(
        first_execution.with_current_use(
            first.operation(first_execution.verified()),
            Box::new(|| Box::pin(async { Ok(1) }))
        ),
        second_execution.with_current_use(
            second.operation(second_execution.verified()),
            Box::new(|| Box::pin(async { Ok(2) }))
        ),
    );
    assert_eq!(left, Err(RequestContextUseError::Denied));
    assert_eq!(right, Ok(2));
    assert!(matches!(
        first_host
            .bind_current_execution(&second.reference, &second.destination)
            .await,
        Err(RequestContextUseError::WrongContext)
    ));
    let entered = AtomicBool::new(false);
    assert_eq!(
        second_execution
            .with_current_use(
                RequestContextUse::AdmitTurn {
                    session_id: second.session_id.clone()
                },
                Box::new(|| {
                    entered.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                })
            )
            .await,
        Err(RequestContextUseError::Unavailable)
    );
    assert!(!entered.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unavailable_or_inconsistent_native_record_never_reaches_current_callback() {
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    let execution = host
        .bind_current_execution(&owner.reference, &owner.destination)
        .await
        .unwrap();
    for (failure, expected) in [
        (
            RequestAuthorityOwnerError::Unavailable,
            RequestContextUseError::Unavailable,
        ),
        (
            RequestAuthorityOwnerError::InvalidRecord,
            RequestContextUseError::WrongContext,
        ),
    ] {
        *owner.inner.read_error.lock().unwrap() = Some(failure);
        let entered = AtomicBool::new(false);
        assert_eq!(
            execution
                .with_current_use(
                    owner.operation(execution.verified()),
                    Box::new(|| {
                        entered.store(true, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    })
                )
                .await,
            Err(expected)
        );
        assert!(!entered.load(Ordering::SeqCst));
    }
    assert_eq!(owner.uses.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn current_owner_refuses_wrong_session_or_partition_before_read() {
    let fixture = Fixture::new();
    let owner = CurrentOwner::new(&fixture, &fixture.root);
    let host = current_host(&fixture, owner.clone());
    let execution = host
        .bind_current_execution(&owner.reference, &owner.destination)
        .await
        .unwrap();
    for resource in 0..3 {
        let mut operation = owner.operation(execution.verified());
        if let RequestContextUse::ReadSession {
            session_id,
            binding,
        } = &mut operation
        {
            match resource {
                0 => *session_id = SessionId::new(),
                1 => *binding = SessionContextBinding::Raw,
                _ => {
                    *binding = SessionContextBinding::Scoped(
                        SessionContextPartition::new(
                            execution.verified().reference().issuer().clone(),
                            execution.verified().destination().realm_id.clone(),
                            RequestContextPartitionId::new("private-other-partition").unwrap(),
                        )
                        .unwrap(),
                    )
                }
            }
        }
        let entered = AtomicBool::new(false);
        assert_eq!(
            execution
                .with_current_use(
                    operation,
                    Box::new(|| {
                        entered.store(true, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    })
                )
                .await,
            Err(RequestContextUseError::Denied)
        );
        assert!(!entered.load(Ordering::SeqCst));
    }
}
