//! Construction-only controls. The installed policies refuse every operation;
//! this fixture does not simulate a controller, input, or permission decision.
use super::*;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization::work::{LocalAuthorizationClock, LocalAuthorizationTime};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::OperationAuthorizationError;
use meerkat_core::authorization::PreparedAuthorizationBinding;

struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: 100,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}
struct RefusingPolicies;
impl AdmittedWorkPolicyOwner for RefusingPolicies {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}
impl OperationPolicyOwner for RefusingPolicies {
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}
fn configuration() -> meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration {
    let root = meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::Human,
        "bundle-owner",
        meerkat_core::TrustDomainId::new("bundle-test").unwrap(),
    )
    .unwrap();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root,
                namespace: EvidenceId::new("bundle").unwrap(),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        )
        .unwrap(),
    );
    meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration {
        grants,
        ingress: Arc::new(|_, _, _, _| {
            Err(
                meerkat_runtime::input_authority::NativeAdmissionError::Readiness(
                    meerkat_runtime::traits::ControllerReadinessFailure::PolicyUnavailable,
                ),
            )
        }),
        invocation_owner: Arc::new(RefusingPolicies),
        operation_owner: Arc::new(RefusingPolicies),
    }
}
fn bundle() -> PersistenceBundle {
    PersistenceBundle::new(
        Arc::new(MemoryStore::new()),
        Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
        Arc::new(MemoryBlobStore::new()),
    )
}

#[tokio::test]
async fn memory_bundle_configures_its_existing_persistent_adapter_before_sharing() {
    let original = bundle();
    let expected_store = original.runtime_store();
    let configured = original
        .with_local_grant_authorization(configuration())
        .expect("actual bundle setup");
    assert!(Arc::ptr_eq(&configured.runtime_store(), &expected_store));
    let adapter = configured.runtime_adapter();
    let session = meerkat_core::SessionId::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        adapter.register_session(session.clone()),
    )
    .await
    .expect("bounded actual persistent registration")
    .unwrap();
    assert!(
        expected_store
            .load_ops_lifecycle(&meerkat_runtime::LogicalRuntimeId::for_session(&session))
            .await
            .unwrap()
            .is_some()
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        adapter.unregister_session(&session),
    )
    .await
    .unwrap()
    .unwrap();
}

#[test]
fn memory_bundle_cannot_replace_an_already_exported_adapter() {
    let original = bundle();
    let actual_adapter = original.runtime_adapter();
    let result = original.with_local_grant_authorization(configuration());
    assert!(
        matches!(
            result,
            Err(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::Busy,
                }
            )
        ),
        "sharing prevents changing authority on a different adapter"
    );
    drop(actual_adapter);
}
