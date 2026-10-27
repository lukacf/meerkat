"""Declared first jobs are bounded source data, never offline fixtures or proof."""
import hashlib
import json
import importlib.util
import io
from pathlib import Path
import sys
import zipfile

import pytest

def load_candidate(name):
    return __import__("launchkit." + name, fromlist=[name])
candidate = load_candidate("application_first_use")
FIRST_USE_PATH, MAX_METADATA_BYTES = candidate.FIRST_USE_PATH, candidate.MAX_METADATA_BYTES
first_use_suggestions = candidate.first_use_suggestions
from launchkit.generator import PackageFile


TASK = {"label": "Find my saved wishlist", "request": "Show the books I saved for later.",
        "description": "Read the collection without changing it."}


def suggestions(value):
    return first_use_suggestions([PackageFile(FIRST_USE_PATH, json.dumps(value))])


def test_declared_jobs_preserve_source_text_and_do_not_import_application_or_use_trial_fixtures():
    content = json.dumps({"tasks": [TASK]}, ensure_ascii=False)
    files = [PackageFile("application/__init__.py", "raise AssertionError('never import application')\n"),
             PackageFile("application/trial.json", json.dumps({"cases": [{
                 "label": "Fake approval", "request": "Commit through synthetic confirmation.",
                 "fixtures": {"private-fixture": "not-installed-input"}}]})),
             PackageFile(FIRST_USE_PATH, content)]
    result = first_use_suggestions(files)
    assert result["tasks"] == [TASK]
    assert result["metadata_sha256"] == hashlib.sha256(content.encode("utf-8")).hexdigest()
    assert result["scope"] == "authored-first-use-suggestions"
    assert "do not prove installed execution" in result["detail"]
    assert not {"fixtures", "verified", "available", "accepted", "auto_send"} & result.keys()
    assert "not-installed-input" not in json.dumps(result)
    assert first_use_suggestions({FIRST_USE_PATH: (content.encode("utf-8"), 0o644)}) == result


def test_missing_catalog_never_converts_an_offline_case_into_an_installed_job():
    result = first_use_suggestions([PackageFile("application/trial.json", json.dumps({"cases": [TASK]}))])
    assert result["tasks"] == [] and result["metadata_sha256"] is None
    assert "describe your own job" in result["detail"]


def test_full_unicode_limits_fit_within_source_byte_bound_without_truncation():
    task = {"label": "\U0001f31f" * 120, "request": "\U0001f31f" * 3000,
            "description": "\U0001f31f" * 400}
    content = json.dumps({"tasks": [task]}, ensure_ascii=False).encode("utf-8")
    assert len(content) < MAX_METADATA_BYTES
    assert first_use_suggestions({FIRST_USE_PATH: (content, 0o644)})["tasks"] == [task]
    multiline = {"label": "Read the collection", "request": " \tShow my wishlist.\nExplain what finished means.\n "}
    assert suggestions({"tasks": [multiline]})["tasks"] == [multiline]


def test_whole_file_byte_limit_is_exact_and_distinct_from_per_job_character_limits():
    base = json.dumps({"tasks": [TASK]}).encode("utf-8")
    content = base + b" " * (MAX_METADATA_BYTES - len(base))
    assert first_use_suggestions({FIRST_USE_PATH: (content, 0o644)})["tasks"] == [TASK]
    assert first_use_suggestions({FIRST_USE_PATH: (content + b" ", 0o644)})["tasks"] == []
    assert suggestions({"tasks": [{"label": "Job", "request": "Read the notes."}] * 8})["tasks"] == [
        {"label": "Job", "request": "Read the notes."}] * 8


@pytest.mark.parametrize("value", [
    {"tasks": []}, {"tasks": [TASK] * 9}, {"tasks": [None]}, {"tasks": [TASK], "fixtures": {}},
    {"tasks": [{**TASK, "auto_send": True}]}, {"tasks": [{**TASK, "fixtures": {}}]},
    {"tasks": [{**TASK, "verified": True}]}, {"tasks": [{**TASK, "identity": "lead:main"}]},
    {"tasks": [{**TASK, "request": None}]}, {"tasks": [{**TASK, "label": " "}]},
    {"tasks": [{**TASK, "label": "x" * 121}]}, {"tasks": [{**TASK, "request": "x" * 3001}]},
    {"tasks": [{**TASK, "description": "x" * 401}]}, {"tasks": [{**TASK, "description": []}]},
    {"tasks": [{**TASK, "label": "hidden\nline"}]}, {"tasks": [{**TASK, "request": "hidden\x00job"}]},
    {"tasks": [{**TASK, "request": "\ud800"}]},
    {"tasks": [TASK, {"label": "Partial", "request": "x" * 3001}]},
])
def test_invalid_or_authority_bearing_metadata_cannot_publish_partial_jobs_or_echo_values(value):
    result = suggestions(value)
    assert result["tasks"] == [] and result["detail"]
    assert "describe your own job" in result["detail"]
    assert not {"fixtures", "verified", "auto_send"} & result.keys()


