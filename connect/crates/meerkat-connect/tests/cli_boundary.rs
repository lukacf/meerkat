use std::io::Write;
use std::process::{Command, Stdio};

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
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let frames = String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert!(
        !root.path().join(".rkat").exists(),
        "refused input must not open native storage"
    );
    (frames, output)
}

#[test]
fn stdio_refuses_unbound_controls_without_opening_a_native_session() {
    let (frames, output) = stdio_frames(concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"h:send\",\"method\":\"connect/send\",\"params\":{\"scope\":\"forged\",\"text\":\"never admitted\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"h:answer\",\"method\":\"connect/prompt\",\"params\":{\"actor\":\"forged-viewer\"}}\n"
    ));
    assert_eq!(
        frames.len(),
        2,
        "stdio must return bounded protocol refusals: {:?}",
        output.status
    );
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
    assert_eq!(
        frames.len(),
        2,
        "unsupported stdio is not a protocol refusal: {:?}",
        output.status
    );
    for frame in frames {
        assert!(matches!(
            frame["error"]["code"].as_i64(),
            Some(-32700 | -32600 | -32602)
        ));
        assert!(!frame.to_string().contains("private-frame-canary"));
    }
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-frame-canary"));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// One Forge login's `meerkat-connect --stdio`, started as the Forge front
/// starts one for each login: a cleared environment with the application's
/// provider key, and the application's one realm (`forge-<application_id>`).
struct ForgeLogin {
    child: std::process::Child,
    frames: std::sync::mpsc::Receiver<serde_json::Value>,
}

impl ForgeLogin {
    const BOUND: std::time::Duration = std::time::Duration::from_secs(90);

    fn start(root: &std::path::Path, login: &str) -> Self {
        let user = root.join("user");
        let mut child = Command::new(env!("CARGO_BIN_EXE_meerkat-connect"))
            .arg("--stdio")
            .current_dir(root)
            .env_clear()
            .env("HOME", &user)
            .env("PATH", "")
            .env("OPENAI_API_KEY", "sk-fixture-concurrent-openers")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (frames_out, frames) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let frame = serde_json::from_str(&line).expect("protocol-only stdout");
                if frames_out.send(frame).is_err() {
                    break;
                }
            }
        });
        let initialize = serde_json::json!({"jsonrpc":"2.0","id":"h:initialize",
            "method":"connect/initialize","params":{"protocol_version":1,
            "application":{"application_id":"concurrent","instance_id":login,
                "origin":"http://127.0.0.1:12345"},
            "viewer":{"principal_id":login,"login_scope_id":login,
                "expires_at_ms":now_ms() + 300_000},
            "realm":{"realm_id":"forge-concurrent","context_root":root,
                "state_root":root.join("state"),"user_config_root":user},
            "model":"gpt-5.4"}});
        let mut line = serde_json::to_vec(&initialize).unwrap();
        line.push(b'\n');
        child.stdin.as_mut().unwrap().write_all(&line).unwrap();
        Self { child, frames }
    }

    /// The answer to `connect/initialize`, past any notification before it.
    fn opened(&self) -> serde_json::Value {
        let deadline = std::time::Instant::now() + Self::BOUND;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let frame = self
                .frames
                .recv_timeout(left)
                .expect("connect/initialize answered within the bound");
            if frame["id"] == "h:initialize" {
                return frame;
            }
        }
    }

    /// End the input: the process retires its session and exits.
    fn finish(mut self) -> std::process::ExitStatus {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + Self::BOUND;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the process exits after its input ends"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The Forge front starts one `meerkat-connect --stdio` for each login, all
/// on the application's one realm. Two logins that start at the same moment,
/// on a realm neither has created yet, each open their own session on it; a
/// third login opens it while both are open; and each process ends cleanly.
#[test]
fn concurrent_forge_logins_open_one_application_realm() {
    let root = tempfile::tempdir().unwrap();
    let first = ForgeLogin::start(root.path(), "login-a");
    let second = ForgeLogin::start(root.path(), "login-b");
    let opened = [first.opened(), second.opened()];
    for frame in &opened {
        assert!(
            frame["error"].is_null(),
            "a concurrent opener could not open the realm: {frame}"
        );
        assert!(frame["result"]["session_id"].is_string(), "{frame}");
    }
    assert_ne!(
        opened[0]["result"]["session_id"], opened[1]["result"]["session_id"],
        "each login opens its own session"
    );
    let third = ForgeLogin::start(root.path(), "login-c");
    let later = third.opened();
    assert!(
        later["error"].is_null(),
        "a later login could not open the realm the others hold open: {later}"
    );
    for login in [first, second, third] {
        assert!(login.finish().success());
    }
}
