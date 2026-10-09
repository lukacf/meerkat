//! Actual MCP launch and lifecycle controls. These exercise the host-installed
//! mechanical profile, not caller authentication or authorization policy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{McpRouter, McpStdioLaunchProfile};
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use std::collections::HashMap;
use std::path::Path;

const BOUND: Duration = Duration::from_secs(15);

fn requirement(root: &Path, unsupported: bool) -> ExecutionConfinement {
    ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_owned())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_owned())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: unsupported,
    }
    .try_into()
    .unwrap()
}

fn marker_config(root: &Path) -> McpServerConfig {
    McpServerConfig::stdio(
        "required-child",
        "/bin/sh",
        vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
        HashMap::from([("MARKER".into(), root.join("entered").display().to_string())]),
    )
}

#[tokio::test]
async fn required_mcp_confinement_never_falls_back_to_an_unconfined_child() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let profile = McpStdioLaunchProfile::required(requirement(&root, true), root.clone());
    assert!(matches!(
        profile.capabilities(),
        Err(ConfinementRefusal::UnsupportedRequirement)
    ));
    let result = tokio::time::timeout(
        BOUND,
        McpConnection::connect_and_enumerate_with_stdio_profile(&marker_config(&root), &profile),
    )
    .await
    .expect("pre-spawn refusal is bounded");
    assert!(matches!(
        result,
        Err(McpError::Confinement(
            ConfinementRefusal::UnsupportedRequirement
        ))
    ));
    assert!(!root.join("entered").exists());
}

