//! Test-only custody of the byte-frozen PairedHost management process. No live application.
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::time::{sleep, timeout};

const BOUND: Duration = Duration::from_secs(15);
const DIAGNOSTIC_LIMIT: usize = 64 * 1024;

#[derive(Default)]
struct Diagnostic {
    prefix: Vec<u8>,
    bytes: u64,
    digest: Sha256,
    eof: bool,
    read_failed: bool,
}

pub struct Owner {
    pub origin: String,
    pub code: String,
    root: Option<tempfile::TempDir>,
    child: Option<Child>,
    stdout: Option<ChildStdout>,
    pgid: Pid,
    group_owned: bool,
    diagnostic: Arc<Mutex<Diagnostic>>,
    stop_drain: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
    drain_joined: bool,
    settled: Option<Value>,
}
impl Owner {
    pub async fn start() -> Self {
        let interpreter = std::env::var("CONNECT_PAIRED_HOST_TEST_PYTHON")
            .expect("explicit isolated PairedHost interpreter required");
        let source = std::env::var("CONNECT_PAIRED_HOST_TEST_SOURCE")
            .expect("explicit immutable PairedHost source required");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paired-host-role");
        let root = tempfile::tempdir().unwrap();
        let child = Command::new(interpreter)
            .args(["-I", "-B"])
            .arg(fixture.join("owner.py"))
            .arg(&source)
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
        // Own the child before any fallible pipe or reader setup.
        let pgid = Pid::from_raw(child.id() as i32);
        let mut owner = Self {
            origin: String::new(),
            code: String::new(),
            root: Some(root),
            child: Some(child),
            stdout: None,
            pgid,
            group_owned: true,
            diagnostic: Arc::new(Mutex::new(Diagnostic::default())),
            stop_drain: Arc::new(AtomicBool::new(false)),
            drain: None,
            drain_joined: false,
            settled: None,
        };
        let Some(stdout) = owner.child.as_mut().unwrap().stdout.take() else {
            owner.fail("stdout_missing", 0);
        };
        let flags = match fcntl(&stdout, FcntlArg::F_GETFL) {
            Ok(flags) => OFlag::from_bits_retain(flags),
            Err(_) => owner.fail("stdout_flags", 0),
        };
        if fcntl(&stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).is_err() {
            owner.fail("stdout_nonblocking", 0);
        }
        owner.stdout = Some(stdout);
        let Some(mut stderr) = owner.child.as_mut().unwrap().stderr.take() else {
            owner.fail("stderr_missing", 0);
        };
        let flags = match fcntl(&stderr, FcntlArg::F_GETFL) {
            Ok(flags) => OFlag::from_bits_retain(flags),
            Err(_) => owner.fail("stderr_flags", 0),
        };
        if fcntl(&stderr, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).is_err() {
            owner.fail("stderr_nonblocking", 0);
        }
        let diagnostic = owner.diagnostic.clone();
        let stop_drain = owner.stop_drain.clone();
        let capture = diagnostic.clone();
        let stop = stop_drain.clone();
        // Drain throughout the native trajectory. Only a bounded private prefix
        // is retained; the full stream contributes to the byte count and hash.
        let drain = thread::Builder::new()
            .name("paired-host-fixture-stderr".into())
            .spawn(move || {
                let mut buffer = [0; 8192];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => {
                            capture.lock().unwrap().eof = true;
                            break;
                        }
                        Ok(n) => {
                            let mut capture = capture.lock().unwrap();
                            capture.bytes += n as u64;
                            capture.digest.update(&buffer[..n]);
                            let keep = n.min(DIAGNOSTIC_LIMIT - capture.prefix.len());
                            capture.prefix.extend_from_slice(&buffer[..keep]);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => {
                            capture.lock().unwrap().read_failed = true;
                            break;
                        }
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                }
            });
        owner.drain = match drain {
            Ok(handle) => Some(handle),
            Err(_) => owner.fail("stderr_reader_spawn", 0),
        };
        let started = owner.line("startup").await;
        let Some(origin) = started["origin"].as_str() else {
            owner.fail("startup_shape", 0);
        };
        let Some(code) = started["code"].as_str() else {
            owner.fail("startup_shape", 0);
        };
        owner.origin = origin.into();
        owner.code = code.into();
        owner
    }

    async fn line(&mut self, phase: &'static str) -> Value {
        let mut line = Vec::new();
        let received = timeout(BOUND, async {
            loop {
                let mut byte = [0];
                match self.stdout.as_mut().unwrap().read(&mut byte) {
                    Ok(0) => return false,
                    Ok(_) if byte[0] == b'\n' => return true,
                    Ok(_) => {
                        line.push(byte[0]);
                        if line.len() > 1024 * 1024 {
                            return false;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        sleep(Duration::from_millis(2)).await;
                    }
                    Err(_) => return false,
                }
            }
        })
        .await;
        if !matches!(received, Ok(true)) {
            self.fail(phase, line.len());
        }
        match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => self.fail(phase, line.len()),
        }
    }

    pub async fn control(&mut self, command: &str) -> Value {
        let mut bytes = serde_json::to_vec(command).unwrap();
        bytes.push(b'\n');
        if self
            .child
            .as_mut()
            .unwrap()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .is_err()
        {
            self.fail("control_write", 0);
        }
        let value = self.line("control_read").await;
        assert_eq!(value["ack"], command);
        value["observation"].clone()
    }

