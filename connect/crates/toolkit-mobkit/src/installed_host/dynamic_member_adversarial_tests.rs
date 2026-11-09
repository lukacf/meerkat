//! Independent class-admission controls. Artifact checks below do not claim
//! member publication, invocation authority, or an executed callback.
use super::dynamic_members::{MemberClassAdmissionRequest, PreparedMemberClassTools};
use super::*;
use meerkat_core::{
    CompiledMemberToolAction, CompiledMemberToolGrant, CompiledToolConsequence, MobMemberBinding,
};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledClassCallback, CompiledLocalPolicy, CompiledMemberClass, CompiledMemberClassInstance,
    PreparedNativeMember, validate_member_classes,
};
use serde_json::json;
use toolkit_package::Id;

struct CallbackFixture {
    tools: Arc<[Arc<meerkat_core::ToolDef>]>,
    calls: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for CallbackFixture {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        self.tools.clone()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls.lock().unwrap().push(call.name.into());
        Ok(
            meerkat_core::ToolResult::new(
                call.id.into(),
                "fixture callback executed".into(),
                false,
            )
            .into(),
        )
    }
}

fn factory(definitions: Vec<meerkat_core::ToolDef>) -> Arc<CallbackFixture> {
    Arc::new(CallbackFixture {
        tools: definitions
            .into_iter()
            .map(Arc::new)
            .collect::<Vec<_>>()
            .into(),
        calls: Default::default(),
    })
}

fn class(id: &str, tool: &str) -> CompiledMemberClass {
    CompiledMemberClass {
        purpose: Default::default(),
        id: Id::new(id).unwrap(),
        profile: "project-owner".into(),
        factory: Id::new("host:project-owner").unwrap(),
        callback_account: None,
        callbacks: vec![CompiledClassCallback {
            name: tool.into(),
            description: "Read only this member's selected project".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": false
            }),
        }],
        grants: vec![CompiledMemberToolGrant {
            tool_name: tool.into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: CompiledToolConsequence::R0,
        }],
    }
}

fn instance(class: &CompiledMemberClass, identity: &str) -> CompiledMemberClassInstance {
    let owner = super::tests::principal(identity);
    CompiledMemberClassInstance {
        class: class.id.clone(),
        member: PreparedNativeMember {
            member: MobMemberBinding {
                mob_id: "factory".into(),
                member: identity.into(),
                role: class.profile.clone(),
            },
            logical_owner: owner.clone(),
            context_generation: 1,
            audience: meerkat_authorization_contracts::constraints::AudienceRef::Destination {
                authority: owner,
                destination_id: identity.into(),
            },
            setup_capability: None,
        },
    }
}

#[test]
fn zero_instance_class_needs_no_placeholder_surface_or_policy_member() {
    let (_directory, selection) = super::tests::fixture();
    let local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let mut native = local.native_host.unwrap();
    let original_members = native.members.clone();
    let selected = class("project-owner", "project_read");
    native.member_classes = vec![selected.clone()];
    validate_member_classes(&native.member_classes).unwrap();

    let validated = validate_installed_artifacts(&native, |reference| {
        read_bundle_artifact(&selection, reference)
    })
    .unwrap();

    assert_eq!(native.members, original_members);
    assert_eq!(validated.surfaces.len(), original_members.len());
    assert!(
        native
            .members
            .iter()
            .all(|member| member.member.role != selected.profile)
    );
    let unadmitted = instance(&selected, "project-first");
    assert!(
        !validated
            .tool_policy
            .contains_member(&unadmitted.member.member)
    );
}

#[test]
fn sharing_a_factory_does_not_merge_classes_or_concrete_destinations() {
    let read = class("project-read", "project_read");
    let search = class("project-search", "project_search");
    assert_eq!(read.factory, search.factory);
    validate_member_classes(&[read.clone(), search.clone()]).unwrap();
    let realm = meerkat_core::mob_realm_id("factory").unwrap();
    let first = instance(&read, "project-one");
    let second = instance(&search, "project-two");
    let first_before = first.clone();
    let second_before = second.clone();

    let first_policy = read.materialize(&realm, &first).unwrap();
    let second_policy = search.materialize(&realm, &second).unwrap();

    assert_eq!(first_policy.member_identity, "project-one");
    assert_eq!(second_policy.member_identity, "project-two");
    assert_eq!(first_policy.grants, read.grants);
    assert_eq!(second_policy.grants, search.grants);
    assert_ne!(first_policy.grants, second_policy.grants);
    assert_eq!(first, first_before);
    assert_eq!(second, second_before);
    assert!(read.materialize(&realm, &second).is_err());
    assert!(search.materialize(&realm, &first).is_err());
}

