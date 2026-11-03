#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::process::Command;
use std::time::Duration;

use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsoleAccessRequest, ConsolePolicy,
    ConsoleRestJsonRequest, DiscoverySpec, JsonRpcResponse, META_CONTRACT_VERSION, MetaConfig,
    ModuleConfig, ModuleRouteRequest, PreSpawnData, RestartPolicy, RuntimeDecisionInputs,
    RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedEvent, build_runtime_decision_state,
    handle_console_rest_json_route, handle_meta_rpc_json, route_module_call,
    run_discovered_module_once, start_meta_runtime,
};
use tempfile::TempDir;

fn shell_module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"},{"kid":"kid-next","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtbmV4dC1zZWNyZXQ"}]}"#.to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn trusted_toml_for_sdk_console_probe() -> String {
    r#"
[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "memory"]
restart_policy = "on_failure"

[[modules]]
id = "routing"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"
"#
    .to_string()
}

fn module_probe_config(module_ids: &[&str]) -> MetaConfig {
    let modules = module_ids
        .iter()
        .enumerate()
        .map(|(idx, module_id)| {
            let tool_list = format!("{module_id}/tools.list");
            let representative_call = format!("{module_id}/tool.call");
            let envelope = serde_json::json!({
                "event_id": format!("evt-{module_id}"),
                "source": "module",
                "timestamp_ms": 100 + idx as u64,
                "event": {
                    "kind": "module",
                    "module": module_id,
                    "event_type": "ready",
                    "payload": {
                        "family": module_id,
                        "health": {"state": "healthy"},
                        "tools": {
                            "list_method": tool_list,
                            "representative_call": {
                                "method": representative_call,
                                "params_schema": {"tool": "string", "input": "json"}
                            }
                        }
                    }
                }
            });
            shell_module(module_id, &format!("printf '%s\\n' '{}'", envelope))
        })
        .collect();

    let discovery_modules = module_ids
        .iter()
        .map(|module| (*module).to_string())
        .collect();
    let pre_spawn = module_ids
        .iter()
        .map(|module_id| PreSpawnData {
            module_id: (*module_id).to_string(),
            env: vec![("MODULE_FAMILY".to_string(), (*module_id).to_string())],
        })
        .collect();

    MetaConfig {
        modules,
        discovery: DiscoverySpec {
            namespace: "phase0b".to_string(),
            modules: discovery_modules,
        },
        pre_spawn,
    }
}

