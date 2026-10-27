//! Regressions use native grants, file-store reconstruction and controller
//! custody. They exercise no model, native input admission, business operation
//! or run settlement.

use super::*;
use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grant_policy::{AdmittedWorkPolicyOwner, WorkOwnerAllowance},
    grants::FileGrantOwnerStore,
    policy::{LocalPolicyAllowance, LocalPolicyPurpose},
};
use meerkat_authorization_contracts::{
    constraints::AudienceRef,
    evidence::EvidenceDigest,
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{GrantLineageRef, InputAuthorityAssociationCandidate, NativeWorkTarget},
};
use meerkat_core::{PrincipalKind, TrustDomainId, authorization::PreparedAuthorizationBinding};
use meerkat_runtime::{
    MeerkatMachine, input_authority::NativeAdmissionError, meerkat_machine::ScopedMandateOwner,
};
use std::{os::unix::fs::PermissionsExt, path::Path};

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
pub(crate) fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("toolkit-original-test").unwrap(),
    )
    .unwrap()
}
fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("source"),
        namespace: "originals".into(),
    }
}
fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: domain(),
            resource_id: value.into(),
        },
        revision: id("1"),
        digest: EvidenceDigest::of_bytes(value.as_bytes()),
    }
}
fn store(path: &Path) -> Arc<dyn GrantOwnerStore> {
    Arc::new(FileGrantOwnerStore::new(path))
}
pub(crate) fn initialized(path: &Path) -> Arc<ToolkitOriginalOwner> {
    initialized_with_clock(path, Arc::new(HostAuthorizationClock))
}
pub(crate) fn initialized_with_clock(
    path: &Path,
    clock: Arc<dyn meerkat_authorization::clock::LocalAuthorizationClock>,
) -> Arc<ToolkitOriginalOwner> {
    Arc::new(
        ToolkitOriginalOwner::initialize(
            LocalGrantConfiguration {
                root: principal("admin"),
                namespace: id("grants"),
                generation: 1,
            },
            store(path),
            clock,
            domain(),
        )
        .unwrap(),
    )
}
fn restored(path: &Path) -> Arc<ToolkitOriginalOwner> {
    Arc::new(
        ToolkitOriginalOwner::restore(store(path), Arc::new(HostAuthorizationClock), domain())
            .unwrap(),
    )
}
fn base(owner: &ToolkitOriginalOwner) -> GrantLineageRef {
    owner
        .grants()
        .issue_root(
            &principal("admin"),
            id("base"),
            principal("admin"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap()
}
pub(crate) fn source(association: &InputAuthorityAssociation) -> AuthenticatedOriginal {
    let c = association.candidate();
    AuthenticatedOriginal {
        original_work: c.original_work.clone(),
        occurrence_scope: c.ingress_namespace.occurrence_scope.clone(),
        original_authentication: c.original_authentication.clone(),
        root_event: c.root_event.clone(),
        contributing_work: c.contributing_work.clone(),
        target: c.target.clone(),
        authority_basis: c.authority_basis.clone(),
    }
}
pub(crate) fn claims(label: &str, base: &GrantLineageRef) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("user"),
        ingress_actor: principal("frontend"),
        represented_subject: None,
        original_authentication: evidence(label),
        logical_executor: principal("agent"),
        target: NativeWorkTarget {
            logical_owner: principal("agent"),
            logical_runtime: id("runtime"),
            context: id("session-document"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal("user"),
            },
        },
        original_work: OriginalWorkRef {
            authority: principal("source"),
            work: id(label),
        },
        root_event: evidence(label),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::GrantLineage {
            lineage: vec![base.clone()],
        },
        controller_grant_lineage: Vec::new(),
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: meerkat_core::RealmId::parse("test-realm").unwrap(),
            ingress: domain(),
            occurrence_scope: id(label),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .unwrap()
}
fn enroll(
    owner: &ToolkitOriginalOwner,
    association: InputAuthorityAssociation,
    child: &str,
) -> Arc<RetainedToolkitOriginal> {
    owner
        .enroll_new_authenticated(
            association.clone(),
            source(&association),
            &principal("user"),
            &principal("frontend"),
            &principal("admin"),
            id(child),
            ExecutionRestrictions::unrestricted(),
            |_| Ok(None),
        )
        .unwrap()
}
fn lineage(original: &RetainedToolkitOriginal) -> &[GrantLineageRef] {
    let WorkAuthorityBasis::GrantLineage { lineage } =
        &original.association.candidate().authority_basis
    else {
        unreachable!()
    };
    lineage
}
fn recover(
    owner: &ToolkitOriginalOwner,
    association: &InputAuthorityAssociation,
) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
    owner.restore_authenticated(
        association.clone(),
        source(association),
        &principal("user"),
        &principal("frontend"),
        || Ok(None),
    )
}

// The machine has real governed custody but no admitted work. No operation is
// allowed by these test owners; their purpose is to install a closed profile.
struct ClosedPolicy;
impl ScopedMandateOwner for ClosedPolicy {
    fn authorize_invocation(
        &self,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Err(NativeAdmissionError::Readiness(
            ControllerReadinessFailure::PolicyUnavailable,
        ))
    }
}
impl AdmittedWorkPolicyOwner for ClosedPolicy {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}
impl OperationPolicyOwner for ClosedPolicy {
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}
pub(crate) fn machine(owner: &Arc<ToolkitOriginalOwner>) -> MeerkatMachine {
    let policy = Arc::new(ClosedPolicy);
    MeerkatMachine::ephemeral()
        .with_local_grant_authorization(owner.configuration(
            ScopedInvocation::from_policy(policy.clone()),
            policy,
            None,
        ))
        .unwrap()
}

#[test]
fn sibling_cancellation_restores_only_the_other_original_and_rejects_old_handles() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("grants");
    assert!(matches!(
        ToolkitOriginalOwner::restore(store(&path), Arc::new(HostAuthorizationClock), domain()),
        Err(GrantDurabilityError::Missing)
    ));
    let owner = initialized(&path);
    let base = base(&owner);
    let a = enroll(&owner, claims("a", &base), "a-child");
    let b = enroll(&owner, claims("b", &base), "b-child");
    let retained_a = a.association.clone();
    let retained_b = b.association.clone();
    let native = machine(&owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    owner
        .cancel_original(&a, &principal("admin"), &mut custody)
        .unwrap();
    owner
        .cancel_original(&a, &principal("admin"), &mut custody)
        .unwrap();
    drop(custody);
    assert!(matches!(
        owner
            .grants()
            .resolve_lineage(lineage(&a), &principal("agent"), None),
        Err(GrantRefusal::Denied)
    ));
    assert!(
        owner
            .grants()
            .resolve_lineage(lineage(&b), &principal("agent"), None)
            .is_ok()
    );
    assert!(
        owner
            .grants()
            .resolve_lineage(&[base.clone()], &principal("admin"), None)
            .is_ok()
    );
    drop(native);
    drop(owner);
    let owner = restored(&path);
    assert!(matches!(
        recover(&owner, &retained_a),
        Err(OriginalWriterError::Grant(GrantRefusal::Denied))
    ));
    let recovered_b = recover(&owner, &retained_b).unwrap();
    assert!(recovered_b.association == retained_b);
    let native = machine(&owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    assert_eq!(
        owner.cancel_original(&b, &principal("admin"), &mut custody),
        Err(OriginalWriterError::WrongOwner)
    );
    assert!(
        owner
            .grants()
            .resolve_lineage(lineage(&recovered_b), &principal("agent"), None)
            .is_ok()
    );
}

#[test]
fn original_operation_child_cannot_be_a_sibling_controller_in_either_restore_order() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let base = base(&owner);
    let a = enroll(&owner, claims("a", &base), "a-child");
    let mut b = claims("b", &base).candidate().clone();
    b.controller_grant_lineage = lineage(&a).to_vec();
    let b = InputAuthorityAssociation::new(b).unwrap();
    let mut persisted = false;
    assert!(matches!(
        owner.enroll_new_authenticated(
            b.clone(),
            source(&b),
            &principal("user"),
            &principal("frontend"),
            &principal("admin"),
            id("b-refused-child"),
            ExecutionRestrictions::unrestricted(),
            |_| {
                persisted = true;
                Ok(None)
            }
        ),
        Err(OriginalWriterError::Conflict)
    ));
    assert!(!persisted);
    // Construct a retained application record with an independently issued
    // native child. On cold reconstruction the conflicting record can arrive
    // first; loading the other record must still refuse before gateway ready.
    let b_child = owner
        .grants()
        .issue_child(
            &principal("admin"),
            &base,
            id("b-child"),
            principal("agent"),
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let mut b = b.candidate().clone();
    b.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![base, b_child],
    };
    let b = InputAuthorityAssociation::new(b).unwrap();
    let retained_a = a.association.clone();
    drop(owner);
    let owner = restored(&path);
    recover(&owner, &b).unwrap();
    assert!(matches!(
        recover(&owner, &retained_a),
        Err(OriginalWriterError::Conflict)
    ));
}

#[test]
fn failed_source_insert_never_publishes_the_issued_child_as_an_original() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let base = base(&owner);
    let original = claims("uncommitted", &base);
    let mut issued = None;
    assert!(matches!(
        owner.enroll_new_authenticated(
            original.clone(),
            source(&original),
            &principal("user"),
            &principal("frontend"),
            &principal("admin"),
            id("orphan"),
            ExecutionRestrictions::unrestricted(),
            |association| {
                issued = Some(association.clone());
                Err(OriginalWriterError::Unavailable)
            }
        ),
        Err(OriginalWriterError::Unavailable)
    ));
    let issued = issued.unwrap();
    let WorkAuthorityBasis::GrantLineage { lineage } = &issued.candidate().authority_basis else {
        unreachable!()
    };
    assert!(
        owner
            .grants()
            .resolve_lineage(lineage, &principal("agent"), None)
            .is_ok()
    );
    assert!(
        owner
            .admitted_original_basis(
                &original.candidate().ingress_namespace,
                &original.candidate().original_work
            )
            .unwrap()
            .is_none()
    );
}

fn ingress_for(original: &RetainedToolkitOriginal) -> NativeIngressContext {
    let claims = original.association.candidate();
    let mut prompt = meerkat_runtime::input::PromptInput::new("source custody test", None);
    prompt.header.authority_association = Some(original.association.clone());
    let input = Input::Prompt(prompt);
    NativeIngressContext::from_trusted_ingress(
        &input,
        claims.requester.clone(),
        claims.ingress_actor.clone(),
        claims.ingress_namespace.realm.clone(),
        claims.original_authentication.clone(),
    )
    .unwrap()
}

