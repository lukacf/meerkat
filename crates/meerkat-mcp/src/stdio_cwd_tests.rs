//! A stdio server's configured working directory
//! ([`McpStdioConfig::cwd`](meerkat_core::mcp_config::McpStdioConfig::cwd))
//! at the launch seam: where the process runs, and every refusal before it
//! exists.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::McpRouter;
use meerkat_core::{McpStdioCwdError, McpStdioCwdRefusal};
use std::collections::HashMap;

const BOUND: Duration = Duration::from_secs(15);

/// The configuration of `name` launching `command` with the working
/// directory set as written, bypassing the builder's own check.
fn with_raw_cwd(name: &str, command: &str, cwd: &str) -> McpServerConfig {
    let mut config = McpServerConfig::stdio(name, command, vec![], HashMap::new());
    let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
        unreachable!("a stdio config");
    };
    stdio.cwd = Some(cwd.to_owned());
    config
}

/// A configured working directory that is missing, not a directory, or not
/// inspectable refuses the launch typed, and nothing is spawned: the launch
/// never falls back to another directory. (Fails-old: the field was
/// launched without a working directory, so the server ran in the host's.)
#[cfg(unix)]
#[tokio::test]
async fn a_missing_or_non_directory_cwd_refuses_the_launch_and_spawns_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("entered");
    let file = root.join("not-a-directory");
    std::fs::write(&file, b"a file").unwrap();
    for (cwd, reason) in [
        (root.join("missing"), McpStdioCwdRefusal::Missing),
        (file.clone(), McpStdioCwdRefusal::NotADirectory),
        (file.join("below-a-file"), McpStdioCwdRefusal::Inaccessible),
    ] {
        let config = McpServerConfig::stdio(
            "cwd-launch",
            "/bin/sh",
            vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
            HashMap::from([("MARKER".into(), marker.display().to_string())]),
        )
        .with_cwd(cwd.display().to_string())
        .expect("an absolute directory is a valid configuration");
        let result = tokio::time::timeout(
            BOUND,
            McpConnection::connect_and_enumerate_with_stdio_profile(
                &config,
                &McpStdioLaunchProfile::trusted_host(),
            ),
        )
        .await
        .expect("the refusal is bounded");
        let Err(McpError::WorkingDirectory(error)) = result else {
            panic!("{cwd:?} must refuse the launch with its working directory");
        };
        assert_eq!(
            error,
            McpStdioCwdError {
                server: "cwd-launch".into(),
                reason,
            }
        );
        assert!(
            !error.to_string().contains(&root.display().to_string()),
            "the refusal names the server, never the path: {error}"
        );
        assert!(!marker.exists(), "nothing was spawned for {cwd:?}");
    }
}

/// A relative working directory, and with one set a command that is a
/// relative path with a separator (here the expanded form of an `mcp.toml`
/// `command = "${TOOL}"` with `TOOL=bin/srv`), are refused before staging,
/// reload staging and connection, with no effect. (Fails-old: they were
/// staged and launched.)
#[cfg(unix)]
#[tokio::test]
async fn a_relative_cwd_or_an_ambiguous_command_is_refused_before_any_effect() {
    for (command, cwd, reason) in [
        ("/bin/sh", "relative/dir", McpStdioCwdRefusal::NotAbsolute),
        ("/bin/sh", "", McpStdioCwdRefusal::NotAbsolute),
        ("bin/srv", "/srv/cwd", McpStdioCwdRefusal::AmbiguousCommand),
        (
            "./bin/srv",
            "/srv/cwd",
            McpStdioCwdRefusal::AmbiguousCommand,
        ),
    ] {
        let config = with_raw_cwd("cwd-config", command, cwd);
        let expected = McpStdioCwdError {
            server: "cwd-config".into(),
            reason,
        };
        let connected = McpConnection::connect_and_enumerate_with_stdio_profile(
            &config,
            &McpStdioLaunchProfile::trusted_host(),
        )
        .await;
        assert!(
            matches!(&connected, Err(McpError::WorkingDirectory(error)) if *error == expected),
            "connect {command} in {cwd:?}"
        );
        let mut router = McpRouter::new_with_stdio_profile(McpStdioLaunchProfile::trusted_host());
        let before = router.external_tool_surface_snapshot();
        assert!(
            matches!(
                router.stage_add(config.clone()),
                Err(McpError::WorkingDirectory(error)) if error == expected
            ),
            "stage_add {command} in {cwd:?}"
        );
        assert!(
            matches!(
                router.stage_reload(config.clone()),
                Err(McpError::WorkingDirectory(error)) if error == expected
            ),
            "stage_reload {command} in {cwd:?}"
        );
        assert_eq!(router.external_tool_surface_snapshot(), before);
        assert!(router.active_server_names().is_empty());
    }
}

