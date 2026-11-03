"""Declared first jobs are bounded source data, never offline fixtures or proof."""
import hashlib
import json

import pytest

from launchkit.application_first_use import FIRST_USE_PATH, MAX_METADATA_BYTES, first_use_suggestions
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


def test_authored_walkthrough_details_are_preserved_without_new_authority_or_execution():
    task = {**TASK, "inputs": ["The books you want to save"], "prerequisites": [],
        "expected_result": "Your requested books appear in your saved wishlist.",
        "result_location": "Open Saved books in the application."}
    assert first_use_suggestions({FIRST_USE_PATH: (json.dumps({"tasks": [task]},
        ensure_ascii=False).encode("utf-8"), 0o644)})["tasks"] == [task]
    task.update(inputs=["\U0001f31f" * 400] * 8, prerequisites=["Connected account"] * 8,
        expected_result="\U0001f31f" * 600, result_location="\U0001f31f" * 400)
    assert first_use_suggestions({FIRST_USE_PATH: (json.dumps({"tasks": [task]},
        ensure_ascii=False).encode("utf-8"), 0o644)})["tasks"] == [task]


@pytest.mark.parametrize("field,value", [
    ("inputs", "A file"), ("inputs", ["x"] * 9), ("inputs", [None]),
    ("prerequisites", [{"credential": "never project"}]), ("prerequisites", [" "]),
    ("inputs", ["x" * 401]), ("prerequisites", ["hidden\x00value"]),
    ("expected_result", "x" * 601), ("expected_result", True),
    ("result_location", "x" * 401), ("result_location", "\ud800"),
])
def test_invalid_walkthrough_is_a_safe_whole_catalog_fallback(field, value):
    result = suggestions({"tasks": [TASK, {**TASK, field: value}]})
    assert result["tasks"] == [] and result["entry_id"] is None
    assert "describe your own job" in result["detail"]
    assert "never project" not in result["detail"]
