use crate::application_bootstrap::read_json;
use super::*;
use meerkat_core::{
    CompiledApplicationToolPolicy, CompiledMemberToolAction, CompiledMemberToolGrant,
    CompiledMemberToolGrants, CompiledPolicySourceProvenance, CompiledToolConsequence,
    PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision, PrincipalKind, PrincipalRef,
    TrustDomainId,
};
use serde_json::json;

const HANDOFF: &[u8] = br#"{"mcpServers":{"toolkit-setup":{"command":"/usr/bin/python3","args":["-I","-B","/retained/adapter.py"]}}}"#;

pub(super) fn input(root: &Path) -> ApplicationSetupInput {
    let principal = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "application",
        TrustDomainId::new("application-setup-fixture").unwrap(),
    )
    .unwrap();
    let helper_identity = "helper:setup";
    let alias = roster_member_id_for_identity(helper_identity);
    let member = MobMemberBinding {
        mob_id: "household".into(),
        role: "research".into(),
        member: alias.to_string(),
    };
    let definition = serde_json::from_value(json!({"id":"household","profiles":{
        "research":{"model":"claude-opus-4-8","provider":"anthropic","skills":["research"],
            "tools":{"shell":true,"comms":true,"mcp":["browser"],"deny":["remove_all"]}}
    }}))
    .unwrap();
    let roster = serde_json::from_value(json!([
        {"identity":"ordinary","profile":"research","labels":{"purpose":"unchanged"},
            "context":{"owner":"ordinary"},"additional_instructions":["Preserve me"]},
        {"identity":helper_identity,"profile":"research","binding":{"kind":"session"},"labels":{"purpose":"setup"},
            "context":{"owner":"setup"},"additional_instructions":["Research with selected tools"]}
    ]))
    .unwrap();
    let surface_bytes = toolkit_package::canonical_json(&vec![
        meerkat_core::ToolDef::new("research", "existing research tool", json!({})).identity(),
    ])
    .unwrap();
    let helper_source_surface = SelectedHostArtifact {
        path: root.join("helper-surface.json"),
        sha256: hash(&surface_bytes),
    };
    std::fs::write(&helper_source_surface.path, &surface_bytes).unwrap();
    let native_members = serde_json::from_value(json!([
        {"member":{"mob_id":"household","role":"research","member":"ordinary"},
            "logical_owner":principal,"context_generation":3,"audience":{"audience":"principal","principal":principal},
            "initial_surface":{"name":"ordinary-surface.json","sha256":"1".repeat(64)}},
        {"member":member,"logical_owner":principal,"context_generation":7,
            "audience":{"audience":"principal","principal":principal},
            "initial_surface":{"name":"helper-surface.json","sha256":helper_source_surface.sha256}}
    ])).unwrap();
    // Historical data-shape fixture only. No owner is initialized, no grant is
    // issued and these decoded references cannot authorize a native effect.
    let reference = json!({"root_authority":principal,"authority_namespace":"fixture",
        "authority_generation":1,"authority_incarnation":"3a5f9973-3a12-4977-9c69-b1002c559d51",
        "grant_id":"historical-selection","issued_revision":1});
    let helper = serde_json::from_value(json!({"member":member,"ingress_actor":principal,
        "logical_executor":principal,"represented_subject":null,"controller_parent":[reference],
        "operation_basis":{"kind":"grant_lineage","lineage":[reference]}}))
    .unwrap();
    ApplicationSetupInput {
        schema: 3,
        realm: ApplicationSetupRealm {
            realm_id: "household".into(),
            context_root: root.to_owned(),
            state_root: root.join("state"),
            user_config_root: Some(root.join("user")),
        },
        definition: ApplicationSetupDefinition::Native { definition },
        roster,
        native_members,
        helper,
        helper_source_surface,
        helper_tools: ApplicationSetupTools::InertAcquisition,
        helper_profile: ApplicationSetupProfile::Dedicated,
        tool_policy: ApplicationToolPolicyInput {
            provider_id: PolicyProviderId::new("application").unwrap(),
            policy_id: PolicyId::new("tools").unwrap(),
            revision: PolicyRevision(3),
            source: CompiledPolicySourceProvenance {
                source_id: "explicit-input".into(),
                source_digest: PolicyDigest::from_canonical_bytes(b"explicit-input"),
            },
            members: vec![
                CompiledMemberToolGrants {
                    member_identity: "ordinary".into(),
                    grants: vec![],
                },
                CompiledMemberToolGrants {
                    member_identity: alias.to_string(),
                    grants: vec![CompiledMemberToolGrant {
                        tool_name: meerkat_toolkit::setup::SETUP_REPORT.into(),
                        action: CompiledMemberToolAction::Invoke,
                        consequence: CompiledToolConsequence::R2,
                    }],
                },
            ],
        },
    }
}

pub(super) fn prepared(root: &Path) -> PreparedResources {
    let artifact_directory = root.join("retained-preparer");
    std::fs::create_dir(&artifact_directory).unwrap();
    std::fs::write(artifact_directory.join("prepare.stdout"), HANDOFF).unwrap();
    PreparedResources {
        artifact_directory,
        connection: PreparedSetupConnection::from_preparer_output(HANDOFF).unwrap(),
    }
}

