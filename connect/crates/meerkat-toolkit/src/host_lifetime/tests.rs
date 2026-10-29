//! Selected application artifacts and physical component lifetimes only.
//! Association fixtures are freely constructible claims, not authenticated
//! originals, native grants, admission receipts or durable custody.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::authority::{ChannelInboxObservation, ChannelRequestKey};
use meerkat_authorization_contracts::constraints::{
    AudienceRef, ExecutionRestrictions, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, InputAuthorityAssociationCandidate, NativeWorkTarget,
    OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::auth::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_core::connection::MobMemberBinding;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use toolkit_package::canonical_json;

struct Fixture {
    _directory: tempfile::TempDir,
    selection: HostBootstrapSelection,
    owner: Arc<NativeHostLifetime>,
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}

fn principal(kind: PrincipalKind, value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, value, TrustDomainId::new("household").unwrap()).unwrap()
}

fn artifact(path: PathBuf, bytes: &[u8]) -> SelectedHostArtifact {
    std::fs::write(&path, bytes).unwrap();
    SelectedHostArtifact {
        path,
        sha256: hex::encode(Sha256::digest(bytes)),
    }
}

impl Fixture {
    fn new() -> Self {
        Self::with_policy(|_| {})
    }

    fn with_policy(change: impl FnOnce(&mut policy::CompiledLocalPolicy)) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bundle = root.join("shared/policy/bundle.1");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(root.join("shared/activations")).unwrap();
        std::fs::create_dir_all(root.join("shared/control")).unwrap();
        let source = artifact(bundle.join("policy.json"), br#"{"schema":1}"#);
        let requester = principal(PrincipalKind::Human, "person:original");
        let mut policy = policy::CompiledLocalPolicy {
            schema: 7,
            native_host: None,
            deployment: Id::new("home").unwrap(),
            source_policy_sha256: source.sha256.clone(),
            components: vec![Id::new("slack-extension").unwrap()],
            channels: vec![policy::CompiledChannelBinding {
                component: Id::new("slack-extension").unwrap(),
                channel_id: "slack".into(),
                service_subject: "service:slack".into(),
                provider_account: "account:family".into(),
                sender_id: "sender".into(),
                requester: requester.clone(),
                ingress_actor: principal(PrincipalKind::ServiceAccount, "service:slack"),
                target_identity: "agent:target".into(),
                member: MobMemberBinding {
                    mob_id: "paired_host".into(),
                    role: "identity".into(),
                    member: "mk--agent_ctarget".into(),
                },
                realm: meerkat_core::mob_realm_id("paired_host").unwrap(),
                required_surfaces: vec!["channel:slack".into()],
            }],
        };
        let row = &policy.channels[0];
        let domain = ResourceDomain {
            authority: row.ingress_actor.clone(),
            namespace: "channel-events".into(),
        };
        let reference = policy::CompiledArtifactReference {
            name: "fixture.json".into(),
            sha256: "a".repeat(64),
        };
        policy.native_host = Some(policy::CompiledNativeHost {
            setup: None,
            service_parent_administration: None,
            remote_operator: None,
            realm: row.realm.clone(),
            grant_administrator: principal(PrincipalKind::ServiceAccount, "grant-owner"),
            ingress_domain: domain,
            application_tool_policy: reference.clone(),
            members: vec![policy::CompiledNativeMember {
                setup_capability: None,
                member: row.member.clone(),
                logical_owner: principal(PrincipalKind::SharedAgent, "agent:target"),
                context_generation: 1,
                audience: AudienceRef::Principal {
                    principal: requester,
                },
                initial_surface: reference,
            }],
            routes: vec![],
        });
        change(&mut policy);
        let local_policy = artifact(
            bundle.join("toolkit-local-policy.json"),
            &canonical_json(&policy).unwrap(),
        );
        let manifest = serde_json::json!({"artifacts":[
            {"path":"policy.json","sha256":source.sha256},
            {"path":"toolkit-local-policy.json","sha256":local_policy.sha256}
        ]});
        let policy_manifest = artifact(
            bundle.join("manifest.json"),
            &canonical_json(&manifest).unwrap(),
        );
        let revision = format!("policy-v1:sha256:{}", policy_manifest.sha256);
        let activation = artifact(
            root.join("shared/activations/1.json"),
            &canonical_json(&serde_json::json!({
                "activation_id":"1",
                "policy_bundle":{
                    "path":"shared/policy/bundle.1", "policy_digest":revision
                }
            }))
            .unwrap(),
        );
        std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
        let selection = HostBootstrapSelection {
            deployment_root: root,
            activation,
            policy_manifest,
            local_policy,
        };
        let owner = Arc::new(NativeHostLifetime::bootstrap(selection.clone()).unwrap());
        Self {
            _directory: directory,
            selection,
            owner,
        }
    }

    fn open(&self) -> HostComponentObservation {
        self.owner
            .open_component(&Id::new("slack-extension").unwrap())
            .unwrap()
    }

    fn source(&self) -> ChannelInboxObservation {
        ChannelInboxObservation {
            key: ChannelRequestKey::new("slack", "head").unwrap(),
            target_identity: "agent:target".into(),
            source: serde_json::from_value(serde_json::json!({
                "version":1,
                "channel_id":"slack",
                "extension_id":"slack-extension",
                "service_subject":"service:slack",
                "provider_account":"account:family",
                "requester":"person:original",
                "sender_id":"sender",
                "partition":"private-chat",
                "policy_digest":self.owner.policy_revision.as_str()
            }))
            .unwrap(),
            members: vec![],
        }
    }

    fn association(&self) -> InputAuthorityAssociation {
        let row = &self.owner.selected_policy().channels[0];
        let member = &self
            .owner
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .members[0];
        let domain = ResourceDomain {
            authority: row.ingress_actor.clone(),
            namespace: "channel-events".into(),
        };
        let evidence = HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: domain.clone(),
                resource_id: "source-row".into(),
            },
            revision: id("source-revision"),
            digest: EvidenceDigest::from_array([7; 32]),
        };
        let original = OriginalWorkRef {
            authority: row.ingress_actor.clone(),
            work: id("head"),
        };
        // No native authenticator, policy owner or runtime is invoked. Even
        // matching these historical claims proves only selection correspondence.
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: row.requester.clone(),
            ingress_actor: row.ingress_actor.clone(),
            represented_subject: None,
            original_authentication: evidence.clone(),
            logical_executor: member.logical_owner.clone(),
            target: NativeWorkTarget {
                logical_owner: member.logical_owner.clone(),
                logical_runtime: id("native-fixture-runtime"),
                context: id("native-fixture-context"),
                context_generation: member.context_generation,
                audience: member.audience.clone(),
            },
            original_work: original.clone(),
            root_event: evidence.clone(),
            contributing_work: vec![original],
            authority_basis: WorkAuthorityBasis::HostPolicy { policy: evidence },
            controller_grant_lineage: vec![],
            controller_model: None,
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: vec![],
            ingress_namespace: QualifiedIngressNamespace {
                realm: row.realm.clone(),
                ingress: domain,
                occurrence_scope: id("dispatch-group"),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .unwrap()
    }
}

