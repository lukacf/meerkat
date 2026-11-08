//! D50 S1: an authenticated scoped original on the real native loop. The
//! installed ingress check is `ScopedOriginalIngress`; the session document
//! is bound to the original's destination before work arrives.

use super::*;
use meerkat_runtime::accept::AcceptOutcome;
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, ScopedGrantComposition, ScopedInvocation,
    ScopedMandateOwner,
};

fn sources() -> Vec<OriginalWorkRef> {
    vec![
        OriginalWorkRef {
            authority: principal("source-owner"),
            work: id("source-a"),
        },
        OriginalWorkRef {
            authority: principal("source-owner"),
            work: id("source-b"),
        },
    ]
}

/// The adapter's custody: the requester's request `work-7` with its two
/// sources, addressed to `target` under `basis`, and each source as an
/// original it holds under its own host policy.
struct Adapter {
    target: Mutex<Option<NativeWorkTarget>>,
    basis: Mutex<Option<WorkAuthorityBasis>>,
}
impl AuthenticatedOriginalSource for Adapter {
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        Option<AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        let target = self.target.lock().expect("fixture").clone();
        let basis = self.basis.lock().expect("fixture").clone();
        if ingress.requester() != &principal("requester") {
            return Ok(None);
        }
        if let Some(source) = sources().into_iter().find(|source| source == original) {
            return Ok(target.map(|target| AuthenticatedOriginal {
                occurrence_scope: id(&format!("occurrence-{}", source.work.as_str())),
                original_work: source,
                original_authentication: evidence("original-authentication"),
                root_event: evidence("original-event"),
                contributing_work: Vec::new(),
                target,
                authority_basis: WorkAuthorityBasis::HostPolicy {
                    policy: evidence("source-policy"),
                },
            }));
        }
        if original.work != id("work-7") {
            return Ok(None);
        }
        Ok(target
            .zip(basis)
            .map(|(target, authority_basis)| AuthenticatedOriginal {
                original_work: OriginalWorkRef {
                    authority: principal("source-owner"),
                    work: id("work-7"),
                },
                occurrence_scope: id("occurrence-7"),
                original_authentication: evidence("original-authentication"),
                root_event: evidence("original-event"),
                contributing_work: sources(),
                target,
                authority_basis,
            }))
    }

    fn retained_custody(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        meerkat_runtime::meerkat_machine::RetainedCustody,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(if sources().contains(original) {
            meerkat_runtime::meerkat_machine::RetainedCustody::Retained(
                WorkAuthorityBasis::HostPolicy {
                    policy: evidence("source-policy"),
                },
            )
        } else {
            meerkat_runtime::meerkat_machine::RetainedCustody::NotRetained
        })
    }
}

