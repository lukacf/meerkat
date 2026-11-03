//! Test-only custody of the byte-frozen Forge auth process. No live application.
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
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::{sleep, timeout};

pub const PASSWORD: &str = "  fixture läsbar 密码 +&= password  ";
const BOUND: Duration = Duration::from_secs(15);

pub struct Owner {
    pub origin: String,
    root: tempfile::TempDir,
    child: Option<Child>,
}
impl Owner {
    pub async fn start() -> Self {
        let interpreter = std::env::var("CONNECT_FORGE_TEST_PYTHON")
            .expect("explicit existing Forge interpreter required");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/forge-auth");
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
                    Sha256::digest(fs::read(fixture.join(name)).unwrap())
                ),
                digest
            );
        }
        let root = tempfile::tempdir().unwrap();
        let child = Command::new(interpreter)
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
        let mut owner = Self {
            origin: String::new(),
            root,
            child: Some(child),
        };
        let mut stdout = owner.child.as_mut().unwrap().stdout.take().unwrap();
        let flags = OFlag::from_bits_retain(fcntl(&stdout, FcntlArg::F_GETFL).unwrap());
        fcntl(&stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let line = timeout(BOUND, async {
            let mut line = Vec::new();
            loop {
                let mut byte = [0];
                match stdout.read(&mut byte) {
                    Ok(0) => panic!("Forge owner ended before its startup receipt"),
                    Ok(_) if byte[0] == b'\n' => break line,
                    Ok(_) => {
                        line.push(byte[0]);
                        assert!(line.len() <= 1024);
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
        owner.origin = serde_json::from_slice::<Value>(&line).unwrap()["origin"]
            .as_str()
            .unwrap()
            .into();
        owner
    }

    /// Canary values remain in this temporary synthetic owner only; callers
    /// assert they are absent elsewhere, then let the TempDir remove them.
    pub async fn finish(&mut self) -> Vec<String> {
        self.child
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(b"x")
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
        assert_eq!(
            result,
            json!({"requests":[{"method":"GET","path":"/login","status":200},
            {"method":"POST","path":"/login","status":303},
            {"method":"GET","path":"/console/forge/frontends","status":501}],
            "sessions_after_close":0,"server_task_done":true,"producer":false,"host_identity":false})
        );
        let canaries: Vec<String> = serde_json::from_slice(
            &fs::read(self.root.path().join("private-canaries.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(canaries.len(), 2);
        canaries
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
