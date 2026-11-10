#!/usr/bin/env python3
"""Refuse an in-repo package pinned below its version in this tree.

Every product of this repository (Core, Meta, Connect, Forge and their SDKs)
consumes the others at the version this tree builds: in development and CI
the build of the same commit, in a release the same release's artifacts. A
pin of an in-repo package is therefore either a same-commit reference (a
path, `file:`, `link:` or workspace dependency) or names exactly the
package's version in this tree; a range may admit later versions but no
earlier one, and it must admit the version here: no ceiling at or below it,
no strict floor at it, no floor above it. A requirement that names one
version above the version here is refused the same way.

The in-repo packages and their versions come from the tree itself: the Cargo
workspace members (and the root table's renamed entries), every package.json
and pyproject.toml outside vendored, fixture and test trees, and the
products' release tags (ci/products.json). ci/inrepo-version-pins.json adds
what the tree cannot say: former package names, release asset prefixes,
further release tags, and the allowlist.

Pins it finds in every tracked text file:
  - dependencies in package.json and package-lock.json (structured);
  - requirement specifiers (`meerkat-meta==0.9.11`), npm specifiers
    (`@rkat/sdk@0.9.11`), Cargo dependency lines (`meerkat-meta = "0.9.11"`)
    and npm dependency entries outside package.json;
  - release URLs and `{"repository", "tag"}` release selections, of the
    monorepo by their tag, and of the retired standalone repositories always
    (their releases predate the monorepo);
  - versioned release asset names (`meerkat_meta-0.9.3-py3-none-any.whl`,
    `meta-rpc-gateway-0.9.10-...`), and tracked files named like them;
  - version constants whose name names a product (`CORE_REQUIREMENT =
    "=0.9.3"`, `MOBKIT_VERSION = "0.8.47"`).

The allowlist covers upgrade-test fixtures and recorded history, each entry
with its reason. An entry without `match` or `key` covers whole files, which
are not read; it is stale when it matches no tracked file. An entry with
`match` (a regular expression searched in the pin) covers those pins only. In
a JSON file an entry may also carry `key`, a regular expression searched in
the key path of the value that starts on the pin's line (object keys joined
by `/`, array items by their index): it then covers only the pins of the
values under those keys, so two rows of one file that pin the same release
keep apart. An entry with `match` or `key` is stale when it covers no pin. A
temporary entry names, in `removed_by`, the change that removes its pins;
when that change lands the entry is stale and goes.

Published release inputs (`published_release_inputs` in the configuration,
with its reason) are files that pin published assets of an in-repo release
by digest. Those cannot name a version before its release is published, so a
pin there may name the version in this tree or the release before it (the
newest release heading below that version in the product's CHANGELOG.md),
nothing older and nothing newer. A listed path may come with a `match`: the
rule then applies to the pins it matches only, the file's other pins follow
the ordinary rule, and the path is stale when it applies to no pin. Such a
file is read even where a whole-file allow entry covers its directory. A
listed path that matches no tracked file is stale. `version_constants` names
the package of a version constant whose name names no product
(`VERSION = "0.9.11"` in a given file); a configured constant the file no
longer holds is stale.

`--write` first moves every plain-text pin of a dependency manifest
(package.json and a lock's root package, pyproject.toml, a requirements file,
Cargo.toml) that no allow entry covers to the version of the package it pins,
and a range's ceiling that version reaches to the next release that may break
it (the next minor on the 0.x line, else the next major: `>=0.9.11 <0.10.0`
becomes `>=0.10.0 <0.11.0`); a floor or an exact requirement above that
version moves down to it, and a strict floor at it becomes inclusive (`>0.9.11`
becomes `>=0.9.11`). The version bumps run it so that a bump moves
those pins in the same commit. A
registry lock entry and a requirement with a hash cannot move this way and stay
findings, as do pins in prose and code, which the docs stamper or a person
moves.

usage: check_inrepo_version_pins.py [--root DIR] [--config FILE] [--write]
Exit 0 when clean, 1 on a finding or a stale allowlist entry, 2 on an error.
"""

from __future__ import annotations

import argparse
import bisect
import dataclasses
import json
import re
import subprocess
import sys
from json.decoder import scanstring
from pathlib import Path, PurePosixPath

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised only on Python < 3.11
    import tomli as tomllib

CONFIG = "ci/inrepo-version-pins.json"
PRODUCTS = "ci/products.json"
VERSION = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
RELEASE_VERSION = r"\d+\.\d+\.\d+"
VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")
NPM_DEPENDENCY_MAPS = ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies")
SAME_COMMIT_SPEC = re.compile(r"^(?:file|link|workspace|portal):")
DIGEST_RE = re.compile(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])")
NPM_MANIFESTS = ("package.json", "package-lock.json", "npm-shrinkwrap.json")
CODE_SUFFIXES = (".py", ".ts", ".tsx", ".mts", ".cts", ".mjs", ".cjs", ".js", ".jsx", ".sh", ".bash", ".rs")
# Words in a constant's name that say which product's version it holds.
PRODUCT_WORDS = {
    "CORE": "core", "RKAT": "core", "MEERKAT": "core",
    "META": "meta", "MOBKIT": "meta", "GUIDE": "meta", "RECEIVER": "meta",
    "CONNECT": "connect", "TOOLKIT": "connect",
    "FORGE": "forge",
}
# Constants of something other than a product release.
NON_PRODUCT_WORDS = {"CONTRACT", "SCHEMA", "PROTOCOL", "FORMAT", "NODE", "PYTHON", "NPM", "UV", "MCP", "API"}
OWNER = "lukacf"
MONOREPO = "meerkat"
# The standalone repositories the products came from, and the product each held.
RETIRED_REPOSITORIES = {"meerkat-meta": "meta", "meerkat-mobkit": "meta", "meerkat-connect": "connect"}


class PinError(Exception):
    """The inventory or the configuration cannot be read."""


