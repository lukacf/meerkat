#!/usr/bin/env python3
"""Decide which CI lanes a run needs.

Each lane is a group of jobs in .github/workflows/ci.yml that read the same
inputs (see LANE_JOBS). Pushes, dispatches and an empty or unreadable diff run
every lane, so main's run, the evidence release.yml requires before a tag,
always covers every job. A pull request runs only the lanes its changed files
reach and do not leave to the push:

- docs, top-level markdown, the non-CI workflows and the files only fmt-lint
  and audit read (see `skippable`) reach no lane;
- the classifier and its test reach every lane;
- the other top-level scripts reach the console lane, whose fixture test greps
  every script for port reservations;
- a file under one of the AREAS prefixes or in AREA_FILES reaches exactly
  those lanes;
- Rust sources and build configuration (RUST_INPUTS) reach every lane but
  console-perf, which bundles only console/ and packages/console-*;
- any other file, a change to ci.yml itself (the pull request must prove its
  own CI) and the files every Rust job runs (the repository's cargo wrapper
  and the fetch of warmed dependency builds) reach every lane.

A pull request runs the rust and integration lanes for a Rust change and
leaves the lanes that compile the same crate in another configuration (the
voice and extension-state features, the Python SDK gateway, the console
backends, the flow editor binary) to the push, unless the change is one of
their own inputs (see `deferred_lanes`): code a feature gates, a feature's own
module or crate, a backend module the lane drives end to end, a file such an
input embeds or loads (LANE_INPUTS), or the build configuration, which every
lane reads. A deferred lane still gates the release, because release.yml
refuses a tag until the push run of the exact commit on main is green. The
classifier prints the lanes a pull request deferred.

An area may list fewer lanes only when no job outside them reads its files.
scripts/test_ci_changes.py scans the repository's sources, manifests and
configuration for references into a lane-limited or skippable area or into
the crates (paths, $ROOT/ paths, bare area names, joined path parts, globs
cut at their first wildcard) and fails on any reading area it has not
audited, or whose lanes the read area would not reach, or whose lanes a pull
request changing the read crate file would defer, so a new cross-area read
cannot silently skip the job it affects. It also checks that FEATURE_PATHS
covers every module a feature gates, and that a file a lane's own input
embeds or loads runs that lane.

ci.yml runs the base commit's copy of this script, so a pull request never
classifies itself with the classifier it changes.

Usage: ci-changes.py --event NAME [--github-output FILE] [PATH ...]
Paths default to `git diff --no-renames --name-only -z HEAD^1 HEAD`, the
pull request's own changes when HEAD is GitHub's merge commit for it. A
rename counts as both of its paths, so moving a file the jobs read into
docs/ still runs them. Prints and appends `<lane>=true|false` for every lane,
in one write, to FILE.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path
from typing import Callable

# The lanes, each named by the `changes` output its jobs test in ci.yml.
LANE_JOBS = {
    "rust": ("clippy", "test"),
    "integration": ("test-integration",),
    "voice": ("test-voice", "voice-clippy"),
    "extension_state": ("test-extension-state", "extension-state-lint"),
    "python": ("test-python",),
    "console": ("console", "console-browser", "console-fixtures", "console-acceptance"),
    "console_perf": ("console-perf",),
    "flow_editor": ("flow-editor",),
}
LANES = tuple(LANE_JOBS)
EVERY_LANE = frozenset(LANES)
CONSOLE_LANES = frozenset({"console", "console_perf", "flow_editor"})

# Files nothing in the Rust, console, SDK or example jobs reads. docs/rct is
# not here: tests/console_route_auth.rs and the governance_check binary read
# it. Nor is .claude: src/mobpack.rs reads .claude/skills. Nor is any markdown
# below the top level: crates embed prompt files with include_str!.
SKIPPABLE_FILES = frozenset(
    {
        ".github/CODEOWNERS",
        ".github/release.yml",
        ".github/workflows/release.yml",
        ".github/workflows/e2e-live.yml",
        ".github/workflows/publish-docs.yml",
        # fmt-lint, which every run includes, reads these and no lane job
        # does: the hook contract test, cargo package's file list, and the
        # generated Bazel files' freshness check (no CI or release job
        # builds with Bazel; every BUILD.bazel is skippable for the same
        # reason, see `skippable`).
        ".pre-commit-config.yaml",
        ".gitignore",
        "MODULE.bazel",
        ".bazelrc",
        ".bazelversion",
        # cargo-deny's policy, read by the audit job every run includes.
        "deny.toml",
    }
)
# tools/continuity-export holds standalone scripts whose tests fmt-lint runs;
# no lane job reads them.
SKIPPABLE_PREFIXES = ("docs/", "tools/bazel/", "tools/continuity-export/")

# Prefix -> the lanes a change below it can reach. The first match decides,
# so a narrower prefix comes before the area that contains it.
AREAS = (
    # tests/console_customization_fixtures.rs (an integration test the
    # console-fixtures job also runs) reads these, beside the console's own
    # harnesses and the flow editor's browser tests.
    ("console/fixtures/", CONSOLE_LANES | {"integration"}),
    # The console app and its scenario harnesses. The flow editor's browser
    # tests load console files too. The Rust crates embed only the built
    # bundle, crates/meerkat-meta/console-dist.
    ("console/", CONSOLE_LANES),
    # The console and flow editor component packages, which the console,
    # console-perf's bundle and the flow editor import.
    ("packages/", CONSOLE_LANES),
    # The flow editor app; the crates embed only crates/meerkat-meta/flow-editor-dist.
    # console/fixture-ready.test.cjs greps it for port reservations.
    ("flow-editor/", frozenset({"flow_editor", "console"})),
    # The Python SDK tests, the Rust integration tests that run its parity
    # and reference scripts, and the console fixture test's grep.
    ("sdk/python/", frozenset({"python", "integration", "console"})),
    # The Rust integration tests validate the TypeScript SDK, and the console
    # imports its live module. test-typescript runs on every pull request.
    ("sdk/typescript/", frozenset({"integration", "console", "console_perf", "flow_editor"})),
    # Out-of-tree examples of meerkat-meta (clippy and the integration
    # lane's example build compile them; an integration test reads the
    # incident pack) and the console packs console-acceptance runs and the
    # console fixture test greps.
    ("examples/", frozenset({"rust", "integration", "console"})),
    # A Forge test that pins the shutdown horizon the RPC gateway source
    # advertises. Forge's own workflow runs it and no lane here does; like
    # every other installer path, a change to it reaches every lane. Named
    # here so scripts/test_ci_changes.py audits its read of that source.
    ("installer/tests/test_gateway_shutdown_horizon.py", EVERY_LANE),
)

# Single files -> the lanes a change to them can reach. They take precedence
# over the rules for their directory.
AREA_FILES = {
    # test-python runs `make test-python`, and the flow editor's source
    # contract test (test:controller) reads its targets.
    "Makefile": frozenset({"python", "flow_editor"}),
    # Only the nextest jobs read it; the feature jobs run cargo test.
    ".config/nextest.toml": frozenset({"rust", "integration"}),
    # The memory eval harness integration tests run it; the console fixture
    # test greps it like every script.
    "scripts/memory-evals": frozenset({"integration", "console"}),
}

# Rust sources and build configuration reach every job that compiles Rust,
# which is every lane but console-perf.
RUST_INPUTS = ("crates/", ".cargo/")
RUST_INPUT_FILES = frozenset({"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"})
RUST_LANES = EVERY_LANE - {"console_perf"}

# The lanes a pull request leaves to the push when a Rust change reaches
# them: each compiles meerkat-meta again in another configuration, after the
# rust and integration lanes have compiled and tested the default one.
RUST_DEFERRED_LANES = frozenset({"voice", "extension_state", "python", "console", "flow_editor"})

# Build configuration every lane's build reads: a dependency or toolchain
# change (a Meerkat Core bump, say) can break any end-to-end suite, so a pull
# request that changes one runs every lane.
BUILD_CONFIGURATION_FILES = RUST_INPUT_FILES | {"crates/meerkat-meta/Cargo.toml"}
BUILD_CONFIGURATION_PREFIXES = (".cargo/",)

# Feature lane -> the cfg marker of code compiled only with its feature, and
# the paths that are its own inputs: modules the feature gates whole (a file
# declared under the cfg need not repeat it), or a crate only the feature
# builds. scripts/test_ci_changes.py checks that every module a feature gates
# lies under one of its paths.
FEATURE_GATES = {
    "voice": re.compile(r'feature\s*=\s*"openai-live"'),
    "extension_state": re.compile(r'feature\s*=\s*"extension-state"'),
}
FEATURE_PATHS = {
    # The transcript fixture the voice modules include_str!, and the runtime
    # store forwarding probe, a test module only openai-live builds compile.
    "voice": (
        "crates/meerkat-meta/src/console_voice/",
        "crates/meerkat-meta/tests/fixtures/console_voice_v1.json",
        "crates/meerkat-meta/src/runtime_store_forwarding_probe",
    ),
    "extension_state": ("crates/meta-extension-state/", "crates/meerkat-meta/src/extension_state"),
}

# Rust inputs -> the deferred lanes that drive them end to end or read them,
# beyond what FEATURE_PATHS lists: a pull request that changes one runs those
# lanes. Exact files and directories only. scripts/test_ci_changes.py checks
# that a file one of these (or feature-gated code) embeds or loads, and a
# crates/ file a console, flow editor or SDK test reads, runs the same lanes.
LANE_INPUTS = (
    # The bundles the backends embed and the browser lanes serve.
    ("crates/meerkat-meta/console-dist/", {"console"}),
    ("crates/meerkat-meta/flow-editor-dist/", {"flow_editor"}),
    # The console backends, and the HTTP console and aggregator they serve.
    ("crates/meerkat-meta/examples/", {"console"}),
    ("crates/meerkat-meta/src/http_console.rs", {"console"}),
    ("crates/meerkat-meta/src/http_console/", {"console"}),
    ("crates/meerkat-meta/src/console_aggregator/", {"console"}),
    # The flow editor binary, its HTTP surface and the mobpack authoring it drives.
    ("crates/meerkat-meta/src/bin/meta_flow_editor.rs", {"flow_editor"}),
    ("crates/meerkat-meta/src/http_flow_editor.rs", {"flow_editor"}),
    ("crates/meerkat-meta/src/mobpack.rs", {"flow_editor"}),
    # The RPC gateway the Python SDK tests drive. The flow editor's source
    # contract test reads rpc.rs too.
    ("crates/meerkat-meta/src/bin/rpc_gateway.rs", {"python"}),
    ("crates/meerkat-meta/src/rpc_gateway.rs", {"python", "extension_state"}),
    ("crates/meerkat-meta/src/rpc.rs", {"python", "flow_editor"}),
    ("crates/meerkat-meta/src/rpc/", {"python"}),
    # Other sources the console and flow editor contract tests read.
    ("crates/meerkat-meta/src/mcp_apps_sandbox.rs", {"console"}),
    ("crates/meerkat-meta/src/rpc/topology_methods.rs", {"console"}),
    ("crates/meerkat-meta/src/rpc/workgraph_methods.rs", {"console"}),
    ("crates/meerkat-meta/src/unified_runtime/http.rs", {"flow_editor"}),
    # Data the inputs above or feature-gated code embed or load, and the
    # fixtures the console's voice session tests and the Python SDK tests read.
    ("crates/meerkat-meta/assets/release-targets.json", {"console"}),
    ("crates/meerkat-meta/src/adaptive_layer_decision.schema.json", {"flow_editor"}),
    ("crates/meerkat-meta/tests/fixtures/console_voice_v1.json", {"console"}),
    ("crates/meerkat-meta/tests/fixtures/application_tool_policies_init_params.json", {"python"}),
    ("crates/meerkat-meta/tests/fixtures/live_contracts_v1.json", {"python"}),
    ("crates/meerkat-meta/tests/fixtures/role_migrations_init_params.json", {"extension_state", "python"}),
    ("crates/meerkat-meta/tests/fixtures/security_idempotency/security-head-evolution.json",
     {"voice", "extension_state"}),
    # The mob announcement process tests run in the extension-state jobs and
    # drive the announcement module and keyed inject end to end.
    ("crates/meerkat-meta/src/mob_announcements/", {"extension_state"}),
    ("crates/meerkat-meta/src/runtime/keyed_inject.rs", {"extension_state"}),
    # The control channel the keyed inject verbs ride, the runtime and builder
    # that compose its plane, and the identity runtime it delivers to.
    ("crates/meerkat-meta/src/runtime/cross_mob_control.rs", {"extension_state"}),
    ("crates/meerkat-meta/src/runtime/cross_mob_remote.rs", {"extension_state"}),
    ("crates/meerkat-meta/src/unified_runtime/cross_mob.rs", {"extension_state"}),
    ("crates/meerkat-meta/src/unified_runtime/builder.rs", {"extension_state"}),
    ("crates/meerkat-meta/src/identity_first/runtime.rs", {"extension_state"}),
    ("crates/meerkat-meta/tests/mob_announcements_delivery_process.rs", {"extension_state"}),
    ("crates/meerkat-meta/tests/mob_announcements_remote_process.rs", {"extension_state"}),
)

# The classifier and its contract decide every other path's lanes.
CLASSIFIER_FILES = frozenset({"scripts/ci-changes.py", "scripts/test_ci_changes.py"})
# Every Rust job builds through the wrapper and, on a pull request, first
# fetches the dependency builds warm-deps.yml warmed for its branch.
RUST_JOB_FILES = frozenset({"scripts/repo-cargo", "scripts/ci-warm-deps.py",
                            ".github/actions/fetch-warm-deps/action.yml"})


def top_level_script(path: str) -> bool:
    return path.startswith("scripts/") and path.count("/") == 1 and path not in RUST_JOB_FILES


def skippable(path: str) -> bool:
    if path.startswith("docs/rct/"):
        return False
    if path.startswith(SKIPPABLE_PREFIXES) or path.rpartition("/")[2] == "BUILD.bazel":
        return True
    if "/" not in path and (path.endswith(".md") or path.startswith("LICENSE-")):
        return True
    return path in SKIPPABLE_FILES


def rust_input(path: str) -> bool:
    return path in RUST_INPUT_FILES or path.startswith(RUST_INPUTS)


def path_lanes(path: str) -> frozenset:
    """The lanes a change to this one path can reach."""
    if path in CLASSIFIER_FILES or path in RUST_JOB_FILES:
        return EVERY_LANE
    if path in AREA_FILES:
        return AREA_FILES[path]
    if top_level_script(path):
        # fmt-lint runs them; console/fixture-ready.test.cjs greps them.
        return frozenset({"console"})
    if skippable(path):
        return frozenset()
    if rust_input(path):
        return RUST_LANES
    for prefix, lanes in AREAS:
        if path.startswith(prefix):
            return lanes
    return EVERY_LANE


def read_changed_file(path: str) -> str | None:
    """A changed file's text in the checked-out tree, or in the base commit
    when the change deleted it. None when neither can be read."""
    try:
        return Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        pass
    try:
        return subprocess.run(
            ["git", "show", f"HEAD^1:{path}"], check=True, capture_output=True, text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError, UnicodeDecodeError):
        return None


def deferred_lanes(path: str, read: Callable[[str], str | None] = read_changed_file) -> frozenset:
    """The lanes this path reaches that a pull request leaves to the push."""
    if not rust_input(path):
        return frozenset()
    if path in BUILD_CONFIGURATION_FILES or path.startswith(BUILD_CONFIGURATION_PREFIXES):
        return frozenset()
    deferred = set(RUST_DEFERRED_LANES)
    for lane, prefixes in FEATURE_PATHS.items():
        if path.startswith(prefixes):
            deferred.discard(lane)
    for prefix, lanes in LANE_INPUTS:
        if path.startswith(prefix):
            deferred -= lanes
    if path.endswith(".rs"):
        text = read(path)
        for lane, gate in FEATURE_GATES.items():
            # Unreadable text runs the lane rather than guess.
            if text is None or gate.search(text):
                deferred.discard(lane)
    return frozenset(deferred)


def classify(event: str, paths: list[str],
             read: Callable[[str], str | None] = read_changed_file) -> tuple[frozenset, frozenset]:
    """(the lanes this run needs, the lanes a pull request leaves to the push)."""
    if event != "pull_request" or not paths:
        return EVERY_LANE, frozenset()
    lanes = frozenset()
    reached = frozenset()
    for path in paths:
        reach = path_lanes(path)
        reached |= reach
        lanes |= reach - deferred_lanes(path, read)
    return lanes, reached - lanes


def needed_lanes(event: str, paths: list[str],
                 read: Callable[[str], str | None] = read_changed_file) -> frozenset:
    return classify(event, paths, read)[0]


def needs_full_run(event: str, paths: list[str],
                   read: Callable[[str], str | None] = read_changed_file) -> bool:
    return needed_lanes(event, paths, read) == EVERY_LANE


def output_lines(lanes: frozenset) -> list[str]:
    return [f"{lane}={'true' if lane in lanes else 'false'}" for lane in LANES]


def changed_paths() -> list[str]:
    # With rename detection git lists only a rename's destination, which
    # hides the removal of its source. -z keeps unusual names unquoted.
    result = subprocess.run(
        ["git", "diff", "--no-renames", "--name-only", "-z", "HEAD^1", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return [path for path in result.stdout.split("\0") if path]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--event", required=True)
    parser.add_argument("--github-output")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args(argv)
    paths = args.paths
    if not paths and args.event == "pull_request":
        try:
            paths = changed_paths()
        except (OSError, subprocess.CalledProcessError, UnicodeDecodeError) as error:
            print(f"cannot list changed files ({error}); running every lane")
            paths = []
    lanes, deferred = classify(args.event, paths)
    for path in paths:
        reached = path_lanes(path)
        print(f"{' '.join(lane for lane in LANES if lane in reached) or 'no lane'}: {path}")
    if deferred:
        print(f"left to the push: {' '.join(lane for lane in LANES if lane in deferred)}")
    lines = output_lines(lanes)
    print("\n".join(lines))
    if args.github_output:
        with open(args.github_output, "a", encoding="utf-8") as out:
            out.write("".join(line + "\n" for line in lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
