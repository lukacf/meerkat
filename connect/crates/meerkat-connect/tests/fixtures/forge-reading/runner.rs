//! Test-only custody of frozen Forge configuration and isolated native Reading owners.
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    time::Duration,
};
use tokio::time::{sleep, timeout};

pub const PASSWORD: &str = "  fixture läsbar 密码 +&= password  ";
const BOUND: Duration = Duration::from_secs(15);
// Existing 900-second installed startup budget plus bounded pre-connect fixture work.
const NATIVE_STARTUP_BOUND: Duration = Duration::from_secs(900 + 60);

pub struct Owner {
    pub origin: String,
    root: tempfile::TempDir,
    child: Option<Child>,
    stdout: ChildStdout,
}
impl Owner {
    pub async fn start() -> Self {
        let interpreter = std::env::var("CONNECT_FORGE_TEST_PYTHON")
            .expect("explicit existing Forge interpreter required");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/forge-reading");
        verify_application_sources(&fixture);
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(interpreter)
            .args(["-I", "-B"])
            .arg(fixture.join("owner.py"))
            .arg(root.path())
            .env_clear()
            .env("HOME", root.path())
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let flags = OFlag::from_bits_retain(fcntl(&stdout, FcntlArg::F_GETFL).unwrap());
        fcntl(&stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let mut owner = Self {
            origin: String::new(),
            root,
            child: Some(child),
            stdout,
        };
        owner.origin = owner.line().await["origin"].as_str().unwrap().into();
        owner
    }

    async fn line(&mut self) -> Value {
        let line = timeout(BOUND, async {
            let mut line = Vec::new();
            loop {
                let mut byte = [0];
                match self.stdout.read(&mut byte) {
                    Ok(0) => panic!("Forge owner ended before its startup receipt"),
                    Ok(_) if byte[0] == b'\n' => break line,
                    Ok(_) => {
                        line.push(byte[0]);
                        assert!(line.len() <= 1024 * 1024);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        sleep(Duration::from_millis(2)).await
                    }
                    Err(error) => panic!("Forge owner startup pipe: {error}"),
                }
            }
        })
        .await
        .expect("bounded Forge owner startup");
        serde_json::from_slice(&line).unwrap()
    }

    pub async fn control(&mut self, command: &str) -> Value {
        let mut bytes = serde_json::to_vec(command).unwrap();
        bytes.push(b'\n');
        self.child
            .as_mut()
            .unwrap()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let value = self.line().await;
        assert_eq!(value["ack"], command);
        value["observation"].clone()
    }

    /// Canary values remain in this temporary synthetic owner only; callers
    /// assert they are absent elsewhere, then let the TempDir remove them.
    pub async fn finish(&mut self) -> (Value, Vec<String>) {
        self.child
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(b"\"stop\"\n")
            .unwrap();
        timeout(BOUND, async {
            while self.child.as_mut().unwrap().try_wait().unwrap().is_none() {
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("bounded Forge owner join");
        let child = self.child.take().unwrap();
        let pid = Pid::from_raw(child.id() as i32);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "synthetic owner stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(killpg(pid, None), Err(Errno::ESRCH));
        let result: Value =
            serde_json::from_slice(&fs::read(self.root.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result["server_task_done"], true);
        assert_eq!(result["application_task_done"], true);
        assert_eq!(result["sessions"], 0);
        assert_eq!(result["token_present"], false);
        let canaries: Vec<String> = serde_json::from_slice(
            &fs::read(self.root.path().join("private-canaries.json")).unwrap(),
        )
        .unwrap();
        assert!(canaries.len() >= 2);
        (result, canaries)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
            let _ = child.wait();
        }
    }
}

fn verify_application_sources(fixture: &Path) {
    for (name, digest) in [
        (
            "console_auth.py",
            "6ac2c413749834d4a0a2059b780270125658dd252835b3908c6b612b58e86496",
        ),
        (
            "console_configuration.py",
            "c7466967f62bd988654bcdb1743f52512724ed8bc72665b997c502a828ac7876",
        ),
    ] {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(
                    fs::read(fixture.parent().unwrap().join("forge-auth").join(name)).unwrap()
                )
            ),
            digest
        );
    }
    for (name, digest) in [
        (
            "__init__.py",
            "3892d46c790ac41a3aaa96e56b0f553b608246f5a890d69a1b490994a67cc9c9",
        ),
        (
            "frontends.py",
            "76fbf315035a5578e3d6cd53226d027a65fdee5523a81bd17562dd346b9275d6",
        ),
        (
            "collection.py",
            "04992f703791704c51cc56415b9d35c7942188e1f78f507de345beddc424bbd7",
        ),
        (
            "telegram_runtime.py",
            "8a9ad9d005cceab26a492812ce9487b7571ccc934811dcdc8e345bb275fd57c8",
        ),
    ] {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(fixture.join("application").join(name)).unwrap())
            ),
            digest
        );
    }
}

