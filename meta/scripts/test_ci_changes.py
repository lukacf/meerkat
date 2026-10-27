#!/usr/bin/env python3
"""Contracts for scripts/ci-changes.py, which runs on a pull request only
the CI lanes its changed files can reach."""

import contextlib
import importlib.util
import io
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-changes.py"
SPEC = importlib.util.spec_from_file_location("ci_changes", SCRIPT)
ci_changes = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci_changes)


def classify_commit(base: dict[str, str], head: dict[str, str]) -> str:
    """Run ci-changes.py as CI does, in a new repository whose HEAD commit
    turns the `base` tree into the `head` tree ({path: text}), so git
    itself lists the changed paths. Returns the GITHUB_OUTPUT text."""
    # A git hook exports GIT_DIR and GIT_INDEX_FILE; neither they nor the
    # caller's git configuration may reach the scratch repository.
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
    env.update(GIT_AUTHOR_NAME="ci", GIT_AUTHOR_EMAIL="ci@example.invalid")
    env.update(GIT_COMMITTER_NAME="ci", GIT_COMMITTER_EMAIL="ci@example.invalid")
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        output = Path(tmp) / "output"

        def git(*args: str) -> None:
            subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True)

        git("init", "-q")
        for tree in (base, head):
            for path in set(base) - set(tree):
                (repo / path).unlink()
            for path, text in tree.items():
                (repo / path).parent.mkdir(parents=True, exist_ok=True)
                (repo / path).write_text(text, encoding="utf-8")
            git("add", "-A")
            git("commit", "-q", "-m", "tree")
        subprocess.run(
            [sys.executable, str(SCRIPT), "--event", "pull_request", "--github-output", str(output)],
            cwd=repo,
            env=env,
            check=True,
            capture_output=True,
        )
        return output.read_text(encoding="utf-8")


def expected_output(*lanes: str) -> str:
    return "".join(f"{lane}={'true' if lane in lanes else 'false'}\n" for lane in ci_changes.LANES)


EVERY_LANE = set(ci_changes.LANES)
CONSOLE_LANES = {"console", "console_perf", "flow_editor"}
RUST_LANES = EVERY_LANE - {"console_perf"}
# What a pull request runs for a Rust change that is no feature's own input.
RUST_PR_LANES = {"rust", "integration"}
FEATURE_LANES = set(ci_changes.FEATURE_GATES)


def no_gates(path: str) -> str:
    """A changed file's text without any feature gate."""
    return "pub fn changed() {}\n"


# Where a file may name a path in a lane-limited or skippable area, or in the
# crates (a pull request can defer lanes past a crate file): a path below
# that area, repository-relative, reached with ../, or under a shell variable
# ($ROOT/, ${repo_root}/); a glob, cut at its first wildcard; or the bare area
# name as a quoted string (git grep and similar path arguments). Rust
# comments are skipped: they document, they do not read. Bare names are not
# read from Rust, where words like "console" name modes and routes.
AREA_NAMES = ("sdk/python", "sdk/typescript", "flow-editor", "console", "sdk", "scripts", "docs",
              "examples", "packages", "tools/bazel", "crates")
# Skippable files, and single files with lanes of their own (AREA_FILES),
# whose readers the scan audits too.
SKIPPABLE_NAMES = ("README.md", "CHANGELOG.md", "CONTRIBUTING.md", "CLAUDE.md", "LICENSE-MIT",
                   "LICENSE-APACHE", ".github/CODEOWNERS", ".github/release.yml",
                   ".github/workflows/release.yml", ".github/workflows/e2e-live.yml",
                   ".github/workflows/publish-docs.yml", ".pre-commit-config.yaml", ".gitignore",
                   "BUILD.bazel", "MODULE.bazel", ".bazelrc", ".bazelversion", "deny.toml",
                   "Makefile", ".config/nextest.toml")
_NAMES = "|".join(re.escape(name) for name in sorted(AREA_NAMES, key=len, reverse=True))
_FILES = "|".join(re.escape(name) for name in SKIPPABLE_NAMES)
_REST = r"(/[^\s\"'`),;]*)?"
_ROOTED = r"(?:\.\./)+|\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/"
PATH_REFERENCE = re.compile(
    # ../console/x, $ROOT/console, ${root}/sdk: a path whatever follows.
    rf"(?:{_ROOTED})({_NAMES})(?![\w-]){_REST}"
    # console/x relative to the repository root: needs the slash.
    rf"|(?<![\w./@$-])({_NAMES})(/[^\s\"'`),;]+)"
    # A skippable file, rooted or repository-relative.
    rf"|(?:{_ROOTED}|(?<![\w./@$-]))({_FILES})(?![\w.-])"
)
# A bare area name counts inside a bracketed or parenthesized list of quoted
# strings that names at least two areas (a git grep pathspec list, say), and
# a path joined part by part ("scripts", "repo-cargo" or "crates" / "x" /
# "fixture.json") counts as the joined path. A quoted name that is an object
# key or value (id: "docs", "scripts": {...}) is an identifier, not a path.
LIST_START = re.compile(r"[\[(]")
QUOTED_ITEM = re.compile(r"\s*[\"'`]([^\"'`\n]*)[\"'`]")
QUOTED = re.compile(r"[\"'`]([^\"'`\n]*)[\"'`]")
JOINED = re.compile(rf"[\"'`]({_NAMES})[\"'`]((?:\s*[,/]\s*[\"'`][\w.-]+[\"'`])+)")

COMMENT_LINE = re.compile(r"^\s*(?://|#|/\*|\*|--|<!--)")
OBJECT_VALUE = re.compile(r"\w+[\"']?\s*:\s*[\"'`]$")
BLOCKS = (re.compile(r'"""[\s\S]*?"""'), re.compile(r"'''[\s\S]*?'''"), re.compile(r"/\*[\s\S]*?\*/"))
SCANNED_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".cjs", ".mjs", ".py", ".sh", ".json", ".toml",
                    ".yml", ".yaml"}
SCANNED_NAMES = {"Makefile"}
UNSCANNED_DIRS = {"node_modules", "dist", ".tmp", "target", ".git", "__pycache__", "console-dist",
                  "flow-editor-dist", "output", ".venv", "docs", ".claude", ".codemob-mcp", "fixtures"}
MAX_SCANNED_BYTES = 1 << 20
WILDCARD = re.compile(r"[*?\[{]")