#[tokio::test]
async fn actual_close_waits_for_entered_guard_and_never_reopens() {
    let f = Fixture::new();
    f.open();
    let id = Id::new("slack-extension").unwrap();
    let (entered, _) = f
        .owner
        .component(&id)
        .unwrap()
        .retain(f.owner.incarnation, None)
        .unwrap();
    let closing = f
        .owner
        .begin_close_component(&id, HostCloseReason::OperatorDisable)
        .unwrap();
    assert_eq!(closing.state, HostCloseState::Closing);
    assert!(f.owner.open_component(&id).is_err());
    assert!(
        f.owner
            .component(&id)
            .unwrap()
            .retain(f.owner.incarnation, None)
            .is_err()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            f.owner.join_close(&closing.handle)
        )
        .await
        .is_err()
    );
    assert_eq!(
        f.owner.observe_close(&closing.handle).unwrap().state,
        HostCloseState::Closing
    );
    drop(entered);
    assert_eq!(
        f.owner.join_close(&closing.handle).await.unwrap().state,
        HostCloseState::Settled
    );
    assert!(f.owner.open_component(&id).is_err());
    assert_eq!(
        f.owner.join_close(&closing.handle).await.unwrap().state,
        HostCloseState::Settled
    );
}

#[tokio::test]
async fn close_waiter_cancellation_preserves_same_pending_owner_and_incarnation() {
    let f = Fixture::new();
    f.open();
    let (entered, _) = f
        .owner
        .activation
        .retain(f.owner.incarnation, None)
        .unwrap();
    let close = f.owner.host_lost().unwrap();
    let owner = f.owner.clone();
    let handle = close.handle.clone();
    let waiter = tokio::spawn(async move { owner.join_close(&handle).await });
    tokio::task::yield_now().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(
        f.owner.observe_close(&close.handle).unwrap().state,
        HostCloseState::Closing
    );
    drop(entered);
    assert_eq!(
        f.owner.join_close(&close.handle).await.unwrap().state,
        HostCloseState::Settled
    );
    let fresh = NativeHostLifetime::bootstrap(f.selection.clone()).unwrap();
    assert_eq!(
        fresh.observe_close(&close.handle),
        Err(HostLifetimeError::WrongOwner)
    );
}

