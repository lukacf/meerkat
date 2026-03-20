#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, timeout};

use serde_json::{Value, json};
use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        let debug = target_dir.join("debug/rkat");
        if debug.exists() {
            return Some(debug);
        }
        let release = target_dir.join("release/rkat");
        if release.exists() {
            return Some(release);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let debug = workspace_root.join("target/debug/rkat");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/rkat");
    if release.exists() {
        return Some(release);
    }
    None
}

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn skip_if_no_prereqs() -> bool {
    if rkat_binary_path().is_none() {
        eprintln!("Skipping: rkat binary not found (build with `cargo build -p meerkat-cli`)");
        return true;
    }
    false
}

fn skip_if_no_api_prereqs() -> bool {
    if skip_if_no_prereqs() {
        return true;
    }
    if anthropic_api_key().is_none() {
        eprintln!(
            "Skipping: no Anthropic API key (set ANTHROPIC_API_KEY or RKAT_ANTHROPIC_API_KEY)"
        );
        return true;
    }
    false
}

async fn write_mobpack_fixture(project_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mob_dir = project_dir.join("mobpack-fixture");
    tokio::fs::create_dir_all(mob_dir.join("skills")).await?;

    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        "[mobpack]\nname = \"smoke-mobpack\"\nversion = \"1.0.0\"\n",
    )
    .await?;

    let definition = format!(
        r#"{{
  "id":"smoke-mobpack",
  "orchestrator":{{"profile":"lead"}},
  "profiles":{{
    "lead":{{
      "model":"{}",
      "skills":[],
      "tools":{{"comms":true}},
      "peer_description":"Lead",
      "external_addressable":true
    }}
  }},
  "skills":{{}}
}}"#,
        smoke_model()
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition).await?;
    tokio::fs::write(mob_dir.join("skills").join("review.md"), "# Review\n").await?;

    Ok(mob_dir)
}

async fn run_rkat(
    rkat: &Path,
    cwd: &Path,
    args: &[&str],
    api_key: Option<&str>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(rkat);
    cmd.current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(key) = api_key {
        cmd.env("ANTHROPIC_API_KEY", key)
            .env("RKAT_ANTHROPIC_API_KEY", key);
    }
    cmd.args(args);
    let output = timeout(Duration::from_secs(180), cmd.output()).await??;
    Ok(output)
}

fn output_ok_or_err(output: std::process::Output, args: &[&str]) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!(
            "command failed (exit {:?}): rkat {}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn signer_from_pack(pack_bytes: &[u8]) -> Result<(String, String), Box<dyn std::error::Error>> {
    let files = meerkat_mob_pack::targz::extract_targz_safe(pack_bytes)?;
    let sig = files
        .get("signature.toml")
        .ok_or("missing signature.toml")?;
    let value: toml::Value = toml::from_str(std::str::from_utf8(sig)?)?;
    let signer_id = value
        .get("signer_id")
        .and_then(toml::Value::as_str)
        .ok_or("missing signer_id")?
        .to_string();
    let public_key = value
        .get("public_key")
        .and_then(toml::Value::as_str)
        .ok_or("missing public_key")?
        .to_string();
    Ok((signer_id, public_key))
}

async fn write_rpc_state_probe_mobpack_fixture(
    project_dir: &Path,
    mob_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mob_dir = project_dir.join(format!("{mob_id}-fixture"));
    tokio::fs::create_dir_all(&mob_dir).await?;
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{mob_id}\"\nversion = \"1.0.0\"\n"),
    )
    .await?;

    let definition = format!(
        r#"{{
  "id":"{mob_id}",
  "profiles":{{
    "lead":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "external_addressable":true,
      "peer_description":"Lead coordinator"
    }},
    "worker":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "peer_description":"Worker specialist"
    }},
    "reviewer":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "peer_description":"Review specialist"
    }}
  }},
  "wiring":{{
    "auto_wire_orchestrator":false,
    "role_wiring":[{{"a":"lead","b":"worker"}},{{"a":"worker","b":"reviewer"}}]
  }},
  "skills":{{}}
}}"#,
        model = smoke_model()
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition).await?;
    Ok(mob_dir)
}