#[test]
fn source_candidate_self_authenticates_but_cannot_be_a_contributor_and_drop_withdraws() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let base = base(&owner);
    let a = enroll(&owner, claims("a", &base), "a-child");
    let mut same_occurrence = claims("b", &base).candidate().clone();
    same_occurrence.ingress_namespace = a.association.candidate().ingress_namespace.clone();
    let b = enroll(
        &owner,
        InputAuthorityAssociation::new(same_occurrence).unwrap(),
        "b-child",
    );
    let ingress = ingress_for(&a);
    let namespace = &a.association.candidate().ingress_namespace;
    let work = &a.association.candidate().original_work;
    // Persisting source evidence alone is neither authenticated current
    // candidate custody nor retained contributor custody.
    assert!(
        owner
            .authenticated_original(&ingress, namespace, work)
            .unwrap()
            .is_none()
    );
    assert!(owner.admitted_original_basis(namespace, work).unwrap().is_none());
    let pending = owner.record_candidate(&a).unwrap();
    assert!(matches!(owner.retained_custody(namespace, work), Ok(RetainedCustody::NotRetained)),
        "an unenrolled candidate has no recovery promise");
    assert!(
        owner
            .authenticated_original(&ingress, namespace, work)
            .unwrap()
            .is_some()
    );
    assert!(owner.admitted_original_basis(namespace, work).unwrap().is_none());
    assert!(matches!(
        owner.record_candidate(&b),
        Err(OriginalWriterError::Conflict)
    ));
    let mut next = namespace.clone();
    next.occurrence_scope = id("next-occurrence");
    assert!(
        owner
            .authenticated_original(&ingress, &next, work)
            .unwrap()
            .is_none()
    );
    // Explicitly naming the pending source as a contributor is also refused
    // before another candidate can be published, including a self-reference.
    let mut later = claims("later", &base).candidate().clone();
    later.contributing_work = vec![work.clone()];
    let later = enroll(
        &owner,
        InputAuthorityAssociation::new(later).unwrap(),
        "later-child",
    );
    assert!(matches!(
        owner.record_candidate(&later),
        Err(OriginalWriterError::Conflict)
    ));
    let mut self_named = claims("self-named", &base).candidate().clone();
    self_named.contributing_work = vec![self_named.original_work.clone()];
    let self_named = enroll(
        &owner,
        InputAuthorityAssociation::new(self_named).unwrap(),
        "self-child",
    );
    assert!(matches!(
        owner.record_candidate(&self_named),
        Err(OriginalWriterError::Conflict)
    ));
    drop(pending);
    assert!(
        owner
            .authenticated_original(&ingress, namespace, work)
            .unwrap()
            .is_none()
    );
    assert!(owner.admitted_original_basis(namespace, work).unwrap().is_none());
    // Withdrawn source custody does not erase the exact durable binding.
    assert!(Arc::ptr_eq(&recover(&owner, &a.association).unwrap(), &a));
    drop(owner.record_candidate(&b).unwrap());
}

#[test]
fn source_settlement_promotes_exact_custody_but_cold_restore_does_not() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let base = base(&owner);
    let a = enroll(&owner, claims("a", &base), "a-child");
    let namespace = &a.association.candidate().ingress_namespace;
    let work = &a.association.candidate().original_work;
    let mut next = namespace.clone();
    next.occurrence_scope = id("next");
    let ingress = ingress_for(&a);
    // This calls the trusted source settlement boundary directly. It tests its
    // publication semantics, not an executed native admission/deduplication.
    // Actual Meta must call it only after its exact native acknowledgement.
    let acknowledged = owner.record_candidate(&a).unwrap().admit().unwrap();
    assert!(Arc::ptr_eq(&acknowledged, &a));
    assert!(
        owner
            .authenticated_original(&ingress, &next, work)
            .unwrap()
            .is_some()
    );
    assert!(owner.admitted_original_basis(&next, work).unwrap().is_some());
    // An exact replay guard does not withdraw already admitted custody.
    drop(owner.record_candidate(&a).unwrap());
    assert!(owner.admitted_original_basis(&next, work).unwrap().is_some());
    let mut later = claims("later", &base).candidate().clone();
    later.contributing_work = vec![work.clone()];
    let later = enroll(
        &owner,
        InputAuthorityAssociation::new(later).unwrap(),
        "later-child",
    );
    drop(owner.record_candidate(&later).unwrap());
    owner.release_original(&a).unwrap();
    assert!(owner.admitted_original_basis(&next, work).unwrap().is_none());
    assert!(matches!(
        owner.record_candidate(&later),
        Err(OriginalWriterError::Conflict)
    ));
    let saved = a.association.clone();
    drop(owner);
    let owner = restored(&path);
    let restored = recover(&owner, &saved).unwrap();
    assert!(owner.admitted_original_basis(namespace, work).unwrap().is_none());
    assert!(
        owner
            .authenticated_original(&ingress, namespace, work)
            .unwrap()
            .is_none()
    );
    let pending = owner.record_candidate(&restored).unwrap();
    assert!(
        owner
            .authenticated_original(&ingress, namespace, work)
            .unwrap()
            .is_some()
    );
    assert!(owner.admitted_original_basis(namespace, work).unwrap().is_none());
    drop(pending);
}

pub(crate) fn private_event(label: &str) -> crate::private_originals::PrivateAuthenticatedEvent {
    use crate::private_originals::{PrivateAuthenticatedEvent, PrivateSourceEvidence};
    let sha256 = evidence(label)
        .digest
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let observation = PrivateSourceEvidence {
        resource_id: id(label),
        revision: id("1"),
        sha256,
    };
    PrivateAuthenticatedEvent {
        route: toolkit_package::Id::new("route").unwrap(),
        original: id(label),
        occurrence: id(label),
        original_authentication: observation.clone(),
        root_event: observation,
        contributors: vec![],
        content_sha256: "a".repeat(64),
    }
}

pub(crate) fn private_route(route: &str) -> crate::private_originals::PrivateOriginalRoute {
    use crate::private_originals::PrivateOriginalRoute;
    PrivateOriginalRoute {
        id: toolkit_package::Id::new(route).unwrap(),
        grant_caller: principal("admin"),
        application_parent: None,
        controller_parent: None,
        grant_target: toolkit_package::Id::new("agent").unwrap(),
        grant_audience: AudienceRef::Principal {
            principal: principal("user"),
        },
        realm: meerkat_core::RealmId::parse("test-realm").unwrap(),
        requester: principal("user"),
        ingress_actor: principal("frontend"),
        logical_executor: principal("agent"),
        represented_subject: None,
        source_authority: principal("source"),
        authentication_domain: domain(),
        event_domain: domain(),
    }
}

pub(crate) async fn private_originals(
    owner: Arc<ToolkitOriginalOwner>,
    path: &Path,
) -> Arc<crate::private_originals::PrivateOriginals> {
    private_originals_with_store(
        owner,
        Arc::new(toolkit_store::SqliteStore::open(path).unwrap()),
    )
    .await
}

