#!/usr/bin/env python3
"""Fail when a non-shipping (test-only) Cargo feature can reach a release binary
that Bazel builds.

The release roots are the targets every `release-build-*` case of
scripts/buildbuddy-bazel-poc builds; they must match the binaries
.github/workflows/release.yml packages from the BuildBuddy outputs.

(a) Every Bazel build of the release roots with -c opt, every
    `release-build-*` case of the launcher, passes
    --define=meerkat_release_build=1, which //tools/bazel:release_build
    matches; no workflow, Make target or script builds with -c opt any other
    way.
(b) Evaluated under that define, no target in the roots' dependency closure
    enables a feature listed in scripts/non-shipping-features.json or
    forwards one (`dep/feature`, `dep?/feature`), and none is a
    *_test_support test variant. A select keyed on //tools/bazel:release_build
    contributes every branch but its default (a more specific key could
    match beside the release one); any other select contributes every branch,
    since a platform or configuration key may match on some release target. A
    value the check cannot read whole fails it: features must be literal
    string lists, their sums and selects of those; dependencies may also add
    crate_universe's all_crate_deps() (external crates); an alias must name a
    literal label.
(c) Evaluated the same way, no crate_universe repository that MODULE.bazel.lock
    renders for an external crate listed under "external_crate_features" in
    scripts/non-shipping-features.json enables one of that crate's listed
    features in its library (the rules_rust patch moves them into a select on
    //tools/bazel:release_build), and every listed crate has such a
    repository.

Usage: check-bazel-release-features.py [repo-root]
Exit 0 = clean, 1 = violations, 2 = the inputs cannot be read.
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

LAUNCHER = "scripts/buildbuddy-bazel-poc"
RELEASE_WORKFLOW = ".github/workflows/release.yml"
FEATURES_FILE = "scripts/non-shipping-features.json"
MODULE_LOCK = "MODULE.bazel.lock"
CRATE_EXTENSION_SUFFIX = "//crate_universe:extensions.bzl%crate"
# A crate_universe spoke repository: <hub>__<crate>-<version> ("+" in a
# version becomes "-").
CRATE_REPO = re.compile(r"^(?P<hub>[A-Za-z0-9_.]+)__(?P<crate>.+)-(?P<version>\d+\.\d+\.\d+[0-9A-Za-z.-]*)$")
SETTING_PACKAGE = "tools/bazel"
SETTING_LABEL = "//tools/bazel:release_build"
# The same setting as a key in an external repository's BUILD file, which must
# name the main repository.
RELEASE_KEYS = {SETTING_LABEL, f"@{SETTING_LABEL}", f"@@{SETTING_LABEL}"}
DEFINE_NAME = "meerkat_release_build"
DEFINE_FLAG = f"--define={DEFINE_NAME}=1"
RUST_RULES = ("rust_library", "rust_binary", "rust_proc_macro", "rust_shared_library", "rust_static_library")
TEST_VARIANT_SUFFIXES = ("_test_support", "_with_runtime_test_support")
# Paths whose text names the opt flags without running a build: this guard and
# its contract test.
SELF_PATHS = {
    "scripts/check-bazel-release-features.py",
    "scripts/test_check_bazel_release_features.py",
}
OPT_FLAG = re.compile(r"(?:--compilation_mode[= ]opt\b|(?<![\w-])-c[= ]opt\b)")
SKIP_DIRS = {".git", "target", "node_modules", "bazel-out"}

RULE = re.compile(r"^([a-z_]+)\(\n(.*?)\n\)$", re.S | re.M)
ATTR = re.compile(r"^ {4}([a-z_]+) = ", re.M)
SELECT = re.compile(r"select\(\{(.*?)\}\)", re.S)
BRANCH_KEY = re.compile(r'"([^"]+)"\s*:')
QUOTED = re.compile(r'"([^"]*)"')
STRING = r'"[^"\\\n]*"'
# A list of string literals and nothing else: an identifier, call or comment
# inside it cannot be evaluated.
LIST = rf"\[\s*(?:{STRING}\s*,\s*)*(?:{STRING}\s*)?\]"
LIST_EXPR = re.compile(rf"^{LIST}(?:\s*\+\s*{LIST})*$")
# crate_universe's macro for a package's external crates, which carry no
# workspace feature and are not walked.
EXTERNAL_DEPS = rf"all_crate_deps\(\s*(?:[a-z_]+\s*=\s*(?:{STRING}|True|False)\s*,\s*)*(?:[a-z_]+\s*=\s*(?:{STRING}|True|False)\s*)?\)"
EXTERNAL_DEPS_CALL = re.compile(EXTERNAL_DEPS)
DEPS_EXPR = re.compile(rf"^(?:{LIST}|{EXTERNAL_DEPS})(?:\s*\+\s*(?:{LIST}|{EXTERNAL_DEPS}))*$")
DEFAULT_CONDITION = "//conditions:default"


class GuardError(Exception):
    pass


def read(root: pathlib.Path, rel: str) -> str:
    try:
        return (root / rel).read_text(encoding="utf-8")
    except OSError as error:
        raise GuardError(f"cannot read {rel}: {error}") from error


def non_shipping_features(root: pathlib.Path) -> set[str]:
    try:
        features = json.loads(read(root, FEATURES_FILE))["features"]
    except (KeyError, TypeError, ValueError) as error:
        raise GuardError(f"{FEATURES_FILE} has no feature list: {error}") from error
    if not features or not all(isinstance(feature, str) and feature for feature in features):
        raise GuardError(f"{FEATURES_FILE} must list at least one feature name")
    return set(features)


def is_non_shipping(feature: str, features: set[str]) -> bool:
    return feature.rpartition("/")[2] in features


def select_branches(select_body: str, where: str) -> list[tuple[str, str]]:
    """Every (key, value) of one select(), each key and value consumed whole.

    A key must be a string literal and a value a literal string list or a sum
    of them; anything else (a variable, a call, a nested select) cannot be
    evaluated and fails. The text before the first literal key must be empty:
    a leading key that is not a literal would otherwise go unread.
    """
    marks = list(BRANCH_KEY.finditer(select_body))
    if not marks:
        raise GuardError(f"{where}: cannot evaluate a select() without branches")
    leading = select_body[:marks[0].start()].strip()
    if leading:
        raise GuardError(f"{where}: cannot evaluate select() key {leading[:60]!r}")
    branches = []
    for index, mark in enumerate(marks):
        end = marks[index + 1].start() if index + 1 < len(marks) else len(select_body)
        value = select_body[mark.end():end].strip()
        if value.endswith(","):
            value = value[:-1].rstrip()
        if not LIST_EXPR.match(value):
            raise GuardError(f"{where}: cannot evaluate select() branch {mark.group(1)!r}: {value[:60]!r}")
        branches.append((mark.group(1), value))
    keys = [key for key, _ in branches]
    if len(set(keys)) != len(keys):
        raise GuardError(f"{where}: cannot evaluate a select() that repeats a branch key")
    return branches


def release_alternatives(select_body: str, where: str) -> str:
    """Every branch a release build can take from one select(), as one list.

    Under the release define, a select keyed on //tools/bazel:release_build
    takes that branch or a more specific key that also matches, never its
    default; a select without that key takes whichever platform or
    configuration key matches on the target being built, or its default. The
    check keeps every alternative it cannot rule out.
    """
    branches = select_branches(select_body, where)
    if any(key in RELEASE_KEYS for key, _ in branches):
        branches = [(key, value) for key, value in branches if key != DEFAULT_CONDITION]
    items = [item for _, value in branches for item in QUOTED.findall(value)]
    return "[" + ", ".join(f'"{item}"' for item in items) + "]"


def evaluate(expr: str, where: str) -> str:
    """Replace every select() in an attribute value with its release alternatives."""
    if "select(" in expr and expr.count("select(") != len(SELECT.findall(expr)):
        raise GuardError(f"{where}: cannot evaluate a nested or malformed select()")
    return SELECT.sub(lambda match: release_alternatives(match.group(1), where), expr)


def attributes(body: str) -> dict[str, str]:
    marks = list(ATTR.finditer(body))
    values = {}
    for index, mark in enumerate(marks):
        end = marks[index + 1].start() if index + 1 < len(marks) else len(body)
        values[mark.group(1)] = body[mark.end():end].rstrip().rstrip(",")
    return values


def string_list(expr: str, where: str) -> list[str]:
    """A literal string list, or a sum of literal string lists."""
    expr = expr.strip()
    if not LIST_EXPR.match(expr):
        raise GuardError(f"{where}: cannot evaluate {expr[:60]!r} as a string list")
    return QUOTED.findall(expr)


def dependency_labels(expr: str, package: str, where: str) -> list[str]:
    """The workspace labels of a deps value read whole: literal string lists
    summed with all_crate_deps() calls."""
    expr = expr.strip()
    if not DEPS_EXPR.match(expr):
        raise GuardError(f"{where}: cannot evaluate {expr[:60]!r} as dependency lists")
    return labels_in(EXTERNAL_DEPS_CALL.sub("", expr), package)


def labels_in(expr: str, package: str) -> list[str]:
    labels = []
    for value in QUOTED.findall(expr):
        if value.startswith(":"):
            labels.append(f"//{package}{value}")
        elif value.startswith("//"):
            labels.append(value if ":" in value else f"{value}:{value.rsplit('/', 1)[-1]}")
    return labels


def build_files(root: pathlib.Path):
    for path in sorted(root.rglob("BUILD.bazel")):
        parts = path.relative_to(root).parts
        if any(part in SKIP_DIRS or part.startswith("bazel-") for part in parts[:-1]):
            continue
        yield path


def load_rules(root: pathlib.Path) -> dict[str, tuple[str, str, dict[str, str]]]:
    rules = {}
    for path in build_files(root):
        package = path.parent.relative_to(root).as_posix()
        package = "" if package == "." else package
        for match in RULE.finditer(path.read_text(encoding="utf-8")):
            kind, body = match.group(1), match.group(2)
            attrs = attributes(body)
            name = QUOTED.findall(attrs.get("name", ""))
            if not name:
                continue
            rules[f"//{package}:{name[0]}"] = (kind, package, attrs)
    return rules


def check_setting(rules) -> list[str]:
    """The selects' release branch applies exactly when the launcher's define is set."""
    entry = rules.get(SETTING_LABEL)
    if entry is None or entry[0] != "config_setting":
        return [f"{SETTING_LABEL} is not a config_setting in {SETTING_PACKAGE}/BUILD.bazel"]
    attrs = entry[2]
    raw = attrs.get("define_values", "")
    pairs = dict(re.findall(r'"([^"]+)":\s*"([^"]*)"', raw))
    extra = sorted(set(attrs) - {"name", "define_values", "visibility"})
    if pairs != {DEFINE_NAME: "1"} or extra:
        return [
            f"{SETTING_LABEL} must match exactly {DEFINE_FLAG} and nothing else; "
            f"found define_values {raw.strip()!r}, other conditions {extra}"
        ]
    return []


def launcher_cases(text: str) -> dict[str, str]:
    cases = {}
    for match in re.finditer(r"^  ([A-Za-z0-9_-]+)\)\n(.*?)^    ;;$", text, re.S | re.M):
        cases[match.group(1)] = match.group(2)
    return cases


def check_launcher(root: pathlib.Path) -> tuple[list[str], set[str], set[str]]:
    errors = []
    cases = launcher_cases(read(root, LAUNCHER))
    release_cases = {name for name in cases if name.startswith("release-build-")}
    if not release_cases:
        errors.append(f"{LAUNCHER} defines no release-build-* case")
    roots: set[str] | None = None
    for name, body in sorted(cases.items()):
        builds_opt = bool(OPT_FLAG.search(body))
        if name in release_cases and not builds_opt:
            errors.append(f"{LAUNCHER} {name} does not build with --compilation_mode=opt")
        if (builds_opt or name in release_cases) and DEFINE_FLAG not in body:
            errors.append(f"{LAUNCHER} {name} builds release binaries without {DEFINE_FLAG}")
        if name in release_cases:
            targets = re.search(r'default_target="([^"]*)"', body)
            labels = set(targets.group(1).split()) if targets else set()
            if not labels:
                errors.append(f"{LAUNCHER} {name} names no default_target")
            elif roots is None:
                roots = labels
            elif labels != roots:
                errors.append(f"{LAUNCHER} {name} builds {sorted(labels)}, other release cases {sorted(roots)}")
    return errors, roots or set(), release_cases


def check_workflow(root: pathlib.Path, roots: set[str], release_cases: set[str]) -> list[str]:
    errors = []
    text = read(root, RELEASE_WORKFLOW)
    for command in sorted(set(re.findall(r"bb_command:\s*([A-Za-z0-9_-]+)", text))):
        if command not in release_cases:
            errors.append(f"{RELEASE_WORKFLOW} builds release binaries with {command}, not a checked release-build-* case")
    packaged = set()
    for block in re.findall(r"declare -A BIN_PATHS=\((.*?)\)", text, re.S):
        for _, path in re.findall(r'\[([^\]]+)\]="([^"]+)"', block):
            package, _, name = path.rpartition("/")
            packaged.add(f"//{package}:{name}")
    if not packaged:
        errors.append(f"{RELEASE_WORKFLOW} packages no BuildBuddy BIN_PATHS")
    elif packaged != roots:
        errors.append(
            f"{RELEASE_WORKFLOW} packages {sorted(packaged)} but {LAUNCHER} release-build cases build {sorted(roots)}"
        )
    return errors


def check_other_opt_builds(root: pathlib.Path) -> list[str]:
    errors = []
    candidates = [root / "Makefile", root / ".bazelrc"]
    for directory in ("scripts", ".github", "tools"):
        base = root / directory
        if base.is_dir():
            candidates.extend(path for path in sorted(base.rglob("*")) if path.is_file())
    for path in candidates:
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel == LAUNCHER or rel in SELF_PATHS or any(part in SKIP_DIRS for part in path.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in enumerate(text.splitlines(), 1):
            if OPT_FLAG.search(line) and DEFINE_FLAG not in line:
                errors.append(f"{rel}:{number} builds with -c opt outside the {LAUNCHER} release-build cases: {line.strip()}")
    return errors


def check_closure(rules, roots: set[str], features: set[str]) -> tuple[list[str], int, int]:
    errors = []
    seen: set[str] = set()
    selects = 0
    stack = sorted(roots)
    while stack:
        label = stack.pop()
        if label in seen:
            continue
        seen.add(label)
        entry = rules.get(label)
        if entry is None:
            errors.append(f"{label} is in the release closure but no BUILD.bazel defines it")
            continue
        kind, package, attrs = entry
        if kind == "alias":
            actual = attrs.get("actual", "").strip()
            if not re.fullmatch(STRING, actual):
                raise GuardError(f"{label}: cannot evaluate alias actual {actual[:60]!r}")
            stack.extend(labels_in(actual, package))
            continue
        if kind not in RUST_RULES:
            errors.append(f"{label} is a {kind}, not a Rust library or binary, in the release closure")
            continue
        if label.endswith(TEST_VARIANT_SUFFIXES):
            errors.append(f"{label} is a test-support variant in the release closure")
        expr = attrs.get("crate_features", "[]")
        selects += expr.count("select(")
        for feature in string_list(evaluate(expr, label), label):
            if is_non_shipping(feature, features):
                errors.append(f"{label} enables non-shipping feature {feature!r} in the release configuration")
        for attr in ("deps", "proc_macro_deps"):
            stack.extend(dependency_labels(evaluate(attrs.get(attr, "[]"), label), package, f"{label} {attr}"))
    return errors, len(seen), selects


def external_crate_features(root: pathlib.Path) -> dict[str, set[str]]:
    """The external crates' test-only features: {crate: {feature}}."""
    try:
        external = json.loads(read(root, FEATURES_FILE)).get("external_crate_features", {})
    except (AttributeError, ValueError) as error:
        raise GuardError(f"{FEATURES_FILE} cannot be read: {error}") from error
    if not isinstance(external, dict) or not all(
        isinstance(crate, str)
        and crate
        and isinstance(features, list)
        and features
        and all(isinstance(feature, str) and feature for feature in features)
        for crate, features in external.items()
    ):
        raise GuardError(
            f"{FEATURES_FILE} external_crate_features must map crate names to non-empty feature lists"
        )
    return {crate: set(features) for crate, features in external.items()}