def version_key(value):
    match = VERSION_RE.match(value) if isinstance(value, str) else None
    if not match:
        return None
    major, minor, patch, pre = match.groups()
    # A prerelease sorts before its release.
    return (int(major), int(minor), int(patch), 0 if pre else 1, pre or "")


def below(pinned: str, expected: str) -> bool:
    pinned_key, expected_key = version_key(pinned), version_key(expected)
    return pinned_key is not None and expected_key is not None and pinned_key < expected_key


def full_version(value: str) -> str:
    """A one- or two-part version with its missing parts as zero: 0.10 is 0.10.0."""
    return ".".join((value.split(".") + ["0", "0"])[:3])


def next_breaking(version: str) -> str:
    """The first release after `version` that may break it: the next minor on the 0.x line, else the next major."""
    major, minor = version_key(version)[:2]
    return f"0.{minor + 1}.0" if major == 0 else f"{major + 1}.0.0"


CEILING_RE = re.compile(r"(<=|<)[ \t]*v?(\d+(?:\.\d+){0,2})(?![0-9.])")
# One comparator of an npm range, a PEP 440 specifier or a Cargo requirement: an
# operator and a version, or a bare version that starts a word.
COMPARATOR_RE = re.compile(r"(?:(===|==|~=|!=|>=|<=|>|<|=|\^|~)[ \t]*|(?<![0-9A-Za-z.+_-]))v?(\d+(?:\.\d+){0,2})"
                           r"(-[0-9A-Za-z.-]+)?(\.\*)?")


def comparator_admits(match: re.Match, target: tuple) -> bool:
    """Whether one comparator of a range lets `target` through."""
    operator, base, pre, wildcard = match.groups()
    bound = version_key(full_version(base) + (pre or ""))
    if bound is None:
        return True
    if operator == "<":
        return bound > target
    if operator == "<=":
        return bound >= target
    if operator == ">":
        return bound < target
    if operator == "!=":
        parts = len(base.split(".")) if wildcard else len(bound)
        return bound[:parts] != target[:parts]
    # `>=`, `^`, `~`, `~=`, an exact pin and a bare version are floors here: a floor
    # below the version is a pin below it, which the caller reports from the floor.
    return bound <= target


def range_admits(spec: str, version: str) -> bool:
    """Whether some alternative of an npm range, a PEP 440 specifier or a Cargo requirement admits `version`.

    An alternative admits it when no bound shuts it out: no ceiling at or below it, no
    strict floor at it, and no floor or exact pin above it.
    """
    target = version_key(version)
    if target is None:
        return True
    if spec.startswith("npm:"):
        spec = spec.rpartition("@")[2]
    for alternative in spec.split("||"):
        # An npm hyphen range `A - B` is `>=A <=B`.
        alternative = re.sub(r"(?<=[0-9A-Za-z])\s+-\s+(?=v?\d)", " <=", alternative)
        if all(comparator_admits(match, target) for match in COMPARATOR_RE.finditer(alternative)):
            return True
    return False


def move_range(spec: str, pinned: str, expected: str) -> str:
    """Move a range's floor to `expected`, and a ceiling it reaches to the next release that may break.

    A floor above `expected` moves down to it as one below it moves up, and a strict
    floor at it becomes inclusive.
    """
    moved = move_version(spec, pinned, expected) if pinned and pinned != expected else spec
    moved = re.sub(rf"(?<![<>=!~^])>(?!=)([ \t]*v?{re.escape(expected)}(?![0-9]|\.[0-9]|-))", r">=\1", moved)
    target = version_key(expected)

    def lift(match):
        bound = version_key(full_version(match.group(2)))
        reached = bound is not None and target is not None and (bound <= target if match.group(1) == "<"
                                                                 else bound < target)
        return f"<{next_breaking(expected)}" if reached else match.group(0)

    return CEILING_RE.sub(lift, moved)


