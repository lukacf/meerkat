use super::*;
use meerkat_core::{
    ToolApplicationControlRequest, ToolApplicationIngress, ToolApplicationOperation,
    ToolApplicationRequest,
};

struct Ingress(AtomicBool);
impl ToolApplicationIngress for Ingress {
    fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.0
            .load(Ordering::Acquire)
            .then_some(())
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

struct UiOwner;
impl AdmittedWorkPolicyOwner for UiOwner {
    fn authorize_tool_application(
        &self,
        control: &ToolApplicationControlRequest,
        _binding: &PreparedAuthorizationBinding,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if control
            .ingress()
            .as_any()
            .downcast_ref::<Ingress>()
            .is_none()
        {
            return Err(denied().into());
        }
        control.revalidate()?;
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
        })
    }
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        panic!("a UI request must not borrow an admitted model run")
    }
}
impl OperationPolicyOwner for UiOwner {
    fn authorize_tool_application(
        &self,
        control: &ToolApplicationControlRequest,
        binding: &PreparedAuthorizationBinding,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if !matches!(&control.request().operation, ToolApplicationOperation::ReadResource { uri } if uri == "ui://fixture/view")
            || !matches!(&binding.facts().operation, AuthorizationOperation::Source(SourceAuthorizationFacts { target: SourceAuthorizationTarget::External(target), usage: SourceAuthorizationUse::Read })
                if target.authority == who("resource") && target.namespace.as_ref() == "public" && target.id.as_ref() == "record")
        {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![tuple("read", "public")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        panic!("a UI resource must use its exact fresh submission")
    }
}

fn control(ingress: Arc<Ingress>) -> Arc<ToolApplicationControlRequest> {
    ToolApplicationControlRequest::from_trusted_ingress(
        SessionId::new(),
        ToolApplicationRequest {
            tool_call_id: "original-tool".into(),
            extension: "io.modelcontextprotocol/ui".into(),
            operation: ToolApplicationOperation::ReadResource {
                uri: "ui://fixture/view".into(),
            },
        },
        ingress,
    )
    .expect("trusted UI request")
}

fn read_binding() -> PreparedAuthorizationBinding {
    let mut facts = source(SourceAuthorizationUse::Read, "public")
        .facts()
        .clone();
    facts.execution_scope = OperationExecutionScope::Domain;
    PreparedAuthorizationBinding::new(facts)
}

#[test]
fn tool_application_requires_explicit_fresh_owner_support() {
    let fixture = Fixture::new();
    let (old_policy, _, _) = fixture.policy(fixture.association());
    let ingress = Arc::new(Ingress(AtomicBool::new(true)));
    let binding = read_binding();
    assert!(
        old_policy
            .tool_application_authorization(control(ingress.clone()))
            .authorization()
            .prepare(&binding)
            .is_err()
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        fixture.grants.clone(),
        Arc::new(UiOwner),
        Arc::new(UiOwner),
    ));
    let context = policy.tool_application_authorization(control(ingress.clone()));
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("fresh UI admission");
    assert!(prepared.check_current(&binding).is_ok());
    ingress.0.store(false, Ordering::Release);
    assert!(prepared.check_current(&binding).is_err());
}

#[test]
fn tool_application_rejects_borrowed_run_scope_and_changed_publication() {
    let fixture = Fixture::new();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        fixture.grants.clone(),
        Arc::new(UiOwner),
        Arc::new(UiOwner),
    ));
    let context =
        policy.tool_application_authorization(control(Arc::new(Ingress(AtomicBool::new(true)))));
    assert!(
        context
            .authorization()
            .prepare(&source(SourceAuthorizationUse::Read, "public"))
            .is_err()
    );
    let binding = read_binding();
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("fresh UI admission");
    {
        let _change = fixture
            .publication
            .begin_owner_change()
            .expect("publication");
    }
    assert!(prepared.check_current(&binding).is_err());
}
