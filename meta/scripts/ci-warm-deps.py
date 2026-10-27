#!/usr/bin/env python3
"""Warm a pull request's Rust dependency builds from its own branch.

A pull request's CI restores caches saved on main only, so a Cargo.lock change
(a Meerkat Core bump) compiles its dependencies cold in every Rust job.
.github/workflows/warm-deps.yml, dispatched on the branch, runs `build GRAPH`
for each graph below: refuse dependencies from outside crates.io and any
[patch], blank every workspace target, run the graph's cargo commands, and pack
deps/, build/ and .fingerprint/ without the workspace's outputs, mtimes kept,
into an artifact (stored apart from the Actions cache and its budget). In each
Rust job of the branch's pull request (JOB_GRAPHS), `fetch` extracts the
artifact of exactly that job's name from the newest successful dispatch on the
head branch and prints one line however it ends; on a miss the job builds as
it always did. The name binds the graph, `rustc -vV`, Cargo.lock, the
workspace manifests and the environment that shapes dependency builds;
scripts/test_ci_warm_deps.py checks each job's environment against its graph.
"""

import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.parse
import zipfile
from pathlib import Path, PurePosixPath

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
WORKFLOW = ".github/workflows/warm-deps.yml"
# A dispatch run warms a pull request only from the head commit or one of its
# ancestors, and only if none of the files that build or fetch an artifact
# changed in between.
BUILDER_FILES = frozenset({WORKFLOW, "scripts/ci-warm-deps.py", ".github/actions/fetch-warm-deps/action.yml"})
TARBALL = "warm-deps.tar"
OUTPUTS = ("deps", "build", ".fingerprint")
LINE_TABLES = {"CARGO_PROFILE_DEV_DEBUG": "line-tables-only"}
META = ["-p", "meerkat-meta", "--locked"]
EXAMPLES = [arg for name in ("library_mode_reference", "console_acceptance_fixture", "memory_console_reference",
                             "workgraph_console_reference", "mcp_apps_gateway_fixture") for arg in ("--example", name)]
# Graph -> (the env its consumer jobs set, their cargo commands). Each command
# selects the packages, features and target kinds a consumer's does, so the
# same dependency units build: dev-dependencies only beside test, example or
# bench targets. Commands make, npm and examples/001 run are copied here.
GRAPHS = {
    "clippy": ({}, [["clippy", "--workspace", "--all-targets", "--locked"]]),
    "test": ({"CARGO_PROFILE_DEV_DEBUG": "0"}, [
        ["test", "--workspace", "--locked", "--lib", "--bins", "--tests", "--no-run"],
        ["build", "--workspace", "--locked", "--examples"]]),
    "voice-test": (LINE_TABLES, [["test", *META, "--features", "openai-live", "--lib", "--bins", "--tests",
                                  "--no-run"]]),
    "voice-clippy": (LINE_TABLES, [["clippy", *META, "--features", "openai-live", "--lib", "--bin", "rpc_gateway",
                                    "--bin", "meta_gateway"]]),
    "extension-state-test": (LINE_TABLES, [
        ["test", "-p", "meta-extension-state", "--locked", "--no-run"],
        ["test", *META, "--features", "extension-state", "--lib", "--test", "extension_state", "--no-run"]]),
    "extension-state-lint": (LINE_TABLES, [
        ["check", *META, "--no-default-features"],
        ["clippy", *META, "--features", "extension-state", "--lib", "--test", "extension_state"]]),
    "dev-bins": (LINE_TABLES, [["build", *META, "--bin", "rpc_gateway", "--bin", "meta_flow_editor"]]),
    "dev-examples": (LINE_TABLES, [
        ["build", *META, *EXAMPLES],
        ["build", *META, "--example", "incident_command_center", "--bin", "mcp_fixture"],
        ["test", *META, "--example", "console_acceptance_fixture", "--no-run"],
        ["test", *META, "--test", "console_customization_fixtures", "--no-run"]]),
}
# ci.yml job -> the graph it extracts.
JOB_GRAPHS = {
    "clippy": "clippy", "test": "test", "test-integration": "test",
    "test-voice": "voice-test", "voice-clippy": "voice-clippy",
    "test-extension-state": "extension-state-test", "extension-state-lint": "extension-state-lint",
    "test-python": "dev-bins", "flow-editor": "dev-bins",
    "console": "dev-examples", "console-browser": "dev-examples", "console-fixtures": "dev-examples",
    "console-acceptance": "dev-examples",
}


