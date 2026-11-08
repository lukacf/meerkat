"""Controls for the direct production's monorepo source identity, on real Git."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import sys

import pytest

SCRIPT = Path(__file__).with_name("connect-source-identity.py")


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def commit(repo, message):
    git(repo, "add", "-A")
    git(repo, "-c", "user.name=Offline Control", "-c", "user.email=offline@example.invalid",
        "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / "meerkat"
    files = {
        "Cargo.toml": "[workspace]\nmembers = [\"crates/*\", \"connect/crates/*\"]\n",
        "Cargo.lock": "version = 4\n",
        "rust-toolchain.toml": "[toolchain]\nchannel = \"1.97.0\"\n",
        ".cargo/config.toml": "[env]\nRUST_MIN_STACK = \"8388608\"\n",
        "crates/meerkat-core/src/lib.rs": "pub fn core() {}\n",
        "connect/SOURCE-PROVENANCE.md": "# Source provenance\n",
        "connect/PUBLIC-SOURCE-COMMITS.json": "{\"schema_version\": 1}\n",
        "connect/crates/meerkat-connect/src/main.rs": "fn main() {}\n",
        "connect/scripts/run.sh": "#!/bin/sh\n",
    }
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    (root / "connect/scripts/run.sh").chmod(0o755)
    shutil.copyfile(SCRIPT, root / "connect/scripts/connect-source-identity.py")
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    revision = commit(root, "controlled monorepo")
    return root, revision


def run(root, *args):
    return subprocess.run([sys.executable, "-I", str(root / "connect/scripts/connect-source-identity.py"), *args],
                          capture_output=True, text=True, cwd=root)


def record(root, revision, output, export=None):
    result = run(root, "record", "--commit", revision, "--export-commit", export or revision, "--output", str(output))
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def check(root, output, sha256):
    return run(root, "check", "--manifest", str(output), "--manifest-sha256", sha256)


def test_record_is_a_pure_function_of_the_commit_and_checks(repo, tmp_path):
    root, revision = repo
    first = record(root, revision, tmp_path / "a.json")
    second = record(root, revision, tmp_path / "b.json")
    assert first["sha256"] == second["sha256"] and first["revision"] == revision
    value = json.loads((tmp_path / "a.json").read_text())
    assert value["revision"] == revision and value["tree"] == git(root, "rev-parse", "HEAD^{tree}")
    assert value["repository"] == "lukacf/meerkat" and value["product_root"] == "connect"
    assert {row["path"] for row in value["files"]} == {
        "SOURCE-PROVENANCE.md", "PUBLIC-SOURCE-COMMITS.json", "crates/meerkat-connect/src/main.rs",
        "scripts/run.sh", "scripts/connect-source-identity.py"}
    assert {row["path"]: row["git_mode"] for row in value["files"]}["scripts/run.sh"] == "100755"
    assert set(value["workspace"]) == {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"}
    result = check(root, tmp_path / "a.json", first["sha256"])
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"revision": revision, "tree": value["tree"], "status": "verified"}


def test_record_refuses_another_commit_and_an_existing_output(repo, tmp_path):
    root, revision = repo
    for commit in ("a" * 40, "main"):
        assert run(root, "record", "--commit", commit, "--export-commit", revision,
                   "--output", str(tmp_path / "x.json")).returncode != 0
    (tmp_path / "x.json").write_text("{}")
    assert run(root, "record", "--commit", revision, "--export-commit", revision,
               "--output", str(tmp_path / "x.json")).returncode != 0


@pytest.mark.parametrize("change", ["dirty_connect", "dirty_core", "untracked", "workspace_lock"])
def test_dirty_checkout_refuses_record_and_check(repo, tmp_path, change):
    root, revision = repo
    selected = record(root, revision, tmp_path / "m.json")
    target = {"dirty_connect": "connect/crates/meerkat-connect/src/main.rs",
              "dirty_core": "crates/meerkat-core/src/lib.rs",
              "untracked": "connect/crates/meerkat-connect/src/extra.rs",
              "workspace_lock": "Cargo.lock"}[change]
    (root / target).write_text("changed\n")
    result = check(root, tmp_path / "m.json", selected["sha256"])
    assert result.returncode != 0 and "dirty" in result.stderr
    assert run(root, "record", "--commit", revision, "--export-commit", revision,
               "--output", str(tmp_path / "n.json")).returncode != 0


def test_manifest_of_an_earlier_commit_refuses_on_a_later_checkout(repo, tmp_path):
    root, revision = repo
    selected = record(root, revision, tmp_path / "m.json")
    (root / "connect/crates/meerkat-connect/src/main.rs").write_text("fn main() { println!(); }\n")
    commit(root, "later Connect change")
    result = check(root, tmp_path / "m.json", selected["sha256"])
    assert result.returncode != 0 and "another commit" in result.stderr


@pytest.mark.parametrize("change", ["pin", "file_row", "workspace_row", "export_row", "export_commit", "extra_key",
                                    "repository", "reformatted", "revision_and_tree"])
def test_any_manifest_difference_refuses(repo, tmp_path, change):
    root, revision = repo
    selected = record(root, revision, tmp_path / "m.json")
    path = tmp_path / "m.json"
    value = json.loads(path.read_text())
    if change == "pin":
        sha = "0" * 64
    else:
        if change == "file_row":
            value["files"][0]["sha256"] = "0" * 64
        elif change == "workspace_row":
            value["workspace"]["Cargo.lock"]["size"] += 1
        elif change == "export_row":
            value["export"]["records"]["SOURCE-PROVENANCE.md"]["sha256"] = "1" * 64
        elif change == "export_commit":
            value["export"]["commit"] = "d" * 40
        elif change == "extra_key":
            value["note"] = "unselected"
        elif change == "repository":
            value["repository"] = "lukacf/meerkat-connect"
        elif change == "revision_and_tree":
            value["revision"], value["tree"] = "b" * 40, "c" * 40
        data = (json.dumps(value) if change == "reformatted" else json.dumps(value, indent=2, sort_keys=True)) + "\n"
        path.write_text(data)
        sha = hashlib.sha256(data.encode()).hexdigest()
    result = check(root, path, sha)
    assert result.returncode != 0, change


def test_connect_must_be_the_product_directory_of_its_checkout(repo, tmp_path):
    root, revision = repo
    nested = tmp_path / "standalone-connect"
    shutil.copytree(root / "connect", nested / "connect")
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"):
        (nested / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / name, nested / name)
    subprocess.run(["git", "init", "-q", str(nested / "connect")], check=True)
    other = commit(nested / "connect", "connect as its own repository")
    result = run(nested, "record", "--commit", other, "--export-commit", other, "--output", str(tmp_path / "s.json"))
    assert result.returncode != 0 and "connect/ product" in result.stderr


def test_tracked_symlink_refuses_the_census(repo, tmp_path):
    root, _ = repo
    os.symlink("main.rs", root / "connect/crates/meerkat-connect/src/alias.rs")
    revision = commit(root, "tracked link")
    result = run(root, "record", "--commit", revision, "--export-commit", revision, "--output", str(tmp_path / "l.json"))
    assert result.returncode != 0 and "regular tracked files" in result.stderr


def test_export_commit_must_be_in_the_selected_history(repo, tmp_path):
    root, imported = repo
    (root / "connect/crates/meerkat-connect/src/main.rs").write_text("fn main() { }\n")
    selected = commit(root, "monorepo change after the import")
    value = record(root, selected, tmp_path / "m.json", export=imported)
    assert json.loads((tmp_path / "m.json").read_text())["export"]["commit"] == imported
    assert check(root, tmp_path / "m.json", value["sha256"]).returncode == 0
    git(root, "checkout", "-q", "-b", "side", imported)
    (root / "connect/README.md").write_text("side branch\n")
    side = commit(root, "unrelated side commit")
    git(root, "checkout", "-q", selected)
    result = run(root, "record", "--commit", selected, "--export-commit", side, "--output", str(tmp_path / "n.json"))
    assert result.returncode != 0 and "history" in result.stderr
    result = run(root, "record", "--commit", selected, "--export-commit", "e" * 40, "--output", str(tmp_path / "o.json"))
    assert result.returncode != 0