#[test]
fn compiler_bundle_preserves_unselected_inputs_and_reconstructs_exact_helper() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let input = input(&root);
    let original = input.clone();
    assert!(
        original.roster[1].binding.is_some(),
        "explicit Session selection fixture"
    );
    let expected_policy = input.tool_policy.clone().compile().unwrap();
    let (result, policy) = finish(input.validate().unwrap(), prepared(&root)).unwrap();
    let projected_roster = result.project_roster(&original.roster).unwrap();
    assert_eq!(projected_roster[0], original.roster[0]);
    assert_eq!(result.native_members[0], original.native_members[0]);
    assert_eq!(
        result.definition.profiles[&ProfileName::from("research")],
        original.definition.resolve().unwrap().profiles[&ProfileName::from("research")]
    );
    let mut expected_definition = original.definition.resolve().unwrap();
    let projection = result.helper_projection().unwrap();
    projection
        .compose_definition(&mut expected_definition)
        .unwrap();
    assert_eq!(result.definition, expected_definition);
    let mut expected_roster = original.roster;
    expected_roster[1].profile = projection.profile_name().clone();
    assert_eq!(projected_roster, expected_roster);
    assert_eq!(
        result.project_roster(&projected_roster).unwrap(),
        projected_roster
    );
    let mut expected_members = original.native_members;
    expected_members[1].member = projection.member().clone();
    expected_members[1].initial_surface =
        surface::reference(&surface::compile(&original.helper_source_surface).unwrap());
    assert_eq!(result.native_members, expected_members);
    let mut expected_selection = original.helper;
    expected_selection.member = projection.member().clone();
    assert!(result.helper == expected_selection);
    assert_eq!(result.server, *projection.server());
    assert!(
        result
            .server
            .access
            .as_ref()
            .unwrap()
            .permits("setup_acquire_python")
    );
    assert!(
        !result
            .server
            .access
            .as_ref()
            .unwrap()
            .permits("setup_install")
    );
    assert_eq!(policy, expected_policy);
    let actual = CompiledApplicationToolPolicy::parse_canonical_json(&policy).unwrap();
    assert!(
        actual
            .members
            .iter()
            .find(|member| member.member_identity == "ordinary")
            .unwrap()
            .grants
            .is_empty()
    );
    assert_eq!(
        actual
            .members
            .iter()
            .find(|member| member.member_identity == result.helper.member.member)
            .unwrap()
            .grants[0]
            .consequence,
        CompiledToolConsequence::R2
    );
    let output = root.join("bundle");
    let receipt = write_bundle(&output, &result, &policy).unwrap();
    assert_eq!(hash(&std::fs::read(&receipt.path).unwrap()), receipt.sha256);
    let restored: PreparedApplicationSetup =
        read_json(output.join("application-setup.json").to_str().unwrap()).unwrap();
    assert_eq!(
        restored.helper_projection().unwrap().server(),
        projection.server()
    );
    let helper_surface = std::fs::read(output.join(surface::OUTPUT_NAME)).unwrap();
    let identities: Vec<meerkat_core::ToolIdentity> =
        serde_json::from_slice(&helper_surface).unwrap();
    assert_eq!(
        identities.last().unwrap(),
        &toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity()
    );
    assert_eq!(
        hash(&helper_surface),
        restored.native_members[1].initial_surface.sha256
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt.path).unwrap()).unwrap();
    assert!(manifest["artifacts"].as_array().unwrap().iter().any(|row| {
        row["path"] == surface::OUTPUT_NAME && row["sha256"] == hash(&helper_surface)
    }));
    let manifest_before = std::fs::read(&receipt.path).unwrap();
    assert!(write_bundle(&output, &result, &policy).is_err());
    assert_eq!(std::fs::read(&receipt.path).unwrap(), manifest_before);
    // Retaining configuration is not a claim of native portable sealing or
    // materialization; native MCP access-carrier support is independently needed.
    std::fs::write(&result.resources.handoff.path, b"changed").unwrap();
    assert!(result.helper_projection().is_err());
}

#[test]
fn checked_installation_is_explicit_and_restart_retains_its_exact_projection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut selected = input(&root);
    selected.helper_tools = ApplicationSetupTools::CheckedInstallation;
    let original = selected.clone();
    let expected_policy = selected.tool_policy.clone().compile().unwrap();
    let (result, policy) = finish(selected.validate().unwrap(), prepared(&root)).unwrap();
    assert_eq!(policy, expected_policy, "the mode cannot issue tool allowances");
    assert!(result.helper.operation_basis == original.helper.operation_basis);
    assert_eq!(result.helper.controller_parent, original.helper.controller_parent);
    assert_eq!(result.helper.logical_executor, original.helper.logical_executor);
    assert_eq!(result.native_members[0], original.native_members[0]);
    assert_eq!(result.project_roster(&original.roster).unwrap()[0], original.roster[0]);
    let output = root.join("checked-bundle");
    write_bundle(&output, &result, &policy).unwrap();
    let restored: PreparedApplicationSetup =
        read_json(output.join("application-setup.json").to_str().unwrap()).unwrap();
    let projection = restored.helper_projection().unwrap();
    let access = projection.server().access.as_ref().unwrap();
    assert!(access.permits("setup_install"));
    assert!(access.permits("setup_selected_check"));
    assert!(access.permits("setup_selected_install"));
    assert!(access.permits("setup_acquire_python"));
    assert!(!access.permits("unadvertised_operation"));
    assert_eq!(projection.member(), &result.helper.member);
    let mut changed = restored.clone();
    changed.helper_tools = ApplicationSetupTools::Preparation;
    assert!(changed.helper_projection().is_err(), "restart cannot substitute an earlier ceiling");
    let mut changed = restored;
    changed.helper.member.member = "different-helper".into();
    assert!(changed.helper_projection().is_err(), "the mode does not select another member");
}