def debug_dir() -> Path:
    return Path(os.environ["CARGO_TARGET_DIR"]) / "debug"


def members() -> list[dict]:
    meta = json.loads(subprocess.run(["scripts/repo-cargo", "metadata", "--no-deps", "--format-version", "1"],
                                     check=True, capture_output=True, text=True).stdout)
    return [p for p in meta["packages"] if p["id"] in set(meta["workspace_members"])]


def artifact_name(graph: str) -> str:
    env = sorted((k, v) for k, v in os.environ.items() if v and (
        k in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET",
              "CARGO_TARGET_DIR") or k.startswith("CARGO_PROFILE_")))
    rustc = subprocess.run(["rustc", "-vV"], check=True, capture_output=True, text=True).stdout
    release = re.search(r"(?m)^release: (\S+)$", rustc)[1]
    lock = hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest()
    manifests = [hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in [Path("Cargo.toml"), *sorted(Path("crates").glob("*/Cargo.toml"))]]
    build_env = hashlib.sha256(json.dumps([rustc, GRAPHS[graph][1], manifests, env]).encode()).hexdigest()
    return f"warm-deps-{graph}-{release}-{lock}-{build_env[:16]}"


def refuse_foreign_sources(packages: list[dict], root: Path = Path(".")) -> None:
    names = {p["name"] for p in packages}
    for entry in tomllib.loads((root / "Cargo.lock").read_text()).get("package", []):
        source = entry.get("source")
        if source != CRATES_IO and not (source is None and entry["name"] in names):
            sys.exit(f"refused: {entry['name']} {entry['version']} comes from {source or 'a path'}")
    for manifest in [root / "Cargo.toml", *(root / p["manifest_path"] for p in packages)]:
        if {"patch", "replace"} & set(tomllib.loads(manifest.read_text())):
            sys.exit(f"refused: {manifest} patches or replaces dependencies")
    for config in (root / ".cargo/config.toml", root / ".cargo/config"):
        if config.exists() and {"patch", "source", "paths"} & set(tomllib.loads(config.read_text())):
            sys.exit(f"refused: {config} redirects dependency sources")


def workspace_output(name: str, packages: list[dict]) -> bool:
    """Whether a deps/, build/ or .fingerprint/ entry is a workspace target's."""
    names = set()
    for package in packages:
        names |= {package["name"], package["name"].replace("-", "_")}
        names |= {target["name"].replace("-", "_") for target in package["targets"]}
    match = re.match(r"(?:lib)?(.+?)-[0-9a-f]{16}(?:\..+)?$", name)
    return bool(match) and match[1] in names


def build(graph: str) -> str:
    env, commands = GRAPHS[graph]
    os.environ.update(env)
    packages = members()
    refuse_foreign_sources(packages)
    name = artifact_name(graph)
    for package in packages:
        for target in package["targets"]:
            library = {"lib", "rlib", "proc-macro"} & set(target["kind"])
            Path(target["src_path"]).write_text("" if library else "fn main() {}\n")
    for command in commands:
        subprocess.run(["scripts/repo-cargo", *command], check=True)
    left_out = pack(debug_dir(), packages, Path(TARBALL))
    print(f"packed {Path(TARBALL).stat().st_size} bytes; left out {len(left_out)} symlinks or special files")
    return name


def pack(debug: Path, packages: list[dict], tarball: Path) -> list[str]:
    """Pack the dependency outputs under `debug` into `tarball`, mtimes kept,
    and return the entries left out. Every name is a plain file: cargo
    hard-links each build script binary, and a fingerprint whose binary is
    missing rebuilds the script and everything that depends on its crate."""
    left_out = []
    with tarfile.open(tarball, "w", format=tarfile.PAX_FORMAT) as tar:
        for output in OUTPUTS:
            for entry in sorted((debug / output).iterdir()):
                if workspace_output(entry.name, packages):
                    continue
                for path in [entry, *sorted(entry.rglob("*"))] if entry.is_dir() else [entry]:
                    info = tar.gettarinfo(path, f"{output}/{path.relative_to(debug / output)}")
                    if info.islnk():
                        info.type, info.linkname, info.size = tarfile.REGTYPE, "", path.stat().st_size
                    if info.isfile():
                        with path.open("rb") as data:
                            tar.addfile(info, data)
                    elif info.isdir():
                        tar.addfile(info)
                    else:
                        left_out.append(info.name)
    return left_out