#[tokio::test]
async fn required_mcp_staged_refusal_is_a_failed_notice_not_a_stuck_pending_surface() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let profile = McpStdioLaunchProfile::required(requirement(&root, true), root.clone());
    let handle = Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral());
    let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(handle, profile);
    router.stage_add(marker_config(&root)).unwrap();
    router.apply_staged().await.unwrap();
    let notice = tokio::time::timeout(BOUND, async {
        loop {
            let update = router.take_external_updates();
            if let Some(notice) = update.notices.into_iter().find(|notice| {
                notice.target == "required-child"
                    && notice.phase == crate::McpLifecyclePhase::Failed
            }) {
                return notice;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a failed launch must settle its pending obligation");
    let host = serde_json::to_value(notice.to_tool_config_changed_payload()).unwrap();
    assert_eq!(
        host["status_info"]["confinement_refusal"],
        "unsupported_requirement",
    );
    let meerkat_core::Message::SystemNotice(model) = notice.model_setup_failure_notice().unwrap()
    else {
        panic!("failed setup must project a model notice");
    };
    assert_eq!(
        serde_json::to_value(&model.blocks[0]).unwrap()["confinement_refusal"],
        "unsupported_requirement",
    );
    let detail = notice.detail.as_deref().unwrap();
    assert_eq!(
        detail,
        ConfinementRefusal::UnsupportedRequirement.to_string()
    );
    assert!(
        !detail.contains(root.to_str().unwrap()),
        "host paths are not refusal feedback"
    );
    assert!(router.take_external_updates().pending.is_empty());
    assert!(!root.join("entered").exists());
    router.shutdown().await;
    assert!(!root.join("entered").exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod supported_native {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        work: PathBuf,
        server: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let work = root.join("work");
            std::fs::create_dir(&work).unwrap();
            std::fs::write(root.join("secret"), b"host secret").unwrap();
            // Cargo artifacts may have multiple hard links. Give this fixture
            // its own executable outside the writable work subtree; the
            // profile admits only this copy as an explicit read-only literal.
            let server = root.join("mcp-test-server");
            std::fs::copy(mcp_test_server::fixture_binary(), &server).unwrap();
            Self {
                _temp: temp,
                root,
                work,
                server,
            }
        }

        fn profile(&self) -> McpStdioLaunchProfile {
            let mut spec = requirement(&self.work, false).specification().clone();
            let FilesystemAccess::Paths(read) = &mut spec.read else {
                unreachable!()
            };
            read.push(PathAccess::Literal(self.server.clone()));
            McpStdioLaunchProfile::required(spec.try_into().unwrap(), self.work.clone())
        }

        fn config(&self, name: &str) -> McpServerConfig {
            McpServerConfig::stdio(
                name,
                "/bin/sh",
                vec![
                    "-c".into(),
                    r#"
test -z "${HOME+x}" || exit 80
test "$PWD" = "$WORK" || exit 81
if /bin/cat "$SECRET" >/dev/null 2>&1; then exit 82; fi
if (printf escaped > "$OUTSIDE") 2>/dev/null; then exit 83; fi
printf permitted > "$INSIDE"
exec "$SERVER"
"#
                    .into(),
                ],
                HashMap::from([
                    ("WORK".into(), self.work.display().to_string()),
                    (
                        "SECRET".into(),
                        self.root.join("secret").display().to_string(),
                    ),
                    (
                        "OUTSIDE".into(),
                        self.root.join("outside").display().to_string(),
                    ),
                    (
                        "INSIDE".into(),
                        self.work.join("inside").display().to_string(),
                    ),
                    ("SERVER".into(), self.server.display().to_string()),
                ]),
            )
        }
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn required_mcp_launch_enforces_filesystem_and_explicit_environment_then_serves_tools() {
        let fixture = Fixture::new();
        let profile = fixture.profile();
        assert!(profile.capabilities().unwrap().is_some());
        let (connection, tools) = tokio::time::timeout(
            BOUND,
            McpConnection::connect_and_enumerate_with_stdio_profile(
                &fixture.config("allowed"),
                &profile,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(tools.iter().any(|tool| tool.name == "echo"));
        let result = connection
            .call_tool("echo", &serde_json::json!({"message":"permitted sibling"}))
            .await;
        connection.close().await.unwrap();
        assert_eq!(
            meerkat_core::types::text_content(&result.unwrap()),
            "permitted sibling"
        );
        assert_eq!(
            std::fs::read(fixture.work.join("inside")).unwrap(),
            b"permitted"
        );
        assert_eq!(
            std::fs::read(fixture.root.join("secret")).unwrap(),
            b"host secret"
        );
        assert!(!fixture.root.join("outside").exists());
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn required_mcp_stderr_flood_does_not_block_handshake_or_tool_and_close() {
        let fixture = Fixture::new();
        let profile = fixture.profile();
        assert!(profile.capabilities().unwrap().is_some());
        let mut config = fixture.config("stderr-flood");
        let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
            unreachable!()
        };
        // Shell builtins write 513 * 4096 = 2,101,248 bytes before the MCP
        // fixture starts. An undrained pipe cannot reach the handshake.
        let flood = r#"
chunk=0123456789abcdef
i=0
while [ "$i" -lt 8 ]; do
    chunk="$chunk$chunk"
    i=$((i + 1))
done
i=0
while [ "$i" -lt 513 ]; do
    printf '%s' "$chunk" >&2 || exit 84
    i=$((i + 1))
done
"#;
        stdio.args[1] = format!("{flood}\n{}", stdio.args[1]);
        let (connection, tools) = tokio::time::timeout(
            BOUND,
            McpConnection::connect_and_enumerate_with_stdio_profile(&config, &profile),
        )
        .await
        .expect("stderr saturation cannot stall MCP handshake")
        .unwrap();
        assert!(tools.iter().any(|tool| tool.name == "echo"));
        let result = tokio::time::timeout(
            BOUND,
            connection.call_tool("echo", &serde_json::json!({"message":"after stderr flood"})),
        )
        .await;
        tokio::time::timeout(BOUND, connection.close())
            .await
            .expect("confined child close is bounded after stderr flood")
            .unwrap();
        assert_eq!(
            meerkat_core::types::text_content(
                &result.expect("tool response follows stderr flood").unwrap(),
            ),
            "after stderr flood"
        );
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn required_mcp_rejects_unsafe_launch_then_retains_profile_for_a_permitted_sibling() {
        let fixture = Fixture::new();
        let profile = fixture.profile();
        for relative_program in [false, true] {
            let mut refused = marker_config(&fixture.work);
            let McpTransportConfig::Stdio(stdio) = &mut refused.transport else {
                unreachable!()
            };
            let expected = if relative_program {
                stdio.command = "sh".into();
                ConfinementRefusal::InvalidLaunch
            } else {
                stdio
                    .env
                    .insert("BASH_ENV".into(), "untrusted-startup".into());
                ConfinementRefusal::UnsupportedRequirement
            };
            let result = McpConnection::connect_with_stdio_profile(&refused, &profile).await;
            assert!(matches!(result, Err(McpError::Confinement(actual)) if actual == expected));
            assert!(!fixture.work.join("entered").exists());
        }
        let handle = Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral());
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(handle, profile);
        router
            .add_server(fixture.config("reload-target"))
            .await
            .unwrap();
        let mut reload = fixture.config("reload-target");
        let McpTransportConfig::Stdio(stdio) = &mut reload.transport else {
            unreachable!()
        };
        stdio
            .env
            .insert("BASH_ENV".into(), "reload-injection".into());
        router.stage_reload(reload).unwrap();
        router.apply_staged().await.unwrap();
        tokio::time::timeout(BOUND, async {
            loop {
                let update = router.take_external_updates();
                if let Some(notice) = update.notices.iter().find(|notice| {
                    notice.target == "reload-target"
                        && notice.operation
                            == meerkat_core::event::ToolConfigChangeOperation::Reload
                        && notice.phase == crate::McpLifecyclePhase::Failed
                }) {
                    let host =
                        serde_json::to_value(notice.to_tool_config_changed_payload()).unwrap();
                    assert_eq!(
                        host["status_info"]["confinement_refusal"],
                        "unsupported_requirement"
                    );
                    let model = notice
                        .model_setup_failure_notice()
                        .expect("reload refusal model notice");
                    let encoded = serde_json::to_string(&model).unwrap();
                    assert!(encoded.contains("unsupported_requirement"));
                    assert!(!encoded.contains("reload-injection"));
                    assert!(!encoded.contains(fixture.root.to_str().unwrap()));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut sibling = fixture.config("sibling");
        sibling
            .tool_names
            .insert("echo".into(), "sibling_echo".into());
        router.add_server(sibling).await.unwrap();
        let result = router
            .call_tool(
                "sibling_echo",
                &serde_json::json!({"message":"still permitted"}),
            )
            .await;
        router.shutdown().await;
        assert_eq!(
            meerkat_core::types::text_content(&result.unwrap()),
            "still permitted"
        );
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn confined_mcp_cancelled_reap_retains_native_child_until_retry_completes() {
        let fixture = Fixture::new();
        let profile = fixture.profile();
        let custody = StdioChildCustody::default();
        let config = meerkat_core::mcp_config::McpStdioConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "exec /bin/sleep 60".into()],
            env: HashMap::new(),
        };
        let pipes = custody.spawn(&config, &profile).await.unwrap();
        let pid = custody.spawned_pid().await;
        let (entered, resume) = custody.pause_next_reap();
        let first = tokio::spawn({
            let custody = custody.clone();
            async move { custody.terminate().await }
        });
        tokio::time::timeout(BOUND, entered).await.unwrap().unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        drop(resume);
        let result = tokio::time::timeout(BOUND, custody.terminate()).await;
        drop(pipes);
        result
            .expect("retained native child can be reaped")
            .unwrap()
            .unwrap();
        super::super::tests::assert_child_reaped(pid);
        assert_eq!(
            custody
                .direct_kill_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!(custody.terminate().await.is_none());
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "linux",
        ignore = "Linux positive confinement acceptance lane; requires an eligible host"
    )]
    async fn confined_mcp_failed_handshake_reaps_before_returning() {
        let fixture = Fixture::new();
        let profile = fixture.profile();
        assert!(profile.capabilities().unwrap().is_some());
        let config = McpServerConfig::stdio(
            "confined-closes-stdout",
            "/bin/sh",
            vec!["-c".into(), "exec >&-; exec /bin/sleep 60".into()],
            HashMap::new(),
        );
        let custody = StdioChildCustody::default();
        let result = tokio::time::timeout(
            BOUND,
            McpConnection::connect_and_enumerate_with_custody(
                &config,
                McpAuthMode::Stored,
                None,
                None,
                Some(custody.clone()),
                &profile,
            ),
        )
        .await
        .unwrap();
        assert!(
            !matches!(&result, Err(McpError::Confinement(_))),
            "pre-entry confinement refusal is not a failed-handshake cleanup result"
        );
        assert!(result.is_err());
        let pid = tokio::time::timeout(BOUND, custody.spawned_pid())
            .await
            .expect("failed handshake must have spawned a child before returning");
        super::super::tests::assert_child_reaped(pid);
        assert!(custody.terminate().await.is_none());
    }
}
