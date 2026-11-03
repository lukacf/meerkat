"""Match private Forge data to original SDK observations, never admit ingestion.

The root owner reads the original archive after actual child exit, preserving
its filesystem isolation. It supplies the original binding, SDK reports and
SDK close observation here. This module does not open files, reconstruct native
objects, validate root custody, parse provider meaning or grant future access.
"""
from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, fields, is_dataclass
from enum import Enum
import hashlib
import json
import math
from types import MappingProxyType


MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
_KINDS = {
    "ghe.pr_statuses.v1": "statuses", "ghe.pr_threads.v1": "threads",
    "ghe.pr_changes.v1": "changes", "ghe.pr_comments.v1": "discussion",
    "ghe.pr_reviews.v1": "discussion", "ghe.history.v1": "commits",
    "ghe.workflow_runs.v1": "runs", "ghe.workflow_jobs.v1": "jobs",
    "ghe.issues.v1": "issues", "ghe.issue_comments.v1": "issue_comments",
    "ghe.issue_hierarchy.v1": "issue_hierarchy", "ghe.pull_requests.v1": "pull_requests",
}
_CONTEXT = {
    "application": "application_id", "instance": "instance_id",
    "login_scope": "login_scope_id", "viewer": "viewer_id", "origin": "origin",
    "realm": "realm", "session": "session_id", "job": "job_id",
    "server": "server", "agent": "agent",
}


class HistoryMismatch(ValueError):
    def __init__(self):
        super().__init__("Historical data does not match the original collection observations.")


@dataclass(frozen=True, repr=False)
class HistoricalDataMatch:
    """Historical data and original SDK facts, with no readiness/success flag.

    Matching does not independently verify parsed payload contents against the
    native result digest. Those bytes come from the original protected archive;
    the full native result's digest covers more than its extracted text payload.
    Keep the raw archive and reports in root custody for governed admission.
    """
    rows: tuple
    reports: tuple
    close: object
    archive_sha256: str | None

    def __repr__(self):
        return "HistoricalDataMatch(<host-private>)"


