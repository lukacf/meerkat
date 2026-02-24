#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
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

    let pack_args = vec![
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

    let inspect_args = vec![
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

    let validate_args = vec![
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

    let pack_args = vec![
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, Some(&api_key)).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let deploy_args = vec![
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
async fn e2e_smoke_mobpack_deploy_signed_strict_live() -> Result<(), Box<dyn std::error::Error>> {
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

    let pack_args = vec![
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
        format!("[signers]\n{} = \"{}\"\n", signer_id, public_key),
    )
    .await?;

    let deploy_args = vec![
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

    let pack_args = vec![
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let wasm_args = vec![
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

    let pack_args = vec![
        "mob".to_string(),
        "pack".to_string(),
        mob_dir.display().to_string(),
        "-o".to_string(),
        pack.display().to_string(),
    ];
    let pack_refs: Vec<&str> = pack_args.iter().map(String::as_str).collect();
    let pack_out = run_rkat(&rkat, &project_dir, &pack_refs, None).await?;
    let _ = output_ok_or_err(pack_out, &pack_refs).map_err(std::io::Error::other)?;

    let wasm_args = vec![
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