async fn write_flow_probe_mobpack_fixture(
    project_dir: &Path,
    mob_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mob_dir = project_dir.join(format!("{mob_id}-fixture"));
    tokio::fs::create_dir_all(&mob_dir).await?;
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{mob_id}\"\nversion = \"1.0.0\"\n"),
    )
    .await?;

    let definition = format!(
        r#"{{
  "id":"{mob_id}",
  "profiles":{{
    "lead":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "external_addressable":true,
      "peer_description":"Lead synthesizer"
    }},
    "analyst":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "peer_description":"Analyst"
    }},
    "reviewer":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "peer_description":"Reviewer"
    }}
  }},
  "wiring":{{
    "auto_wire_orchestrator":false,
    "role_wiring":[{{"a":"lead","b":"analyst"}},{{"a":"lead","b":"reviewer"}},{{"a":"analyst","b":"reviewer"}}]
  }},
  "flows":{{
    "main":{{
      "description":"Three-step swarm synthesis smoke",
      "steps":{{
        "analyze":{{
          "role":"analyst",
          "message":"Analyze the assigned task and include the literal token ANALYZE_OK in your answer.",
          "timeout_ms":120000
        }},
        "review":{{
          "role":"reviewer",
          "message":"Review the prior analysis and include the literal token REVIEW_OK in your answer.",
          "depends_on":["analyze"],
          "timeout_ms":120000
        }},
        "synthesize":{{
          "role":"lead",
          "message":"Synthesize the prior outputs and include the literal token FLOW_MATRIX_23 in your answer.",
          "depends_on":["review"],
          "timeout_ms":120000
        }}
      }}
    }}
  }},
  "skills":{{}}
}}"#,
        model = smoke_model()
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition).await?;
    Ok(mob_dir)
}

async fn write_turn_probe_mobpack_fixture(
    project_dir: &Path,
    mob_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mob_dir = project_dir.join(format!("{mob_id}-fixture"));
    tokio::fs::create_dir_all(&mob_dir).await?;
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{mob_id}\"\nversion = \"1.0.0\"\n"),
    )
    .await?;

    let definition = format!(
        r#"{{
  "id":"{mob_id}",
  "profiles":{{
    "worker":{{
      "model":"{model}",
      "tools":{{"comms":true}},
      "external_addressable":true,
      "peer_description":"Turn-driven worker"
    }},
    "broken":{{
      "model":"definitely-invalid-live-smoke-model",
      "tools":{{"comms":true}},
      "external_addressable":true,
      "peer_description":"Deterministic failure worker"
    }}
  }},
  "wiring":{{"auto_wire_orchestrator":false,"role_wiring":[]}},
  "skills":{{}}
}}"#,
        model = smoke_model()
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition).await?;
    Ok(mob_dir)
}

