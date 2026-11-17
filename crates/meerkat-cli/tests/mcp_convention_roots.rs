//! External contract for offline MCP commands and CLI convention roots.

#![cfg(feature = "mcp")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn rkat_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "rkat.exe" } else { "rkat" });
    path
}

fn run_rkat(
    binary: &std::path::Path,
    context_root: &std::path::Path,
    user_root: &std::path::Path,
    ambient_cwd: &std::path::Path,
    ambient_home: &std::path::Path,
    args: &[&str],
) -> Output {
    Command::new(binary)
        .current_dir(ambient_cwd)
        .env("HOME", ambient_home)
        .arg("--context-root")
        .arg(context_root)
        .arg("--user-config-root")
        .arg(user_root)
        .args(args)
        .output()
        .expect("run rkat")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "rkat failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn offline_mcp_commands_honor_explicit_convention_roots() {
    let temp = tempfile::TempDir::new().unwrap();
    let context_root = temp.path().join("explicit-context");
    let user_root = temp.path().join("explicit-user");
    let ambient_cwd = temp.path().join("ambient-cwd");
    let ambient_home = temp.path().join("ambient-home");
    for dir in [&context_root, &user_root, &ambient_cwd, &ambient_home] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let binary = rkat_binary();

    let add_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "add",
            "explicit-user",
            "--scope",
            "user",
            "--",
            "echo",
            "user",
        ],
    );
    assert_success(&add_user);
    let user_config = user_root.join(".rkat/mcp.toml");
    assert!(user_config.exists());
    assert!(!ambient_home.join(".rkat/mcp.toml").exists());

    let add_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "add",
            "explicit-project",
            "--scope",
            "project",
            "--",
            "echo",
            "project",
        ],
    );
    assert_success(&add_project);
    let project_config = context_root.join(".rkat/mcp.toml");
    assert!(project_config.exists());
    assert!(!ambient_cwd.join(".rkat/mcp.toml").exists());

    let list_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "list", "--scope", "user", "--json"],
    );
    assert_success(&list_user);
    let listed = String::from_utf8(list_user.stdout).unwrap();
    assert!(listed.contains("explicit-user"));
    assert!(!listed.contains("explicit-project"));

    let get_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &[
            "mcp",
            "get",
            "explicit-project",
            "--scope",
            "project",
            "--json",
        ],
    );
    assert_success(&get_project);
    assert!(
        String::from_utf8(get_project.stdout)
            .unwrap()
            .contains("explicit-project")
    );

    let remove_user = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "remove", "explicit-user", "--scope", "user"],
    );
    assert_success(&remove_user);
    let remove_project = run_rkat(
        &binary,
        &context_root,
        &user_root,
        &ambient_cwd,
        &ambient_home,
        &["mcp", "remove", "explicit-project", "--scope", "project"],
    );
    assert_success(&remove_project);

    assert!(
        !std::fs::read_to_string(user_config)
            .unwrap()
            .contains("explicit-user")
    );
    assert!(
        !std::fs::read_to_string(project_config)
            .unwrap()
            .contains("explicit-project")
    );
}

/// One temporary home, state root, context root and user config root, with
/// the user `mcp.toml` a row writes.
struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    state: PathBuf,
    context: PathBuf,
    user: PathBuf,
}