#[test]
fn an_instance_cannot_smuggle_grants_or_a_factory_over_its_class_selection() {
    let selected = class("project-owner", "project_read");
    let original = instance(&selected, "project-one");
    for (field, value) in [
        ("factory", json!("host:unselected")),
        (
            "grants",
            json!([{"tool_name": "delete_project", "action": "invoke", "consequence": "r0"}]),
        ),
        ("callbacks", json!([])),
    ] {
        let mut candidate = serde_json::to_value(&original).unwrap();
        candidate[field] = value;
        assert!(
            serde_json::from_value::<CompiledMemberClassInstance>(candidate).is_err(),
            "instance field {field} cannot replace the selected declaration"
        );
    }
    let before = toolkit_package::canonical_json(&selected).unwrap();
    let row = selected
        .materialize(&meerkat_core::mob_realm_id("factory").unwrap(), &original)
        .unwrap();
    assert_eq!(row.grants.len(), 1);
    assert_eq!(row.grants[0].tool_name, "project_read");
    assert_eq!(toolkit_package::canonical_json(&selected).unwrap(), before);
}

fn request(selected: &CompiledMemberClass, member: &str) -> MemberClassAdmissionRequest {
    MemberClassAdmissionRequest {
        id: Id::new(format!("admit-{member}")).unwrap(),
        instance: instance(selected, member),
        tools: PreparedMemberClassTools::from_factory(
            selected,
            &selected.factory,
            factory(
                selected
                    .callbacks
                    .iter()
                    .map(|callback| callback.definition().unwrap())
                    .collect(),
            ),
        )
        .unwrap(),
    }
}

#[test]
fn actual_factory_rejects_foreign_factory_missing_callback_and_duplicate_names() {
    let selected = class("project-owner", "project_read");
    let actual = selected.callbacks[0].definition().unwrap();
    assert!(
        PreparedMemberClassTools::from_factory(
            &selected,
            &Id::new("host:other").unwrap(),
            factory(vec![actual.clone()]),
        )
        .is_err()
    );
    assert!(
        PreparedMemberClassTools::from_factory(&selected, &selected.factory, factory(vec![]))
            .is_err()
    );
    let mut same_name_other_schema = actual.clone();
    same_name_other_schema.input_schema = json!({"type":"object"});
    assert!(
        PreparedMemberClassTools::from_factory(
            &selected,
            &selected.factory,
            factory(vec![actual, same_name_other_schema]),
        )
        .is_err(),
        "duplicate composed names cannot pick the first matching definition"
    );
}

#[test]
fn schema_and_description_substitution_are_refused_even_with_the_same_tool_identity() {
    let selected = class("project-owner", "project_read");
    let declared = selected.callbacks[0].definition().unwrap();
    for change_schema in [true, false] {
        let mut changed = declared.clone();
        if change_schema {
            changed.input_schema["additionalProperties"] = json!(true);
        } else {
            changed.description = "Read any other project".into();
        }
        assert_eq!(
            changed.identity(),
            declared.identity(),
            "held identities alone cannot detect this substitution"
        );
        assert!(
            PreparedMemberClassTools::from_factory(
                &selected,
                &selected.factory,
                factory(vec![changed])
            )
            .is_err()
        );
    }
}