/// One host invocation policy over the real grant owner: the requester may
/// invoke the executor for itself while its issued invocation grant
/// resolves. Its admission answer and its admitted-work answer come from the
/// same object and the same grant publication.
struct GrantedInvocation {
    grants: Arc<LocalGrantAuthority>,
    lineage: GrantLineageRef,
}
impl GrantedInvocation {
    fn invocable(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        claims.requester == principal("requester")
            && claims.logical_executor == principal("executor")
            && claims.represented_subject.is_none()
            && self
                .grants
                .resolve_lineage(
                    std::slice::from_ref(&self.lineage),
                    &principal("requester"),
                    None,
                )
                .is_ok()
    }
}
impl AdmittedWorkPolicyOwner for GrantedInvocation {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if !self.invocable(association) {
            return Err(denied().into());
        }
        InvocationOwner.authorize_admitted_work(association, binding, purpose, now_ms)
    }
}
impl ScopedMandateOwner for GrantedInvocation {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if ingress.requester() != &principal("requester") || !self.invocable(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

struct AnyCustody;
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for AnyCustody
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

fn scoped_input(
    runtime: &LogicalRuntimeId,
    controller: &GrantLineageRef,
    operation: &GrantLineageRef,
    pin: &meerkat_core::ControllerModelClient,
    change: impl FnOnce(&mut InputAuthorityAssociationCandidate),
) -> Input {
    scoped_submission(
        runtime,
        controller,
        operation,
        pin,
        change,
        "original-authentication",
    )
}

/// The original event `event-7`, bound to its final input with
/// `bind_scoped_original` under one transport authentication observation.
fn scoped_submission(
    runtime: &LogicalRuntimeId,
    controller: &GrantLineageRef,
    operation: &GrantLineageRef,
    pin: &meerkat_core::ControllerModelClient,
    change: impl FnOnce(&mut InputAuthorityAssociationCandidate),
    transport_authentication: &str,
) -> Input {
    let mut candidate = association(
        runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .clone();
    // The original's source owner is not the ingress authority.
    candidate.original_work.authority = principal("source-owner");
    candidate.contributing_work = sources();
    change(&mut candidate);
    meerkat_runtime::meerkat_machine::bind_scoped_original(
        Input::Prompt(PromptInput::new(
            "Try delete, then read if the operation is refused",
            None,
        )),
        InputAuthorityAssociation::new(candidate).expect("claims"),
        meerkat_runtime::identifiers::IdempotencyKey::new("event-7"),
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").expect("realm"),
        evidence(transport_authentication),
        pin.clone(),
    )
    .expect("the final input is bound to its trusted ingress observation")
}

/// The authenticated original with its two sources, its grant mandate and
/// its addressed destination is admitted through the installed scoped check
/// and carries a native turn to its reply. An unauthenticated contributor,
/// an original the adapter did not authenticate, or a destination other
/// than the addressed one is refused at admission without touching the
/// session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authenticated_scoped_original_carries_a_native_turn() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    let adapter = Arc::new(Adapter {
        target: Mutex::new(None),
        basis: Mutex::new(None),
    });
    let invocation_grant = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("invocation"),
            principal("requester"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("actual requester invocation grant");
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: ResourceDomain {
            authority: principal("ingress"),
            namespace: "prompts".into(),
        },
        originals: adapter.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(GrantedInvocation {
            grants: Arc::clone(&grants),
            lineage: invocation_grant.clone(),
        })),
        operation_owner: Arc::new(RecordOwner),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration();
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("exclusive actual native host setup"),
    );
    let provider = Arc::new(ScriptedProvider::default());
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
        false,
    )
    .await
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let registration = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("current registration");
    let target = association(
        &runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .target
    .clone();
    *adapter.target.lock().expect("fixture") = Some(target.clone());
    *adapter.basis.lock().expect("fixture") = Some(WorkAuthorityBasis::GrantLineage {
        lineage: vec![operation.clone()],
    });
    // The adapter authenticates the original, but the session document
    // serves no destination yet: a scoped original needs a current binding.
    let unbound = scoped_input(&runtime, &controller, &operation, &pin, |_| {});
    assert!(
        machine
            .accept_input_with_completion(&session_id, unbound)
            .await
            .is_err(),
        "a scoped original with no current binding is refused at admission"
    );
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the session document's destination");

    for (label, change) in [
        (
            "an unauthenticated contributor",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.contributing_work.push(OriginalWorkRef {
                    authority: principal("ingress"),
                    work: id("source-c"),
                });
            }) as Box<dyn FnOnce(&mut InputAuthorityAssociationCandidate)>,
        ),
        (
            "an original the adapter did not authenticate",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.original_work.work = id("work-8");
            }),
        ),
        (
            "another destination generation",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.target.context_generation += 1;
            }),
        ),
    ] {
        let refused = scoped_input(&runtime, &controller, &operation, &pin, change);
        assert!(
            machine
                .accept_input_with_completion(&session_id, refused)
                .await
                .is_err(),
            "{label} is refused at admission"
        );
    }
    assert!(tools.0.lock().expect("body entries").is_empty());

    // The runtime's replay lookup says this is a first authentication, so the
    // transport observation must be the original's own: a different one is
    // refused before any turn.
    let foreign_authentication = scoped_submission(
        &runtime,
        &controller,
        &operation,
        &pin,
        |_| {},
        "fresh-transport-authentication",
    );
    assert!(
        machine
            .accept_input_with_completion(&session_id, foreign_authentication)
            .await
            .is_err(),
        "a first authentication by another observation is refused"
    );
    assert!(tools.0.lock().expect("body entries").is_empty());

    let input = scoped_input(&runtime, &controller, &operation, &pin, |_| {});
    let (first, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("the authenticated original is admitted");
    assert!(
        matches!(first, AcceptOutcome::Accepted { .. }),
        "the native replay lookup recognises the first occurrence: {first:?}"
    );
    let completion = completion.expect("actual native completion waiter");
    tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
        .await
        .expect("same native run reached the third controller request");
    assert_eq!(*tools.0.lock().expect("body entries"), ["read_record"]);
    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("native run completes")
        .expect("native completion observation");
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("the authenticated original releases its reply: {outcome:?}")
    };
    assert_eq!(result.text, "read completed after refusal");

    // A retry of the same original event under a fresh transport
    // authentication passes the same check and is recognised as a replay by
    // the native replay lookup, not by a caller flag: no second turn runs.
    let retry = scoped_submission(
        &runtime,
        &controller,
        &operation,
        &pin,
        |_| {},
        "retry-transport-authentication",
    );
    let (replayed, _) = machine
        .accept_input_with_completion(&session_id, retry)
        .await
        .expect("the retry is answered");
    assert!(
        matches!(replayed, AcceptOutcome::Deduplicated { .. }),
        "the native replay lookup recognises the retry: {replayed:?}"
    );
    assert_eq!(
        *tools.0.lock().expect("body entries"),
        ["read_record"],
        "a replay runs no second turn"
    );

    // Changing the original's authentication reference does not make a
    // retry: the claims no longer match the adapter's custody view.
    let relabelled = scoped_submission(
        &runtime,
        &controller,
        &operation,
        &pin,
        |c| c.original_authentication = evidence("another-authentication"),
        "retry-transport-authentication",
    );
    assert!(
        machine
            .accept_input_with_completion(&session_id, relabelled)
            .await
            .is_err(),
        "a changed original authentication reference is refused"
    );

    // The host revokes the requester's invocation grant: the same original
    // is refused at admission through the installed invocation policy.
    grants
        .revoke(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            &invocation_grant,
            &mut AnyCustody,
        )
        .expect("the host revokes the requester's invocation grant");
    let revoked = scoped_submission(
        &runtime,
        &controller,
        &operation,
        &pin,
        |_| {},
        "after-revocation-transport-authentication",
    );
    assert!(
        machine
            .accept_input_with_completion(&session_id, revoked)
            .await
            .is_err(),
        "a requester without a current invocation grant is refused"
    );
}

