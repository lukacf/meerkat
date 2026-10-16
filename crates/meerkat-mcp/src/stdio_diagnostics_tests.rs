//! Stdio setup diagnostics (#230): the typed failure class and the bounded,
//! redacted stderr tail of a failed stdio server attempt, read by the host
//! through the adapter (latest retained failure per server) or the exact
//! attempt's observer, and never reaching the model: not in the setup
//! failure notice, the lifecycle detail, the host event payload or the
//! attempt's readiness detail, on any path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use meerkat_core::ExternalToolDeltaPhase;
use meerkat_core::mcp_attachment::{
    McpAttachmentAttempt, McpAttachmentReadiness, McpAttachmentRefusal, McpStdioSetupFailureClass,
    McpStdioSetupPhase,
};
use std::collections::HashMap;

/// What every fixture server writes to stderr: it must reach the host's tail
/// and never anything model-facing.
const SENTINEL: &str = "STDERR-SENTINEL-7f3a";
/// A provider-shaped key a fixture prints: shape redaction removes it.
const FAKE_KEY: &str = "sk-test-ABCDEFGHIJKLMNOPQRSTUV";
const WAIT: Duration = Duration::from_secs(40);

/// Answers `initialize` (its id read from the request) with `reply`, a
/// printf format taking the id.
fn answer_initialize(reply: &str) -> String {
    format!(
        r#"IFS= read -r line
id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
printf '{reply}\n' "$id"
"#
    )
}

const INITIALIZED: &str = r#"{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"diag","version":"0"}}}"#;

/// Reads (and ignores) stdin until it closes, then exits 0.
const DRAIN_STDIN: &str = "while IFS= read -r _; do :; done\n";

/// Serves `initialize` and `tools/list` (no tools) until stdin closes.
const SERVE: &str = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"diag","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id" ;;
  esac
done
"#;

fn shell_server(name: &str, script: &str) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        "/bin/sh".to_string(),
        vec!["-c".to_string(), script.to_string()],
        HashMap::new(),
    )
}

fn with_timeout(mut config: McpServerConfig, seconds: u32) -> McpServerConfig {
    config.connect_timeout_secs = Some(seconds);
    config
}

fn exits_before_initialize(name: &str) -> McpServerConfig {
    shell_server(
        name,
        &format!("printf 'boom {SENTINEL} %s\\n' '{FAKE_KEY}' >&2\nexit 3\n"),
    )
}

fn adapter() -> Arc<McpRouterAdapter> {
    Arc::new(McpRouterAdapter::new(McpRouter::new_with_surface_handle(
        Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
    )))
}

/// A prepared control that always permits the apply.
struct Permit(McpServerConfig);

impl McpAttachmentAuthority for Permit {
    fn server(&self) -> &McpServerConfig {
        &self.0
    }
    fn authorize_apply(&self) -> Result<(), McpAttachmentRefusal> {
        Ok(())
    }
    fn authorize_persist(
        &self,
        _scope: meerkat_core::mcp_config::McpScope,
    ) -> Result<(), McpAttachmentRefusal> {
        Err(McpAttachmentRefusal::Denied)
    }
}

async fn stage_observed(
    adapter: &Arc<McpRouterAdapter>,
    config: &McpServerConfig,
) -> McpAttachmentAttempt {
    adapter
        .stage_add_governed_observed(config.clone(), Arc::new(Permit(config.clone())))
        .await
        .expect("stage under a permitting control")
}

/// Add `config` (ungoverned), apply, and collect lifecycle notices until
/// one for it reaches `phase`.
async fn add_until(
    adapter: &Arc<McpRouterAdapter>,
    config: McpServerConfig,
    phase: ExternalToolDeltaPhase,
) -> Vec<ExternalToolDelta> {
    let name = config.name.clone();
    adapter.stage_add(config).await.expect("stage");
    adapter.apply_staged().await.expect("apply");
    until_notice(adapter, &name, phase).await
}

async fn until_notice(
    adapter: &Arc<McpRouterAdapter>,
    name: &str,
    phase: ExternalToolDeltaPhase,
) -> Vec<ExternalToolDelta> {
    let mut seen = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let update = adapter.poll_external_updates().await;
            let done = update
                .notices
                .iter()
                .any(|notice| notice.target == name && notice.phase == phase);
            seen.extend(update.notices);
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {phase:?} notice for {name}: {seen:?}"));
    seen
}