def normalize_pypi(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def key_of(ecosystem: str, name: str):
    return ecosystem, normalize_pypi(name) if ecosystem == "pypi" else name


@dataclasses.dataclass(frozen=True)
class Package:
    ecosystem: str  # cargo, npm, pypi or product
    name: str
    version: str
    source: str


@dataclasses.dataclass
class Finding:
    path: str
    line: int
    shape: str
    pin: str
    package: str
    pinned: str
    expected: str
    movable: bool = False
    # A pin of a published release input, and the release before `expected` it may also name.
    published: bool = False
    previous: str = ""
    # A range or an exact pin that shuts out `expected`: a ceiling at or below it, a strict
    # floor at it, or a floor above it.
    excludes: bool = False
    # In a JSON file a `key` entry scopes: the key path of the value that starts on the line.
    key: str | None = None

    def describe(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        if self.shape == "retired-repository":
            return (f"{where}: {self.pin}: a release of the retired standalone repository; "
                    f"use {self.package} {self.expected} of this repository")
        if self.excludes and not below(self.pinned, self.expected):
            if below(self.expected, self.pinned):
                return (f"{where}: {self.shape} {self.pin}: pins {self.package} {self.pinned}, "
                        f"above its version {self.expected} in this tree")
            return (f"{where}: {self.shape} {self.pin}: admits no {self.package} {self.expected}, its version in this "
                    f"tree (a bound shuts it out)")
        if self.published:
            allowed = (f"a published release input names {self.expected} or the release before it, {self.previous}"
                       if self.previous else
                       f"a published release input names {self.expected} or the release before it, and the "
                       f"product's CHANGELOG.md has none")
            if self.pinned and below(self.expected, self.pinned):
                relation = f"above its version {self.expected}"
            elif self.previous:
                relation = f"older than the release before its version {self.expected}"
            else:
                relation = f"below its version {self.expected}"
            return (f"{where}: {self.shape} {self.pin}: pins {self.package} {self.pinned or 'an open range'}, "
                    f"{relation} in this tree ({allowed})")
        if not self.pinned:
            return (f"{where}: {self.shape} {self.pin}: admits {self.package} below {self.expected}; "
                    f"pin {self.expected} or use a same-commit reference")
        return (f"{where}: {self.shape} {self.pin}: pins {self.package} {self.pinned}, "
                f"below its version {self.expected} in this tree")


class Inventory:
    """The in-repo packages, by ecosystem and name, at their versions in this tree."""

    def __init__(self, root: Path, config: dict, tracked: list[str]):
        self.root = root
        self.packages: dict[tuple[str, str], Package] = {}
        self.tag_prefixes: list[tuple[str, tuple[str, str]]] = []
        self.assets: dict[str, tuple[str, str]] = {}
        # Each product's root and CHANGELOG.md, and the packages configured as products of their own.
        self.product_roots: dict[str, str] = {}
        self.changelogs: dict[str, str] = {}
        self.product_packages: dict[tuple[str, str], str] = {}
        self._headings: dict[str, list[str]] = {}
        self._load_cargo()
        self._load_manifests(tracked)
        self._load_products()
        self._load_config(config)

    def add(self, ecosystem: str, name: str, version: str, source: str) -> None:
        if version_key(version) is None:
            return
        key = key_of(ecosystem, name)
        if key in self.packages and self.packages[key].version != version:
            raise PinError(f"{ecosystem} package {name} has two versions in this tree: "
                           f"{self.packages[key].version} ({self.packages[key].source}) and {version} ({source})")
        self.packages.setdefault(key, Package(ecosystem, name, version, source))

    def get(self, ecosystem: str, name: str):
        return self.packages.get(key_of(ecosystem, name))

    def _root_manifest(self) -> dict:
        path = self.root / "Cargo.toml"
        return tomllib.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}

    def _load_cargo(self) -> None:
        workspace = self._root_manifest().get("workspace") or {}
        workspace_version = (workspace.get("package") or {}).get("version")
        for member in workspace.get("members") or []:
            for directory in sorted(self.root.glob(member)):
                manifest = directory / "Cargo.toml"
                if not manifest.is_file():
                    continue
                package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package") or {}
                version = package.get("version")
                if isinstance(version, dict) and version.get("workspace"):
                    version = workspace_version
                if isinstance(package.get("name"), str) and isinstance(version, str):
                    self.add("cargo", package["name"], version, manifest.relative_to(self.root).as_posix())
        # Renamed entries of the root table: `alias = { package = "real", ... }`.
        for alias, spec in (workspace.get("dependencies") or {}).items():
            if isinstance(spec, dict) and isinstance(spec.get("package"), str):
                real = self.get("cargo", spec["package"])
                if real is not None and self.get("cargo", alias) is None:
                    self.packages[key_of("cargo", alias)] = dataclasses.replace(real, name=alias)

    def _load_manifests(self, tracked: list[str]) -> None:
        for path in tracked:
            name = PurePosixPath(path).name
            if name not in ("package.json", "pyproject.toml"):
                continue
            if re.search(r"(^|/)(node_modules|vendor|licenses|fixtures|tests?|__tests__)/", path):
                continue
            try:
                text = (self.root / path).read_text(encoding="utf-8")
                if name == "package.json":
                    data = json.loads(text)
                    if isinstance(data, dict) and isinstance(data.get("name"), str):
                        self.add("npm", data["name"], data.get("version"), path)
                else:
                    project = tomllib.loads(text).get("project") or {}
                    if isinstance(project.get("name"), str):
                        self.add("pypi", project["name"], project.get("version"), path)
            except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
                raise PinError(f"{path} cannot be read: {error}") from error

    def _load_products(self) -> None:
        path = self.root / PRODUCTS
        if not path.is_file():
            return
        products = json.loads(path.read_text(encoding="utf-8")).get("products") or []
        roots = {product.get("name"): product.get("root") for product in products}
        for product in products:
            name, root, prefix = product.get("name"), product.get("root"), product.get("tag_prefix")
            if not isinstance(name, str) or not isinstance(root, str) or not isinstance(prefix, str):
                raise PinError(f"{PRODUCTS} lists a product without a name, root and tag_prefix")
            self.product_roots[name] = root
            self.changelogs[name] = f"{root}/CHANGELOG.md" if root else "CHANGELOG.md"
            if root == "":
                version = ((self._root_manifest().get("workspace") or {}).get("package") or {}).get("version")
                versions = {version} if version else set()
            else:
                versions = {package.version for package in self.packages.values() if package.ecosystem == "cargo"
                            and self._owner(package.source, roots) == name}
            if len(versions) != 1:
                raise PinError(f"product {name} has no single crate version in this tree: {sorted(versions)}")
            self.packages[("product", name)] = Package("product", name, versions.pop(), PRODUCTS)
            self.tag_prefixes.append((prefix, ("product", name)))

    @staticmethod
    def _owner(source: str, roots: dict) -> str:
        for name, root in roots.items():
            if root and (source == root or source.startswith(root + "/")):
                return name
        return "core"

    def _load_config(self, config: dict) -> None:
        for former, current in (config.get("former_names") or {}).items():
            ecosystem, _, name = former.partition(":")
            package = self.target(current, f"former name {former}")
            if self.get(ecosystem, name) is None:
                self.packages[key_of(ecosystem, name)] = dataclasses.replace(
                    self.packages[package], ecosystem=ecosystem, name=name)
        for name, target in (config.get("products") or {}).items():
            key = self.target(target, f"product {name}")
            self.packages[("product", name)] = dataclasses.replace(self.packages[key], ecosystem="product")
            # A product of its own inside another product's root (Forge in meta/) keeps
            # its CHANGELOG.md beside its manifest.
            self.product_packages[key] = name
            self.changelogs[name] = PurePosixPath(self.packages[key].source).parent.joinpath("CHANGELOG.md").as_posix()
        for prefix, target in (config.get("release_tags") or {}).items():
            self.tag_prefixes.append((prefix, self.target(target, f"release tag {prefix}")))
        # Longest prefix first: meta-installer-v before meta-v before v.
        self.tag_prefixes.sort(key=lambda item: -len(item[0]))
        for prefix, target in (config.get("release_assets") or {}).items():
            self.assets[prefix] = self.target(target, f"release asset {prefix}")
        # Every wheel and npm tarball name of an in-repo package.
        for key, package in list(self.packages.items()):
            if package.ecosystem == "pypi":
                self.assets.setdefault(re.sub(r"[-.]+", "_", package.name), key)
            elif package.ecosystem == "npm" and package.name.startswith("@"):
                self.assets.setdefault(package.name[1:].replace("/", "-"), key)

    def target(self, target: str, label: str) -> tuple[str, str]:
        ecosystem, _, name = target.partition(":")
        key = key_of(ecosystem, name)
        if key not in self.packages:
            raise PinError(f"{label} names {target}, which is not an in-repo package of this tree")
        return key

    def expected(self, key: tuple[str, str]) -> tuple[str, str]:
        package = self.packages[key]
        return package.name, package.version

    def product_of(self, key: tuple[str, str]):
        """The product that releases a package: its own when configured, else the owner of its manifest's root."""
        if key[0] == "product":
            return key[1]
        if key in self.product_packages:
            return self.product_packages[key]
        package = self.packages.get(key)
        return self._owner(package.source, self.product_roots) if package is not None else None

    def previous_release(self, key: tuple[str, str]):
        """The newest release heading of the package's product CHANGELOG.md below its version here."""
        path = self.changelogs.get(self.product_of(key))
        if path is None:
            return None
        if path not in self._headings:
            full = self.root / path
            text = full.read_text(encoding="utf-8") if full.is_file() else ""
            self._headings[path] = re.findall(rf"^## \[({VERSION})\]", text, re.M)
        expected = self.packages[key].version
        earlier = [version for version in self._headings[path] if below(version, expected)]
        return max(earlier, key=version_key) if earlier else None

    def tag(self, tag: str):
        for prefix, key in self.tag_prefixes:
            version = tag[len(prefix):] if tag.startswith(prefix) else None
            if version is not None and re.fullmatch(RELEASE_VERSION, version):
                return key, version
        return None


def npm_floor(spec: str):
    """The lowest version an npm range admits; None when it has no lower bound."""
    spec = spec.strip()
    if spec.startswith("npm:"):
        spec = spec.rpartition("@")[2]
    floors = []
    for alternative in spec.split("||"):
        alternative = alternative.strip()
        hyphen = re.match(rf"^v?({VERSION})\s+-\s+", alternative)
        if hyphen:
            floors.append(hyphen.group(1))
            continue
        lower = None
        for comparator in alternative.split():
            match = re.match(rf"^(>=|>|=|\^|~)?v?({VERSION})$", comparator)
            if match and (lower is None or below(lower, match.group(2))):
                lower = match.group(2)
        floors.append(lower)
    if not floors or any(floor is None for floor in floors):
        return None
    return min(floors, key=version_key)


def pep440_floor(specifier: str):
    lower = None
    for clause in specifier.split(","):
        match = re.match(r"^\s*(===|==|~=|>=|>)\s*v?(\d+(?:\.\d+)*)(\.\*)?\s*$", clause)
        if not match:
            continue
        version = ".".join((match.group(2).split(".") + ["0", "0"])[:3])
        if lower is None or below(lower, version):
            lower = version
    return lower


def cargo_floor(spec: str):
    lower = None
    for clause in spec.split(","):
        match = re.match(r"^\s*(?:=|\^|~|>=|>)?\s*v?(\d+(?:\.\d+){0,2})(-[0-9A-Za-z.-]+)?\s*$", clause)
        if not match:
            continue
        version = ".".join((match.group(1).split(".") + ["0", "0"])[:3]) + (match.group(2) or "")
        if lower is None or below(lower, version):
            lower = version
    return lower


def alternatives(names) -> str:
    return "|".join(re.escape(name) for name in sorted(names, key=len, reverse=True)) or "(?!)"


class Scanner:
    def __init__(self, root: Path, inventory: Inventory, published=(), constants=None):
        self.root = root
        self.inventory = inventory
        # The published release inputs (PublishedEntry: a path pattern and an optional match).
        self.published = list(published)
        # Version constants without a product word, configured per file: {path: {name: package key}}.
        self.constants = constants or {}
        self.constants_seen: set[tuple[str, str]] = set()
        self.findings: list[Finding] = []
        packages = inventory.packages.values()
        pypi = "|".join(re.sub(r"(?:\\-|_|\\\.)", "[-_.]", re.escape(name))
                        for name in sorted({p.name for p in packages if p.ecosystem == "pypi"}, key=len, reverse=True))
        specifier = r"(?:===|==|~=|>=|>)[ \t]*v?\d+(?:\.\d+)*(?:\.\*)?"
        bound = r"(?:<=|<|!=)[ \t]*v?\d+(?:\.\d+)*(?:\.\*)?"
        self.requirement_re = re.compile(
            rf"(?<![A-Za-z0-9_./-])({pypi or '(?!)'})(?:\[[^\]\n]*\])?[ \t]*({specifier}"
            rf"(?:[ \t]*,[ \t]*(?:{bound}|{specifier}))*)", re.IGNORECASE)
        self.npm_spec_re = re.compile(rf"(?<![\w/.@-])(@rkat/[a-z0-9-]+)@([\^~]|>=)?({VERSION})")
        self.npm_entry_re = re.compile(r'"(@rkat/[a-z0-9-]+)"[ \t]*:[ \t]*"([^"\n]*)"')
        crates = alternatives({p.name for p in packages if p.ecosystem == "cargo"})
        self.cargo_line_re = re.compile(rf'^[ \t]*({crates})[ \t]*=[ \t]*(?:"([^"\n]*)"|\{{([^}}\n]*)\}})')
        repositories = alternatives((MONOREPO, *RETIRED_REPOSITORIES))
        self.release_re = re.compile(
            rf"github\.com/{OWNER}/({repositories})/releases/(?:download|tag)/([^/\s\"'`)<>\]\\]+)")
        self.retired_git_re = re.compile(
            rf"git\+https://github\.com/{OWNER}/({alternatives(RETIRED_REPOSITORIES)})(?:\.git)?\b")
        self.asset_re = re.compile(
            rf"(?<![A-Za-z0-9_.-])({alternatives(inventory.assets)})-v?({RELEASE_VERSION})(?=[-._][A-Za-z0-9])")
        self.constant_re = re.compile(
            r"\b((?:[A-Z][A-Z0-9_]*)?(?:VERSION|REQUIREMENT))\b[ \t]*(?::[^=\n]{0,40})?=[ \t]*(?:Some\()?([\"'])(=?)"
            rf"({VERSION})\2")
        # Every pin shape holds a dotted version (two parts in a quoted Cargo or npm
        # requirement) or names the repository owner.
        self.cheap_re = re.compile(r"\d\.\d+\.\d|lukacf/|\"[=^~<>]*\d+\.\d+\"")

    def check(self, path, line, shape, pin, key, pinned, movable=False, spec=None):
        if key is None:
            return
        name, expected = self.inventory.expected(key)
        published = next((entry for entry in self.published if entry.applies(path, pin)), None)
        if published is not None:
            published.used += 1
            # A published asset pinned by digest: this tree's version, or the release
            # before it until this version's release is published; never older or newer.
            if pinned == expected:
                return
            previous = self.inventory.previous_release(key)
            if pinned is not None and pinned == previous:
                return
            self.findings.append(Finding(path, line, shape, pin, name, pinned or "", expected, False,
                                         published=True, previous=previous or ""))
            return
        # A requirement must also admit the version here: a ceiling at or below it, a strict
        # floor at it, or a floor or an exact pin above it shuts it out.
        excludes = spec is not None and not range_admits(spec, expected)
        if pinned is None or below(pinned, expected) or excludes:
            self.findings.append(Finding(path, line, shape, pin, name, pinned or "", expected, movable,
                                         excludes=excludes))

    def scan(self, path: str) -> None:
        name = PurePosixPath(path).name
        match = self.asset_re.search(name)
        if match:
            self.check(path, 0, "tracked-artifact", match.group(0), self.inventory.assets[match.group(1)],
                       match.group(2))
        try:
            raw = (self.root / path).read_bytes()
        except OSError:
            return
        if b"\0" in raw[:8192]:
            return
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            return
        if name == "package.json":
            self.scan_package_json(path, text)
        elif name in ("package-lock.json", "npm-shrinkwrap.json"):
            self.scan_package_lock(path, text)
        elif name.endswith(".json"):
            self.scan_json_selections(path, text)
        if self.cheap_re.search(text):
            self.scan_text(path, text, name)

    def scan_package_json(self, path: str, text: str) -> None:
        data = self._json(text)
        if isinstance(data, dict):
            for kind in NPM_DEPENDENCY_MAPS:
                for name, spec in (data.get(kind) or {}).items():
                    self.npm_dependency(path, text, kind, name, spec)

    def npm_dependency(self, path, text, kind, name, spec) -> None:
        if self.inventory.get("npm", name) is None or not isinstance(spec, str) or SAME_COMMIT_SPEC.match(spec):
            return
        floor = npm_floor(spec)
        line = self.line_of(text, f'"{name}": "{spec}"')
        self.check(path, line, kind, f"{name}@{spec}", key_of("npm", name), floor, floor is not None, spec)

    def scan_package_lock(self, path: str, text: str) -> None:
        data = self._json(text)
        packages = data.get("packages") if isinstance(data, dict) else None
        if not isinstance(packages, dict):
            return
        for location, entry in packages.items():
            if not isinstance(entry, dict):
                continue
            if location == "":
                for kind in NPM_DEPENDENCY_MAPS:
                    for name, spec in (entry.get(kind) or {}).items():
                        self.npm_dependency(path, text, kind, name, spec)
                continue
            match = re.search(r"(?:^|/)node_modules/(@rkat/[a-z0-9-]+)$", location)
            if not match or entry.get("link") or self.inventory.get("npm", match.group(1)) is None:
                continue
            version = entry.get("version")
            self.check(path, self.line_of(text, f'"{location}"'), "locked", f"{match.group(1)}@{version} ({location})",
                       key_of("npm", match.group(1)), version if isinstance(version, str) else None)

    def scan_json_selections(self, path: str, text: str) -> None:
        if '"repository"' not in text or '"tag"' not in text:
            return

        def walk(value):
            if isinstance(value, dict):
                repository, tag = value.get("repository"), value.get("tag")
                if isinstance(repository, str) and repository.startswith(f"{OWNER}/") and isinstance(tag, str) and tag:
                    self.release(path, self.line_of(text, f'"{tag}"'), repository.split("/", 1)[1], tag,
                                 f"{repository} {tag}")
                for item in value.values():
                    walk(item)
            elif isinstance(value, list):
                for item in value:
                    walk(item)

        walk(self._json(text))

    def release(self, path, line, repository, tag, pin) -> None:
        if repository in RETIRED_REPOSITORIES:
            # The monorepo's own product tags (v, meta-v, connect-v) mean other
            # things there; only a configured package tag (forge-v) names it.
            resolved = self.inventory.tag(tag)
            key = (resolved[0] if resolved is not None and resolved[0][0] != "product"
                   else ("product", RETIRED_REPOSITORIES[repository]))
            name, expected = self.inventory.expected(key)
            self.findings.append(Finding(path, line, "retired-repository", pin, name, tag, expected))
            return
        resolved = self.inventory.tag(tag) if repository == MONOREPO else None
        if resolved is not None:
            self.check(path, line, "release", pin, resolved[0], resolved[1])

    def scan_text(self, path: str, text: str, name: str) -> None:
        manifest = name in NPM_MANIFESTS
        code = name.endswith(CODE_SUFFIXES)
        # A scan with the cheap pattern finds the candidate lines; the pin patterns
        # run on those lines only, numbered by counting newlines forward.
        last_start, number = 0, 1
        for hit in self.cheap_re.finditer(text):
            start = text.rfind("\n", 0, hit.start()) + 1
            if start < last_start:
                continue
            number += text.count("\n", last_start, start)
            end = text.find("\n", hit.end())
            end = len(text) if end < 0 else end
            last_start = end + 1
            self.scan_line(path, number, text[start:end], name, manifest, code)
            number += 1

    def scan_line(self, path, number, line, name, manifest, code) -> None:
        for match in self.release_re.finditer(line):
            self.release(path, number, match.group(1), match.group(2),
                         f"{OWNER}/{match.group(1)} release {match.group(2)}")
        for match in self.retired_git_re.finditer(line):
            product_name, expected = self.inventory.expected(("product", RETIRED_REPOSITORIES[match.group(1)]))
            self.findings.append(Finding(path, number, "retired-repository", match.group(0), product_name, "",
                                         expected))
        for match in self.asset_re.finditer(line):
            # A release asset whose digest sits beside it is pinned by its bytes: only a new
            # release's published digest moves it.
            self.check(path, number, "asset", match.group(0), self.inventory.assets[match.group(1)],
                       match.group(2), movable=not DIGEST_RE.search(line) and not name.endswith(".json"))
        for match in self.requirement_re.finditer(line):
            self.check(path, number, "requirement", match.group(0), key_of("pypi", match.group(1)),
                       pep440_floor(match.group(2)), "--hash" not in line, match.group(2))
        for match in self.npm_spec_re.finditer(line):
            if self.inventory.get("npm", match.group(1)) is not None:
                self.check(path, number, "npm-spec", match.group(0), key_of("npm", match.group(1)),
                           match.group(3), True, (match.group(2) or "") + match.group(3))
        if code:
            for match in self.constant_re.finditer(line):
                product = self.constant_product(match.group(1))
                if product is not None:
                    self.check(path, number, "constant", match.group(0), ("product", product), match.group(4))
                elif match.group(1) in self.constants.get(path, {}):
                    self.constants_seen.add((path, match.group(1)))
                    self.check(path, number, "constant", match.group(0), self.constants[path][match.group(1)],
                               match.group(4))
        if manifest:
            return
        for match in self.npm_entry_re.finditer(line):
            if self.inventory.get("npm", match.group(1)) is None or SAME_COMMIT_SPEC.match(match.group(2)):
                continue
            floor = npm_floor(match.group(2))
            self.check(path, number, "npm-entry", match.group(0), key_of("npm", match.group(1)), floor,
                       floor is not None, match.group(2))
        match = self.cargo_line_re.match(line)
        if match:
            crate, plain, table = match.group(1), match.group(2), match.group(3)
            if table is not None:
                version = re.search(r'\bversion\s*=\s*"([^"]*)"', table)
                if re.search(r"\b(path|workspace|git)\s*=", table) or not version:
                    return
                plain = version.group(1)
            if plain is not None and re.match(r"^\s*[=^~<>]*\s*v?\d", plain):
                self.check(path, number, "cargo", match.group(0).strip(), key_of("cargo", crate),
                           cargo_floor(plain), True, plain)

    def constant_product(self, name: str):
        words = name.split("_")
        if any(word in NON_PRODUCT_WORDS for word in words):
            return None
        for word in words:
            product = PRODUCT_WORDS.get(word)
            if product is not None and ("product", product) in self.inventory.packages:
                return product
        return None

    @staticmethod
    def _json(text: str):
        try:
            return json.loads(text)
        except ValueError:
            return None

    @staticmethod
    def line_of(text: str, needle: str) -> int:
        index = text.find(needle)
        return text.count("\n", 0, index) + 1 if index >= 0 else 0


def glob_regex(pattern: str) -> re.Pattern:
    regex, index = "", 0
    while index < len(pattern):
        if pattern.startswith("**/", index):
            regex, index = regex + "(?:.*/)?", index + 3
        elif pattern.startswith("**", index):
            regex, index = regex + ".*", index + 2
        elif pattern[index] == "*":
            regex, index = regex + "[^/]*", index + 1
        elif pattern[index] == "?":
            regex, index = regex + "[^/]", index + 1
        else:
            regex, index = regex + re.escape(pattern[index]), index + 1
    return re.compile(rf"^{regex}$")


JSON_SPACE = re.compile(r"[ \t\n\r]*")


def json_value_lines(text: str) -> dict[int, str]:
    """For each line of a JSON document, the key path of the first value that starts on it.

    Object keys are joined by `/` and array items named by their index: the URL of
    `{"files": {"guide/rpc_gateway": {"url": ...}}}` sits under `files/guide/rpc_gateway/url`.
    Raises ValueError when the text is not one JSON value.
    """
    starts = [0] + [match.end() for match in re.finditer("\n", text)]
    decoder = json.JSONDecoder()
    lines: dict[int, str] = {}

    def skip(index: int) -> int:
        return JSON_SPACE.match(text, index).end()

    def expect(index: int, char: str) -> None:
        if text[index:index + 1] != char:
            raise ValueError(f"expected {char!r} at offset {index}")

    def value(index: int, path: tuple[str, ...]) -> int:
        index = skip(index)
        lines.setdefault(bisect.bisect_right(starts, index), "/".join(path))
        opening = text[index:index + 1]
        if opening not in ("{", "["):
            if opening == '"':
                return scanstring(text, index + 1)[1]
            return decoder.raw_decode(text, index)[1]
        closing = "}" if opening == "{" else "]"
        index = skip(index + 1)
        if text[index:index + 1] == closing:
            return index + 1
        position = 0
        while True:
            if opening == "{":
                expect(index, '"')
                key, index = scanstring(text, index + 1)
                index = skip(index)
                expect(index, ":")
                index = value(index + 1, (*path, key))
            else:
                index = value(index, (*path, str(position)))
                position += 1
            index = skip(index)
            if text[index:index + 1] != ",":
                expect(index, closing)
                return index + 1
            index = skip(index + 1)

    end = skip(value(0, ()))
    if end != len(text):
        raise ValueError(f"extra data at offset {end}")
    return lines


@dataclasses.dataclass
class AllowEntry:
    index: int
    path_text: str
    path: re.Pattern
    match: re.Pattern | None
    key: re.Pattern | None = None
    used: int = 0

    @property
    def whole(self) -> bool:
        """An entry without match or key covers whole files, which are not read."""
        return self.match is None and self.key is None

    def label(self) -> str:
        return (self.path_text + (f" ({self.match.pattern})" if self.match else "")
                + (f" [key {self.key.pattern}]" if self.key else ""))


@dataclasses.dataclass
class PublishedEntry:
    """A published release input: a path pattern, and the pins it applies to when it has a match."""
    path_text: str
    path: re.Pattern
    match: re.Pattern | None
    used: int = 0

    def applies(self, path: str, pin: str) -> bool:
        return self.path.match(path) is not None and (self.match is None or self.match.search(pin) is not None)

    def label(self) -> str:
        return self.path_text + (f" ({self.match.pattern})" if self.match else "")


def load_allowlist(config: dict) -> list[AllowEntry]:
    entries = []
    for index, raw in enumerate(config.get("allow") or []):
        if not isinstance(raw, dict) or not isinstance(raw.get("path"), str) or not raw["path"]:
            raise PinError(f"allow entry {index} needs a path")
        unknown = set(raw) - {"path", "match", "key", "reason", "removed_by"}
        if unknown:
            raise PinError(f"allow entry {index} ({raw['path']}) has unknown keys {sorted(unknown)}")
        if not isinstance(raw.get("reason"), str) or len(raw["reason"].split()) < 4:
            raise PinError(f"allow entry {index} ({raw['path']}) needs its reason")
        if "removed_by" in raw and (not isinstance(raw["removed_by"], str) or len(raw["removed_by"].split()) < 3):
            raise PinError(f"allow entry {index} ({raw['path']}) removed_by must name the change that removes it")
        patterns = {}
        for field in ("match", "key"):
            value = raw.get(field)
            if value is not None and (not isinstance(value, str) or not value):
                raise PinError(f"allow entry {index} ({raw['path']}) {field} must be a regular expression")
            patterns[field] = re.compile(value) if value else None
        entries.append(AllowEntry(index, raw["path"], glob_regex(raw["path"]), patterns["match"], patterns["key"]))
    return entries


def load_published(config: dict) -> list[PublishedEntry]:
    """The published release inputs: path patterns, each with an optional match; they need a reason."""
    raw = config.get("published_release_inputs")
    if raw is None:
        return []
    if not isinstance(raw, dict) or set(raw) - {"paths", "reason"}:
        raise PinError("published_release_inputs takes paths and a reason")
    paths = raw.get("paths") or []
    if not isinstance(paths, list):
        raise PinError("published_release_inputs paths must be a list")
    entries = []
    for item in paths:
        if isinstance(item, str) and item:
            entries.append(PublishedEntry(item, glob_regex(item), None))
        elif (isinstance(item, dict) and not set(item) - {"path", "match"} and isinstance(item.get("path"), str)
              and item["path"] and isinstance(item.get("match"), str) and item["match"]):
            entries.append(PublishedEntry(item["path"], glob_regex(item["path"]), re.compile(item["match"])))
        else:
            raise PinError("published_release_inputs paths must be path patterns or {path, match} objects")
    if entries and (not isinstance(raw.get("reason"), str) or len(raw["reason"].split()) < 4):
        raise PinError("published_release_inputs needs its reason")
    return entries


def load_constants(config: dict, inventory: Inventory) -> dict[str, dict[str, tuple[str, str]]]:
    """Version constants whose name names no product, per file: the package each one pins."""
    raw = config.get("version_constants") or {}
    if not isinstance(raw, dict):
        raise PinError("version_constants maps a file to its constants")
    constants = {}
    for path, names in raw.items():
        if not isinstance(names, dict) or not names:
            raise PinError(f"version_constants of {path} must map constant names to packages")
        constants[path] = {name: inventory.target(target, f"version constant {name} of {path}")
                           for name, target in names.items()}
    return constants


def tracked_files(root: Path) -> list[str]:
    result = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=False)
    if result.returncode != 0:
        raise PinError(f"git ls-files failed: {result.stderr.decode(errors='replace').strip()}")
    return sorted(path for path in result.stdout.decode("utf-8").split("\0") if path)