@pytest.mark.parametrize("content", [b"not JSON", b"\xff", b'{"tasks":[],"tasks":[]}',
    b'{"tasks":[{"label":"A","label":"B","request":"C"}]}',
    b'{"tasks":[{"label":"A","request":NaN}]}',
    b'{"tasks":[{"label":"A","request":1e999}]}', b" " * (MAX_METADATA_BYTES + 1)])
def test_malformed_duplicate_nonfinite_and_oversized_metadata_has_no_partial_projection(content):
    result = first_use_suggestions({FIRST_USE_PATH: (content, 0o644)})
    assert result["tasks"] == [] and result["detail"]
    assert result["metadata_sha256"] == hashlib.sha256(content).hexdigest()
    assert "NaN" not in result["detail"] and "1e999" not in result["detail"]


def test_duplicate_source_path_is_rejected_instead_of_selecting_a_catalog():
    result = first_use_suggestions([PackageFile(FIRST_USE_PATH, json.dumps({"tasks": [TASK]}))] * 2)
    assert result["tasks"] == [] and result["metadata_sha256"] is None


EXPERIENCE_PATH = "application/experience.json"
ENTRY = {"id": "reading-room", "label": "Reading room", "path": "/reading", "port": 8800}
DEFINITION = {"name": "Installed reader", "description": "Read saved plans.", "target": "linux",
    "entrypoint": "reader:main", "agents": [{"identity": "reader:main", "name": "Reader"}],
    "environment": [{"key": "APP_UI_PORT", "secret": False}]}


def handoff_files(entry_id="reading-room", *, entries=None):
    return [PackageFile(FIRST_USE_PATH, json.dumps({"tasks": [TASK], "entry_id": entry_id})),
        PackageFile(EXPERIENCE_PATH, json.dumps({"entries": entries if entries is not None else [ENTRY]})),
        PackageFile("application/__init__.py", "raise AssertionError('must not import application')\n")]


def assert_manual(result):
    assert result["tasks"] == [TASK] and result["entry_id"] is None
    assert "manually" in result["detail"] and "could not be matched" in result["detail"]
    assert not {"url", "endpoint", "available", "verified", "auth", "accepted", "auto_send"} & result.keys()


def test_exact_same_package_entry_id_opts_into_declared_fragment_support_without_proof():
    files = handoff_files()
    result = first_use_suggestions(files, DEFINITION)
    assert result["entry_id"] == ENTRY["id"] and result["tasks"] == [TASK]
    assert "versioned local URL fragment" in result["detail"] and "does not prove availability" in result["detail"]
    assert not {"url", "endpoint", "available", "verified", "auth", "accepted", "auto_send"} & result.keys()
    mapping = {item.path: (item.content.encode(), 0o644) for item in files}
    assert first_use_suggestions(mapping, DEFINITION) == result


def test_tasks_only_stays_manual_even_if_interface_metadata_is_missing_or_invalid():
    files = [PackageFile(FIRST_USE_PATH, json.dumps({"tasks": [TASK]})),
        PackageFile(EXPERIENCE_PATH, "invalid optional interface metadata")]
    result = first_use_suggestions(files, DEFINITION)
    assert result["tasks"] == [TASK] and result["entry_id"] is None
    assert "handoff" not in result["detail"]


@pytest.mark.parametrize("entry_id", [None, True, 1, [], {}, "", " Reading-room", "Reading-room", "reading-room ",
    "reading.room", "reading/room", "reading-room#token", "https://outside.invalid/", "x" * 65, "\ud800"])
def test_invalid_entry_id_is_manual_and_never_discards_useful_task_text_or_echoes_ref(entry_id):
    result = first_use_suggestions(handoff_files(entry_id), DEFINITION)
    assert_manual(result)
    assert "outside.invalid" not in result["detail"] and "token" not in result["detail"]


def test_missing_wrong_and_duplicate_experience_references_preserve_tasks_with_manual_fallback():
    files = handoff_files("another-room")
    assert_manual(first_use_suggestions(files, DEFINITION))
    assert_manual(first_use_suggestions(handoff_files()[:1], DEFINITION))
    assert_manual(first_use_suggestions(handoff_files(entries=[ENTRY, ENTRY]), DEFINITION))
    duplicated_path = handoff_files() + [PackageFile(EXPERIENCE_PATH, json.dumps({"entries": [ENTRY]}))]
    assert_manual(first_use_suggestions(duplicated_path, DEFINITION))


@pytest.mark.parametrize("experience", ["not JSON", '{"entries":[],"entries":[]}', " " * 64001,
    json.dumps({"entries": [{**ENTRY, "url": "https://outside.invalid/"}]}),
    json.dumps({"entries": [ENTRY, {"id": "unsafe", "label": "Unsafe", "path": "/../admin"}]}),
    json.dumps({"entries": [{**ENTRY, "path": "/reading?token=private-canary"}]}),
    json.dumps({"entries": [{**ENTRY, "port_field": "PRIVATE_TOKEN"}]}),
    json.dumps({"entries": [{**ENTRY, "port": 80}]}), b"\xff"])