/// A tool dispatched inside the real governed native turn builds the helper
/// fence from its own dispatch (`from_dispatch`) and commits a keyed
/// WorkGraph admission through that dispatch's own work owners; an
/// equal-looking call that is not the dispatch's own cannot build one. The
/// same dispatch then issues the helper's child controller grant from an
/// independent parent through its own work owners
/// (`GovernedHelperChildIssuance::from_dispatch`): the child is retained by
/// the grant owner and resolves as the helper's controller lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_dispatch_builds_its_helper_fence_from_its_own_operation() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    let adapter = Arc::new(Adapter {
        target: Mutex::new(None),
        basis: Mutex::new(None),
    });
    let invocation_grant = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("invocation"),
            principal("requester"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("actual requester invocation grant");
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: ResourceDomain {
            authority: principal("ingress"),
            namespace: "prompts".into(),
        },
        originals: adapter.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(GrantedInvocation {
            grants: Arc::clone(&grants),
            lineage: invocation_grant.clone(),
        })),
        operation_owner: Arc::new(RecordOwner),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration();
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("exclusive actual native host setup"),
    );
    let provider = Arc::new(ScriptedProvider::default());
    let helper_graph =
        meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
    // The independent parent the helper's child is issued from.
    let helper_parent = grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("grant-owner"),
            id("helper-parent"),
            principal("helper-operator"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("actual independent parent grant");
    let child_intent = meerkat_authorization::grants::ChildGrantIntent {
        issuer: principal("helper-operator"),
        parent_lineage: vec![helper_parent.clone()],
        id: id("helper-child"),
        grantee: principal("helper"),
        represented_subject: None,
        requested: ExecutionRestrictions::unrestricted(),
    };
    let tools = Arc::new(HelperTools {
        entries: Mutex::new(Vec::new()),
        originals: adapter.clone(),
        graph: helper_graph,
        outcomes: Mutex::new(Vec::new()),
        child_intent,
    });
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
        false,
    )
    .await
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let registration = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("current registration");
    let target = association(
        &runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .target
    .clone();
    *adapter.target.lock().expect("fixture") = Some(target.clone());
    *adapter.basis.lock().expect("fixture") = Some(WorkAuthorityBasis::GrantLineage {
        lineage: vec![operation.clone()],
    });
    // The adapter authenticates the original, but the session document
    // serves no destination yet: a scoped original needs a current binding.
    let unbound = scoped_input(&runtime, &controller, &operation, &pin, |_| {});
    assert!(
        machine
            .accept_input_with_completion(&session_id, unbound)
            .await
            .is_err(),
        "a scoped original with no current binding is refused at admission"
    );
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the session document's destination");

    for (label, change) in [
        (
            "an unauthenticated contributor",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.contributing_work.push(OriginalWorkRef {
                    authority: principal("ingress"),
                    work: id("source-c"),
                });
            }) as Box<dyn FnOnce(&mut InputAuthorityAssociationCandidate)>,
        ),
        (
            "an original the adapter did not authenticate",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.original_work.work = id("work-8");
            }),
        ),
        (
            "another destination generation",
            Box::new(|c: &mut InputAuthorityAssociationCandidate| {
                c.target.context_generation += 1;
            }),
        ),
    ] {
        let refused = scoped_input(&runtime, &controller, &operation, &pin, change);
        assert!(
            machine
                .accept_input_with_completion(&session_id, refused)
                .await
                .is_err(),
            "{label} is refused at admission"
        );
    }
    assert!(tools.entries.lock().expect("body entries").is_empty());

    // The runtime's replay lookup says this is a first authentication, so the
    // transport observation must be the original's own: a different one is
    // refused before any turn.
    let foreign_authentication = scoped_submission(
        &runtime,
        &controller,
        &operation,
        &pin,
        |_| {},
        "fresh-transport-authentication",
    );
    assert!(
        machine
            .accept_input_with_completion(&session_id, foreign_authentication)
            .await
            .is_err(),
        "a first authentication by another observation is refused"
    );
    assert!(tools.entries.lock().expect("body entries").is_empty());

    let input = scoped_input(&runtime, &controller, &operation, &pin, |_| {});
    let (first, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("the authenticated original is admitted");
    assert!(
        matches!(first, AcceptOutcome::Accepted { .. }),
        "the native replay lookup recognises the first occurrence: {first:?}"
    );
    let completion = completion.expect("actual native completion waiter");
    tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
        .await
        .expect("same native run reached the third controller request");
    assert_eq!(
        *tools.entries.lock().expect("body entries"),
        ["read_record"]
    );
    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("native run completes")
        .expect("native completion observation");
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("the authenticated original releases its reply: {outcome:?}")
    };
    assert_eq!(result.text, "read completed after refusal");
    let outcomes = tools.outcomes.lock().expect("fence outcomes").clone();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        (
            outcomes[0].created,
            outcomes[0].refusal,
            outcomes[0].lookalike_refused
        ),
        (true, None, true),
        "the read_record dispatch admitted its helper item through its own operation"
    );
    let Some(Ok(issued)) = &outcomes[0].child else {
        panic!(
            "the dispatch issued the helper's child: {:?}",
            outcomes[0].child
        )
    };
    assert!(matches!(
        issued,
        meerkat_authorization::grants::ChildGrantOutcome::Issued { .. }
    ));
    assert_eq!(
        grants
            .issued_reference(&id("helper-child"))
            .expect("lookup")
            .as_ref(),
        issued.lineage().last(),
        "the grant owner retains exactly the issued child"
    );
    grants
        .resolve_controller_lineage(issued.lineage(), &principal("helper"), None)
        .expect("the child resolves as the helper's controller lineage");
}