def scan(root: Path, inventory: Inventory, tracked: list[str], allowlist: list[AllowEntry],
         published=(), constants=None) -> Scanner:
    whole = [entry for entry in allowlist if entry.whole]
    scanner = Scanner(root, inventory, published, constants)
    for path in tracked:
        # A published release input is always read: its pins follow their own rule, which
        # no whole-file entry waives.
        covering = [] if any(entry.path.match(path) for entry in published) else \
            [entry for entry in whole if entry.path.match(path)]
        for entry in covering:
            entry.used += 1
        if not covering:
            scanner.scan(path)
    return scanner


def attach_keys(root: Path, findings: list[Finding], allowlist: list[AllowEntry]) -> None:
    """Give each finding in a JSON file that a `key` entry scopes the key path of its line's value."""
    scoped = [entry.path for entry in allowlist if entry.key is not None]
    files: dict[str, dict[int, str]] = {}
    for finding in findings:
        if not finding.line or not finding.path.endswith(".json") or \
                not any(pattern.match(finding.path) for pattern in scoped):
            continue
        if finding.path not in files:
            try:
                files[finding.path] = json_value_lines((root / finding.path).read_text(encoding="utf-8"))
            except ValueError as error:
                raise PinError(f"{finding.path} cannot be read as JSON for an entry with a key: {error}") from error
        finding.key = files[finding.path].get(finding.line, "")