def strip_comments(text: str) -> str:
    """Drop `#` comments outside string literals (rendered crate BUILD files
    annotate select branch items with their platform)."""
    lines = []
    for line in text.split("\n"):
        quoted = False
        escaped = False
        for index, char in enumerate(line):
            if escaped:
                escaped = False
            elif char == "\\" and quoted:
                escaped = True
            elif char == '"':
                quoted = not quoted
            elif char == "#" and not quoted:
                line = line[:index].rstrip()
                break
        lines.append(line)
    return "\n".join(lines)


def crate_universe_build_files(root: pathlib.Path):
    """(repository, BUILD file content) of every crate_universe repository
    MODULE.bazel.lock records."""
    try:
        lock = json.loads(read(root, MODULE_LOCK))
    except ValueError as error:
        raise GuardError(f"{MODULE_LOCK} is not JSON: {error}") from error
    extensions = lock.get("moduleExtensions") if isinstance(lock, dict) else None
    if not isinstance(extensions, dict):
        raise GuardError(f"{MODULE_LOCK} records no module extensions")
    for key, extension in sorted(extensions.items()):
        if not key.endswith(CRATE_EXTENSION_SUFFIX) or not isinstance(extension, dict):
            continue
        for scope in extension.values():
            specs = scope.get("generatedRepoSpecs", {}) if isinstance(scope, dict) else {}
            for repository, spec in sorted(specs.items()):
                attributes = spec.get("attributes", {}) if isinstance(spec, dict) else {}
                content = attributes.get("build_file_content")
                if isinstance(content, str):
                    yield repository, content