/// An attempt refused for its working directory, at configuration or at
/// launch, is a configuration refusal, not a setup failure.
#[test]
fn a_working_directory_refusal_is_an_invalid_config_disposition() {
    for reason in [
        McpStdioCwdRefusal::NotAbsolute,
        McpStdioCwdRefusal::AmbiguousCommand,
        McpStdioCwdRefusal::Missing,
        McpStdioCwdRefusal::NotADirectory,
    ] {
        let error = McpError::WorkingDirectory(McpStdioCwdError {
            server: "cwd".into(),
            reason,
        });
        assert_eq!(
            crate::attempt_guard::open_for_error(&error, None),
            crate::McpAttemptOpenOutcome::Refused {
                refusal: crate::McpAttemptRefusal::InvalidConfig
            }
        );
    }
}

/// Rows that launch the real stdio fixture.
#[cfg(feature = "workspace-tests")]
mod fixture {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The relative file the fixture reads: the configuration file the
    /// upstream GitHub MCP server reads from its working directory.
    const CONFIG_FILE: &str = "github-mcp-server-config.json";
    const AMBIENT_CONTENT: &str = "{\"ambient\":\"conflicting tool descriptions\"}";
    const SELECTED_CONTENT: &str = "{\"selected\":\"installation\"}";
    /// Set in the child process this module's harness starts.
    const CHILD_ENV: &str = "MEERKAT_MCP_CWD_CHILD";
    const SELECTED_ENV: &str = "MEERKAT_MCP_CWD_SELECTED";
    const SELECTED_OWN_ENV: &str = "MEERKAT_MCP_CWD_SELECTED_OWN";
    const DONE: &str = "mcp-cwd-child-done";

    /// What the fixture's `cwd_report` tool saw.
    #[derive(Debug)]
    struct Report {
        cwd: PathBuf,
        pid: u32,
        file: Option<String>,
    }

    fn parse_report(blocks: &[meerkat_core::types::ContentBlock]) -> Report {
        let report: serde_json::Value =
            serde_json::from_str(&meerkat_core::types::text_content(blocks)).unwrap();
        Report {
            cwd: PathBuf::from(report["cwd"].as_str().expect("cwd")),
            pid: u32::try_from(report["pid"].as_u64().expect("pid")).unwrap(),
            file: report["file"].as_str().map(str::to_owned),
        }
    }

    /// The fixture reporting its working directory and the relative
    /// configuration file it reads there, launched in `cwd` when given.
    fn reporting_server(name: &str, cwd: Option<&Path>) -> McpServerConfig {
        let server = McpServerConfig::stdio(
            name,
            mcp_test_server::fixture_binary()
                .to_string_lossy()
                .to_string(),
            vec!["--cwd-report".into(), CONFIG_FILE.into()],
            HashMap::new(),
        );
        match cwd {
            Some(cwd) => server
                .with_cwd(cwd.display().to_string())
                .expect("an absolute directory and an absolute command"),
            None => server,
        }
    }

    async fn router_report(router: &McpRouter) -> Result<Report, McpError> {
        router
            .call_tool("cwd_report", &serde_json::json!({}))
            .await
            .map(|blocks| parse_report(&blocks))
    }

    async fn launch_and_report(cwd: Option<&Path>) -> Report {
        let config = reporting_server("github", cwd);
        let (connection, tools) = tokio::time::timeout(
            BOUND,
            McpConnection::connect_and_enumerate_with_stdio_profile(
                &config,
                &McpStdioLaunchProfile::trusted_host(),
            ),
        )
        .await
        .expect("the launch is bounded")
        .expect("the launch succeeds");
        assert!(tools.iter().any(|tool| tool.name == "cwd_report"));
        let result = connection
            .call_tool("cwd_report", &serde_json::json!({}))
            .await
            .expect("the report");
        connection.close().await.expect("close");
        parse_report(&result)
    }

