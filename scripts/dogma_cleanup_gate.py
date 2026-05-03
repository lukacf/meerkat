#!/usr/bin/env python3
"""Provisional dogma cleanup review-readiness gate.

The gate is intentionally small: it validates the Review Readiness Packet and
checks changed files for the mechanical failure modes this campaign keeps
admitting. It is not a semantic proof. Typed old-path metadata should replace
the literal reference checks once the follow-up exists.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
from pathlib import PurePosixPath
import re
import subprocess
import sys
from typing import Iterable

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python <3.11 fallback.
    tomllib = None


FORBIDDEN_PROOF_PATTERNS: tuple[tuple[str, str], ...] = (
    (
        r"\b(?:test|fixture|harness|mock)[-_ ]only\b",
        "test-only proof does not prove production boundary uncallability",
    ),
    (r"\bwrapped\b", "wrapped is not an amputation classification"),
    (r"\bwrapper[-_ ]only\b", "wrapper-only cleanup leaves the old path callable"),
    (
        r"\b(?:replacement|compat(?:ibility)?|temporary|thin|new)\s+wrapper\b|"
        r"\bwrapper\s+(?:around|beside|while|over)\b",
        "wrapper cleanup leaves the old path callable",
    ),
    (r"\bmirrored\b", "mirrored is not an amputation classification"),
    (r"\bmirror[-_ ]only\b", "mirror-only cleanup leaves the old path callable"),
    (
        r"\b(?:replacement|compat(?:ibility)?|temporary|thin|new)\s+mirror\b|"
        r"\bmirror\s+(?:around|beside|while|over)\b",
        "mirror cleanup leaves the old path callable",
    ),
    (r"\badapter added\b", "adapter-added cleanup leaves the old path callable"),
    (
        r"\b(?:replacement|compat(?:ibility)?|temporary|thin|new)\s+adapter\b|"
        r"\badapter\s+(?:around|beside|while|over|redirects?)\b",
        "adapter cleanup leaves the old path callable",
    ),
    (r"\bpreferred path exists\b", "preferred-path cleanup leaves the old path callable"),
    (r"\bpreferred[-_ ]path[-_ ]only\b", "preferred-path-only cleanup leaves the old path callable"),
    (
        r"\b(?:remains?|still|kept|left|leaves)\s+callable\b",
        "proof admits the old path remains callable",
    ),
    (
        r"\b(?:callable|retained)\s+for\s+(?:backwards?\s+)?compatibility\b|"
        r"\b(?:backwards?\s+)?compatibility\s+(?:wrapper|adapter|mirror|fallback)\b|"
        r"\b(?:wrapper|adapter|mirror|fallback)\s+for\s+(?:backwards?\s+)?compatibility\b",
        "compatibility-retained proof leaves the old path callable",
    ),
    (
        r"\b(?:remains?|still|kept|left|leaves|stays?|is|are)?\s*"
        r"(?:supported|exported|available|routed|working)\s+for\s+"
        r"(?:backwards?\s+)?compatibility\b",
        "compatibility-retained proof leaves the old path callable",
    ),
    (
        r"\b(?:alias(?:es)?|facades?|shadow[-_ ]routes?|shadow[-_ ]surfaces?)\b"
        r"[^|\n.]*\b(?:rollout|migration|compat(?:ibility)?|legacy|old|previous|retained|"
        r"kept|continues?|routes?|serves?|handles?)\b|"
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:alias(?:es)?|facades?|shadow[-_ ]routes?|shadow[-_ ]surfaces?)\b",
        "alias/facade proof leaves the old path callable",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?)\s+"
        r"(?:remains?|remained|still|stays?|is|are|was|were|continues?|continued|"
        r"keeps?|kept|left)\s+"
        r"(?:as|behind|on|via|through|using|with|under)\s+"
        r"(?:(?:an?|the)\s+)?(?:alias(?:es)?|facades?|shadow[-_ ]routes?|shadow[-_ ]surfaces?)\b",
        "alias/facade proof leaves the old path callable",
    ),
    (
        r"\bcompatibility\s+(?:traffic|requests?|calls?|callers?|clients?|consumers?|"
        r"integrations?)\s+"
        r"(?:is|are|was|were|remains?|remained|still|continues?|continued|can|may)?\s*"
        r"(?:handled|routed|served|accepted|supported|processed|sent|flowing|flows?)\s+"
        r"(?:by|through|via|to|on|against)\s+"
        r"(?:(?:the|a|an)\s+)?(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|"
        r"actions?|aliases?|facades?)\b",
        "compatibility-retained proof leaves the old path callable",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|"
        r"clients?|users?|callers?|sdks?|apps?|tools?|deployments?|access)\s+"
        r"(?:remains?|remained|still|stays?|is|are|was|were|can|may|should|might|must|"
        r"continue(?:s)?(?:\s+to)?|will|would|shall|could|keeps?|kept|left)\s+"
        r"(?:(?:fully|still)\s+)?(?:be\s+)?(?:usable|available|reachable|callable|supported|working|routable|operable|accessible|"
        r"operational|open|in\s+service|exposed|published|grandfathered|"
        r"use|used|using|call|called|calling|reach|reached|reaching|operate|operated|operating|"
        r"access|accessed|accessing|invoke|invokes|invoked|invoking|hit|hits|hitting|"
        r"submit|submits|submitted|submitting|import|imports|imported|importing|"
        r"respond|responds|responded|responding|"
        r"serve|serves|served|serving|accept|accepts|accepted|accepting|"
        r"function|functions|functioned|functioning|handle|handles|handled|handling|"
        r"preserved|enabled|maintained|allowed|retained|kept|importable|invokable|active|live|online)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|"
        r"clients?|users?|callers?|sdks?|apps?|tools?|deployments?|access)\s+"
        r"(?:(?:will|would|shall|could|should|might|must)(?:\s+still)?|"
        r"(?:need|needs|needed)\s+to|"
        r"(?:(?:is|are|was|were)\s+)?(?:expected|intended)\s+to|"
        r"ought\s+to)\s+"
        r"(?:remain|stay|continue(?:s)?(?:\s+to)?(?:\s+be)?|be)\s+"
        r"(?:(?:fully|still)\s+)?(?:usable|available|reachable|callable|supported|working|routable|operable|accessible|"
        r"operational|open|in\s+service|invocable|invokable|importable|active|live|online)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated|older)\s+"
        r"(?:clients?|users?|callers?|sdks?|apps?|tools?|deployments?)\s+"
        r"(?:(?:should|might|must)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:need|needs|needed)\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:(?:is|are|was|were)\s+)?(?:expected|intended)\s+to"
        r"(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"ought\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:can|may|will|would|shall|could)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"still\s+(?:can|may|will|would|shall|could)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:keep|continue)(?:s)?(?:\s+to)?|remain able to)\s+"
        r"(?:use|using|call|calling|reach|reaching|operate|operating|access|accessing|"
        r"invoke|invoking|hit|hitting|submit|submitting|import|importing)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated|older)\s+"
        r"(?:clients?|users?|callers?|consumers?|integrations?|sdks?|apps?|tools?|deployments?)\s+"
        r"(?:(?:should|might|must)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:need|needs|needed)\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:(?:is|are|was|were)\s+)?(?:expected|intended)\s+to"
        r"(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"ought\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:can|may|will|would|shall|could)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"still\s+(?:can|may|will|would|shall|could)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:keep|continue)(?:s)?(?:\s+to)?|remain able to)\s+"
        r"(?:(?:use|using|call|calling|reach|reaching|operate|operating|access|accessing|"
        r"invoke|invoking|hit|hitting|submit|submitting|import|importing)\s+)?"
        r"(?:through|via|against|on|it|them|that|this|the\s+old\s+path)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:callers?|clients?|users?|consumers?|integrations?|sdks?|apps?|tools?|deployments?)\s+"
        r"(?:(?:should|might|must)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:need|needs|needed)\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:(?:is|are|was|were)\s+)?(?:expected|intended)\s+to"
        r"(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"ought\s+to(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:can|may|will|would|shall|could)(?:\s+still)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"still\s+(?:can|may|will|would|shall|could)?(?:\s+(?:keep|continue(?:s)?)(?:\s+to)?)?|"
        r"(?:keep|continue)(?:s)?(?:\s+to)?|remain able to|(?:keeps?|kept)\s+)\s*"
        r"(?:use|using|call|calling|reach|reaching|operate|operating|access|accessing|"
        r"invoke|invoking|hit|hitting|submit|submitting|import|importing)\s+"
        r"(?:(?:requests?|calls?)\s+)?(?:to\s+)?"
        r"(?:(?:the|a|an)\s+)?(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+(?:clients?|users?|callers?|"
        r"consumers?|integrations?)\s+"
        r"(?:are|were|remain|remains|remained|stay|stays|stayed|still)\s+"
        r"(?:served|handled|accepted|routed|supported)\s+by\s+"
        r"(?:(?:the|a|an)\s+)?(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+(?:clients?|users?|callers?|"
        r"consumers?|integrations?)\s+"
        r"(?:remain|remains|remained|stay|stays|stayed|continue(?:s)?|continued)\s+"
        r"(?:on|at|against|with|behind)\s+"
        r"(?:(?:the|a|an)\s+)?(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:(?:the|that|this)\s+)?"
        r"(?:(?:old|legacy|previous|existing|former|deprecated)\s+)?"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|"
        r"clients?|users?|callers?|consumers?|integrations?|access)\s+"
        r"(?:remains?|still|stays?|is|are|was|were|kept|left|continue(?:s)?(?:\s+to)?)\s+"
        r"(?:(?:fully|still)\s+)?(?:usable|available|reachable|callable|supported|working|routable|operable|accessible|"
        r"operational|open|in\s+service|"
        r"respond|responding|serve|serving|accept|accepting|function|functioning|handle|handled|"
        r"work|preserved|enabled|maintained|allowed|exposed|published|grandfathered|"
        r"retained|kept|left|"
        r"active|live|online)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:keeps?|keeping|kept|leaves?|leaving|left|retains?|retaining|retained|"
        r"preserves?|preserving|preserved|enables?|enabling|enabled|maintains?|maintaining|"
        r"maintained|allows?|allowing|allowed)\s+"
        r"(?:(?:the|that|this)\s+)?(?:(?:old|legacy|previous|existing|former|deprecated)\s+)?"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|"
        r"clients?|users?|callers?|consumers?|integrations?|access)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:keeps?|kept|leaves?|left|retains?|retained|preserves?|preserved|enables?|enabled|"
        r"maintains?|maintained|allows?|allowed)\s+"
        r"(?:(?:the|that|this)\s+)?(?:(?:old|legacy|previous|existing|former|deprecated)\s+)?"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|access)\s+"
        r"(?:(?:fully|still)\s+)?(?:usable|available|reachable|callable|supported|working|routable|operable|accessible|"
        r"operational|open|in\s+service|"
        r"responds|responding|serves|serving|accepts|accepting|functions|functioning|"
        r"handles|handling|preserved|enabled|maintained|allowed|exposed|published|"
        r"grandfathered|active|live|online)\b",
        "proof admits old callers can still use the old path",
    ),
    (
        r"\b(?:old|legacy|previous|existing|former|deprecated)\s+"
        r"(?:helpers?|paths?|routes?|endpoints?|apis?|services?|surfaces?|exports?|actions?|"
        r"clients?|users?|callers?|consumers?|integrations?|access)\b"
        r"[^|\n.]{0,120}\b(?:semver|abi|api|compat(?:ibility)?)\s+stability\b",
        "compatibility-retained proof leaves the old path callable",
    ),
)

UNCALLABLE_PROOF_PATTERNS: tuple[re.Pattern[str], ...] = (
    re.compile(
        r"\b(?:compile(?:[- ]fail(?:ure|s)?|s? fails?)|does not compile|compiler rejects)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(?:deny|denies|denied|reject|rejects|rejected|block|blocks|blocked)\s+"
        r"(?:calls?|requests?|routes?|exports?|access)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(?:calls?|requests?|routes?|exports?|access)\s+"
        r"(?:are|is)\s+(?:denied|rejected|blocked)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(?:not|no longer)\s+(?:exported|routed|registered|callable|reachable|available)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(?:route|export|api|symbol|handler)\s+(?:removed|deleted|unregistered)\b",
        re.IGNORECASE,
    ),
    re.compile(r"\b(?:not routable|not callable|boundary[- ]uncallable)\b", re.IGNORECASE),
)

NON_PRODUCTION_PROOF_CONTEXT = re.compile(
    r"\b(?:unit\s+tests?|test\s+fixtures?|fixtures?|harness(?:es)?|mocks?|mocked|synthetic)\b",
    re.IGNORECASE,
)

PRODUCTION_BOUNDARY_CONTEXT = re.compile(
    r"\b(?:type\s+system|compiler|route\s+registry|router|handler|"
    r"export|symbol|api|public\s+(?:route|api|export|handler|symbol)|"
    r"production\s+(?:router|handler|route|api|binary)|"
    r"runtime\s+(?:rejects?|denies|denied|blocks?|blocked|removed|removes|"
    r"unregistered|unregisters|no\s+longer\s+(?:routes?|exports?|calls?|accepts?)))\b",
    re.IGNORECASE,
)

PUBLIC_CARRIER_NAME = re.compile(
    r"""
    ^\+\s*
    (?:
      (?:pub\s+)?(?:struct|enum|type|trait|fn|const)\s+
      [A-Za-z0-9_]*(?:Legacy|Compat|Compatibility|Fallback|Mirror|Carrier|Bag|Cache|Shadow|RawJson|Json)[A-Za-z0-9_]*
      |
      export\s+(?:interface|type|class|function|const)\s+
      [A-Za-z0-9_]*(?:Legacy|Compat|Compatibility|Fallback|Mirror|Carrier|Bag|Cache|Shadow|RawJson|Json)[A-Za-z0-9_]*
      |
      pub\s+[A-Za-z0-9_]*(?:legacy|compat|fallback|mirror|carrier|bag|cache|shadow|raw_json|json)[A-Za-z0-9_]*\s*:
    )
    """,
    re.VERBOSE,
)

COMPAT_SHIM_LINE = re.compile(
    r"\b(?:compat(?:ibility)? shim|legacy fallback|fallback compatibility|"
    r"wrapper[- ]only|mirror[- ]only|adapter added|preferred path exists|"
    r"preferred[- ]path[- ]only|"
    r"temporary (?:adapter|wrapper|mirror)|backwards? compatibility)\b",
    re.IGNORECASE,
)

DOGMA_CLEANUP_SIGNAL = re.compile(
    r"\b(?:dogma cleanup|dogma-cleanup|dogma violations?|dogma ledger|"
    r"canonicalization beside the old path)\b",
    re.IGNORECASE,
)

DOGMA_CLEANUP_BODY_MARKER = re.compile(
    r"(?im)^\s*(?:[-*]\s*)?(?:dogma cleanup|dogma-cleanup)(?:\s+pr)?\s*:\s*(?:yes|true|required)\b"
)

DOGMA_CLEANUP_DIRECT_PATH = re.compile(
    r"(^|/)(?:dogma|dogma[-_]cleanup)(?:[-_/]|$)",
    re.IGNORECASE,
)

DOGMA_CLEANUP_REVIEW_PATHS: tuple[tuple[re.Pattern[str], str], ...] = (
    (
        re.compile(
            r"^(?:docs/(?:process/)?dogma|docs/.*dogma|scripts/.*dogma|"
            r"\.github/PULL_REQUEST_TEMPLATE\.md|\.github/workflows/ci\.yml)",
            re.IGNORECASE,
        ),
        "dogma review process path",
    ),
    (
        re.compile(
            r"^(?:meerkat-runtime/|meerkat-rpc/src/(?:session_runtime\.rs|handlers/auth\.rs)|"
            r"meerkat-rest/src/(?:auth|lib\.rs)|meerkat-auth-core/|"
            r"meerkat-machine-schema/src/catalog/dsl/auth_machine\.rs|specs/machines/auth|"
            r"meerkat-core/src/generated/|meerkat-wasm/|sdks/|xtask/|scripts/)",
            re.IGNORECASE,
        ),
        "known dogma cleanup surface",
    ),
)

NON_CARGO_DOGMA_CAMPAIGN_SURFACE_PATHS: tuple[tuple[re.Pattern[str], str], ...] = (
    (
        re.compile(r"^(?:sdks/[^/]+/src/|xtask/src/)", re.IGNORECASE),
        "known dogma campaign source deletion",
    ),
    (
        re.compile(r"^specs/(?:machines|compositions)/", re.IGNORECASE),
        "known dogma campaign source deletion",
    ),
)

NON_PRODUCTION_WORKSPACE_MEMBER_PREFIXES = (
    "test-fixtures/",
    "tests/",
    "vendor/",
)

NON_PRODUCTION_WORKSPACE_MEMBERS = {
    "xtask",
}

DOGMA_GATE_OWNING_PATHS: tuple[tuple[re.Pattern[str], str], ...] = (
    (re.compile(r"^Makefile$"), "dogma gate-owning path"),
    (
        re.compile(r"^\.github/(?:PULL_REQUEST_TEMPLATE\.md|workflows/ci\.yml)$"),
        "dogma gate-owning path",
    ),
    (
        re.compile(
            r"^scripts/(?:dogma_cleanup_gate\.py|tests/dogma_cleanup_gate_test\.py|"
            r"fixtures/dogma_cleanup_gate/)"
        ),
        "dogma gate-owning path",
    ),
    (re.compile(r"^docs/process/dogma-cleanup-gate\.md$"), "dogma gate-owning path"),
)

DOGMA_CLEANUP_DIFF_SIGNAL = re.compile(
    r"(?<![A-Za-z0-9])(?:dogma|old[-_ ]path|old authority|local authority|public carrier|"
    r"canonicali[sz]ation beside the old path|runtime metadata|surface request lifecycle|"
    r"machine governance|legacy|compat(?:ibility)?|fallback|mirror(?:ed)?|"
    r"carrier|cache|json bag|raw[_ -]?json|string mirror|surface|"
    r"wrapper[-_ ]only|adapter|preferred[-_ ]path|route removed|not exported|"
    r"AuthMachine|OAuth|SDK|WASM|scanner|allowlist)(?![A-Za-z0-9])",
    re.IGNORECASE,
)

STRUCTURAL_SOURCE_DELETION = re.compile(
    r"""
    ^-\s*
    (?:
      (?:pub(?:\([^)]*\))?\s+)?
      (?:(?:async|unsafe|extern)\s+)*
      (?:fn|struct|enum|trait|type|const|static|mod)\s+[A-Za-z_][A-Za-z0-9_]*
      |
      (?:pub(?:\([^)]*\))?\s+)?[A-Za-z_][A-Za-z0-9_]*\s*:\s*
      |
      export\s+(?:interface|type|class|function|const)\s+[A-Za-z_][A-Za-z0-9_]*
      |
      (?:unsafe\s+)?
      impl(?:\s*<[^>{}]+>)?\s+
      (?:
        [A-Za-z_][A-Za-z0-9_:]*(?:\s*<[^>{}]+>)?\s+for\s+
      )?
      [A-Za-z_][A-Za-z0-9_:]*(?:\s*<[^>{}]+>)?\s*(?:where\b[^{}]*)?\{?
      |
      (?:router|route|routes|endpoint|handler)\s*[.(]
      |
      [A-Z][A-Za-z0-9_]*(?:\s*\([^)]*\)|\s*\{[^}]*\})?\s*,?\s*(?://.*)?
    )
    """,
    re.VERBOSE,
)

GENERATED_OR_SCHEMA_PATH = re.compile(
    r"(^schemas/|^schema/|/schemas?/|^artifacts/.*(?:schema|generated|sdk)|"
    r"(?:^|/)src/generated/|"
    r"^sdks/[^/]+/(?:dist|generated|src/generated)/|"
    r"(?:^|/)(?:openapi|schema|schemas)[^/]*\.(?:json|yaml|yml)$|"
    r"\.(?:schema|schemas)\.(?:json|yaml|yml)$)",
    re.IGNORECASE,
)

IMPLEMENTATION_SUFFIXES = {
    ".rs",
    ".py",
    ".ts",
    ".tsx",
    ".js",
    ".mjs",
    ".cjs",
    ".sh",
    ".toml",
    ".json",
    ".yaml",
    ".yml",
}


@dataclass(frozen=True)
class ProofEntry:
    old_path: str
    classification: str
    search_literal: str
    proof: str
    follow_up: str


@dataclass(frozen=True)
class AddedLine:
    path: str
    line: str


@dataclass(frozen=True)
class ChangedLine:
    path: str
    line: str


@dataclass(frozen=True)
class GitHubPullRequest:
    title: str
    body: str
    labels: tuple[str, ...]
    head_sha: str


def fail(message: str) -> None:
    print(f"dogma cleanup gate failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def run(cmd: list[str], cwd: Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=check)


def git_output(args: list[str], cwd: Path) -> str:
    try:
        return run(["git", *args], cwd=cwd).stdout.strip()
    except subprocess.CalledProcessError as exc:
        detail = exc.stderr.strip() or exc.stdout.strip()
        fail(f"git {' '.join(args)} failed: {detail}")


def git_ref_exists(ref: str, cwd: Path) -> bool:
    result = run(["git", "rev-parse", "--verify", "--quiet", ref], cwd=cwd, check=False)
    return result.returncode == 0


def resolve_base_ref(repo: Path, requested: str) -> str:
    if git_ref_exists(requested, repo):
        return requested
    if requested == "origin/main":
        for fallback in ("main", "master"):
            if git_ref_exists(fallback, repo):
                return fallback
    fail(f"base ref `{requested}` was not found; set DOGMA_CLEANUP_BASE or pass --base")


def normalize_heading(value: str) -> str:
    value = re.sub(r"[`*_]", "", value).strip().lower()
    return re.sub(r"[^a-z0-9]+", " ", value).strip()


def markdown_headings(markdown: str) -> list[tuple[int, str, int, int]]:
    headings: list[tuple[int, str, int, int]] = []
    in_fence = False
    fence_marker = ""
    offset = 0
    for line in markdown.splitlines(keepends=True):
        fence = re.match(r"^\s*(```+|~~~+)", line)
        if fence:
            marker = fence.group(1)
            if in_fence and marker[0] == fence_marker[0] and len(marker) >= len(fence_marker):
                in_fence = False
                fence_marker = ""
            elif not in_fence:
                in_fence = True
                fence_marker = marker
            offset += len(line)
            continue
        if not in_fence:
            heading = re.match(r"^(#{1,6})\s+(.+?)\s*$", line.rstrip("\r\n"))
            if heading:
                headings.append((len(heading.group(1)), heading.group(2), offset, offset + len(line)))
        offset += len(line)
    return headings


def find_section(markdown: str, heading: str) -> str | None:
    wanted = normalize_heading(heading)
    headings = markdown_headings(markdown)
    for index, (level, title, start_offset, end_offset) in enumerate(headings):
        if normalize_heading(title) != wanted:
            continue
        start = end_offset
        end = len(markdown)
        for later_level, _later_title, later_start, _later_end in headings[index + 1 :]:
            if later_level <= level:
                end = later_start
                break
        return markdown[start:end]
    return None


def require_heading(markdown: str, heading: str) -> str:
    section = find_section(markdown, heading)
    if section is None:
        fail(f"Review Readiness Packet is missing required heading: {heading}")
    return section


def split_table_row(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def is_separator_row(line: str) -> bool:
    cells = split_table_row(line)
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell.strip()) for cell in cells)


def strip_cell(value: str) -> str:
    value = value.replace("<br>", " ").replace("<br/>", " ").replace("<br />", " ")
    value = value.strip()
    if value.startswith("`") and value.endswith("`") and len(value) >= 2:
        value = value[1:-1]
    return value.strip()


def header_key(value: str) -> str:
    value = strip_cell(value).lower()
    return re.sub(r"[^a-z0-9]+", "_", value).strip("_")


def parse_proof_entries(section: str) -> list[ProofEntry]:
    lines = section.splitlines()
    for index, line in enumerate(lines):
        if not line.lstrip().startswith("|"):
            continue
        if index + 1 >= len(lines) or not is_separator_row(lines[index + 1]):
            continue
        headers = [header_key(cell) for cell in split_table_row(line)]
        if "old_path" not in headers or "classification" not in headers:
            continue
        rows: list[ProofEntry] = []
        for row_line in lines[index + 2 :]:
            if not row_line.lstrip().startswith("|"):
                break
            cells = split_table_row(row_line)
            if len(cells) < len(headers):
                cells.extend([""] * (len(headers) - len(cells)))
            row = {headers[cell_index]: strip_cell(cells[cell_index]) for cell_index in range(len(headers))}
            old_path = row.get("old_path", "")
            classification = row.get("classification", "")
            search_literal = row.get("search_literal") or old_path
            proof = row.get("mechanical_proof", "") or row.get("proof", "")
            follow_up = row.get("follow_up", "") or row.get("followup", "")
            rows.append(
                ProofEntry(
                    old_path=old_path,
                    classification=classification,
                    search_literal=search_literal,
                    proof=proof,
                    follow_up=follow_up,
                )
            )
        return rows
    return []


def canonical_classification(value: str) -> str | None:
    normalized = re.sub(r"[^a-z0-9]+", " ", strip_cell(value).lower()).strip()
    if normalized == "deleted":
        return "deleted"
    if normalized in {
        "uncallable",
        "made uncallable",
        "made uncallable at boundary",
        "uncallable at boundary",
        "boundary uncallable",
    }:
        return "uncallable"
    if normalized in {
        "retained follow up",
        "retained with linked follow up",
        "intentionally retained with linked follow up",
        "intentionally retained",
    }:
        return "retained"
    return None


def proof_has_forbidden_language(section: str) -> list[str]:
    messages: list[str] = []
    for pattern, message in FORBIDDEN_PROOF_PATTERNS:
        if re.search(pattern, section, flags=re.IGNORECASE):
            messages.append(message)
    return messages


def has_uncallable_boundary_proof(proof: str) -> bool:
    return any(pattern.search(proof) is not None for pattern in UNCALLABLE_PROOF_PATTERNS)


def uses_non_production_only_boundary_proof(proof: str) -> bool:
    return (
        NON_PRODUCTION_PROOF_CONTEXT.search(proof) is not None
        and has_uncallable_boundary_proof(proof)
        and PRODUCTION_BOUNDARY_CONTEXT.search(proof) is None
    )


def validate_packet(markdown: str, head_sha: str) -> list[ProofEntry]:
    readiness_section = require_heading(markdown, "Review Readiness Packet")
    proof_section = require_heading(markdown, "Old Path Amputation Proof")
    require_heading(markdown, "Complexity Delta")
    require_heading(markdown, "Sample Passing Old Path Amputation Proof")
    require_heading(markdown, "Sample Failing Old Path Amputation Proof")

    head_match = re.search(
        r"(?im)^\s*Current head SHA\s*:\s*`?([0-9a-f]{40})`?\s*$",
        readiness_section,
    )
    if not head_match:
        fail("Review Readiness Packet must include a `Current head SHA: <sha>` line")
    packet_head = head_match.group(1)
    if packet_head != head_sha:
        fail(f"Review Readiness Packet Current head SHA must be {head_sha}")
    if not re.search(r"\bcommand output\b", markdown, flags=re.IGNORECASE):
        fail("Review Readiness Packet must include command output")

    entries = parse_proof_entries(proof_section)
    if not entries:
        fail("Old Path Amputation Proof must contain a markdown table with Old path and Classification columns")

    section_forbidden = proof_has_forbidden_language(proof_section)
    if section_forbidden:
        fail("Old Path Amputation Proof uses non-amputating proof language: " + "; ".join(section_forbidden))

    for entry in entries:
        forbidden = proof_has_forbidden_language(
            "\n".join([entry.classification, entry.proof, entry.follow_up])
        )
        if forbidden:
            fail(
                f"Old Path Amputation Proof row `{entry.old_path}` uses non-amputating proof language: "
                + "; ".join(forbidden)
            )
        if not entry.old_path or entry.old_path.lower() in {"n/a", "none", "not applicable"}:
            fail("each Old Path Amputation Proof row must name an old path")
        classification = canonical_classification(entry.classification)
        if classification is None:
            fail(
                "Old Path Amputation Proof classifications must be deleted, "
                "made uncallable at boundary, or retained with linked follow-up"
            )
        if classification in {"deleted", "uncallable"}:
            literal = entry.search_literal.strip()
            if len(literal) < 3:
                fail(
                    f"{classification} old path `{entry.old_path}` needs a search literal "
                    "of at least 3 characters"
                )
            if not literal_names_old_path(entry):
                fail(
                    f"{classification} old path `{entry.old_path}` search literal `{literal}` "
                    "must name or overlap the old path"
                )
        if classification == "retained":
            if not re.search(r"\bLUC-\d+\b|https?://", entry.follow_up):
                fail(f"retained old path `{entry.old_path}` must link a follow-up")
            fail(
                f"retained old path `{entry.old_path}` is non-clean residual work; "
                "split it out or move the PR to Rework until the old path is deleted "
                "or made uncallable"
            )
        if classification == "uncallable":
            if uses_non_production_only_boundary_proof(entry.proof):
                fail(
                    f"uncallable old path `{entry.old_path}` cites only non-production fixture, "
                    "harness, mock, or unit-test evidence; cite production-boundary evidence"
                )
            if not has_uncallable_boundary_proof(entry.proof):
                fail(
                    f"uncallable old path `{entry.old_path}` must cite mechanical boundary proof "
                    "such as compile failure, rejected calls, removed route/export, or not-callable evidence"
                )
    return entries


def changed_files(repo: Path, base: str) -> list[str]:
    merge_base = git_output(["merge-base", base, "HEAD"], repo)
    outputs = [
        git_output(["diff", "--name-only", "--diff-filter=ACDMR", f"{merge_base}..HEAD", "--"], repo),
        git_output(["diff", "--cached", "--name-only", "--diff-filter=ACDMR", "--"], repo),
        git_output(["diff", "--name-only", "--diff-filter=ACDMR", "--"], repo),
        git_output(["ls-files", "--others", "--exclude-standard"], repo),
    ]
    seen: set[str] = set()
    files: list[str] = []
    for output in outputs:
        for line in output.splitlines():
            if line and line not in seen:
                seen.add(line)
                files.append(line)
    return files


def added_lines(repo: Path, base: str) -> list[AddedLine]:
    merge_base = git_output(["merge-base", base, "HEAD"], repo)
    diffs = [
        git_output(["diff", "--unified=0", "--no-ext-diff", f"{merge_base}..HEAD", "--"], repo),
        git_output(["diff", "--cached", "--unified=0", "--no-ext-diff", "--"], repo),
        git_output(["diff", "--unified=0", "--no-ext-diff", "--"], repo),
    ]
    current_path = ""
    lines: list[AddedLine] = []
    for diff in diffs:
        for line in diff.splitlines():
            if line.startswith("+++ b/"):
                current_path = line[6:]
                continue
            if line.startswith("+") and not line.startswith("+++"):
                lines.append(AddedLine(current_path, line))
    untracked = git_output(["ls-files", "--others", "--exclude-standard"], repo)
    for rel_path in untracked.splitlines():
        path = repo / rel_path
        try:
            for line in path.read_text(encoding="utf-8").splitlines():
                lines.append(AddedLine(rel_path, f"+{line}"))
        except (FileNotFoundError, UnicodeDecodeError):
            continue
    return lines


def changed_diff_lines(repo: Path, base: str) -> list[ChangedLine]:
    merge_base = git_output(["merge-base", base, "HEAD"], repo)
    diffs = [
        git_output(["diff", "--unified=0", "--no-ext-diff", f"{merge_base}..HEAD", "--"], repo),
        git_output(["diff", "--cached", "--unified=0", "--no-ext-diff", "--"], repo),
        git_output(["diff", "--unified=0", "--no-ext-diff", "--"], repo),
    ]
    lines: list[ChangedLine] = []
    for diff in diffs:
        current_path = ""
        old_path = ""
        for line in diff.splitlines():
            if line.startswith("--- a/"):
                old_path = line[6:]
                if not current_path:
                    current_path = old_path
                continue
            if line.startswith("+++ b/"):
                current_path = line[6:]
                continue
            if line == "+++ /dev/null":
                current_path = old_path
                continue
            if line.startswith(("+", "-")) and not line.startswith(("+++", "---")):
                lines.append(ChangedLine(current_path, line))
    untracked = git_output(["ls-files", "--others", "--exclude-standard"], repo)
    for rel_path in untracked.splitlines():
        path = repo / rel_path
        try:
            for line in path.read_text(encoding="utf-8").splitlines():
                lines.append(ChangedLine(rel_path, f"+{line}"))
        except (FileNotFoundError, UnicodeDecodeError):
            continue
    return lines


def is_generated_or_schema(path: str) -> bool:
    return GENERATED_OR_SCHEMA_PATH.search(path) is not None


def is_docs_scripts_or_tests(path: str) -> bool:
    if path == "Makefile":
        return True
    if path.startswith(("docs/", "scripts/", ".github/", "tests/")):
        return True
    if "/tests/" in path or "/fixtures/" in path or path.endswith(("_test.rs", ".md", ".mdx")):
        return True
    return False


def is_impl_scan_path(path: str) -> bool:
    if is_docs_scripts_or_tests(path):
        return False
    return Path(path).suffix in IMPLEMENTATION_SUFFIXES


def is_reference_scan_path(path: str) -> bool:
    if path.startswith(("docs/", ".github/", "tests/")):
        return False
    if "/tests/" in path or "/fixtures/" in path or path.endswith((".md", ".mdx")):
        return False
    return Path(path).suffix in IMPLEMENTATION_SUFFIXES


def validate_complexity_delta(markdown: str, files: list[str]) -> None:
    section = require_heading(markdown, "Complexity Delta")
    required = {
        "Docs/scripts/tests": r"^\s*[-*]?\s*Docs/scripts/tests\s*:\s*(.+)$",
        "Non-test implementation": r"^\s*[-*]?\s*Non-test implementation\s*:\s*(.+)$",
        "Generated/schema churn": r"^\s*[-*]?\s*Generated/schema churn\s*:\s*(.+)$",
    }
    found: dict[str, str] = {}
    for label, pattern in required.items():
        match = re.search(pattern, section, flags=re.IGNORECASE | re.MULTILINE)
        if not match:
            fail(f"Complexity Delta must separate `{label}`")
        found[label] = match.group(1).strip()

    generated = [path for path in files if is_generated_or_schema(path)]
    generated_line = found["Generated/schema churn"]
    if generated:
        if re.search(r"\b(?:0|none|no)\b", generated_line, flags=re.IGNORECASE):
            fail(
                "Complexity Delta says generated/schema churn is absent, "
                f"but changed generated/schema paths exist: {', '.join(generated)}"
            )
        if str(len(generated)) not in generated_line:
            fail(
                "Complexity Delta must include the generated/schema changed-file count "
                f"({len(generated)}) when generated/schema paths changed"
            )


def validate_diff_hygiene(lines: Iterable[AddedLine]) -> None:
    public_carrier_hits: list[str] = []
    shim_hits: list[str] = []
    for added in lines:
        if not is_impl_scan_path(added.path):
            continue
        if PUBLIC_CARRIER_NAME.search(added.line):
            public_carrier_hits.append(f"{added.path}: {added.line[1:].strip()}")
        if COMPAT_SHIM_LINE.search(added.line):
            shim_hits.append(f"{added.path}: {added.line[1:].strip()}")
    if public_carrier_hits:
        fail("new public carrier-like names found:\n  " + "\n  ".join(public_carrier_hits))
    if shim_hits:
        fail("broad compatibility shim language found in added implementation lines:\n  " + "\n  ".join(shim_hits))


def tracked_files(repo: Path) -> list[str]:
    outputs = [
        git_output(["ls-files"], repo),
        git_output(["ls-files", "--others", "--exclude-standard"], repo),
    ]
    seen: set[str] = set()
    files: list[str] = []
    for output in outputs:
        for line in output.splitlines():
            if line and line not in seen:
                seen.add(line)
                files.append(line)
    return files


def production_ref_paths(paths: Iterable[str]) -> list[str]:
    selected: list[str] = []
    for path in paths:
        if is_reference_scan_path(path):
            selected.append(path)
    return selected


def normalize_reference_fragment(value: str) -> str:
    normalized = re.sub(r"[^a-z0-9]+", " ", value.lower()).strip()
    return re.sub(r"\s+", " ", normalized)


def workspace_members_from_manifest_text(text: str) -> tuple[str, ...]:
    if tomllib is not None:
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            return ()
        workspace = data.get("workspace", {})
        members = workspace.get("members", []) if isinstance(workspace, dict) else []
        return tuple(member for member in members if isinstance(member, str))

    members_match = re.search(r"(?ms)^\s*members\s*=\s*\[(.*?)\]", text)
    if members_match is None:
        return ()
    return tuple(re.findall(r'"([^"]+)"', members_match.group(1)))


def workspace_members_from_manifest(repo: Path) -> tuple[str, ...]:
    manifest = repo / "Cargo.toml"
    try:
        text = manifest.read_text(encoding="utf-8")
    except FileNotFoundError:
        return ()
    return workspace_members_from_manifest_text(text)


def git_file_text(repo: Path, ref: str, path: str) -> str | None:
    result = run(["git", "show", f"{ref}:{path}"], cwd=repo, check=False)
    if result.returncode != 0:
        return None
    return result.stdout


def workspace_members_from_git_ref(repo: Path, ref: str) -> tuple[str, ...]:
    text = git_file_text(repo, ref, "Cargo.toml")
    if text is None:
        return ()
    return workspace_members_from_manifest_text(text)


def expand_workspace_member(repo: Path, member: str) -> list[str]:
    normalized = member.strip().strip("/")
    if not normalized:
        return []
    if "*" not in normalized:
        return [PurePosixPath(normalized).as_posix()]
    expanded: list[str] = []
    for path in sorted(repo.glob(normalized)):
        if path.is_dir():
            expanded.append(path.relative_to(repo).as_posix())
    return expanded


def expand_workspace_member_at_ref(repo: Path, ref: str, member: str) -> list[str]:
    normalized = member.strip().strip("/")
    if not normalized:
        return []
    if "*" not in normalized:
        return [PurePosixPath(normalized).as_posix()]
    result = run(["git", "ls-tree", "-r", "--name-only", ref, "--"], cwd=repo, check=False)
    if result.returncode != 0:
        return []
    expanded: list[str] = []
    for file_path in result.stdout.splitlines():
        if not file_path.endswith("/Cargo.toml"):
            continue
        candidate = file_path.removesuffix("/Cargo.toml")
        if PurePosixPath(candidate).match(normalized):
            expanded.append(PurePosixPath(candidate).as_posix())
    return sorted(set(expanded))


def is_production_workspace_member(member: str) -> bool:
    normalized = PurePosixPath(member.strip().strip("/")).as_posix()
    if normalized in NON_PRODUCTION_WORKSPACE_MEMBERS:
        return False
    return not any(normalized.startswith(prefix) for prefix in NON_PRODUCTION_WORKSPACE_MEMBER_PREFIXES)


def production_workspace_src_roots(repo: Path, base: str | None = None) -> tuple[str, ...]:
    roots: list[str] = []
    seen: set[str] = set()

    def add_root(member_path: str) -> None:
        if not is_production_workspace_member(member_path):
            return
        root = f"{member_path}/src/"
        if root not in seen:
            seen.add(root)
            roots.append(root)

    for member in workspace_members_from_manifest(repo):
        for expanded in expand_workspace_member(repo, member):
            add_root(expanded)
    if base is not None:
        merge_base = git_output(["merge-base", base, "HEAD"], repo)
        for member in workspace_members_from_git_ref(repo, merge_base):
            for expanded in expand_workspace_member_at_ref(repo, merge_base, member):
                add_root(expanded)
    return tuple(roots)


def production_workspace_source_path_reason(repo: Path, path: str, base: str | None = None) -> str | None:
    if not is_reference_scan_path(path) or is_generated_or_schema(path):
        return None
    normalized = PurePosixPath(path).as_posix()
    for src_root in production_workspace_src_roots(repo, base):
        if normalized.startswith(src_root):
            return "production workspace crate source deletion"
    return None


def literal_matches(repo: Path, literal: str, paths: Iterable[str]) -> list[str]:
    matches: list[str] = []
    if not literal or literal.lower() in {"none", "n/a", "not applicable"}:
        return matches
    normalized_literal = normalize_reference_fragment(literal)
    for rel_path in paths:
        path = repo / rel_path
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        except FileNotFoundError:
            continue
        for index, line in enumerate(text.splitlines(), start=1):
            normalized_line = normalize_reference_fragment(line)
            if literal in line or (
                normalized_literal and normalized_literal in normalized_line
            ):
                matches.append(f"{rel_path}:{index}:{line.strip()}")
    return matches


GENERIC_OLD_PATH_TOKENS = {
    "old",
    "legacy",
    "previous",
    "existing",
    "former",
    "deprecated",
    "path",
    "paths",
    "route",
    "routes",
    "endpoint",
    "endpoints",
    "api",
    "apis",
    "helper",
    "helpers",
    "service",
    "services",
    "surface",
    "surfaces",
    "export",
    "exports",
    "action",
    "actions",
}


def literal_names_old_path(entry: ProofEntry) -> bool:
    old_path = strip_cell(entry.old_path)
    literal = strip_cell(entry.search_literal)
    old_norm = normalize_reference_fragment(old_path)
    literal_norm = normalize_reference_fragment(literal)
    if not old_norm or not literal_norm:
        return False
    if old_norm in literal_norm or literal_norm in old_norm:
        return True
    old_tokens = {
        token
        for token in re.findall(r"[a-z0-9_]{3,}", old_path.lower())
        if token not in GENERIC_OLD_PATH_TOKENS
    }
    literal_tokens = {
        token
        for token in re.findall(r"[a-z0-9_]{3,}", literal.lower())
        if token not in GENERIC_OLD_PATH_TOKENS
    }
    return bool(old_tokens & literal_tokens)


def validate_old_path_references(repo: Path, entries: Iterable[ProofEntry]) -> None:
    paths = production_ref_paths(tracked_files(repo))
    for entry in entries:
        classification = canonical_classification(entry.classification)
        if classification not in {"deleted", "uncallable"}:
            continue
        literal = entry.search_literal.strip()
        matches = literal_matches(repo, literal, paths)
        if not matches:
            continue
        if classification == "deleted":
            fail(
                f"deleted old path `{entry.old_path}` still has production references for `{literal}`:\n  "
                + "\n  ".join(matches[:20])
            )
        fail(
            f"uncallable old path `{entry.old_path}` still has production references for `{literal}`; "
            "remove the callable production carrier, prove it as inert/fail-closed code, or classify "
            "remaining work as retained with linked follow-up:\n  "
            + "\n  ".join(matches[:20])
        )


def dogma_review_path_reason(path: str) -> str | None:
    for pattern, reason in DOGMA_CLEANUP_REVIEW_PATHS:
        if pattern.search(path):
            return reason
    return None


def dogma_campaign_surface_path_reason(repo: Path, path: str, base: str | None = None) -> str | None:
    reason = production_workspace_source_path_reason(repo, path, base)
    if reason is not None:
        return reason
    if not is_reference_scan_path(path) or is_generated_or_schema(path):
        return None
    for pattern, reason in NON_CARGO_DOGMA_CAMPAIGN_SURFACE_PATHS:
        if pattern.search(path):
            return reason
    return None


def dogma_gate_owning_path_reason(path: str) -> str | None:
    for pattern, reason in DOGMA_GATE_OWNING_PATHS:
        if pattern.search(path):
            return reason
    return None


def stable_diff_path_reason(path: str) -> str | None:
    reason = dogma_gate_owning_path_reason(path)
    if reason is not None:
        return reason
    reason = dogma_review_path_reason(path)
    if reason is not None:
        return reason
    if is_reference_scan_path(path):
        return "ordinary source path"
    return None


def is_structural_source_deletion(line: str) -> bool:
    return STRUCTURAL_SOURCE_DELETION.search(line) is not None


def stable_dogma_cleanup_signals(repo: Path, base: str) -> list[str]:
    signals: list[str] = []
    files = changed_files(repo, base)
    for path in files:
        reason = dogma_gate_owning_path_reason(path)
        if reason is not None:
            signals.append(f"{path}: {reason}")
            continue
        if DOGMA_CLEANUP_DIRECT_PATH.search(path):
            signals.append(f"{path}: dogma path")

    for changed in changed_diff_lines(repo, base):
        reason = stable_diff_path_reason(changed.path)
        if reason is not None and DOGMA_CLEANUP_DIFF_SIGNAL.search(changed.line):
            signals.append(f"{changed.path}: {reason}: {changed.line[1:].strip()}")
            continue
        campaign_reason = dogma_campaign_surface_path_reason(repo, changed.path, base)
        if campaign_reason is not None and is_structural_source_deletion(changed.line):
            signals.append(f"{changed.path}: {campaign_reason}: {changed.line[1:].strip()}")

    seen: set[str] = set()
    deduped: list[str] = []
    for signal in signals:
        if signal in seen:
            continue
        seen.add(signal)
        deduped.append(signal)
        if len(deduped) >= 5:
            break
    return deduped


def read_github_pull_request(event_path: Path) -> GitHubPullRequest | None:
    try:
        event = json.loads(event_path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"GitHub event payload not found: {event_path}")
    except json.JSONDecodeError as exc:
        fail(f"GitHub event payload is not valid JSON: {exc}")
    pull_request = event.get("pull_request")
    if not isinstance(pull_request, dict):
        return None
    labels = []
    for label in pull_request.get("labels") or []:
        if isinstance(label, dict) and isinstance(label.get("name"), str):
            labels.append(label["name"])
    head = pull_request.get("head")
    head_sha = head.get("sha", "") if isinstance(head, dict) else ""
    return GitHubPullRequest(
        title=str(pull_request.get("title") or ""),
        body=str(pull_request.get("body") or ""),
        labels=tuple(labels),
        head_sha=head_sha,
    )


def dogma_cleanup_required(pr: GitHubPullRequest, repo: Path, base: str) -> tuple[bool, str]:
    title_and_labels = "\n".join([pr.title, *pr.labels])
    if DOGMA_CLEANUP_SIGNAL.search(title_and_labels) is not None:
        return True, "title or labels"
    if DOGMA_CLEANUP_BODY_MARKER.search(pr.body) is not None:
        return True, "PR body marker"
    signals = stable_dogma_cleanup_signals(repo, base)
    if signals:
        return True, "stable diff signal: " + "; ".join(signals)
    return False, "no dogma cleanup PR signal or stable diff signal"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate a dogma cleanup Review Readiness Packet.")
    parser.add_argument("--packet", help="Markdown file containing the Review Readiness Packet.")
    parser.add_argument("--repo", default=".", help="Repository root. Defaults to current directory.")
    parser.add_argument("--base", default=os.environ.get("DOGMA_CLEANUP_BASE", "origin/main"))
    parser.add_argument("--head-sha", help="Override the current head SHA. Useful for fixtures.")
    parser.add_argument(
        "--github-event",
        help=(
            "GitHub event JSON. For pull_request events with dogma cleanup signals, "
            "or stable dogma-shaped diff signals, the PR body is treated as the packet "
            "when --packet is omitted."
        ),
    )
    parser.add_argument(
        "--packet-only",
        action="store_true",
        help="Validate packet structure/proof language only; skip git diff and reference checks.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).resolve()
    base = resolve_base_ref(repo, args.base)
    packet_label = "GitHub PR body"
    github_pr = None
    if args.github_event:
        github_pr = read_github_pull_request(Path(args.github_event))
        if github_pr is None:
            print("Dogma cleanup gate skipped: not a pull_request event.")
            return 0
        required, reason = dogma_cleanup_required(github_pr, repo, base)
        if not required:
            print(f"Dogma cleanup gate skipped: {reason}.", flush=True)
            return 0
        print(f"Dogma cleanup gate required by {reason}.", flush=True)

    if args.packet:
        packet_path = Path(args.packet)
        if not packet_path.is_absolute():
            packet_path = repo / packet_path
        markdown = packet_path.read_text(encoding="utf-8")
        packet_label = str(packet_path)
    elif github_pr is not None:
        markdown = github_pr.body
    else:
        fail("--packet is required unless --github-event supplies a dogma cleanup pull_request body")

    head_sha = args.head_sha or (
        github_pr.head_sha if github_pr is not None and github_pr.head_sha else ""
    )
    if not head_sha:
        head_sha = git_output(["rev-parse", "HEAD"], repo)

    entries = validate_packet(markdown, head_sha)
    if not args.packet_only:
        files = changed_files(repo, base)
        validate_complexity_delta(markdown, files)
        validate_diff_hygiene(added_lines(repo, base))
        validate_old_path_references(repo, entries)

    print(f"Dogma cleanup gate passed for {packet_label}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