impl Roots {
    fn new(user_mcp_toml: &str) -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let roots = Self {
            home: temp.path().join("home"),
            state: temp.path().join("state"),
            context: temp.path().join("context"),
            user: temp.path().join("user"),
            _temp: temp,
        };
        for dir in [&roots.home, &roots.state, &roots.context, &roots.user] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(roots.user.join(".rkat")).unwrap();
        std::fs::write(roots.user_mcp_toml(), user_mcp_toml).unwrap();
        roots
    }

    fn user_mcp_toml(&self) -> PathBuf {
        self.user.join(".rkat/mcp.toml")
    }

    /// `rkat <root flags> <args>` with no provider key, `set` in its
    /// environment and `unset` absent from it.
    fn rkat(&self, set: &[(&str, &str)], unset: &[&str], args: &[&str]) -> Output {
        let mut command = Command::new(rkat_binary());
        for name in [
            "RKAT_ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY",
            "RKAT_GEMINI_API_KEY",
            "GEMINI_API_KEY",
        ] {
            command.env_remove(name);
        }
        for name in unset {
            command.env_remove(name);
        }
        command
            .current_dir(&self.context)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .envs(set.iter().copied())
            .arg("--state-root")
            .arg(&self.state)
            .arg("--context-root")
            .arg(&self.context)
            .arg("--user-config-root")
            .arg(&self.user)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run rkat")
    }
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A variable that is not set is the failure of the one server that names
/// it. `alpha` names one that is not set and `beta` one that is: `list` and
/// `get` show both, `alpha` as written with the variable it waits for, and a
/// command addressed to `beta` reaches `beta`.
#[test]
fn an_unset_variable_fails_only_the_mcp_server_that_names_it() {
    const ABSENT: &str = "RKAT_ROW_MCP_LIST_ABSENT_TOKEN";
    const PRESENT: &str = "RKAT_ROW_MCP_LIST_PRESENT_HOME";
    let roots = Roots::new(&format!(
        "[[servers]]\nname = \"alpha\"\nurl = \"https://alpha.example/mcp\"\n\
         headers = {{ Authorization = \"Bearer ${{{ABSENT}}}\" }}\n\n\
         [[servers]]\nname = \"beta\"\ncommand = \"${{{PRESENT}}}/bin/beta\"\n"
    ));
    let rkat = |args: &[&str]| roots.rkat(&[(PRESENT, "/opt/row-present")], &[ABSENT], args);

    let list = rkat(&["mcp", "list"]);
    assert_success(&list);
    let listed = stdout_of(&list);
    let row = |name: &str| {
        listed
            .lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_else(|| panic!("`{name}` is listed: {listed}"))
            .to_owned()
    };
    assert!(
        row("alpha").contains("unresolved") && row("alpha").contains(ABSENT),
        "the unresolved server names its variable: {listed}"
    );
    assert!(
        row("beta").contains("/opt/row-present/bin/beta"),
        "a set variable still expands: {listed}"
    );

    let list_json = rkat(&["mcp", "list", "--json"]);
    assert_success(&list_json);
    let entries: serde_json::Value = serde_json::from_slice(&list_json.stdout).unwrap();
    let entry = |name: &str| {
        entries
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("`{name}` is listed: {entries}"))
            .clone()
    };
    assert_eq!(
        entry("alpha")["unresolved"],
        serde_json::json!({ "field": "headers.Authorization", "variable": ABSENT }),
        "{entries}"
    );
    assert_eq!(
        entry("alpha")["headers"]["Authorization"],
        format!("Bearer ${{{ABSENT}}}"),
        "an unresolved entry is shown as written: {entries}"
    );
    assert_eq!(entry("beta")["command"], "/opt/row-present/bin/beta");
    assert!(entry("beta").get("unresolved").is_none(), "{entries}");

    let get_other = rkat(&["mcp", "get", "beta"]);
    assert_success(&get_other);
    assert!(
        stdout_of(&get_other).contains("/opt/row-present/bin/beta"),
        "{}",
        stdout_of(&get_other)
    );

    let get_unresolved = rkat(&["mcp", "get", "alpha"]);
    assert_success(&get_unresolved);
    let shown = stdout_of(&get_unresolved);
    assert!(
        shown.contains("Unresolved:")
            && shown.contains("headers.Authorization")
            && shown.contains(ABSENT),
        "{shown}"
    );

    // `beta` is a stdio server, so a login addressed to it is refused for
    // what `beta` is: the command reached it.
    let login_other = rkat(&["mcp", "login", "beta", "--allow-headless"]);
    assert!(!login_other.status.success());
    let refused = stderr_of(&login_other);
    assert!(
        refused.contains("streamable HTTP servers only") && !refused.contains(ABSENT),
        "a login addressed to the other server reaches it: {refused}"
    );

    let login_unresolved = rkat(&["mcp", "login", "alpha", "--allow-headless"]);
    assert!(!login_unresolved.status.success());
    let refused = stderr_of(&login_unresolved);
    assert!(
        refused.contains("'alpha'")
            && refused.contains("headers.Authorization")
            && refused.contains(ABSENT),
        "the refusal names the server, the field and the variable: {refused}"
    );
}