def crate_universe_repositories(root: pathlib.Path) -> set[str]:
    """The name of every repository the crate extension generates in
    MODULE.bazel.lock: the hubs and their crates' repositories."""
    try:
        lock = json.loads(read(root, MODULE_LOCK))
    except ValueError as error:
        raise GuardError(f"{MODULE_LOCK} is not JSON: {error}") from error
    names: set[str] = set()
    extensions = lock.get("moduleExtensions") if isinstance(lock, dict) else None
    for key, extension in sorted((extensions or {}).items()) if isinstance(extensions, dict) else []:
        if not key.endswith(CRATE_EXTENSION_SUFFIX) or not isinstance(extension, dict):
            continue
        for scope in extension.values():
            specs = scope.get("generatedRepoSpecs", {}) if isinstance(scope, dict) else {}
            names.update(specs)
    return names


def check_external_crates(root: pathlib.Path, external: dict[str, set[str]]) -> tuple[list[str], int]:
    """No rendered repository of a listed external crate enables a listed
    feature in a release build of its library."""
    if not external:
        return [], 0
    errors = []
    checked = 0
    rendered = {crate: 0 for crate in external}
    repositories = crate_universe_repositories(root)
    for repository, content in crate_universe_build_files(root):
        match = CRATE_REPO.match(repository)
        if match is None or match.group("crate") not in external:
            continue
        crate = match.group("crate")
        library = crate.replace("-", "_")
        libraries = []
        redirected = False
        for rule in RULE.finditer(strip_comments(content)):
            kind, attrs = rule.group(1), attributes(rule.group(2))
            if kind == "rust_library" and QUOTED.findall(attrs.get("name", ""))[:1] == [library]:
                libraries.append(attrs)
            # crate_universe's `override_target_lib`: the repository builds no
            # library of its own, its name is an alias of another
            # repository's library (MODULE.bazel points the live stack's
            # tokio at the main one). What links it links that library,
            # which is checked where it is rendered. Only a plain label
            # counts (one string literal, no select), and only one of a
            # repository this lock's crate extension generates: its last
            # `+` part, the name the lock records, for a canonical label.
            if kind == "alias" and QUOTED.findall(attrs.get("name", ""))[:1] == [library]:
                actual = attrs.get("actual", "").strip()
                target = re.fullmatch(rf'"@@?([A-Za-z0-9_.+~-]+)//:{re.escape(library)}"', actual)
                if target and target.group(1).rsplit("+", 1)[-1] in repositories - {repository}:
                    redirected = True
        if not libraries and redirected:
            continue
        if not libraries:
            raise GuardError(f"@{repository}: cannot find the rendered rust_library {library!r}")
        rendered[crate] += 1
        for attrs in libraries:
            checked += 1
            label = f"@{repository}//:{library}"
            for feature in string_list(evaluate(attrs.get("crate_features", "[]"), label), label):
                if feature in external[crate]:
                    errors.append(f"{label} enables non-shipping feature {feature!r} in the release configuration")
    for crate, count in sorted(rendered.items()):
        if count == 0:
            raise GuardError(
                f"{MODULE_LOCK} renders no crate_universe repository of {crate}, which "
                f"{FEATURES_FILE} lists under external_crate_features"
            )
    return errors, checked


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    try:
        features = non_shipping_features(root)
        rules = load_rules(root)
        errors = check_setting(rules)
        launcher_errors, roots, release_cases = check_launcher(root)
        errors += launcher_errors
        errors += check_workflow(root, roots, release_cases)
        errors += check_other_opt_builds(root)
        closure_errors, walked, selects = check_closure(rules, roots, features)
        errors += closure_errors
        external_errors, external_checked = check_external_crates(root, external_crate_features(root))
        errors += external_errors
    except GuardError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if errors:
        print("Release binaries can compile test-only code:", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1
    print(
        f"Release closure of {', '.join(sorted(roots))}: {walked} targets, "
        f"{selects} release selects evaluated, {external_checked} external crate "
        f"libraries checked, no non-shipping feature"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
