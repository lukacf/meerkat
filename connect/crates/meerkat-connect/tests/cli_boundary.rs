use std::process::{Command, Stdio};
use std::io::Write;

#[test]
fn noninteractive_invocation_refuses_before_opening_realm_state() {
    let root = tempfile::tempdir().expect("temporary invocation root");
    let state = root.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_meerkat-connect"))
        .args(["--realm", "family", "--state-root"])
        .arg(&state)
        .stdin(Stdio::null())
        .output()
        .expect("run CLI");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires an interactive terminal"));
    assert!(
        !state.exists(),
        "rejected batch invocation must not create a realm"
    );
}

#[test]
fn help_is_available_without_a_terminal_or_provider_auth() {
    let output = Command::new(env!("CARGO_BIN_EXE_meerkat-connect"))
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .expect("CLI help");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--realm"));
}

fn stdio_frames(input: &str) -> (Vec<serde_json::Value>, std::process::Output) {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_meerkat-connect"))
        .arg("--stdio")
        .current_dir(root.path())
        .env_clear()
        .env("HOME", root.path())
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    let frames = String::from_utf8(output.stdout.clone()).unwrap().lines()
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert!(!root.path().join(".rkat").exists(), "refused input must not open native storage");
    (frames, output)
}

#[test]
fn stdio_refuses_unbound_controls_without_opening_a_native_session() {
    let (frames, output) = stdio_frames(concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"h:send\",\"method\":\"connect/send\",\"params\":{\"scope\":\"forged\",\"text\":\"never admitted\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"h:answer\",\"method\":\"connect/prompt\",\"params\":{\"actor\":\"forged-viewer\"}}\n"
    ));
    assert_eq!(frames.len(), 2, "stdio must return bounded protocol refusals: {:?}", output.status);
    assert_eq!(frames[0]["id"], "h:send");
    assert_eq!(frames[0]["error"]["data"]["code"], "invalid_binding");
    assert_eq!(frames[1]["error"]["code"], -32601);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("forged-viewer"));
}

#[test]
fn stdio_rejects_mutation_notifications_and_duplicate_decoded_keys() {
    let (frames, output) = stdio_frames(concat!(
        "{\"jsonrpc\":\"2.0\",\"method\":\"connect/send\",\"params\":{\"text\":\"private-frame-canary\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"h:duplicate\",\"method\":\"connect/initialize\",\"params\":{\"viewer\":{},\"\\u0076iewer\":{\"principal_id\":\"private-frame-canary\"}}}\n"
    ));
    assert_eq!(frames.len(), 2, "unsupported stdio is not a protocol refusal: {:?}", output.status);
    for frame in frames {
        assert!(matches!(frame["error"]["code"].as_i64(), Some(-32700 | -32600 | -32602)));
        assert!(!frame.to_string().contains("private-frame-canary"));
    }
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-frame-canary"));
}