/// `add` and `remove` write every other entry back as it was written: a
/// `${VAR}` reference is neither expanded nor dropped, set or not.
#[test]
fn adding_and_removing_an_mcp_server_keeps_variable_references_as_written() {
    const ABSENT: &str = "RKAT_ROW_MCP_WRITE_ABSENT_TOKEN";
    const PRESENT: &str = "RKAT_ROW_MCP_WRITE_PRESENT_HOME";
    let written = format!(
        "# kept\n[[servers]]\nname = \"alpha\"\nurl = \"https://alpha.example/mcp\"\n\
         headers = {{ Authorization = \"Bearer ${{{ABSENT}}}\" }}\n\n\
         [[servers]]\nname = \"beta\"\ncommand = \"${{{PRESENT}}}/bin/beta\"\n"
    );
    let roots = Roots::new(&written);
    let rkat = |args: &[&str]| roots.rkat(&[(PRESENT, "/opt/row-present")], &[ABSENT], args);
    let file = || std::fs::read_to_string(roots.user_mcp_toml()).unwrap();

    assert_success(&rkat(&[
        "mcp", "add", "gamma", "--scope", "user", "--", "echo", "gamma",
    ]));
    let added = file();
    assert!(
        added.starts_with(&written),
        "the earlier entries are byte-equal: {added}"
    );
    assert!(added.contains("name = \"gamma\""), "{added}");

    assert_success(&rkat(&["mcp", "remove", "gamma"]));
    let removed = file();
    assert!(!removed.contains("gamma"), "{removed}");
    for line in [
        format!("headers = {{ Authorization = \"Bearer ${{{ABSENT}}}\" }}\n"),
        format!("command = \"${{{PRESENT}}}/bin/beta\"\n"),
    ] {
        assert!(
            removed.contains(&line),
            "`{line}` is byte-equal after the remove: {removed}"
        );
    }
    assert!(!removed.contains("/opt/row-present"), "{removed}");
}

/// The text forms of `list` and `get` show no expanded credential: every env
/// and header value is masked, and a URL is shown without its userinfo and
/// its query. The `--json` forms are unchanged: scripts read them.
#[test]
fn mcp_text_listings_mask_env_header_and_url_credentials() {
    const VARIABLE: &str = "RKAT_ROW_MCP_MASK_VALUE";
    const VALUE: &str = "row-marker-0123456789";
    let roots = Roots::new(&format!(
        "[[servers]]\nname = \"remote\"\n\
         url = \"https://user:${{{VARIABLE}}}@remote.example/mcp?key=${{{VARIABLE}}}\"\n\
         headers = {{ X-Row-Header = \"${{{VARIABLE}}}\" }}\n\n\
         [[servers]]\nname = \"local\"\ncommand = \"tool\"\n\
         env = {{ ROW_SETTING = \"${{{VARIABLE}}}\" }}\n"
    ));
    let rkat = |args: &[&str]| roots.rkat(&[(VARIABLE, VALUE)], &[], args);

    for args in [
        &["mcp", "list"][..],
        &["mcp", "get", "remote"][..],
        &["mcp", "get", "local"][..],
    ] {
        let output = rkat(args);
        assert_success(&output);
        let text = format!("{}{}", stdout_of(&output), stderr_of(&output));
        assert!(
            !text.contains(VALUE),
            "`rkat {}` shows no expanded credential: {text}",
            args.join(" ")
        );
        assert!(text.contains("remote") || text.contains("local"), "{text}");
    }
    let shown = stdout_of(&rkat(&["mcp", "get", "remote"]));
    assert!(
        shown.contains("remote.example/mcp") && shown.contains("X-Row-Header"),
        "the host, the path and the header's name stay readable: {shown}"
    );

    for args in [
        &["mcp", "list", "--json"][..],
        &["mcp", "get", "remote", "--json"][..],
        &["mcp", "get", "local", "--json"][..],
    ] {
        let output = rkat(args);
        assert_success(&output);
        assert!(
            stdout_of(&output).contains(VALUE),
            "`rkat {}` is unchanged",
            args.join(" ")
        );
    }
}