#[test]
fn late_gateway_publication_after_actual_host_close_never_enters_callback() {
    let f = Fixture::new();
    let mut calls = 0;
    f.owner
        .publish_gateway_ready(&mut || {
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 1);
    f.owner.host_lost().unwrap();
    assert_eq!(
        f.owner.publish_gateway_ready(&mut || {
            calls += 1;
            Ok(())
        }),
        Err(HostLifetimeError::Denied)
    );
    assert_eq!(calls, 1);
}

#[test]
fn physical_writer_fence_blocks_selection_and_ready_callback_until_release() {
    let f = Fixture::new();
    f.open();
    let source = f.source();
    let association = f.association();
    let exclusive = f.owner.deny_fence.exclusive().unwrap();
    let mut calls = 0;
    assert_eq!(
        f.owner.validate_source_selection(&source, &association),
        Err(HostLifetimeError::Unavailable)
    );
    assert_eq!(
        f.owner.publish_gateway_ready(&mut || {
            calls += 1;
            Ok(())
        }),
        Err(HostLifetimeError::Unavailable)
    );
    assert_eq!(calls, 0);
    drop(exclusive);
    f.owner
        .validate_source_selection(&source, &association)
        .unwrap();
    f.owner
        .publish_gateway_ready(&mut || {
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 1);
}

#[test]
fn selected_source_requires_open_component_and_observes_current_deny() {
    let f = Fixture::new();
    let source = f.source();
    let association = f.association();
    assert_eq!(
        f.owner.validate_source_selection(&source, &association),
        Err(HostLifetimeError::Denied)
    );
    f.open();
    f.owner
        .validate_source_selection(&source, &association)
        .unwrap();
    let deny = f.selection.deployment_root.join("shared/control/deny.json");
    std::fs::write(
        &deny,
        br#"{"revision":1,"denied":["channel:slack"],"applied_unlogged":{}}"#,
    )
    .unwrap();
    assert_eq!(
        f.owner.validate_source_selection(&source, &association),
        Err(HostLifetimeError::Denied)
    );
    std::fs::write(
        &deny,
        br#"{"revision":2,"denied":[],"applied_unlogged":{}}"#,
    )
    .unwrap();
    f.owner
        .validate_source_selection(&source, &association)
        .unwrap();
    f.owner
        .begin_close_component(
            &Id::new("slack-extension").unwrap(),
            HostCloseReason::Quarantine,
        )
        .unwrap();
    assert_eq!(
        f.owner.validate_source_selection(&source, &association),
        Err(HostLifetimeError::Denied)
    );
}

#[test]
fn selected_source_refuses_mismatched_labels_and_qualified_native_participants() {
    let f = Fixture::new();
    f.open();
    let source = f.source();
    let association = f.association();
    f.owner
        .validate_source_selection(&source, &association)
        .unwrap();
    for field in 0..9 {
        let mut changed = source.clone();
        match field {
            0 => changed.source.channel_id = "other".into(),
            1 => changed.source.extension_id = "other".into(),
            2 => changed.source.service_subject = "other".into(),
            3 => changed.source.provider_account = "other".into(),
            4 => changed.source.sender_id = "other".into(),
            5 => changed.source.requester = "other".into(),
            6 => changed.target_identity = "other".into(),
            7 => changed.source.policy_digest = "other".into(),
            _ => changed.source.requester = "person:original@another-domain".into(),
        }
        assert_eq!(
            f.owner.validate_source_selection(&changed, &association),
            Err(HostLifetimeError::Denied),
            "source field {field}"
        );
    }
    for field in 0..4 {
        let mut changed = association.candidate().clone();
        match field {
            0 => {
                changed.requester = PrincipalRef::in_domain(
                    PrincipalKind::Human,
                    "person:original",
                    TrustDomainId::new("other-household").unwrap(),
                )
                .unwrap()
            }
            1 => {
                changed.ingress_actor = principal(PrincipalKind::ServiceAccount, "another-ingress")
            }
            2 => {
                changed.ingress_namespace.realm = meerkat_core::mob_realm_id("another-mob").unwrap()
            }
            _ => changed.target.context_generation += 1,
        }
        let changed = InputAuthorityAssociation::new(changed).unwrap();
        assert_eq!(
            f.owner.validate_source_selection(&source, &changed),
            Err(HostLifetimeError::Denied),
            "association field {field}"
        );
    }
}

#[test]
fn current_selection_refuses_changed_artifact_or_missing_activation() {
    let f = Fixture::new();
    f.owner.validate_current_selection().unwrap();
    let policy_path = f
        .selection
        .local_policy
        .path
        .parent()
        .unwrap()
        .join("policy.json");
    std::fs::write(policy_path, br#"{"schema":2}"#).unwrap();
    assert_eq!(
        f.owner.validate_current_selection(),
        Err(HostLifetimeError::InvalidSelection)
    );
    std::fs::remove_file(f.selection.deployment_root.join("current")).unwrap();
    assert_eq!(
        f.owner.validate_current_selection(),
        Err(HostLifetimeError::Unavailable)
    );
}

#[test]
fn selected_member_destination_refuses_other_member_realm_or_duplicate_members() {
    let f = Fixture::new();
    let row = &f.owner.selected_policy().channels[0];
    assert_eq!(
        f.owner
            .destination_for_member(&row.realm, &row.member)
            .unwrap(),
        f.owner
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .members[0]
    );
    let mut other = row.member.clone();
    other.member = "mk--other".into();
    assert_eq!(
        f.owner.destination_for_member(&row.realm, &other),
        Err(HostLifetimeError::Denied)
    );
    assert_eq!(
        f.owner
            .destination_for_member(&meerkat_core::mob_realm_id("other").unwrap(), &row.member),
        Err(HostLifetimeError::Denied)
    );
    let mut policy = f.owner.selected_policy().clone();
    let native = policy.native_host.as_mut().unwrap();
    let mut other = native.members[0].clone();
    other.context_generation += 1;
    native.members.push(other);
    assert_eq!(policy.validate(), Err(HostLifetimeError::InvalidSelection));
    let compiled = serde_json::to_value(f.owner.selected_policy()).unwrap();
    let destination = &compiled["native_host"]["members"][0];
    assert!(destination.get("context").is_none());
    assert!(destination.get("logical_runtime").is_none());
    assert!(compiled["channels"][0].get("destination").is_none());
}

#[test]
fn bootstrap_and_policy_reject_legacy_authority_fields() {
    let f = Fixture::new();
    for field in ["inbox_path", "issuer_id", "issuer_key_path"] {
        let mut value = serde_json::to_value(&f.selection).unwrap();
        value[field] = serde_json::json!("legacy");
        assert!(
            serde_json::from_value::<HostBootstrapSelection>(value).is_err(),
            "{field}"
        );
    }
    let policy = f.owner.selected_policy();
    let mut legacy_tenant = serde_json::to_value(policy).unwrap();
    legacy_tenant["tenant"] = serde_json::json!("household");
    assert!(serde_json::from_value::<policy::CompiledLocalPolicy>(legacy_tenant).is_err());
    let mut old_initializer = serde_json::to_value(policy).unwrap();
    old_initializer["native_host"]["initialize_grant_owner"] = serde_json::json!({
        "root": policy.native_host.as_ref().unwrap().grant_administrator,
        "namespace": "old-app-root", "generation": 1
    });
    assert!(serde_json::from_value::<policy::CompiledLocalPolicy>(old_initializer).is_err());
    let mut old = policy.clone();
    for schema in [1, 2, 3, 4] {
        old.schema = schema;
        assert_eq!(old.validate(), Err(HostLifetimeError::InvalidSelection));
    }
    let mut old_target = serde_json::to_value(policy).unwrap();
    old_target["channels"][0]["destination"] =
        serde_json::to_value(f.association().candidate().target.clone()).unwrap();
    assert!(serde_json::from_value::<policy::CompiledLocalPolicy>(old_target).is_err());
    // Removed channel fields must not survive as apparent access controls.
    // Real native restrictions and approved access lifetimes remain separate.
    for (field, value) in [
        (
            "ceiling",
            serde_json::json!({
                "actions": [], "services": [], "frontends": [], "output_audiences": []
            }),
        ),
        ("lifetime_ms", serde_json::json!(60_000)),
    ] {
        let mut old = serde_json::to_value(policy).unwrap();
        old["channels"][0][field] = value;
        assert!(
            serde_json::from_value::<policy::CompiledLocalPolicy>(old).is_err(),
            "{field}"
        );
    }
    for field in ["max_hops", "local_checkpoint", "local_read", "local_stage"] {
        let mut value = serde_json::to_value(policy).unwrap();
        value["channels"][0][field] = serde_json::json!(false);
        assert!(
            serde_json::from_value::<policy::CompiledLocalPolicy>(value).is_err(),
            "{field}"
        );
    }
}

#[test]
fn physical_selection_fence_refuses_symlink_and_hardlink_aliases() {
    let f = Fixture::new();
    let directory = f.selection.deployment_root.join("shared/control");
    let lock = directory.join("deny.lock");
    let symlink = directory.join("alias.lock");
    std::os::unix::fs::symlink(&lock, &symlink).unwrap();
    assert!(matches!(
        LocalFence::open(symlink),
        Err(HostLifetimeError::InvalidSelection)
    ));
    let hardlink = directory.join("hard.lock");
    std::fs::hard_link(&lock, &hardlink).unwrap();
    assert!(matches!(
        LocalFence::open(hardlink),
        Err(HostLifetimeError::InvalidSelection)
    ));
    assert_eq!(
        f.owner.validate_current_selection(),
        Err(HostLifetimeError::InvalidSelection)
    );
}

#[test]
fn bounded_operation_owns_both_actual_close_state_mutexes_through_release() {
    let f = Fixture::new();
    f.open();
    let component = Id::new("slack-extension").unwrap();
    let mut released = None;
    f.owner
        .with_component_open(&component, None, None, || {
            assert!(f.owner.activation.entry.try_write().is_err());
            assert!(f.owner.activation.machine.try_write().is_ok());
            assert!(
                f.owner
                    .component(&component)
                    .unwrap()
                    .entry
                    .try_write()
                    .is_err()
            );
            assert!(
                f.owner
                    .component(&component)
                    .unwrap()
                    .machine
                    .try_write()
                    .is_ok()
            );
            let mut recursive = false;
            assert_eq!(
                f.owner
                    .with_component_open(&component, None, None, || recursive = true),
                Err(HostLifetimeError::Unavailable)
            );
            assert!(!recursive);
            released = Some("exact private result");
        })
        .unwrap();
    assert_eq!(released, Some("exact private result"));
    f.owner
        .begin_close_component(&component, HostCloseReason::OperatorDisable)
        .unwrap();
    let mut late = false;
    assert_eq!(
        f.owner
            .with_component_open(&component, None, None, || late = true),
        Err(HostLifetimeError::Denied)
    );
    assert!(!late);
}

#[test]
fn synchronous_setup_fence_uses_current_activation_and_selection() {
    let f = Fixture::new();
    assert_eq!(
        f.owner
            .with_current_setup(|| {
                // Entry excludes close/callbacks while the actual phase remains readable.
                assert!(f.owner.activation.entry.try_write().is_err());
                assert!(f.owner.activation.machine.try_write().is_ok());
                Ok(17)
            })
            .unwrap(),
        17
    );
    f.owner
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    let called = std::cell::Cell::new(false);
    assert!(
        f.owner
            .with_current_setup(|| {
                called.set(true);
                Ok(())
            })
            .is_err()
    );
    assert!(!called.get());

    let f = Fixture::new();
    std::fs::write(&f.selection.local_policy.path, b"changed selected policy").unwrap();
    assert!(
        f.owner
            .with_current_setup(|| {
                called.set(true);
                Ok(())
            })
            .is_err()
    );
    assert!(!called.get());
}

#[test]
fn synchronous_setup_rejects_concurrent_and_recursive_entry() {
    use std::sync::mpsc::sync_channel;
    let mut f = Fixture::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(LocalAuthorizationPublication::new())
        .unwrap();
    let (entered, observed) = sync_channel(1);
    let (resume, resumed) = sync_channel(1);
    let owner = f.owner.clone();
    let worker = std::thread::spawn(move || {
        owner.with_current_setup(|| {
            let mut recursive_called = false;
            let recursive = owner.with_current_setup(|| {
                recursive_called = true;
                Ok(())
            });
            let native_entry = owner.validate_native_entry(None);
            entered
                .send((recursive, recursive_called, native_entry))
                .unwrap();
            resumed.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(17)
        })
    });
    let recursive = observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut concurrent_called = false;
    let concurrent = f.owner.with_current_setup(|| {
        concurrent_called = true;
        Ok(())
    });
    let native_entry = f.owner.validate_native_entry(None);
    let selected = f.owner.validate_current_selection();
    resume.send(()).unwrap();
    let result = worker.join().unwrap();
    // Callback recursion remains refused; this closed phase observation is
    // permitted because it invokes no callback and changes no authority.
    assert_eq!(
        recursive,
        (Err(HostLifetimeError::Unavailable), false, Ok(()))
    );
    assert_eq!(concurrent, Err(HostLifetimeError::Unavailable));
    assert!(!concurrent_called);
    assert_eq!(native_entry, Ok(()));
    assert_eq!(selected, Ok(()));
    assert_eq!(result, Ok(17));
    assert_eq!(f.owner.validate_native_entry(None), Ok(()));
}

#[test]
fn current_selection_and_native_entry_share_phase_readers() {
    use std::sync::mpsc::sync_channel;

    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    // The actual current-selection path and publication entry must both read
    // Open while an independent read-only phase observation remains held.
    let (entered, observed) = sync_channel(1);
    f.owner.activation.entered_for_test.set(entered).unwrap();
    let reader = f.owner.activation.machine.read().unwrap();
    let (finished, results) = sync_channel(1);
    let owner = f.owner.clone();
    let worker = std::thread::spawn(move || {
        let selection = owner.validate_current_selection();
        let entry = publication
            .observe(|| owner.validate_native_entry(None))
            .map(|(entry, current)| (entry, current.check_current().is_ok()));
        finished.send((selection, entry)).unwrap();
    });
    let entry_held = observed.recv_timeout(Duration::from_secs(2));
    let while_reading = results.recv_timeout(Duration::from_secs(2));
    let completed_while_reading = while_reading.is_ok();
    drop(reader);
    let result = match while_reading {
        Ok(result) => Some(result),
        Err(_) => results.recv_timeout(Duration::from_secs(2)).ok(),
    };
    let joined = worker.join();
    // Release the held guard and join before every assertion, including when
    // a regression blocks or refuses instead of sharing the actual read.
    assert!(
        entry_held.is_ok(),
        "native entry did not acquire its phase read"
    );
    assert!(
        completed_while_reading,
        "current selection waited for a phase reader"
    );
    joined.unwrap();
    assert!(matches!(result, Some((Ok(()), Ok((Ok(()), true))))));

    // An actual exclusive phase writer still makes retain refuse immediately;
    // no waiting, retry or cached phase is introduced under physical custody.
    let writer = f.owner.activation.machine.write().unwrap();
    let (returned, refusal) = sync_channel(1);
    let owner = f.owner.clone();
    let worker = std::thread::spawn(move || {
        returned
            .send((
                owner.validate_current_selection(),
                owner.current_setup_policy(),
                owner.validate_native_entry(None),
            ))
            .unwrap();
    });
    let refused = refusal.recv_timeout(Duration::from_secs(2));
    drop(writer);
    let joined = worker.join();
    joined.unwrap();
    assert_eq!(
        refused,
        Ok((
            Err(HostLifetimeError::Unavailable),
            Err(HostLifetimeError::Unavailable),
            Err(HostLifetimeError::Unavailable)
        ))
    );
    assert_eq!(f.owner.validate_current_selection(), Ok(()));
    assert_eq!(f.owner.validate_native_entry(None), Ok(()));
}

#[tokio::test]
async fn closed_policy_and_native_observations_share_phase_then_join_close() {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};

    let mut f = Fixture::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(LocalAuthorizationPublication::new())
        .unwrap();
    let component = Id::new("slack-extension").unwrap();
    f.open();
    let expected = f.owner.selected_policy().clone();
    // Hold the actual internal read custody, not a phase snapshot. Production
    // readers only validate/copy and never invoke a callback or await.
    let activation_read = f
        .owner
        .activation
        .observe_open(f.owner.incarnation, None)
        .unwrap();
    let component_owner = f.owner.component(&component).unwrap();
    let component_read = component_owner
        .observe_open(f.owner.incarnation, None)
        .unwrap();
    let (returned, response) = sync_channel(1);
    let owner = f.owner.clone();
    let observed_component = component.clone();
    let observer = std::thread::spawn(move || {
        let policy = owner.current_setup_policy();
        let native = owner.validate_native_entry(Some(&observed_component));
        let mut mutated = false;
        let mutation = owner.with_current_setup(|| {
            mutated = true;
            Ok(())
        });
        returned.send((policy, native, mutation, mutated)).unwrap();
    });
    let simultaneous = response.recv_timeout(Duration::from_secs(2));

    let (started, starting) = sync_channel(1);
    let (closed, closing) = sync_channel(1);
    let owner = f.owner.clone();
    let closer = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = owner.begin_close_activation(HostCloseReason::Shutdown);
        closed.send(result).unwrap();
    });
    let close_started = starting.recv_timeout(Duration::from_secs(2));
    let early_close = closing.recv_timeout(Duration::from_millis(100));
    let waited_for_reader = matches!(&early_close, Err(RecvTimeoutError::Timeout));
    // No assertion can strand either real worker behind a held read guard.
    // Standard lock fairness is not part of this control's claim.
    drop(component_read);
    drop(activation_read);
    let observer_joined = observer.join();
    let close_result = match early_close {
        Ok(result) => Some(result),
        Err(_) => closing.recv_timeout(Duration::from_secs(2)).ok(),
    };
    let closer_joined = closer.join();
    let close = close_result.unwrap().unwrap();
    let closing_policy = f.owner.current_setup_policy();
    let closing_native = f.owner.validate_native_entry(Some(&component));
    // Settle the actual close before assertions that the normalized exclusive
    // comparison is expected to fail. Keep both Closing and Settled checks.
    let settlement = f.owner.join_close(&close.handle).await;
    let settled_policy = f.owner.current_setup_policy();
    observer_joined.unwrap();
    closer_joined.unwrap();
    assert!(close_started.is_ok());
    assert!(waited_for_reader);
    assert_eq!(closing_policy, Err(HostLifetimeError::Denied));
    assert_eq!(closing_native, Err(HostLifetimeError::Denied));
    assert_eq!(settlement.unwrap().state, HostCloseState::Settled);
    assert_eq!(settled_policy, Err(HostLifetimeError::Denied));
    let (policy, native, mutation, mutated) = simultaneous.unwrap();
    assert_eq!(policy, Ok(expected));
    assert_eq!(native, Ok(()));
    // A pure phase reader does not own callback entry. The callback above is
    // synchronous and cannot change phase while these machine reads are held.
    assert_eq!(mutation, Ok(()));
    assert!(mutated);
}

