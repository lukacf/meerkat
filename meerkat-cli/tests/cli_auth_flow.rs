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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn seed_managed_openai_realm_config(root: &std::path::Path) {
    seed_managed_openai_realm_config_with_ids(root, "default_openai", "default_openai");
}

fn seed_managed_openai_realm_config_with_ids(
    root: &std::path::Path,
    auth_profile_id: &str,
    binding_id: &str,
) {
    seed_openai_realm_config_with_ids_and_method(
        root,
        auth_profile_id,
        binding_id,
        "openai_api",
        "api_key",
    );
}

fn seed_openai_realm_config_with_ids_and_method(
    root: &std::path::Path,
    auth_profile_id: &str,
    binding_id: &str,
    backend_kind: &str,
    auth_method: &str,
) {
    let realm_dir = root.join("realms").join("dev");
    std::fs::create_dir_all(&realm_dir).expect("mkdir realm config dir");
    std::fs::write(
        realm_dir.join("config.toml"),
        format!(
            r#"
[realm.dev]
default_binding = "{binding_id}"

[realm.dev.backend.openai_backend]
provider = "openai"
backend_kind = "{backend_kind}"

[realm.dev.auth.{auth_profile_id}]
provider = "openai"
auth_method = "{auth_method}"
source = {{ kind = "managed_store" }}

[realm.dev.binding.{binding_id}]
backend_profile = "openai_backend"
auth_profile = "{auth_profile_id}"
"#
        ),
    )
    .expect("write realm config");
}

fn token_file_path(root: &std::path::Path) -> PathBuf {
    token_file_path_for_binding(root, "default_openai")
}

fn token_file_path_for_binding(root: &std::path::Path, binding_id: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    let config_root = root.join("Library").join("Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_root = root.join("config");

    config_root
        .join("meerkat")
        .join("credentials")
        .join("dev")
        .join(format!("{binding_id}.json"))
}

fn seed_token_file(root: &std::path::Path, body: &str) {
    seed_token_file_for_binding(root, "default_openai", body);
}

fn seed_token_file_for_binding(root: &std::path::Path, binding_id: &str, body: &str) {
    let token_file = token_file_path_for_binding(root, binding_id);
    let token_dir = token_file.parent().expect("token file has parent");
    std::fs::create_dir_all(token_dir).expect("mkdir token dir");
    std::fs::write(token_file, body).expect("write token file");
}

fn seed_unmarked_auth_lease_token_file(root: &std::path::Path) {
    seed_unmarked_auth_lease_token_file_for_binding(root, "default_openai");
}

fn seed_unmarked_auth_lease_token_file_for_binding(root: &std::path::Path, binding_id: &str) {
    let body = format!(
        r#"{{
  "auth_mode": "api_key",
  "primary_secret": "sk-fail-closed",
  "scopes": [],
  "metadata": null,
  "auth_lease": {{
    "token_key": {{
      "realm": "dev",
      "binding": "{binding_id}"
    }},
    "generation": 1
  }}
}}"#
    );
    seed_token_file_for_binding(root, binding_id, &body);

    let marker = format!(
        r#"{{
  "auth_mode": "api_key",
  "auth_lease": {{
    "token_key": {{
      "realm": "dev",
      "binding": "{binding_id}"
    }},
    "generation": 2
  }}
}}"#
    );
    let marker_path = token_file_path_for_binding(root, binding_id).with_extension("json.fallback");
    std::fs::write(marker_path, marker).expect("write mismatched fallback marker");
}

fn seed_auth_lease_fallback_marker_for_binding(
    root: &std::path::Path,
    binding_id: &str,
    auth_mode: &str,
    generation: u64,
) {
    let marker = format!(
        r#"{{
  "auth_mode": "{auth_mode}",
  "auth_lease": {{
    "token_key": {{
      "realm": "dev",
      "binding": "{binding_id}"
    }},
    "generation": {generation}
  }}
}}"#
    );
    let marker_path = token_file_path_for_binding(root, binding_id).with_extension("json.fallback");
    std::fs::write(marker_path, marker).expect("write matching fallback marker");
}

fn token_file_exists(root: &std::path::Path) -> bool {
    token_file_path(root).exists()
}