# Every place one area's files read files that changes there might not
# reach, and the lanes that must run when the read files change because
# the reading file runs in them. A pair missing here is a new cross-area
# read: decide its lanes in ci-changes.py and add it.
CROSS_AREA_READS = {
    # The console app imports sdk/typescript/src/live; console-perf bundles
    # the console sources and the flow editor's browser tests load them.
    ("console/", "sdk/typescript/"): CONSOLE_LANES,
    # console/fixture-ready.test.cjs (the console job) greps the flow
    # editor, the SDKs and the scripts for port reservations.
    ("console/", "flow-editor/"): {"console"},
    ("console/", "sdk/"): {"console"},
    ("console/", "sdk/python/"): {"console"},
    ("console/", "scripts/"): {"console"},
    # Rust integration tests run the Python SDK's parity and reference
    # scripts and read its sources.
    ("crates/*/tests/", "sdk/python/"): {"integration"},
    # Rust integration tests validate the TypeScript SDK and read its types.
    ("crates/*/tests/", "sdk/typescript/"): {"integration"},
    # The MCP Apps sandbox tests (src/mcp_apps_sandbox.rs and
    # tests/meta_gateway_bootstrap.rs) compare the console's sandbox sources
    # with the copies the crate embeds from console-dist. A console change
    # that leaves console-dist behind fails embedded:freshness in the console
    # lane, and one that updates console-dist changes crates/ and so runs the
    # rust and integration lanes these tests run in, so these reads need no
    # lane of their own.
    ("crates/*/src/", "console/"): set(),
    ("crates/*/tests/", "console/"): set(),
    # examples/001 runs in console-acceptance and loads console sources; the
    # other packs name the console or the TypeScript SDK for manual runs.
    ("examples/", "console/"): {"console"},
    ("examples/", "sdk/typescript/"): {"console"},
    # The flow editor's browser tests start console fixtures.
    ("flow-editor/", "console/"): {"flow_editor"},
    # The flow editor imports its component packages, and its source
    # contract test (test:controller) reads the Makefile's targets.
    ("flow-editor/", "packages/"): {"flow_editor"},
    ("flow-editor/", "Makefile"): {"flow_editor"},
    # The console component packages' tests import console sources; the
    # console job runs them.
    ("packages/", "console/"): {"console"},
    # The console and console-perf's bundle import the component packages;
    # the console's own tests read its fixtures; fixture-ready greps
    # examples/.
    ("console/", "packages/"): {"console", "console_perf"},
    ("console/", "console/fixtures/"): {"console"},
    ("console/", "examples/"): {"console"},
    # meerkat-meta's manifest declares the out-of-tree examples that clippy,
    # the integration lane's example build and console-acceptance compile.
    ("crates/*/Cargo.toml", "examples/"): {"rust", "integration", "console"},
    # tests/console_customization_fixtures.rs runs in the integration lane
    # and in console-fixtures.
    ("crates/*/tests/", "console/fixtures/"): {"integration", "console"},
    # The memory eval harness tests run scripts/memory-evals.
    ("crates/*/tests/", "scripts/memory-evals"): {"integration"},
    # Workflow files: ci.yml runs console scripts and keys the console lane's
    # npm cache on console/package-lock.json, keys flow-editor's on its own,
    # installs sdk/python in test-python and runs examples/001's topology
    # script in console-acceptance. Its contract scripts run in fmt-lint and
    # test-typescript runs on every run, so neither needs a lane; release,
    # docs and live workflows are not CI lanes.
    (".github/", "console/"): {"console"},
    (".github/", "flow-editor/"): {"flow_editor"},
    (".github/", "sdk/python/"): {"python"},
    (".github/", "examples/"): {"console"},
    (".github/", "sdk/typescript/"): set(),
    (".github/", "scripts/"): set(),
    (".github/", "scripts/memory-evals"): set(),
    (".github/", "docs/"): set(),
    (".github/", "skippable"): set(),
    (".github/", "packages/"): set(),
    # forge.yml's path filters name the crate sources Forge's tests read (a
    # test below checks them); Forge's jobs are not lanes here.
    (".github/", "crates/"): set(),
    # make test-python reads sdk/python; the release targets and local
    # hooks are not CI lanes.
    ("Makefile", "sdk/python/"): {"python"},
    ("Makefile", "sdk/typescript/"): set(),
    ("Makefile", "scripts/"): set(),
    ("Makefile", "skippable"): set(),
    (".pre-commit-config.yaml", "scripts/"): set(),
    # Contract tests, the classifier and release scripts run in fmt-lint,
    # which always runs.
    ("scripts/", "console/"): set(),
    ("scripts/", "console/fixtures/"): set(),
    ("scripts/", "flow-editor/"): set(),
    ("scripts/", "sdk/"): set(),
    ("scripts/", "sdk/python/"): set(),
    ("scripts/", "sdk/typescript/"): set(),
    ("scripts/", "scripts/"): set(),
    ("scripts/", "scripts/memory-evals"): set(),
    ("scripts/", "docs/"): set(),
    ("scripts/", "skippable"): set(),
    # Forge's separate workflow exercises these reads. Its path filters include
    # installer, Console/packages, the Python SDK and root README/licenses.
    # They do not require an additional lane in this workflow's classifier.
    ("installer/", "console/"): set(),
    ("installer/", "packages/"): set(),
    ("installer/", "sdk/python/"): set(),
    ("installer/", "skippable"): set(),
    # A dependency's examples/* workspace metadata in Forge's lockfile names
    # examples but does not load this repository's example applications.
    ("installer/", "examples/"): set(),
    # installer/tests/test_gateway_shutdown_horizon.py pins the shutdown
    # horizon crates/meerkat-meta/src/rpc_gateway.rs advertises. Forge's
    # workflow runs it, and its path filters name that source, so a pull
    # request that changes it runs the test; no lane here does. Its read of
    # the Python SDK is like every installer read of it: Forge's path
    # filters include sdk/python.
    ("installer/tests/test_gateway_shutdown_horizon.py", "crates/"): set(),
    ("installer/tests/test_gateway_shutdown_horizon.py", "sdk/python/"): set(),
    ("scripts/", "Makefile"): set(),
    ("scripts/", ".config/nextest.toml"): set(),
    ("scripts/", "examples/"): set(),
    ("scripts/", "packages/"): set(),
    ("scripts/", "tools/bazel/"): set(),
    # Tests outside the crates read crate files: the console's (the embedded
    # bundle's freshness, the HTTP route and RPC method lists, the voice
    # transcript fixture), the flow editor's source contract and bundle
    # build, the Python SDK's wire fixtures, and the example packs' release
    # targets. A pull request that changes only such a file must still run
    # the reader's lanes; a test below checks that file by file.
    ("console/", "crates/"): {"console"},
    ("flow-editor/", "crates/"): {"flow_editor"},
    ("sdk/python/", "crates/"): {"python"},
    ("examples/", "crates/"): {"rust", "integration", "console"},
    # test-typescript runs on every run, fmt-lint runs the scripts and the
    # hooks' configuration, and the workspace manifest names its members.
    ("sdk/typescript/", "crates/"): set(),
    ("scripts/", "crates/"): set(),
    # fmt-lint runs the continuity export scripts' tests on every run; they
    # read their golden files below crates/meerkat-meta/tests/fixtures and
    # check the pins and tables of the export guide under docs/.
    ("tools/", "crates/"): set(),
    ("tools/", "docs/"): set(),
    (".pre-commit-config.yaml", "crates/"): set(),
    ("Cargo.toml", "crates/"): set(),

}


