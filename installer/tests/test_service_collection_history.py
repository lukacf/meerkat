"""Synthetic historical-data matching controls, not native or root acceptance."""
from dataclasses import replace
import hashlib
import importlib
import json
import os
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace

import pytest

from launchkit.service_collection_history import HistoryMismatch, match_history


@pytest.fixture
def sdk(monkeypatch):
    source = os.environ.get("FORGE_COLLECTION_SDK_SOURCE")
    if not source:
        pytest.skip("Explicit Connect SDK source required.")
    package = ModuleType("meerkat_toolkit")
    package.__path__ = [str(Path(source) / "meerkat_toolkit")]
    monkeypatch.setitem(sys.modules, "meerkat_toolkit", package)
    names = ("service_setup", "service_probe", "service_collection_transport", "service_collection")
    for name in names:
        monkeypatch.delitem(sys.modules, "meerkat_toolkit." + name, raising=False)
    result = SimpleNamespace(**{name: importlib.import_module("meerkat_toolkit." + name) for name in names})
    yield result
    for name in names:
        sys.modules.pop("meerkat_toolkit." + name, None)


@pytest.fixture
def sample(sdk):
    collection, probe, setup = sdk.service_collection, sdk.service_probe, sdk.service_setup
    # Expired observations must remain historical; no new native request occurs.
    binding = setup.ServiceSetupBinding("meerkat-forge", "original-instance", "https://forge.invalid",
        "viewer", "login", 1, "realm", "/context", "/state", "/config", "a" * 64,
        {"kind": "file", "root": "/original-credentials"})
    resource = {"program": "ghe.pr_statuses.v1", "selector": {"number": 17}}
    arguments = {"owner": "team", "repo": "project", "number": 17}
    encoded = json.dumps(arguments, sort_keys=True, separators=(",", ":")).encode()
    native = probe.ServiceProbeReport("requirement", binding.application_id, binding.instance_id,
        binding.login_scope_id, binding.principal_id, binding.origin, binding.realm_id,
        "session", "job", "server", "forge-ingestion", "read", resource,
        probe.ProbeStatus.TOOL_COMPLETED, "input", "run", "call", "b" * 64, "c" * 64,
        probe.ProbeDispatchOutcome.COMMITTED, True, True)
    call = collection.CollectionCallObservation("source", "requirement", resource, 1, "read",
        hashlib.sha256(encoded).hexdigest(), native)
    requirement = collection.RequiredOperationObservation("requirement", resource, ("read",), "verified", (call,))
    report = collection.ServiceCollectionReport("source", "attempt", binding.application_id,
        binding.instance_id, binding.login_scope_id, binding.principal_id, binding.origin,
        binding.realm_id, "session", "job", "official-github", "enterprise-selected-reads",
        "forge-ingestion", collection.CollectionStatus.HISTORICAL_CHECKS_COMPLETED, (requirement,),
        "rechecked_after_last_close", 1, "unqualified_credential_continuity",
        collection.CollectionLimits(4, 1024, 1024), 1, len(encoded), 64)
    native_json = {name: getattr(native, name) for name in native.__dataclass_fields__}
    row = {"source_id": "source", "requirement_id": "requirement", "resource": resource,
        "context": {"application": binding.application_id, "instance": binding.instance_id,
            "login_scope": "login", "viewer": "viewer", "origin": binding.origin,
            "realm": "realm", "session": "session", "job": "job", "server": "server",
            "agent": "forge-ingestion"},
        "collection": {"kind": "statuses", "data": {"state": "failure", "statuses": []}},
        "observations": [{"operation": "read", "arguments": arguments, "native": native_json,
            "payload": "exact\nquote\"nul\0", "structured_companion": None}]}
    archive = {"schema": "forge.ghe.historical-data.v1", "evidence_kind": "historical_data_only",
        "native_instance_id": binding.instance_id, "rows": [row]}
    return SimpleNamespace(binding=binding, report=report, archive=archive,
        close=setup.ServiceSetupClose(True, 0, True), sdk=sdk)


def encoded(sample):
    return json.dumps(sample.archive, ensure_ascii=False).encode()


def match(sample, archive=None):
    return match_history(encoded(sample) if archive is None else archive,
        binding=sample.binding, reports=(sample.report,), close=sample.close)


def test_original_failed_provider_data_and_original_report_are_retained(sample):
    result = match(sample)
    assert result.reports[0] is sample.report and result.close is sample.close
    assert result.rows[0]["collection"]["data"]["state"] == "failure"
    assert result.rows[0]["observations"][0]["payload"] == "exact\nquote\"nul\0"
    assert result.archive_sha256 == hashlib.sha256(encoded(sample)).hexdigest()
    assert repr(result) == "HistoricalDataMatch(<host-private>)"
    with pytest.raises(TypeError):
        result.rows[0]["collection"]["data"]["state"] = "success"