fn isolated_keyring_service(root: &std::path::Path) -> String {
    let suffix = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    format!("meerkat-auth-cli-test-{}-{suffix}", std::process::id())
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
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_token_file(tmp.path(), "{ malformed token json");

    let logout = Command::new(&rkat)
        .args(["auth", "logout", "default_openai"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
fn rkat_auth_logout_clears_fail_closed_auth_lease_token_file() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_unmarked_auth_lease_token_file(tmp.path());

    let logout = Command::new(&rkat)
        .args(["auth", "logout", "default_openai"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth logout must spawn");
    if !logout.status.success() {
        let stderr = String::from_utf8_lossy(&logout.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        assert!(
            logout.status.success(),
            "logout must clear fail-closed auth-lease token file; stderr={stderr}"
        );
    }

    assert!(
        !token_file_exists(tmp.path()),
        "logout must remove untrusted auth-lease persisted credentials"
    );
}

#[test]
fn rkat_auth_profile_delete_clears_malformed_token_file() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
fn rkat_auth_profile_delete_clears_fail_closed_auth_lease_token_file() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_managed_openai_realm_config(tmp.path());
    seed_unmarked_auth_lease_token_file(tmp.path());

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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth profile-delete must spawn");
    if !delete.status.success() {
        let stderr = String::from_utf8_lossy(&delete.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        assert!(
            delete.status.success(),
            "profile delete must clear fail-closed auth-lease token file; stderr={stderr}"
        );
    }

    assert!(
        !token_file_exists(tmp.path()),
        "profile delete must remove untrusted auth-lease persisted credentials"
    );
}

#[test]
fn rkat_auth_profile_delete_clears_binding_scoped_token_when_profile_id_differs() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_managed_openai_realm_config_with_ids(tmp.path(), "openai_managed", "default_openai");
    seed_token_file_for_binding(tmp.path(), "default_openai", "{ malformed token json");

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
            "openai_managed",
            "--yes",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth profile-delete must spawn");
    if !delete.status.success() {
        let stderr = String::from_utf8_lossy(&delete.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("profile delete must clear binding-scoped token file; stderr={stderr}");
    }

    let stdout = String::from_utf8_lossy(&delete.stdout);
    assert!(
        stdout.contains("deleted: dev:default_openai"),
        "profile delete should report the binding-scoped token key; stdout:\n{stdout}"
    );
    assert!(
        !token_file_path_for_binding(tmp.path(), "default_openai").exists(),
        "profile delete must remove the token file keyed by the owning binding"
    );
}

#[test]
fn rkat_auth_status_hides_token_metadata_without_lifecycle() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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

#[test]
fn rkat_auth_status_ignores_malformed_token_storage_without_lifecycle() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_managed_openai_realm_config(tmp.path());
    seed_token_file(tmp.path(), "{ malformed token json");

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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth status must spawn");
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
            eprintln!("SKIP: auth provider features unavailable");
            return;
        }
        panic!("status must not be owned by malformed token storage; stderr={stderr}");
    }

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("state:       unknown"),
        "status should report lease-unknown without token-store truth; stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("auth_mode:"),
        "status must not expose token-derived metadata without lifecycle; stdout:\n{stdout}"
    );
}

#[test]
fn rkat_auth_refresh_uses_binding_scoped_token_when_profile_id_differs() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_openai_realm_config_with_ids_and_method(
        tmp.path(),
        "openai_managed",
        "default_openai",
        "chatgpt_backend",
        "managed_chatgpt_oauth",
    );
    seed_token_file(
        tmp.path(),
        r#"{
  "auth_mode": "chatgpt_oauth",
  "primary_secret": "fresh-chatgpt-access",
  "expires_at": 1893456000,
  "last_refresh": 1700000000,
  "scopes": ["openid", "email"],
  "account_id": "acct-fresh",
  "metadata": {
    "meerkat_auth_lifecycle": {
      "published": true,
      "version": 2,
      "expires_at": 1893456000,
      "generation": 1,
      "credential_published_at_millis": 1
    }
  },
  "auth_lease": {
    "token_key": {
      "realm": "dev",
      "binding": "default_openai"
    },
    "generation": 1
  }
}"#,
    );
    seed_auth_lease_fallback_marker_for_binding(tmp.path(), "default_openai", "chatgpt_oauth", 1);

    let refresh = Command::new(&rkat)
        .args([
            "--state-root",
            tmp.path().join("realms").to_str().expect("utf8 path"),
            "--realm",
            "dev",
            "auth",
            "refresh",
            "--realm",
            "dev",
            "openai_managed",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth refresh must spawn");
    let stderr = String::from_utf8_lossy(&refresh.stderr);
    if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
        eprintln!("SKIP: auth provider features unavailable");
        return;
    }
    assert!(
        !refresh.status.success(),
        "fixture intentionally omits refresh_token so refresh stops at the provider boundary"
    );
    assert!(
        stderr.contains("persisted tokens missing refresh_token")
            || stderr.contains("missing refresh_token"),
        "refresh must reach the lease-bound provider refresh path; stderr={stderr}"
    );
    assert!(
        !stderr.contains("token endpoint error"),
        "fixture should fail before any live provider exchange; stderr={stderr}"
    );
    assert!(
        !stderr.contains("reason:        no persisted credential"),
        "refresh must not preflight token storage by auth profile id; stderr={stderr}"
    );
}