#[test]
fn actual_factory_output_is_retained_without_inventing_grants_for_extra_tools() {
    let selected = class("project-owner", "project_read");
    let callback =
        selected.callbacks[0]
            .definition()
            .unwrap()
            .with_provenance(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Comms,
                source_id: "selected-host-fixture".into(),
                artifact: None,
            });
    let extra = meerkat_core::ToolDef::new(
        "delete_project",
        "Unapproved operation",
        json!({"type":"object"}),
    );
    let definitions = vec![callback, extra];
    let before = serde_json::to_value(&definitions).unwrap();
    let actual = factory(definitions.clone());
    let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> = actual.clone();
    let prepared =
        PreparedMemberClassTools::from_factory(&selected, &selected.factory, dispatcher.clone())
            .unwrap();
    assert!(Arc::ptr_eq(&prepared.dispatcher(), &dispatcher));
    assert!(
        actual.calls.lock().unwrap().is_empty(),
        "preparation executes no callback"
    );
    assert_eq!(
        serde_json::to_value(prepared.definitions()).unwrap(),
        before
    );
    assert_eq!(
        prepared.identities(),
        definitions
            .iter()
            .map(meerkat_core::ToolDef::identity)
            .collect::<Vec<_>>()
    );
    let row = prepared
        .grants(
            &meerkat_core::mob_realm_id("factory").unwrap(),
            &instance(&selected, "project-one"),
        )
        .unwrap();
    assert_eq!(row.grants, selected.grants);
    assert!(
        !row.grants
            .iter()
            .any(|grant| grant.tool_name == "delete_project")
    );
}

#[test]
fn request_cannot_replace_the_packaged_class_with_a_wider_valid_declaration() {
    let selected = class("project-owner", "project_read");
    let realm = meerkat_core::mob_realm_id("factory").unwrap();
    let valid = request(&selected, "project-one");
    assert!(
        valid
            .validate(&realm, std::slice::from_ref(&selected))
            .is_ok()
    );
    assert!(valid.validate(&realm, &[]).is_err());

    let mut widened = selected.clone();
    widened.grants.push(CompiledMemberToolGrant {
        tool_name: "project_write".into(),
        action: CompiledMemberToolAction::Invoke,
        consequence: CompiledToolConsequence::R0,
    });
    widened.validate().unwrap();
    let forged = request(&widened, "project-one");
    assert!(
        forged
            .validate(&realm, std::slice::from_ref(&selected))
            .is_err()
    );

    let mut foreign = request(&selected, "project-one");
    foreign.instance.class = Id::new("unknown").unwrap();
    assert!(
        foreign
            .validate(&realm, std::slice::from_ref(&selected))
            .is_err()
    );
}

#[test]
fn request_structure_refuses_foreign_realm_role_and_unqualified_destinations() {
    let selected = class("project-owner", "project_read");
    let realm = meerkat_core::mob_realm_id("factory").unwrap();
    let foreign_realm = meerkat_core::mob_realm_id("other").unwrap();
    assert!(
        request(&selected, "project-one")
            .validate(&foreign_realm, std::slice::from_ref(&selected))
            .is_err()
    );
    for change in 0..4 {
        let mut candidate = request(&selected, "project-one");
        let unqualified = meerkat_core::PrincipalRef::new(
            meerkat_core::PrincipalKind::ServiceAccount,
            "unqualified",
        )
        .unwrap();
        match change {
            0 => candidate.instance.member.member.role = "coordinator".into(),
            1 => candidate.instance.member.logical_owner = unqualified,
            2 => {
                candidate.instance.member.audience =
                    meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                        principal: unqualified,
                    }
            }
            3 => candidate.instance.member.context_generation = 0,
            _ => unreachable!(),
        }
        assert!(
            candidate
                .validate(&realm, std::slice::from_ref(&selected))
                .is_err()
        );
    }
}

// These controls exercise the real policy owner and SQLite reopening. They do
// not fabricate a native build identity or claim held-add completion.
struct ClassRule {
    owner: meerkat_core::PrincipalRef,
    returned_owner: meerkat_core::PrincipalRef,
    selected: CompiledMemberClass,
    bootstrap: Vec<u8>,
    current: std::collections::BTreeMap<String, Arc<std::sync::atomic::AtomicBool>>,
    expire_next_reservation: std::sync::atomic::AtomicBool,
}

struct ClassReservation {
    owner: meerkat_core::PrincipalRef,
    current: Arc<std::sync::atomic::AtomicBool>,
    provenance: serde_json::Map<String, serde_json::Value>,
    expire_on_second_check: bool,
    checks: std::sync::atomic::AtomicUsize,
}