#[test]
fn command_dispatch_and_invalid_inputs_refuse_before_resource_or_bundle_writes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    assert_eq!(
        crate::application_bootstrap::run(&["--prepare-application-setup".into()]),
        Some(std::process::ExitCode::FAILURE)
    );
    for case in [
        "schema",
        "foreign_roster",
        "wrong_role",
        "duplicate",
        "policy",
        "member",
        "source",
    ] {
        let mut selected = input(&root);
        match case {
            "schema" => selected.schema = 0,
            "foreign_roster" => {
                selected.roster[1].identity =
                    meerkat_mobkit::identity_first::AgentIdentity::parse("foreign").unwrap()
            }
            "wrong_role" => selected.roster[1].profile = ProfileName::from("other"),
            "duplicate" => selected.roster.push(selected.roster[1].clone()),
            "policy" => selected.tool_policy.revision = PolicyRevision(0),
            "member" => selected.native_members[1].member.role = "other".into(),
            "source" => {
                let ApplicationSetupDefinition::Native { definition } = &mut selected.definition
                else {
                    unreachable!()
                };
                definition.source_identity = Some(
                    meerkat_mob::MobDefinitionSourceIdentity::mobpack("verified-artifact", vec![]),
                )
            }
            _ => unreachable!(),
        }
        let path = root.join(format!("{case}.json"));
        std::fs::write(&path, serde_json::to_vec(&selected).unwrap()).unwrap();
        let output = root.join(format!("{case}-bundle"));
        let args = vec![
            "--prepare-application-setup".into(),
            "--input".into(),
            path.to_str().unwrap().into(),
            "--output".into(),
            output.to_str().unwrap().into(),
        ];
        let mut progress = InstallerProgress::default();
        assert!(command(&args, &mut progress).is_err(), "{case}");
        assert_eq!(progress.phase, "validation");
        assert!(!progress.effects_may_have_occurred);
        assert!(!output.exists());
        assert!(!root.join("state").exists());
    }
}

#[test]
fn restart_refuses_foreign_roster_missing_profile_and_substituted_server() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let (result, _) = finish(input(&root).validate().unwrap(), prepared(&root)).unwrap();
    let mut wrong_definition_mob = result.clone();
    wrong_definition_mob.definition.id = meerkat_mob::MobId::from("foreign");
    assert!(wrong_definition_mob.helper_projection().is_err());
    let mut wrong_source_mob = result.clone();
    wrong_source_mob.source_helper_member.mob_id = "foreign".into();
    assert!(wrong_source_mob.helper_projection().is_err());
    let mut verified_definition = result.clone();
    verified_definition.definition.source_identity = Some(
        meerkat_mob::MobDefinitionSourceIdentity::mobpack("verified", vec![]),
    );
    assert!(verified_definition.helper_projection().is_err());
    let mut foreign = result.clone();
    foreign.source_helper_roster.identity =
        meerkat_mobkit::identity_first::AgentIdentity::parse("foreign").unwrap();
    assert!(foreign.helper_projection().is_err());
    let mut missing = result.clone();
    missing
        .definition
        .profiles
        .remove(&ProfileName::from(result.helper.member.role.clone()));
    assert!(missing.helper_projection().is_err());
    let mut borrowed_roster = input(&root).roster;
    borrowed_roster[0].profile = ProfileName::from(result.helper.member.role.clone());
    assert!(result.project_roster(&borrowed_roster).is_err());
    let mut borrowed_policy_role = result.clone();
    borrowed_policy_role.native_members[0].member.role = result.helper.member.role.clone();
    assert!(borrowed_policy_role.helper_projection().is_err());
    let mut widened = result.clone();
    widened.server.access = None;
    assert!(widened.helper_projection().is_err());
    let mut other = result;
    other.resources.handoff.path = root.join("different.stdout");
    assert!(other.helper_projection().is_err());
}

#[tokio::test]
async fn cancellation_error_waits_for_entered_preparer_cleanup_before_returning() {
    use std::time::Duration;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let validated = input(&root).validate().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
    let (joined_tx, joined_rx) = tokio::sync::oneshot::channel();
    // Inert owner-protocol control: it represents cancellation after a child
    // entered, then holds cleanup. No provider, process or native effect runs.
    let mut task = tokio::spawn(compose_after_preparation(validated, async move {
        entered_tx.send(()).unwrap();
        cancel_rx.await.unwrap();
        joined_tx.send(()).unwrap();
        cleanup_rx.await.unwrap();
        Err(crate::ConnectError::InvalidInput(
            "cancelled after cleanup".into(),
        ))
    }));
    let observed: Result<(), &'static str> = async {
        tokio::time::timeout(Duration::from_secs(2), entered_rx)
            .await
            .map_err(|_| "owner did not enter")?
            .map_err(|_| "owner entry lost")?;
        cancel_tx
            .send(())
            .map_err(|_| "owner cancellation receiver lost")?;
        tokio::time::timeout(Duration::from_secs(2), joined_rx)
            .await
            .map_err(|_| "owner cleanup did not enter")?
            .map_err(|_| "owner cleanup entry lost")?;
        if task.is_finished() {
            return Err("caller released entered cleanup before settlement");
        }
        Ok(())
    }
    .await;
    // Release and join even when a bounded observation failed. This inert
    // fixture owns no provider/native effect or surviving background worker.
    let _ = cleanup_tx.send(());
    let outcome = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    if outcome.is_err() {
        task.abort();
        let _ = task.await;
        panic!("preparation failed to join");
    }
    observed.unwrap();
    assert!(outcome.unwrap().unwrap().is_err());
    assert!(!root.join("state").exists());
}