def move_version(text: str, pinned: str, expected: str) -> str:
    return re.sub(rf"(?<![0-9.]){re.escape(pinned)}(?![0-9]|\.[0-9])", expected, text, count=1)


def covering(finding: Finding, allowlist: list[AllowEntry]) -> list[AllowEntry]:
    """The allow entries with a `match` or a `key` that cover a finding."""
    return [entry for entry in allowlist
            if not entry.whole and entry.path.match(finding.path)
            and (entry.match is None or entry.match.search(finding.pin))
            and (entry.key is None or (finding.key is not None and entry.key.search(finding.key)))]


def manifest_file(path: str) -> bool:
    """The files --write edits: dependency manifests and locks, never prose or code."""
    name = PurePosixPath(path).name
    return (name in NPM_MANIFESTS or name in ("pyproject.toml", "Cargo.toml", "setup.cfg")
            or re.fullmatch(r"(?:.*[-_.])?requirements(?:[-_.].*)?\.txt", name) is not None)


def write_pins(root: Path, findings: list[Finding]) -> list[str]:
    """Move every movable finding in a dependency manifest to its expected version, in place."""
    by_path: dict[str, list[Finding]] = {}
    for finding in findings:
        if finding.movable and finding.pinned and manifest_file(finding.path):
            by_path.setdefault(finding.path, []).append(finding)
    changed = []
    for path, items in sorted(by_path.items()):
        full = root / path
        text = full.read_text(encoding="utf-8")
        if PurePosixPath(path).name in NPM_MANIFESTS:
            updated = write_npm_manifest(text, items)
        else:
            lines = text.split("\n")
            for finding in items:
                index = finding.line - 1
                if 0 <= index < len(lines) and finding.pin in lines[index]:
                    moved = move_range(finding.pin, finding.pinned, finding.expected)
                    lines[index] = lines[index].replace(finding.pin, moved, 1)
            updated = "\n".join(lines)
        if updated != text:
            full.write_text(updated, encoding="utf-8")
            changed.append(path)
    return changed