impl crate::private_originals::ApplicationServicePolicyReservation for ClassReservation {
    fn owner(&self) -> &meerkat_core::PrincipalRef {
        &self.owner
    }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.expire_on_second_check
            && self
                .checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            self.current
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
        if self.current.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}

impl crate::private_originals::ApplicationServicePolicy for ClassRule {
    fn reserve(
        &self,
        _: &crate::private_originals::ApplicationServicePolicyRequest<'_>,
    ) -> Result<crate::private_originals::ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(crate::private_originals::ApplicationServicePolicyDecision::RequiresHuman)
    }

    fn reserve_member_policy(
        &self,
        request: &super::dynamic_members::ApplicationMemberPolicyRequest<'_>,
    ) -> Result<tool_policy::ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        let member = &request.admission.instance.member.member.member;
        let current = self.current.get(member).ok_or(HostLifetimeError::Denied)?;
        let canonical = meerkat_core::CompiledApplicationToolPolicy::parse_canonical_json(
            request.canonical_policy,
        )
        .map_err(|_| HostLifetimeError::Denied)?;
        let bootstrap =
            meerkat_core::CompiledApplicationToolPolicy::parse_canonical_json(&self.bootstrap)
                .unwrap();
        if request.owner != &self.owner
            || request.deployment != "deployment"
            || request.realm != &meerkat_core::mob_realm_id("factory").unwrap()
            || request.admission.class != self.selected
            || request.admission.instance != instance(&self.selected, member)
            || canonical.provider_id != bootstrap.provider_id
            || canonical.policy_id != bootstrap.policy_id
            || canonical.members != bootstrap.members
            || !current.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(tool_policy::ApplicationToolPolicyRecoveryDecision::Permit(
            Box::new(ClassReservation {
                owner: self.returned_owner.clone(),
                current: current.clone(),
                provenance: [("fixture".into(), "exact-class-and-bootstrap".into())]
                    .into_iter()
                    .collect(),
                expire_on_second_check: self
                    .expire_next_reservation
                    .swap(false, std::sync::atomic::Ordering::SeqCst),
                checks: std::sync::atomic::AtomicUsize::new(0),
            }),
        ))
    }
}

fn class_policy() -> Vec<u8> {
    meerkat_core::CompiledApplicationToolPolicy::new(
        meerkat_core::PolicyProviderId::new("application").unwrap(),
        meerkat_core::PolicyId::new("selected").unwrap(),
        meerkat_core::PolicyRevision(1),
        meerkat_core::CompiledPolicySourceProvenance {
            source_id: "independent-class-fixture".into(),
            source_digest: meerkat_core::PolicyDigest::from_canonical_bytes(
                b"independent-class-fixture",
            ),
        },
        vec![meerkat_core::CompiledMemberToolGrants {
            member_identity: "coordinator".into(),
            grants: vec![],
        }],
    )
    .unwrap()
    .canonical_json()
    .unwrap()
}

fn class_rule(selected: &CompiledMemberClass, wrong_owner: bool) -> Arc<ClassRule> {
    let owner = super::tests::principal("application");
    Arc::new(ClassRule {
        returned_owner: if wrong_owner {
            super::tests::principal("different-application")
        } else {
            owner.clone()
        },
        owner,
        selected: selected.clone(),
        bootstrap: class_policy(),
        expire_next_reservation: std::sync::atomic::AtomicBool::new(false),
        current: ["project-one", "project-two"]
            .into_iter()
            .map(|member| {
                (
                    member.into(),
                    Arc::new(std::sync::atomic::AtomicBool::new(true)),
                )
            })
            .collect(),
    })
}

fn class_scope() -> toolkit_store::StoreScope {
    toolkit_store::StoreScope {
        realm: meerkat_core::mob_realm_id("factory").unwrap().to_string(),
        owner: "deployment".into(),
    }
}