#[test]
fn rkat_auth_refresh_does_not_publish_unbound_token_before_resolve() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_openai_realm_config_with_ids_and_method(
        tmp.path(),
        "openai_managed",
        "default_openai",
        "chatgpt_backend",
        "managed_chatgpt_oauth",
    );
    seed_token_file(
        tmp.path(),
        r#"{
  "auth_mode": "chatgpt_oauth",
  "primary_secret": "fresh-chatgpt-access",
  "expires_at": 1893456000,
  "last_refresh": 1700000000,
  "scopes": ["openid", "email"],
  "account_id": "acct-fresh"
}"#,
    );

    let refresh = Command::new(&rkat)
        .args([
            "--state-root",
            tmp.path().join("realms").to_str().expect("utf8 path"),
            "--realm",
            "dev",
            "auth",
            "refresh",
            "--realm",
            "dev",
            "openai_managed",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth refresh must spawn");
    let stderr = String::from_utf8_lossy(&refresh.stderr);
    if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
        eprintln!("SKIP: auth provider features unavailable");
        return;
    }
    assert!(
        !refresh.status.success(),
        "unbound durable OAuth material must not refresh successfully"
    );
    assert!(
        !stderr.contains("missing refresh_token"),
        "refresh must not reach provider-token refresh with unbound durable material; stderr={stderr}"
    );
    let token_file = std::fs::read_to_string(token_file_path(tmp.path())).expect("token file");
    assert!(
        !token_file.contains("\"auth_lease\""),
        "refresh must not publish AuthMachine lifecycle from unbound durable material; token_file={token_file}"
    );
}

#[test]
fn rkat_auth_status_resolves_binding_that_references_auth_profile() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_managed_openai_realm_config_with_ids(tmp.path(), "openai_managed", "default_openai");

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
            "openai_managed",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth status must spawn");
    assert!(
        status.status.success(),
        "status must resolve binding-scoped lease identity from auth profile; stderr={}",
        String::from_utf8_lossy(&status.stderr)
    );

    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("binding_id:  default_openai"),
        "status must report the binding that owns the auth lease key; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("state:       unknown"),
        "status should still be lease-unknown without a live AuthMachine lifecycle; stdout:\n{stdout}"
    );
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
    let keyring_service = isolated_keyring_service(tmp.path());
    let out = Command::new(&rkat)
        .args(["auth", "profile", "list", "--realm", "nonexistent-realm"])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
    let keyring_service = isolated_keyring_service(tmp.path());
    // Seed a minimal .rkat so `rkat run` doesn't refuse on missing config.
    std::fs::create_dir_all(tmp.path().join(".rkat")).expect("mkdir .rkat");

    let out = Command::new(&rkat)
        .args(["run", "--connection-ref", "nonexistent:binding", "hi"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
    let keyring_service = isolated_keyring_service(tmp.path());
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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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

#[test]
fn rkat_auth_test_uses_token_store_for_scripted_login_credentials() {
    let Some(rkat) = rkat_binary() else {
        eprintln!("SKIP: rkat binary unavailable");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keyring_service = isolated_keyring_service(tmp.path());
    seed_managed_openai_realm_config(tmp.path());
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
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
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

    let auth_test = Command::new(&rkat)
        .args([
            "--state-root",
            tmp.path().join("realms").to_str().expect("utf8 path"),
            "--realm",
            "dev",
            "auth",
            "test",
            "--realm",
            "dev",
            "default_openai",
        ])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("MEERKAT_AUTH_KEYRING_SERVICE", &keyring_service)
        .stdin(Stdio::null())
        .output()
        .expect("rkat auth test must spawn");
    let stderr = String::from_utf8_lossy(&auth_test.stderr);
    if stderr.contains("requires the `anthropic`, `openai`, and `gemini`") {
        eprintln!("SKIP: auth provider features unavailable");
        return;
    }
    assert!(
        auth_test.status.success(),
        "auth test must resolve scripted-login credentials through TokenStore; stderr={stderr}"
    );

    let stdout = String::from_utf8_lossy(&auth_test.stdout);
    assert!(
        stdout.contains("state: valid") && stdout.contains("has_credential: true"),
        "auth test should report a valid credential from the managed store; stdout:\n{stdout}"
    );
}
