#!/usr/bin/env python3
"""Published crates' tests pass standalone.

A published crate's tests are the tests a downloader runs: plain `cargo test`
inside the unpacked package, with no sibling checkout, no workspace feature
unification and no repository files. This script proves that for every crate
scripts/release-rust-crates.sh lists.

  published_crates_standalone.py census --out DIR [--crate NAME ...]
      [--stack mirror|default] [--allow-dirty]

      Package every selected crate, unpack the archives into a synthetic
      workspace whose [patch.crates-io] points each crate at its unpacked copy,
      and then, one crate at a time (so sibling crates cannot turn features
      on):
        - `cargo test -p C --no-run` with default features (the command a
          downloader types must build);
        - `cargo test -p C --features <declared>` passes, where <declared> is
          the crate's [package.metadata.meerkat.standalone-tests] features;
        - test-name parity: per test target, the standalone test names equal
          the workspace's names for the same package and features (plus
          `workspace-tests` when the crate declares it, as every workspace
          test build of the crate passes), minus the workspace-only tests
          ci/standalone-workspace-only.toml declares.
      --stack mirror (default) sets RUST_MIN_STACK to the workspace's
      .cargo/config.toml value; --stack default leaves Rust's own default, to
      record which targets need the larger stack. --out must not lie under a
      directory with a Cargo configuration (the repository's included): the
      standalone builds run inside it, and a downloader has none.

  published_crates_standalone.py guard [--crate NAME ...]

      Static check, no build. For every selected crate:
        - its [package] exclude and the manifest's `excluded` list agree;
        - no packaged Rust file reads outside the package: no include_* or
          #[path] that leaves the package root, no CARGO_MANIFEST_DIR walked
          up with parent() or ".." (inline, or through a same-file string
          constant), no workspace_root/repo_root helper;
        - no packaged test skips itself when a binary or artifact is missing;
        - code under a cfg that needs `workspace-tests` is not scanned: the
          package leaves that feature off, so the code never builds there;
        - the declared test features exist and leave `workspace-tests` off,
          and the packaged README.md prints the test command;
        - the crate's test features (the declared ones, and `workspace-tests`
          when it declares that feature) are on its Bazel unit-test target,
          its lib.rs carries the test-feature canary over exactly them, and
          no path-only self dev-dependency turns them on (the workspace's
          test commands pass them; the edge would make the crate's lib test
          wait for its own library).
      A flagged line that is not a test escape is listed under the crate's
      `guard_allow` with its reason.

  published_crates_standalone.py floor --product NAME --out DIR
      [--mode pr|release] [--allow-dirty] [--no-build]

      The published-floor check of a product other than Core (ci/products.json:
      meta, connect). In the one workspace a product compiles against the
      in-tree Core (and Connect against the in-tree Meta) through path
      dependencies; a published product requires the versions its manifests
      declare from crates.io. The floor check builds the product's crates the
      way a downloader resolves them:
        - package: a product with a publish order packages those crates
          (`cargo package --no-verify`, every in-repo crate patched to its
          path so packaging resolves) and weighs each crate file (see
          package-size below; a crate above the allowed size fails here,
          stage `package`); an unpublished product's manifests are
          normalized the same way (workspace inheritance resolved, `path`
          dropped from every dependency outside the product);
        - unpack them into a scratch workspace whose [patch.crates-io] covers
          only the product's own crates, with the repository lock as the
          seed and the repository's [env] (the forced LIBSQLITE3_FLAGS);
        - resolve: every other in-repo crate must come from crates.io at the
          declared version (a version crates.io lacks fails here, naming it:
          the product needs an unreleased dependency);
        - build: what a downloader compiles, the product's libraries and
          binaries: `cargo check --workspace --lib --bins` (--mode pr) or
          `cargo build --workspace --lib --bins` (--mode release). Test
          targets are not part of a published crate's contract, and a
          product's packaged tests may read files its package does not ship
          (Meta's lib tests read its tests/ directory, which its package
          leaves out).
      Writes floor.json to --out, with the sha256 of the scratch workspace's
      resolved Cargo.lock (lock_sha256) once resolution succeeded. Exit 1
      names the first failing stage. --no-build stops after the resolve
      stage (package, unpack, cargo metadata) and builds nothing: it only
      verifies that the declared versions resolve, and to which lock; like
      every stage it publishes nothing and writes its files only under --out.

  published_crates_standalone.py package-size --product NAME --package-dir DIR

      Weigh the crate files another step packaged into DIR (a Cargo target
      directory's `package/`): one archive for each crate the product
      publishes (ci/products.json), none for a product that publishes none.
      It packages nothing. crates.io refuses a crate file above 10 MiB; a
      file above 9.5 MiB fails here, with the heaviest top-level entries of
      its package. A missing archive is a usage error, never a pass. The
      census and the floor check weigh the archives they package the same
      way, before they unpack them.

Exit 0 = proven, 1 = a crate fails, 2 = usage error or unreadable input.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import zlib

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised only on Python < 3.11
    import tomli as tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = pathlib.Path("ci") / "standalone-workspace-only.toml"
# .cargo/config.toml [env] of the workspace; mirrored, never read from the
# package (a downloader has no repository config).
MIRRORED_MIN_STACK = "8388608"
DOC_TARGET = "doc"


class UsageError(Exception):
    pass


# ---------------------------------------------------------------------------
# Inputs
# ---------------------------------------------------------------------------


def release_crates(root: pathlib.Path) -> list[str]:
    script = root / "scripts" / "release-rust-crates.sh"
    out = subprocess.run([str(script)], check=True, capture_output=True, text=True).stdout
    return [line.strip() for line in out.splitlines() if line.strip()]


def crate_dirs(root: pathlib.Path) -> dict[str, pathlib.Path]:
    out = subprocess.run(
        [cargo(), "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(out)
    return {pkg["name"]: pathlib.Path(pkg["manifest_path"]).parent for pkg in meta["packages"]}


def load_manifest(root: pathlib.Path) -> dict:
    path = root / MANIFEST
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as err:
        raise UsageError(f"cannot read {MANIFEST}: {err}") from err
    crates = data.get("crates") or {}
    for name, entry in crates.items():
        for key in ("excluded", "workspace_only", "guard_allow"):
            for item in entry.get(key) or []:
                if not str(item.get("reason", "")).strip():
                    raise UsageError(f"{MANIFEST}: {name}.{key} entry {item} has no reason")
        for item in entry.get("workspace_only") or []:
            names = [*(item.get("modules") or []), *(item.get("tests") or [])]
            if not item.get("target") or not names or not all(isinstance(n, str) and n for n in names):
                raise UsageError(
                    f"{MANIFEST}: {name}.workspace_only entry for {item.get('target')!r} needs a target "
                    "and at least one module or test name"
                )
    return crates


def standalone_tests(manifest: dict) -> dict:
    meta = ((manifest.get("package") or {}).get("metadata") or {}).get("meerkat") or {}
    return meta.get("standalone-tests") or {}


def standalone_features(manifest: dict) -> list[str]:
    return list(standalone_tests(manifest).get("features") or [])


def standalone_min_stack(manifest: dict) -> int | None:
    value = standalone_tests(manifest).get("min-stack")
    return int(value) if value is not None else None


WORKSPACE_TESTS = "workspace-tests"


def test_command(manifest: dict) -> str | None:
    """The command a consumer runs in the published package, as the crate's
    README.md prints it; None when plain `cargo test` is the whole story."""
    features = standalone_features(manifest)
    stack = standalone_min_stack(manifest)
    if not features and stack is None:
        return None
    command = "cargo test" + (f" --features {','.join(features)}" if features else "")
    return f"RUST_MIN_STACK={stack} {command}" if stack is not None else command


def published_test_contract_failures(crate: str, manifest: dict, package_root: pathlib.Path, packaged: set[str]) -> list[str]:
    """The published test contract: the declared feature set exists and never
    turns on `workspace-tests`, which only the workspace's own test builds
    pass; and README.md, which the package carries, prints the exact test
    command."""
    failures = []
    features = (manifest.get("features") or {})
    for feature in standalone_features(manifest):
        if feature == WORKSPACE_TESTS:
            failures.append(f"{crate}: standalone-tests features must not include {WORKSPACE_TESTS}")
        elif feature not in features:
            failures.append(f"{crate}: standalone-tests feature {feature} is not declared in [features]")
    command = test_command(manifest)
    if command is not None:
        readme = package_root / "README.md"
        if "README.md" not in packaged:
            failures.append(f"{crate}: README.md must ship with the package and print `{command}`")
        elif command not in readme.read_text():
            failures.append(f"{crate}: README.md must print the standalone test command `{command}`")
    return failures


def cargo() -> str:
    return os.environ.get("CARGO_BIN", "cargo")


def bazel_unit_test_features(build_text: str, target: str) -> list[str] | None:
    """The crate_features of the generated `rust_test` named `target` in a
    BUILD.bazel, or None when there is no such target."""
    rule = re.search(r'\nrust_test\(\n    name = "' + re.escape(target) + r'",\n(.*?)\n\)\n', build_text, re.S)
    if rule is None:
        return None
    features = re.search(r'\n    crate_features = \[\n(.*?)\n    \]', rule.group(1), re.S)
    return re.findall(r'"([^"]+)"', features.group(1)) if features else []


def test_features(manifest: dict) -> list[str]:
    """The features every workspace test build of the crate passes: its
    declared standalone-test features, then `workspace-tests` when it declares
    that feature (scripts/ci-products.mjs testFeaturesOf)."""
    declared = list(dict.fromkeys(standalone_features(manifest)))
    if WORKSPACE_TESTS in (manifest.get("features") or {}):
        declared.append(WORKSPACE_TESTS)
    return declared


def bazel_workspace_tests_failures(crate: str, manifest: dict, package_root: pathlib.Path) -> list[str]:
    """A crate's Bazel unit-test target turns on every test feature of the
    crate (scripts/generate-bazel-rust-builds.mjs adds them to its feature
    union), or the Bazel lanes silently drop the tests they gate."""
    wanted = test_features(manifest)
    if not wanted:
        return []
    target = f"{crate.replace('-', '_')}_unit_test"
    try:
        build_text = (package_root / "BUILD.bazel").read_text()
    except OSError as err:
        return [f"{crate}: has test features but its BUILD.bazel cannot be read: {err}"]
    features = bazel_unit_test_features(build_text, target)
    if features is None:
        return [f"{crate}: has test features but BUILD.bazel has no {target}"]
    missing = [feature for feature in wanted if feature not in features]
    if missing:
        return [f"{crate}: BUILD.bazel {target} does not enable {', '.join(missing)}; run make buildbuddy-generate"]
    return []


CANARY_ENV = "MEERKAT_WORKSPACE_TEST_FEATURES"
# The canary in any whitespace layout (rustfmt splits a long cfg over lines):
# `#[cfg(...)]` (no `]` inside) on a module whose generic function asserts on
# the variable in an inline const, and a `#[used]` static that instantiates
# that function. A generic function's inline const is evaluated only where
# the function is instantiated, which only code generation does: a test
# binary trips the canary, `cargo check`, clippy and an editor's check run do
# not. A plain `const _` assert would trip all of them.
CANARY = re.compile(
    r'#\[cfg\((?P<cfg>[^\]]*)\)\]\s*'
    r'mod test_features_canary \{\s*'
    r'fn require<T>\(\) \{\s*'
    r'const \{\s*'
    r'let _ = size_of::<T>\(\);\s*'
    r'assert!\(\s*'
    r'option_env!\("' + CANARY_ENV + r'"\)\.is_none\(\),\s*'
    r'"(?P<message>(?:[^"\\]|\\.)*)"\s*,?\s*'
    r'\);\s*'
    r'\}\s*'
    r'\}\s*'
    r'#\[used\]\s*'
    r'static REQUIRE: fn\(\) = require::<\(\)>;\s*'
    r'\}',
    re.S,
)


def rust_string_value(literal: str) -> str:
    """A Rust string literal's text as the compiler reads it, for the escapes
    a canary message uses: a backslash at the end of a line drops the line
    break and the next line's leading whitespace, and `\\"` and `\\\\` stand
    for the character."""
    text = re.sub(r"\\\n\s*", "", literal)
    return re.sub(r'\\(["\\])', r"\1", text)


def canary_features(cfg: str) -> list[str] | None:
    """The features a canary's cfg, `all(test, not(PREDICATE))`, requires,
    where PREDICATE is `feature = "x"` or `all(feature = "x", ...)`; None for
    any other cfg. Whitespace is ignored."""
    compact = re.sub(r"\s+", "", cfg)
    whole = re.fullmatch(r'all\(test,not\((?P<predicate>.*)\)\)', compact)
    if not whole:
        return None
    predicate = whole.group("predicate")
    one = re.fullmatch(r'feature="([^"]+)"', predicate)
    if one:
        return [one.group(1)]
    many = re.fullmatch(r'all\((feature="[^"]+"(?:,feature="[^"]+")+),?\)', predicate)
    if many:
        return re.findall(r'feature="([^"]+)"', many.group(1))
    return None


def canary_failures(crate: str, manifest: dict, package_root: pathlib.Path) -> list[str]:
    """A crate with test features carries exactly one test-feature canary in
    its lib.rs, over exactly those features, whose message names the command
    to run (`cargo test -p <crate> --features <them>`): in the repository
    (.cargo/config.toml [env] sets the variable) a test build without them
    fails to compile instead of compiling the tests they gate out. A crate
    without test features carries none."""
    wanted = test_features(manifest)
    try:
        text = (package_root / "src" / "lib.rs").read_text()
    except OSError as err:
        return [f"{crate}: has test features but its src/lib.rs cannot be read: {err}"] if wanted else []
    found = list(CANARY.finditer(text))
    reads = text.count(f'option_env!("{CANARY_ENV}")')
    if not wanted:
        return [f"{crate}: has no test features but its lib.rs carries a test-feature canary"] if reads else []
    if len(found) != 1 or reads != 1:
        return [
            f"{crate}: lib.rs must carry exactly one test-feature canary (a cfg(all(test, not(...))) "
            f"test_features_canary module asserting on {CANARY_ENV} in a generic function's inline const, "
            "and the #[used] static that instantiates it)"
        ]
    features = canary_features(found[0].group("cfg"))
    if features is None or sorted(features) != sorted(wanted):
        return [f"{crate}: the lib.rs canary requires {features}, not the crate's test features {wanted}"]
    command = f"cargo test -p {crate} --features {','.join(wanted)}"
    if f"`{command}`" not in rust_string_value(found[0].group("message")):
        return [f"{crate}: the lib.rs canary message must name `{command}`"]
    return []


def self_dev_dependency_failures(crate: str, manifest: dict) -> list[str]:
    """No crate dev-depends on itself: the workspace's test commands pass its
    test features, and a self edge would make its lib test wait for its own
    library (and swallow `--features <crate>/<feature>` in non-test builds)."""
    tables = [manifest.get("dev-dependencies") or {}]
    for target in (manifest.get("target") or {}).values():
        tables.append((target or {}).get("dev-dependencies") or {})
    failures = []
    for table in tables:
        for key, spec in table.items():
            name = spec.get("package", key) if isinstance(spec, dict) else key
            if name == crate or (isinstance(spec, dict) and spec.get("path") == "."):
                failures.append(
                    f"{crate}: dev-depends on itself ({key}); the workspace's test commands pass its test features "
                    "(scripts/core-workspace-args.mjs --tests)"
                )
    return failures


def toolchain(root: pathlib.Path) -> str | None:
    path = root / "rust-toolchain.toml"
    if not path.exists():
        return None
    return ((tomllib.loads(path.read_text()).get("toolchain") or {}).get("channel")) or None


# ---------------------------------------------------------------------------
# Guard (static)
# ---------------------------------------------------------------------------

INCLUDE_MACRO = re.compile(r'\b(include_str|include_bytes|include)!\s*\(\s*"([^"]+)"')
INCLUDE_FROM_MANIFEST = re.compile(
    r'\b(include_str|include_bytes|include)!\s*\(\s*concat!\s*\(\s*env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"([^"]+)"'
)
PATH_ATTR = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')
MANIFEST_DIR = re.compile(r'CARGO_MANIFEST_DIR')
# A same-file string constant: a statement that joins CARGO_MANIFEST_DIR with
# the constant's name walks wherever the constant's literal does.
STR_CONST = re.compile(r'\b(?:const|static)\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&\s*(?:\'static\s+)?str\s*=\s*"([^"\\]*)"\s*;')
ROOT_HELPER = re.compile(r'\bfn\s+(workspace_root|repo_root|repository_root)\s*\(')
SKIP_MISSING = re.compile(r'"Skipping[^"]*\b(missing|not found|not built)\b', re.IGNORECASE)
STATEMENT_WINDOW = 400


def packaged_files(root: pathlib.Path, crate: str) -> list[str]:
    proc = subprocess.run(
        [cargo(), "package", "--list", "-p", crate, "--allow-dirty"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise UsageError(f"cargo package --list -p {crate} failed:\n{proc.stderr.strip()}")
    return [line.strip() for line in proc.stdout.splitlines() if line.strip()]


def strip_comments(text: str) -> str:
    """Blank Rust comments (keeping newlines, so line numbers hold) and keep
    string literals, so a pattern quoted in documentation is not a finding."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            out.append(" " * (end - i))
            i = end
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append("".join(ch if ch == "\n" else " " for ch in text[i:j]))
            i = j
        elif (c == "r" or text.startswith("br", i)) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            j = i + (2 if text.startswith("br", i) else 1)
            hashes = 0
            while j < n and text[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and text[j] == '"':
                close = '"' + "#" * hashes
                end = text.find(close, j + 1)
                end = n if end < 0 else end + len(close)
                out.append(text[i:end])
                i = end
            else:
                out.append(c)
                i += 1
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(text[i : j + 1])
            i = j + 1
        elif c == "'" and i + 2 < n and (text[i + 2] == "'" or text[i + 1] == "\\"):
            end = text.find("'", i + 2)
            end = n if end < 0 else end + 1
            out.append(text[i:end])
            i = end
        else:
            out.append(c)
            i += 1
    return "".join(out)


CFG_ATTR = re.compile(r"#\s*(!?)\s*\[\s*cfg\s*\(")
CFG_TOKEN = re.compile(r'\s*(?:([A-Za-z_][A-Za-z0-9_]*)|("(?:[^"\\]|\\.)*")|([(),=]))')
ATTR_OPEN = re.compile(r"\s*#\s*!?\s*\[")
ITEM_START = re.compile(
    r"(?:pub\s*(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern(?:\s+\"[^\"]*\")?)\s+)*"
    r"(?:(?:fn|mod|impl|struct|enum|union|trait|use|type|static|const|crate)\b|macro_rules\s*!)"
)


def cfg_requires(predicate: str, feature: str) -> bool:
    """Whether a `cfg(...)` predicate holds only with `feature` on: the feature
    itself, an all() with such a member, an any() whose members all need it.
    not(), other names and anything unparsed do not need it."""
    tokens: list[str] = []
    pos = 0
    while predicate[pos:].strip():
        match = CFG_TOKEN.match(predicate, pos)
        if not match:
            return False
        tokens.append(match.group(1) or match.group(2) or match.group(3))
        pos = match.end()

    def parse(i: int) -> tuple[bool, int]:
        name = tokens[i]
        if not (name[0].isalpha() or name[0] == "_"):
            raise ValueError(name)
        i += 1
        if i < len(tokens) and tokens[i] == "=":
            value = tokens[i + 1]
            if not value.startswith('"'):
                raise ValueError(value)
            return name == "feature" and value[1:-1] == feature, i + 2
        if i < len(tokens) and tokens[i] == "(":
            i += 1
            members: list[bool] = []
            while tokens[i] != ")":
                needs, i = parse(i)
                members.append(needs)
                if tokens[i] == ",":
                    i += 1
            if name == "all":
                return any(members), i + 1
            if name == "any":
                return bool(members) and all(members), i + 1
            return False, i + 1
        return False, i

    try:
        needs, end = parse(0)
    except (ValueError, IndexError):
        return False
    return needs and end == len(tokens)


def skip_literal(text: str, i: int) -> int | None:
    """The index after the string, raw string or char literal that starts at
    `i`, or None when none starts there (a lifetime is not a literal)."""
    n, c = len(text), text[i]
    if c in "rb" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
        j = i + 2 if text.startswith("br", i) else i + 1
        if text[j - 1] == "r":
            hashes = 0
            while j < n and text[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and text[j] == '"':
                end = text.find('"' + "#" * hashes, j + 1)
                return n if end < 0 else end + 1 + hashes
            return None
        return skip_literal(text, j) if j < n and text[j] in "\"'" else None
    if c == '"':
        j = i + 1
        while j < n and text[j] != '"':
            j += 2 if text[j] == "\\" else 1
        return min(j + 1, n)
    if c == "'" and i + 2 < n and text[i + 1] == "\\":
        end = text.find("'", i + 3)
        return n if end < 0 else end + 1
    if c == "'" and i + 2 < n and text[i + 2] == "'":
        return i + 3
    return None


def matching_close(text: str, i: int) -> int | None:
    """The index of the bracket that closes the one at `i`, skipping literals."""
    depth, n = 0, len(text)
    while i < n:
        end = skip_literal(text, i)
        if end is not None:
            i = end
            continue
        if text[i] in "([{":
            depth += 1
        elif text[i] in ")]}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def skip_attributes(text: str, i: int) -> int:
    """The index of the first code after the whitespace and attributes at `i`."""
    while match := ATTR_OPEN.match(text, i):
        close = matching_close(text, match.end() - 1)
        if close is None:
            return len(text)
        i = close + 1
    while i < len(text) and text[i].isspace():
        i += 1
    return i


def item_end(text: str, i: int) -> int | None:
    """The index after the item whose attributes end at `i`: its block or its
    `;`. None when the attributes carry a statement, an expression or anything
    else that is not an item, or when the item does not close."""
    i = skip_attributes(text, i)
    if not ITEM_START.match(text, i):
        return None
    depth, n = 0, len(text)
    while i < n:
        end = skip_literal(text, i)
        if end is not None:
            i = end
            continue
        c = text[i]
        if c == "{" and depth == 0:
            close = matching_close(text, i)
            return None if close is None else close + 1
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth < 0:
                return None
        elif c == ";" and depth == 0:
            return i + 1
        i += 1
    return None


def blank_compiled_out(text: str, feature: str = WORKSPACE_TESTS) -> str:
    """Blank (keeping newlines, so line numbers hold) the code a cfg that needs
    `feature` compiles out: the whole file when its leading inner attribute
    needs it, and each item (fn, mod, impl, use, ...) whose outer attribute
    needs it. The package never turns `workspace-tests` on, so that code never
    builds there. A cfg on a statement or an expression keeps its code, so a
    misread extent can only keep code in the scan, never hide it."""
    out = list(text)
    pos = 0
    while match := CFG_ATTR.search(text, pos):
        close = matching_close(text, match.end() - 1)
        attr = re.compile(r"\s*\]").match(text, close + 1) if close is not None else None
        if attr is None:
            pos = match.end()
            continue
        pos = attr.end()
        if not cfg_requires(text[match.end() : close], feature):
            continue
        if match.group(1):
            if skip_attributes(text, 0) >= pos:
                return "".join(ch if ch == "\n" else " " for ch in text)
            continue
        end = item_end(text, pos)
        if end is None:
            continue
        for k in range(match.start(), end):
            if out[k] != "\n":
                out[k] = " "
        pos = end
    return "".join(out)


def ships(packaged: set[str], base: str, literal: str) -> bool:
    """Whether a path a source file names (relative to `base`, a package-relative
    directory) is a file the package ships. Normalised lexically: cargo package
    copies a symlink's target, so the link's place in the package is what counts."""
    joined = os.path.normpath(os.path.join(base, literal))
    return not joined.startswith("..") and joined in packaged


def scan_source(packaged: set[str], relative: str, text: str) -> list[tuple[str, int, str]]:
    """Return (kind, line, excerpt) for every guard finding in one packaged
    file, given with its comments stripped. Code the package compiles out is
    not scanned (see blank_compiled_out)."""
    text = blank_compiled_out(text)
    findings: list[tuple[str, int, str]] = []
    base = os.path.dirname(relative)
    consts = {match.group(1): match.group(2) for match in STR_CONST.finditer(text)}

    def line_of(offset: int) -> int:
        return text.count("\n", 0, offset) + 1

    for match in INCLUDE_MACRO.finditer(text):
        if not ships(packaged, base, match.group(2)):
            findings.append(("include-escape", line_of(match.start()), match.group(0)))
    for match in INCLUDE_FROM_MANIFEST.finditer(text):
        if not ships(packaged, "", match.group(2).lstrip("/")):
            findings.append(("include-escape", line_of(match.start()), match.group(0)))
    for match in PATH_ATTR.finditer(text):
        if not ships(packaged, base, match.group(1)):
            findings.append(("path-escape", line_of(match.start()), match.group(0)))
    for match in MANIFEST_DIR.finditer(text):
        window = text[match.start() : match.start() + STATEMENT_WINDOW]
        statement = window.split(";", 1)[0]
        named = [literal for name, literal in consts.items() if re.search(rf"\b{re.escape(name)}\b", statement)]
        if (".parent()" in statement or '".."' in statement or '"../' in statement
                or any(literal == ".." or literal.startswith("../") or "/../" in literal for literal in named)):
            findings.append(("manifest-dir-walk", line_of(match.start()), statement.strip()[:120]))
    for match in ROOT_HELPER.finditer(text):
        findings.append(("root-helper", line_of(match.start()), match.group(0)))
    for match in SKIP_MISSING.finditer(text):
        findings.append(("skip-when-missing", line_of(match.start()), match.group(0)))
    return findings


def guard(root: pathlib.Path, crates: list[str]) -> int:
    manifest = load_manifest(root)
    dirs = crate_dirs(root)
    failures: list[str] = []
    for crate in crates:
        entry = manifest.get(crate) or {}
        package_root = dirs[crate]
        cargo_manifest = tomllib.loads((package_root / "Cargo.toml").read_text())
        excludes = set((cargo_manifest.get("package") or {}).get("exclude") or [])
        declared = {item["path"] for item in entry.get("excluded") or []}
        for path in sorted(excludes - declared):
            failures.append(f"{crate}: [package] exclude {path} is not in {MANIFEST} excluded")
        for path in sorted(declared - excludes):
            failures.append(f"{crate}: {MANIFEST} excluded {path} is not in [package] exclude")
        for path in sorted(declared):
            if not (package_root / path).exists():
                failures.append(f"{crate}: excluded {path} does not exist")
        allowed = {(item["path"], item["kind"]) for item in entry.get("guard_allow") or []}
        used: set[tuple[str, str]] = set()
        packaged = set(packaged_files(root, crate))
        for relative in sorted(packaged):
            if not relative.endswith(".rs"):
                continue
            text = strip_comments((package_root / relative).read_text(errors="replace"))
            for kind, line, excerpt in scan_source(packaged, relative, text):
                if (relative, kind) in allowed:
                    used.add((relative, kind))
                    continue
                failures.append(f"{crate}: {relative}:{line}: {kind}: {excerpt}")
        for path, kind in sorted(allowed - used):
            failures.append(f"{crate}: guard_allow {path} {kind} matches nothing (stale entry)")
        failures.extend(published_test_contract_failures(crate, cargo_manifest, package_root, packaged))
        failures.extend(bazel_workspace_tests_failures(crate, cargo_manifest, package_root))
        failures.extend(canary_failures(crate, cargo_manifest, package_root))
        failures.extend(self_dev_dependency_failures(crate, cargo_manifest))
        print(f"  {crate:<36}{'FAIL' if any(f.startswith(crate + ':') for f in failures) else 'OK'}")
    for failure in failures:
        print(f"FAIL {failure}", file=sys.stderr)
    return 1 if failures else 0


# ---------------------------------------------------------------------------
# Census (build and run)
# ---------------------------------------------------------------------------

RUNNING = re.compile(r"^\s+Running (?:unittests )?(\S+) \(")
DOC_TESTS = re.compile(r"^\s+Doc-tests (\S+)")
LISTED = re.compile(r"^(.+): test$")


def parse_listing(output: str, package_dir: str = "") -> dict[str, set[str]]:
    """Map test target (source path, or "doc") to its test names.

    rustdoc names a doc test by its source path relative to the workspace
    root, which differs between the repository (`crates/<dir>/src/..`) and the
    unpacked package (`<name>-<version>/src/..`); `package_dir` is that prefix,
    stripped so both name the same test the same way."""
    names: dict[str, set[str]] = {}
    current: str | None = None
    prefix = package_dir.rstrip("/") + "/" if package_dir else ""
    for line in output.splitlines():
        running = RUNNING.match(line)
        if running:
            current = running.group(1)
            names.setdefault(current, set())
            continue
        if DOC_TESTS.match(line):
            current = DOC_TARGET
            names.setdefault(current, set())
            continue
        listed = LISTED.match(line)
        if listed and current is not None:
            name = listed.group(1)
            if current == DOC_TARGET and prefix and name.startswith(prefix):
                name = name[len(prefix) :]
            names[current].add(name)
    return names


def workspace_only_names(entry: dict, target: str, names: set[str]) -> set[str]:
    """The names in `target` the manifest declares workspace-only: each named
    test, and every test under each named module path."""
    out: set[str] = set()
    for item in entry.get("workspace_only") or []:
        if item.get("target") != target:
            continue
        out |= names & set(item.get("tests") or [])
        for module in item.get("modules") or []:
            prefix = module + "::"
            out |= {name for name in names if name.startswith(prefix)}
    return out


def stale_workspace_only(entry: dict, workspace: dict[str, set[str]]) -> list[str]:
    """Declared workspace-only names that no workspace test matches any more
    (renamed or removed): a stale declaration would hide nothing today but
    could hide a new test that takes the name."""
    problems: list[str] = []
    for item in entry.get("workspace_only") or []:
        target = item.get("target")
        names = workspace.get(target, set())
        for test in item.get("tests") or []:
            if test not in names:
                problems.append(f"{target}: declared workspace-only test {test} is not a workspace test")
        for module in item.get("modules") or []:
            if not any(name.startswith(module + "::") for name in names):
                problems.append(f"{target}: declared workspace-only module {module} holds no workspace test")
    return problems


def parity(entry: dict, workspace: dict[str, set[str]], standalone: dict[str, set[str]]) -> list[str]:
    """Differences between the standalone names and the workspace names the package keeps."""
    excluded_targets = {item["path"] for item in entry.get("excluded") or []}
    problems: list[str] = stale_workspace_only(entry, workspace)
    for target in sorted(set(workspace) | set(standalone)):
        expected = set() if target in excluded_targets else set(workspace.get(target, set()))
        expected -= workspace_only_names(entry, target, expected)
        got = standalone.get(target, set())
        for name in sorted(expected - got):
            problems.append(f"{target}: {name} runs in the workspace but not standalone")
        for name in sorted(got - expected):
            problems.append(f"{target}: {name} runs standalone but not in the workspace")
    return problems


def run(cmd: list[str], cwd: pathlib.Path, env: dict[str, str], log: pathlib.Path) -> tuple[int, str]:
    proc = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    log.write_text(f"$ {' '.join(cmd)}\n# cwd {cwd}\n{proc.stdout}\n# rc {proc.returncode}\n")
    return proc.returncode, proc.stdout


def build_env(stack: str, target_dir: pathlib.Path, rust_toolchain: str | None) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in ("RUST_MIN_STACK", "CARGO_TARGET_DIR")}
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["CARGO_PROFILE_TEST_DEBUG"] = "0"
    env["CARGO_PROFILE_DEV_DEBUG"] = "0"
    env["CARGO_TERM_COLOR"] = "never"
    if rust_toolchain:
        env["RUSTUP_TOOLCHAIN"] = rust_toolchain
    if stack == "mirror":
        env["RUST_MIN_STACK"] = MIRRORED_MIN_STACK
    return env


# ---------------------------------------------------------------------------
# Package size (the registry's upload limit)
# ---------------------------------------------------------------------------

# The largest crate file crates.io accepts. The figure is the registry's own,
# from its refusal of an upload (HTTP 413): "max upload size is: 10485760".
REGISTRY_MAX_CRATE_BYTES = 10_485_760
# The largest crate file these checks accept: 9.5 MiB. Below the registry's
# limit, so the failure comes while the package can still be trimmed in an
# ordinary change, not at a release.
ALLOWED_MAX_CRATE_BYTES = 9_961_472
LARGEST_ENTRIES_SHOWN = 6


class OversizedPackage(Exception):
    """A packaged crate file is above ALLOWED_MAX_CRATE_BYTES; `problems` are
    package_size_problems' lines."""

    def __init__(self, problems: list[str]):
        super().__init__("\n".join(problems))
        self.problems = problems


def packaged_archives(package_dir: pathlib.Path, crates: list[str]) -> dict[str, pathlib.Path]:
    """The one crate file `cargo package` wrote into `package_dir` for each
    crate."""
    found: dict[str, pathlib.Path] = {}
    for name in crates:
        archives = sorted(package_dir.glob(f"{name}-[0-9]*.crate"))
        if len(archives) != 1:
            raise UsageError(f"expected one archive for {name} in {package_dir}, found {len(archives)}")
        found[name] = archives[0]
    return found


def largest_entries(archive: pathlib.Path) -> list[tuple[str, int, int]]:
    """The top-level directories and files of a package that weigh most in
    its crate file: (name, bytes unpacked, bytes compressed), heaviest
    compressed first. Each file is compressed on its own, so the compressed
    figures are approximate (a little above its share of the archive)."""
    totals: dict[str, list[int]] = {}
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle:
            parts = pathlib.PurePosixPath(member.name).parts[1:]
            if not member.isfile() or not parts:
                continue
            total = totals.setdefault(f"{parts[0]}/" if len(parts) > 1 else parts[0], [0, 0])
            total[0] += member.size
            total[1] += len(zlib.compress(bundle.extractfile(member).read(), 6))
    ranked = sorted(totals.items(), key=lambda item: (-item[1][1], item[0]))
    return [(name, unpacked, packed) for name, (unpacked, packed) in ranked[:LARGEST_ENTRIES_SHOWN]]


def package_size_problems(archives: dict[str, pathlib.Path]) -> list[str]:
    """Why crates.io would refuse, or soon refuse, a crate file for its size:
    for each one above ALLOWED_MAX_CRATE_BYTES a line with the crate, its
    size and both limits, then the heaviest top-level entries of its package
    (what to trim). Empty when every crate file fits."""
    problems: list[str] = []
    for crate, archive in archives.items():
        size = archive.stat().st_size
        if size <= ALLOWED_MAX_CRATE_BYTES:
            continue
        problems.append(
            f"{crate}: {archive.name} is {size} bytes, {size - ALLOWED_MAX_CRATE_BYTES} above the allowed maximum of "
            f"{ALLOWED_MAX_CRATE_BYTES} bytes ({ALLOWED_MAX_CRATE_BYTES / 1048576:g} MiB); crates.io refuses a crate "
            f"file above {REGISTRY_MAX_CRATE_BYTES} bytes ({REGISTRY_MAX_CRATE_BYTES / 1048576:g} MiB). Trim the "
            f"package ([package] exclude in the crate's Cargo.toml). Its heaviest top-level entries:"
        )
        problems += [f"    {name:<24} about {packed} bytes compressed, {unpacked} unpacked"
                     for name, unpacked, packed in largest_entries(archive)]
    return problems


def print_size_problems(problems: list[str]) -> None:
    """package_size_problems' lines on stderr: FAIL before each crate's line,
    its entries below it."""
    for problem in problems:
        print(problem if problem.startswith(" ") else f"FAIL {problem}", file=sys.stderr)


def package_size(root: pathlib.Path, product_name: str, package_dir: pathlib.Path) -> int:
    order = load_product(root, product_name).get("publish_order")
    if not isinstance(order, list) or not order:
        print(f"{product_name} publishes no crate: no package to weigh")
        return 0
    archives = packaged_archives(package_dir, order)
    for crate, archive in archives.items():
        print(f"  {crate:<36}{archive.stat().st_size} bytes "
              f"(allowed {ALLOWED_MAX_CRATE_BYTES}, registry limit {REGISTRY_MAX_CRATE_BYTES})")
    problems = package_size_problems(archives)
    print_size_problems(problems)
    return 1 if problems else 0


def package_and_unpack(root: pathlib.Path, crates: list[str], all_crates: list[str], dirs: dict[str, pathlib.Path],
                       out: pathlib.Path, allow_dirty: bool, env: dict[str, str]) -> dict[str, pathlib.Path]:
    """Package `crates` and unpack their archives into out/ws. Raises
    OversizedPackage, before anything is unpacked, when a crate file is above
    the allowed size."""
    patch = out / "package-patch.toml"
    patch.write_text(
        "[patch.crates-io]\n"
        + "".join(f'{name} = {{ path = "{dirs[name]}" }}\n' for name in all_crates)
    )
    cmd = [cargo(), "package", "--no-verify", "--config", str(patch)]
    if allow_dirty:
        cmd.append("--allow-dirty")
    for name in crates:
        cmd += ["-p", name]
    rc, output = run(cmd, root, env, out / "package.log")
    if rc != 0:
        raise UsageError(f"cargo package failed (see {out / 'package.log'})")
    version = None
    archives = packaged_archives(pathlib.Path(env["CARGO_TARGET_DIR"]) / "package", crates)
    problems = package_size_problems(archives)
    if problems:
        raise OversizedPackage(problems)
    ws = out / "ws"
    ws.mkdir()
    unpacked: dict[str, pathlib.Path] = {}
    for name, archive in archives.items():
        with tarfile.open(archive, "r:gz") as bundle:
            bundle.extractall(ws, filter="data") if hasattr(tarfile, "data_filter") else bundle.extractall(ws)
        unpacked[name] = ws / archive.name[: -len(".crate")]
        version = version or archive.name[len(name) + 1 : -len(".crate")]
    members = "".join(f'    "{path.name}",\n' for path in unpacked.values())
    patches = "".join(f'{name} = {{ path = "{path.name}" }}\n' for name, path in unpacked.items())
    (ws / "Cargo.toml").write_text(
        f'[workspace]\nresolver = "2"\nmembers = [\n{members}]\n\n[patch.crates-io]\n{patches}'
    )
    shutil.copyfile(root / "Cargo.lock", ws / "Cargo.lock")
    return unpacked


def cargo_config_above(path: pathlib.Path) -> pathlib.Path | None:
    """The first Cargo configuration file Cargo would read for a build run in
    `path` from the directories above it (`.cargo/config.toml` or
    `.cargo/config` of `path` or an ancestor), or None. The user's own
    $CARGO_HOME configuration is not one: a downloader has that too."""
    home = pathlib.Path(os.environ.get("CARGO_HOME") or pathlib.Path.home() / ".cargo").resolve()
    own = {home / "config.toml", home / "config"}
    for directory in [path, *path.parents]:
        for name in ("config.toml", "config"):
            candidate = directory / ".cargo" / name
            if candidate.is_file() and candidate.resolve() not in own:
                return candidate
    return None


def census(root: pathlib.Path, crates: list[str], out: pathlib.Path, stack: str, allow_dirty: bool) -> int:
    manifest = load_manifest(root)
    all_crates = release_crates(root)
    dirs = crate_dirs(root)
    rust_toolchain = toolchain(root)
    # The standalone builds run in out/ws: a Cargo configuration above it (the
    # repository's [env], for one) would reach them, and a downloader has none.
    config = cargo_config_above(out.resolve())
    if config is not None:
        raise UsageError(
            f"--out {out} lies under {config.parent.parent}, whose Cargo configuration ({config}) the standalone "
            "builds would read; choose a directory outside the repository"
        )
    out.mkdir(parents=True, exist_ok=False)
    logs = out / "logs"
    logs.mkdir()
    standalone_env = build_env(stack, out / "target-standalone", rust_toolchain)
    workspace_env = build_env(stack, out / "target-workspace", rust_toolchain)
    # Every release crate is unpacked: the selected crates' dependencies must
    # resolve to the unpacked siblings, never to an older registry release.
    unpacked = package_and_unpack(root, all_crates, all_crates, dirs, out, allow_dirty, standalone_env)
    ws = out / "ws"
    rows: list[dict] = []
    failed = False
    for crate in crates:
        entry = manifest.get(crate) or {}
        packaged = tomllib.loads((unpacked[crate] / "Cargo.toml").read_text())
        features = standalone_features(packaged)
        feature_args = ["--features", ",".join(features)] if features else []
        row: dict = {"crate": crate, "features": features, "stack": stack, "problems": []}

        rc, _ = run([cargo(), "test", "-p", crate, "--no-run"], ws, standalone_env, logs / f"{crate}.naive.log")
        row["naive_build"] = rc == 0
        if rc != 0:
            row["problems"].append("default-feature `cargo test --no-run` does not build")

        rc, output = run([cargo(), "test", "-p", crate, *feature_args, "--no-fail-fast"], ws, standalone_env,
                         logs / f"{crate}.test.log")
        row["tests_pass"] = rc == 0
        row["results"] = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
        if rc != 0:
            row["problems"].append("`cargo test` with the declared features fails")

        rc_s, standalone_listing = run([cargo(), "test", "-p", crate, *feature_args, "--", "--list", "--format", "terse"],
                                       ws, standalone_env, logs / f"{crate}.list-standalone.log")
        # The workspace runs the crate's tests with `workspace-tests` too when
        # it declares that feature: every workspace test build passes it (no
        # dev-dependency does), and the parity below takes the tests it gates
        # out again (ci/standalone-workspace-only.toml).
        workspace_features = [*features, *([WORKSPACE_TESTS] if WORKSPACE_TESTS in (packaged.get("features") or {}) else [])]
        workspace_feature_args = ["--features", ",".join(workspace_features)] if workspace_features else []
        rc_w, workspace_listing = run([cargo(), "test", "-p", crate, *workspace_feature_args, "--", "--list", "--format", "terse"],
                                      root, workspace_env, logs / f"{crate}.list-workspace.log")
        if rc_s != 0 or rc_w != 0:
            row["problems"].append(f"listing failed (standalone rc {rc_s}, workspace rc {rc_w})")
        else:
            standalone = parse_listing(standalone_listing, unpacked[crate].name)
            workspace = parse_listing(workspace_listing, str(dirs[crate].relative_to(root)))
            row["targets"] = {t: len(n) for t, n in sorted(standalone.items())}
            row["problems"] += parity(entry, workspace, standalone)
        failed |= bool(row["problems"])
        rows.append(row)
        print(f"  {crate:<36}{'FAIL' if row['problems'] else 'OK'}", flush=True)
        for problem in row["problems"]:
            print(f"      {problem}", flush=True)
    (out / "census.json").write_text(json.dumps(rows, indent=2) + "\n")
    return 1 if failed else 0


# ---------------------------------------------------------------------------
# Published floor (one product against the registry)
# ---------------------------------------------------------------------------

PRODUCTS = pathlib.Path("ci") / "products.json"


def load_product(root: pathlib.Path, name: str) -> dict:
    try:
        products = json.loads((root / PRODUCTS).read_text())["products"]
    except (OSError, ValueError, KeyError) as err:
        raise UsageError(f"cannot read {PRODUCTS}: {err}") from err
    for product in products:
        if product.get("name") == name:
            if not product.get("root"):
                raise UsageError("Core has no published floor: it is the dependency the floor checks")
            return product
    raise UsageError(f"{PRODUCTS} lists no product {name!r}")


def workspace_members(root: pathlib.Path) -> list[dict]:
    out = subprocess.run(
        [cargo(), "metadata", "--no-deps", "--format-version", "1"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    meta = json.loads(out)
    members = set(meta["workspace_members"])
    return [pkg for pkg in meta["packages"] if pkg["id"] in members]


def under(path: pathlib.Path, root: pathlib.Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def toml_value(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{json.dumps(key)} = {toml_value(item)}" for key, item in value.items()) + " }"
    raise UsageError(f"cannot write {value!r} as TOML")


def toml_document(document: dict) -> str:
    """A manifest as TOML: scalar keys first, then tables, then arrays of
    tables (the subset Cargo manifests use)."""
    lines: list[str] = []

    def table(prefix: str, body: dict) -> None:
        scalars = {k: v for k, v in body.items() if not isinstance(v, dict) and not (isinstance(v, list) and v and all(isinstance(i, dict) for i in v))}
        tables = {k: v for k, v in body.items() if isinstance(v, dict)}
        arrays = {k: v for k, v in body.items() if isinstance(v, list) and v and all(isinstance(i, dict) for i in v)}
        if prefix and (scalars or not tables):
            lines.append(f"[{prefix}]")
        for key, value in scalars.items():
            lines.append(f"{json.dumps(key)} = {toml_value(value)}")
        if prefix and (scalars or not tables):
            lines.append("")
        for key, value in tables.items():
            table(f"{prefix}.{json.dumps(key)}" if prefix else json.dumps(key), value)
        for key, items in arrays.items():
            for item in items:
                lines.append(f"[[{prefix + '.' if prefix else ''}{json.dumps(key)}]]")
                for k, v in item.items():
                    lines.append(f"{json.dumps(k)} = {toml_value(v)}")
                lines.append("")

    table("", document)
    return "\n".join(lines) + "\n"


def normalized_dependency(name: str, spec, workspace_deps: dict, kept_paths: dict[pathlib.Path, str],
                          crate_dir: pathlib.Path, root: pathlib.Path):
    """A dependency as `cargo package` writes it: workspace inheritance
    resolved, and no `path` for a crate outside the product (`kept_paths`
    maps each product crate's directory to its path in the scratch
    workspace)."""
    if isinstance(spec, str):
        return spec
    spec = dict(spec)
    if spec.pop("workspace", False):
        inherited = workspace_deps.get(name)
        if inherited is None:
            raise UsageError(f"{crate_dir.name}: {name} inherits a workspace dependency the root does not declare")
        inherited = {"version": inherited} if isinstance(inherited, str) else dict(inherited)
        member_features = spec.pop("features", [])
        merged = {**inherited, **spec}
        if member_features or inherited.get("features"):
            merged["features"] = sorted(set(inherited.get("features", [])) | set(member_features))
        spec = merged
        base = root
    else:
        base = crate_dir
    if "path" in spec:
        target = (base / spec["path"]).resolve()
        if target in kept_paths:
            spec["path"] = kept_paths[target]
        else:
            spec.pop("path")
            if "version" not in spec:
                raise UsageError(f"{crate_dir.name}: {name} is a path dependency with no version, so no published build can resolve it")
    return spec


def normalized_manifest(crate_dir: pathlib.Path, root: pathlib.Path, kept_paths: dict[pathlib.Path, str]) -> dict:
    root_manifest = tomllib.loads((root / "Cargo.toml").read_text())
    workspace_package = root_manifest["workspace"].get("package", {})
    workspace_deps = root_manifest["workspace"].get("dependencies", {})
    manifest = tomllib.loads((crate_dir / "Cargo.toml").read_text())
    package = manifest["package"]
    for key, value in list(package.items()):
        if isinstance(value, dict) and value.get("workspace") is True:
            package[key] = workspace_package[key]
    package.pop("workspace", None)

    def dependencies(table: dict) -> dict:
        return {name: normalized_dependency(name, spec, workspace_deps, kept_paths, crate_dir, root)
                for name, spec in table.items()}

    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        if key in manifest:
            manifest[key] = dependencies(manifest[key])
    for target in (manifest.get("target") or {}).values():
        for key in ("dependencies", "dev-dependencies", "build-dependencies"):
            if key in target:
                target[key] = dependencies(target[key])
    if (manifest.get("lints") or {}).get("workspace") is True:
        manifest["lints"] = root_manifest["workspace"].get("lints", {})
    return manifest


def surface_flags(metadata: dict, members) -> list[str]:
    """`--lib` and `--bins` for the floor members' published surface: each
    flag only when some member has such a target (Cargo refuses `--lib`
    when no selected package has a library)."""
    kinds = {kind for pkg in metadata["packages"] if pkg["name"] in members
             for target in pkg["targets"] for kind in target["kind"]}
    flags = []
    if kinds & {"lib", "rlib", "dylib", "proc-macro"}:
        flags.append("--lib")
    if "bin" in kinds:
        flags.append("--bins")
    if not flags:
        raise UsageError("the product's crates have no library or binary target to check")
    return flags


def resolve_problem(stderr: str) -> str:
    """What keeps a product's declared dependencies from resolving against
    crates.io, from `cargo metadata`'s stderr: the requirements no published
    version satisfies and the crates that are not published at all, else
    cargo's first error line."""
    unmet = sorted(set(re.findall(r"failed to select a version for the requirement `([^`]+)`", stderr)))
    unpublished = sorted(set(re.findall(r"no matching package named `([^`]+)` found", stderr)))
    if unmet or unpublished:
        return "; ".join([*unmet, *(f"{name} is not on crates.io" for name in unpublished)])
    errors = [line.strip().removeprefix("error: ") for line in stderr.splitlines() if line.strip().startswith("error: ")]
    if errors:
        return errors[0]
    lines = [line for line in stderr.strip().splitlines() if line.strip()]
    return lines[-1] if lines else "no output"


def repository_env(root: pathlib.Path) -> str:
    """The repository's Cargo [env] table, as a config file for the scratch
    workspace: the floor builds with the build settings CI tested."""
    config = tomllib.loads((root / ".cargo" / "config.toml").read_text())
    env = config.get("env") or {}
    return "[env]\n" + "".join(f"{json.dumps(key)} = {toml_value(value)}\n" for key, value in env.items())


def floor(root: pathlib.Path, product_name: str, out: pathlib.Path, mode: str, allow_dirty: bool,
          resolve_only: bool = False) -> int:
    product = load_product(root, product_name)
    product_root = (root / product["root"]).resolve()
    members = workspace_members(root)
    product_crates = [pkg for pkg in members if under(pathlib.Path(pkg["manifest_path"]).parent, product_root)]
    if not product_crates:
        raise UsageError(f"no workspace member lies under {product['root']}/")
    in_repo = {pkg["name"]: pkg for pkg in members}
    if out.exists():
        raise UsageError(f"--out {out} exists; the floor check writes a fresh directory")
    out.mkdir(parents=True)
    logs = out / "logs"
    logs.mkdir()
    env = build_env("mirror", out / "target", toolchain(root))
    report: dict = {"product": product_name, "mode": mode, "crates": [pkg["name"] for pkg in product_crates],
                    "stage": None, "problems": []}

    def finish(stage: str, problems: list[str]) -> int:
        report["stage"] = stage
        report["problems"] = problems
        (out / "floor.json").write_text(json.dumps(report, indent=2) + "\n")
        if problems:
            verdict = f"not releasable ({stage})"
        elif stage == "resolve":
            verdict = "resolves against published dependencies (resolution only, nothing built)"
        else:
            verdict = "releasable against published dependencies"
        print(f"{product_name}: {verdict}")
        for problem in problems:
            print(f"  {problem}")
        return 1 if problems else 0

    publish_order = product.get("publish_order")
    ws = out / "ws"
    if isinstance(publish_order, list) and publish_order:
        # Package the publishing crates as crates.io receives them. Packaging
        # resolves their dependencies, so every in-repo crate is patched to
        # its path for that step only.
        unknown = [name for name in publish_order if name not in in_repo]
        if unknown:
            raise UsageError(f"{PRODUCTS}: {product_name} publishes unknown crates {', '.join(unknown)}")
        dirs = {name: pathlib.Path(pkg["manifest_path"]).parent for name, pkg in in_repo.items()}
        patched = [name for name, pkg in in_repo.items() if pkg.get("publish") is None or pkg.get("publish")]
        try:
            unpacked = package_and_unpack(root, publish_order, sorted(set(patched) | set(publish_order)), dirs, out, allow_dirty, env)
        except OversizedPackage as err:
            # A crate file the registry would refuse: nothing to resolve or
            # build, whatever its dependencies.
            return finish("package", err.problems)
        report["package_bytes"] = {
            name: archive.stat().st_size
            for name, archive in packaged_archives(pathlib.Path(env["CARGO_TARGET_DIR"]) / "package", publish_order).items()
        }
        floor_members = {name: path for name, path in unpacked.items()}
    else:
        # An unpublished product: its manifests normalized as cargo package
        # would write them; the product's own crates keep their paths.
        ws.mkdir()
        floor_members = {}
        crate_dirs_by_name = {pkg["name"]: pathlib.Path(pkg["manifest_path"]).parent.resolve() for pkg in product_crates}
        kept = {directory: f"../{name}" for name, directory in crate_dirs_by_name.items()}
        for name, crate_dir in crate_dirs_by_name.items():
            target = ws / name
            shutil.copytree(crate_dir, target, ignore=shutil.ignore_patterns("target", "BUILD.bazel"))
            (target / "Cargo.toml").write_text(toml_document(normalized_manifest(crate_dir, root, kept)))
            floor_members[name] = target
        member_list = "".join(f'    "{path.name}",\n' for path in floor_members.values())
        (ws / "Cargo.toml").write_text(f'[workspace]\nresolver = "2"\nmembers = [\n{member_list}]\n')
        shutil.copyfile(root / "Cargo.lock", ws / "Cargo.lock")
    (ws / ".cargo").mkdir(exist_ok=True)
    (ws / ".cargo" / "config.toml").write_text(repository_env(root))

    # Resolve: every other in-repo crate from crates.io at the declared
    # version, never a path or git source.
    proc = subprocess.run([cargo(), "metadata", "--format-version", "1"], cwd=ws, env=env, capture_output=True, text=True)
    (logs / "resolve.log").write_text(f"$ cargo metadata\n# cwd {ws}\n{proc.stderr}\n# rc {proc.returncode}\n")
    if proc.returncode != 0:
        return finish("resolve", [f"declared dependency versions do not resolve from crates.io: {resolve_problem(proc.stderr)}"])
    resolved = json.loads(proc.stdout)
    problems = []
    for pkg in resolved["packages"]:
        if pkg["name"] in floor_members:
            continue
        if pkg["name"] in in_repo:
            source = pkg.get("source") or "path"
            if not source.startswith("registry+"):
                problems.append(f"{pkg['name']} {pkg['version']} comes from {source}, not crates.io")
    if problems:
        return finish("resolve", problems)
    report["registry_dependencies"] = sorted(
        f"{pkg['name']} {pkg['version']}" for pkg in resolved["packages"] if pkg["name"] in in_repo and pkg["name"] not in floor_members
    )
    # The resolution the build uses (and a later resolution compares with):
    # cargo metadata rewrote the seeded lock of the scratch workspace.
    lock = ws / "Cargo.lock"
    if not lock.is_file():
        return finish("resolve", ["the scratch workspace has no Cargo.lock after resolution"])
    report["lock_sha256"] = hashlib.sha256(lock.read_bytes()).hexdigest()
    if resolve_only:
        return finish("resolve", [])

    command = [cargo(), "check" if mode == "pr" else "build", "--workspace", *surface_flags(resolved, floor_members)]
    rc, output = run(command, ws, env, logs / "build.log")
    if rc != 0:
        errors = [line for line in output.splitlines() if line.startswith("error")][:10]
        return finish("build", [f"`{' '.join(command[1:])}` against the published dependencies fails", *errors])
    return finish("build", [])


# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    census_parser = sub.add_parser("census")
    census_parser.add_argument("--out", required=True, type=pathlib.Path)
    census_parser.add_argument("--crate", action="append", default=[])
    census_parser.add_argument("--stack", choices=("mirror", "default"), default="mirror")
    census_parser.add_argument("--allow-dirty", action="store_true")
    census_parser.add_argument("--shard", default="1/1", help="K/N: test every Nth crate starting at the Kth")
    guard_parser = sub.add_parser("guard")
    guard_parser.add_argument("--crate", action="append", default=[])
    floor_parser = sub.add_parser("floor")
    floor_parser.add_argument("--product", required=True)
    floor_parser.add_argument("--out", required=True, type=pathlib.Path)
    floor_parser.add_argument("--mode", choices=("pr", "release"), default="pr")
    floor_parser.add_argument("--allow-dirty", action="store_true")
    floor_parser.add_argument("--no-build", action="store_true",
                              help="stop after resolution: package, unpack and resolve, build nothing")
    size_parser = sub.add_parser("package-size")
    size_parser.add_argument("--product", required=True)
    size_parser.add_argument("--package-dir", required=True, type=pathlib.Path,
                             help="the directory the crate files were packaged into (<target>/package)")
    args = parser.parse_args(argv)
    try:
        if args.command == "floor":
            return floor(ROOT, args.product, args.out, args.mode, args.allow_dirty, resolve_only=args.no_build)
        if args.command == "package-size":
            return package_size(ROOT, args.product, args.package_dir)
        crates = release_crates(ROOT)
        if args.crate:
            unknown = sorted(set(args.crate) - set(crates))
            if unknown:
                raise UsageError(f"not release crates: {', '.join(unknown)}")
            crates = [name for name in crates if name in set(args.crate)]
        if args.command == "guard":
            return guard(ROOT, crates)
        match = re.fullmatch(r"([1-9][0-9]*)/([1-9][0-9]*)", args.shard)
        if not match or int(match.group(1)) > int(match.group(2)):
            raise UsageError(f"--shard must be K/N with 1 <= K <= N, got {args.shard}")
        k, n = int(match.group(1)), int(match.group(2))
        crates = crates[k - 1 :: n]
        return census(ROOT, crates, args.out, args.stack, args.allow_dirty)
    except OversizedPackage as err:
        print_size_problems(err.problems)
        return 1
    except UsageError as err:
        print(f"published_crates_standalone: {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