def write_npm_manifest(text: str, items: list[Finding]) -> str:
    """Rewrite the manifest's (and a lock's root package's) dependency ranges, keeping the layout."""
    for finding in items:
        name, _, spec = finding.pin[1:].partition("@") if finding.pin.startswith("@") else finding.pin.partition("@")
        name = ("@" + name) if finding.pin.startswith("@") else name
        old = f'"{name}": "{spec}"'
        new = f'"{name}": "{move_range(spec, finding.pinned, finding.expected)}"'
        text = text.replace(old, new)
    return text


def run(root: Path, config_path: Path, write: bool, out=sys.stdout, err=sys.stderr) -> int:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8")) if config_path.is_file() else {}
        if config and config.get("schema_version") != 1:
            raise PinError(f"{config_path} has an unsupported schema_version")
        tracked = tracked_files(root)
        inventory = Inventory(root, config, tracked)
        allowlist = load_allowlist(config)
        published = load_published(config)
        constants = load_constants(config, inventory)
        scanner = scan(root, inventory, tracked, allowlist, published, constants)
        attach_keys(root, scanner.findings, allowlist)
        if write:
            # An allowed pin (an upgrade fixture, or a pin another change removes)
            # stays as it is: only the pins the check would report move.
            unallowed = [finding for finding in scanner.findings if not covering(finding, allowlist)]
            for path in write_pins(root, unallowed):
                print(f"moved in-repo pins in {path}", file=out)
            allowlist = load_allowlist(config)
            published = load_published(config)
            scanner = scan(root, inventory, tracked, allowlist, published, constants)
            attach_keys(root, scanner.findings, allowlist)
        findings = scanner.findings
    except (PinError, OSError, ValueError, tomllib.TOMLDecodeError, re.error) as error:
        print(f"check_inrepo_version_pins: {error}", file=err)
        return 2
    reported = []
    for finding in findings:
        entries = covering(finding, allowlist)
        for entry in entries:
            entry.used += 1
        if not entries:
            reported.append(finding)
    stale = [entry for entry in allowlist if entry.used == 0]
    stale_paths = [entry.label() for entry in published if not any(entry.path.match(path) for path in tracked)]
    stale_matches = [entry.label() for entry in published
                     if entry.match is not None and entry.used == 0 and entry.label() not in stale_paths]
    stale_constants = [(path, name) for path, names in constants.items() for name in names
                       if (path, name) not in scanner.constants_seen]
    for finding in reported:
        print(finding.describe(), file=err)
    for entry in stale:
        print(f"{CONFIG}: allow entry {entry.index} {entry.label()} covers nothing; delete it", file=err)
    for text in stale_paths:
        print(f"{CONFIG}: published release input {text} matches no tracked file; delete it", file=err)
    for text in stale_matches:
        print(f"{CONFIG}: published release input {text} applies to no pin; delete it", file=err)
    for path, name in stale_constants:
        print(f"{CONFIG}: version constant {name} of {path} is not in that file; delete it", file=err)
    if reported or stale or stale_paths or stale_matches or stale_constants:
        count = len(stale) + len(stale_paths) + len(stale_matches) + len(stale_constants)
        print(f"check_inrepo_version_pins: {len(reported)} pin(s) below, or shutting out, the version of the in-repo "
              f"package they pin, "
              f"{count} stale allow entr{'y' if count == 1 else 'ies'}. Pin the version in this tree, use a "
              f"same-commit reference, or allow the pin in {CONFIG} with its reason.", file=err)
        return 1
    temporary = sum(1 for raw in config.get("allow") or [] if "removed_by" in raw)
    print(f"check_inrepo_version_pins: every in-repo pin names its version in this tree "
          f"({len(allowlist)} allow entries, {temporary} temporary, {len(published)} published release input paths)",
          file=out)
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--config", type=Path, help=f"default: <root>/{CONFIG}")
    parser.add_argument("--write", action="store_true",
                        help="first move every plain-text pin no allow entry covers to the version of the package "
                             "it pins")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    return run(root, args.config or root / CONFIG, args.write)


if __name__ == "__main__":
    sys.exit(main())