def test_failed_final_recheck_and_nonzero_exit_keep_completed_parser_data(sample):
    sample.report = replace(sample.report,
        status=sample.sdk.service_collection.CollectionStatus.SELECTION_CHANGED,
        observed_selection="changed_or_unconfirmed",
        requirements=(replace(sample.report.requirements[0], assessment=None),))
    sample.close = replace(sample.close, exit_code=1, native_resources_closed=False)
    result = match(sample)
    assert result.reports[0].status.value == "selection_changed"
    assert result.close.exit_code == 1 and len(result.rows) == 1
    assert not hasattr(result, "ready") and not hasattr(result, "ingestion_success")


@pytest.mark.parametrize("field", ["instance", "login_scope", "viewer", "origin", "realm", "session", "job", "server"])
def test_foreign_context_is_refused_without_disclosing_content(sample, field):
    sample.archive["rows"][0]["context"][field] = "PRIVATE-FOREIGN"
    with pytest.raises(HistoryMismatch) as error:
        match(sample)
    assert "PRIVATE" not in str(error.value)


@pytest.mark.parametrize("change", ["argument", "resource", "native", "missing_call", "duplicate_row", "kind"])
def test_exact_requirement_and_call_correlation(sample, change):
    row = sample.archive["rows"][0]
    if change == "argument":
        row["observations"][0]["arguments"] = {"number": 18}
    elif change == "resource":
        row["resource"] = {"program": "ghe.pr_statuses.v1", "selector": {"number": 18}}
    elif change == "native":
        row["observations"][0]["native"] = dict(row["observations"][0]["native"], run_id="foreign")
    elif change == "missing_call":
        row["observations"] = []
    elif change == "duplicate_row":
        sample.archive["rows"].append(row)
    else:
        row["collection"]["kind"] = "issues"
    with pytest.raises(HistoryMismatch):
        match(sample)


def test_missing_archive_is_only_history_without_verified_rows(sample):
    with pytest.raises(HistoryMismatch):
        match_history(None, binding=sample.binding, reports=(sample.report,), close=sample.close)
    requirement = replace(sample.report.requirements[0], assessment=None, calls=())
    report = replace(sample.report, status=sample.sdk.service_collection.CollectionStatus.NATIVE_REFUSED,
        requirements=(requirement,), observed_selection="not_observed", total_calls=0)
    result = match_history(None, binding=sample.binding, reports=(report,), close=sample.close)
    assert result.rows == () and result.reports == (report,) and result.archive_sha256 is None


def test_distinct_native_final_reads_match_but_reused_native_identity_does_not(sample):
    requirement = sample.report.requirements[0]
    second_native = replace(requirement.calls[0].native, input_id="input2", run_id="run2", tool_call_id="call2")
    second_call = replace(requirement.calls[0], ordinal=2, native=second_native)
    sample.report = replace(sample.report, requirements=(replace(requirement, calls=(*requirement.calls, second_call)),), total_calls=2)
    second = dict(sample.archive["rows"][0]["observations"][0],
        native={name: getattr(second_native, name) for name in second_native.__dataclass_fields__})
    sample.archive["rows"][0]["observations"].append(second)
    assert len(match(sample).rows[0]["observations"]) == 2
    # This deliberately malformed synthetic SDK report is not native evidence.
    sample.report = replace(sample.report, requirements=(replace(requirement, calls=(requirement.calls[0], requirement.calls[0])),))
    second["native"] = sample.archive["rows"][0]["observations"][0]["native"]
    with pytest.raises(HistoryMismatch):
        match(sample)


def test_unjoined_child_foreign_binding_and_duplicate_json_are_refused(sample):
    sample.close = replace(sample.close, process_joined=False)
    with pytest.raises(HistoryMismatch):
        match(sample)
    sample.close = replace(sample.close, process_joined=True)
    sample.binding = replace(sample.binding, principal_id="foreign")
    with pytest.raises(HistoryMismatch):
        match(sample)
    sample.binding = replace(sample.binding, principal_id="viewer")
    with pytest.raises(HistoryMismatch):
        match(sample, b'{"schema":"forge.ghe.historical-data.v1","schema":"other"}')


def test_nonfinite_json_unsupported_argument_numbers_and_oversize_are_refused(sample, monkeypatch):
    for raw in (b'{"value":NaN}', b'{"value":1e9999}'):
        with pytest.raises(HistoryMismatch):
            match(sample, raw)
    for number in (1.5, 1 << 64):
        sample.archive["rows"][0]["observations"][0]["arguments"] = {"number": number}
        with pytest.raises(HistoryMismatch):
            match(sample)
    monkeypatch.setattr("launchkit.service_collection_history.MAX_ARCHIVE_BYTES", 16)
    with pytest.raises(HistoryMismatch):
        match(sample)