def gh(path: str, stdout=subprocess.PIPE) -> bytes:
    return subprocess.run(["gh", "api", path], check=True, stdout=stdout, stderr=subprocess.PIPE, timeout=240).stdout


def refusal(member: tarfile.TarInfo) -> str | None:
    parts = PurePosixPath(member.name).parts
    if member.name.startswith("/") or ".." in parts or not parts or parts[0] not in OUTPUTS:
        return f"entry {member.name} leaves deps/, build/ and .fingerprint/"
    if not (member.isfile() or member.isdir()):
        return f"entry {member.name} is not a plain file or directory"
    return None


def extract(archive: Path) -> str | None:
    """Extract the artifact's tarball into the target dir, member by member,
    stopping at the first entry that is not a plain file or directory under
    deps/, build/ or .fingerprint/. Returns why it stopped, or None."""
    with zipfile.ZipFile(archive) as bundle:
        if bundle.namelist() != [TARBALL]:
            return f"the artifact holds {bundle.namelist()[:3]}, not {TARBALL} alone"
        with bundle.open(TARBALL) as stream, tarfile.open(fileobj=stream, mode="r|") as tar:
            for member in tar:
                why = refusal(member)
                if why:
                    return why
                tar.extract(member, debug_dir(), filter="data")
    return None


def lineage_refusal(repo: str, run_sha: str, head_sha: str) -> str | None:
    """Why a dispatch run's commit may not warm the pull request head, or None."""
    compare = json.loads(gh(f"repos/{repo}/compare/{run_sha}...{head_sha}"))
    if compare["status"] not in ("identical", "ahead"):
        return f"its commit {run_sha[:12]} is not the head or an ancestor ({compare['status']})"
    files = compare.get("files", [])
    if len(files) >= 300:
        return f"{len(files)} files changed since {run_sha[:12]}, too many to check the builder files"
    changed = sorted(BUILDER_FILES & {name for f in files for name in (f["filename"], f.get("previous_filename"))})
    return f"{', '.join(changed)} changed since {run_sha[:12]}" if changed else None


def fetch() -> str:
    if os.environ.get("GITHUB_EVENT_NAME") != "pull_request":
        return "not a pull request"
    repo, head, job = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_HEAD_REF"], os.environ["GITHUB_JOB"]
    if os.environ.get("HEAD_REPOSITORY") != repo:
        return "a fork's pull request"
    if job not in JOB_GRAPHS:
        return f"job {job} has no graph"
    name, head_sha, skipped = artifact_name(JOB_GRAPHS[job]), os.environ["PR_HEAD_SHA"], ""
    query = f"event=workflow_dispatch&status=success&per_page=20&branch={urllib.parse.quote(head, safe='')}"
    runs = json.loads(gh(f"repos/{repo}/actions/workflows/{Path(WORKFLOW).name}/runs?{query}"))
    for run in runs["workflow_runs"]:
        if (run["event"], run["head_branch"], run["path"].split("@")[0], run["head_repository"]["full_name"]) \
                != ("workflow_dispatch", head, WORKFLOW, repo):
            continue
        listed = json.loads(gh(f"repos/{repo}/actions/runs/{run['id']}/artifacts?name={name}"))
        found = [a for a in listed["artifacts"] if a["name"] == name and not a["expired"]]
        if not found:
            continue
        why = lineage_refusal(repo, run["head_sha"], head_sha)
        if why:
            skipped = f"; skipped run {run['id']}: {why}"
            continue
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / "artifact.zip"
            with archive.open("wb") as out:
                gh(f"repos/{repo}/actions/artifacts/{found[0]['id']}/zip", stdout=out)
            why = extract(archive)
        return f"refused {name} from run {run['id']}: {why}" if why else f"extracted {name} from run {run['id']}"
    return f"no {name} from a successful warm-deps dispatch on {head}{skipped}"


def main(argv: list[str]) -> int:
    if argv[:1] == ["build"] and len(argv) == 2:
        name = build(argv[1])
        print(f"name={name}")
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as out:
            out.write(f"name={name}\n")
        return 0
    if argv != ["fetch"]:
        sys.exit("usage: ci-warm-deps.py build GRAPH | fetch")
    try:
        outcome = fetch()
    except Exception as error:  # any failure falls back to the job's own build
        outcome = f"none ({type(error).__name__}: {error})"
    print(f"warm dependencies: {outcome}".splitlines()[0])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