def reader_role(path: str) -> str:
    """The area a referencing file runs as: crates split by target kind."""
    parts = path.split("/")
    if parts[0] == "crates" and len(parts) > 2:
        return f"crates/*/{parts[2]}" + ("/" if len(parts) > 3 else "")
    for prefix, _ in ci_changes.AREAS:
        if path.startswith(prefix):
            return prefix
    return parts[0] + "/" if len(parts) > 1 else parts[0]


def area_of(path: str) -> str:
    if path in ci_changes.AREA_FILES:
        return path
    if ci_changes.skippable(path) and not path.startswith("docs/"):
        return "skippable"
    for prefix, _ in ci_changes.AREAS:
        if path.startswith(prefix):
            return prefix
    return path.split("/", 1)[0] + "/"


def tracked_files(root: Path, target: str) -> list[str]:
    """Files at or below `target`, or named by its extension-less start (an
    import such as ../sdk/typescript/src/live). Plain string comparison:
    the text may hold glob characters."""
    candidate = root / target
    if candidate.is_file():
        return [target]
    if candidate.is_dir():
        found = []
        for directory, subdirectories, files in os.walk(candidate):
            subdirectories[:] = [d for d in subdirectories if d not in {"node_modules", ".git", "dist", ".tmp"}]
            found += [Path(directory, name).relative_to(root).as_posix() for name in files]
        return found
    parent = candidate.parent
    if parent.is_dir():
        return [(parent / entry.name).relative_to(root).as_posix() for entry in parent.iterdir()
                if entry.is_file() and entry.name.startswith(candidate.name + ".")]
    return []


def target_lanes(root: Path, target: str) -> frozenset | None:
    """The lanes a change anywhere under `target` is sure to reach (the
    intersection over its files), or None when it names nothing."""
    files = tracked_files(root, target)
    if not files:
        return None
    lanes = ci_changes.EVERY_LANE
    for name in files:
        lanes &= ci_changes.path_lanes(name)
    return lanes


def quoted_lists(text: str):
    """Scan quoted pathspec lists without ambiguous repeated whitespace.

    A failed candidate still allows a later or nested list to be inspected.
    As with the previous recognizer, commas are optional and a trailing comma
    is accepted. Contents are names only, never evaluated as source code.
    """
    search = 0
    while start := LIST_START.search(text, search):
        cursor, values = start.end(), []
        while item := QUOTED_ITEM.match(text, cursor):
            values.append(item.group(1))
            cursor = item.end()
            while cursor < len(text) and text[cursor].isspace():
                cursor += 1
            if cursor < len(text) and text[cursor] == ",":
                cursor += 1
            while cursor < len(text) and text[cursor].isspace():
                cursor += 1
            if cursor < len(text) and text[cursor] in "])":
                yield values
                search = cursor + 1
                break
        else:
            search = start.end()


def references(text: str, *, bare: bool) -> list[str]:
    found = []
    for match in PATH_REFERENCE.finditer(text):
        if match.group(1):
            found.append(match.group(1) + (match.group(2) or ""))
        elif match.group(3):
            found.append(match.group(3) + match.group(4))
        elif not OBJECT_VALUE.search(text, max(0, match.start() - 40), match.start()):
            found.append(match.group(5))
    if bare:
        for values in quoted_lists(text):
            names = [name for name in values if name in AREA_NAMES]
            if len(names) >= 2:
                found += names
        found += ["/".join([match.group(1), *QUOTED.findall(match.group(2))]) for match in JOINED.finditer(text)]
    cut = []
    for target in found:
        wildcard = WILDCARD.search(target)
        if wildcard:
            target = target[:wildcard.start()]
        cut.append(target.rstrip("./") or target)
    return cut


def cross_area_reads(root: Path) -> dict[tuple[str, str], set[str]]:
    reads: dict[tuple[str, str], set[str]] = {}
    for directory, subdirectories, files in os.walk(root):
        subdirectories[:] = [d for d in subdirectories
                             if d not in UNSCANNED_DIRS and not d.startswith(".release-workflow-fixture")]
        for name in files:
            if Path(name).suffix not in SCANNED_SUFFIXES and name not in SCANNED_NAMES:
                continue
            path = Path(directory, name)
            relative = path.relative_to(root).as_posix()
            try:
                if path.stat().st_size > MAX_SCANNED_BYTES:
                    continue
                text = path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            # Comments and docstrings document; they do not read.
            for block in BLOCKS:
                text = block.sub("", text)
            text = "\n".join(line for line in text.splitlines() if not COMMENT_LINE.match(line))
            for target in references(text, bare=path.suffix != ".rs"):
                lanes = target_lanes(root, target)
                if lanes is None or lanes == ci_changes.EVERY_LANE:
                    continue
                reader, read = area_of(relative), area_of(target.rstrip("/") + ("/" if (root / target).is_dir() else ""))
                if (root / target).is_dir():
                    read = target.rstrip("/") + "/" if target.rstrip("/") in AREA_NAMES else read
                if reader == read or (read != "skippable" and relative.startswith(read)):
                    continue
                reads.setdefault((reader_role(relative), read), set()).add(f"{relative} -> {target}")
    return reads


# A file module declaration and the attributes before it, at the start of a
# line (so a commented-out declaration is not one).
MODULE_DECLARATION = re.compile(
    r"(?m)^[ \t]*(?P<attributes>(?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<name>\w+)\s*;"
)
CFG_ATTRIBUTE = re.compile(r"#\[cfg\(([^\]]*)\)\]")
PATH_ATTRIBUTE = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def module_file(source: Path, declaration: re.Match) -> Path:
    """The file a `mod name;` declaration in `source` loads."""
    explicit = PATH_ATTRIBUTE.search(declaration["attributes"])
    if explicit:
        candidates = [source.parent / explicit[1]]
    else:
        # Children of a crate root or mod.rs live beside it; those of x.rs in x/.
        root_like = source.name in ("lib.rs", "main.rs", "mod.rs") or source.parent.name == "bin"
        directory = source.parent if root_like else source.with_suffix("")
        candidates = [directory / f"{declaration['name']}.rs", directory / declaration["name"] / "mod.rs"]
    found = [candidate for candidate in candidates if candidate.is_file()]
    assert found, f"cannot find the file of module {declaration['name']} declared in {source}"
    return found[0]