struct RpcSurfaceChild {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

async fn spawn_mob_rpc_surface(
    rkat: &Path,
    cwd: &Path,
    pack: &Path,
    prompt: &str,
    api_key: Option<&str>,
) -> Result<RpcSurfaceChild, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(rkat);
    cmd.current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .args([
            "mob",
            "deploy",
            &pack.display().to_string(),
            prompt,
            "--surface",
            "rpc",
            "--trust-policy",
            "permissive",
        ]);
    if let Some(key) = api_key {
        cmd.env("ANTHROPIC_API_KEY", key)
            .env("RKAT_ANTHROPIC_API_KEY", key);
    }
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().ok_or("missing child stdin")?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    Ok(RpcSurfaceChild {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

async fn rpc_send(
    surface: &mut RpcSurfaceChild,
    request: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let line = format!("{}\n", serde_json::to_string(request)?);
    surface.stdin.write_all(line.as_bytes()).await?;
    surface.stdin.flush().await?;
    Ok(())
}

async fn rpc_read_response(
    surface: &mut RpcSurfaceChild,
    timeout_secs: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let mut line = String::new();
        timeout(
            Duration::from_secs(timeout_secs),
            surface.stdout.read_line(&mut line),
        )
        .await??;
        if line.trim().is_empty() {
            continue;
        }
        // Skip non-JSON lines (e.g. deploy status output) gracefully.
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if parsed.get("id").is_some() {
            return Ok(parsed);
        }
    }
}

async fn rpc_call(
    surface: &mut RpcSurfaceChild,
    id: u64,
    method: &str,
    params: Value,
    timeout_secs: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    rpc_send(
        surface,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await?;
    let response = rpc_read_response(surface, timeout_secs).await?;
    if !response["error"].is_null() {
        return Err(format!("rpc {method} failed: {response}").into());
    }
    Ok(response["result"].clone())
}

async fn shutdown_rpc_surface(
    mut surface: RpcSurfaceChild,
) -> Result<(), Box<dyn std::error::Error>> {
    drop(surface.stdin);
    let status = timeout(Duration::from_secs(20), surface.child.wait()).await??;
    if !status.success() {
        return Err(format!("rpc surface exited unsuccessfully: {status}").into());
    }
    Ok(())
}

async fn poll_flow_status_until_terminal(
    surface: &mut RpcSurfaceChild,
    mob_id: &str,
    run_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut request_id = 10_000u64;
    loop {
        let status = rpc_call(
            surface,
            request_id,
            "mob/flow_status",
            json!({
                "mob_id": mob_id,
                "run_id": run_id,
            }),
            30,
        )
        .await?;
        request_id += 1;
        let Some(run) = status.get("run") else {
            return Err("mob/flow_status returned no run payload".into());
        };
        let state = run
            .get("status")
            .and_then(Value::as_str)
            .ok_or("mob/flow_status missing run.status")?;
        if matches!(state, "completed" | "failed" | "canceled") {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("flow {run_id} did not reach terminal state: {status}").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn poll_members_until(
    surface: &mut RpcSurfaceChild,
    mob_id: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut request_id = 20_000u64;
    loop {
        let members = rpc_call(
            surface,
            request_id,
            "mob/members",
            json!({ "mob_id": mob_id }),
            15,
        )
        .await?;
        request_id += 1;
        if predicate(&members) {
            return Ok(members);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("mob/members predicate did not converge: {members}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "integration-real: spawns rkat binary"]
async fn e2e_smoke_mobpack_pack_inspect_validate() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }

    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_dir = write_mobpack_fixture(&project_dir).await?;
    let pack = project_dir.join("smoke.mobpack");
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let pack_stdout = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;
    assert!(
        !pack_stdout.is_empty(),
        "mob pack should print digest output, got empty stdout"
    );

    let inspect_args = [
        "mob".to_string(),
        "inspect".to_string(),
        pack.display().to_string(),
    ];
    let inspect_refs: Vec<&str> = inspect_args.iter().map(String::as_str).collect();
    let inspect_out = run_rkat(&rkat, &project_dir, &inspect_refs, None).await?;
    let inspect_stdout =
        output_ok_or_err(inspect_out, &inspect_refs).map_err(std::io::Error::other)?;
    assert!(
        inspect_stdout.contains("name\tsmoke-mobpack") && inspect_stdout.contains("digest\t"),
        "inspect output missing expected fields: {inspect_stdout}"
    );

    let validate_args = [
        "mob".to_string(),
        "validate".to_string(),
        pack.display().to_string(),
    ];
    let validate_refs: Vec<&str> = validate_args.iter().map(String::as_str).collect();
    let validate_out = run_rkat(&rkat, &project_dir, &validate_refs, None).await?;
    let validate_stdout =
        output_ok_or_err(validate_out, &validate_refs).map_err(std::io::Error::other)?;
    assert!(
        validate_stdout.starts_with("valid\t"),
        "validate output should start with valid<TAB>, got: {validate_stdout}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_mobpack_deploy_unsigned_permissive_live()
-> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let api_key = anthropic_api_key().ok_or("missing API key")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_dir = write_mobpack_fixture(&project_dir).await?;
    let pack = project_dir.join("unsigned.mobpack");
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, Some(&api_key)).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let deploy_args = [
        "mob".to_string(),
        "deploy".to_string(),
        pack.display().to_string(),
        "Reply with OK".to_string(),
        "--trust-policy".to_string(),
        "permissive".to_string(),
    ];
    let deploy_refs: Vec<&str> = deploy_args.iter().map(String::as_str).collect();
    let deploy_out = run_rkat(&rkat, &project_dir, &deploy_refs, Some(&api_key)).await?;
    let deploy_stdout =
        output_ok_or_err(deploy_out, &deploy_refs).map_err(std::io::Error::other)?;
    assert!(
        deploy_stdout.contains("deployed\tmob=smoke-mobpack\tsurface=cli"),
        "deploy output missing expected deployment marker: {deploy_stdout}"
    );
    assert!(
        deploy_stdout.contains("warning\tunsigned pack accepted in permissive mode"),
        "permissive unsigned warning expected: {deploy_stdout}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_28_cli_mobpack_deploy_signed_strict_live()
-> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let api_key = anthropic_api_key().ok_or("missing API key")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_dir = write_mobpack_fixture(&project_dir).await?;
    let pack = project_dir.join("signed.mobpack");
    let key_path = project_dir.join("signing.key");
    tokio::fs::write(
        &key_path,
        "0707070707070707070707070707070707070707070707070707070707070707",
    )
    .await?;
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
        "--sign".to_string(),
        key_path.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, Some(&api_key)).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let pack_bytes = tokio::fs::read(&pack).await?;
    let (signer_id, public_key) = signer_from_pack(&pack_bytes)?;
    let trust_path = project_dir.join(".rkat").join("trusted-signers.toml");
    tokio::fs::create_dir_all(trust_path.parent().ok_or("missing trust parent")?).await?;
    tokio::fs::write(
        &trust_path,
        format!("[signers]\n{signer_id} = \"{public_key}\"\n"),
    )
    .await?;

    let deploy_args = [
        "mob".to_string(),
        "deploy".to_string(),
        pack.display().to_string(),
        "Reply with OK".to_string(),
        "--trust-policy".to_string(),
        "strict".to_string(),
    ];
    let deploy_refs: Vec<&str> = deploy_args.iter().map(String::as_str).collect();
    let deploy_out = run_rkat(&rkat, &project_dir, &deploy_refs, Some(&api_key)).await?;
    let deploy_stdout =
        output_ok_or_err(deploy_out, &deploy_refs).map_err(std::io::Error::other)?;
    assert!(
        deploy_stdout.contains("deployed\tmob=smoke-mobpack\tsurface=cli"),
        "strict signed deploy output missing expected marker: {deploy_stdout}"
    );
    assert!(
        !deploy_stdout.contains("\nwarning\t"),
        "strict signed deploy should not emit warnings: {deploy_stdout}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: wasm surface smoke"]
async fn e2e_smoke_wasm_surface_gate() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_dir = write_mobpack_fixture(&project_dir).await?;
    let pack = project_dir.join("wasm-smoke.mobpack");

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let wasm_args = [
        "mob".to_string(),
        "web".to_string(),
        "build".to_string(),
        pack.display().to_string(),
        "-o".to_string(),
        project_dir.join("web-out").display().to_string(),
    ];
    let wasm_refs: Vec<&str> = wasm_args.iter().map(String::as_str).collect();
    let wasm_out = run_rkat(&rkat, &project_dir, &wasm_refs, None).await?;
    if !wasm_out.status.success() {
        return Err(format!(
            "wasm smoke command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&wasm_out.stdout),
            String::from_utf8_lossy(&wasm_out.stderr)
        )
        .into());
    }
    let out_dir = project_dir.join("web-out");
    for file in [
        "index.html",
        "runtime.js",
        "runtime_bg.wasm",
        "mobpack.bin",
        "manifest.web.toml",
    ] {
        assert!(
            out_dir.join(file).exists(),
            "wasm build succeeded but artifact is missing: {}",
            out_dir.join(file).display()
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: wasm forbidden-capability negative smoke"]
async fn e2e_smoke_wasm_forbidden_capability_rejected() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_dir = project_dir.join("forbidden-mobpack");
    tokio::fs::create_dir_all(mob_dir.join("skills")).await?;
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        r#"[mobpack]
name = "forbidden-web"
version = "1.0.0"

[requires]
capabilities = ["shell"]
"#,
    )
    .await?;
    tokio::fs::write(
        mob_dir.join("definition.json"),
        br#"{"id":"forbidden-web"}"#,
    )
    .await?;
    let pack = project_dir.join("forbidden-web.mobpack");

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let wasm_args = [
        "mob".to_string(),
        "web".to_string(),
        "build".to_string(),
        pack.display().to_string(),
        "-o".to_string(),
        project_dir.join("web-out").display().to_string(),
    ];
    let wasm_refs: Vec<&str> = wasm_args.iter().map(String::as_str).collect();
    let wasm_out = run_rkat(&rkat, &project_dir, &wasm_refs, None).await?;
    assert!(
        !wasm_out.status.success(),
        "web build should reject forbidden capabilities"
    );
    let stderr = String::from_utf8_lossy(&wasm_out.stderr);
    assert!(
        stderr.contains("forbidden capability 'shell' is not allowed for web builds"),
        "unexpected stderr: {stderr}"
    );
    Ok(())
}

// ===========================================================================
// Supplemental: CLI mob RPC surface state-machine probe
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: spawns rkat binary with mob rpc surface"]
async fn e2e_cli_mob_rpc_state_machine_probe() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }

    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_id = "scenario-29-swarm";
    let mob_dir = write_rpc_state_probe_mobpack_fixture(&project_dir, mob_id).await?;
    let pack = project_dir.join("scenario-29.mobpack");
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let mut surface = spawn_mob_rpc_surface(&rkat, &project_dir, &pack, "bootstrap", None).await?;

    let initialize = rpc_call(&mut surface, 1, "initialize", json!({}), 20).await?;
    let methods = initialize["methods"]
        .as_array()
        .ok_or("initialize must return methods array")?;
    assert!(
        methods
            .iter()
            .any(|value| value.as_str() == Some("mob/status"))
            && methods
                .iter()
                .any(|value| value.as_str() == Some("mob/flow_run")),
        "initialize missing mob methods: {initialize}"
    );

    let listed = rpc_call(&mut surface, 2, "mob/list", json!({}), 15).await?;
    let mobs = listed["mobs"]
        .as_array()
        .ok_or("mob/list missing mobs array")?;
    assert!(
        mobs.iter()
            .any(|entry| entry["mob_id"].as_str() == Some(mob_id) && entry["status"] == "Running"),
        "deployed mob should be visible and running: {listed}"
    );

    let spawned = rpc_call(
        &mut surface,
        3,
        "mob/spawn_many",
        json!({
            "mob_id": mob_id,
            "specs": [
                {"profile":"lead","meerkat_id":"lead-1","runtime_mode":"turn_driven"},
                {"profile":"worker","meerkat_id":"worker-1","runtime_mode":"turn_driven"},
                {"profile":"reviewer","meerkat_id":"reviewer-1","runtime_mode":"turn_driven"}
            ]
        }),
        30,
    )
    .await?;
    let results = spawned["results"]
        .as_array()
        .ok_or("mob/spawn_many missing results array")?;
    assert_eq!(results.len(), 3, "expected three spawn results: {spawned}");
    assert!(
        results.iter().all(|entry| entry["ok"] == true),
        "all spawn_many entries should succeed: {spawned}"
    );
    let worker_session_id = results[1]["session_id"]
        .as_str()
        .ok_or("worker spawn result missing session_id")?
        .to_string();

    let members = poll_members_until(&mut surface, mob_id, |payload| {
        payload["members"]
            .as_array()
            .is_some_and(|members| members.len() == 3)
    })
    .await?;
    let members_array = members["members"]
        .as_array()
        .ok_or("members array missing")?;
    assert!(
        members_array
            .iter()
            .any(|entry| entry["meerkat_id"].as_str() == Some("worker-1")),
        "worker should appear in mob/members: {members}"
    );

    let _ = rpc_call(
        &mut surface,
        4,
        "mob/wire",
        json!({"mob_id": mob_id, "a":"lead-1", "b":"worker-1"}),
        15,
    )
    .await?;
    let wired_members = poll_members_until(&mut surface, mob_id, |payload| {
        payload["members"].as_array().is_some_and(|members| {
            members.iter().any(|entry| {
                entry["meerkat_id"].as_str() == Some("lead-1")
                    && entry["wired_to"].as_array().is_some_and(|wired| {
                        wired.iter().any(|peer| peer.as_str() == Some("worker-1"))
                    })
            })
        })
    })
    .await?;
    assert!(
        wired_members["members"].as_array().is_some(),
        "wired members payload malformed: {wired_members}"
    );

    let _ = rpc_call(
        &mut surface,
        5,
        "mob/unwire",
        json!({"mob_id": mob_id, "a":"lead-1", "b":"worker-1"}),
        15,
    )
    .await?;

    let appended = rpc_call(
        &mut surface,
        6,
        "mob/append_system_context",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "text": "Always include the token CTX_MOB_29.",
            "source": "mob",
            "idempotency_key": "scenario-29-worker"
        }),
        15,
    )
    .await?;
    assert_eq!(appended["session_id"], worker_session_id);
    assert_eq!(appended["status"], "staged");

    let sent = rpc_call(
        &mut surface,
        7,
        "mob/send",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "message": "Reply with TURN_PROBE_29 and include CTX_MOB_29."
        }),
        120,
    )
    .await?;
    assert_eq!(sent["sent"], true);

    let read_after_send = rpc_call(
        &mut surface,
        8,
        "session/read",
        json!({ "session_id": worker_session_id }),
        30,
    )
    .await?;
    let last_text = read_after_send["last_assistant_text"]
        .as_str()
        .ok_or("session/read missing last_assistant_text after mob/send")?
        .to_uppercase();
    assert!(
        last_text.contains("TURN_PROBE_29") && last_text.contains("CTX_MOB_29"),
        "mob/send turn should materialize assistant output with the staged context: {read_after_send}"
    );

    let _ = rpc_call(
        &mut surface,
        9,
        "mob/retire",
        json!({"mob_id": mob_id, "meerkat_id":"reviewer-1"}),
        15,
    )
    .await?;
    let after_retire = poll_members_until(&mut surface, mob_id, |payload| {
        payload["members"]
            .as_array()
            .is_some_and(|members| members.len() == 2)
    })
    .await?;
    assert!(
        after_retire["members"].as_array().is_some_and(|members| {
            members
                .iter()
                .all(|entry| entry["meerkat_id"].as_str() != Some("reviewer-1"))
        }),
        "retired reviewer should disappear from members: {after_retire}"
    );

    let _ = rpc_call(
        &mut surface,
        10,
        "mob/respawn",
        json!({"mob_id": mob_id, "meerkat_id":"worker-1"}),
        15,
    )
    .await?;
    let after_respawn = poll_members_until(&mut surface, mob_id, |payload| {
        payload["members"].as_array().is_some_and(|members| {
            members.iter().any(|entry| {
                entry["meerkat_id"].as_str() == Some("worker-1") && entry["state"] == "Active"
            })
        })
    })
    .await?;
    assert!(
        after_respawn["members"]
            .as_array()
            .is_some_and(|members| members.len() == 2),
        "respawn should preserve two active members: {after_respawn}"
    );
    let respawned_session_id = after_respawn["members"]
        .as_array()
        .and_then(|members| {
            members.iter().find_map(|entry| {
                (entry["meerkat_id"].as_str() == Some("worker-1"))
                    .then(|| entry["member_ref"]["session_id"].as_str())
                    .flatten()
                    .map(ToString::to_string)
            })
        })
        .ok_or("respawned worker session_id missing")?;
    let sent_after_respawn = rpc_call(
        &mut surface,
        11,
        "mob/send",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "message": "Reply with RESPAWN_PROBE_29."
        }),
        120,
    )
    .await?;
    assert_eq!(sent_after_respawn["sent"], true);

    let read_after_respawn = rpc_call(
        &mut surface,
        12,
        "session/read",
        json!({ "session_id": respawned_session_id }),
        30,
    )
    .await?;
    let respawn_text = read_after_respawn["last_assistant_text"]
        .as_str()
        .ok_or("session/read missing last_assistant_text after respawn")?
        .to_uppercase();
    assert!(
        respawn_text.contains("RESPAWN_PROBE_29"),
        "respawned worker should answer after respawn: {read_after_respawn}"
    );

    let stopped = rpc_call(
        &mut surface,
        13,
        "mob/lifecycle",
        json!({"mob_id": mob_id, "action":"stop"}),
        15,
    )
    .await?;
    assert_eq!(stopped["ok"], true);
    let stopped_status = rpc_call(
        &mut surface,
        14,
        "mob/status",
        json!({"mob_id": mob_id}),
        15,
    )
    .await?;
    assert_eq!(stopped_status["status"], "Stopped");

    let resumed = rpc_call(
        &mut surface,
        15,
        "mob/lifecycle",
        json!({"mob_id": mob_id, "action":"resume"}),
        15,
    )
    .await?;
    assert_eq!(resumed["ok"], true);
    let resumed_status = rpc_call(
        &mut surface,
        16,
        "mob/status",
        json!({"mob_id": mob_id}),
        15,
    )
    .await?;
    assert_eq!(resumed_status["status"], "Running");

    let events = rpc_call(
        &mut surface,
        17,
        "mob/events",
        json!({"mob_id": mob_id, "after_cursor": 0, "limit": 200}),
        15,
    )
    .await?;
    let event_count = events["events"].as_array().map_or(0, Vec::len);
    assert!(
        event_count >= 5,
        "state-machine probe should emit a non-trivial event ledger: {events}"
    );

    let destroyed = rpc_call(
        &mut surface,
        18,
        "mob/lifecycle",
        json!({"mob_id": mob_id, "action":"destroy"}),
        15,
    )
    .await?;
    assert_eq!(destroyed["ok"], true);
    let listed_after_destroy = rpc_call(&mut surface, 19, "mob/list", json!({}), 15).await?;
    assert!(
        listed_after_destroy["mobs"]
            .as_array()
            .is_some_and(|mobs| mobs
                .iter()
                .all(|entry| entry["mob_id"].as_str() != Some(mob_id))),
        "destroyed mob should disappear from mob/list: {listed_after_destroy}"
    );

    shutdown_rpc_surface(surface).await?;
    Ok(())
}

// ===========================================================================
// Scenario 30: CLI mob RPC surface live flow probe
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API mob flow"]
async fn e2e_scenario_30_cli_mob_rpc_flow_probe() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let api_key = anthropic_api_key().ok_or("missing API key")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_id = "scenario-30-flow";
    let mob_dir = write_flow_probe_mobpack_fixture(&project_dir, mob_id).await?;
    let pack = project_dir.join("scenario-30.mobpack");
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, Some(&api_key)).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let mut surface =
        spawn_mob_rpc_surface(&rkat, &project_dir, &pack, "bootstrap", Some(&api_key)).await?;
    let _ = rpc_call(&mut surface, 101, "initialize", json!({}), 20).await?;

    let _ = rpc_call(
        &mut surface,
        102,
        "mob/spawn_many",
        json!({
            "mob_id": mob_id,
            "specs": [
                {"profile":"lead","meerkat_id":"lead-1","runtime_mode":"turn_driven"},
                {"profile":"analyst","meerkat_id":"analyst-1","runtime_mode":"turn_driven"},
                {"profile":"reviewer","meerkat_id":"reviewer-1","runtime_mode":"turn_driven"}
            ]
        }),
        30,
    )
    .await?;

    let flows = rpc_call(
        &mut surface,
        103,
        "mob/flows",
        json!({"mob_id": mob_id}),
        15,
    )
    .await?;
    assert!(
        flows["flows"]
            .as_array()
            .is_some_and(|flows| flows.iter().any(|flow| flow.as_str() == Some("main"))),
        "mob should expose the main flow: {flows}"
    );

    let started = rpc_call(
        &mut surface,
        104,
        "mob/flow_run",
        json!({
            "mob_id": mob_id,
            "flow_id": "main",
            "params": { "ticket": "FLOW-23" }
        }),
        20,
    )
    .await?;
    let run_id = started["run_id"]
        .as_str()
        .ok_or("mob/flow_run missing run_id")?
        .to_string();

    let terminal = poll_flow_status_until_terminal(&mut surface, mob_id, &run_id).await?;
    let run = terminal["run"]
        .as_object()
        .ok_or("terminal flow status missing run object")?;
    let terminal_status = run
        .get("status")
        .and_then(Value::as_str)
        .ok_or("run.status missing")?;
    assert!(
        matches!(terminal_status, "completed" | "failed"),
        "live flow probe should reach a non-canceled terminal status: {terminal}"
    );
    let step_ledger = run
        .get("step_ledger")
        .and_then(Value::as_array)
        .ok_or("run.step_ledger missing")?;
    assert!(
        step_ledger.len() >= 3,
        "flow should record at least three step ledger entries: {terminal}"
    );
    let failures = run
        .get("failure_ledger")
        .and_then(Value::as_array)
        .ok_or("run.failure_ledger missing")?;

    let events = rpc_call(
        &mut surface,
        105,
        "mob/events",
        json!({"mob_id": mob_id, "after_cursor": 0, "limit": 200}),
        15,
    )
    .await?;
    let items = events["events"]
        .as_array()
        .ok_or("mob/events missing events array")?;
    match terminal_status {
        "completed" => {
            assert!(
                step_ledger.iter().any(|entry| {
                    entry["step_id"].as_str() == Some("synthesize")
                        && entry["status"].as_str() == Some("completed")
                }),
                "completed flow should include a completed synthesize step: {terminal}"
            );
            assert!(
                failures.is_empty(),
                "completed flow should not record failure ledger entries: {terminal}"
            );
            assert!(
                items.iter().any(|event| {
                    event["kind"]["type"].as_str() == Some("flow_completed")
                        && event["kind"]["run_id"].as_str() == Some(run_id.as_str())
                }),
                "event ledger should record FlowCompleted: {events}"
            );
        }
        "failed" => {
            assert!(
                !failures.is_empty(),
                "failed flow should surface failure ledger entries: {terminal}"
            );
            assert!(
                items.iter().any(|event| {
                    event["kind"]["type"].as_str() == Some("flow_failed")
                        && event["kind"]["run_id"].as_str() == Some(run_id.as_str())
                }),
                "event ledger should record FlowFailed for terminal failures: {events}"
            );
        }
        _ => unreachable!("terminal status filtered above"),
    }

    let _ = rpc_call(
        &mut surface,
        106,
        "mob/lifecycle",
        json!({"mob_id": mob_id, "action":"destroy"}),
        15,
    )
    .await?;
    shutdown_rpc_surface(surface).await?;
    Ok(())
}