pub(crate) async fn private_originals_with_store(
    owner: Arc<ToolkitOriginalOwner>,
    store: Arc<dyn toolkit_store::ToolkitStore>,
) -> Arc<crate::private_originals::PrivateOriginals> {
    use crate::private_originals::PrivateOriginals;
    Arc::new(
        PrivateOriginals::new(
            toolkit_store::StoreScope {
                realm: "test-realm".into(),
                owner: "originals".into(),
            },
            store,
            owner,
            vec![private_route("route")],
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn private_source_store_conflicts_on_changed_event_and_restores_only_evidence() {
    use crate::private_originals::PrivateOriginalError;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let path = dir.path().join("originals.sqlite3");
    let adapter = private_originals(owner.clone(), &path).await;
    let event = private_event("one");
    let (a, b) = tokio::join!(
        adapter.retain_event(event.clone()),
        adapter.retain_event(event.clone())
    );
    let reference = a.unwrap();
    assert!(reference == b.unwrap());
    let mut changed = event.clone();
    changed.content_sha256 = "b".repeat(64);
    assert!(matches!(
        adapter.retain_event(changed).await,
        Err(PrivateOriginalError::Conflict)
    ));
    assert!(
        adapter
            .binding_evidence(&reference)
            .await
            .unwrap()
            .is_none()
    );
    drop(adapter);
    drop(owner);
    // A real cold reopen restores a fresh native owner over the same intact
    // canonical pair; dropping an adapter does not release its live binding.
    let owner = restored(&dir.path().join("grants"));
    let adapter = private_originals(owner, &path).await;
    assert!(adapter.event(&reference).await.unwrap() == event);
    assert!(
        adapter
            .binding_evidence(&reference)
            .await
            .unwrap()
            .is_none()
    );
    // Persisted source facts alone cannot establish a task-grant revocation handle.
    assert!(matches!(
        adapter.revoke_original_operation_grant(&reference),
        Err(PrivateOriginalError::Unavailable)
    ));

    // The source adapter verifies the complete stored body, including any final
    // association. A digest mismatch or a lossy typed round-trip is not absence.
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (raw, original_digest): (String, String) = connection
        .query_row("SELECT data,digest FROM toolkit_records", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    connection
        .execute("UPDATE toolkit_records SET digest=?1", ["0".repeat(64)])
        .unwrap();
    assert!(matches!(
        adapter.event(&reference).await,
        Err(PrivateOriginalError::Conflict)
    ));
    assert!(matches!(
        adapter.binding_evidence(&reference).await,
        Err(PrivateOriginalError::Conflict)
    ));
    let mut data: toolkit_store::RecordData = serde_json::from_str(&raw).unwrap();
    data.body
        .as_object_mut()
        .unwrap()
        .remove("final_association");
    connection
        .execute(
            "UPDATE toolkit_records SET data=?1,digest=?2",
            rusqlite::params![
                serde_json::to_string(&data).unwrap(),
                data.digest().unwrap()
            ],
        )
        .unwrap();
    assert!(matches!(
        adapter.binding_evidence(&reference).await,
        Err(PrivateOriginalError::Conflict)
    ));
    connection
        .execute(
            "UPDATE toolkit_records SET data=?1,digest=?2",
            rusqlite::params![raw, original_digest],
        )
        .unwrap();
    assert!(adapter.event(&reference).await.unwrap() == event);
}

#[tokio::test]
async fn private_binding_cas_precedes_native_handle_and_cancel_is_original_specific() {
    use crate::private_originals::{PrivateOriginalCancellation, PrivateOriginalError};
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let base = base(&owner);
    let adapter = private_originals(owner.clone(), &dir.path().join("originals.sqlite3")).await;
    let native = Arc::new(machine(&owner));
    adapter.bind_machine(native.clone()).unwrap();
    let mut originals = Vec::new();
    for label in ["a", "b"] {
        let reference = adapter.retain_event(private_event(label)).await.unwrap();
        let association = claims(label, &base);
        let persist_adapter = &adapter;
        let persist_reference = &reference;
        let original = owner
            .enroll_new_authenticated_async(
                association.clone(),
                source(&association),
                &principal("user"),
                &principal("frontend"),
                &principal("admin"),
                id(&format!("{label}-child")),
                ExecutionRestrictions::unrestricted(),
                move |final_association| async move {
                    persist_adapter
                        .persist_binding(persist_reference, &final_association)
                        .await
                },
            )
            .await
            .unwrap();
        assert!(
            adapter.binding_evidence(&reference).await.unwrap().as_ref()
                == Some(original.association())
        );
        assert!(matches!(
            adapter.route_for_binding(original.association()),
            Err(PrivateOriginalError::Unavailable)
        ));
        adapter
            .hold_binding(reference.clone(), original.clone())
            .await
            .unwrap();
        assert_eq!(adapter.bound_input_capture_for_test(&reference), (false, Some(false)),
            "custom source custody does not request installed-host input capture");
        assert_eq!(
            adapter.route_for_binding(original.association()).unwrap(),
            reference.route
        );
        assert_eq!(
            adapter
                .persist_binding(&reference, original.association())
                .await,
            Err(OriginalWriterError::Conflict)
        );
        originals.push((reference, original));
    }
    let mut wrong = originals[0].0.clone();
    wrong.original = originals[1].0.original.clone();
    assert!(matches!(
        adapter.revoke_original_operation_grant(&wrong),
        Err(PrivateOriginalError::Unavailable)
    ));
    assert!(
        adapter
            .revoke_original_operation_grant(&originals[0].0)
            .unwrap()
            == PrivateOriginalCancellation::OperationGrantRevoked
    );
    assert!(matches!(
        owner
            .grants()
            .resolve_lineage(lineage(&originals[0].1), &principal("agent"), None),
        Err(GrantRefusal::Denied)
    ));
    assert!(
        owner
            .grants()
            .resolve_lineage(lineage(&originals[1].1), &principal("agent"), None)
            .is_ok()
    );
    assert!(
        owner
            .grants()
            .resolve_lineage(&[base], &principal("admin"), None)
            .is_ok()
    );
    // Neither the durable association nor source-custody promotion was changed.
    assert!(
        adapter
            .binding_evidence(&originals[0].0)
            .await
            .unwrap()
            .as_ref()
            == Some(originals[0].1.association())
    );
    assert!(
        owner
            .admitted_original_basis(
                &originals[0].1.association().candidate().ingress_namespace,
                &originals[0].1.association().candidate().original_work
            )
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn standing_original_cancel_is_durable_without_revoking_shared_grant_or_controller() {
    use crate::private_originals::{PrivateOriginalError, service_grants::tests as access};
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let grants_path = dir.path().join("grants");
    let source_path = dir.path().join("originals");
    let owner = initialized(&grants_path);
    let adapter = access::adapter(owner.clone(), &source_path).await;
    let decision = access::decision();
    let access = adapter
        .apply_service_access_decision(decision)
        .await
        .unwrap();
    let a_work = OriginalWorkRef {
        authority: principal("source"),
        work: id("a"),
    };
    let shared = adapter
        .service_grant_lineage(&access.grant, &a_work)
        .await
        .unwrap();
    let controller = owner
        .grants()
        .issue_root(
            &principal("admin"),
            id("controller"),
            principal("agent"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let mut held = Vec::new();
    let mut references = Vec::new();
    let mut pending_a = None;
    for label in ["a", "b"] {
        let mut c = claims(label, &shared[0]).candidate().clone();
        c.authority_basis = WorkAuthorityBasis::GrantLineage {
            lineage: shared.clone(),
        };
        c.controller_grant_lineage = vec![controller.clone()];
        let association = InputAuthorityAssociation::new(c).unwrap();
        let reference = adapter.retain_event(private_event(label)).await.unwrap();
        let retained = adapter
            .bind_access_source(
                &reference,
                &access.grant,
                association.clone(),
                source(&association),
            )
            .await
            .unwrap();
        // The native disposition reads a valid held original even before
        // source admission promotion. This is not a native permission result.
        assert_eq!(
            owner.original_disposition(
                &association.candidate().ingress_namespace,
                &association.candidate().original_work,
            ),
            Ok(OriginalDisposition::Active),
        );
        // This explicitly exercises source-owner acknowledgement only, not
        // native admission or an executed operation-currentness check.
        let pending = owner.record_candidate(&retained).unwrap();
        if label == "a" {
            pending_a = Some(pending);
        } else {
            pending.admit().unwrap();
        }
        held.push(retained);
        references.push(reference);
    }
    let native_bytes = std::fs::read(&grants_path).unwrap();
    let old_a = adapter
        .binding_evidence(&references[0])
        .await
        .unwrap()
        .unwrap();
    let (_, stamp) = owner.publication.observe(|| ()).unwrap();
    adapter
        .record_operation_cancelled(&references[0])
        .await
        .unwrap();
    // A later real acknowledgement may still promote source custody. It must
    // never erase the already-persisted operation cancellation.
    pending_a.take().unwrap().admit().unwrap();
    assert!(stamp.check_current().is_err());
    assert_eq!(std::fs::read(&grants_path).unwrap(), native_bytes);
    assert!(matches!(
        owner.original_disposition(
            &held[0].association().candidate().ingress_namespace,
            &held[0].association().candidate().original_work,
        ),
        Ok(OriginalDisposition::OperationCancelled { .. }),
    ));
    assert_eq!(
        owner.original_disposition(
            &held[1].association().candidate().ingress_namespace,
            &held[1].association().candidate().original_work,
        ),
        Ok(OriginalDisposition::Active),
    );
    {
        let originals = owner.originals.lock().unwrap();
        let a = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|entry| Arc::ptr_eq(&entry.retained, &held[0]))
            .unwrap();
        let b = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|entry| Arc::ptr_eq(&entry.retained, &held[1]))
            .unwrap();
        assert!(matches!(
            a.operation,
            OriginalOperationDisposition::Cancelled { .. }
        ));
        assert!(b.operation == OriginalOperationDisposition::Open);
        assert!(a.source_custody == Some(SourceCustody::Admitted));
        assert!(b.source_custody == Some(SourceCustody::Admitted));
        assert!(a.association == old_a);
        assert!(a.association.candidate().controller_grant_lineage == vec![controller.clone()]);
    }
    assert!(matches!(
        owner.record_candidate(&held[0]),
        Err(OriginalWriterError::OperationCancelled)
    ));
    assert!(owner.record_candidate(&held[1]).is_ok());
    // Reusing the old read-Open association must re-read disposition under the
    // writer mutex. It cannot publish an Open binding after durable cancel.
    assert!(matches!(
        adapter
            .bind_access_source(&references[0], &access.grant, old_a.clone(), source(&old_a))
            .await,
        Err(PrivateOriginalError::Denied)
    ));
    assert!(Arc::ptr_eq(
        &adapter
            .bind_access_source(
                &references[1],
                &access.grant,
                held[1].association.clone(),
                held[1].custody.clone()
            )
            .await
            .unwrap(),
        &held[1]
    ));
    let native = machine(&owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    assert!(matches!(
        owner.cancel_original(&held[0], &principal("admin"), &mut custody),
        Err(OriginalWriterError::UnsupportedMandate)
    ));
    drop(custody);
    drop(native);
    owner
        .grants()
        .resolve_lineage(&shared, &principal("agent"), None)
        .unwrap()
        .check_current()
        .unwrap();
    owner
        .grants()
        .resolve_lineage(std::slice::from_ref(&controller), &principal("agent"), None)
        .unwrap()
        .check_current()
        .unwrap();
    assert_eq!(std::fs::read(&grants_path).unwrap(), native_bytes);
    let b_claims = held[1].association.clone();
    let parent = shared[..shared.len() - 1].to_vec();
    drop(held);
    drop(adapter);
    drop(owner);
    let owner = restored(&grants_path);
    let adapter = access::restored_adapter(owner.clone(), &source_path, parent).await;
    assert!(matches!(
        adapter
            .bind_access_source(&references[0], &access.grant, old_a.clone(), source(&old_a))
            .await,
        Err(PrivateOriginalError::Denied)
    ));
    let cancelled = owner.find_exact(&old_a).unwrap().unwrap();
    assert!(matches!(
        owner.record_candidate(&cancelled),
        Err(OriginalWriterError::OperationCancelled)
    ));
    let b = adapter
        .bind_access_source(
            &references[1],
            &access.grant,
            b_claims.clone(),
            source(&b_claims),
        )
        .await
        .unwrap();
    assert!(owner.record_candidate(&b).is_ok());
    // Immutable cancelled source evidence remains, but cold decoding has not
    // restored admitted contributor custody or native cancellation settlement.
    assert!(
        owner
            .admitted_original_basis(&old_a.candidate().ingress_namespace, &a_work)
            .unwrap()
            .is_none()
    );
    assert!(
        adapter
            .binding_evidence(&references[0])
            .await
            .unwrap()
            .as_ref()
            == Some(&old_a)
    );
    assert_eq!(std::fs::read(&grants_path).unwrap(), native_bytes);
}

#[tokio::test]
async fn cancel_before_source_binding_cannot_create_a_new_binding_on_retry() {
    use crate::private_originals::{PrivateOriginalError, service_grants::tests as access};
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let grants_path = dir.path().join("grants");
    let owner = initialized(&grants_path);
    let adapter = access::adapter(owner.clone(), &dir.path().join("originals")).await;
    let granted = adapter
        .apply_service_access_decision(access::decision())
        .await
        .unwrap();
    let event = adapter
        .retain_event(private_event("cancel-first"))
        .await
        .unwrap();
    adapter.record_operation_cancelled(&event).await.unwrap();
    // Repeating the exact authenticated event must not reset its disposition.
    assert!(
        adapter
            .retain_event(private_event("cancel-first"))
            .await
            .unwrap()
            == event
    );
    let work = OriginalWorkRef {
        authority: principal("source"),
        work: id("cancel-first"),
    };
    let lineage = adapter
        .service_grant_lineage(&granted.grant, &work)
        .await
        .unwrap();
    let mut c = claims("cancel-first", &lineage[0]).candidate().clone();
    c.authority_basis = WorkAuthorityBasis::GrantLineage { lineage };
    let association = InputAuthorityAssociation::new(c).unwrap();
    let before = std::fs::read(&grants_path).unwrap();
    assert!(matches!(
        adapter
            .bind_access_source(
                &event,
                &granted.grant,
                association.clone(),
                source(&association)
            )
            .await,
        Err(PrivateOriginalError::Denied)
    ));
    assert!(adapter.binding_evidence(&event).await.unwrap().is_none());
    assert!(owner.find_exact(&association).unwrap().is_none());
    assert_eq!(std::fs::read(&grants_path).unwrap(), before);
}

#[tokio::test]
async fn durable_binding_callback_and_original_cancel_share_the_writer_mutex() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let standing = owner
        .grants()
        .issue_root(
            &principal("admin"),
            id("standing"),
            principal("agent"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let association = claims("held-write", &standing);
    let c = association.candidate().clone();
    let adapter = private_originals(owner.clone(), &dir.path().join("originals")).await;
    let reference = adapter
        .retain_event(private_event("held-write"))
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let binding_adapter = &adapter;
    let binding_reference = &reference;
    let binding = owner.bind_issued_access_async(
        association.clone(),
        source(&association),
        &c.requester,
        &c.ingress_actor,
        None,
        |value| async move {
            let observed = binding_adapter
                .persist_or_observe_binding(binding_reference, &value, true)
                .await?;
            entered.send(()).unwrap();
            released.await.unwrap();
            Ok(observed)
        },
    );
    tokio::pin!(binding);
    tokio::select! {
        _ = waiting => {},
        result = &mut binding => panic!("binding escaped the held durable callback: {result:?}"),
    }
    // The callback has committed its row but not published. Cancellation must
    // refuse before it can interleave a second durable write/publication.
    assert!(matches!(
        adapter.record_operation_cancelled(&reference).await,
        Err(crate::private_originals::PrivateOriginalError::Unavailable)
    ));
    release.send(()).unwrap();
    let retained = binding.await.unwrap();
    adapter
        .record_operation_cancelled(&reference)
        .await
        .unwrap();
    assert!(matches!(
        owner.record_candidate(&retained),
        Err(OriginalWriterError::OperationCancelled)
    ));
    assert!(adapter.binding_evidence(&reference).await.unwrap().as_ref() == Some(&association));
}

#[tokio::test]
async fn standing_rebuild_cannot_reclassify_an_original_specific_task_child() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let base = base(&owner);
    let task = enroll(&owner, claims("task", &base), "task-child");
    let task_claims = task.association.clone();
    let mut shared = claims("standing", &base).candidate().clone();
    shared.authority_basis = task_claims.candidate().authority_basis.clone();
    let shared = InputAuthorityAssociation::new(shared).unwrap();
    assert!(matches!(
        owner
            .bind_issued_access_async(
                shared.clone(),
                source(&shared),
                &principal("user"),
                &principal("frontend"),
                None,
                |_| async { Ok(None) }
            )
            .await,
        Err(OriginalWriterError::Conflict)
    ));
    drop(task);
    drop(owner);
    let owner = restored(&path);
    owner
        .bind_issued_access_async(
            shared.clone(),
            source(&shared),
            &principal("user"),
            &principal("frontend"),
            None,
            |_| async { Ok(None) },
        )
        .await
        .unwrap();
    // A standalone historical row cannot establish the converse task binding:
    // its leaf is already retained as another original's standing permission.
    assert!(matches!(
        recover(&owner, &task_claims),
        Err(OriginalWriterError::Conflict)
    ));
}

// Faults surround actual SQLite CAS, including an owned worker that can finish
// after its awaiting cancellation future is dropped. These tests do not treat
// future drop as cancellation of SQLite's blocking work.
pub(crate) mod cancellation_store_faults {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
    use toolkit_store::*;

    pub(crate) struct CancelStore {
        sqlite: Arc<SqliteStore>,
        fault: AtomicU8,
        fail_readback: AtomicBool,
        source_audits: AtomicUsize,
        commits: Arc<AtomicUsize>,
        entered: tokio::sync::Notify,
        release: Arc<tokio::sync::Notify>,
        workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    }
    impl CancelStore {
        pub(crate) fn new(path: &Path) -> Arc<Self> {
            Arc::new(Self {
                sqlite: Arc::new(SqliteStore::open(path).unwrap()),
                fault: AtomicU8::new(0),
                fail_readback: AtomicBool::new(false),
                source_audits: AtomicUsize::new(0),
                commits: Arc::new(AtomicUsize::new(0)),
                entered: tokio::sync::Notify::new(),
                release: Arc::new(tokio::sync::Notify::new()),
                workers: Mutex::new(Vec::new()),
            })
        }
        pub(crate) async fn stored_cancelled(&self) -> bool {
            let records = self
                .sqlite
                .list_records(
                    &StoreScope {
                        realm: "test-realm".into(),
                        owner: "originals".into(),
                    },
                    None,
                    10,
                )
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            records[0].data.body["operation_cancelled"]
                .as_bool()
                .unwrap()
        }
        pub(crate) fn hold_next_cancellation(&self) {
            self.fault.store(2, Ordering::SeqCst);
        }
        pub(crate) async fn wait_for_held_cancellation(&self) {
            self.entered.notified().await;
        }
        pub(crate) async fn release_and_join_cancellation(&self) {
            self.release.notify_one();
            self.join_workers().await;
        }
        async fn join_workers(&self) {
            let workers = std::mem::take(&mut *self.workers.lock().unwrap());
            for worker in workers {
                worker.await.unwrap();
            }
        }
    }
    #[async_trait::async_trait]
    impl ToolkitStore for CancelStore {
        fn capabilities(&self) -> StoreCapabilities {
            self.sqlite.capabilities()
        }
        async fn now_ms(&self) -> StoreResult<i64> {
            self.sqlite.now_ms().await
        }
        async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
            self.sqlite.admit(request).await
        }
        async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
            if self.fail_readback.swap(false, Ordering::SeqCst) {
                return Err(StoreError::Worker);
            }
            self.sqlite.get(key).await
        }
        async fn list_records(
            &self,
            scope: &StoreScope,
            after_id: Option<&str>,
            limit: u32,
        ) -> StoreResult<Vec<StoredRecord>> {
            self.source_audits.fetch_add(1, Ordering::SeqCst);
            self.sqlite.list_records(scope, after_id, limit).await
        }
        async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
            if transition.data.body["operation_cancelled"] != serde_json::Value::Bool(true) {
                return self.sqlite.compare_and_swap(transition).await;
            }
            match self.fault.swap(0, Ordering::SeqCst) {
                1 => {
                    self.sqlite.compare_and_swap(transition).await?;
                    self.commits.fetch_add(1, Ordering::SeqCst);
                    self.fail_readback.store(true, Ordering::SeqCst);
                    Err(StoreError::Worker)
                }
                2 => {
                    let sqlite = self.sqlite.clone();
                    let release = self.release.clone();
                    let commits = self.commits.clone();
                    let (send, receive) = tokio::sync::oneshot::channel();
                    let worker = tokio::spawn(async move {
                        release.notified().await;
                        let result = sqlite.compare_and_swap(transition).await;
                        assert!(result.is_ok(), "scripted SQLite worker must commit");
                        commits.fetch_add(1, Ordering::SeqCst);
                        // The caller may have dropped its wait. Worker custody
                        // stays in the test and is joined after explicit release.
                        let _ = send.send(result);
                    });
                    self.workers.lock().unwrap().push(worker);
                    self.entered.notify_one();
                    receive.await.map_err(|_| StoreError::Worker)?
                }
                _ => {
                    let result = self.sqlite.compare_and_swap(transition).await?;
                    self.commits.fetch_add(1, Ordering::SeqCst);
                    Ok(result)
                }
            }
        }
        async fn claim_record(
            &self,
            key: &StoreKey,
            expected_revision: u64,
            worker: &str,
            ttl_ms: u64,
        ) -> StoreResult<RecordLease> {
            self.sqlite
                .claim_record(key, expected_revision, worker, ttl_ms)
                .await
        }
        async fn renew_record_claim(
            &self,
            lease: &RecordLease,
            ttl_ms: u64,
        ) -> StoreResult<RecordLease> {
            self.sqlite.renew_record_claim(lease, ttl_ms).await
        }
        async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
            self.sqlite.recover_record_claim(prior).await
        }
        async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
            self.sqlite.release_record_claim(lease).await
        }
        async fn read_events(
            &self,
            key: &StoreKey,
            after_sequence: u64,
            limit: u32,
        ) -> StoreResult<Vec<StoredEvent>> {
            self.sqlite.read_events(key, after_sequence, limit).await
        }
        async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
            self.sqlite.get_effect(key).await
        }
        async fn list_effects(
            &self,
            key: &StoreKey,
            after_id: Option<&str>,
            limit: u32,
        ) -> StoreResult<Vec<StoredEffect>> {
            self.sqlite.list_effects(key, after_id, limit).await
        }
        async fn claim_effect(
            &self,
            key: &EffectKey,
            expected_record_revision: u64,
            worker: &str,
            ttl_ms: u64,
        ) -> StoreResult<EffectClaimOutcome> {
            self.sqlite
                .claim_effect(key, expected_record_revision, worker, ttl_ms)
                .await
        }
        async fn acknowledge_effect(
            &self,
            lease: &EffectLease,
            outcome: EffectOutcome,
        ) -> StoreResult<StoredEffect> {
            self.sqlite.acknowledge_effect(lease, outcome).await
        }
        async fn claim_reconciliation(
            &self,
            key: &EffectKey,
            worker: &str,
            ttl_ms: u64,
        ) -> StoreResult<StoredEffect> {
            self.sqlite.claim_reconciliation(key, worker, ttl_ms).await
        }
        async fn reconcile_effect(
            &self,
            lease: &EffectLease,
            resolution: Reconciliation,
        ) -> StoreResult<StoredEffect> {
            self.sqlite.reconcile_effect(lease, resolution).await
        }
    }

    async fn bind_standing(
        owner: &ToolkitOriginalOwner,
        adapter: &crate::private_originals::PrivateOriginals,
        reference: &crate::private_originals::PrivateOriginalRef,
        association: &InputAuthorityAssociation,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let c = association.candidate();
        owner
            .bind_issued_access_async(
                association.clone(),
                source(association),
                &c.requester,
                &c.ingress_actor,
                None,
                |value| async move {
                    adapter
                        .persist_or_observe_binding(reference, &value, true)
                        .await
                },
            )
            .await
    }

    #[tokio::test]
    async fn committed_cancel_with_lost_reply_and_readback_stays_pending_until_exact_reconciliation()
     {
        for already_bound in [false, true] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let grant_path = dir.path().join("grants");
            let owner = initialized(&grant_path);
            let store = CancelStore::new(&dir.path().join("originals"));
            let adapter = private_originals_with_store(owner.clone(), store.clone()).await;
            let standing = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("standing"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let controller = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("controller"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let mut c = claims("uncertain", &standing).candidate().clone();
            c.controller_grant_lineage = vec![controller.clone()];
            let association = InputAuthorityAssociation::new(c).unwrap();
            let reference = adapter
                .retain_event(private_event("uncertain"))
                .await
                .unwrap();
            let held = if already_bound {
                let value = bind_standing(&owner, &adapter, &reference, &association)
                    .await
                    .unwrap();
                owner.record_candidate(&value).unwrap().admit().unwrap();
                Some(value)
            } else {
                None
            };
            let grant_bytes = std::fs::read(&grant_path).unwrap();
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            store.fault.store(1, Ordering::SeqCst);
            assert!(matches!(
                adapter.record_operation_cancelled(&reference).await,
                Err(crate::private_originals::PrivateOriginalError::Unavailable)
            ));
            assert!(stamp.check_current().is_err());
            assert_eq!(
                owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                ),
                Err(ControllerReadinessFailure::PolicyUnavailable),
            );
            assert!(store.stored_cancelled().await);
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            assert!(matches!(
                bind_standing(&owner, &adapter, &reference, &association).await,
                Err(OriginalWriterError::Unavailable)
            ));
            assert!(
                adapter.binding_evidence(&reference).await.unwrap().as_ref()
                    == held.as_ref().map(|v| v.association())
            );
            {
                let originals = owner.originals.lock().unwrap();
                assert_eq!(originals.len(), 1);
                assert!(matches!(
                    &originals[0],
                    OriginalRecord::Bound(HeldOriginal {
                        operation: OriginalOperationDisposition::CancellationPending,
                        source_custody: Some(SourceCustody::Admitted), custody_settlement: None,
                        ..
                    }) | OriginalRecord::UnboundCancellation {
                        operation: OriginalOperationDisposition::CancellationPending,
                        ..
                    }
                ));
            }
            if let Some(held) = &held {
                assert!(matches!(
                    owner.record_candidate(held),
                    Err(OriginalWriterError::Unavailable)
                ));
                assert!(
                    owner
                        .admitted_original_basis(
                            &association.candidate().ingress_namespace,
                            &association.candidate().original_work
                        )
                        .unwrap()
                        .is_some()
                );
            }
            // Only this exact cancellation path reconciles the committed source
            // disposition. Ordinary event/binding reads above left it Pending.
            adapter
                .record_operation_cancelled(&reference)
                .await
                .unwrap();
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            assert!(matches!(
                bind_standing(&owner, &adapter, &reference, &association).await,
                Err(OriginalWriterError::OperationCancelled)
            ));
            for grant in [&standing, &controller] {
                owner
                    .grants()
                    .resolve_lineage(std::slice::from_ref(grant), &principal("agent"), None)
                    .unwrap()
                    .check_current()
                    .unwrap();
            }
            assert_eq!(std::fs::read(&grant_path).unwrap(), grant_bytes);
            store.join_workers().await;
            let stored = store
                .sqlite
                .list_records(
                    &StoreScope {
                        realm: "test-realm".into(),
                        owner: "originals".into(),
                    },
                    None,
                    10,
                )
                .await
                .unwrap()
                .pop()
                .unwrap();
            {
                let originals = owner.originals.lock().unwrap();
                let evidence = match &originals[0] {
                    OriginalRecord::Bound(HeldOriginal {
                        operation: OriginalOperationDisposition::Cancelled { record },
                        ..
                    })
                    | OriginalRecord::UnboundCancellation {
                        operation: OriginalOperationDisposition::Cancelled { record },
                        ..
                    } => record,
                    _ => panic!("exact reconciliation must retain its canonical cancellation row"),
                };
                assert!(evidence.as_ref() == &stored);
            }
            // Reopen only after every scripted worker is joined. Bound history
            // retains the same row while refusing rebind; unbound history is
            // materialized by exact cancellation reconciliation, not a fabricated
            // association or restored admission receipt.
            drop(held);
            drop(adapter);
            drop(owner);
            let owner = restored(&grant_path);
            let adapter = private_originals_with_store(owner.clone(), store.clone()).await;
            if already_bound {
                assert!(matches!(
                    bind_standing(&owner, &adapter, &reference, &association).await,
                    Err(OriginalWriterError::OperationCancelled)
                ));
                assert!(owner.find_exact(&association).unwrap().is_some());
            } else {
                assert!(owner.originals.lock().unwrap().is_empty());
                adapter
                    .record_operation_cancelled(&reference)
                    .await
                    .unwrap();
                assert!(owner.find_exact(&association).unwrap().is_none());
            }
            let originals = owner.originals.lock().unwrap();
            let evidence = match &originals[0] {
                OriginalRecord::Bound(HeldOriginal {
                    operation: OriginalOperationDisposition::Cancelled { record },
                    source_custody: None, custody_settlement: None,
                    ..
                })
                | OriginalRecord::UnboundCancellation {
                    operation: OriginalOperationDisposition::Cancelled { record },
                    ..
                } => record,
                _ => panic!("cold source evidence must not restore admitted custody"),
            };
            assert!(evidence.as_ref() == &stored);
            drop(originals);
            let disposition = owner
                .original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                )
                .unwrap();
            let OriginalDisposition::OperationCancelled { evidence } = disposition else {
                panic!("cold exact cancellation must remain cancelled");
            };
            assert_eq!(evidence.resource.domain, domain());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&evidence.resource.resource_id).unwrap(),
                serde_json::json!({"schema": stored.data.schema, "key": stored.key}),
            );
            assert!(evidence.revision == id(&stored.revision.to_string()));
            assert_eq!(
                evidence
                    .digest
                    .bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                stored.content_digest,
            );
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn dropped_cancel_before_sqlite_commit_fences_bound_and_unbound_originals() {
        for already_bound in [false, true] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let owner = initialized(&dir.path().join("grants"));
            let store = CancelStore::new(&dir.path().join("originals"));
            let adapter = private_originals_with_store(owner.clone(), store.clone()).await;
            let standing = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("standing"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let association = claims("dropped", &standing);
            let reference = adapter
                .retain_event(private_event("dropped"))
                .await
                .unwrap();
            let held = if already_bound {
                Some(
                    bind_standing(&owner, &adapter, &reference, &association)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            store.fault.store(2, Ordering::SeqCst);
            let mut cancel = Box::pin(adapter.record_operation_cancelled(&reference));
            tokio::select! {
                _ = store.entered.notified() => {},
                result = &mut cancel => panic!("cancel escaped the held SQLite worker: {result:?}"),
            }
            drop(cancel);
            assert!(stamp.check_current().is_err());
            assert_eq!(
                owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work,
                ),
                Err(ControllerReadinessFailure::PolicyUnavailable),
            );
            assert!(!store.stored_cancelled().await);
            // The existing writer lock is now available, but the previously
            // read Open row cannot revive this exact original or first-bind it.
            assert!(matches!(
                bind_standing(&owner, &adapter, &reference, &association).await,
                Err(OriginalWriterError::Unavailable)
            ));
            assert!(
                adapter.binding_evidence(&reference).await.unwrap().as_ref()
                    == held.as_ref().map(|v| v.association())
            );
            if let Some(held) = &held {
                assert!(matches!(
                    owner.record_candidate(held),
                    Err(OriginalWriterError::Unavailable)
                ));
            }
            store.release.notify_one();
            store.join_workers().await;
            assert!(store.stored_cancelled().await);
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            // Even a completed worker does not itself change process custody.
            assert!(matches!(
                bind_standing(&owner, &adapter, &reference, &association).await,
                Err(OriginalWriterError::Unavailable)
            ));
            adapter
                .record_operation_cancelled(&reference)
                .await
                .unwrap();
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            assert!(matches!(
                bind_standing(&owner, &adapter, &reference, &association).await,
                Err(OriginalWriterError::OperationCancelled)
            ));
        }
    }

    #[tokio::test]
    async fn source_format_is_audited_once_at_open_and_legacy_pages_are_refused() {
        use crate::private_originals::{PrivateOriginalError, PrivateOriginals};
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&dir.path().join("grants"));
        let store = CancelStore::new(&dir.path().join("originals"));
        let scope = StoreScope {
            realm: "test-realm".into(),
            owner: "originals".into(),
        };
        let adapter = PrivateOriginals::new(
            scope.clone(),
            store.clone(),
            owner.clone(),
            vec![private_route("route")],
        )
        .await
        .unwrap();
        assert_eq!(store.source_audits.load(Ordering::SeqCst), 1);
        for i in 0..12 {
            adapter
                .retain_event(private_event(&format!("source-{i}")))
                .await
                .unwrap();
        }
        assert_eq!(
            store.source_audits.load(Ordering::SeqCst),
            1,
            "no per-original format scans"
        );
        drop(adapter);
        // Unrelated rows share this store scope. Pagination must not stop before
        // a legacy source record, and refusal must not rewrite either format.
        for i in 0..256 {
            let key = StoreKey {
                realm: scope.realm.clone(),
                owner: scope.owner.clone(),
                id: format!("0-fixture-{i:03}"),
            };
            store
                .sqlite
                .admit(AdmissionRequest {
                    admission_key: key.id.clone(),
                    key,
                    data: RecordData {
                        schema: "test.other-record".into(),
                        version: 1,
                        body: serde_json::json!({}),
                    },
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
        }
        let key = StoreKey {
            realm: scope.realm.clone(),
            owner: scope.owner.clone(),
            id: "zz-legacy-source".into(),
        };
        store
            .sqlite
            .admit(AdmissionRequest {
                admission_key: key.id.clone(),
                key,
                data: RecordData {
                    schema: "toolkit.authenticated-source-event".into(),
                    version: 2,
                    body: serde_json::json!({
                        "event": private_event("legacy"),
                        "final_association": null,
                        "operation_cancelled": true,
                    }),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let before = store.sqlite.list_records(&scope, None, 512).await.unwrap();
        let (_, stamp) = owner.publication.observe(|| ()).unwrap();
        assert!(matches!(
            PrivateOriginals::new(
                scope.clone(),
                store.clone(),
                owner,
                vec![private_route("route")]
            )
            .await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(store.source_audits.load(Ordering::SeqCst) >= 3);
        assert!(store.sqlite.list_records(&scope, None, 512).await.unwrap() == before);
        stamp.check_current().unwrap();
    }

    fn projected_reference(
        event: &crate::private_originals::PrivateAuthenticatedEvent,
    ) -> crate::private_originals::PrivateOriginalRef {
        use sha2::{Digest, Sha256};
        crate::private_originals::PrivateOriginalRef {
            route: event.route.clone(),
            original: event.original.clone(),
            event_sha256: format!(
                "{:x}",
                Sha256::digest(toolkit_package::canonical_json(event).unwrap())
            ),
        }
    }

    #[tokio::test]
    async fn canonical_row_rejects_changed_alias_before_and_after_cold_restore() {
        use crate::private_originals::{PrivateOriginalError, PrivateOriginals};
        for difference in ["requester", "root_event", "content"] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let grants_path = dir.path().join("grants");
            let database_path = dir.path().join("originals");
            let owner = initialized(&grants_path);
            let scope = StoreScope {
                realm: "test-realm".into(),
                owner: "originals".into(),
            };
            let routes = || {
                let mut bad = private_route("alias");
                if difference == "requester" {
                    bad.requester = principal("other-user");
                }
                vec![private_route("route"), bad]
            };
            let sqlite = Arc::new(SqliteStore::open(&database_path).unwrap());
            let adapter =
                PrivateOriginals::new(scope.clone(), sqlite.clone(), owner.clone(), routes())
                    .await
                    .unwrap();
            let standing = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("standing"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let association = claims("canonical", &standing);
            let event = private_event("canonical");
            let reference = adapter.retain_event(event.clone()).await.unwrap();
            let held = bind_standing(&owner, &adapter, &reference, &association)
                .await
                .unwrap();
            owner.record_candidate(&held).unwrap().admit().unwrap();
            let mut alias_event = event;
            alias_event.route = toolkit_package::Id::new("alias").unwrap();
            if difference == "root_event" {
                alias_event.root_event = private_event("other-root").root_event;
            }
            if difference == "content" {
                alias_event.content_sha256 = "b".repeat(64);
            }
            let bad = projected_reference(&alias_event);
            let before = sqlite.list_records(&scope, None, 10).await.unwrap();
            let grant_bytes = std::fs::read(&grants_path).unwrap();
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            assert!(matches!(
                adapter.retain_event(alias_event.clone()).await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert!(matches!(
                adapter.cancel_original_operation(&bad).await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert!(sqlite.list_records(&scope, None, 10).await.unwrap() == before);
            stamp.check_current().unwrap();
            assert_eq!(
                owner.original_disposition(
                    &association.candidate().ingress_namespace,
                    &association.candidate().original_work
                ),
                Ok(OriginalDisposition::Active)
            );
            assert!(Arc::ptr_eq(
                &owner.find_exact(&association).unwrap().unwrap(),
                &held
            ));
            drop(held);
            drop(adapter);
            drop(sqlite);
            drop(owner);

            // Reopen both durable owners, with no loaded canonical binding.
            let owner = restored(&grants_path);
            let sqlite = Arc::new(SqliteStore::open_existing(&database_path).unwrap());
            let adapter =
                PrivateOriginals::new(scope.clone(), sqlite.clone(), owner.clone(), routes())
                    .await
                    .unwrap();
            assert!(owner.originals.lock().unwrap().is_empty());
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            assert!(matches!(
                adapter.retain_event(alias_event).await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert!(matches!(
                adapter.cancel_original_operation(&bad).await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert!(sqlite.list_records(&scope, None, 10).await.unwrap() == before);
            stamp.check_current().unwrap();
            assert!(owner.originals.lock().unwrap().is_empty());
            assert_eq!(std::fs::read(&grants_path).unwrap(), grant_bytes);
            assert!(
                bind_standing(&owner, &adapter, &reference, &association)
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn equivalent_alias_cancel_survives_database_and_owner_reopen() {
        use crate::private_originals::{PrivateOriginalCancellation, PrivateOriginals};
        for alias_wins in [false, true] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let grants_path = dir.path().join("grants");
            let database_path = dir.path().join("originals");
            let scope = StoreScope {
                realm: "test-realm".into(),
                owner: "originals".into(),
            };
            let owner = initialized(&grants_path);
            let sqlite = Arc::new(SqliteStore::open(&database_path).unwrap());
            let adapter = PrivateOriginals::new(
                scope.clone(),
                sqlite.clone(),
                owner.clone(),
                vec![private_route("route"), private_route("alias")],
            )
            .await
            .unwrap();
            let standing = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("standing"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let association = claims("canonical", &standing);
            let event = private_event("canonical");
            let mut alias_event = event.clone();
            alias_event.route = toolkit_package::Id::new("alias").unwrap();
            // Exercise either exact first event as the one durable source row.
            if alias_wins {
                adapter.retain_event(alias_event.clone()).await.unwrap();
            }
            let reference = adapter.retain_event(event.clone()).await.unwrap();
            let alias = adapter.retain_event(alias_event.clone()).await.unwrap();
            assert!(reference != alias);
            assert!(adapter.event(&reference).await.unwrap() == event);
            assert!(adapter.event(&alias).await.unwrap() == alias_event);
            let held = bind_standing(&owner, &adapter, &reference, &association)
                .await
                .unwrap();
            let before = sqlite.list_records(&scope, None, 10).await.unwrap();
            assert_eq!(before.len(), 1);
            let grant_bytes = std::fs::read(&grants_path).unwrap();
            assert!(matches!(
                adapter.cancel_original_operation(&alias).await,
                Ok(PrivateOriginalCancellation::OperationsCancelled)
            ));
            let after = sqlite.list_records(&scope, None, 10).await.unwrap();
            assert_eq!(after.len(), 1);
            assert_eq!(before[0].key, after[0].key);
            assert!(after[0].revision > before[0].revision);
            assert_eq!(before[0].data.body["event"], after[0].data.body["event"]);
            assert_eq!(before[0].data.body["source"], after[0].data.body["source"]);
            assert_eq!(after[0].data.body["operation_cancelled"], true);
            assert!(adapter.binding_evidence(&alias).await.unwrap().as_ref() == Some(&association));
            drop(held);
            drop(adapter);
            drop(sqlite);
            drop(owner);

            let owner = restored(&grants_path);
            let sqlite = Arc::new(SqliteStore::open_existing(&database_path).unwrap());
            let adapter = PrivateOriginals::new(
                scope.clone(),
                sqlite.clone(),
                owner.clone(),
                vec![private_route("route"), private_route("alias")],
            )
            .await
            .unwrap();
            assert!(owner.originals.lock().unwrap().is_empty());
            for reference in [&reference, &alias] {
                assert!(matches!(
                    bind_standing(&owner, &adapter, reference, &association).await,
                    Err(OriginalWriterError::OperationCancelled)
                ));
                assert!(matches!(
                    owner.original_disposition(
                        &association.candidate().ingress_namespace,
                        &association.candidate().original_work
                    ),
                    Ok(OriginalDisposition::OperationCancelled { .. })
                ));
            }
            assert!(sqlite.list_records(&scope, None, 10).await.unwrap() == after);
            assert_eq!(std::fs::read(&grants_path).unwrap(), grant_bytes);
        }
    }

    #[tokio::test]
    async fn canonical_pending_cancel_rejects_changed_alias_and_reconciles_equivalent_alias() {
        use crate::private_originals::{
            PrivateOriginalCancellation, PrivateOriginalError, PrivateOriginals,
        };
        for already_bound in [false, true] {
            let dir = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let owner = initialized(&dir.path().join("grants"));
            let store = CancelStore::new(&dir.path().join("originals"));
            let scope = StoreScope {
                realm: "test-realm".into(),
                owner: "originals".into(),
            };
            let adapter = PrivateOriginals::new(
                scope.clone(),
                store.clone(),
                owner.clone(),
                vec![private_route("route"), private_route("alias")],
            )
            .await
            .unwrap();
            let reference = adapter
                .retain_event(private_event("canonical"))
                .await
                .unwrap();
            let standing = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    id("standing"),
                    principal("agent"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let association = claims("canonical", &standing);
            let held = if already_bound {
                Some(
                    bind_standing(&owner, &adapter, &reference, &association)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let mut alias_event = private_event("canonical");
            alias_event.route = toolkit_package::Id::new("alias").unwrap();
            let alias = adapter.retain_event(alias_event.clone()).await.unwrap();
            alias_event.content_sha256 = "b".repeat(64);
            let bad = projected_reference(&alias_event);
            let namespace = QualifiedIngressNamespace {
                realm: meerkat_core::RealmId::parse("test-realm").unwrap(),
                ingress: domain(),
                occurrence_scope: id("canonical"),
            };
            let work = OriginalWorkRef {
                authority: principal("source"),
                work: id("canonical"),
            };
            store.hold_next_cancellation();
            let mut cancellation = Box::pin(adapter.record_operation_cancelled(&reference));
            tokio::select! {
                _ = store.wait_for_held_cancellation() => {}
                result = &mut cancellation => panic!("cancellation escaped held SQLite CAS: {result:?}"),
            }
            drop(cancellation);
            let before = store.sqlite.list_records(&scope, None, 10).await.unwrap();
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            assert!(matches!(
                adapter.cancel_original_operation(&bad).await,
                Err(PrivateOriginalError::Conflict)
            ));
            stamp.check_current().unwrap();
            assert!(store.sqlite.list_records(&scope, None, 10).await.unwrap() == before);
            assert_eq!(
                owner.original_disposition(&namespace, &work),
                Err(ControllerReadinessFailure::PolicyUnavailable)
            );
            store.release_and_join_cancellation().await;
            assert!(matches!(
                adapter.cancel_original_operation(&bad).await,
                Err(PrivateOriginalError::Conflict)
            ));
            stamp.check_current().unwrap();
            assert!(matches!(
                adapter.cancel_original_operation(&alias).await,
                Ok(PrivateOriginalCancellation::OperationsCancelled)
            ));
            if let Some(held) = &held {
                assert!(
                    owner
                        .find_exact(&association)
                        .unwrap()
                        .is_some_and(|saved| Arc::ptr_eq(&saved, held))
                );
            }
            assert_eq!(owner.originals.lock().unwrap().len(), 1);
            assert_eq!(
                store
                    .sqlite
                    .list_records(&scope, None, 10)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert!(matches!(
                owner.original_disposition(&namespace, &work),
                Ok(OriginalDisposition::OperationCancelled { .. })
            ));
        }
    }

    #[tokio::test]
    async fn invalid_cancel_reference_cannot_create_or_poison_an_original() {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&dir.path().join("grants"));
        let adapter = private_originals(owner.clone(), &dir.path().join("originals")).await;
        let standing = owner
            .grants()
            .issue_root(
                &principal("admin"),
                id("standing"),
                principal("agent"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let association = claims("validated", &standing);
        let reference = adapter
            .retain_event(private_event("validated"))
            .await
            .unwrap();
        for already_bound in [false, true] {
            let held = if already_bound {
                Some(
                    bind_standing(&owner, &adapter, &reference, &association)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let (_, stamp) = owner.publication.observe(|| ()).unwrap();
            let mut wrong = reference.clone();
            wrong.event_sha256 = "0".repeat(64);
            assert!(matches!(
                adapter.record_operation_cancelled(&wrong).await,
                Err(crate::private_originals::PrivateOriginalError::Conflict)
            ));
            wrong = reference.clone();
            wrong.original = id("not-retained");
            assert!(matches!(
                adapter.record_operation_cancelled(&wrong).await,
                Err(crate::private_originals::PrivateOriginalError::Unavailable)
            ));
            stamp.check_current().unwrap();
            assert_eq!(
                owner.originals.lock().unwrap().len(),
                usize::from(already_bound)
            );
            if let Some(held) = held {
                assert!(owner.record_candidate(&held).is_ok());
            }
        }
        assert!(
            bind_standing(&owner, &adapter, &reference, &association)
                .await
                .is_ok()
        );
    }
}

/// The real native callback projects existing Toolkit storage evidence, without
/// reading SQLite or changing source/admission custody. These are component
/// checks, not an executed native operation/final-check regression.
#[tokio::test]
async fn native_original_disposition_rejects_unknown_wrong_owner_and_invalid_row() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let adapter = private_originals(owner.clone(), &dir.path().join("originals")).await;
    let association = claims("native-disposition", &base(&owner));
    let claims = association.candidate();
    let reference = adapter
        .retain_event(private_event("native-disposition"))
        .await
        .unwrap();
    assert_eq!(
        owner.original_disposition(&claims.ingress_namespace, &claims.original_work),
        Err(ControllerReadinessFailure::PolicyUnavailable),
        "retention alone has no held original disposition",
    );
    adapter
        .record_operation_cancelled(&reference)
        .await
        .unwrap();
    assert!(matches!(
        owner.original_disposition(&claims.ingress_namespace, &claims.original_work),
        Ok(OriginalDisposition::OperationCancelled { .. }),
    ));
    let mut wrong = claims.ingress_namespace.clone();
    wrong.ingress.authority = principal("other-source-owner");
    assert_eq!(
        owner.original_disposition(&wrong, &claims.original_work),
        Err(ControllerReadinessFailure::PolicyUnavailable),
    );
    let mut unknown = claims.original_work.clone();
    unknown.work = id("unknown");
    assert_eq!(
        owner.original_disposition(&claims.ingress_namespace, &unknown),
        Err(ControllerReadinessFailure::PolicyUnavailable),
    );
    // Inject an invalid retained evidence row into the test owner's existing
    // record. It must not be projected as a native cancellation or Active.
    {
        let mut originals = owner.originals.lock().unwrap();
        let OriginalOperationDisposition::Cancelled { record } = originals[0].operation_mut()
        else {
            panic!("confirmed cancellation row");
        };
        Arc::make_mut(record).content_digest = "f".repeat(64);
    }
    assert_eq!(
        owner.original_disposition(&claims.ingress_namespace, &claims.original_work),
        Err(ControllerReadinessFailure::PolicyUnavailable),
    );
}


/// The observer runs on a different OS thread from the holder and release
/// coordinator, so a synchronous wait cannot prevent release. This is a
/// source-owner callback control, not an executed native operation.
#[test]
fn native_original_disposition_waits_for_held_collection_before_reading_open_row() {
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    const BOUND: Duration = Duration::from_secs(2);
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let retained = enroll(
        &owner,
        claims("disposition-contention", &base(&owner)),
        "disposition-contention-child",
    );
    let claims = retained.association().candidate();
    let namespace = claims.ingress_namespace.clone();
    let original = claims.original_work.clone();
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (observed_tx, observed_rx) = mpsc::channel();
    let holder_owner = owner.clone();
    let observer_owner = owner.clone();

    let (locked, entered, waited, observation, released, holder, observer) =
        std::thread::scope(|scope| {
            let holder = scope.spawn(move || {
                let Ok(held) = holder_owner.originals.lock() else {
                    return (false, false);
                };
                let announced = locked_tx.send(()).is_ok();
                let released = release_rx.recv_timeout(BOUND).is_ok();
                drop(held);
                (announced, released)
            });
            let locked = locked_rx.recv_timeout(BOUND);
            let observer = scope.spawn(move || {
                let entered = entered_tx.send(()).is_ok();
                let observation = observer_owner.original_disposition(&namespace, &original);
                let delivered = observed_tx.send(observation).is_ok();
                (entered, delivered)
            });
            let entered = entered_rx.recv_timeout(BOUND);
            let early = observed_rx.recv_timeout(Duration::from_millis(50));
            let waited = matches!(&early, Err(RecvTimeoutError::Timeout));

            // Always release before asserting any result. Even an early
            // refusal or failed handshake must leave both real threads joined.
            let released = release_tx.send(());
            let observation = match early {
                Err(RecvTimeoutError::Timeout) => observed_rx.recv_timeout(BOUND),
                result => result,
            };
            let holder = holder.join();
            let observer = observer.join();
            (
                locked,
                entered,
                waited,
                observation,
                released,
                holder,
                observer,
            )
        });

    assert!(locked.is_ok(), "the holder must own the actual collection");
    assert!(entered.is_ok(), "the actual callback observer must enter");
    assert!(
        released.is_ok(),
        "the holder must receive the explicit release"
    );
    assert!(
        matches!(holder, Ok((true, true))),
        "the holder must join after release"
    );
    assert!(
        matches!(observer, Ok((true, true))),
        "the observer must return and join"
    );
    assert!(
        waited,
        "the callback returned while its collection was held"
    );
    assert!(matches!(observation, Ok(Ok(OriginalDisposition::Active))));
}

#[test]
fn native_original_disposition_refuses_poisoned_collection() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let owner = initialized(&dir.path().join("grants"));
    let retained = enroll(
        &owner,
        claims("disposition-poison", &base(&owner)),
        "disposition-poison-child",
    );
    let claims = retained.association().candidate();
    let poison_owner = owner.clone();
    let joined = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let _held = poison_owner.originals.lock().unwrap();
                panic!("synthetic original collection owner panic");
            })
            .join()
    });
    let observation = owner.original_disposition(&claims.ingress_namespace, &claims.original_work);

    // The panicking owner is joined before observing or asserting. The real
    // poisoned mutex remains poisoned; no recovery or replacement is performed.
    assert!(
        joined.is_err(),
        "the synthetic owner must unwind while holding the collection"
    );
    assert!(owner.originals.is_poisoned());
    assert!(matches!(
        observation,
        Err(ControllerReadinessFailure::PolicyUnavailable)
    ));
}

/// Exercise the same validated canonical source records consumed by the real
/// submit caller. This is not a claim that two live same-association handles
/// can be submitted: hold_binding separately refuses that duplicate.
#[tokio::test]
async fn native_event_key_survives_route_alias_and_cold_source_restore() {
    use crate::private_originals::{PrivateOriginalError, PrivateOriginals};
    let mut keys = Vec::new();
    for (original, occurrence) in [("one", "occurrence-a"), ("two", "occurrence-a"), ("one", "occurrence-b")] {
        let dir = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
        let grants_path = dir.path().join("grants");
        let database_path = dir.path().join("originals");
        let scope = toolkit_store::StoreScope { realm: "test-realm".into(), owner: "originals".into() };
        let owner = initialized(&grants_path);
        let sqlite = Arc::new(toolkit_store::SqliteStore::open(&database_path).unwrap());
        let adapter = PrivateOriginals::new(scope.clone(), sqlite.clone(), owner.clone(), vec![private_route("route"), private_route("alias")]).await.unwrap();
        let mut event = private_event(original);
        event.occurrence = id(occurrence);
        let reference = adapter.retain_event(event.clone()).await.unwrap();
        event.route = toolkit_package::Id::new("alias").unwrap();
        let alias = adapter.retain_event(event.clone()).await.unwrap();
        assert!(reference.event_sha256 != alias.event_sha256);
        let canonical = adapter.original_event_key(&reference).await.unwrap();
        assert_eq!(canonical, adapter.original_event_key(&alias).await.unwrap());
        // An occurrence change cannot mutate this original's retained row.
        event.occurrence = id("changed-in-place");
        assert!(matches!(adapter.retain_event(event).await, Err(PrivateOriginalError::Conflict)));
        drop(adapter);
        drop(sqlite);
        drop(owner);
        let owner = restored(&grants_path);
        let sqlite = Arc::new(toolkit_store::SqliteStore::open_existing(&database_path).unwrap());
        let adapter = PrivateOriginals::new(scope, sqlite, owner, vec![private_route("route"), private_route("alias")]).await.unwrap();
        assert_eq!(canonical, adapter.original_event_key(&alias).await.unwrap());
        assert_eq!(canonical, adapter.original_event_key(&reference).await.unwrap());
        // Separate canonical originals or occurrences never collapse merely
        // because all other installed namespace and source facts are equal.
        assert!(!keys.contains(&canonical));
        keys.push(canonical);
    }
    assert_eq!(keys.len(), 3);
}

#[path = "original_ingress_target_tests.rs"]
mod target_tests;


// These controls bind actual Inputs through the public source owner, with real
// grants and SQLite source records. Direct source acknowledgement is deliberate:
// no model, native admission, run or operation settlement is claimed here.
mod admitted_rebind {
    use super::*;
    use crate::private_originals::{PrivateOriginalRef, PrivateOriginals};
    use meerkat_core::{
        AgentError, AgentLlmClient, ControllerModelSelection, LlmStreamResult, Message,
        Provider, ProviderParamsOverride, SessionId, SessionLlmIdentity, ToolDef,
    };
    use meerkat_runtime::PromptInput;

    fn selection() -> ControllerModelSelection {
        ControllerModelSelection::new(
            SessionLlmIdentity {
                model: "rebind-fixture".into(),
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(
                serde_json::json!({"realm": "test-realm", "account": "rebind-fixture"}),
            )
            .unwrap(),
            "rebind-fixture".into(),
            "rebind-fixture".into(),
        )
    }

    struct UnusedClient;
    #[async_trait::async_trait]
    impl AgentLlmClient for UnusedClient {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(selection())
        }
        async fn stream_response(
            &self,
            _: &[Message],
            _: &[Arc<ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            panic!("source rebind must not execute a model")
        }
        fn provider(&self) -> Provider {
            Provider::Anthropic
        }
        fn model(&self) -> &str {
            "rebind-fixture"
        }
    }

    struct Fixture {
        owner: Arc<ToolkitOriginalOwner>,
        adapter: Arc<PrivateOriginals>,
        original: Arc<RetainedToolkitOriginal>,
        reference: PrivateOriginalRef,
        session: SessionId,
        _directory: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let owner = initialized(&directory.path().join("grants"));
            let adapter = private_originals(owner.clone(), &directory.path().join("source.sqlite3"))
                .await;
            let session = SessionId::new();
            let base = base(&owner);
            let reference = adapter.retain_event(private_event("rebind")).await.unwrap();
            let mut candidate = claims("rebind", &base).candidate().clone();
            candidate.target.logical_runtime = id(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string(),
            );
            candidate.target.context = id(&session.to_string());
            candidate.controller_model = Some(selection());
            candidate.controller_grant_lineage = vec![base];
            let association = InputAuthorityAssociation::new(candidate).unwrap();
            let original = owner
                .enroll_new_authenticated_async(
                    association.clone(),
                    source(&association),
                    &principal("user"),
                    &principal("frontend"),
                    &principal("admin"),
                    id("rebind-child"),
                    ExecutionRestrictions::unrestricted(),
                    |association| {
                        let adapter = adapter.clone();
                        let reference = reference.clone();
                        async move { adapter.persist_binding(&reference, &association).await }
                    },
                )
                .await
                .unwrap();
            adapter.hold_binding(reference.clone(), original.clone()).await.unwrap();
            Self {
                owner,
                adapter,
                original,
                reference,
                session,
                _directory: directory,
            }
        }

        fn bind(
            &self,
            input: Input,
            key: &str,
        ) -> Result<(Input, PendingToolkitOriginal), RuntimeDriverError> {
            self.owner.bind_authenticated_input(
                &self.original,
                &self.session,
                input,
                IdempotencyKey::new(key),
                self.original.association().candidate().original_authentication.clone(),
                ControllerModelClient::new(selection(), Arc::new(UnusedClient)),
            )
        }

        fn admit(&self) {
            let (_, pending) = self.bind(Input::Prompt(PromptInput::new("initial", None)), "initial")
                .unwrap();
            assert!(pending.pending);
            pending.admit().unwrap();
        }
    }

    #[tokio::test]
    async fn admitted_public_rebind_and_exact_replay_preserve_publication() {
        let fixture = Fixture::new().await;
        fixture.admit();
        let (_, stamp) = fixture.owner.publication.observe(|| ()).unwrap();
        let input = Input::Prompt(PromptInput::new("later host input", None));
        let input_id = input.id().clone();
        let (bound, pending) = fixture.bind(input.clone(), "later").unwrap();
        assert_eq!(bound.id(), &input_id);
        assert!(
            bound.header().authority_association.as_ref() == Some(fixture.original.association())
        );
        assert!(!pending.pending);
        drop(pending);
        assert!(
            stamp.check_current().is_ok(),
            "a read-only rebind must preserve the actual stamp"
        );
        let (replay, pending) = fixture.bind(input, "later").unwrap();
        assert_eq!(replay.id(), &input_id);
        assert!(!pending.pending);
        assert!(Arc::ptr_eq(&pending.admit().unwrap(), &fixture.original));
        assert!(
            stamp.check_current().is_ok(),
            "source replay acknowledgement must also be read-only"
        );
    }

    #[tokio::test]
    async fn new_source_candidate_and_acknowledgement_publish_changes() {
        let fixture = Fixture::new().await;
        let (_, before_candidate) = fixture.owner.publication.observe(|| ()).unwrap();
        let (_, pending) = fixture.bind(Input::Prompt(PromptInput::new("initial", None)), "initial")
            .unwrap();
        assert!(pending.pending);
        assert!(matches!(before_candidate.check_current(), Err(PublicationError::Changed)));
        let (_, before_acknowledgement) = fixture.owner.publication.observe(|| ()).unwrap();
        pending.admit().unwrap();
        assert!(matches!(
            before_acknowledgement.check_current(),
            Err(PublicationError::Changed)
        ));
    }

    #[tokio::test]
    async fn cancelled_and_poisoned_admitted_sources_cannot_rebind() {
        let cancelled = Fixture::new().await;
        cancelled.admit();
        cancelled.adapter.record_operation_cancelled(&cancelled.reference)
            .await.unwrap();
        assert!(matches!(
            cancelled.owner.record_candidate(&cancelled.original),
            Err(OriginalWriterError::OperationCancelled),
        ));
        assert!(cancelled.bind(
            Input::Prompt(PromptInput::new("after cancel", None)), "cancelled"
        ).is_err());

        let poisoned = Fixture::new().await;
        poisoned.admit();
        let (_, stamp) = poisoned.owner.publication.observe(|| ()).unwrap();
        poisoned.owner.publication.poison();
        assert!(matches!(stamp.check_current(), Err(PublicationError::Unavailable)));
        assert!(matches!(
            poisoned.owner.record_candidate(&poisoned.original),
            Err(OriginalWriterError::Unavailable),
        ));
        assert!(poisoned.bind(
            Input::Prompt(PromptInput::new("after poison", None)), "poisoned"
        ).is_err());

        let collection_poisoned = Fixture::new().await;
        collection_poisoned.admit();
        let owner = collection_poisoned.owner.clone();
        let joined = std::thread::scope(|scope| {
            scope.spawn(move || {
                let _collection = owner.originals.lock().unwrap();
                panic!("synthetic collection owner unwind");
            }).join()
        });
        assert!(joined.is_err());
        assert!(collection_poisoned.owner.originals.is_poisoned());
        assert!(matches!(
            collection_poisoned.owner.record_candidate(&collection_poisoned.original),
            Err(OriginalWriterError::Unavailable),
        ));
    }

    #[tokio::test]
    async fn native_preparation_preserves_custody_until_consumed_staging() {
        let fixture = Fixture::new().await;
        let input = Input::Prompt(PromptInput::new("prepared first input", None));
        let input_id = input.id().clone();
        let (_, before) = fixture.owner.publication.observe(|| ()).unwrap();
        let prepared = fixture.owner.prepare_authenticated_input(
            &fixture.original, &fixture.session, input, IdempotencyKey::new("prepared"),
            fixture.original.association().candidate().original_authentication.clone(),
            ControllerModelClient::new(selection(), Arc::new(UnusedClient)),
        ).unwrap();
        assert_eq!(prepared.input().id(), &input_id);
        assert!(prepared.input().header().authority_association.as_ref()
            == Some(fixture.original.association()));
        assert!(prepared.input().header().ingress_context.is_some());
        let claims = fixture.original.association().candidate();
        let ingress = prepared.input().header().ingress_context.as_ref().unwrap().clone();
        assert!(fixture.owner.authenticated_original(&ingress, &claims.ingress_namespace,
            &claims.original_work).unwrap().is_none());
        assert!(fixture.owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap().is_none());
        before.check_current().unwrap();
        drop(prepared);
        before.check_current().unwrap();
        assert!(fixture.owner.authenticated_original(&ingress, &claims.ingress_namespace, &claims.original_work).unwrap().is_none());

        let input = Input::Prompt(PromptInput::new("actual staged input", None));
        let input_id = input.id().clone();
        let prepared = fixture.owner.prepare_authenticated_input(
            &fixture.original, &fixture.session, input, IdempotencyKey::new("staged"),
            claims.original_authentication.clone(),
            ControllerModelClient::new(selection(), Arc::new(UnusedClient)),
        ).unwrap();
        let (bound, pending) = prepared.stage().unwrap();
        assert_eq!(bound.id(), &input_id);
        assert!(pending.pending);
        assert!(matches!(before.check_current(), Err(PublicationError::Changed)));
        assert!(fixture.owner.authenticated_original(&ingress, &claims.ingress_namespace, &claims.original_work).unwrap().is_some());
        assert!(fixture.owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap().is_none());
        pending.admit().unwrap();
        assert!(fixture.owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap().is_some());
    }

    #[tokio::test]
    async fn prepared_input_cannot_stage_after_actual_source_cancellation() {
        let fixture = Fixture::new().await;
        let prepared = fixture.owner.prepare_authenticated_input(
            &fixture.original, &fixture.session,
            Input::Prompt(PromptInput::new("cancel before stage", None)),
            IdempotencyKey::new("cancel-before-stage"),
            fixture.original.association().candidate().original_authentication.clone(),
            ControllerModelClient::new(selection(), Arc::new(UnusedClient)),
        ).unwrap();
        fixture.adapter.record_operation_cancelled(&fixture.reference).await.unwrap();
        assert!(prepared.stage().is_err());
        let claims = fixture.original.association().candidate();
        assert!(fixture.owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap().is_none());
        // Refused staging does not turn cancellation into retained custody.
        assert!(matches!(fixture.owner.original_disposition(
            &claims.ingress_namespace, &claims.original_work),
            Ok(OriginalDisposition::OperationCancelled { .. })));
    }
}

// Real store/owner construction controls. They establish live adapter custody,
// not a durable C1 budget or native admission.
mod canonical_adapter {
    use super::*;
    use crate::private_originals::{PrivateOriginalError, PrivateOriginals};
    use toolkit_store::{AdmissionRequest, RecordData, SqliteStore, StoreKey, StoreScope,
        ToolkitStore, Transition};

    fn scope() -> StoreScope {
        StoreScope { realm: "test-realm".into(), owner: "originals".into() }
    }

    #[tokio::test]
    async fn one_live_adapter_binds_exact_store_scope_and_cannot_be_replaced() {
        let directory = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir().unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let sqlite: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open(directory.path().join("source")).unwrap());
        let other: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open(directory.path().join("other")).unwrap());
        let before = std::fs::read(directory.path().join("grants")).unwrap();
        let (_, stamp) = owner.publication.observe(|| ()).unwrap();
        let adapter = Arc::new(PrivateOriginals::new(scope(), sqlite.clone(), owner.clone(),
            vec![private_route("route")]).await.unwrap());
        let clone = adapter.clone();
        assert!(Arc::ptr_eq(&adapter, &clone));
        assert!(matches!(PrivateOriginals::new(scope(), sqlite.clone(), owner.clone(),
            vec![private_route("route")]).await, Err(PrivateOriginalError::Denied)));
        assert!(matches!(PrivateOriginals::new(scope(), other.clone(), owner.clone(),
            vec![private_route("route")]).await, Err(PrivateOriginalError::Denied)));
        let changed = StoreScope { owner: "changed-scope".into(), ..scope() };
        assert!(matches!(PrivateOriginals::new(changed.clone(), sqlite.clone(), owner.clone(),
            vec![private_route("route")]).await, Err(PrivateOriginalError::Denied)));
        assert!(owner.check_canonical_source(sqlite.as_ref(), &scope()).is_ok());
        assert!(matches!(owner.check_canonical_source(other.as_ref(), &scope()),
            Err(OriginalWriterError::WrongOwner)));
        assert!(matches!(owner.check_canonical_source(sqlite.as_ref(), &changed),
            Err(OriginalWriterError::WrongOwner)));
        drop(adapter);
        assert!(clone.retain_event(private_event("live-clone")).await.is_ok());
        let weak = Arc::downgrade(&sqlite);
        drop(clone);
        drop(sqlite);
        assert!(weak.upgrade().is_none(), "the source binding must not retain its store");
        assert!(matches!(PrivateOriginals::new(scope(), other, owner.clone(),
            vec![private_route("route")]).await, Err(PrivateOriginalError::Unavailable)));
        stamp.check_current().unwrap();
        assert_eq!(std::fs::read(directory.path().join("grants")).unwrap(), before);

        // The installed-style store owns its publication source Arc. The
        // adapter's reverse weak selection must not turn that into a cycle.
        let published_owner = initialized(&directory.path().join("published-grants"));
        let weak_owner = Arc::downgrade(&published_owner);
        let published: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open_published(
            directory.path().join("published-source"), published_owner.clone()).unwrap());
        let weak_store = Arc::downgrade(&published);
        let adapter = PrivateOriginals::new(scope(), published.clone(), published_owner.clone(),
            vec![private_route("route")]).await.unwrap();
        drop(published_owner);
        drop(published);
        drop(adapter);
        assert!(weak_store.upgrade().is_none());
        assert!(weak_owner.upgrade().is_none());
    }

    #[tokio::test]
    async fn rejected_unpublished_constructor_does_not_consume_live_binding() {
        let directory = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir().unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let sqlite: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open(directory.path().join("source")).unwrap());
        assert!(matches!(PrivateOriginals::new(scope(), sqlite.clone(), owner.clone(),
            vec![private_route("route"), private_route("route")]).await,
            Err(PrivateOriginalError::Denied)));
        assert!(owner.canonical_source.lock().unwrap().is_none());
        let key = StoreKey { realm: scope().realm, owner: scope().owner, id: "bad-format".into() };
        let saved = sqlite.admit(AdmissionRequest { key: key.clone(), admission_key: "bad-format".into(),
            data: RecordData { schema: "toolkit.authenticated-source-event".into(), version: 2,
                body: serde_json::json!({}) }, events: vec![], effects: vec![] }).await.unwrap();
        assert!(PrivateOriginals::new(scope(), sqlite.clone(), owner.clone(),
            vec![private_route("route")]).await.is_err());
        assert!(owner.canonical_source.lock().unwrap().is_none());
        // The fixture owner repairs its never-published malformed test row.
        sqlite.compare_and_swap(Transition { key, expected_revision: saved.record().revision,
            expected_digest: Some(saved.record().content_digest.clone()), claim: None,
            data: RecordData { schema: "test.unrelated".into(), version: 1, body: serde_json::json!({}) },
            events: vec![], effects: vec![] }).await.unwrap();
        let adapter = PrivateOriginals::new(scope(), sqlite, owner,
            vec![private_route("route")]).await.unwrap();
        assert!(adapter.retain_event(private_event("after-repair")).await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_constructors_cannot_publish_two_live_adapters() {
        let directory = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir().unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let sqlite: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open(directory.path().join("source")).unwrap());
        let (left, right) = tokio::join!(
            PrivateOriginals::new(scope(), sqlite.clone(), owner.clone(), vec![private_route("route")]),
            PrivateOriginals::new(scope(), sqlite, owner, vec![private_route("route")]),
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let refused = if left.is_err() { left } else { right };
        assert!(matches!(refused, Err(PrivateOriginalError::Denied)));
    }
}