    /// The child process's half of the row below: its own working directory
    /// is the ambient one the parent chose.
    fn selected_versus_ambient_in_this_process() {
        let ambient = std::env::current_dir().unwrap();
        let selected = PathBuf::from(std::env::var_os(SELECTED_ENV).unwrap());
        let selected_own = PathBuf::from(std::env::var_os(SELECTED_OWN_ENV).unwrap());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                // The selected installation has no configuration file: the
                // server runs there and reads none, not the ambient one.
                let report = launch_and_report(Some(&selected)).await;
                assert_eq!(report.cwd, selected);
                assert_eq!(report.file, None, "{report:?}");
                // A selected directory with its own file: only that one.
                let report = launch_and_report(Some(&selected_own)).await;
                assert_eq!(report.cwd, selected_own);
                assert_eq!(report.file.as_deref(), Some(SELECTED_CONTENT));
                // Control: without a working directory the server runs in
                // the host's directory and reads its conflicting file.
                let report = launch_and_report(None).await;
                assert_eq!(report.cwd, ambient);
                assert_eq!(report.file.as_deref(), Some(AMBIENT_CONTENT));
            });
        println!("{DONE}");
    }

    /// Connect's acceptance row: the host process runs in an ambient
    /// directory holding a conflicting relative configuration file, and a
    /// server launched with a selected working directory reads only the
    /// selected one. The host's directory is process-global, so the row
    /// runs in a child process (this test binary again, exactly this test)
    /// whose working directory is the ambient one, and requires its done
    /// marker, so a wrong test path fails instead of passing. (Fails-old:
    /// the server ran in the ambient directory and read its file.)
    #[test]
    fn a_launch_with_cwd_reads_only_the_selected_directory_not_the_ambient_one() {
        if std::env::var_os(CHILD_ENV).is_some() {
            selected_versus_ambient_in_this_process();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let ambient = root.join("ambient");
        let selected = root.join("selected");
        let selected_own = root.join("selected-own");
        for dir in [&ambient, &selected, &selected_own] {
            std::fs::create_dir(dir).unwrap();
        }
        std::fs::write(ambient.join(CONFIG_FILE), AMBIENT_CONTENT).unwrap();
        std::fs::write(selected_own.join(CONFIG_FILE), SELECTED_CONTENT).unwrap();
        let fixture = mcp_test_server::fixture_binary().canonicalize().unwrap();
        let test_path = format!(
            "{}::a_launch_with_cwd_reads_only_the_selected_directory_not_the_ambient_one",
            module_path!()
                .split_once("::")
                .map_or(module_path!(), |(_, path)| path)
        );
        // Re-runs this test binary, not a server launch.
        #[allow(clippy::disallowed_methods)]
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &test_path, "--nocapture", "--test-threads", "1"])
            .current_dir(&ambient)
            .env(CHILD_ENV, "1")
            .env(SELECTED_ENV, &selected)
            .env(SELECTED_OWN_ENV, &selected_own)
            .env(mcp_test_server::FIXTURE_ENV, &fixture)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = child.spawn().expect("start the child test");
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = String::new();
            let mut err = String::new();
            let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
            let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
            let _ = sender.send((out, err));
        });
        let Ok((out, err)) = receiver.recv_timeout(Duration::from_secs(120)) else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child test did not finish within its bound");
        };
        let status = child.wait().unwrap();
        assert!(
            status.success() && out.contains(DONE),
            "child test {test_path} failed ({status:?})\nstdout:\n{out}\nstderr:\n{err}"
        );
    }

    /// A definition that differs only in its working directory is a new
    /// launch: reloading with it replaces the running process, which runs
    /// in the new directory. (Fails-old: the reload launched the same
    /// command in the same directory.)
    #[cfg(unix)]
    #[tokio::test]
    async fn reloading_a_definition_that_differs_only_in_cwd_relaunches_in_the_new_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (first_dir, second_dir) = (root.join("first"), root.join("second"));
        std::fs::create_dir(&first_dir).unwrap();
        std::fs::create_dir(&second_dir).unwrap();
        let handle = Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral());
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            handle,
            McpStdioLaunchProfile::trusted_host(),
        );
        router
            .add_server(reporting_server("relocated", Some(&first_dir)))
            .await
            .expect("the first launch");
        let first = router_report(&router).await.expect("the first report");
        assert_eq!(first.cwd, first_dir);

        router
            .stage_reload(reporting_server("relocated", Some(&second_dir)))
            .expect("stage the reload");
        router.apply_staged().await.expect("apply the reload");
        let second = tokio::time::timeout(BOUND, async {
            loop {
                let _ = router.take_external_updates();
                if let Ok(report) = router_report(&router).await
                    && report.cwd == second_dir
                {
                    return report;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the reload relaunches in the new directory");
        assert_ne!(second.pid, first.pid, "a new process runs the definition");
        let old = nix::unistd::Pid::from_raw(i32::try_from(first.pid).unwrap());
        tokio::time::timeout(BOUND, async {
            while nix::sys::signal::kill(old, None).is_ok() {
                let _ = router.take_external_updates();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the replaced process is gone");
        router.shutdown().await;
    }
}