#[test]
fn preparation_shutdown_signals_reach_cleanup_and_pre_bundle_check() {
    use nix::{sys::signal::Signal, sys::signal::kill, unistd::Pid};
    use std::{
        process::Child,
        time::{Duration, Instant},
    };

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    fn wait_for(path: &Path, child: &mut Child, deadline: Instant) {
        while !path.exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "signal child exited before {path:?}"
            );
            assert!(
                Instant::now() < deadline,
                "signal child did not reach {path:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // Only newly spawned test children receive OS signals. The parent, other
    // tests, the preparer and the user's application receive none.
    for signal in [Signal::SIGINT, Signal::SIGTERM] {
        for phase in ["preparation", "pre_bundle"] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let mut child = OwnedChild(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "application_setup::tests::preparation_shutdown_signal_child",
                        "--nocapture",
                    ])
                    .env("MEERKAT_CONNECT_SIGNAL_CHILD_ROOT", &root)
                    .env("MEERKAT_CONNECT_SIGNAL_CHILD_PHASE", phase)
                    .stdin(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(15);
            wait_for(&root.join("ready"), &mut child.0, deadline);
            let pid = i32::try_from(child.0.id()).unwrap();
            kill(Pid::from_raw(pid), signal).unwrap();
            wait_for(&root.join("observed"), &mut child.0, deadline);
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "entered cleanup returned early"
            );
            assert!(!root.join("bundle").exists());
            std::fs::write(root.join("release"), b"release owned cleanup").unwrap();
            wait_for(&root.join("finished"), &mut child.0, deadline);
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(
                        status.success(),
                        "signal child failed for {signal:?}/{phase}"
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "signal child did not settle");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!root.join("bundle").exists());
        }
    }
}

#[test]
fn preparation_shutdown_signal_child() {
    let Some(root) = std::env::var_os("MEERKAT_CONNECT_SIGNAL_CHILD_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let phase = std::env::var("MEERKAT_CONNECT_SIGNAL_CHILD_PHASE").unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut shutdown = PreparationShutdown::register().unwrap();
            assert!(!shutdown.observed_before_bundle());
            let validated = input(&root).validate().unwrap();
            let wait_release = || async {
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    while !root.join("release").exists() {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
            };
            if phase == "preparation" {
                let result = compose_after_preparation(validated, async {
                    std::fs::write(root.join("ready"), b"both handlers registered").unwrap();
                    shutdown.cancelled().await.unwrap();
                    std::fs::write(root.join("observed"), b"entered cleanup").unwrap();
                    wait_release().await;
                    Err(crate::ConnectError::InvalidInput(
                        "cancelled after owned cleanup".into(),
                    ))
                })
                .await;
                match result {
                    Err(error) => assert!(error.contains("cancelled after owned cleanup")),
                    Ok(_) => panic!("cancelled preparation produced a bundle"),
                }
            } else {
                assert_eq!(phase, "pre_bundle");
                let _prepared = compose_after_preparation(validated, async { Ok(prepared(&root)) })
                    .await
                    .unwrap();
                std::fs::write(root.join("ready"), b"preparation already joined").unwrap();
                // Let this child's reactor receive the actual OS event; only
                // the same synchronous pre-bundle check consumes the signal.
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    while !shutdown.observed_before_bundle() {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                std::fs::write(root.join("observed"), b"bundle write refused").unwrap();
                wait_release().await;
            }
            assert!(
                shutdown.observed_before_bundle(),
                "consumed cancellation must remain visible"
            );
            assert!(!root.join("bundle").exists());
            std::fs::write(root.join("finished"), b"owned cleanup settled").unwrap();
        });
}

fn exclusive_input(root: &Path) -> ApplicationSetupInput {
    let mut selected = input(root);
    let ApplicationSetupDefinition::Native { definition } = &mut selected.definition else {
        unreachable!()
    };
    let profile = definition.profiles[&ProfileName::from("research")].clone();
    definition
        .profiles
        .insert(ProfileName::from("integration-helper"), profile);
    selected.roster[1].profile = ProfileName::from("integration-helper");
    selected.native_members[1].member.role = "integration-helper".into();
    selected.helper.member.role = "integration-helper".into();
    selected.helper_profile = ApplicationSetupProfile::ExclusiveApplicationRole;
    selected
}

#[test]
fn toml_source_uses_native_parser_and_retains_exact_input_identity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let raw = "# App-owned comment remains byte-for-byte.\n[mob]\nid = \"household\"\n\n[profiles.research]\nmodel = \"claude-opus-4-8\"\nrole_summary = \"App-private metadata\"\n[profiles.research.tools]\nshell = true\ncomms = true\nmcp = [\"browser\"]\n";
    let mut selected = input(&root);
    selected.definition = ApplicationSetupDefinition::Toml { text: raw.into() };
    let mut changed_raw = selected.clone();
    changed_raw.definition = ApplicationSetupDefinition::Toml {
        text: format!("{raw}# Different source identity.\n"),
    };
    let expected_source = MobDefinition::from_toml(raw).unwrap();
    let validated = selected.validate().unwrap();
    assert_eq!(validated.definition, expected_source);
    assert_ne!(
        validated.input_sha256,
        changed_raw.validate().unwrap().input_sha256
    );
    let (result, _) = finish(validated, prepared(&root)).unwrap();
    let restored: PreparedApplicationSetup =
        serde_json::from_slice(&toolkit_package::canonical_json(&result).unwrap()).unwrap();
    let ApplicationSetupDefinition::Toml { text } = &restored.definition_source else {
        panic!("raw TOML was replaced by a native/rewritten representation")
    };
    assert_eq!(text.as_bytes(), raw.as_bytes());
    let mut projected_source = expected_source;
    restored
        .helper_projection()
        .unwrap()
        .compose_definition(&mut projected_source)
        .unwrap();
    assert_eq!(restored.definition, projected_source);
    for text in [
        "[mob]\nid = ",
        "[mob]\nid = \"household\"\n[profiles.research]\nmodel = \"claude-opus-4-8\"\nsystem_prompt = \"Unsupported native profile key\"\n",
    ] {
        let mut invalid = input(&root);
        invalid.definition = ApplicationSetupDefinition::Toml { text: text.into() };
        assert!(invalid.validate().is_err());
    }
    assert!(!root.join("state").exists());
}