#[test]
fn closed_policy_observation_shares_healthy_callback_and_refuses_replaced_selection() {
    let f = Fixture::new();
    let expected = f.owner.current_setup_policy().unwrap();
    let during_mutation = f
        .owner
        .with_current_setup(|| Ok(f.owner.current_setup_policy()))
        .unwrap();
    assert_eq!(during_mutation, Ok(expected.clone()));
    assert_eq!(f.owner.current_setup_policy(), Ok(expected));
    std::fs::write(&f.selection.local_policy.path, b"replaced selected policy").unwrap();
    assert!(f.owner.current_setup_policy().is_err());
}

#[tokio::test]
async fn setup_callback_panic_keeps_owner_unavailable() {
    use std::future::Future;
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};
    use std::task::{Context, Poll, Waker};
    let mut f = Fixture::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(LocalAuthorizationPublication::new())
        .unwrap();
    let (entered, callback_entered) = sync_channel(1);
    let (release, released) = sync_channel(1);
    let callback_owner = f.owner.clone();
    let callback = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            callback_owner.with_current_setup(|| -> Result<()> {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                panic!("bounded setup callback panic")
            })
        }))
    });
    let callback_started = callback_entered.recv_timeout(Duration::from_secs(2));
    let (poll_started, poll_observed) = sync_channel(1);
    let (completed, completion_observed) = sync_channel(1);
    let observer_owner = f.owner.clone();
    let observer = std::thread::spawn(move || {
        let mut wait = std::pin::pin!(observer_owner.wait_closed());
        let mut context = Context::from_waker(Waker::noop());
        // Signal immediately before the actual first poll. The bounded
        // interval below lets this observer enter while the callback is held.
        poll_started.send(()).unwrap();
        let result = wait.as_mut().poll(&mut context);
        completed.send(result).unwrap();
        // A poison-only observation would return Pending while still Open.
        // Drop that future, rather than leaving a regressed observer parked.
        result
    });
    let observer_started = poll_observed.recv_timeout(Duration::from_secs(2));
    let early = completion_observed.recv_timeout(Duration::from_millis(100));
    let release_result = release.send(());
    let callback_result = callback.join();
    let observer_result = observer.join();
    // Release and join both actual threads before every final assertion.
    assert!(callback_started.is_ok());
    assert!(observer_started.is_ok());
    assert!(release_result.is_ok());
    assert!(matches!(early, Err(RecvTimeoutError::Timeout)));
    assert!(callback_result.unwrap().is_err());
    assert!(
        matches!(observer_result, Ok(Poll::Ready(()))),
        "the observer begun during setup must finish when callback entry is poisoned"
    );
    assert!(f.owner.activation.entry.is_poisoned());
    assert!(!f.owner.activation.machine.is_poisoned());
    assert_eq!(
        f.owner
            .activation
            .machine
            .read()
            .unwrap()
            .state()
            .lifetime_phase,
        ComponentLifetimePhase::Open
    );
    assert_eq!(
        f.owner.validate_current_selection(),
        Err(HostLifetimeError::Unavailable)
    );
    assert_eq!(
        f.owner.validate_native_entry(None),
        Err(HostLifetimeError::Unavailable)
    );
    assert_eq!(
        f.owner.current_setup_policy(),
        Err(HostLifetimeError::Unavailable)
    );
    let mut called = false;
    assert_eq!(
        f.owner.with_current_setup(|| {
            called = true;
            Ok(())
        }),
        Err(HostLifetimeError::Unavailable)
    );
    assert!(!called);
    assert!(matches!(
        f.owner.open_component(&Id::new("slack-extension").unwrap()),
        Err(HostLifetimeError::Unavailable)
    ));
    assert!(matches!(
        f.owner.begin_close_activation(HostCloseReason::Shutdown),
        Err(HostLifetimeError::Unavailable)
    ));
    // A poisoned owner wakes this observation, but cannot produce a close or
    // successful settlement receipt, as the refused close above establishes.
    tokio::time::timeout(Duration::from_secs(1), f.owner.wait_closed())
        .await
        .unwrap();
}