def gated_modules(root: Path) -> dict[str, set[str]]:
    """Feature lane -> the files of the modules a crate's sources declare
    under that feature's cfg, and of every module those declare in turn
    (which need not repeat the cfg)."""
    gated = {lane: set() for lane in ci_changes.FEATURE_GATES}
    pending = []
    for source in sorted((root / "crates").glob("*/src/**/*.rs")):
        for declaration in MODULE_DECLARATION.finditer(source.read_text(encoding="utf-8")):
            cfgs = " ".join(CFG_ATTRIBUTE.findall(declaration["attributes"]))
            lanes = {lane for lane, gate in ci_changes.FEATURE_GATES.items() if gate.search(cfgs)}
            if lanes:
                pending.append((module_file(source, declaration), lanes))
    while pending:
        module, lanes = pending.pop()
        relative = module.relative_to(root).as_posix()
        if all(relative in gated[lane] for lane in lanes):
            continue
        for lane in lanes:
            gated[lane].add(relative)
        for declaration in MODULE_DECLARATION.finditer(module.read_text(encoding="utf-8")):
            pending.append((module_file(module, declaration), lanes))
    return gated


def read_from_root(path: str) -> str | None:
    try:
        return (ROOT / path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None


# What a crate source embeds or loads: include_str!/include_bytes! of a path
# relative to the file or (through concat! of CARGO_MANIFEST_DIR) to its
# crate, and path-like joins in a file that names CARGO_MANIFEST_DIR.
INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*(concat!\(\s*env!\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*)?"([^"]+)"')
MANIFEST_JOIN = re.compile(r'\.join\(\s*"([^"]*[/.][^"]*)"\s*\)')


def embedded_files(root: Path) -> set[tuple[str, str]]:
    """(source, data file) for every non-Rust file a crate source embeds or loads."""
    pairs = set()
    for source in sorted((root / "crates").glob("*/**/*.rs")):
        crate = root.joinpath(*source.relative_to(root).parts[:2])
        text = source.read_text(encoding="utf-8", errors="replace")
        targets = [(crate if match[1] else source.parent) / match[2].lstrip("/") for match in INCLUDE.finditer(text)]
        if "CARGO_MANIFEST_DIR" in text:
            targets += [crate / literal for literal in MANIFEST_JOIN.findall(text)]
        for target in targets:
            target = Path(os.path.normpath(target))
            # Data inside a crate; a join up to the crate or repository root
            # (tests locating the checkout) is not data.
            if not target.is_relative_to(root / "crates") or len(target.relative_to(root).parts) < 3:
                continue
            files = [target] if target.is_file() else sorted(target.rglob("*")) if target.is_dir() else []
            for found in files:
                if found.is_file() and found.suffix != ".rs":
                    pairs.add((source.relative_to(root).as_posix(), found.relative_to(root).as_posix()))
    return pairs


FORGE_WORKFLOW = ".github/workflows/forge.yml"
# Installer files that name another area's files without reading this
# repository's copies: the web lockfile's examples/* workspace metadata, and
# the .gitignore every generated application package carries (compiler.py
# writes it; the deployment source and update lists name it).
FORGE_NAMED_NOT_READ = ("examples/", ".gitignore")


def forge_pull_request_paths(root: Path) -> list[str]:
    workflow = yaml.safe_load((root / FORGE_WORKFLOW).read_text())
    # YAML 1.1 reads the bare key `on` as True.
    return workflow.get("on", workflow.get(True))["pull_request"]["paths"]


def path_filter_matches(patterns: list[str], path: str) -> bool:
    """GitHub's path filter: the last pattern that matches decides, a
    pattern that starts with ! excludes, ** crosses directories and * and ?
    do not."""
    matched = False
    for pattern in patterns:
        negative = pattern.startswith("!")
        tokens = re.findall(r"\*\*|\*|\?|[^*?]+", pattern[1:] if negative else pattern)
        expression = "".join(".*" if token == "**" else "[^/]*" if token == "*" else "[^/]" if token == "?"
                             else re.escape(token) for token in tokens)
        if re.fullmatch(expression, path):
            matched = not negative
    return matched


def named_not_read(name: str) -> bool:
    return any(name == entry or (entry.endswith("/") and name.startswith(entry)) for entry in FORGE_NAMED_NOT_READ)