def test_invalid_experience_is_checked_atomically_and_safely_falls_back_to_existing_jobs(experience):
    files = {FIRST_USE_PATH: (json.dumps({"tasks": [TASK], "entry_id": ENTRY["id"]}).encode(), 0o644),
        EXPERIENCE_PATH: (experience.encode() if isinstance(experience, str) else experience, 0o644)}
    result = first_use_suggestions(files, DEFINITION)
    assert_manual(result)
    assert "private-canary" not in result["detail"] and "outside.invalid" not in result["detail"]


def test_existing_parser_checks_selected_entry_and_nonsecret_definition_fields_without_private_values():
    entry = {"id": "other-view", "label": "Other view", "path": "/other", "port_field": "APP_UI_PORT"}
    files = handoff_files("other-view", entries=[ENTRY, entry])
    assert first_use_suggestions(files, DEFINITION)["entry_id"] == "other-view"
    assert_manual(first_use_suggestions(files))
    assert first_use_suggestions(handoff_files())["entry_id"] == "reading-room"
    secret_definition = {**DEFINITION, "environment": [{"key": "APP_UI_PORT", "secret": True}]}
    assert_manual(first_use_suggestions(files, secret_definition))


def test_bounded_identifier_uses_existing_parser_normalized_ids_without_whitespace_normalization():
    longest = "r" + "a" * 63
    assert first_use_suggestions(handoff_files(longest, entries=[{**ENTRY, "id": longest}]))["entry_id"] == longest
    no_id_entry = {key: value for key, value in ENTRY.items() if key != "id"}
    assert first_use_suggestions(handoff_files("interface-1", entries=[no_id_entry]))["entry_id"] == "interface-1"


def test_invalid_task_catalog_never_becomes_a_handoff_even_with_a_valid_route():
    files = handoff_files()
    files[0] = PackageFile(FIRST_USE_PATH, json.dumps({"tasks": [{**TASK, "auto_send": True}], "entry_id": ENTRY["id"]}))
    result = first_use_suggestions(files, DEFINITION)
    assert result["tasks"] == [] and result["entry_id"] is None
    files[0] = PackageFile(FIRST_USE_PATH, '{"tasks":[],"entry_id":"reading-room","entry_id":"reading-room"}')
    assert first_use_suggestions(files, DEFINITION)["entry_id"] is None


def archive_for_projection(tmp_path):
    files = {item.path: item.content for item in handoff_files(entries=[{key: value for key, value in ENTRY.items() if key != "port"}])}
    files[EXPERIENCE_PATH] = json.dumps({"entries": [{"id": ENTRY["id"], "label": ENTRY["label"],
        "path": ENTRY["path"], "port_field": "APP_UI_PORT"}]})
    files.update({"deployment.json": json.dumps(DEFINITION), "app.py": "raise AssertionError('never execute app')\n",
        "install.sh": "exit 99\n", "implementation-report.json": json.dumps({"guide_version": 4,
            "build_id": "installed-build", "verification": {"digest": "a" * 64}})})
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for name, content in files.items():
            archive.writestr(name, content)
    payload = output.getvalue()
    (tmp_path / "package.zip").write_bytes(payload)
    receipt = {"source_verified": True, "package_sha256": hashlib.sha256(payload).hexdigest(),
        "guide_version": 4, "build_id": "installed-build", "accepted_source_digest": "a" * 64}
    return receipt


@pytest.mark.asyncio
async def test_local_checked_installed_snapshot_passes_exact_definition_and_projects_only_the_ref(tmp_path):
    local = load_candidate("installed_feedback")
    local.first_use_suggestions = first_use_suggestions
    receipt = archive_for_projection(tmp_path)
    class Service:
        async def guide_status(self, identity, *, state):
            return receipt
        def _directory(self, identity):
            return tmp_path
    returned, metadata = await local._snapshot(Service(), "guide", {})
    assert returned == receipt and metadata["first_use"]["entry_id"] == ENTRY["id"]
    assert metadata["first_use"]["tasks"] == [TASK] and "url" not in metadata["first_use"]
    assert metadata["project"]["name"] == DEFINITION["name"]


def test_remote_checked_package_metadata_uses_same_exact_entry_ref_without_remote_contact(tmp_path):
    remote = load_candidate("remote_installed_feedback")
    remote.first_use_suggestions = first_use_suggestions
    from launchkit.deployment_updates import checked_package
    receipt = archive_for_projection(tmp_path)
    _, files = checked_package(tmp_path, "package.zip", receipt["package_sha256"])
    result = remote._metadata(files, receipt)
    assert result["first_use"]["entry_id"] == ENTRY["id"] and result["first_use"]["tasks"] == [TASK]
    assert not {"available", "url", "endpoint", "auto_send", "verified"} & result["first_use"].keys()
    damaged = {**files, EXPERIENCE_PATH: (b'{"entries":[]}', 0o644)}
    assert_manual(remote._metadata(damaged, receipt)["first_use"])