    // Every assertion failure retains only fixed lifecycle fields publicly.
    // Raw stderr stays bounded in the private temporary directory, never stdout.
    fn settle(&mut self, terminate: bool) -> Value {
        if let Some(value) = self.settled.clone() {
            if terminate && (self.child.is_some() || self.group_owned) {
                // Preserve the first failed observation even when cleanup later
                // succeeds. Reaping the direct child does not surrender its PGID.
                self.settled = None;
                let cleanup = self.settle(true);
                let mut failed = value;
                failed["failure_cleanup"] = cleanup;
                self.settled = Some(failed.clone());
                return failed;
            }
            return value;
        }
        let pid = self.pgid;
        let already_exited = self
            .child
            .as_mut()
            .is_none_or(|child| matches!(child.try_wait(), Ok(Some(_))));
        let kill = if terminate && killpg(pid, None) != Err(Errno::ESRCH) {
            Some(killpg(pid, Signal::SIGKILL).map_err(|e| e as i32))
        } else {
            None
        };
        let deadline = Instant::now() + BOUND;
        let mut status = None;
        let mut wait_failed = false;
        if let Some(child) = self.child.as_mut() {
            loop {
                match child.try_wait() {
                    Ok(Some(value)) => {
                        status = Some(value);
                        break;
                    }
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Ok(None) => break,
                    Err(_) => {
                        wait_failed = true;
                        break;
                    }
                }
            }
        }
        let mut absent = killpg(pid, None) == Err(Errno::ESRCH);
        if terminate {
            while !absent && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
                absent = killpg(pid, None) == Err(Errno::ESRCH);
            }
        }
        self.group_owned = !absent;
        // Normal exit closes stderr. On broken cleanup, stop the nonblocking
        // reader too; never wait indefinitely for an inherited pipe writer.
        let drain_deadline = Instant::now() + Duration::from_secs(1);
        while self.drain.as_ref().is_some_and(|h| !h.is_finished())
            && Instant::now() < drain_deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        self.stop_drain.store(true, Ordering::Release);
        let stop_deadline = Instant::now() + Duration::from_secs(1);
        while self.drain.as_ref().is_some_and(|h| !h.is_finished())
            && Instant::now() < stop_deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        if self.drain.as_ref().is_some_and(|h| h.is_finished()) {
            self.drain_joined = self.drain.take().unwrap().join().is_ok();
        }
        let drain_joined = self.drain_joined;
        let capture = self.diagnostic.lock().unwrap();
        let value = json!({"pid": pid.as_raw(), "already_exited": already_exited,
            "exit_code": status.and_then(|s| s.code()), "signal": status.and_then(|s| s.signal()),
            "child_joined": self.child.is_none() || status.is_some(), "wait_failed": wait_failed,
            "kill_result": kill, "process_group_absent": absent,
            "stderr_bytes": capture.bytes, "stderr_sha256": format!("{:x}", capture.digest.clone().finalize()),
            "stderr_truncated": capture.bytes > capture.prefix.len() as u64,
            "stderr_eof": capture.eof, "stderr_read_failed": capture.read_failed,
            "stderr_reader_joined": drain_joined});
        self.settled = Some(value.clone());
        if status.is_some() {
            self.child.take();
        }
        value
    }

    fn fail(&mut self, phase: &'static str, partial_stdout_bytes: usize) -> ! {
        let mut value = self.settle(true);
        value["phase"] = json!(phase);
        value["partial_stdout_bytes"] = json!(partial_stdout_bytes);
        self.retain_failure(&value);
        panic!("isolated PairedHost fixture failed: {value}");
    }

    fn retain_failure(&mut self, value: &Value) {
        if let Some(root) = self.root.take() {
            let path = root.keep();
            let capture = self.diagnostic.lock().unwrap();
            let stderr_path = path.join("stderr.private");
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut file) = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(stderr_path)
            {
                let _ = file.write_all(&capture.prefix);
            }
            let _ = fs::write(
                path.join("runner-result.json"),
                serde_json::to_vec_pretty(value).unwrap(),
            );
            eprintln!("PairedHost fixture private diagnostics: {}", path.display());
        }
    }

    /// Canary values remain in this temporary synthetic owner only; callers
    /// assert they are absent elsewhere, then let the TempDir remove them.
    pub async fn finish(&mut self) -> (Value, Vec<String>) {
        if self
            .child
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(b"\"stop\"\n")
            .is_err()
        {
            self.fail("stop_write", 0);
        }
        let joined = timeout(BOUND, async {
            loop {
                match self.child.as_mut().unwrap().try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) => sleep(Duration::from_millis(2)).await,
                    Err(_) => break false,
                }
            }
        })
        .await;
        if !matches!(joined, Ok(true)) {
            self.fail("join", 0);
        }
        let value = self.settle(false);
        if value["exit_code"] != 0
            || value["process_group_absent"] != true
            || value["stderr_bytes"] != 0
            || value["stderr_eof"] != true
            || value["stderr_reader_joined"] != true
            || value["stderr_read_failed"] != false
        {
            self.fail("settlement", 0);
        }
        let root = self.root.as_ref().unwrap().path();
        let result: Value =
            serde_json::from_slice(&fs::read(root.join("result.json")).unwrap()).unwrap();
        assert_eq!(result["server_task_done"], true);
        assert_eq!(result["native_runtime_started"], false);
        assert_eq!(result["provider_started"], false);
        let canaries: Vec<String> =
            serde_json::from_slice(&fs::read(root.join("private-canaries.json")).unwrap()).unwrap();
        assert!(canaries.len() >= 2);
        (result, canaries)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.child.is_some() || self.group_owned {
            let value = self.settle(true);
            self.retain_failure(&value);
            eprintln!("PairedHost fixture failure cleanup: {value}");
        }
    }
}