#[test]
fn exclusive_application_role_preserves_keys_and_rejects_initial_borrowers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let selected = exclusive_input(&root);
    for native in [false, true] {
        let mut borrowed = selected.clone();
        if native {
            borrowed.native_members[0].member.role = borrowed.helper.member.role.clone();
        } else {
            borrowed.roster[0].profile = ProfileName::from(borrowed.helper.member.role.clone());
        }
        assert!(borrowed.validate().is_err());
    }
    let before = selected.clone();
    let (result, policy) = finish(selected.validate().unwrap(), prepared(&root)).unwrap();
    assert!(result.helper == before.helper);
    let mut expected_members = before.native_members.clone();
    expected_members[1].initial_surface =
        surface::reference(&surface::compile(&before.helper_source_surface).unwrap());
    assert_eq!(result.native_members, expected_members);
    assert_eq!(
        result.project_roster(&before.roster).unwrap(),
        before.roster
    );
    assert_eq!(policy, before.tool_policy.compile().unwrap());
    let mut expected = before.definition.resolve().unwrap();
    expected
        .profiles
        .get_mut(&ProfileName::from("integration-helper"))
        .unwrap()
        .as_inline_mut()
        .unwrap()
        .tools
        .mcp_servers
        .push(result.server.clone());
    expected
        .profiles
        .get_mut(&ProfileName::from("integration-helper"))
        .unwrap()
        .as_inline_mut()
        .unwrap()
        .tools
        .rust_bundles
        .push(meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE.into());
    assert_eq!(result.definition, expected);
    assert_eq!(result.helper.member.role, "integration-helper");
    assert!(
        !result
            .definition
            .profiles
            .keys()
            .any(|name| name.as_str().starts_with("toolkit-setup-"))
    );
    result.helper_projection().unwrap();
}

#[test]
fn live_roster_projection_preserves_current_rows_and_never_resurrects_helper() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let selected = exclusive_input(&root);
    let original = selected.roster.clone();
    let (result, _) = finish(selected.validate().unwrap(), prepared(&root)).unwrap();
    let mut current = original.clone();
    current[0].identity =
        meerkat_mobkit::identity_first::AgentIdentity::parse("new-current-agent").unwrap();
    current[0]
        .additional_instructions
        .push("New current instructions".into());
    current.reverse();
    assert_eq!(result.project_roster(&current).unwrap(), current);
    let only_helper = vec![original[1].clone()];
    assert_eq!(result.project_roster(&only_helper).unwrap(), only_helper);
    for case in [
        "missing",
        "identity",
        "profile",
        "binding",
        "borrower",
        "duplicate",
    ] {
        let mut invalid = original.clone();
        match case {
            "missing" => {
                invalid.remove(1);
            }
            "identity" => {
                invalid[1].identity =
                    meerkat_mobkit::identity_first::AgentIdentity::parse("replacement-helper")
                        .unwrap()
            }
            "profile" => invalid[1].profile = ProfileName::from("other"),
            "binding" => invalid[1].binding = None,
            "borrower" => invalid[0].profile = invalid[1].profile.clone(),
            "duplicate" => invalid.push(invalid[1].clone()),
            _ => unreachable!(),
        }
        let before = invalid.clone();
        assert!(result.project_roster(&invalid).is_err(), "{case}");
        assert_eq!(invalid, before);
    }
    // These are pure projection controls over native DurableAgentSpec values,
    // not a claim that an application has wrapped its live RosterProvider yet.
}

#[test]
fn helper_surface_and_policy_inputs_refuse_before_preparation_or_bundle_writes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for case in [
        "old_schema",
        "source_hash",
        "source_name",
        "source_changed",
        "missing_policy",
        "foreign_policy",
        "output_collision",
    ] {
        let mut selected = input(&root);
        match case {
            "old_schema" => selected.schema = 2,
            "source_hash" => selected.native_members[1].initial_surface.sha256 = "f".repeat(64),
            "source_name" => {
                selected.native_members[1].initial_surface.name = "different.json".into()
            }
            "source_changed" => {
                std::fs::write(&selected.helper_source_surface.path, b"[]").unwrap()
            }
            "missing_policy" => selected.tool_policy.members[1].grants.clear(),
            "foreign_policy" => selected.tool_policy.members[1].member_identity = "foreign".into(),
            "output_collision" => {
                selected.native_members[0].initial_surface.name = surface::OUTPUT_NAME.into()
            }
            _ => unreachable!(),
        }
        let input_path = root.join(format!("{case}.json"));
        std::fs::write(&input_path, serde_json::to_vec(&selected).unwrap()).unwrap();
        let output = root.join(format!("{case}-bundle"));
        let mut progress = InstallerProgress::default();
        assert!(
            command(
                &[
                    "--prepare-application-setup".into(),
                    "--input".into(),
                    input_path.to_str().unwrap().into(),
                    "--output".into(),
                    output.to_str().unwrap().into()
                ],
                &mut progress
            )
            .is_err(),
            "{case}"
        );
        assert_eq!(progress.phase, "validation");
        assert!(!progress.effects_may_have_occurred);
        assert!(!output.exists());
        assert!(!root.join("state").exists());
    }
}