#[tokio::test]
async fn closing_owner_poison_never_fabricates_settlement() {
    // Inject actual synchronization poison after obtaining real close handles.
    // Cover both the released callback gate and the still-authoritative machine,
    // including the component observation inside activation join.
    for poison_entry in [false, true] {
        for poison_component in [false, true] {
            let f = Fixture::new();
            f.open();
            let closing = f
                .owner
                .begin_close_activation(HostCloseReason::Shutdown)
                .unwrap();
            let component = Id::new("slack-extension").unwrap();
            let component_closing = f
                .owner
                .begin_close_component(&component, HostCloseReason::Shutdown)
                .unwrap();
            let (owner, handle) = if poison_component {
                (
                    f.owner.component(&component).unwrap().clone(),
                    component_closing.handle,
                )
            } else {
                (f.owner.activation.clone(), closing.handle.clone())
            };
            let poisoned = owner.clone();
            assert!(
                std::thread::spawn(move || {
                    if poison_entry {
                        let _guard = poisoned.entry.write().unwrap();
                        panic!("closed owner entry poison");
                    } else {
                        let _guard = poisoned.machine.write().unwrap();
                        panic!("closed owner phase poison");
                    }
                })
                .join()
                .is_err()
            );
            assert!(matches!(
                owner.observe(&handle, f.owner.native_publication.as_ref()),
                Err(HostLifetimeError::Unavailable)
            ));
            assert!(matches!(
                f.owner.observe_close(&handle),
                Err(HostLifetimeError::Unavailable)
            ));
            assert!(matches!(
                owner
                    .join(&handle, f.owner.native_publication.as_ref())
                    .await,
                Err(HostLifetimeError::Unavailable)
            ));
            assert!(matches!(
                f.owner.join_close(&closing.handle).await,
                Err(HostLifetimeError::Unavailable)
            ));
        }
    }
}