async fn bound_class_policy(
    store: Arc<dyn toolkit_store::ToolkitStore>,
    rule: Arc<ClassRule>,
) -> (
    PreparedInstalledToolPolicy,
    Arc<tool_policy::service_policy::ServicePolicyOwner>,
) {
    let policy = PreparedInstalledToolPolicy::from_canonical(&rule.bootstrap).unwrap();
    let owner = policy
        .bind_service_owner(
            store,
            class_scope(),
            rule.owner.clone(),
            Some(rule.clone()),
            vec![rule.selected.clone()],
        )
        .await
        .unwrap();
    (policy, owner)
}

fn coordinator_still_selected(policy: &PreparedInstalledToolPolicy) {
    let surface = meerkat_core::PreparedToolSurface {
        tools: vec![],
        projected: vec![],
        catalog: meerkat_core::PreparedToolCatalog::VisibleOnly,
    };
    let member = MobMemberBinding {
        mob_id: "factory".into(),
        member: "coordinator".into(),
        role: "coordinator".into(),
    };
    assert!(policy.select(&member, &surface).is_ok());
}

#[tokio::test]
async fn wrong_application_reservation_cannot_admit_or_create_a_second_record() {
    let directory = tempfile::tempdir().unwrap();
    let store: Arc<dyn toolkit_store::ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap());
    let selected = class("project-owner", "project_read");
    let (policy, _owner) = bound_class_policy(store.clone(), class_rule(&selected, true)).await;
    assert!(matches!(
        policy
            .admit_member_class(request(&selected, "project-one"))
            .await,
        Err(HostLifetimeError::WrongOwner)
    ));
    assert_eq!(policy.member_class_registration_capacity().unwrap(), 0);
    coordinator_still_selected(&policy);
    assert_eq!(
        store
            .list_records(&class_scope(), None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn qualified_foreign_owner_and_audience_need_the_actual_application_decision() {
    let directory = tempfile::tempdir().unwrap();
    let store: Arc<dyn toolkit_store::ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap());
    let selected = class("project-owner", "project_read");
    let (policy, _owner) = bound_class_policy(store.clone(), class_rule(&selected, false)).await;
    for foreign_audience in [false, true] {
        let mut candidate = request(&selected, "project-one");
        let foreign = super::tests::principal("another-project");
        if foreign_audience {
            candidate.instance.member.audience =
                meerkat_authorization_contracts::constraints::AudienceRef::Destination {
                    authority: foreign,
                    destination_id: "another-project".into(),
                };
        } else {
            candidate.instance.member.logical_owner = foreign;
        }
        assert!(
            candidate
                .validate(
                    &meerkat_core::mob_realm_id("factory").unwrap(),
                    std::slice::from_ref(&selected)
                )
                .is_ok(),
            "qualified labels are structurally valid, so the application must decide"
        );
        assert!(matches!(
            policy.admit_member_class(candidate).await,
            Err(HostLifetimeError::Denied)
        ));
    }
    assert_eq!(policy.member_class_registration_capacity().unwrap(), 0);
    coordinator_still_selected(&policy);
    assert_eq!(
        store
            .list_records(&class_scope(), None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn retiring_one_reserved_member_preserves_second_and_bootstrap_on_sqlite_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let selected = class("project-owner", "project_read");
    let rule = class_rule(&selected, false);
    let store: Arc<dyn toolkit_store::ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(&path).unwrap());
    let (policy, owner) = bound_class_policy(store.clone(), rule.clone()).await;
    let first = policy
        .admit_member_class(request(&selected, "project-one"))
        .await
        .unwrap();
    let second_request = request(&selected, "project-two");
    let second_dispatcher = second_request.tools.dispatcher();
    let second = policy.admit_member_class(second_request).await.unwrap();
    assert!(Arc::ptr_eq(
        &second.dispatcher().unwrap(),
        &second_dispatcher
    ));
    assert_eq!(policy.member_class_registration_capacity().unwrap(), 2);
    rule.current["project-one"].store(false, std::sync::atomic::Ordering::SeqCst);
    coordinator_still_selected(&policy);
    assert!(
        owner
            .observed_admission(&first.instance().member.member)
            .is_err()
    );
    assert!(
        owner
            .observed_admission(&second.instance().member.member)
            .unwrap()
            .is_some()
    );
    first.retire().await.unwrap();
    assert_eq!(policy.member_class_registration_capacity().unwrap(), 1);
    assert_eq!(second.instance(), &instance(&selected, "project-two"));
    coordinator_still_selected(&policy);
    assert!(
        owner
            .observed_admission(&second.instance().member.member)
            .unwrap()
            .is_some()
    );
    let records = store.list_records(&class_scope(), None, 10).await.unwrap();
    assert_eq!(
        records.len(),
        1,
        "class decisions use the existing canonical slot"
    );
    let second_row = records[0].data.body["admissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "admit-project-two")
        .unwrap();
    assert_eq!(
        second_row["instance"],
        serde_json::to_value(second.instance()).unwrap()
    );
    assert_eq!(
        second_row["phase"]["kind"], "reserved",
        "native preparation was not fabricated"
    );
    drop(first);
    drop(second);
    drop(owner);
    drop(policy);
    drop(store);

    let reopened_store: Arc<dyn toolkit_store::ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(&path).unwrap());
    let (reopened, reopened_owner) = bound_class_policy(reopened_store.clone(), rule).await;
    assert_eq!(reopened.member_class_registration_capacity().unwrap(), 1);
    coordinator_still_selected(&reopened);
    assert!(
        reopened_owner
            .observed_admission(&instance(&selected, "project-two").member.member)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reopened_store
            .list_records(&class_scope(), None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let replayed = reopened
        .admit_member_class(request(&selected, "project-two"))
        .await
        .unwrap();
    assert_eq!(replayed.instance(), &instance(&selected, "project-two"));
    assert_eq!(
        reopened.member_class_registration_capacity().unwrap(),
        1,
        "exact replay cannot duplicate an existing admission"
    );
    assert!(
        reopened
            .admit_member_class(request(&selected, "project-one"))
            .await
            .is_err(),
        "retired admissions cannot be replayed into permission"
    );
}

#[tokio::test]
async fn failure_after_durable_admission_replays_without_losing_or_duplicating_authority() {
    let directory = tempfile::tempdir().unwrap();
    let store: Arc<dyn toolkit_store::ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap());
    let selected = class("project-owner", "project_read");
    let rule = class_rule(&selected, false);
    let (policy, owner) = bound_class_policy(store.clone(), rule.clone()).await;
    rule.expire_next_reservation
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        matches!(
            policy
                .admit_member_class(request(&selected, "project-one"))
                .await,
            Err(HostLifetimeError::Denied)
        ),
        "the post-CAS reservation check fails before an opaque handle is returned"
    );
    let records = store.list_records(&class_scope(), None, 10).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].data.body["admissions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        records[0].data.body["admissions"][0]["phase"]["kind"],
        "reserved"
    );
    let persisted_revision = records[0].revision;
    assert!(!rule.current["project-one"].load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        policy
            .admit_member_class(request(&selected, "project-one"))
            .await
            .is_err(),
        "replay cannot revive expired permission"
    );
    coordinator_still_selected(&policy);

    rule.current["project-one"].store(true, std::sync::atomic::Ordering::SeqCst);
    let retry = request(&selected, "project-one");
    let actual_dispatcher = retry.tools.dispatcher();
    let replayed = policy.admit_member_class(retry).await.unwrap();
    assert!(Arc::ptr_eq(
        &replayed.dispatcher().unwrap(),
        &actual_dispatcher
    ));
    assert!(
        owner
            .observed_admission(&replayed.instance().member.member)
            .unwrap()
            .is_some()
    );
    let after = store.list_records(&class_scope(), None, 10).await.unwrap();
    assert_eq!(
        after[0].revision, persisted_revision,
        "replay retains the same canonical decision without a new write"
    );
    assert_eq!(
        after[0].data.body["admissions"].as_array().unwrap().len(),
        1
    );

    let mut changed = request(&selected, "project-one");
    changed.instance.member.context_generation = 2;
    assert!(policy.admit_member_class(changed).await.is_err());
    let mut different_id = request(&selected, "project-one");
    different_id.id = Id::new("another-admission").unwrap();
    assert!(policy.admit_member_class(different_id).await.is_err());
    replayed.retire().await.unwrap();
    coordinator_still_selected(&policy);
    assert!(
        policy
            .admit_member_class(request(&selected, "project-one"))
            .await
            .is_err()
    );
}