def _plain(value):
    if isinstance(value, Enum):
        return value.value
    if is_dataclass(value):
        return {field.name: _plain(getattr(value, field.name)) for field in fields(value)}
    if isinstance(value, Mapping):
        return {key: _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    return value


def _encoded(value):
    return json.dumps(_plain(value), sort_keys=True, ensure_ascii=False,
                      separators=(",", ":"), allow_nan=False).encode("utf-8")


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise HistoryMismatch()
        result[key] = value
    return result


def _reject_constant(_):
    raise HistoryMismatch()


def _finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise HistoryMismatch()
    return number


def _freeze(value):
    if type(value) is dict:
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if type(value) is list:
        return tuple(_freeze(item) for item in value)
    return value


def _argument_numbers(value):
    # Compiled Forge selectors use integer/string/bool/null arguments. Do not
    # guess Rust floating-point formatting for an unsupported argument shape.
    if type(value) is float or type(value) is int and not -(1 << 63) <= value < (1 << 64):
        raise HistoryMismatch()
    if type(value) is dict:
        for item in value.values():
            _argument_numbers(item)
    elif type(value) is list:
        for item in value:
            _argument_numbers(item)


def match_history(archive, *, binding, reports, close):
    """Consume original public SDK facts after process join, including failures.

    `archive` is bounded original file bytes, or None when no file was created.
    `reports` must be the retained event.report objects from this client's SDK
    event stream, including partial/failure reports. The SDK and root owner
    retain authority; types and equality checks here only prevent mismatches.
    Expired login observations remain history and never become current access.
    """
    from meerkat_toolkit.service_collection import ServiceCollectionReport
    from meerkat_toolkit.service_setup import ServiceSetupBinding, ServiceSetupClose

    try:
        if (type(binding) is not ServiceSetupBinding or type(close) is not ServiceSetupClose
                or close.process_joined is not True or type(close.exit_code) is not int
                or type(reports) is not tuple or len(reports) > 256
                or any(type(report) is not ServiceCollectionReport for report in reports)):
            raise HistoryMismatch()
        expected = {
            "application_id": binding.application_id, "instance_id": binding.instance_id,
            "login_scope_id": binding.login_scope_id, "viewer_id": binding.principal_id,
            "origin": binding.origin, "realm": binding.realm_id,
        }
        selected, attempts, sources, required, native_ids = {}, set(), set(), set(), set()
        for report in reports:
            if (any(getattr(report, key) != value for key, value in expected.items())
                    or report.family != "official-github" or report.profile != "enterprise-selected-reads"
                    or report.agent != "forge-ingestion" or report.source_id in sources
                    or report.attempt_id in attempts):
                raise HistoryMismatch()
            sources.add(report.source_id)
            attempts.add(report.attempt_id)
            for requirement in report.requirements:
                key = (report.source_id, requirement.required_operation_id)
                if key in selected:
                    raise HistoryMismatch()
                selected[key] = (report, requirement)
                if requirement.assessment == "verified":
                    required.add(key)
        if archive is None:
            rows, digest = [], None
        else:
            if type(archive) is not bytes or not 0 < len(archive) <= MAX_ARCHIVE_BYTES:
                raise HistoryMismatch()
            data = json.loads(archive.decode("utf-8"), object_pairs_hook=_unique,
                              parse_constant=_reject_constant, parse_float=_finite_float)
            if (type(data) is not dict
                    or set(data) != {"schema", "evidence_kind", "native_instance_id", "rows"}
                    or data["schema"] != "forge.ghe.historical-data.v1"
                    or data["evidence_kind"] != "historical_data_only"
                    or data["native_instance_id"] != binding.instance_id
                    or type(data["rows"]) is not list or not 1 <= len(data["rows"]) <= 256):
                raise HistoryMismatch()
            rows, digest = data["rows"], hashlib.sha256(archive).hexdigest()
        seen = set()
        for row in rows:
            if (type(row) is not dict or set(row) != {
                    "source_id", "requirement_id", "resource", "context", "collection", "observations"}
                    or type(row["source_id"]) is not str or type(row["requirement_id"]) is not str):
                raise HistoryMismatch()
            key = (row["source_id"], row["requirement_id"])
            if key in seen or key not in selected:
                raise HistoryMismatch()
            seen.add(key)
            _, requirement = selected[key]
            if (_encoded(row["resource"]) != _encoded(requirement.resource)
                    or type(row["resource"]) is not dict or type(row["collection"]) is not dict
                    or set(row["collection"]) != {"kind", "data"}
                    or row["collection"]["kind"] != _KINDS.get(row["resource"].get("program"))
                    or row["collection"]["kind"] is None
                    or type(row["collection"]["data"]) is not dict
                    or type(row["observations"]) is not list or not requirement.calls
                    or len(row["observations"]) != len(requirement.calls)):
                raise HistoryMismatch()
            for observation, call in zip(row["observations"], requirement.calls):
                if (type(observation) is not dict or set(observation) != {
                        "operation", "arguments", "native", "payload", "structured_companion"}
                        or observation["operation"] != call.raw_operation
                        or type(observation["payload"]) is not str
                        or observation["structured_companion"] is not None
                        and type(observation["structured_companion"]) is not str
                        or _encoded(observation["native"]) != _encoded(call.native)
                        or _encoded(row["context"]) != _encoded({
                            key: getattr(call.native, attr) for key, attr in _CONTEXT.items()})):
                    raise HistoryMismatch()
                # Preserve legitimate repeated read operations with distinct
                # native identities. Never treat equal arguments as a replay.
                identity = (call.native.input_id, call.native.run_id, call.native.tool_call_id)
                if None in identity or identity in native_ids:
                    raise HistoryMismatch()
                native_ids.add(identity)
                _argument_numbers(observation["arguments"])
                if hashlib.sha256(_encoded(observation["arguments"])).hexdigest() != call.arguments_sha256:
                    raise HistoryMismatch()
                observation["payload"].encode("utf-8")
                if observation["structured_companion"] is not None:
                    observation["structured_companion"].encode("utf-8")
        if not required <= seen:
            raise HistoryMismatch()
        return HistoricalDataMatch(tuple(_freeze(row) for row in rows), reports, close, digest)
    except (ValueError, TypeError, KeyError, AttributeError, UnicodeError, RecursionError):
        raise HistoryMismatch() from None