#[test]
fn native_publication_tracks_actual_component_and_activation_close() {
    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    let component = Id::new("slack-extension").unwrap();
    f.open();
    let (entry, prepared) = publication
        .observe(|| f.owner.validate_native_entry(Some(&component)))
        .unwrap();
    entry.unwrap();
    f.owner
        .begin_close_component(&component, HostCloseReason::OperatorDisable)
        .unwrap();
    assert!(
        prepared.check_current().is_err(),
        "an older native projection cannot survive the actual phase change"
    );
    let (entry, _) = publication
        .observe(|| f.owner.validate_native_entry(Some(&component)))
        .unwrap();
    assert!(matches!(entry, Err(HostLifetimeError::Denied)));
    assert!(
        f.owner.validate_native_entry(None).is_ok(),
        "component close does not pretend the whole activation closed"
    );
    let (_, prepared) = publication
        .observe(|| f.owner.validate_native_entry(None))
        .unwrap();
    f.owner
        .begin_close_activation(HostCloseReason::ActivationReplaced)
        .unwrap();
    assert!(prepared.check_current().is_err());
    let (entry, _) = publication
        .observe(|| f.owner.validate_native_entry(None))
        .unwrap();
    assert!(matches!(entry, Err(HostLifetimeError::Denied)));
}

#[tokio::test]
async fn host_loss_closes_activation_before_waiting_for_component_without_holding_publication() {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};

    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    f.open();
    let (_, prepared) = publication
        .observe(|| f.owner.validate_native_entry(None))
        .unwrap();
    let component = Id::new("slack-extension").unwrap();
    let held = f
        .owner
        .component(&component)
        .unwrap()
        .machine
        .write()
        .unwrap();
    let (finished, result) = sync_channel(1);
    let owner = f.owner.clone();
    let worker = std::thread::spawn(move || {
        finished.send(owner.host_lost()).unwrap();
    });
    // The actual activation transition, not a thread-start flag, proves that
    // close entered. It must close ingress before waiting on the held component.
    let entry_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while prepared.check_current().is_ok() && std::time::Instant::now() < entry_deadline {
        std::thread::yield_now();
    }
    let entered_close = prepared.check_current().is_err();
    if !entered_close {
        drop(held);
        worker.join().unwrap();
        panic!("host loss did not publish its activation close");
    }
    // Publication starts before the phase transition releases its mutex.
    // A transient Unavailable is therefore not evidence of a failed close.
    let phase_deadline = std::time::Instant::now() + Duration::from_secs(2);
    let activation_closed = loop {
        match f.owner.validate_native_entry(None) {
            Err(HostLifetimeError::Denied) => break true,
            Ok(()) | Err(HostLifetimeError::Unavailable)
                if std::time::Instant::now() < phase_deadline =>
            {
                std::thread::yield_now();
            }
            _ => break false,
        }
    };
    if !activation_closed {
        drop(held);
        worker.join().unwrap();
        panic!("activation did not finish closing before its held component");
    }
    let while_held = result.recv_timeout(Duration::from_millis(100));

    // A real native writer must remain obtainable while another component's
    // phase is held. The former outer close publication blocks this writer,
    // reproducing the other half of the setup/close lock inversion.
    let (published, observed) = sync_channel(1);
    let probe_publication = publication.clone();
    let writer = std::thread::spawn(move || {
        let change = probe_publication.begin_owner_change().map(drop);
        published.send(change).unwrap();
    });
    let writer_while_held = observed.recv_timeout(Duration::from_secs(2));
    drop(held);
    let closing = match while_held {
        Err(RecvTimeoutError::Timeout) => result.recv_timeout(Duration::from_secs(2)).unwrap(),
        premature => {
            worker.join().unwrap();
            writer.join().unwrap();
            panic!("host loss abandoned its held component: {premature:?}");
        }
    }
    .unwrap();
    worker.join().unwrap();
    writer.join().unwrap();
    assert!(matches!(writer_while_held, Ok(Ok(()))));
    assert_eq!(closing.state, HostCloseState::Closing);
    let settled = f.owner.join_close(&closing.handle).await.unwrap();
    assert_eq!(settled.state, HostCloseState::Settled);
    assert!(f.owner.components.values().all(|component| matches!(
        component.machine.read().unwrap().state().lifetime_phase,
        ComponentLifetimePhase::Closed
    )));
}