// ===========================================================================
// Scenario 29: CLI mob RPC surface member-turn probe
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API mob member turn"]
async fn e2e_scenario_29_cli_mob_rpc_member_turn_probe() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let api_key = anthropic_api_key().ok_or("missing API key")?;
    let tmp = TempDir::new()?;
    let project_dir = tmp.path().join("project");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let mob_id = "scenario-29-turn";
    let mob_dir = write_turn_probe_mobpack_fixture(&project_dir, mob_id).await?;
    let pack = project_dir.join("scenario-29.mobpack");
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let pack_args = [
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, Some(&api_key)).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let mut surface =
        spawn_mob_rpc_surface(&rkat, &project_dir, &pack, "bootstrap", Some(&api_key)).await?;
    let _ = rpc_call(&mut surface, 201, "initialize", json!({}), 20).await?;

    let spawned = rpc_call(
        &mut surface,
        202,
        "mob/spawn",
        json!({
            "mob_id": mob_id,
            "profile": "worker",
            "meerkat_id": "worker-1",
            "runtime_mode": "turn_driven"
        }),
        20,
    )
    .await?;
    let original_session_id = spawned["session_id"]
        .as_str()
        .ok_or("mob/spawn missing session_id")?
        .to_string();

    let appended = rpc_call(
        &mut surface,
        203,
        "mob/append_system_context",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "text": "Always include the token CTX_MOB_29.",
            "source": "mob",
            "idempotency_key": "scenario-29-context"
        }),
        20,
    )
    .await?;
    assert_eq!(appended["status"], "staged");
    assert_eq!(appended["session_id"], original_session_id);

    let sent = rpc_call(
        &mut surface,
        204,
        "mob/send",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "message": "Reply with TURN_PROBE_29 and include CTX_MOB_29."
        }),
        120,
    )
    .await?;
    assert_eq!(sent["sent"], true);

    let read_after_send = rpc_call(
        &mut surface,
        205,
        "session/read",
        json!({ "session_id": original_session_id }),
        30,
    )
    .await?;
    let last_text = read_after_send["last_assistant_text"]
        .as_str()
        .ok_or("session/read missing last_assistant_text after mob/send")?
        .to_uppercase();
    assert!(
        last_text.contains("TURN_PROBE_29") && last_text.contains("CTX_MOB_29"),
        "mob/send turn should materialize assistant output with the staged context: {read_after_send}"
    );

    match rpc_call(
        &mut surface,
        206,
        "mob/spawn",
        json!({
            "mob_id": mob_id,
            "profile": "broken",
            "meerkat_id": "broken-1",
            "runtime_mode": "turn_driven"
        }),
        20,
    )
    .await
    {
        Err(spawn_err) => {
            assert!(
                spawn_err
                    .to_string()
                    .contains("definitely-invalid-live-smoke-model"),
                "broken member spawn should fail with the invalid model error: {spawn_err}"
            );
        }
        Ok(broken) => {
            let broken_session_id = broken["session_id"]
                .as_str()
                .ok_or("mob/spawn missing broken session_id")?
                .to_string();
            let broken_send_err = rpc_call(
                &mut surface,
                207,
                "mob/send",
                json!({
                    "mob_id": mob_id,
                    "meerkat_id": "broken-1",
                    "message": "This turn must fail because the member model is invalid."
                }),
                60,
            )
            .await
            .expect_err("broken member must fail no later than its first execution attempt");
            assert!(
                broken_send_err.to_string().contains("rpc mob/send failed"),
                "unexpected broken member error: {broken_send_err}"
            );
            let broken_read = rpc_call(
                &mut surface,
                208,
                "session/read",
                json!({ "session_id": broken_session_id }),
                30,
            )
            .await?;
            assert!(
                broken_read["last_assistant_text"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "failed member turns should not fabricate assistant text: {broken_read}"
            );
        }
    }

    let _ = rpc_call(
        &mut surface,
        209,
        "mob/respawn",
        json!({"mob_id": mob_id, "meerkat_id":"worker-1"}),
        20,
    )
    .await?;
    let respawned_members = poll_members_until(&mut surface, mob_id, |payload| {
        payload["members"].as_array().is_some_and(|members| {
            members.iter().any(|entry| {
                entry["meerkat_id"].as_str() == Some("worker-1") && entry["state"] == "Active"
            })
        })
    })
    .await?;
    let respawned_session_id = respawned_members["members"]
        .as_array()
        .and_then(|members| {
            members.iter().find_map(|entry| {
                (entry["meerkat_id"].as_str() == Some("worker-1"))
                    .then(|| entry["member_ref"]["session_id"].as_str())
                    .flatten()
                    .map(ToString::to_string)
            })
        })
        .ok_or("respawned worker session_id missing")?;
    let sent_after_respawn = rpc_call(
        &mut surface,
        210,
        "mob/send",
        json!({
            "mob_id": mob_id,
            "meerkat_id": "worker-1",
            "message": "Reply with RESPAWN_PROBE_29."
        }),
        120,
    )
    .await?;
    assert_eq!(sent_after_respawn["session_id"], respawned_session_id);

    let read_after_respawn = rpc_call(
        &mut surface,
        211,
        "session/read",
        json!({ "session_id": respawned_session_id }),
        30,
    )
    .await?;
    let respawn_text = read_after_respawn["last_assistant_text"]
        .as_str()
        .ok_or("session/read missing last_assistant_text after respawn")?
        .to_uppercase();
    assert!(
        respawn_text.contains("RESPAWN_PROBE_29"),
        "respawned worker should answer on its new session: {read_after_respawn}"
    );

    let events = rpc_call(
        &mut surface,
        212,
        "mob/events",
        json!({"mob_id": mob_id, "after_cursor": 0, "limit": 200}),
        20,
    )
    .await?;
    assert!(
        events["events"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "member-turn probe should expose a non-empty event ledger: {events}"
    );

    let _ = rpc_call(
        &mut surface,
        213,
        "mob/lifecycle",
        json!({"mob_id": mob_id, "action":"destroy"}),
        15,
    )
    .await?;
    shutdown_rpc_surface(surface).await?;
    Ok(())
}