#[test]
fn helper_surface_compiler_preserves_exact_identities_and_refuses_name_substitution() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let selected = input(&root);
    let original: Vec<meerkat_core::ToolIdentity> =
        serde_json::from_slice(&std::fs::read(&selected.helper_source_surface.path).unwrap())
            .unwrap();
    let compiled = surface::compile(&selected.helper_source_surface).unwrap();
    let identities: Vec<meerkat_core::ToolIdentity> = serde_json::from_slice(&compiled).unwrap();
    assert_eq!(&identities[..original.len()], original.as_slice());
    assert_eq!(identities.len(), original.len() + 1);
    assert!(
        !identities
            .iter()
            .any(|tool| tool.name.as_str() == toolkit_mobkit::FIND_AND_INSTALL)
    );
    let already = SelectedHostArtifact {
        path: root.join("already.json"),
        sha256: hash(&compiled),
    };
    std::fs::write(&already.path, &compiled).unwrap();
    assert_eq!(surface::compile(&already).unwrap(), compiled);
    let mut changed: serde_json::Value = serde_json::from_slice(&compiled).unwrap();
    let duplicate_report = changed[1].clone();
    changed.as_array_mut().unwrap().push(duplicate_report);
    let duplicate = toolkit_package::canonical_json(&changed).unwrap();
    let duplicate_ref = SelectedHostArtifact {
        path: root.join("duplicate.json"),
        sha256: hash(&duplicate),
    };
    std::fs::write(&duplicate_ref.path, duplicate).unwrap();
    assert!(surface::compile(&duplicate_ref).is_err());
    let mut changed = identities;
    changed.last_mut().unwrap().provenance = Some(meerkat_core::ToolProvenance::new(
        meerkat_core::ToolSourceKind::Mcp,
        "foreign",
    ));
    let bytes = toolkit_package::canonical_json(&changed).unwrap();
    let foreign = SelectedHostArtifact {
        path: root.join("foreign.json"),
        sha256: hash(&bytes),
    };
    std::fs::write(&foreign.path, bytes).unwrap();
    assert!(surface::compile(&foreign).is_err());
}

#[test]
fn changed_helper_artifact_or_policy_is_refused_before_bundle_creation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let (result, policy) = finish(input(&root).validate().unwrap(), prepared(&root)).unwrap();
    let mut old = result.clone();
    old.schema = 2;
    assert!(old.helper_projection().is_err());
    let mut changed = result.clone();
    changed.native_members[1].initial_surface.sha256 = "f".repeat(64);
    assert!(changed.helper_projection().is_err());
    let output = root.join("tampered-bundle");
    assert!(write_bundle(&output, &changed, &policy).is_err());
    assert!(!output.exists());
    let output = root.join("wrong-policy-bundle");
    assert!(write_bundle(&output, &result, b"{}").is_err());
    assert!(!output.exists());
    std::fs::write(&result.helper_source_surface.as_ref().unwrap().path, b"[]").unwrap();
    let output = root.join("changed-source-bundle");
    assert!(write_bundle(&output, &result, &policy).is_err());
    assert!(!output.exists());
}

#[test]
fn legacy_command_rejects_duplicate_known_fields_before_effects() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let selected = input(&root);
    let original = serde_json::to_string(&selected).unwrap();
    for (field, value) in [
        ("schema", "3".to_owned()),
        ("helper_tools", serde_json::to_string(&selected.helper_tools).unwrap()),
    ] {
        let duplicate = format!("{{\"{field}\":{value},{}", &original[1..]);
        let path = root.join(format!("duplicate-{field}.json"));
        std::fs::write(&path, duplicate).unwrap();
        let output = root.join(format!("duplicate-{field}-bundle"));
        let args = vec![
            "--prepare-application-setup".into(),
            "--input".into(),
            path.to_str().unwrap().into(),
            "--output".into(),
            output.to_str().unwrap().into(),
        ];
        let mut progress = InstallerProgress::default();
        let error = command(&args, &mut progress).unwrap_err();
        assert!(error.contains(&format!("duplicate field `{field}`")), "{error}");
        assert_eq!(progress.phase, "validation");
        assert!(!progress.effects_may_have_occurred);
        assert!(!output.exists());
        assert!(!root.join("state").exists());
    }
}