#[tokio::test]
async fn admitted_native_setup_issuance_finishes_before_close_and_close_first_refuses_new_issue() {
    use meerkat_authorization::{
        clock::HostAuthorizationClock,
        grants::{
            DurableGrantOwner, FileGrantOwnerStore, GrantOwnerChange, GrantOwnerIdentity,
            GrantOwnerStore, GrantStoreError, LocalGrantAuthority, LocalGrantConfiguration,
        },
    };
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};

    // Pause the actual native durable append, after issue_root owns both its
    // generated owner and publication. All data/lease semantics remain those
    // of the actual FileGrantOwnerStore; no fake grant is constructed.
    struct PausedAppend {
        inner: FileGrantOwnerStore,
        entered: SyncSender<()>,
        resume: Mutex<Receiver<()>>,
    }
    impl GrantOwnerStore for PausedAppend {
        fn load(&self) -> std::result::Result<Option<DurableGrantOwner>, GrantStoreError> {
            self.inner.load()
        }
        fn create(
            &self,
            identity: &GrantOwnerIdentity,
        ) -> std::result::Result<(), GrantStoreError> {
            self.inner.create(identity)
        }
        fn append(&self, change: &GrantOwnerChange) -> std::result::Result<(), GrantStoreError> {
            self.entered
                .send(())
                .map_err(|_| GrantStoreError::Refused)?;
            self.resume
                .lock()
                .map_err(|_| GrantStoreError::Refused)?
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| GrantStoreError::Refused)?;
            self.inner.append(change)
        }
        fn release(&self) {
            self.inner.release();
        }
    }

    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    f.open();
    let (entered, observed) = sync_channel(1);
    let (resume, resumed) = sync_channel(1);
    let store = Arc::new(PausedAppend {
        inner: FileGrantOwnerStore::new(f._directory.path().join("setup-grants")),
        entered,
        resume: Mutex::new(resumed),
    });
    let root = principal(PrincipalKind::Human, "operator");
    let application = principal(PrincipalKind::ServiceAccount, "application");
    let grants = Arc::new(
        LocalGrantAuthority::new_durable(
            LocalGrantConfiguration {
                root: root.clone(),
                namespace: id("setup-close-test"),
                generation: 1,
            },
            store.clone(),
            publication,
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let (issued, issue_result) = sync_channel(1);
    let setup_owner = f.owner.clone();
    let setup_grants = grants.clone();
    let setup_root = root.clone();
    let setup_application = application.clone();
    let setup_worker = std::thread::spawn(move || {
        let result = setup_owner.with_current_setup(|| {
            setup_grants
                .issue_root(
                    &setup_root,
                    id("admitted-before-close"),
                    setup_application,
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .map_err(|_| HostLifetimeError::Unavailable)
        });
        issued.send(result).unwrap();
    });
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    // Read the actual selected owner while the real native durable append is
    // paused. Keep assertions until the original workers have been released.
    let selected_while_issuing = f.owner.validate_current_selection();
    let entry_held = f.owner.activation.entry.try_write().is_err();
    let phase_readable = f.owner.activation.machine.try_write().is_ok();
    let (closed, close_result) = sync_channel(1);
    let close_owner = f.owner.clone();
    let close_worker = std::thread::spawn(move || {
        closed.send(close_owner.host_lost()).unwrap();
    });
    let while_issuing = close_result.recv_timeout(Duration::from_millis(100));
    resume.send(()).unwrap();
    let issued = issue_result.recv_timeout(Duration::from_secs(2)).unwrap();
    let closing = match while_issuing {
        Err(RecvTimeoutError::Timeout) => {
            close_result.recv_timeout(Duration::from_secs(2)).unwrap()
        }
        premature => {
            setup_worker.join().unwrap();
            close_worker.join().unwrap();
            panic!("close passed or abandoned admitted native issuance: {premature:?}");
        }
    }
    .unwrap();
    setup_worker.join().unwrap();
    close_worker.join().unwrap();
    assert_eq!(selected_while_issuing, Ok(()));
    assert!(entry_held, "entered issuance still owns the close boundary");
    assert!(
        phase_readable,
        "status reads the same generated phase during issuance"
    );
    let issued = issued.unwrap();
    grants
        .resolve_lineage(&[issued], &application, None)
        .unwrap();
    let called = std::cell::Cell::new(false);
    assert!(matches!(
        f.owner.with_current_setup(|| {
            called.set(true);
            grants
                .issue_root(
                    &root,
                    id("refused-after-close"),
                    application,
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .map_err(|_| HostLifetimeError::Unavailable)
        }),
        Err(HostLifetimeError::Denied)
    ));
    assert!(!called.get());
    assert_eq!(
        f.owner.join_close(&closing.handle).await.unwrap().state,
        HostCloseState::Settled
    );
    drop(grants);
    let retained = store.load().unwrap().unwrap();
    assert_eq!(retained.changes.len(), 1);
    store.release();
}

#[test]
fn schema7_operator_is_explicit_and_cannot_be_inferred_from_application() {
    let fixture = Fixture::new();
    let policy = fixture.owner.selected_policy().clone();
    let mut value = serde_json::to_value(&policy).unwrap();
    assert!(value["native_host"]["remote_operator"].is_null());
    assert_eq!(
        policy.canonical_bytes().unwrap(),
        canonical_json(&policy).unwrap()
    );
    value["schema"] = serde_json::json!(6);
    let old: policy::CompiledLocalPolicy = serde_json::from_value(value).unwrap();
    assert!(old.canonical_bytes().is_err());
    let mut same = policy;
    let native = same.native_host.as_mut().unwrap();
    native.remote_operator = Some(native.grant_administrator.clone());
    assert!(same.canonical_bytes().is_err());
}

#[test]
fn synchronous_consent_keeps_selected_file_fence_but_releases_phase() {
    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    let (_, current) = publication
        .observe(|| f.owner.validate_native_entry(None))
        .unwrap();
    let actual = f
        .owner
        .with_current_consent(|| {
            assert!(
                f.owner.deny_fence.exclusive().is_err(),
                "selected files remain reserved"
            );
            assert!(
                f.owner.activation.machine.try_write().is_ok(),
                "decision does not retain a phase guard"
            );
            assert!(f.owner.validate_native_entry(None).is_ok());
            assert!(current.check_current().is_ok());
            // Close is not held behind a human/decision callback. Native final
            // publication checking, not this file reservation, refuses stale work.
            f.owner
                .begin_close_activation(HostCloseReason::OperatorDisable)
                .unwrap();
            assert!(current.check_current().is_err());
            17
        })
        .unwrap();
    assert_eq!(actual, 17);
    assert!(
        f.owner.deny_fence.exclusive().is_ok(),
        "reservation released after synchronous result"
    );
    let called = std::cell::Cell::new(false);
    assert!(f.owner.with_current_consent(|| called.set(true)).is_err());
    assert!(!called.get());

    let mut changed = Fixture::new();
    Arc::get_mut(&mut changed.owner)
        .unwrap()
        .bind_native_authorization_publication(LocalAuthorizationPublication::new())
        .unwrap();
    std::fs::write(
        &changed.selection.local_policy.path,
        b"changed selected policy",
    )
    .unwrap();
    assert!(
        changed
            .owner
            .with_current_consent(|| called.set(true))
            .is_err()
    );
    assert!(!called.get());
    let unbound = Fixture::new();
    assert!(
        unbound
            .owner
            .with_current_consent(|| called.set(true))
            .is_err()
    );
    assert!(!called.get());
}

#[test]
fn native_administration_entry_holds_close_fence_and_refuses_closed_or_contended_owner() {
    let f = Fixture::new();
    // No grant-publication stamp is used as a substitute for the actual entry
    // fence. Native administration supplies its own writer custody and policy.
    assert!(f.owner.native_publication.is_none());
    let reservation = Arc::new(());
    let retained = f
        .owner
        .with_native_administration_entry(|| {
            assert!(f.owner.activation.entry.try_write().is_err());
            assert!(f.owner.activation.machine.try_write().is_ok());
            let mut recursive = false;
            assert_eq!(
                f.owner
                    .with_native_administration_entry(|| recursive = true),
                Err(HostLifetimeError::Unavailable)
            );
            assert!(!recursive);
            reservation.clone()
        })
        .unwrap();
    assert!(Arc::ptr_eq(&reservation, &retained));
    let guard = f.owner.activation.entry.write().unwrap();
    let mut called = false;
    let contended = f.owner.with_native_administration_entry(|| called = true);
    drop(guard);
    assert_eq!(contended, Err(HostLifetimeError::Unavailable));
    assert!(!called);
    // Close wins before entry: the callback cannot capture new custody.
    f.owner
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    assert_eq!(
        f.owner.with_native_administration_entry(|| called = true),
        Err(HostLifetimeError::Denied)
    );
    assert!(!called);
    // An entry that already won returned its actual custody to the caller.
    // Same-login retirement/drain is covered by the login owner's control.
    assert_eq!(Arc::strong_count(&retained), 2);
    drop(retained);
    assert_eq!(Arc::strong_count(&reservation), 1);
}

#[tokio::test]
async fn healthy_setup_callback_shares_phase_observation_without_sharing_callback_entry() {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};

    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    let component = Id::new("slack-extension").unwrap();
    f.open();
    let expected_policy = f.owner.selected_policy().clone();
    let (entered, entering) = sync_channel(1);
    let (release, released) = sync_channel(1);
    let callback_owner = f.owner.clone();
    let callback = std::thread::spawn(move || {
        callback_owner.with_current_setup(|| {
            let mut called = false;
            let recursive = callback_owner.with_current_setup(|| {
                called = true;
                Ok(())
            });
            entered.send((recursive, called)).unwrap();
            // Hold the real callback entry before any native issuance. This
            // control covers the healthy branch only, not unwind currentness.
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
    });
    let callback_entered = entering.recv_timeout(Duration::from_secs(2));
    let (returned, response) = sync_channel(1);
    let observer_owner = f.owner.clone();
    let observed_component = component.clone();
    let observer = std::thread::spawn(move || {
        let native = publication
            .observe(|| observer_owner.validate_native_entry(Some(&observed_component)))
            .map(|(result, stamp)| (result, stamp.check_current().is_ok()));
        let policy = observer_owner.current_setup_policy();
        let mut called = false;
        let callback = observer_owner.with_current_setup(|| {
            called = true;
            Ok(())
        });
        returned.send((native, policy, callback, called)).unwrap();
    });
    let observed = response.recv_timeout(Duration::from_secs(2));
    let (starting, started) = sync_channel(1);
    let (closed, closing) = sync_channel(1);
    let close_owner = f.owner.clone();
    let closer = std::thread::spawn(move || {
        starting.send(()).unwrap();
        closed
            .send(close_owner.begin_close_activation(HostCloseReason::Shutdown))
            .unwrap();
    });
    let close_started = started.recv_timeout(Duration::from_secs(2));
    let before_release = closing.recv_timeout(Duration::from_millis(100));
    let close_waited = matches!(&before_release, Err(RecvTimeoutError::Timeout));
    // Neither a refused old reader nor a failed response may strand the real
    // callback or close. Release and join all owners before success assertions.
    let release_result = release.send(());
    let callback_result = callback.join();
    let observer_result = observer.join();
    let close_result = match before_release {
        Ok(result) => Some(result),
        Err(_) => closing.recv_timeout(Duration::from_secs(2)).ok(),
    };
    let closer_result = closer.join();
    let close = close_result.unwrap().unwrap();
    let settlement = f.owner.join_close(&close.handle).await;
    let after_close = f.owner.validate_native_entry(Some(&component));

    assert_eq!(
        callback_entered.unwrap(),
        (Err(HostLifetimeError::Unavailable), false)
    );
    assert!(close_started.is_ok());
    assert!(
        close_waited,
        "close must remain excluded by the actual callback owner"
    );
    assert!(release_result.is_ok());
    assert_eq!(callback_result.unwrap(), Ok(()));
    observer_result.unwrap();
    closer_result.unwrap();
    assert_eq!(settlement.unwrap().state, HostCloseState::Settled);
    assert_eq!(after_close, Err(HostLifetimeError::Denied));
    let (native, policy, callback, called) = observed.unwrap();
    assert!(
        matches!(&native, Ok((Ok(()), true))),
        "healthy callback hid current native phase facts: {native:?}"
    );
    assert_eq!(policy, Ok(expected_policy));
    assert_eq!(callback, Err(HostLifetimeError::Unavailable));
    assert!(!called);
}

#[tokio::test]
async fn phase_observation_refuses_actual_phase_writer_and_invalid_phase_identity() {
    use std::sync::mpsc::sync_channel;

    let mut f = Fixture::new();
    let publication = LocalAuthorizationPublication::new();
    Arc::get_mut(&mut f.owner)
        .unwrap()
        .bind_native_authorization_publication(publication.clone())
        .unwrap();
    let component = Id::new("slack-extension").unwrap();
    f.open();
    let revision = f.owner.activation.machine.read().unwrap().state().revision;
    assert!(
        f.owner
            .activation
            .observe_open(f.owner.incarnation, Some(revision))
            .is_ok()
    );
    assert_eq!(
        f.owner
            .activation
            .observe_open(Uuid::new_v4(), Some(revision))
            .err(),
        Some(HostLifetimeError::Denied)
    );
    assert_eq!(
        f.owner
            .activation
            .observe_open(f.owner.incarnation, Some(revision + 1))
            .err(),
        Some(HostLifetimeError::Denied)
    );

    // Hold the existing native writer. The real close takes entry and machine
    // write custody before it waits for this publication; no test-only phase
    // state or alternate close owner is manufactured.
    let native_writer = publication.begin_owner_change().unwrap();
    let (closed, closing) = sync_channel(1);
    let close_owner = f.owner.clone();
    let closer = std::thread::spawn(move || {
        closed
            .send(close_owner.begin_close_activation(HostCloseReason::Shutdown))
            .unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let phase_held = loop {
        if f.owner.activation.machine.try_read().is_err() {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::yield_now();
    };
    let (returned, response) = sync_channel(1);
    let observer_owner = f.owner.clone();
    let observer = std::thread::spawn(move || {
        returned
            .send((
                observer_owner.validate_native_entry(None),
                observer_owner.current_setup_policy(),
                observer_owner.validate_current_selection(),
            ))
            .unwrap();
    });
    let observed = response.recv_timeout(Duration::from_secs(2));
    // Drop the actual reservation before joins and assertions, including a
    // regression that waits for phase custody under native publication.
    drop(native_writer);
    let observer_result = observer.join();
    let close_result = closing.recv_timeout(Duration::from_secs(2));
    let closer_result = closer.join();
    let close = close_result.unwrap().unwrap();
    let settlement = f.owner.join_close(&close.handle).await;
    let after_close = f.owner.validate_native_entry(Some(&component));
    let policy_after_close = f.owner.current_setup_policy();

    observer_result.unwrap();
    closer_result.unwrap();
    assert!(phase_held, "real close did not acquire the phase writer");
    assert_eq!(
        observed.unwrap(),
        (
            Err(HostLifetimeError::Unavailable),
            Err(HostLifetimeError::Unavailable),
            Err(HostLifetimeError::Unavailable)
        )
    );
    assert_eq!(settlement.unwrap().state, HostCloseState::Settled);
    assert_eq!(after_close, Err(HostLifetimeError::Denied));
    assert_eq!(policy_after_close, Err(HostLifetimeError::Denied));
}

mod publication_poison;
