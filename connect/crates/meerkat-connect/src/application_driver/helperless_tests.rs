//! Empty held completion and canonical adoption use the production Driver.
//! This does not qualify later native Add/callback execution or a process restart.
use super::*;
use futures_util::FutureExt;
use meerkat_mobkit::identity_first::{
    DurabilityPolicy, HeldBootstrapStart, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MutableRosterProvider,
};
use meerkat_toolkit::host_lifetime::{
    control::HostBootstrapSelection,
    policy::{CompiledArtifactReference, CompiledLocalPolicy, CompiledNativeHost},
};
use std::panic::{AssertUnwindSafe, resume_unwind};

fn plan(root: &std::path::Path) -> ApplicationPreparationPlan {
    let (application, mut owner) =
        crate::application_setup::compilation::tests::helperless_input(root, true);
    owner.native_host.remote_operator = Some(
        meerkat_core::PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            "helperless-test-operator",
            meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
        )
        .unwrap(),
    );
    ApplicationPreparationPlan {
        schema: 1,
        deployment_root: root.to_owned(),
        owner,
        startup: OperatorOwnerStartup::fresh("helperless-held-test", 1).unwrap(),
        authorities: Default::default(),
        application,
        acquisition: None,
        output: root.join("completed"),
        hold_timeout_ms: 30_000,
    }
}

#[test]
fn helperless_plan_omits_helper_authorities_and_refuses_even_empty_present_fields() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let selected = plan(&root);
    let bytes = toolkit_package::canonical_json(&selected).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["authorities"], serde_json::json!({}));
    assert!(wire.get("acquisition").is_none());
    assert_eq!(
        toolkit_package::canonical_json(&read_plan(&bytes).unwrap()).unwrap(),
        bytes
    );
    for (name, value) in [
        ("helper_mcp", serde_json::Value::Null),
        ("helper_catalog", serde_json::Value::Null),
        ("helper_report", serde_json::Value::Null),
        ("agent_setup", serde_json::json!([])),
        ("shared_workflow_setup", serde_json::json!([])),
    ] {
        let mut changed = wire.clone();
        changed["authorities"][name] = value;
        assert!(read_plan(&toolkit_package::canonical_json(&changed).unwrap()).is_err());
    }
    let mut original = wire;
    original["authorities"] = serde_json::json!({"controllers":[{
        "target":{"kind":"original_route","route":"source"},"issuance_id":"selected-controller"}]});
    assert!(read_plan(&toolkit_package::canonical_json(&original).unwrap()).is_ok());
    // Presence checking is a wire constraint only. Actual route selection and
    // controller issuance still require the held native report and owner.
}

