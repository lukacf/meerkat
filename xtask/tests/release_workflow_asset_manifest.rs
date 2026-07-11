#![allow(clippy::expect_used, clippy::panic)]

//! Pinning tests for the public release-asset manifest contract.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::tempdir;

fn workflow_yml_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push(".github/workflows/release.yml");
    path
}

fn read_workflow(path: &Path) -> serde_yaml::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|error| panic!("cannot parse {} as YAML: {error}", path.display()))
}

fn step_script<'a>(
    workflow: &'a serde_yaml::Value,
    path: &Path,
    job_name: &str,
    step_name: &str,
) -> &'a str {
    workflow
        .get("jobs")
        .and_then(|jobs| jobs.get(job_name))
        .and_then(|job| job.get("steps"))
        .and_then(serde_yaml::Value::as_sequence)
        .and_then(|steps| {
            steps.iter().find(|step| {
                step.get("name").and_then(serde_yaml::Value::as_str) == Some(step_name)
            })
        })
        .and_then(|step| step.get("run"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "{} must define {job_name:?} step {step_name:?} with a run script",
                path.display()
            )
        })
}

fn embedded_python(script: &str) -> &str {
    script
        .split_once("python3 - <<'PY'\n")
        .and_then(|(_, rest)| rest.rsplit_once("\nPY").map(|(python, _)| python))
        .unwrap_or_else(|| {
            panic!("manifest step must contain a python3 heredoc; script:\n{script}")
        })
}

fn run_manifest(python: &str, root: &Path) -> Output {
    let interpreter = std::env::var_os("PYTHON").unwrap_or_else(|| OsString::from("python3"));
    Command::new(interpreter)
        .arg("-c")
        .arg(python)
        .current_dir(root)
        .env("RELEASE_TAG", "v0.7.28")
        .output()
        .expect("run release manifest Python")
}

fn manifest_python() -> String {
    let path = workflow_yml_path();
    let workflow = read_workflow(&path);
    let script = step_script(
        &workflow,
        &path,
        "publish_github_release",
        "Build checksum manifest",
    );
    embedded_python(script).to_owned()
}

#[test]
fn release_asset_manifest_contract_stays_flat_and_collision_checked() {
    let path = workflow_yml_path();
    let workflow = read_workflow(&path);
    let script = step_script(
        &workflow,
        &path,
        "publish_github_release",
        "Build checksum manifest",
    );

    for contract in [
        "key=lambda path: (path.name, path.as_posix())",
        "public_name = path.name",
        "previous = artifacts_by_name.get(public_name)",
        "duplicate release asset basename",
        "f\"{sha256(path)}  {public_name}\\n\"",
        "\"artifacts\": list(artifacts_by_name)",
    ] {
        assert!(
            script.contains(contract),
            "release manifest step must preserve `{contract}`; script:\n{script}"
        );
    }

    assert!(
        !script.contains("p.as_posix().lstrip(\"./\")"),
        "index.json must not expose workflow-artifact staging directories"
    );
    assert!(
        !script.contains("xargs sha256sum"),
        "checksums.sha256 must be rendered from public basenames, not staged paths"
    );
}

#[test]
fn nested_release_artifacts_render_flat_sorted_manifests() {
    let temp = tempdir().expect("tempdir");
    let rpc_dir = temp.path().join("meerkat-linux-buildbuddy");
    let cli_dir = temp.path().join("meerkat-macos-buildbuddy");
    std::fs::create_dir_all(&rpc_dir).expect("create rpc artifact directory");
    std::fs::create_dir_all(&cli_dir).expect("create cli artifact directory");

    let cli_name = "rkat-0.7.28-aarch64-apple-darwin.tar.gz";
    let rpc_name = "rkat-rpc-0.7.28-x86_64-unknown-linux-gnu.zip";
    std::fs::write(cli_dir.join(cli_name), b"alpha").expect("write cli archive");
    std::fs::write(rpc_dir.join(rpc_name), b"beta").expect("write rpc archive");

    let output = run_manifest(&manifest_python(), temp.path());
    assert!(
        output.status.success(),
        "manifest script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let checksums = std::fs::read_to_string(temp.path().join("checksums.sha256"))
        .expect("read checksum manifest");
    assert_eq!(
        checksums,
        concat!(
            "8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccdda1ed4018e8f2223f8  ",
            "rkat-0.7.28-aarch64-apple-darwin.tar.gz\n",
            "f44e64e75f3948e9f73f8dfa94721c4ce8cbb4f265c4790c702b2d41cfbf2753  ",
            "rkat-rpc-0.7.28-x86_64-unknown-linux-gnu.zip\n",
        ),
        "checksums must be sorted by flat public basename"
    );

    let index: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("index.json")).expect("read release index"),
    )
    .expect("parse release index");
    assert_eq!(index["version"], "0.7.28");
    assert_eq!(index["tag"], "v0.7.28");
    assert_eq!(index["checksums"], "checksums.sha256");
    assert_eq!(
        index["artifacts"],
        serde_json::json!([cli_name, rpc_name]),
        "index artifacts must be sorted flat public basenames"
    );
}

#[test]
fn duplicate_release_asset_basenames_fail_before_manifest_write() {
    let temp = tempdir().expect("tempdir");
    let duplicate_name = "rkat-0.7.28-aarch64-apple-darwin.tar.gz";
    for directory in ["github-hosted", "buildbuddy"] {
        let artifact_dir = temp.path().join(directory);
        std::fs::create_dir_all(&artifact_dir).expect("create artifact directory");
        std::fs::write(artifact_dir.join(duplicate_name), directory.as_bytes())
            .expect("write duplicate archive");
    }

    let output = run_manifest(&manifest_python(), temp.path());
    assert!(
        !output.status.success(),
        "duplicate public asset names must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate release asset basename") && stderr.contains(duplicate_name),
        "duplicate failure must identify the public name: {stderr}"
    );
    assert!(
        !temp.path().join("checksums.sha256").exists() && !temp.path().join("index.json").exists(),
        "a duplicate set must not leave a successful-looking manifest"
    );
}
