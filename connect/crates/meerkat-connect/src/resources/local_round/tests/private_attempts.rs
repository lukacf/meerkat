//! The real Core attempt/close path over the existing inert stdlib fixture.
//! This is not qualification of the packaged Python Services conversation.
use super::*;
use meerkat_core::{ExternalToolDeltaPhase, SessionId};
use meerkat_mcp::{
    McpAttemptGuardBounds, McpAttemptGuards, McpConstructionOwner, McpRouter, McpStdioLaunchProfile,
};
use std::sync::Arc;

#[tokio::test]
async fn private_services_attempt_retains_normal_abnormal_and_cancelled_dispositions() {
    for (mode, native_custody) in [
        ("normal", false),
        ("abnormal_exit", false),
        ("term_close", false),
        ("initialize_hold", false),
        ("normal", true),
    ] {
        let mut fixture = Case::new("ready");
        fs::write(fixture.output.join("native-mode"), mode).unwrap();
        let python = std::env::var("TOOLKIT_SHARED_TEST_PYTHON").unwrap();
        let handoff = serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{
            "command":python,"args":["-I","-B","-c",super::native_attempts::NATIVE,fixture.output],
        }}}))
        .unwrap();
        let connection =
            crate::setup::PreparedSetupConnection::from_preparer_output(&handoff).unwrap();
        let selected = connection.into_server();
        let mut owner = fixture.owner.take().unwrap();
        retain(&owner.artifacts.join("prepare.stdout"), &handoff).unwrap();
        owner.handoff_sha256 = digest(&handoff);
        let session = SessionId::new();
        let router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            McpStdioLaunchProfile::trusted_host(),
        );
        let attempts = if native_custody {
            private_attempt::PrivateLocalRoundAttempts::for_native_construction(session.clone())
        } else {
            private_attempt::PrivateLocalRoundAttempts::new(
                session.clone(),
                router.construction_id(),
            )
        };
        attempts.retain(selected.clone(), owner).unwrap();
        if native_custody {
            attempts
                .bind_construction(router.construction_id())
                .unwrap();
            assert!(
                attempts
                    .bind_construction(router.construction_id())
                    .is_err(),
                "same construction cannot adopt twice"
            );
            assert!(
                attempts
                    .bind_construction(
                        McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host())
                            .construction_id(),
                    )
                    .is_err(),
                "another router cannot inherit custody"
            );
        }
        let guards = McpAttemptGuards::new(attempts.clone(), McpAttemptGuardBounds::default());
        let mut router =
            router.with_attempt_guards(guards.clone(), McpConstructionOwner::new(session, None));
        // Capture assertions inside the retained scope. Every outcome closes
        // this actual router and joins its guards before it can fail the test.
        let run = std::panic::AssertUnwindSafe(async {
            router.stage_add(selected).unwrap();
            router.apply_staged().await.unwrap();
            tokio::time::timeout(BOUND, async {
                loop {
                    if mode == "initialize_hold" {
                        if fs::read_dir(&fixture.output)
                            .unwrap()
                            .filter_map(Result::ok)
                            .any(|entry| {
                                entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".native-initializing")
                            })
                        {
                            break;
                        }
                    } else if router
                        .take_external_updates()
                        .notices
                        .iter()
                        .any(|row| row.phase == ExternalToolDeltaPhase::Applied)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual native attempt milestone");
            assert!(
                attempts.settled().is_err(),
                "live helper cannot claim settlement"
            );
        })
        .catch_unwind()
        .await;
        attempts.fence();
        let mut closing = tokio::spawn(router.shutdown_with_report());
        let report = (&mut closing).await.expect("retained native close joins");
        let records = guards
            .close_and_join(tokio::time::Instant::now() + BOUND)
            .await;
        assert!(report.is_settled(), "fixture native process was joined");
        assert!(records.iter().all(|row| !matches!(
            row.completion,
            meerkat_mcp::McpGuardCompletion::Held | meerkat_mcp::McpGuardCompletion::TimedOut
        )));
        let expected = matches!(mode, "normal" | "initialize_hold");
        assert_eq!(
            attempts.settled().is_ok(),
            expected,
            "abnormal process settlement cannot become normal Controller cleanup"
        );
        assert_eq!(
            attempts.settled().is_ok(),
            expected,
            "repeat observation preserves disposition"
        );
        if let Err(panic) = run {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
async fn private_services_attempt_refuses_changed_session_or_selected_descriptor_before_spawn() {
    for changed in [
        "session",
        "descriptor",
        "unbound_construction",
        "foreign_construction",
    ] {
        let mut fixture = Case::new("ready");
        fs::write(fixture.output.join("native-mode"), "normal").unwrap();
        let python = std::env::var("TOOLKIT_SHARED_TEST_PYTHON").unwrap();
        let handoff = serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{
            "command":python,"args":["-I","-B","-c",super::native_attempts::NATIVE,fixture.output],
        }}}))
        .unwrap();
        let selected = crate::setup::PreparedSetupConnection::from_preparer_output(&handoff)
            .unwrap()
            .into_server();
        let mut owner = fixture.owner.take().unwrap();
        retain(&owner.artifacts.join("prepare.stdout"), &handoff).unwrap();
        owner.handoff_sha256 = digest(&handoff);
        let session = SessionId::new();
        let router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            McpStdioLaunchProfile::trusted_host(),
        );
        let attempts = if changed.ends_with("construction") {
            let attempts = private_attempt::PrivateLocalRoundAttempts::for_native_construction(
                session.clone(),
            );
            if changed == "foreign_construction" {
                attempts
                    .bind_construction(
                        McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host())
                            .construction_id(),
                    )
                    .unwrap();
            }
            attempts
        } else {
            private_attempt::PrivateLocalRoundAttempts::new(
                session.clone(),
                router.construction_id(),
            )
        };
        attempts.retain(selected.clone(), owner).unwrap();
        let guards = McpAttemptGuards::new(attempts.clone(), McpAttemptGuardBounds::default());
        let mut router = router.with_attempt_guards(
            guards.clone(),
            McpConstructionOwner::new(
                if changed == "session" {
                    SessionId::new()
                } else {
                    session
                },
                None,
            ),
        );
        let mut submitted = selected;
        if changed == "descriptor" {
            let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) =
                &mut submitted.transport
            else {
                unreachable!()
            };
            stdio.args.push("unselected".into());
        }
        let run = std::panic::AssertUnwindSafe(async {
            router.stage_add(submitted).unwrap();
            router.apply_staged().await.unwrap();
            tokio::time::timeout(BOUND, async {
                loop {
                    if router
                        .take_external_updates()
                        .notices
                        .iter()
                        .any(|row| row.phase == ExternalToolDeltaPhase::Failed)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual native guard refusal");
        })
        .catch_unwind()
        .await;
        attempts.fence();
        let report = router.shutdown_with_report().await;
        let records = guards
            .close_and_join(tokio::time::Instant::now() + BOUND)
            .await;
        assert!(report.is_settled() && report.guards_completed());
        assert!(records.iter().all(|row| !matches!(
            row.completion,
            meerkat_mcp::McpGuardCompletion::Held | meerkat_mcp::McpGuardCompletion::TimedOut
        )));
        assert!(
            !fs::read_dir(&fixture.output)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".native-started"))
        );
        assert!(
            attempts.settled().is_ok(),
            "unentered preparation remains owned without a fabricated process close"
        );
        if let Err(panic) = run {
            std::panic::resume_unwind(panic);
        }
    }
}