/// Nothing model-facing, and nothing a host event or readiness detail
/// carries, contains the server's stderr.
fn assert_no_stderr_leak(notices: &[ExternalToolDelta]) {
    for notice in notices {
        let delta = format!("{notice:?}");
        let payload = format!("{:?}", notice.to_tool_config_changed_payload());
        for rendered in [&delta, &payload] {
            assert!(
                !rendered.contains(SENTINEL),
                "stderr in a notice: {rendered}"
            );
            assert!(
                !rendered.contains("sk-test"),
                "stderr in a notice: {rendered}"
            );
        }
        if let Some(message) = notice.model_setup_failure_notice() {
            let model = format!("{message:?}");
            assert!(
                !model.contains(SENTINEL),
                "stderr reached the model: {model}"
            );
            assert!(
                !model.contains("sk-test"),
                "stderr reached the model: {model}"
            );
            assert!(
                !model.contains("Closed") && !model.contains("TimedOut"),
                "the class stays host-facing in this release: {model}"
            );
        }
    }
}

fn assert_readiness_has_no_stderr(readiness: &McpAttachmentReadiness) {
    let rendered = format!("{readiness:?}");
    assert!(!rendered.contains(SENTINEL), "{rendered}");
}

/// The adapter's retained tail of `name`, once its drain caught up with the
/// sentinel line.
async fn tail_with_sentinel(
    adapter: &Arc<McpRouterAdapter>,
    name: &str,
) -> meerkat_core::mcp_attachment::McpStdioStderrTail {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(tail) = adapter.stdio_stderr_tail(name).await
                && tail.text.contains(SENTINEL)
            {
                return tail;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no retained tail with the sentinel for {name}"))
}

#[tokio::test]
async fn a_server_exiting_before_initialize_is_closed_with_its_tail() {
    let adapter = adapter();
    let notices = add_until(
        &adapter,
        exits_before_initialize("exits"),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("exits")
        .await
        .expect("the failed stdio attempt is retained");
    assert_eq!(failure.server, "exits");
    assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
    let McpStdioSetupFailureClass::Closed { exit } = failure.class else {
        panic!("closed during initialize: {failure:?}");
    };
    #[cfg(target_os = "linux")]
    assert_eq!(
        exit,
        Some(meerkat_core::mcp_attachment::McpStdioExit::Code(3)),
        "the server's own exit status"
    );
    #[cfg(not(target_os = "linux"))]
    let _ = exit;
    assert!(failure.stderr_tail_retained);
    let tail = tail_with_sentinel(&adapter, "exits").await;
    assert!(tail.text.contains("boom"), "{:?}", tail.text);
    assert!(
        !tail.text.contains(FAKE_KEY),
        "shape redaction: {:?}",
        tail.text
    );
    assert!(!tail.truncated);
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_rejected_initialize_reports_its_json_rpc_code() {
    let adapter = adapter();
    let script = format!(
        "printf 'starting {SENTINEL}\\n' >&2\n{}{DRAIN_STDIN}",
        answer_initialize(
            r#"{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}"#
        )
    );
    let notices = add_until(
        &adapter,
        shell_server("rejects", &script),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("rejects")
        .await
        .expect("retained");
    assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
    assert_eq!(
        failure.class,
        McpStdioSetupFailureClass::Rejected { code: -32601 }
    );
    tail_with_sentinel(&adapter, "rejects").await;
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_malformed_initialize_result_is_a_protocol_error() {
    let adapter = adapter();
    let script = format!(
        "printf 'starting {SENTINEL}\\n' >&2\n{}{DRAIN_STDIN}",
        answer_initialize(r#"{"jsonrpc":"2.0","id":%s,"result":{}}"#)
    );
    let notices = add_until(
        &adapter,
        shell_server("malformed", &script),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("malformed")
        .await
        .expect("retained");
    assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
    assert_eq!(failure.class, McpStdioSetupFailureClass::ProtocolError);
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_hung_initialize_times_out_in_the_initialize_phase() {
    let adapter = adapter();
    let script = format!("printf 'waiting {SENTINEL}\\n' >&2\n{DRAIN_STDIN}");
    let notices = add_until(
        &adapter,
        with_timeout(shell_server("hangs", &script), 3),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("hangs")
        .await
        .expect("retained");
    assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
    assert_eq!(failure.class, McpStdioSetupFailureClass::TimedOut);
    tail_with_sentinel(&adapter, "hangs").await;
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_hung_tool_listing_times_out_in_the_discovery_phase() {
    let adapter = adapter();
    let script = format!(
        "printf 'discovering {SENTINEL}\\n' >&2\n{}{DRAIN_STDIN}",
        answer_initialize(INITIALIZED)
    );
    let notices = add_until(
        &adapter,
        with_timeout(shell_server("lists", &script), 5),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("lists")
        .await
        .expect("retained");
    assert_eq!(failure.phase, McpStdioSetupPhase::ToolDiscovery);
    assert_eq!(failure.class, McpStdioSetupFailureClass::TimedOut);
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_missing_command_is_a_spawn_failure_without_a_tail() {
    let adapter = adapter();
    let config = McpServerConfig::stdio(
        "missing",
        "/nonexistent/never-run-stdio-diagnostics",
        vec![],
        HashMap::new(),
    );
    let notices = add_until(&adapter, config, ExternalToolDeltaPhase::Failed).await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("missing")
        .await
        .expect("retained");
    assert_eq!(failure.phase, McpStdioSetupPhase::Launch);
    assert_eq!(failure.class, McpStdioSetupFailureClass::SpawnFailed);
    assert!(!failure.stderr_tail_retained);
    assert!(adapter.stdio_stderr_tail("missing").await.is_none());
    adapter.shutdown().await;
}

#[tokio::test]
async fn a_flooded_tail_is_bounded_and_keeps_its_last_line() {
    let adapter = adapter();
    let script = format!(
        "head -c 40000 /dev/zero | tr '\\0' 'x' >&2\nprintf '\\nlast line {SENTINEL}\\n' >&2\nexit 5\n"
    );
    let notices = add_until(
        &adapter,
        shell_server("floods", &script),
        ExternalToolDeltaPhase::Failed,
    )
    .await;
    assert_no_stderr_leak(&notices);
    let failure = adapter
        .stdio_setup_failure("floods")
        .await
        .expect("retained");
    assert!(
        matches!(failure.class, McpStdioSetupFailureClass::Closed { .. }),
        "{failure:?}"
    );
    let tail = tail_with_sentinel(&adapter, "floods").await;
    assert!(tail.truncated);
    assert!(tail.total_bytes >= 40_000, "{tail:?}");
    assert!(tail.text.len() <= 16 * 1024, "{tail:?}");
    assert_eq!(tail.text, format!("last line {SENTINEL}\n"));
    adapter.shutdown().await;
}

/// A server writing far more stderr than a pipe holds before it answers
/// still completes its handshake: its stderr is drained throughout.
#[tokio::test]
async fn a_stderr_flood_never_blocks_the_handshake() {
    let adapter = adapter();
    let script = format!("head -c 262144 /dev/zero | tr '\\0' 'y' >&2\nprintf '\\n' >&2\n{SERVE}");
    let notices = add_until(
        &adapter,
        shell_server("loud", &script),
        ExternalToolDeltaPhase::Applied,
    )
    .await;
    assert_no_stderr_leak(&notices);
    assert!(
        adapter.stdio_setup_failure("loud").await.is_none(),
        "a successful attempt retains nothing"
    );
    adapter.shutdown().await;
}

#[tokio::test]
async fn launch_values_are_redacted_and_short_values_are_not_tracked() {
    let adapter = adapter();
    let mut config = shell_server(
        "secrets",
        &format!(
            "printf 'a %s b %s c %s {SENTINEL}\\n' \"$MY_SECRET_TOKEN\" \"$MY_SECRET_TOKEN\" \"$SHORT_VAL\" >&2\nexit 4\n"
        ),
    );
    if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut config.transport {
        stdio.env.insert(
            "MY_SECRET_TOKEN".to_string(),
            "s3cret-value-ABCDEFGH".to_string(),
        );
        stdio
            .env
            .insert("SHORT_VAL".to_string(), "abc12".to_string());
    }
    let notices = add_until(&adapter, config, ExternalToolDeltaPhase::Failed).await;
    assert_no_stderr_leak(&notices);
    let tail = tail_with_sentinel(&adapter, "secrets").await;
    assert!(!tail.text.contains("s3cret-value"), "{:?}", tail.text);
    assert_eq!(
        tail.text,
        format!("a [REDACTED] b [REDACTED] c abc12 {SENTINEL}\n")
    );
    adapter.shutdown().await;
}

#[tokio::test]
async fn removing_or_re_adding_a_server_drops_its_retained_failure() {
    let adapter = adapter();
    let config = exits_before_initialize("dropped");
    add_until(&adapter, config.clone(), ExternalToolDeltaPhase::Failed).await;
    assert!(adapter.stdio_setup_failure("dropped").await.is_some());
    tail_with_sentinel(&adapter, "dropped").await;

    adapter.stage_remove("dropped").await.expect("stage remove");
    adapter.apply_staged().await.expect("apply remove");
    assert!(adapter.stdio_setup_failure("dropped").await.is_none());
    assert!(adapter.stdio_stderr_tail("dropped").await.is_none());

    // Retained again, then superseded by the next admitted attempt.
    add_until(&adapter, config.clone(), ExternalToolDeltaPhase::Failed).await;
    assert!(adapter.stdio_setup_failure("dropped").await.is_some());
    adapter
        .stage_add(with_timeout(shell_server("dropped", DRAIN_STDIN), 30))
        .await
        .expect("stage a new attempt");
    adapter.apply_staged().await.expect("apply");
    assert!(
        adapter.stdio_setup_failure("dropped").await.is_none(),
        "a newer admitted attempt supersedes the retained failure"
    );
    assert!(adapter.stdio_stderr_tail("dropped").await.is_none());
    adapter.shutdown().await;
}

/// The exact attempt keeps its typed class after it is superseded; only its
/// tail is released.
#[tokio::test]
async fn the_exact_attempt_keeps_its_class_and_loses_its_tail_when_superseded() {
    let adapter = adapter();
    let config = exits_before_initialize("observed");
    let attempt = stage_observed(&adapter, &config).await;
    adapter.apply_staged().await.expect("apply");
    let readiness = attempt.wait("any", WAIT).await;
    assert!(
        matches!(readiness, McpAttachmentReadiness::Failed { .. }),
        "{readiness:?}"
    );
    assert_readiness_has_no_stderr(&readiness);
    let failure = attempt.setup_failure().await.expect("the attempt's class");
    assert_eq!(failure.server, "observed");
    assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
    assert!(matches!(
        failure.class,
        McpStdioSetupFailureClass::Closed { .. }
    ));
    assert!(failure.stderr_tail_retained);
    let tail = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(tail) = attempt.stderr_tail().await
                && tail.text.contains(SENTINEL)
            {
                return tail;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the exact attempt's tail");
    assert!(!tail.text.contains(FAKE_KEY));
    assert_no_stderr_leak(&adapter.poll_external_updates().await.notices);

    let next = stage_observed(
        &adapter,
        &with_timeout(shell_server("observed", DRAIN_STDIN), 30),
    )
    .await;
    adapter
        .apply_staged()
        .await
        .expect("apply the next attempt");
    let kept = attempt.setup_failure().await.expect("the class stays");
    assert_eq!(kept.class, failure.class);
    assert!(!kept.stderr_tail_retained, "{kept:?}");
    assert!(attempt.stderr_tail().await.is_none());
    assert!(
        next.stderr_tail().await.is_none(),
        "the next attempt is pending"
    );
    adapter.shutdown().await;
}

#[tokio::test]
async fn shutdown_during_a_hung_initialize_retains_nothing() {
    let adapter = adapter();
    let config = with_timeout(
        shell_server(
            "shutdown",
            &format!("printf 'waiting {SENTINEL}\\n' >&2\n{DRAIN_STDIN}"),
        ),
        30,
    );
    let attempt = stage_observed(&adapter, &config).await;
    adapter.apply_staged().await.expect("apply");
    tokio::time::sleep(Duration::from_millis(300)).await;
    adapter.shutdown().await;
    // The shutdown stops the attempt: its own accepted failure (the router
    // shut down), never a stdio setup failure, or retired.
    let readiness = attempt.observe("any").await;
    assert!(
        matches!(
            readiness,
            McpAttachmentReadiness::Retired | McpAttachmentReadiness::Failed { .. }
        ),
        "{readiness:?}"
    );
    assert_readiness_has_no_stderr(&readiness);
    assert!(attempt.setup_failure().await.is_none());
    assert!(attempt.stderr_tail().await.is_none());
    assert!(adapter.stdio_setup_failure("shutdown").await.is_none());
    assert!(adapter.stdio_stderr_tail("shutdown").await.is_none());
    assert_no_stderr_leak(&adapter.poll_external_updates().await.notices);
}

#[tokio::test]
async fn a_dropped_router_releases_the_tail() {
    let adapter = adapter();
    let attempt = stage_observed(&adapter, &exits_before_initialize("dropped-router")).await;
    adapter.apply_staged().await.expect("apply");
    let readiness = attempt.wait("any", WAIT).await;
    assert!(matches!(readiness, McpAttachmentReadiness::Failed { .. }));
    assert!(attempt.stderr_tail().await.is_some());
    drop(adapter);
    assert!(attempt.stderr_tail().await.is_none());
    let failure = attempt.setup_failure().await.expect("the class stays");
    assert!(!failure.stderr_tail_retained);
}
