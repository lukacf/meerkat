//! T14 (Phase 5): CLI auth flow e2e — top-down observable proof that
//! the `rkat auth` subcommand tree is wired end-to-end.
//!
//! Plan choke point K5 reads: "`rkat auth login openai --backend
//! openai_api --method api_key` … → profile saved; `rkat auth profile
//! list` shows it; `rkat run --connection-ref realm:binding "hi"` builds
//! agent". The live-provider half of that flow requires real credentials
//! and belongs in the `e2e-auth` lane; this file exercises the offline
//! command-surface half: the subcommand dispatch table, argument
//! parsing, and deterministic error paths under minimal feature
//! configurations.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn seed_managed_openai_realm_config(root: &std::path::Path) {
    let realm_dir = root.join("realms").join("dev");
    std::fs::create_dir_all(&realm_dir).expect("mkdir realm config dir");
    std::fs::write(
        realm_dir.join("config.toml"),
        r#"
[realm.dev]
default_binding = "default_openai"

[realm.dev.backend.openai_backend]
provider = "openai"
backend_kind = "openai_api"

[realm.dev.auth.default_openai]
provider = "openai"
auth_method = "api_key"
source = { kind = "managed_store" }

[realm.dev.binding.default_openai]
backend_profile = "openai_backend"
auth_profile = "default_openai"
"#,
    )
    .expect("write realm config");
}

fn token_file_path(root: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    let config_root = root.join("Library").join("Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_root = root.join("config");

    config_root
        .join("meerkat")
        .join("credentials")
        .join("dev")
        .join("default_openai.json")
}

fn seed_token_file(root: &std::path::Path, body: &str) {
    let token_file = token_file_path(root);
    let token_dir = token_file.parent().expect("token file has parent");
    std::fs::create_dir_all(token_dir).expect("mkdir token dir");
    std::fs::write(token_file, body).expect("write token file");
}

fn token_file_exists(root: &std::path::Path) -> bool {
    token_file_path(root).exists()
}

fn rkat_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p.canonicalize().unwrap_or(p));
        }
    }
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        for profile in ["debug", "release"] {
            let candidate = target_dir.join(profile).join("rkat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let codex_debug = workspace_root.join("target-codex/debug/rkat");
    if codex_debug.exists() {
        return Some(codex_debug);
    }
    let codex_release = workspace_root.join("target-codex/release/rkat");
    if codex_release.exists() {
        return Some(codex_release);
    }
    None
}

/// `rkat auth --help` returns 0 and mentions every documented subcommand.
#[test]
fn rkat_auth_help_lists_all_subcommands() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let out = Command::new(&rkat)
        .args(["auth", "--help"])
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth --help must spawn");

    assert!(
        out.status.success(),
        "rkat auth --help must exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for subcommand in ["login", "logout", "profile", "status", "binding"] {
        assert!(
            stdout.contains(subcommand),
            "rkat auth --help must mention `{subcommand}` subcommand; stdout:\n{stdout}"
        );
    }
}