/// Custody of the same persistent native owner used by the HTTP scenario.
/// Releases only fixture observations; management remains on the real CLI route.
pub struct NativeOwner {
    pub origin: String,
    pub root: std::path::PathBuf,
    child: Option<Child>,
    stdout: ChildStdout,
    evidence: Vec<Value>,
}

pub const NATIVE_PASSWORD: &str = "reading-loopback-private-password";
pub const NATIVE_TOKEN: &str = "987654321:reading-loopback-private-token";

impl NativeOwner {
    pub async fn start() -> Self {
        let input_path = std::env::var("CONNECT_READING_NATIVE_INPUT")
            .expect("exact Reading native owner inputs required");
        let input_bytes = fs::read(&input_path).unwrap();
        let expected = std::env::var("CONNECT_READING_NATIVE_INPUT_SHA256")
            .expect("exact Reading input hash required");
        assert_eq!(format!("{:x}", Sha256::digest(&input_bytes)), expected);
        let input: Value = serde_json::from_slice(&input_bytes).unwrap();
        let keys: std::collections::BTreeSet<_> = input.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["python", "python_sha256", "toolkit_gateway", "toolkit_python", "meta_python",
                         "installed_staging", "installed_staging_sha256", "sdk_selection", "sdk_selection_sha256"].into_iter().collect());
        let python = Path::new(input["python"].as_str().unwrap());
        assert!(python.is_absolute());
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(python).unwrap())), input["python_sha256"].as_str().unwrap());
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/forge-reading");
        verify_application_sources(&fixture);
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(fixture.join("native_console_auth.py")).unwrap())),
                   "15c66c4097aa0e1ffcf5569cf46d40295e80c49ed3741ad2dffef3ee0786e317");
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(fixture.join("native_owner.py")).unwrap())),
                   "f86fd6c424eac640ee75f71fbf7b90bb0151582bb83b5a36f7b3b1dcd8046524");
        let root = tempfile::Builder::new().prefix("connect-reading-native-cli-").tempdir().unwrap().keep();
        fs::create_dir(root.join("tmp")).unwrap();
        fs::write(root.join("selected-input.json"), input_bytes).unwrap();
        let stderr = fs::OpenOptions::new().write(true).create_new(true).open(root.join("owner.stderr")).unwrap();
        let mut command = Command::new(python);
        command.args(["-I", "-B"]).arg(fixture.join("native_owner.py")).arg("--cli-owner");
        for key in ["toolkit_gateway", "toolkit_python", "meta_python", "installed_staging", "sdk_selection"] {
            let value = Path::new(input[key].as_str().unwrap());
            assert!(value.is_absolute());
            command.arg(format!("--{}", key.replace('_', "-"))).arg(value);
        }
        for key in ["installed_staging_sha256", "sdk_selection_sha256"] {
            command.arg(format!("--{}", key.replace('_', "-"))).arg(input[key].as_str().unwrap());
        }
        command.arg("--output").arg(root.join("native"))
            .env_clear().env("PATH", "/usr/bin:/bin").env("TMPDIR", root.join("tmp"))
            .current_dir(&root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(stderr)).process_group(0);
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().expect("spawned piped Reading stdout");
        // Establish the existing process-group guard before fallible pipe setup.
        let mut owner = Self { origin: String::new(), root, child: Some(child), stdout, evidence: Vec::new() };
        let flags = OFlag::from_bits_retain(fcntl(&owner.stdout, FcntlArg::F_GETFL).unwrap());
        fcntl(&owner.stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let ready = owner.phase_with_bound("setup-client-exited", NATIVE_STARTUP_BOUND).await;
        owner.origin = ready["observation"]["origin"].as_str().unwrap().to_owned();
        assert!(owner.origin.starts_with("http://127.0.0.1:"));
        owner
    }

    pub async fn phase(&mut self, expected: &str) -> Value {
        self.phase_with_bound(expected, Duration::from_secs(180)).await
    }

    async fn phase_with_bound(&mut self, expected: &str, bound: Duration) -> Value {
        let line = timeout(bound, async {
            let mut line = Vec::new();
            loop {
                let mut byte = [0];
                match self.stdout.read(&mut byte) {
                    Ok(0) => panic!("native Reading owner ended before its observation; evidence {}", self.root.display()),
                    Ok(_) if byte[0] == b'\n' => break line,
                    Ok(_) => { line.push(byte[0]); assert!(line.len() <= 256 * 1024); }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => sleep(Duration::from_millis(2)).await,
                    Err(_) => panic!("native Reading owner pipe failed"),
                }
            }
        }).await.expect("bounded native Reading observation");
        let value: Value = serde_json::from_slice(&line).unwrap();
        self.evidence.push(serde_json::json!({"direction":"owner", "value":value}));
        self.save_evidence();
        assert_eq!(value["phase"], expected, "native Reading phase differs; inspect retained owner result");
        value
    }

    pub fn release(&mut self, phase: &str, exited: Option<(u32, i32)>) {
        let mut value = serde_json::json!({"release":phase});
        if let Some((pid, exit_code)) = exited {
            assert_eq!(exit_code, 0);
            value["client_exit"] = serde_json::json!({"pid":pid,"exit_code":exit_code});
        }
        let mut bytes = serde_json::to_vec(&value).unwrap(); bytes.push(b'\n');
        self.child.as_mut().unwrap().stdin.as_mut().unwrap().write_all(&bytes).unwrap();
        self.evidence.push(serde_json::json!({"direction":"harness", "value":value}));
        self.save_evidence();
    }

    fn save_evidence(&self) {
        fs::write(self.root.join("control-observations.json"), serde_json::to_vec_pretty(&self.evidence).unwrap()).unwrap();
    }

    pub async fn finish(&mut self) -> Value {
        let complete = self.phase("finished").await;
        timeout(Duration::from_secs(30), async {
            while self.child.as_mut().unwrap().try_wait().unwrap().is_none() { sleep(Duration::from_millis(2)).await; }
        }).await.expect("native Reading owner bounded normal join");
        // Retain custody until every terminal, result and descendant check has
        // passed. An assertion must still leave Drop able to clear this group.
        let pid = Pid::from_raw(self.child.as_ref().unwrap().id() as i32);
        assert!(self.child.as_mut().unwrap().wait().unwrap().success(), "native Reading owner failed; evidence {}", self.root.display());
        assert_eq!(complete["status"], "passed");
        assert_eq!(complete["result_path"], self.root.join("native/result.json").to_str().unwrap());
        let result: Value = serde_json::from_slice(&fs::read(self.root.join("native/result.json")).unwrap()).unwrap();
        assert_eq!(result["status"], "passed");
        assert_eq!(result["management"]["owner_survived_management_clients"], true);
        assert_eq!(result["owner_survived_setup_exit"], true);
        assert_eq!(killpg(pid, None), Err(Errno::ESRCH));
        self.child.take();
        eprintln!("retained actual CLI Reading management evidence: {}", self.root.display());
        result
    }
}

impl Drop for NativeOwner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
            let _ = child.wait();
            eprintln!("native Reading owner required forced fixture cleanup; evidence {}", self.root.display());
        }
    }
}