#[derive(Debug, Clone)]
struct HelperOutcome {
    created: bool,
    refusal: Option<meerkat::governed_realm::helper_admission::HelperAdmissionRefusal>,
    lookalike_refused: bool,
    child: Option<
        Result<
            meerkat_authorization::grants::ChildGrantOutcome,
            meerkat::governed_realm::helper_child_issuance::HelperChildIssuanceRefusal,
        >,
    >,
}

/// The fixture tools; `read_record` also records a helper WorkGraph item
/// through a fence built from its own governed dispatch.
struct HelperTools {
    entries: Mutex<Vec<String>>,
    originals: Arc<Adapter>,
    graph: meerkat::WorkGraphService,
    outcomes: Mutex<Vec<HelperOutcome>>,
    child_intent: meerkat_authorization::grants::ChildGrantIntent,
}
#[async_trait]
impl AgentToolDispatcher for HelperTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        RecordingTools::default().tools()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.entries
            .lock()
            .expect("entered bodies")
            .push(call.name.into());
        Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        use meerkat::governed_realm::helper_admission::GovernedHelperAdmissionFence;
        if call.name == "read_record" {
            let request = || meerkat::CreateWorkItemRequest {
                title: "setup helper".into(),
                ..Default::default()
            };
            let key = meerkat::WorkAdmissionKey::new("helper-for-read").expect("key");
            let admission = self
                .graph
                .admission_identity(key.clone(), request())
                .expect("identity");
            let args = serde_json::value::RawValue::from_string(call.args.get().to_string())
                .expect("args copy");
            let (id, name) = (call.id.to_string(), call.name.to_string());
            let lookalike = ToolCallView {
                id: &id,
                name: &name,
                args: &args,
            };
            let lookalike_refused = GovernedHelperAdmissionFence::from_dispatch(
                lookalike,
                context,
                self.originals.clone(),
                Arc::<[meerkat_authorization::work::RetainedOriginal]>::from([]),
                "default",
                meerkat::WorkNamespace::default(),
                admission.clone(),
            )
            .is_err();
            let fence = GovernedHelperAdmissionFence::from_dispatch(
                call,
                context,
                self.originals.clone(),
                Arc::<[meerkat_authorization::work::RetainedOriginal]>::from([]),
                "default",
                meerkat::WorkNamespace::default(),
                admission,
            )
            .expect("the dispatch's own operation builds the fence");
            let created = matches!(
                self.graph
                    .create_idempotent_with_fence(key, request(), fence.clone())
                    .await,
                Ok(meerkat::WorkAdmissionOutcome::Created(_))
            );
            // The later effect: the helper's child grant, issued by this
            // dispatch's own work owners.
            let child = Some(
                meerkat::governed_realm::helper_child_issuance::GovernedHelperChildIssuance::from_dispatch(
                    call,
                    context,
                    self.originals.clone(),
                    Arc::<[meerkat_authorization::work::RetainedOriginal]>::from([]),
                    self.child_intent.clone(),
                )
                .and_then(|issuance| issuance.issue_or_recover()),
            );
            self.outcomes
                .lock()
                .expect("fence outcomes")
                .push(HelperOutcome {
                    created,
                    refusal: fence.refusal(),
                    lookalike_refused,
                    child,
                });
        }
        self.dispatch(call).await
    }
}