fn publish(root: &std::path::Path, prepared: &PreparedApplicationSetup) -> HostBootstrapSelection {
    use crate::application_bootstrap::hash;
    let bundle = root.join("shared/policy/helperless");
    std::fs::create_dir_all(&bundle).unwrap();
    for name in ["control", "data", "activations"] {
        std::fs::create_dir_all(root.join("shared").join(name)).unwrap();
    }
    let mut artifacts = Vec::new();
    let mut write = |name: &str, bytes: &[u8]| {
        std::fs::write(bundle.join(name), bytes).unwrap();
        let reference = CompiledArtifactReference {
            name: name.into(),
            sha256: hash(bytes),
        };
        artifacts.push(serde_json::json!({"path":name,"sha256":reference.sha256}));
        reference
    };
    let setup = write(
        "application-setup.json",
        &toolkit_package::canonical_json(prepared).unwrap(),
    );
    let policy = write(
        &prepared.application_tool_policy.name,
        &std::fs::read(
            prepared
                .artifact_directory
                .as_ref()
                .unwrap()
                .join(&prepared.application_tool_policy.name),
        )
        .unwrap(),
    );
    let carrier = write(
        "application-only.json",
        &toolkit_package::canonical_json(
            &crate::application_acquisition::ApplicationOnlySelection {
                schema: 2,
                prepared_application_setup: setup,
            },
        )
        .unwrap(),
    );
    let source = write("policy.json", br#"{"schema":1}"#);
    let owner = prepared.native_owner.as_ref().unwrap();
    let early = &owner.native_host;
    let local = CompiledLocalPolicy {
        schema: 7,
        deployment: owner.deployment.clone(),
        source_policy_sha256: source.sha256,
        components: owner.components.clone(),
        channels: owner.channels.clone(),
        native_host: Some(CompiledNativeHost {
            authentication_domain: None,
            event_domain: None,
            realm: early.realm.clone(),
            grant_administrator: early.grant_administrator.clone(),
            remote_operator: early.remote_operator.clone(),
            service_parent_administration: early.service_parent_administration.clone(),
            ingress_domain: early.ingress_domain.clone(),
            application_tool_policy: policy,
            setup: Some(carrier),
            member_classes: early.member_classes.clone(),
            members: prepared.native_members.clone(),
            routes: early.routes.clone(),
        }),
    };
    let local = write(
        "toolkit-local-policy.json",
        &toolkit_package::canonical_json(&local).unwrap(),
    );
    let manifest =
        toolkit_package::canonical_json(&serde_json::json!({"artifacts":artifacts})).unwrap();
    std::fs::write(bundle.join("manifest.json"), &manifest).unwrap();
    let activation = toolkit_package::canonical_json(&serde_json::json!({"activation_id":"1","policy_bundle":{
        "path":"shared/policy/helperless","policy_digest":format!("policy-v1:sha256:{}", hash(&manifest))}})).unwrap();
    let activation_path = root.join("shared/activations/1.json");
    std::fs::write(&activation_path, &activation).unwrap();
    std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
    HostBootstrapSelection {
        deployment_root: root.to_owned(),
        activation: SelectedHostArtifact {
            path: activation_path,
            sha256: hash(&activation),
        },
        policy_manifest: SelectedHostArtifact {
            path: bundle.join("manifest.json"),
            sha256: hash(&manifest),
        },
        local_policy: SelectedHostArtifact {
            path: bundle.join("toolkit-local-policy.json"),
            sha256: local.sha256,
        },
    }
}

struct NoOperationReview;
#[async_trait::async_trait]
impl meerkat_core::approval::review::OperationReviewer for NoOperationReview {
    async fn review(
        &self,
        _: &meerkat_core::approval::review::ReviewCandidate<'_>,
    ) -> Result<
        meerkat_core::approval::review::ReviewVerdict,
        meerkat_core::approval::review::ReviewerFailure,
    > {
        panic!("empty held roster has no operation to review")
    }
}

#[tokio::test]
async fn empty_native_held_callback_withholds_activation_until_exact_owner_adoption() {
    held_case(false).await;
}

#[tokio::test]
async fn selected_application_review_follows_actual_driver_selection_and_close() {
    held_case(true).await;
}

async fn held_case(review_selected: bool) {
    let directory = crate::application_consent::tests::private_root();
    let root = directory.path().canonicalize().unwrap();
    let plan = plan(&root);
    let bytes = toolkit_package::canonical_json(&plan).unwrap();
    let compilation = Arc::new(
        plan.application
            .clone()
            .validate()
            .unwrap()
            .prepare_without_helper()
            .unwrap(),
    );
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    let owner =
        PreparedInstallationOwner::open(root.clone(), plan.owner.clone(), plan.startup.clone())
            .unwrap();
    let operation_review = if review_selected {
        let issuer = crate::application_consent::ApplicationConsentOwner::create(
            &root,
            crate::application_consent::ConsentApplicationIdentity::from_preparation(&owner)
                .unwrap(),
        )
        .unwrap();
        Some(
            crate::application_consent::ApplicationOperationReview::new(
                issuer,
                Arc::new(NoOperationReview),
                Duration::from_secs(30),
            )
            .bind(&owner)
            .unwrap(),
        )
    } else {
        None
    };
    let expected_review = operation_review
        .as_ref()
        .map(|selection| selection.current().unwrap());
    let prepared = owner
        .prepare(compilation.prepared_tool_policy())
        .await
        .unwrap();
    let originals = prepared.originals();
    let registration = Arc::new(PreparedMemberRegistration::new(
        compilation.clone(),
        compilation.registry(),
        {
            let compilation = compilation.clone();
            Arc::new(move |identity, destination| {
                compilation.selected_destination(identity, destination)
            })
        },
    ));
    let runtime = prepared.gateway_runtime_without_acquisition(registration.clone());
    assert!(runtime.acquisition().is_none());
    let configuration = prepared.take_configuration().unwrap();
    let (deliver, completed) = oneshot::channel();
    let (adopted, receive) = oneshot::channel();
    let (closed, _) = watch::channel(false);
    let driver = Arc::new(Driver {
        operation_review_policy: None,
        operation_review,
        plan,
        plan_reference: SelectedHostArtifact {
            path: root.join("plan.json"),
            sha256: crate::application_bootstrap::hash(&bytes),
        },
        plan_bytes: bytes,
        compilation,
        registration,
        runtime,
        configuration: Mutex::new(Some(configuration)),
        prepared: Mutex::new(Some(prepared)),
        presenters: None,
        selected_policy: None,
        publication: Mutex::new(Publication {
            closed: false,
            admission_stopped: false,
            installed: None,
        }),
        closed,
        selected: AtomicBool::new(false),
        completed: OnceLock::new(),
        deliver: Mutex::new(Some(deliver)),
        adopted: Mutex::new(Some(receive)),
        local_round: None,
        local_round_cleanup: tokio::sync::Mutex::new(Default::default()),
        member_class_context: OnceLock::new(),
        workflow: None,
        workflow_resources: None,
        hosted_mcp: None,
        late_adoption: None,
    });
    let mut held_task = None;
    let caught = AssertUnwindSafe(async {
        assert!(
            matches!(
                driver.selected_operation_review(),
                Err(GatewayHostError::Denied)
            ),
            "a retained issuer does not replace actual preparation selection"
        );
        assert!(driver.prepared_application(None).await.is_err());
        let request = PreparedApplicationRequest {
            bundle: driver.plan_bytes.clone(),
            declared_digest: driver.plan_reference.sha256.clone(),
        };
        assert!(
            driver
                .prepared_application(Some(&request))
                .await
                .unwrap()
                .is_some()
        );
        match (
            &expected_review,
            driver.selected_operation_review().unwrap(),
        ) {
            (None, None) => {}
            (Some(expected), Some(selected)) => assert!(Arc::ptr_eq(expected, &selected)),
            _ => panic!("Driver changed the explicit operation-review selection"),
        }
        assert!(
            driver.prepared_application(Some(&request)).await.is_err(),
            "preparation selection is one-use"
        );
        assert!(
            matches!(
                driver.compose_agent_customizer(None),
                Err(GatewayHostError::AgentCustomizerRequired)
            ),
            "zero initial members still require the declared class factory"
        );
        // Meta itself creates the held token and invokes the production callback.
        // An empty roster has no session/agent to construct and needs no bridge.
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "helperless-empty-held".into(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(vec![])),
            None,
            None,
            Some(driver.compilation.definition().clone()),
        ));
        let complete = driver.clone();
        held_task = Some(tokio::spawn(async move {
            context
                .bootstrap_roster_held(
                    &[],
                    tokio::time::Instant::now() + Duration::from_secs(30),
                    move |members, token| async move {
                        assert!(members.is_empty());
                        complete
                            .complete_held_identity_members(members, token)
                            .await
                            .map_err(|error| error.to_string())
                    },
                )
                .await
        }));
        let manifest = tokio::time::timeout(Duration::from_secs(10), completed)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::application_bootstrap::hash(&std::fs::read(&manifest.path).unwrap()),
            manifest.sha256
        );
        assert!(
            !held_task.as_ref().unwrap().is_finished(),
            "native activation is withheld until final adoption"
        );
        let completed = driver.completed.get().unwrap();
        assert!(
            completed.helper.is_none()
                && completed.resources.is_none()
                && completed.native_members.is_empty()
        );
        let selection = publish(&root, completed);
        let prepared = driver.prepared.lock().unwrap().take().unwrap();
        let (owner, installed) = prepared.adopt(selection.clone()).unwrap();
        assert!(Arc::ptr_eq(&originals, &installed.originals()));
        let native = owner.selected_policy().native_host.as_ref().unwrap();
        let selected = crate::application_acquisition::read_application_only(native, |reference| {
            toolkit_mobkit::installed_host::read_bundle_artifact(&selection, reference)
        })
        .unwrap()
        .unwrap();
        assert!(selected.matches_preparation(completed).unwrap());
        let (installed, acquisition) =
            crate::application_acquisition::prepare_gateway_acquisition_with_retained(
                &selection, native, installed, None,
            )
            .unwrap();
        assert!(acquisition.is_none());
        assert!(Arc::ptr_eq(&originals, &installed.originals()));
        assert!(
            toolkit_mobkit::installed_host::InstalledToolkitHost::open(
                selection,
                toolkit_mobkit::installed_host::NativeOwnerStartup::Restore
            )
            .await
            .is_err(),
            "adoption retains the canonical owner lease instead of reopening it"
        );
        // The public native runtime/adoption and callback Add acceptance remains
        // separate. This component stops at the real canonical host adoption.
        adopted.send(()).unwrap();
        let finished = tokio::time::timeout(Duration::from_secs(10), held_task.as_mut().unwrap())
            .await
            .unwrap();
        // A completed handle must not be polled again even if its inner result
        // fails the assertion below. A timeout retains the live handle for cleanup.
        held_task = None;
        assert!(matches!(
            finished.unwrap().unwrap(),
            HeldBootstrapStart::Activated(_)
        ));
        assert!(
            driver.local_round.is_none()
                && driver.presenters.is_none()
                && driver.workflow.is_none()
        );
    })
    .catch_unwind()
    .await;
    // Retain the join handle across a failed assertion or timeout. Join the
    // fixture's held task before rethrowing; this is test emergency cleanup,
    // not a native adoption or completion receipt.
    let close = driver.begin_close(GatewayHostCloseReason::Shutdown);
    let joined = driver.join_close().await;
    if let Some(held) = held_task {
        held.abort();
        let _ = held.await;
    }
    if let Err(error) = caught {
        resume_unwind(error);
    }
    close.unwrap();
    joined.unwrap();
    assert!(
        matches!(
            driver.selected_operation_review(),
            Err(GatewayHostError::Denied)
        ),
        "closing the Driver refuses further review selection"
    );
}