class CiChangesTests(unittest.TestCase):
    def full(self, *paths, event="pull_request", read=no_gates):
        return ci_changes.needs_full_run(event, list(paths), read)

    def lanes(self, *paths, event="pull_request", read=no_gates):
        return set(ci_changes.needed_lanes(event, list(paths), read))

    def deferred(self, *paths, read=no_gates):
        return set(ci_changes.classify("pull_request", list(paths), read)[1])

    def test_quoted_pathspecs_survive_long_unclosed_whitespace_and_keep_later_reads(self):
        malformed = '["console"' + " " * 20_000 + 'unexpected'
        valid = '["console" "sdk" "scripts", ]'
        self.assertEqual(references(malformed + valid, bare=True), ["console", "sdk", "scripts"])
        self.assertEqual(references('["console" "sdk"' + " " * 20_000 + ']', bare=True),
                         ["console", "sdk"])
        self.assertEqual(list(quoted_lists('(["console" "sdk"]) ["scripts"]')),
                         [["console", "sdk"], ["scripts"]])

    def test_docs_and_top_level_markdown_skip_the_heavy_jobs(self):
        self.assertEqual(self.lanes("docs/concepts/events.mdx", "README.md", "CHANGELOG.md"), set())
        self.assertEqual(self.lanes("LICENSE-MIT"), set())

    def test_non_ci_workflows_skip_the_heavy_jobs_and_python_scripts_run_the_console_lane(self):
        self.assertEqual(self.lanes(".github/workflows/e2e-live.yml", ".github/workflows/publish-docs.yml"), set())
        self.assertEqual(self.lanes(".github/CODEOWNERS", ".github/release.yml", ".github/workflows/release.yml"),
                         set())
        # The shape of PR #642: release.yml plus its python contract test.
        # fmt-lint runs the script; console/fixture-ready.test.cjs greps it.
        self.assertEqual(self.lanes(".github/workflows/release.yml", "scripts/test_release_workflow.py"),
                         {"console"})

    def test_files_only_fmt_lint_and_audit_read_skip_every_lane(self):
        # fmt-lint and audit run on every pull request.
        export_tools = sorted(f"tools/continuity-export/{path.name}"
                              for path in (ROOT / "tools/continuity-export").iterdir() if path.is_file())
        self.assertLessEqual({"tools/continuity-export/continuity_export.py",
                              "tools/continuity-export/continuity-export.mjs",
                              "tools/continuity-export/test_continuity_export.py",
                              "tools/continuity-export/continuity-export.test.mjs"}, set(export_tools))
        for path in (".pre-commit-config.yaml", ".gitignore", "BUILD.bazel", "MODULE.bazel", ".bazelrc",
                     ".bazelversion", "tools/bazel/platforms/BUILD.bazel", "deny.toml",
                     # Generated per crate; only fmt-lint's freshness check reads them.
                     "crates/meerkat-meta/BUILD.bazel",
                     # The continuity export scripts and their tests, and every
                     # other file beside them; fmt-lint runs the tests.
                     *export_tools):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), set())
        # Every other top-level script reaches the console lane only: its
        # fixture test greps scripts/ for port reservations.
        for path in ("scripts/pre-push-clippy.sh", "scripts/pre-push-unit.sh",
                     "scripts/generate-bazel-rust-builds.mjs", "scripts/check-memory-bright-line"):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), {"console"})

    def test_the_classifier_and_its_contract_run_every_lane(self):
        for path in ("scripts/ci-changes.py", "scripts/test_ci_changes.py"):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), EVERY_LANE)

    def test_files_every_lane_reads_run_every_lane(self):
        for path in (
            # ci.yml defines the jobs, so its own pull request must run them.
            ".github/workflows/ci.yml",
            # Read by tests/console_route_auth.rs, governance_check and the
            # console contract tests.
            "docs/rct/console-rest-sse-contract-v0.5.0.json",
            "docs/rct/traceability.md",
            # src/mobpack.rs reads .claude/skills.
            ".claude/skills/meerkat-meta/SKILL.md",
            # Every Rust job builds through it, and on a pull request first
            # fetches the warmed dependency builds.
            "scripts/repo-cargo",
            "scripts/ci-warm-deps.py",
            ".github/actions/fetch-warm-deps/action.yml",
            "scripts/nested/helper.py",
            # Unclassified files fail closed.
            "tests/memory-evals/README.md",
            "unknown-root-file",
        ):
            with self.subTest(path=path):
                self.assertTrue(self.full(path))
                self.assertTrue(self.full("README.md", path), "one such file makes the whole run full")

    def test_rust_sources_reach_every_lane_but_console_perf(self):
        # console-perf bundles only console/ and packages/console-*; every
        # other lane compiles the crates.
        for path in (
            # Markdown below the top level can be include_str! input.
            "crates/meerkat-meta/src/memory/steward_prompt_v0.md",
            "crates/meerkat-meta/src/lib.rs",
            "crates/meerkat-meta/console-dist/index.html",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo/config.toml",
        ):
            with self.subTest(path=path):
                self.assertEqual(set(ci_changes.path_lanes(path)), RUST_LANES)

    def test_a_pull_request_leaves_other_configurations_of_a_rust_change_to_the_push(self):
        # The rust and integration lanes compile and test the default
        # configuration; the push runs the rest, and release.yml requires
        # the push run to be green before a tag.
        for path in ("crates/meerkat-meta/src/governance.rs", "crates/meerkat-meta/tests/list_runs.rs",
                     "crates/meerkat-meta/src/memory/steward_prompt_v0.md",
                     "crates/meta-store-conformance/src/lib.rs"):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), RUST_PR_LANES)
                self.assertEqual(self.deferred(path), RUST_LANES - RUST_PR_LANES)
        # A lane whose own input changes beside it still runs.
        self.assertEqual(self.lanes("crates/meerkat-meta/src/lib.rs", "console/src/App.tsx"),
                         RUST_PR_LANES | CONSOLE_LANES)
        self.assertEqual(self.deferred("crates/meerkat-meta/src/lib.rs", "console/src/App.tsx"),
                         FEATURE_LANES | {"python"})
        self.assertEqual(self.lanes("crates/meerkat-meta/src/lib.rs", "sdk/python/meerkat_meta/types.py"),
                         RUST_PR_LANES | {"python", "console"})
        # Pushes run every lane.
        self.assertEqual(self.lanes("crates/meerkat-meta/src/rpc.rs", event="push"), EVERY_LANE)

    def test_feature_lanes_run_for_their_own_inputs(self):
        def text(content):
            return lambda path: content

        voice = '#[cfg(feature = "openai-live")]\nfn live() {}\n'
        extension = '#![cfg(feature="extension-state")]\n'
        both = voice + '#[cfg(all(test, feature = "extension-state"))]\nmod t;\n'
        for paths, read, lanes in (
            # Code the feature gates, in the file itself.
            (("crates/meerkat-meta/src/governance.rs",), text(voice), RUST_PR_LANES | {"voice"}),
            (("crates/meerkat-meta/tests/extension_state.rs",), text(extension),
             RUST_PR_LANES | {"extension_state"}),
            (("crates/meerkat-meta/src/mob_handle_runtime.rs",), text(both), RUST_PR_LANES | FEATURE_LANES),
            # A module the feature gates whole: its file need not say so.
            (("crates/meerkat-meta/src/console_voice/live_host.rs",), no_gates, RUST_PR_LANES | {"voice"}),
            (("crates/meerkat-meta/src/extension_state.rs",), no_gates, RUST_PR_LANES | {"extension_state"}),
            (("crates/meerkat-meta/src/extension_state/store.rs",), no_gates,
             RUST_PR_LANES | {"extension_state"}),
            # The crate only the extension-state feature builds.
            (("crates/meta-extension-state/src/lib.rs",), no_gates, RUST_PR_LANES | {"extension_state"}),
            # Build configuration every lane's build reads: a dependency or
            # toolchain change runs every lane that compiles Rust.
            (("Cargo.lock",), no_gates, RUST_LANES),
            (("Cargo.toml",), no_gates, RUST_LANES),
            (("rust-toolchain.toml",), no_gates, RUST_LANES),
            ((".cargo/config.toml",), no_gates, RUST_LANES),
            (("crates/meerkat-meta/Cargo.toml",), no_gates, RUST_LANES),
            # A file that cannot be read runs every feature lane.
            (("crates/meerkat-meta/src/governance.rs",), lambda path: None, RUST_PR_LANES | FEATURE_LANES),
            # Only Rust sources are read for gates; other files reach a
            # deferred lane only when FEATURE_PATHS or LANE_INPUTS lists them.
            (("crates/meerkat-meta/src/memory/steward_prompt_v0.md",), text(voice), RUST_PR_LANES),
            (("crates/meerkat-meta/tests/fixtures/console_voice_v1.json",), text(""),
             RUST_PR_LANES | {"voice", "console"}),
            # The bundles the backends embed run the browser lanes that serve them.
            (("crates/meerkat-meta/console-dist/index.html",), no_gates, RUST_PR_LANES | {"console"}),
            (("crates/meerkat-meta/flow-editor-dist/index.html",), no_gates, RUST_PR_LANES | {"flow_editor"}),
        ):
            with self.subTest(paths=paths):
                self.assertEqual(self.lanes(*paths, read=read), lanes)

    def test_backend_modules_run_the_lanes_that_drive_them_end_to_end(self):
        # A pull request that can break an end-to-end suite runs it before
        # the merge: the console backends and HTTP console, the flow editor's
        # binary and mobpack authoring, and the gateway the Python SDK drives.
        for path, lanes in (
            ("crates/meerkat-meta/src/http_console.rs", {"console"}),
            ("crates/meerkat-meta/src/http_console/routes.rs", {"console"}),
            ("crates/meerkat-meta/src/console_aggregator/store.rs", {"console"}),
            ("crates/meerkat-meta/examples/library_mode_reference.rs", {"console"}),
            ("crates/meerkat-meta/src/bin/meta_flow_editor.rs", {"flow_editor"}),
            ("crates/meerkat-meta/src/http_flow_editor.rs", {"flow_editor"}),
            ("crates/meerkat-meta/src/mobpack.rs", {"flow_editor"}),
            ("crates/meerkat-meta/src/rpc.rs", {"python", "flow_editor"}),
            ("crates/meerkat-meta/src/rpc/mob_methods.rs", {"python"}),
            # Sources the console and flow editor contract tests read.
            ("crates/meerkat-meta/src/rpc/topology_methods.rs", {"python", "console"}),
            ("crates/meerkat-meta/src/unified_runtime/http.rs", {"flow_editor"}),
            ("crates/meerkat-meta/src/rpc_gateway.rs", {"python", "extension_state"}),
            ("crates/meerkat-meta/src/bin/rpc_gateway.rs", {"python"}),
            # The keyed inject plane the mob announcement process tests drive:
            # its control channel, its composition and the identity runtime.
            ("crates/meerkat-meta/src/runtime/keyed_inject.rs", {"extension_state"}),
            ("crates/meerkat-meta/src/runtime/cross_mob_control.rs", {"extension_state"}),
            ("crates/meerkat-meta/src/runtime/cross_mob_remote.rs", {"extension_state"}),
            ("crates/meerkat-meta/src/unified_runtime/cross_mob.rs", {"extension_state"}),
            ("crates/meerkat-meta/src/unified_runtime/builder.rs", {"extension_state"}),
            ("crates/meerkat-meta/src/identity_first/runtime.rs", {"extension_state"}),
        ):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), RUST_PR_LANES | lanes)
        # Narrow: neighbours of a backend module are not its inputs.
        for path in ("crates/meerkat-meta/src/http_console_tests/mod.rs", "crates/meerkat-meta/src/rpc_types.rs",
                     "crates/meerkat-meta/src/bin/meta_gateway.rs", "crates/meerkat-meta/src/mobpack_helpers.rs"):
            with self.subTest(path=path):
                self.assertEqual(self.lanes(path), RUST_PR_LANES)

    def test_files_a_deferred_lanes_input_embeds_or_loads_run_that_lane(self):
        """A deferred lane's own input embeds or loads data: include_str!,
        include_bytes! and joins onto CARGO_MANIFEST_DIR. A pull request that
        changes only that data must run the lane too, so FEATURE_PATHS or
        LANE_INPUTS lists it with the same lanes."""
        pairs = embedded_files(ROOT)
        self.assertIn(("crates/meerkat-meta/src/console_voice/captions.rs",
                       "crates/meerkat-meta/tests/fixtures/console_voice_v1.json"), pairs)
        for reader, embedded in sorted(pairs):
            required = self.lanes(reader, read=read_from_root) & ci_changes.RUST_DEFERRED_LANES
            with self.subTest(reader=reader, embedded=embedded):
                missing = required - self.lanes(embedded, read=read_from_root)
                self.assertFalse(missing, f"list {embedded} in LANE_INPUTS with {sorted(missing)}")

    def test_crate_files_read_from_other_areas_run_their_readers_lanes(self):
        """The console, flow editor and SDK tests read crate files: sources
        they check for drift, the embedded bundles, wire fixtures. A pull
        request that changes only such a file must run the reader's lanes,
        so FEATURE_PATHS or LANE_INPUTS lists it with those lanes."""
        reads = cross_area_reads(ROOT)
        self.assertIn("flow-editor/test/source-contract.test.cjs -> crates/meerkat-meta/src/rpc.rs",
                      reads[("flow-editor/", "crates/")])
        self.assertIn("console/src/lib/voice-session.test.ts -> "
                      "crates/meerkat-meta/tests/fixtures/console_voice_v1.json", reads[("console/", "crates/")])
        self.assertIn("sdk/python/tests/test_role_migrations.py -> "
                      "crates/meerkat-meta/tests/fixtures/role_migrations_init_params.json",
                      reads[("sdk/python/", "crates/")])
        for (reader, area), found in sorted(reads.items()):
            required = CROSS_AREA_READS.get((reader, area), set())
            if area != "crates/" or not required:
                continue
            for entry in sorted(found):
                for name in tracked_files(ROOT, entry.split(" -> ")[1]):
                    with self.subTest(entry=entry, file=name):
                        missing = required - self.lanes(name, read=read_from_root)
                        self.assertFalse(missing, f"list {name} in LANE_INPUTS with {sorted(missing)}")

    def test_the_embed_scan_resolves_every_form(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for relative, text in {
                "crates/c/src/a.rs": 'const A: &str = include_str!("../data/a.json");\n'
                                     'const B: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/b.bin"));\n'
                                     'fn load() { let _ = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/set"); }\n'
                                     'fn other() { let _ = dir.join("not-a-path"); }\n',
                "crates/c/data/a.json": "{}",
                "crates/c/data/b.bin": "",
                "crates/c/fixtures/set/one.json": "{}",
                "crates/c/fixtures/set/two.rs": "",
                # The checkout root is not data.
                "crates/c/tests/root.rs": 'fn root() { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."); }\n',
                "README.md": "",
            }.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(text)
            self.assertEqual(embedded_files(root), {
                ("crates/c/src/a.rs", "crates/c/data/a.json"),
                ("crates/c/src/a.rs", "crates/c/data/b.bin"),
                ("crates/c/src/a.rs", "crates/c/fixtures/set/one.json"),
            })

    def test_feature_paths_cover_every_module_a_feature_gates(self):
        """A file declared under a feature's cfg need not repeat the cfg, so
        the classifier cannot see the gate in its text: FEATURE_PATHS must
        list it, or a pull request changing it would compile it nowhere."""
        gated = gated_modules(ROOT)
        self.assertIn("crates/meerkat-meta/src/console_voice/live_host.rs", gated["voice"])
        self.assertIn("crates/meerkat-meta/src/extension_state.rs", gated["extension_state"])
        for lane, files in gated.items():
            for path in sorted(files):
                with self.subTest(lane=lane, path=path):
                    self.assertTrue(path.startswith(ci_changes.FEATURE_PATHS[lane]),
                                    f"add {path.removesuffix('.rs')} to FEATURE_PATHS[{lane!r}]")

    def test_the_gated_module_scan_resolves_every_declaration_form(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for relative, text in {
                "crates/c/src/lib.rs": (
                    '#[cfg(feature = "openai-live")]\npub mod live;\n'
                    '#[cfg(all(test, feature = "extension-state"))]\n#[allow(dead_code)]\npub(crate) mod ext;\n'
                    'mod plain;\n'
                    '#[cfg(feature = "openai-live")]\n#[path = "elsewhere/moved.rs"]\nmod moved;\n'
                ),
                "crates/c/src/live.rs": '#[cfg(feature = "openai-live")]\nmod nested;\n',
                "crates/c/src/live/nested/mod.rs": "",
                "crates/c/src/ext.rs": "",
                "crates/c/src/plain.rs": "",
                "crates/c/src/elsewhere/moved.rs": "",
                "crates/c/src/bin/tool.rs": '#[cfg(feature = "openai-live")]\nmod helper;\n',
                "crates/c/src/bin/helper.rs": "",
            }.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(text)
            gated = gated_modules(root)
        self.assertEqual(gated, {
            "voice": {"crates/c/src/live.rs", "crates/c/src/live/nested/mod.rs",
                      "crates/c/src/elsewhere/moved.rs", "crates/c/src/bin/helper.rs"},
            "extension_state": {"crates/c/src/ext.rs"},
        })
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "crates/c/src").mkdir(parents=True)
            (root / "crates/c/src/lib.rs").write_text('#[cfg(feature = "openai-live")]\nmod missing;\n')
            with self.assertRaisesRegex(AssertionError, "missing"):
                gated_modules(root)

    def test_area_changes_run_only_the_lanes_that_read_them(self):
        for paths, lanes in (
            (("console/package.json", "console/src/ConsoleApp.tsx"), CONSOLE_LANES),
            (("flow-editor/src/main.ts",), {"flow_editor", "console"}),
            (("sdk/python/meerkat_meta/__init__.py",), {"python", "integration", "console"}),
            (("sdk/typescript/src/live.ts",), {"integration"} | CONSOLE_LANES),
            (("sdk/python/meerkat_meta/types.py", "sdk/typescript/src/types.ts"),
             {"python", "integration"} | CONSOLE_LANES),
            (("flow-editor/src/main.ts", "README.md", "docs/guide.md"), {"flow_editor", "console"}),
            # The component packages feed the console, its perf bundle and
            # the flow editor.
            (("packages/console-core/src/index.ts",), CONSOLE_LANES),
            (("packages/flow-editor-core/src/index.ts",), CONSOLE_LANES),
            # tests/console_customization_fixtures.rs reads console fixtures.
            (("console/fixtures/configured-host-shell/config/console.toml",), CONSOLE_LANES | {"integration"}),
            # Out-of-tree examples and the packs console-acceptance runs.
            (("examples/001-incident-command-center-pack/server.rs",), {"rust", "integration", "console"}),
            (("examples/console-extensions/preview.cjs",), {"rust", "integration", "console"}),
            (("Makefile",), {"python", "flow_editor"}),
            ((".config/nextest.toml",), {"rust", "integration"}),
            (("scripts/memory-evals",), {"integration", "console"}),
        ):
            with self.subTest(paths=paths):
                self.assertEqual(self.lanes(*paths), lanes)

    def test_lanes_cover_every_gated_job_once(self):
        jobs = [job for lane_jobs in ci_changes.LANE_JOBS.values() for job in lane_jobs]
        self.assertEqual(len(jobs), len(set(jobs)))
        for _, lanes in ci_changes.AREAS:
            self.assertLessEqual(set(lanes), EVERY_LANE)
        for lanes in ci_changes.AREA_FILES.values():
            self.assertLessEqual(set(lanes), EVERY_LANE)
        self.assertLessEqual(ci_changes.RUST_DEFERRED_LANES, ci_changes.RUST_LANES)
        self.assertEqual(set(ci_changes.FEATURE_PATHS), FEATURE_LANES)
        self.assertLessEqual(FEATURE_LANES, ci_changes.RUST_DEFERRED_LANES)
        for prefix, lanes in ci_changes.LANE_INPUTS:
            with self.subTest(prefix=prefix):
                self.assertTrue(prefix.startswith("crates/") and lanes, prefix)
                self.assertLessEqual(set(lanes), ci_changes.RUST_DEFERRED_LANES)
                self.assertTrue((ROOT / prefix).exists(), f"{prefix} names nothing")

    def test_every_cross_area_read_has_audited_lanes(self):
        """A file that reads another area's files runs in some lanes, so a
        change to the read files must reach those lanes. The scan finds such
        reads in the Rust crates, the console, flow editor and SDK sources,
        the packages, examples and scripts; each must be audited above, and
        the area it reads must reach the lanes its reader runs in."""
        reads = cross_area_reads(ROOT)
        unaudited = {pair: sorted(found)[:3] for pair, found in reads.items() if pair not in CROSS_AREA_READS}
        self.assertEqual(unaudited, {}, "new cross-area reads: give them lanes in ci-changes.py's AREAS")
        for (_, area), required in CROSS_AREA_READS.items():
            with self.subTest(area=area):
                self.assertLessEqual(required, set(ci_changes.path_lanes(area)))
        # The scan sees the reads that motivated the lanes.
        self.assertIn(("console/", "sdk/typescript/"), reads)
        self.assertIn(("crates/*/tests/", "sdk/python/"), reads)
        self.assertIn(("crates/*/tests/", "scripts/memory-evals"), reads)
        self.assertIn(("flow-editor/", "Makefile"), reads)
        self.assertIn(("crates/*/Cargo.toml", "examples/"), reads)

    def test_forge_pull_requests_start_for_every_file_forges_tests_read(self):
        """Forge's tests run only in its own workflow. A pull request that
        changes a file they read outside installer/ must start that workflow,
        so forge.yml's pull_request paths name each such file or a directory
        above it."""
        patterns = forge_pull_request_paths(ROOT)
        uncovered = set()
        for (reader, _), found in cross_area_reads(ROOT).items():
            if not reader.startswith("installer/"):
                continue
            for entry in found:
                source, target = entry.split(" -> ")
                for name in tracked_files(ROOT, target):
                    if not named_not_read(name) and not path_filter_matches(patterns, name):
                        uncovered.add(f"{source} -> {name}")
        self.assertEqual(sorted(uncovered), [], "name these files in forge.yml's pull_request paths")

    def test_forge_path_filters_match_as_github_reads_them(self):
        patterns = ["installer/**", "docs/*.md", "!installer/web/dist/**", "README.md"]
        for path, matched in (
            ("installer/launchkit/compiler.py", True),
            ("installer/web/dist/app.js", False),
            ("docs/a.md", True),
            ("docs/nested/a.md", False),
            ("README.md", True),
            ("README.mdx", False),
            ("sdk/python/x.py", False),
        ):
            with self.subTest(path=path):
                self.assertEqual(path_filter_matches(patterns, path), matched)
        self.assertTrue(named_not_read("examples/package.json"))
        self.assertTrue(named_not_read(".gitignore"))
        self.assertFalse(named_not_read("knowledge/.gitignore"))
        self.assertFalse(named_not_read("crates/meerkat-meta/src/rpc_gateway.rs"))

    def test_the_scan_catches_a_new_cross_area_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for relative, text in {
                "sdk/python/meerkat_meta/types.py": "X = 1\n",
                "sdk/typescript/src/types.ts": "export {}\n",
                "console/src/App.tsx": "export {}\n",
                "docs/guide.md": "# Guide\n",
                "scripts/tool.py": "\n",
                # A real installer source read still needs an audited lane.
                "installer/launchkit/read_source.py": 'Path("crates/x/src/lib.rs").read_text()\n',
                "crates/x/src/lib.rs": '// sdk/python/meerkat_meta/types.py is only named here\n'
                                      'const T: &str = include_str!("../../../sdk/python/meerkat_meta/types.py");\n',
                "flow-editor/src/a.ts": 'import { X } from "../../console/src/App";\n',
                # A git grep pathspec list of bare area names.
                "packages/p/grep.cjs": 'run(["grep", "--", "console", "sdk", "scripts"]);\n',
                # A shell variable root and a glob cut at its first wildcard.
                "examples/run.sh": 'cat "$ROOT/docs/guide.md"\nls sdk/typescript/src/*.ts\n',
                # A manifest reader.
                "tests/Makefile": "check:\n\tcat ../docs/guide.md\n",
                # Identifiers and docstrings are not reads.
                "packages/q/ids.ts": 'const a = { id: "docs", profile: "console" };\nconst b = ["code", "docs"];\n',
                "packages/q/doc.py": '"""See docs/guide.md."""\n',
                # A crate fixture, through a path joined part by part.
                "crates/x/tests/fixtures/wire.json": "{}\n",
                "sdk/python/tests/test_wire.py": 'WIRE = (ROOT / "crates" / "x"\n        / "tests" / "fixtures" / "wire.json")\n',
            }.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(text)
            reads = cross_area_reads(root)
        self.assertEqual(set(reads), {
            ("crates/*/src/", "sdk/python/"), ("flow-editor/", "console/"),
            ("packages/", "console/"), ("packages/", "sdk/"), ("packages/", "scripts/"),
            ("examples/", "docs/"), ("examples/", "sdk/typescript/"), ("tests/", "docs/"),
            ("sdk/python/", "crates/"), ("installer/", "crates/"),
        })
        self.assertEqual(reads[("sdk/python/", "crates/")],
                         {"sdk/python/tests/test_wire.py -> crates/x/tests/fixtures/wire.json"})
        self.assertNotIn(("crates/*/src/", "sdk/python/"), CROSS_AREA_READS)
        self.assertEqual(reads[("installer/", "crates/")],
                         {"installer/launchkit/read_source.py -> crates/x/src/lib.rs"})
        self.assertNotIn(("installer/", "crates/"), CROSS_AREA_READS)

    def test_pushes_dispatches_and_empty_diffs_run_everything(self):
        self.assertEqual(self.lanes("README.md", event="push"), EVERY_LANE)
        self.assertEqual(self.lanes("console/src/App.tsx", event="push"), EVERY_LANE)
        self.assertEqual(self.lanes("README.md", event="workflow_dispatch"), EVERY_LANE)
        self.assertEqual(self.lanes(), EVERY_LANE)

    def test_a_rename_out_of_a_built_tree_runs_its_lanes(self):
        # Moving a component the console still imports into docs/ takes it
        # out of the console build. git reports the move as one rename, whose
        # destination alone is skippable.
        component = "export function ConsoleApp() {\n  return null;\n}\n" * 4
        index = "import { ConsoleApp } from './ConsoleApp';\n"
        base = {"console/src/ConsoleApp.tsx": component, "console/src/index.tsx": index}
        head = {"docs/ConsoleApp.tsx": component, "console/src/index.tsx": index}
        self.assertEqual(classify_commit(base, head), expected_output(*CONSOLE_LANES))
        # Moving a crate's file into docs/ runs the Rust lanes the same way.
        base = {"crates/x/src/lib.rs": component}
        head = {"docs/lib.rs": component}
        self.assertEqual(classify_commit(base, head), expected_output(*RUST_PR_LANES))
        # A deleted source is read from the base commit, so the feature its
        # code gated still runs.
        gated = '#[cfg(feature = "openai-live")]\nfn live() {}\n' * 4
        base = {"crates/x/src/live.rs": gated}
        head = {"docs/live.rs": gated}
        self.assertEqual(classify_commit(base, head), expected_output(*RUST_PR_LANES, "voice"))

    def test_a_docs_only_commit_read_from_git_skips_the_heavy_jobs(self):
        # Control for the test above: the same path from git to verdict
        # skips a commit that edits, adds and renames files within docs/.
        guide = "# Guide\n\nSteps.\n" * 8
        base = {"docs/guide.md": guide, "docs/old.md": "old\n", "crates/x.rs": "fn main() {}\n"}
        head = {
            "docs/guides/guide.md": guide,
            "docs/old.md": "new\n",
            "docs/added.md": "added\n",
            "crates/x.rs": "fn main() {}\n",
        }
        self.assertEqual(classify_commit(base, head), expected_output())

    def test_a_gated_source_read_from_the_checkout_runs_its_feature(self):
        # Control for the deleted-source case: an edited source is read from
        # the checked-out tree, gate or no gate.
        base = {"crates/x/src/a.rs": "fn a() {}\n", "crates/x/src/b.rs": "fn b() {}\n"}
        head = {"crates/x/src/a.rs": 'fn a() {}\n#[cfg(feature = "extension-state")]\nfn e() {}\n',
                "crates/x/src/b.rs": "fn b() {}\n"}
        self.assertEqual(classify_commit(base, head), expected_output(*RUST_PR_LANES, "extension_state"))
        head = {"crates/x/src/a.rs": "fn a() { let _ = 1; }\n", "crates/x/src/b.rs": "fn b() {}\n"}
        self.assertEqual(classify_commit(base, head), expected_output(*RUST_PR_LANES))

    def test_names_git_would_quote_are_read_verbatim(self):
        # Without -z, git quotes this name and no rule could match it.
        base = {"docs/notes.md": "x\n"}
        head = {"docs/notes.md": "x\n", "docs/caf\u00e9 notes.md": "y\n"}
        self.assertEqual(classify_commit(base, head), expected_output())

    def test_main_writes_the_github_output_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "output"
            ci_changes.main(["--event", "pull_request", "--github-output", str(out), "docs/a.md"])
            ci_changes.main(["--event", "pull_request", "--github-output", str(out), "flow-editor/x.ts"])
            ci_changes.main(["--event", "push", "--github-output", str(out), "docs/a.md"])
            self.assertEqual(
                out.read_text(),
                expected_output() + expected_output("flow_editor", "console") + expected_output(*EVERY_LANE),
            )

    def test_main_names_the_lanes_left_to_the_push(self):
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            ci_changes.main(["--event", "pull_request", "crates/meerkat-meta/src/memory/steward_prompt_v0.md"])
        self.assertIn("left to the push: voice extension_state python console flow_editor\n", printed.getvalue())
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            ci_changes.main(["--event", "push", "crates/meerkat-meta/src/memory/steward_prompt_v0.md"])
        self.assertNotIn("left to the push", printed.getvalue())

if __name__ == "__main__":
    unittest.main()