fn run_shell(script: &str, env: &[(&str, &str)]) -> String {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    for (k, v) in env {
        command.env(k, v);
    }
    let output = command.output().expect("shell command should start");
    assert!(
        output.status.success(),
        "command failed: {}\nstdout:\n{}\nstderr:\n{}",
        script,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout utf8")
}

fn gate_enabled(flag: &str) -> bool {
    std::env::var(flag).ok().as_deref() == Some("1")
}

fn skip_unless_gate_enabled(flag: &str) -> bool {
    if gate_enabled(flag) {
        true
    } else {
        eprintln!("skipping external boundary test because {flag} is not set to 1");
        false
    }
}

fn run_phase0b_rpc_gateway(request_json: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
        .env("META_RPC_REQUEST", request_json)
        .output()
        .expect("phase0b rpc gateway should start");
    assert!(
        output.status.success(),
        "phase0b_rpc_gateway failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout utf8")
}

fn module_family_process_config(module_ids: &[&str]) -> (MetaConfig, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir for module-family probes");
    let mut modules = Vec::new();
    let mut discovery_modules = Vec::new();
    let mut pre_spawn = Vec::new();

    for (idx, module_id) in module_ids.iter().enumerate() {
        let script_path = temp.path().join(format!("probe_{module_id}.py"));
        let script = format!(
            r#"#!/usr/bin/env python3
import json
import os

family = "{module_id}"
if os.environ.get("MODULE_FAMILY") != family:
    raise SystemExit(17)
envelope = {{
    "event_id": "evt-{module_id}-{idx}",
    "source": "module",
    "timestamp_ms": {timestamp_ms},
    "event": {{
        "kind": "module",
        "module": family,
        "event_type": "ready",
        "payload": {{
            "family": family,
            "health": {{"state": "healthy"}},
            "process_probe": "python-script-{module_id}",
            "tools": {{
                "list_method": f"{{family}}/tools.list",
                "representative_call": {{
                    "method": f"{{family}}/tool.call",
                    "params_schema": {{"tool": "string", "input": "json"}}
                }}
            }}
        }}
    }}
}}
print(json.dumps(envelope))
"#,
            timestamp_ms = 1_000 + idx as u64
        );
        std::fs::write(&script_path, script).expect("write module-family probe script");

        modules.push(ModuleConfig {
            id: (*module_id).to_string(),
            command: "python3".to_string(),
            args: vec![script_path.to_string_lossy().to_string()],
            restart_policy: RestartPolicy::Never,
        });
        discovery_modules.push((*module_id).to_string());
        pre_spawn.push(PreSpawnData {
            module_id: (*module_id).to_string(),
            env: vec![("MODULE_FAMILY".to_string(), (*module_id).to_string())],
        });
    }

    (
        MetaConfig {
            modules,
            discovery: DiscoverySpec {
                namespace: "phase0b-family-process".to_string(),
                modules: discovery_modules,
            },
            pre_spawn,
        },
        temp,
    )
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t1_rpc_method_matrix_boundary_preflight() {
    let status: JsonRpcResponse = serde_json::from_str(&run_phase0b_rpc_gateway(
        r#"{"jsonrpc":"2.0","id":"1","method":"meta/status","params":{}}"#,
    ))
    .expect("status response json");
    assert_eq!(status.id, serde_json::json!("1"));
    assert_eq!(
        status
            .result
            .as_ref()
            .and_then(|result| result.get("contract_version"))
            .and_then(|value| value.as_str()),
        Some(META_CONTRACT_VERSION)
    );
    assert_eq!(
        status
            .result
            .as_ref()
            .and_then(|result| result.get("loaded_modules"))
            .and_then(|value| value.as_array())
            .map(|modules| modules.len()),
        Some(1)
    );

    let capabilities: JsonRpcResponse = serde_json::from_str(&run_phase0b_rpc_gateway(
        r#"{"jsonrpc":"2.0","id":"2","method":"meta/capabilities","params":{}}"#,
    ))
    .expect("capabilities response json");
    assert_eq!(capabilities.id, serde_json::json!("2"));
    let methods = capabilities
        .result
        .as_ref()
        .and_then(|result| result.get("methods"))
        .and_then(|value| value.as_array())
        .expect("methods array");
    assert!(methods.iter().any(|method| method == "meta/status"));

    let routed: JsonRpcResponse = serde_json::from_str(&run_phase0b_rpc_gateway(
        r#"{"jsonrpc":"2.0","id":"3","method":"routing/tools.list","params":{"probe":"viability"}}"#,
    ))
    .expect("module proxy response json");
    assert_eq!(
        routed
            .result
            .as_ref()
            .and_then(|result| result.get("module_id"))
            .and_then(|value| value.as_str()),
        Some("routing")
    );

    let not_found: JsonRpcResponse = serde_json::from_str(&run_phase0b_rpc_gateway(
        r#"{"jsonrpc":"2.0","id":"4","method":"meta/not_loaded","params":{}}"#,
    ))
    .expect("unknown method response json");
    assert_eq!(not_found.error.map(|err| err.code), Some(-32601));

    let unloaded: JsonRpcResponse = serde_json::from_str(&run_phase0b_rpc_gateway(
        r#"{"jsonrpc":"2.0","id":"5","method":"delivery/tools.list","params":{"probe":"viability"}}"#,
    ))
    .expect("unloaded route response json");
    assert_eq!(unloaded.error.map(|err| err.code), Some(-32601));

    let notification =
        run_phase0b_rpc_gateway(r#"{"jsonrpc":"2.0","method":"meta/status","params":{}}"#);
    assert!(notification.is_empty());
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t2_bigquery_real_boundary() {
    if !skip_unless_gate_enabled("META_RUN_P0B_BQ") {
        return;
    }
    // The run opted in, so a missing project is a configuration error.
    let project = std::env::var("META_P0B_BQ_PROJECT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "META_RUN_P0B_BQ=1 opts in to the real BigQuery boundary test, but \
                 META_P0B_BQ_PROJECT is not set: set it to a GCP project in which the gcloud \
                 credentials may create and delete BigQuery datasets"
            )
        });
    let stdout = run_shell(
        r#"
set -euo pipefail
PROJECT="${META_P0B_BQ_PROJECT:?}"
echo "PROJECT_SELECTED:$(gcloud projects describe "$PROJECT" --format='value(projectId)')"
UNIQ="$(python3 - <<'PY'
import uuid
print(uuid.uuid4().hex)
PY
)"
DATASET="meta_p0b_${UNIQ}"
TABLE="sessions"
STREAM_TABLE="sessions_stream_probe"
cleanup() {
  set +e
  echo "CLEANUP_BEGIN:${PROJECT}:${DATASET}.${TABLE},${STREAM_TABLE}"
  bq --project_id="$PROJECT" rm -f -t "${PROJECT}:${DATASET}.${TABLE}" >/dev/null 2>&1 || true
  bq --project_id="$PROJECT" rm -f -t "${PROJECT}:${DATASET}.${STREAM_TABLE}" >/dev/null 2>&1 || true
  bq --project_id="$PROJECT" rm -f -d "${PROJECT}:${DATASET}" >/dev/null 2>&1 || true
  echo "CLEANUP_END:${PROJECT}:${DATASET}"
}
trap cleanup EXIT
bq --project_id="$PROJECT" --location=US mk -d --description "meta phase0b preflight" "${PROJECT}:${DATASET}"
echo "DATASET_CREATED:${PROJECT}:${DATASET}"
bq --project_id="$PROJECT" --location=US mk \
  --table "${PROJECT}:${DATASET}.${TABLE}" \
  session_id:STRING,updated_at:TIMESTAMP,deleted:BOOL,payload:STRING
echo "TABLE_CREATED:${PROJECT}:${DATASET}.${TABLE}"
bq --project_id="$PROJECT" --location=US mk \
  --table "${PROJECT}:${DATASET}.${STREAM_TABLE}" \
  session_id:STRING,updated_at:TIMESTAMP,deleted:BOOL,payload:STRING
ROWS_FILE="${DATASET}_stream_rows.ndjson"
cat > "${ROWS_FILE}" <<'EOF'
{"session_id":"stream-s1","updated_at":"2024-01-01T00:00:00Z","deleted":false,"payload":"{\"stream\":true}"}
EOF
bq --project_id="$PROJECT" insert "${PROJECT}:${DATASET}.${STREAM_TABLE}" "${ROWS_FILE}"
rm -f "${ROWS_FILE}"
for _ in $(seq 1 60); do
  STREAM_PROBE_COUNT="$(bq --project_id="$PROJECT" query --nouse_legacy_sql --format=csv \
    "SELECT COUNT(*) AS c FROM \`${PROJECT}.${DATASET}.${STREAM_TABLE}\`" | tail -n +2 | tr -d '\r')"
  [ "${STREAM_PROBE_COUNT}" = "1" ] && break
  sleep 1
done
echo "STREAM_PROBE_COUNT:${STREAM_PROBE_COUNT}"
[ "${STREAM_PROBE_COUNT}" = "1" ]
bq --project_id="$PROJECT" query --nouse_legacy_sql \
  "INSERT \`${PROJECT}.${DATASET}.${TABLE}\` (session_id,updated_at,deleted,payload) VALUES ('s1', TIMESTAMP('2024-01-01T00:00:00Z'), FALSE, '{\"ok\":true}')"
bq --project_id="$PROJECT" query --nouse_legacy_sql \
  "INSERT \`${PROJECT}.${DATASET}.${TABLE}\` (session_id,updated_at,deleted,payload) VALUES ('s1', TIMESTAMP('2024-01-01T00:00:01Z'), TRUE, '')"
bq --project_id="$PROJECT" query --nouse_legacy_sql \
  "INSERT \`${PROJECT}.${DATASET}.${TABLE}\` (session_id,updated_at,deleted,payload) VALUES ('s2', TIMESTAMP('2024-01-01T00:00:00Z'), FALSE, '{\"ok\":false}')"
DEDUP_CSV="$(bq --project_id="$PROJECT" query --nouse_legacy_sql --format=csv \
  "SELECT session_id, deleted FROM \`${PROJECT}.${DATASET}.${TABLE}\` QUALIFY ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY updated_at DESC)=1 ORDER BY session_id")"
DEDUP_ROWS="$(echo "$DEDUP_CSV" | tail -n +2 | tr -d '\r')"
while IFS= read -r row; do
  [ -n "$row" ] && echo "DEDUP_ROW:${row}"
done <<EOF
$DEDUP_ROWS
EOF
TOMBSTONE_COUNT="$(bq --project_id="$PROJECT" query --nouse_legacy_sql --format=csv \
  "SELECT COUNT(*) AS c FROM \`${PROJECT}.${DATASET}.${TABLE}\` WHERE session_id='s1' AND deleted=TRUE" | tail -n +2 | tr -d '\r')"
echo "TOMBSTONE_COUNT:${TOMBSTONE_COUNT}"
bq --project_id="$PROJECT" query --nouse_legacy_sql "TRUNCATE TABLE \`${PROJECT}.${DATASET}.${TABLE}\`" >/dev/null
POST_TRUNCATE_COUNT="$(bq --project_id="$PROJECT" query --nouse_legacy_sql --format=csv \
  "SELECT COUNT(*) AS c FROM \`${PROJECT}.${DATASET}.${TABLE}\`" | tail -n +2 | tr -d '\r')"
echo "POST_TRUNCATE_COUNT:${POST_TRUNCATE_COUNT}"
"#,
        &[("META_P0B_BQ_PROJECT", project.as_str())],
    );
    println!("{stdout}");

    assert!(stdout.contains(&format!("PROJECT_SELECTED:{project}")));
    assert!(stdout.contains(&format!("DATASET_CREATED:{project}:meta_p0b_")));
    assert!(stdout.contains(&format!("TABLE_CREATED:{project}:meta_p0b_")));
    assert!(stdout.contains("STREAM_PROBE_COUNT:1"));
    assert!(stdout.contains("DEDUP_ROW:s1,true"));
    assert!(stdout.contains("DEDUP_ROW:s2,false"));
    assert!(stdout.contains("TOMBSTONE_COUNT:1"));
    assert!(stdout.contains("POST_TRUNCATE_COUNT:0"));
    assert!(stdout.contains(&format!("CLEANUP_BEGIN:{project}:meta_p0b_")));
    assert!(stdout.contains(&format!("CLEANUP_END:{project}:meta_p0b_")));
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t3_json_store_filesystem_lock_preflight() {
    let output = run_shell(
        r#"
set -euo pipefail
TMPDIR="$(mktemp -d)"
LOCKFILE="${TMPDIR}/lockfile"
READYFILE="${TMPDIR}/holder_ready"
cleanup() {
  set +e
  if [ -n "${HOLDER_PID:-}" ]; then
    kill "${HOLDER_PID}" >/dev/null 2>&1 || true
    wait "${HOLDER_PID}" 2>/dev/null || true
  fi
  rm -rf "${TMPDIR}"
}
trap cleanup EXIT
python3 - <<'PY' "$LOCKFILE" "$READYFILE" &
import fcntl, sys, time
lockfile, readyfile = sys.argv[1], sys.argv[2]
f = open(lockfile, "w")
fcntl.flock(f, fcntl.LOCK_EX)
open(readyfile, "w").write("ready\n")
time.sleep(10)
PY
HOLDER_PID=$!
for _ in $(seq 1 100); do
  [ -f "$READYFILE" ] && break
  sleep 0.05
done
[ -f "$READYFILE" ]
HELD_RESULT="$(python3 -c 'import fcntl,sys; f=open(sys.argv[1],"w");
try:
    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB); print("acquired")
except BlockingIOError:
    print("blocked")' "$LOCKFILE")"
echo "LOCK_WHILE_HELD:${HELD_RESULT}"
kill "${HOLDER_PID}" >/dev/null 2>&1 || true
wait "${HOLDER_PID}" 2>/dev/null || true
unset HOLDER_PID
RECOVERY_RESULT="$(python3 -c 'import fcntl,sys; f=open(sys.argv[1],"w");
try:
    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB); print("acquired")
except BlockingIOError:
    print("blocked")' "$LOCKFILE")"
echo "LOCK_AFTER_RELEASE:${RECOVERY_RESULT}"
"#,
        &[],
    );
    assert!(output.contains("LOCK_WHILE_HELD:blocked"));
    assert!(output.contains("LOCK_AFTER_RELEASE:acquired"));
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t4_sse_stream_and_reconnect_preflight() {
    let output = run_shell(
        r#"
set -euo pipefail
TMPDIR="$(mktemp -d)"
cleanup() {
  set +e
  if [ -n "${PID:-}" ]; then
    kill "${PID}" >/dev/null 2>&1 || true
    wait "${PID}" 2>/dev/null || true
  fi
  rm -rf "${TMPDIR}"
}
trap cleanup EXIT
cat > "${TMPDIR}/sse_server.py" <<'PY'
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            self.wfile.flush()
            return
        if self.path.startswith("/events"):
            last = self.headers.get("Last-Event-ID", "")
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            if last == "2":
                self.wfile.write(b": keep-alive\n\n")
                self.wfile.write(b"id: 3\n")
                self.wfile.write(b"event: message\n")
                self.wfile.write(b"data: {\"kind\":\"backfill\",\"from\":\"2\"}\n\n")
            else:
                self.wfile.write(b"id: 1\n")
                self.wfile.write(b"event: message\n")
                self.wfile.write(b"data: {\"kind\":\"ready\"}\n\n")
                self.wfile.write(b"id: 2\n")
                self.wfile.write(b"event: message\n")
                self.wfile.write(b"data: {\"kind\":\"steady\"}\n\n")
            self.wfile.flush()
            return
        self.send_response(404)
        self.end_headers()
    def log_message(self, *_):
        return
server = ThreadingHTTPServer(("127.0.0.1", 0), H)
port = server.server_address[1]
print(port, flush=True)
server.serve_forever()
PY
python3 "${TMPDIR}/sse_server.py" > "${TMPDIR}/port.txt" &
PID=$!
for _ in $(seq 1 100); do
  [ -s "${TMPDIR}/port.txt" ] && break
  sleep 0.05
done
PORT="$(cat "${TMPDIR}/port.txt")"
for _ in $(seq 1 100); do
  curl -fsS --max-time 1 "http://127.0.0.1:${PORT}/health" >/dev/null && break
  sleep 0.05
done
curl -fsS --max-time 1 "http://127.0.0.1:${PORT}/health" >/dev/null
FIRST="$(curl -sN --max-time 2 "http://127.0.0.1:${PORT}/events")"
LAST_ID="$(printf '%s\n' "$FIRST" | awk '/^id: / {print $2}' | tail -n1)"
SECOND="$(curl -sN --max-time 2 -H "Last-Event-ID: ${LAST_ID}" "http://127.0.0.1:${PORT}/events")"
KEEPALIVE_COUNT="$(printf '%s\n' "$SECOND" | awk '/^: keep-alive$/ {c++} END {print c+0}')"
echo "FIRST_LAST_ID:${LAST_ID}"
echo "KEEPALIVE_COUNT:${KEEPALIVE_COUNT}"
printf '%s\n' "$FIRST" | awk '/^data: / {print}'
printf '%s\n' "$SECOND" | awk '/^data: / {print}'
"#,
        &[],
    );
    assert!(output.contains("FIRST_LAST_ID:2"));
    assert!(output.contains("KEEPALIVE_COUNT:1"));
    assert!(output.contains("data: {\"kind\":\"ready\"}"));
    assert!(output.contains("data: {\"kind\":\"steady\"}"));
    assert!(output.contains("data: {\"kind\":\"backfill\",\"from\":\"2\"}"));
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t5_auth_oidc_jwks_endpoint_preflight() {
    if !skip_unless_gate_enabled("META_RUN_P0B_OIDC") {
        return;
    }

    let output = run_shell(
        r#"
set -euo pipefail
CONF="$(curl -sS --max-time 10 https://accounts.google.com/.well-known/openid-configuration)"
JWKS="$(CONF_JSON="$CONF" python3 -c 'import json,os; print(json.loads(os.environ["CONF_JSON"])["jwks_uri"])')"
echo "JWKS_URI:$JWKS"
KEY_COUNT="$(curl -sS -L --max-time 10 "$JWKS" | python3 -c 'import json,sys; print(len(json.loads(sys.stdin.read()).get("keys", [])))')"
echo "JWKS_KEY_COUNT:$KEY_COUNT"
set +e
CONF_JSON="$CONF" EXPECTED_JWKS="$JWKS" CANDIDATE_JWKS="${JWKS}?phase0b_mismatch=1" python3 - <<'PY'
import json
import os
import sys

discovered = json.loads(os.environ["CONF_JSON"])["jwks_uri"]
candidate = os.environ["CANDIDATE_JWKS"]
if discovered != os.environ["EXPECTED_JWKS"]:
    raise SystemExit(41)
if discovered != candidate:
    raise SystemExit(23)
raise SystemExit(0)
PY
MISMATCH_RC="$?"
set -e
echo "JWKS_MISMATCH_RC:${MISMATCH_RC}"
"#,
        &[],
    );
    let mut lines = output.lines();
    let jwks_uri = lines
        .next()
        .unwrap_or_default()
        .trim()
        .strip_prefix("JWKS_URI:")
        .unwrap_or_default()
        .to_string();
    let key_count = lines
        .next()
        .unwrap_or_default()
        .trim()
        .strip_prefix("JWKS_KEY_COUNT:")
        .unwrap_or_default()
        .to_string();
    let mismatch_rc = lines
        .next()
        .unwrap_or_default()
        .trim()
        .strip_prefix("JWKS_MISMATCH_RC:")
        .unwrap_or_default()
        .to_string();
    assert!(jwks_uri.starts_with("https://"));
    let count: usize = key_count.parse().expect("jwks key count");
    assert!(count > 0, "jwks must have at least one key");
    assert_eq!(mismatch_rc, "23");
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t6_module_family_process_preflight() {
    let module_families = ["routing", "delivery", "gating", "memory"];
    let (config, _temp_guard) = module_family_process_config(&module_families);

    for module_id in module_families {
        let event = run_discovered_module_once(&config, module_id, Duration::from_secs(1))
            .expect("module should emit ready event");
        assert_eq!(event.source, "module");
        match event.event {
            UnifiedEvent::Module(module_event) => {
                assert_eq!(module_event.module, module_id);
                assert_eq!(module_event.event_type, "ready");
                assert_eq!(module_event.payload["family"], module_id);
                assert_eq!(module_event.payload["health"]["state"], "healthy");
                assert_eq!(
                    module_event.payload["process_probe"],
                    format!("python-script-{module_id}")
                );
                assert_eq!(
                    module_event.payload["tools"]["list_method"],
                    format!("{module_id}/tools.list")
                );
                assert_eq!(
                    module_event.payload["tools"]["representative_call"]["method"],
                    format!("{module_id}/tool.call")
                );
            }
            other => panic!("expected module event, got {other:?}"),
        }
    }

    let runtime = start_meta_runtime(config, vec![], Duration::from_secs(1))
        .expect("runtime should start for module families");
    for module_id in module_families {
        let result = route_module_call(
            &runtime,
            &ModuleRouteRequest {
                module_id: module_id.to_string(),
                method: format!("{module_id}/tools.list"),
                params: serde_json::json!({"probe":"viability"}),
            },
            Duration::from_secs(1),
        );
        match module_id {
            "delivery" | "memory" => {
                let err = result.expect_err("module should require MCP in runtime routing");
                let err_text = format!("{err:?}");
                assert!(
                    err_text.contains("McpRequired"),
                    "expected MCP-required routing error for {module_id}, got {err_text}"
                );
                assert!(
                    err_text.contains(module_id),
                    "expected module id in error for {module_id}, got {err_text}"
                );
            }
            _ => {
                let response = result.expect("module route should be viable");
                assert_eq!(response.module_id, module_id);
                assert_eq!(response.method, format!("{module_id}/tools.list"));
                assert_eq!(
                    response.payload["tools"]["representative_call"]["params_schema"]["tool"],
                    "string"
                );
            }
        }
    }
}

#[test]
#[ignore = "external roadmap preflight check (phase 0b)"]
fn p0b_t7_sdk_console_toolchain_and_payload_preflight() {
    let config = module_probe_config(&["delivery", "routing"]);
    let mut runtime = start_meta_runtime(config, vec![], Duration::from_secs(1))
        .expect("runtime should start for sdk probe");

    let node_request = run_shell(
        r#"node -e 'const req={jsonrpc:"2.0",id:"sdk-node-1",method:"meta/capabilities",params:{}}; process.stdout.write(JSON.stringify(req));'"#,
        &[],
    );
    let rpc_response_json =
        handle_meta_rpc_json(&mut runtime, node_request.trim(), Duration::from_secs(1));
    let rpc_response: JsonRpcResponse =
        serde_json::from_str(&rpc_response_json).expect("rpc response should be valid");
    assert_eq!(rpc_response.id, serde_json::json!("sdk-node-1"));
    assert_eq!(
        rpc_response
            .result
            .as_ref()
            .and_then(|result| result.get("contract_version"))
            .and_then(|value| value.as_str()),
        Some(META_CONTRACT_VERSION)
    );
    let methods = rpc_response
        .result
        .as_ref()
        .and_then(|result| result.get("methods"))
        .and_then(|value| value.as_array())
        .expect("capabilities methods");
    assert!(
        methods
            .iter()
            .any(|method| method == "meta/events/subscribe")
    );

    let python_request = run_shell(
        r#"
python3 - <<'PY'
import json
print(json.dumps({"jsonrpc":"2.0","id":"sdk-py-1","method":"meta/status","params":{}}))
PY
"#,
        &[],
    );
    let py_response_json =
        handle_meta_rpc_json(&mut runtime, python_request.trim(), Duration::from_secs(1));
    let py_validation = run_shell(
        r#"
python3 - <<'PY'
import json, os
resp = json.loads(os.environ["RPC_RESPONSE"])
assert resp["jsonrpc"] == "2.0"
assert resp["id"] == "sdk-py-1"
assert resp["result"]["contract_version"] == os.environ["EXPECTED_CONTRACT"]
print("PY_RESPONSE_VALIDATED:1")
PY
"#,
        &[
            ("RPC_RESPONSE", py_response_json.as_str()),
            ("EXPECTED_CONTRACT", META_CONTRACT_VERSION),
        ],
    );
    assert!(py_validation.contains("PY_RESPONSE_VALIDATED:1"));

    let console_state = build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase0b_dataset".to_string(),
            table: "sessions".to_string(),
        },
        trusted_meta_toml: trusted_toml_for_sdk_console_probe(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec!["alice@example.com".to_string()],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("console decision state should build");

    let authorized = handle_console_rest_json_route(
        &console_state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GoogleOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );
    assert_eq!(authorized.status, 200);
    assert_eq!(
        authorized.body["modules"].as_array().map(|v| v.len()),
        Some(2)
    );

    let unauthorized = handle_console_rest_json_route(
        &console_state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: None,
        },
    );
    assert_eq!(unauthorized.status, 401);

    let _ = runtime.shutdown();
}