#[test]
fn application_policy_preparation_retains_and_validates_setup_closure() {
    use crate::application_acquisition::{
        ApplicationAcquisitionSelection, read_application_acquisition,
    };
    use meerkat_toolkit::{
        authority::Ceiling,
        host_lifetime::{
            control::HostBootstrapSelection,
            policy::{CompiledLocalPolicy, CompiledNativeHost},
        },
    };
    use toolkit_mobkit::installed_host::{
        acquisition::InstalledSetupLimits, read_bundle_artifact, validate_installed_artifacts,
    };
    use toolkit_package::Id;

    for case in [
        "valid",
        "console_valid",
        "standing_valid",
        "task_parent_valid",
        "task_parent_drift",
        "standing_drift",
        "standing_unknown_mode",
        "console_invalid_origin",
        "console_origin_drift",
        "setup_missing",
        "setup_drift",
        "prepared_missing",
        "prepared_drift",
        "prepared_malformed",
        "setup_schema",
        "realm",
        "members",
        "policy",
        "envelope_collision",
        "name_conflict",
        "retained_handoff_drift",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut selected = input(&root);
        if matches!(case, "standing_valid" | "task_parent_valid" | "task_parent_drift" | "standing_drift" | "standing_unknown_mode") {
            selected.helper_tools = ApplicationSetupTools::CheckedInstallation;
        }
        let realm = meerkat_core::mob_realm_id(&selected.helper.member.mob_id).unwrap();
        selected.realm.realm_id = realm.as_str().into();
        // Reuse the existing schema3 fixture's actual retained source artifact.
        // This exercises the installer, not schema4 final native construction.
        let ordinary = std::fs::read(&selected.helper_source_surface.path).unwrap();
        selected.native_members[0].initial_surface.sha256 = hash(&ordinary);
        let (mut prepared, tool_policy) =
            finish(selected.validate().unwrap(), prepared(&root)).unwrap();
        let inputs = root.join("inputs");
        write_bundle(&inputs, &prepared, &tool_policy).unwrap();
        std::fs::write(inputs.join("ordinary-surface.json"), &ordinary).unwrap();
        let source = b"{\"schema\":1}";
        std::fs::write(inputs.join("policy.json"), source).unwrap();
        let principal = prepared.native_members[0].logical_owner.clone();
        let mut native: CompiledNativeHost = serde_json::from_value(json!({
            "realm":realm, "grant_administrator":principal, "remote_operator":null,
            "ingress_domain":{"authority":principal,"namespace":"source"},
            "application_tool_policy":prepared.application_tool_policy,
            "members":prepared.native_members, "routes":[]
        }))
        .unwrap();
        match case {
            "realm" => prepared.realm.realm_id = "other".into(),
            "members" => prepared.native_members[0].context_generation += 1,
            "policy" => prepared.application_tool_policy.name = "other-policy.json".into(),
            _ => {}
        }
        // Noncanonical setup JSON is valid typed input. The installer must keep
        // these selected bytes, not regenerate equivalent canonical JSON.
        let prepared_bytes = if case == "prepared_malformed" {
            b"not-json".to_vec()
        } else {
            serde_json::to_vec_pretty(&prepared).unwrap()
        };
        std::fs::write(inputs.join("application-setup.json"), &prepared_bytes).unwrap();
        let requester = Id::new("requester").unwrap();
        let mut config = ApplicationAcquisitionSelection {
            schema: 1,
            console_setup: None, local_operator_recovery: None,
            standing_policy: None,
            prepared_application_setup: CompiledArtifactReference {
                name: "application-setup.json".into(),
                sha256: hash(&prepared_bytes),
            },
            backend_generation: Id::new("application-owned-generation").unwrap(),
            proposal_ceiling: Ceiling {
                actions: BTreeSet::new(),
                services: BTreeSet::new(),
                frontends: BTreeSet::new(),
                output_audiences: [requester.clone()].into(),
            },
            output_audience: [requester].into(),
            limits: InstalledSetupLimits::default(),
        };
        match case {
            "console_valid" | "console_origin_drift" => config.console_setup = Some(
                crate::application_gateway::console_setup::ConsoleSetupSelection { origin: "https://console.example".into(), access_management: None, recovery_management: None }),
            "console_invalid_origin" => config.console_setup = Some(
                crate::application_gateway::console_setup::ConsoleSetupSelection { origin: "https://console.example/path".into(), access_management: None, recovery_management: None }),
            "standing_valid" | "task_parent_valid" | "task_parent_drift" | "standing_drift" | "standing_unknown_mode" => {
                use crate::application_acquisition::selected_policy::*;
                config.standing_policy = Some(StandingPolicySelection {
                    installation: InstallationSelection::CheckedSoftware,
                    tool_policy: None,
                    access: if matches!(case, "task_parent_valid" | "task_parent_drift") {
                        AccessSelection::ProvisionTaskParent
                    } else { AccessSelection::ConfiguredParent },
                });
                // The same resource/compiler fixture supplies the checked
                // setup surface; this is not evidence of executed installation.
            }
            "setup_schema" => config.schema = 0,
            "envelope_collision" => {
                config.prepared_application_setup.name = "manifest.json".into();
                // The nested bytes/hash are otherwise valid; the envelope
                // name itself must refuse before it can be overwritten.
                std::fs::write(inputs.join("manifest.json"), &prepared_bytes).unwrap();
            }
            "name_conflict" => config.prepared_application_setup.name = "policy.json".into(),
            _ => {}
        }
        let config_bytes = if case == "standing_unknown_mode" {
            let mut malformed = serde_json::to_value(&config).unwrap();
            malformed["standing_policy"]["installation"] = json!("agent_allow");
            serde_json::to_vec_pretty(&malformed).unwrap()
        } else { serde_json::to_vec_pretty(&config).unwrap() };
        native.setup = Some(CompiledArtifactReference {
            name: "native-setup.json".into(),
            sha256: hash(&config_bytes),
        });
        std::fs::write(inputs.join("native-setup.json"), &config_bytes).unwrap();
        match case {
            "setup_missing" => std::fs::remove_file(inputs.join("native-setup.json")).unwrap(),
            "setup_drift" => std::fs::write(inputs.join("native-setup.json"), b"changed").unwrap(),
            "task_parent_drift" => {
                let mut changed = config.clone();
                changed.standing_policy.as_mut().unwrap().access =
                    crate::application_acquisition::selected_policy::AccessSelection::ConfiguredParent;
                std::fs::write(inputs.join("native-setup.json"), serde_json::to_vec(&changed).unwrap()).unwrap();
            }
            "standing_drift" => {
                let mut changed = config.clone();
                changed.standing_policy.as_mut().unwrap().installation =
                    crate::application_acquisition::selected_policy::InstallationSelection::HumanReview;
                std::fs::write(inputs.join("native-setup.json"), serde_json::to_vec(&changed).unwrap()).unwrap();
            }
            "console_origin_drift" => {
                let mut changed = config.clone();
                changed.console_setup.as_mut().unwrap().origin = "https://other.example".into();
                std::fs::write(inputs.join("native-setup.json"), serde_json::to_vec(&changed).unwrap()).unwrap();
            },
            "prepared_missing" => {
                std::fs::remove_file(inputs.join("application-setup.json")).unwrap()
            }
            "prepared_drift" => {
                std::fs::write(inputs.join("application-setup.json"), b"changed").unwrap()
            }
            "retained_handoff_drift" => {
                std::fs::write(&prepared.resources.handoff.path, b"changed").unwrap()
            }
            _ => {}
        }
        let policy = CompiledLocalPolicy {
            schema: 7,
            deployment: Id::new("application").unwrap(),
            source_policy_sha256: hash(source),
            components: vec![],
            channels: vec![],
            native_host: Some(native.clone()),
        };
        let local = policy.canonical_bytes().unwrap();
        let policy_path = root.join("input.json");
        std::fs::write(&policy_path, &local).unwrap();
        let template = root.join("template.json");
        std::fs::write(&template, b"{\"activation_id\":\"owned-by-app\"}").unwrap();
        let bundle = root.join("installed-bundle");
        let activation = root.join("activation.json");
        let args = vec![
            "--prepare-application-policy".into(),
            "--policy".into(),
            policy_path.display().to_string(),
            "--artifacts".into(),
            inputs.display().to_string(),
            "--deployment-root".into(),
            root.display().to_string(),
            "--bundle".into(),
            bundle.display().to_string(),
            "--activation-template".into(),
            template.display().to_string(),
            "--activation".into(),
            activation.display().to_string(),
        ];
        let mut progress = InstallerProgress::default();
        let outcome = crate::application_bootstrap::prepare(&args, &mut progress);
        if matches!(case, "valid" | "console_valid" | "standing_valid" | "task_parent_valid") {
            outcome.unwrap();
            assert_eq!(progress.phase, "receipt");
            let selection = HostBootstrapSelection {
                deployment_root: root.clone(),
                activation: SelectedHostArtifact {
                    path: activation.clone(),
                    sha256: hash(&std::fs::read(&activation).unwrap()),
                },
                policy_manifest: SelectedHostArtifact {
                    path: bundle.join("manifest.json"),
                    sha256: hash(&std::fs::read(bundle.join("manifest.json")).unwrap()),
                },
                local_policy: SelectedHostArtifact {
                    path: bundle.join("toolkit-local-policy.json"),
                    sha256: hash(&local),
                },
            };
            // Read through the actual installed bundle's pinned manifest and
            // the same typed reader used by prepare_gateway_acquisition.
            // ProvisionTaskParent here is only an application request. This
            // fixture has no administrative rule; final policy binding refuses
            // it separately and the installer must not invent that authority.
            assert_eq!(
                read_bundle_artifact(&selection, native.setup.as_ref().unwrap()).unwrap(),
                config_bytes
            );
            assert_eq!(
                read_bundle_artifact(&selection, &config.prepared_application_setup).unwrap(),
                prepared_bytes
            );
            assert!(
                read_application_acquisition(&native, |reference| read_bundle_artifact(
                    &selection, reference
                ))
                .unwrap()
                .is_some()
            );
            validate_installed_artifacts(&native, |reference| {
                read_bundle_artifact(&selection, reference)
            })
            .unwrap();
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap())
                    .unwrap();
            let rows = manifest["artifacts"].as_array().unwrap();
            for reference in [
                native.setup.as_ref().unwrap(),
                &config.prepared_application_setup,
            ] {
                assert_eq!(
                    rows.iter()
                        .filter(|row| row["path"] == reference.name
                            && row["sha256"] == reference.sha256)
                        .count(),
                    1
                );
            }
            // The normal fresh-output contract still refuses overwrite.
            let mut retry = InstallerProgress::default();
            assert!(crate::application_bootstrap::prepare(&args, &mut retry).is_err());
            assert!(!retry.effects_may_have_occurred);
            assert_eq!(
                std::fs::read(bundle.join("application-setup.json")).unwrap(),
                prepared_bytes
            );
        } else {
            assert!(outcome.is_err(), "{case}");
            assert_eq!(progress.phase, "validation", "{case}");
            assert!(!progress.effects_may_have_occurred, "{case}");
            assert!(!bundle.exists(), "{case}");
            assert!(!activation.exists(), "{case}");
        }
        assert!(!root.join("current").exists(), "{case}");
        assert!(!root.join("state").exists(), "{case}");
    }
}