// A real Core begin wait distinguishes a local refusal from a late successful
// local callback. The deliberate wait belongs only to this test factory.
struct ObservedBegin {
    inner: Arc<private_attempt::PrivateLocalRoundAttempts>,
    hold_until_cancel: bool,
}
#[async_trait::async_trait]
impl meerkat_mcp::McpAttemptGuardFactory for ObservedBegin {
    async fn begin(
        &self,
        request: meerkat_mcp::McpAttemptGuardRequest,
        cancellation: meerkat_mcp::McpAttemptCancellation,
    ) -> Result<
        Option<Box<dyn meerkat_mcp::McpAttemptGuard>>,
        meerkat_core::mcp_attachment::McpAttachmentRefusal,
    > {
        let result =
            meerkat_mcp::McpAttemptGuardFactory::begin(&*self.inner, request, cancellation.clone())
                .await;
        if self.hold_until_cancel {
            cancellation.requested().await;
        }
        result
    }
}
#[tokio::test]
async fn private_services_diagnostic_distinguishes_local_refusal_from_actual_begin_bound() {
    for mode in ["binding", "resources", "begin_bound"] {
        let mut fixture = Case::new("ready");
        let python = std::env::var("TOOLKIT_SHARED_TEST_PYTHON").unwrap();
        let handoff = serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{
            "command":python,"args":["-I","-B","-c",super::native_attempts::NATIVE,fixture.output],
        }}}))
        .unwrap();
        let selected = crate::setup::PreparedSetupConnection::from_preparer_output(&handoff)
            .unwrap()
            .into_server();
        let mut owner = fixture.owner.take().unwrap();
        retain(&owner.artifacts.join("prepare.stdout"), &handoff).unwrap();
        owner.handoff_sha256 = digest(&handoff);
        let session = SessionId::new();
        let router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            McpStdioLaunchProfile::trusted_host(),
        );
        let attempts = private_attempt::PrivateLocalRoundAttempts::new(
            session.clone(),
            router.construction_id(),
        );
        attempts.retain(selected.clone(), owner).unwrap();
        if mode == "resources" {
            fs::write(
                fixture.selected_root.join("prepare.py"),
                b"changed after preparation",
            )
            .unwrap();
        }
        let factory = Arc::new(ObservedBegin {
            inner: attempts.clone(),
            hold_until_cancel: mode == "begin_bound",
        });
        let bounds = if mode == "begin_bound" {
            McpAttemptGuardBounds::new(
                Duration::from_secs(1),
                Duration::from_secs(5),
                Duration::from_secs(5),
            )
            .unwrap()
        } else {
            McpAttemptGuardBounds::default()
        };
        let guards = McpAttemptGuards::new(factory, bounds);
        let mut router =
            router.with_attempt_guards(guards.clone(), McpConstructionOwner::new(session, None));
        let mut submitted = selected;
        if mode == "binding" {
            let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) =
                &mut submitted.transport
            else {
                unreachable!()
            };
            stdio.args.push("unselected".into());
        }
        let run = std::panic::AssertUnwindSafe(async {
            router.stage_add(submitted).unwrap();
            router.apply_staged().await.unwrap();
            tokio::time::timeout(BOUND, async {
                loop {
                    if router
                        .take_external_updates()
                        .notices
                        .iter()
                        .any(|row| row.phase == ExternalToolDeltaPhase::Failed)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual Core refusal is observed");
        })
        .catch_unwind()
        .await;
        attempts.fence();
        let report = router.shutdown_with_report().await;
        let records = guards
            .close_and_join(tokio::time::Instant::now() + BOUND)
            .await;
        assert!(report.is_settled());
        assert!(records.iter().all(|row| !matches!(
            row.completion,
            meerkat_mcp::McpGuardCompletion::Held | meerkat_mcp::McpGuardCompletion::TimedOut
        )));
        if let Err(panic) = run {
            std::panic::resume_unwind(panic);
        }
        let diagnostic = attempts.diagnostic(&records);
        assert_eq!(diagnostic["local_truncated"], false);
        assert_eq!(diagnostic["core_truncated"], false);
        assert_eq!(diagnostic["local"].as_array().unwrap().len(), 1);
        let local = &diagnostic["local"][0];
        assert_eq!(
            local["stage"],
            match mode {
                "binding" => "binding_mismatch",
                "resources" => "resources_refused",
                _ => "guard_returned",
            }
        );
        assert_eq!(
            local["cancellation_cause"],
            if mode == "begin_bound" {
                "begin_bound"
            } else {
                "not_requested"
            }
        );
        let lock_wait = local["lock_wait_micros"]
            .as_u64()
            .expect("observed mutex acquisition");
        let local_return = local["local_return_boundary_micros"]
            .as_u64()
            .expect("observed local return boundary");
        assert!(local_return >= lock_wait);
        if mode == "binding" {
            assert!(
                local["resource_verification_micros"].is_null(),
                "no resource check was entered"
            );
        } else {
            let resources = local["resource_verification_micros"]
                .as_u64()
                .expect("completed actual resource check");
            assert!(
                local_return >= resources,
                "local interval includes resource verification"
            );
        }
        assert_eq!(
            attempts.diagnostic(&records)["local"],
            diagnostic["local"],
            "joined-close observations retain durations instead of timing a later read"
        );
        let core = diagnostic["core"].as_array().unwrap();
        if mode == "begin_bound" {
            assert!(core.iter().any(|row| row["open"] == "cancelled"
                && row["cancellation_cause"] == "begin_bound"
                && row["process"] == "not_spawned"));
        } else {
            assert!(core.iter().any(|row| row["stage"] == "begin"
                && row["open"] == "refused"
                && row["refusal"] == "guard"));
        }
        assert!(
            !fs::read_dir(&fixture.output)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".native-started"))
        );
        for forbidden in [
            python.as_str(),
            fixture.selected_root.to_str().unwrap(),
            "unselected",
        ] {
            assert!(!diagnostic.to_string().contains(forbidden));
        }
    }
}
