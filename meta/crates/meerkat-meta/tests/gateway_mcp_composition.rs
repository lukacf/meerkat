//! The composed gateway offers each member factory it constructs to its host
//! for trusted MCP composition and for its operation refusal observer, before
//! the factory is cloned or consumed and before `prepare_bootstrap`. A host
//! that refuses either fails init before any bootstrap. Each case is a
//! separate gateway process over real stdio.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::Write;
use std::sync::Arc;

use meerkat_meta::UnifiedRuntime;
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
    GatewayHostComposition, GatewayHostError, GatewayMcpComposition,
};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;

/// Set (to `persistent`, `ephemeral`, `refuse` or `refuse-observer`) only in
/// the gateway host process; `RECORD` names its record file.
const HOST_PROCESS: &str = "META_GATEWAY_MCP_COMPOSITION_HOST";
const RECORD: &str = "META_GATEWAY_MCP_COMPOSITION_RECORD";
const TEST: &str = "every_gateway_member_factory_is_offered_to_the_host_before_bootstrap";

/// Records each factory offer and the bootstrap, in order, then refuses the
/// Ready publication: the record is all this test needs.
struct ComposingHost {
    refuse: bool,
    refuse_observer: bool,
    record: std::path::PathBuf,
}

impl ComposingHost {
    fn append(&self, line: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.record)
            .expect("record");
        writeln!(file, "{line}").expect("append");
    }
}

#[async_trait::async_trait]
impl GatewayHostComposition for ComposingHost {
    async fn prepare_bootstrap(
        &self,
        spec: meerkat_meta::MobBootstrapSpec,
    ) -> Result<meerkat_meta::MobBootstrapSpec, GatewayHostError> {
        self.append("bootstrap");
        Ok(spec)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        Ok(None)
    }

    async fn bind_runtime(&self, _runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        _publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        Err(GatewayHostError::Denied)
    }

    fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }

    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        self.append(&format!(
            "factory {:?} realm={} account={} installation={}",
            construction.kind(),
            construction.realm().as_str(),
            construction.host_auth_service().is_ok(),
            construction.realm_mcp_installation().is_ok(),
        ));
        if self.refuse {
            Err(GatewayHostError::Unavailable)
        } else {
            Ok(None)
        }
    }

    fn operation_refusal_observer(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<dyn meerkat_core::OperationRefusalObserver>>, GatewayHostError> {
        self.append(&format!("observer {:?}", construction.kind()));
        if self.refuse_observer {
            Err(GatewayHostError::Denied)
        } else {
            Ok(None)
        }
    }
}

#[test]
fn every_gateway_member_factory_is_offered_to_the_host_before_bootstrap() {
    if let Some(mode) = std::env::var_os(HOST_PROCESS) {
        let host = Arc::new(ComposingHost {
            refuse: mode == "refuse",
            refuse_observer: mode == "refuse-observer",
            record: std::env::var_os(RECORD).expect("record path").into(),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let _ = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
            None, host,
        ));
        std::process::exit(0);
    }
    for mode in ["persistent", "ephemeral", "refuse", "refuse-observer"] {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(HOST_PROCESS, mode)
            .env(RECORD, &record)
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("gateway host process");
        let stdout = child.stdout.take().expect("stdout");
        let output = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut { stdout }, &mut text);
            text
        });
        let mut params = serde_json::json!({
            "mob_config": "[mob]\nid = \"gateway-mcp-composition\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        });
        if mode != "ephemeral" {
            params["persistent_state"] = serde_json::json!(dir.path().join("state"));
        }
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            "{}",
            serde_json::json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params})
        )
        .expect("write init");
        stdin.flush().expect("flush init");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(stdin);
        let exited = child.try_wait().unwrap();
        if exited.is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let recorded = std::fs::read_to_string(&record).unwrap_or_default();
        let lines: Vec<&str> = recorded.lines().collect();
        let init_response = output
            .join()
            .expect("stdout reader")
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|message| message["id"] == "init");
        match mode {
            "persistent" => assert_eq!(
                lines,
                [
                    "factory Persistent realm=meta account=true installation=true",
                    "observer Persistent",
                    "bootstrap"
                ],
                "{mode}"
            ),
            "ephemeral" => {
                assert!(
                    lines
                        .first()
                        .is_some_and(|line| line.starts_with("factory ProcessLocal realm=meta")),
                    "{mode}: {lines:?}"
                );
                assert_eq!(lines.last(), Some(&"bootstrap"), "{mode}: {lines:?}");
                let offers = &lines[..lines.len() - 1];
                assert!(
                    offers.len().is_multiple_of(2)
                        && offers.chunks(2).all(|offer| {
                            let kind = offer[0]
                                .strip_prefix("factory ")
                                .and_then(|rest| rest.split_whitespace().next());
                            kind.is_some_and(|kind| offer[1] == format!("observer {kind}"))
                                && (offer[0].ends_with("account=true installation=true")
                                    || offer[0].ends_with("account=false installation=false"))
                        }),
                    "{mode}: every factory is offered before the bootstrap, its MCP composition \
                     and then its observer, with its realm MCP administration exactly when its \
                     account facade: {lines:?}"
                );
            }
            "refuse-observer" => {
                assert_eq!(
                    lines,
                    [
                        "factory Persistent realm=meta account=true installation=true",
                        "observer Persistent"
                    ],
                    "a refused observer reaches no bootstrap"
                );
                assert_eq!(
                    init_response
                        .as_ref()
                        .map(|response| &response["error"]["message"]),
                    Some(&serde_json::json!(GatewayHostError::Denied.to_string())),
                    "the init failure is the host's own error: {init_response:?}"
                );
            }
            _ => {
                assert_eq!(
                    lines,
                    ["factory Persistent realm=meta account=true installation=true"],
                    "a refused factory reaches no bootstrap"
                );
                assert_eq!(
                    init_response
                        .as_ref()
                        .map(|response| &response["error"]["message"]),
                    Some(&serde_json::json!(
                        GatewayHostError::Unavailable.to_string()
                    )),
                    "the init failure is the host's own error: {init_response:?}"
                );
            }
        }
        assert!(
            exited.is_some(),
            "{mode}: the gateway ends after its refused init"
        );
    }
}