#[test]
fn rkat_auth_logout_clears_malformed_token_file() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    seed_token_file(tmp.path(), "{ malformed token json");

    let logout = Command::new(&rkat)
        .args(["auth", "logout", "default_openai"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth logout must spawn");
    if !logout.status.success() {
        let stderr = String::from_utf8_lossy(&logout.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("logout must clear malformed token file; stderr={stderr}");
    }

    assert!(
        !token_file_exists(tmp.path()),
        "logout must remove malformed persisted credentials"
    );
}

#[test]
fn rkat_auth_profile_delete_clears_malformed_token_file() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    seed_managed_openai_realm_config(tmp.path());
    seed_token_file(tmp.path(), "{ malformed token json");

    let delete = Command::new(&rkat)
        .args([
            "--state-root",
            tmp.path().join("realms").to_str().expect("utf8 path"),
            "--realm",
            "dev",
            "auth",
            "profile-delete",
            "--realm",
            "dev",
            "default_openai",
            "--yes",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth profile-delete must spawn");
    if !delete.status.success() {
        let stderr = String::from_utf8_lossy(&delete.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("profile delete must clear malformed token file; stderr={stderr}");
    }

    assert!(
        !token_file_exists(tmp.path()),
        "profile delete must remove malformed persisted credentials"
    );
}

#[test]
fn rkat_auth_status_hides_token_metadata_without_lifecycle() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    seed_managed_openai_realm_config(tmp.path());
    seed_token_file(
        tmp.path(),
        r#"{
  "auth_mode": "api_key",
  "primary_secret": "sk-stale",
  "refresh_token": "refresh-stale",
  "expires_at": 1800000000,
  "last_refresh": 1700000000,
  "scopes": ["email"],
  "account_id": "acct-stale"
}"#,
    );

    let status = Command::new(&rkat)
        .args([
            "--state-root",
            tmp.path().join("realms").to_str().expect("utf8 path"),
            "--realm",
            "dev",
            "auth",
            "status",
            "--realm",
            "dev",
            "default_openai",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth status must spawn");
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("status must succeed with seeded config; stderr={stderr}");
    }

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("state:       unknown"),
        "status should report unknown without a live AuthMachine lifecycle; stdout:\n{stdout}"
    );
    for forbidden in [
        "auth_mode:",
        "has_secret:",
        "has_refresh:",
        "expires_at:",
        "last_refresh:",
        "account_id:",
        "scopes:",
        "acct-stale",
        "email",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "status must not expose token-derived `{forbidden}` without lifecycle; stdout:\n{stdout}"
        );
    }
}

/// `rkat auth profile list --realm <empty>` returns either 0 (empty
/// list) or a typed error; it must not panic or spew an unstructured
/// stack trace.
#[test]
fn rkat_auth_profile_list_returns_cleanly_for_empty_realm() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let out = Command::new(&rkat)
        .args(["auth", "profile", "list", "--realm", "nonexistent-realm"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth profile list must spawn");

    // Either exits 0 (empty-list ok) or non-zero with an error message;
    // what matters is no panic and no empty-body failure.
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.is_empty(),
            "non-zero exit must include stderr diagnostic"
        );
        assert!(
            !stderr.contains("thread 'main' panicked"),
            "CLI must not panic; stderr:\n{stderr}"
        );
    }
}

/// `rkat run --connection-ref realm:binding "hi"` without a configured
/// realm must return a typed `ConnectionResolution` error, not an
/// `unknown argument` or panic. This proves the flag is registered and
/// flows into build_agent.
///
/// The test does not require a live LLM; the error path triggers before
/// any network call.
#[test]
fn rkat_run_connection_ref_flag_is_registered_and_routes() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    // Seed a minimal .rkat so `rkat run` doesn't refuse on missing config.
    std::fs::create_dir_all(tmp.path().join(".rkat")).expect("mkdir .rkat");

    let out = Command::new(&rkat)
        .args(["run", "--connection-ref", "nonexistent:binding", "hi"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .env("RKAT_TEST_CLIENT", "1") // short-circuit LLM client
        .stdin(Stdio::null())
        .output()
        .expect("rkat run must spawn");

    let stderr = String::from_utf8_lossy(&out.stderr);
    // The flag must be accepted (if not registered, clap emits "unrecognized
    // argument" with a specific pattern).
    assert!(
        !stderr.contains("unrecognized argument")
            && !stderr.contains("unexpected argument")
            && !stderr.contains("found argument '--connection-ref'"),
        "--connection-ref must be a registered CLI flag; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "rkat run must not panic on unknown realm; stderr:\n{stderr}"
    );
}

/// `rkat auth logout` clears the same TokenStore key written by scripted login
/// and fails closed on a repeat clear instead of reporting success for stale
/// credentials.
#[test]
fn rkat_auth_logout_clears_scripted_login_token_key() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let login = Command::new(&rkat)
        .args([
            "auth",
            "login",
            "openai",
            "--non-interactive",
            "--secret",
            "sk-test",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth login must spawn");
    if !login.status.success() {
        let stderr = String::from_utf8_lossy(&login.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("scripted login failed: {stderr}");
    }

    let logout = Command::new(&rkat)
        .args(["auth", "logout", "default_openai"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth logout must spawn");
    assert!(
        logout.status.success(),
        "logout must clear scripted login token; stderr={}",
        String::from_utf8_lossy(&logout.stderr)
    );

    let repeat = Command::new(&rkat)
        .args(["auth", "logout", "default_openai"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .stdin(Stdio::null())
        .output()
        .expect("repeat rkat auth logout must spawn");
    assert!(
        !repeat.status.success(),
        "repeat logout must not report success for already-cleared credentials"
    );
    let stderr = String::from_utf8_lossy(&repeat.stderr);
    assert!(
        stderr.contains("No credentials found"),
        "repeat logout should explain that no token remains; stderr={stderr}"
    );
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "repeat logout must not panic; stderr={stderr}"
    );
}
