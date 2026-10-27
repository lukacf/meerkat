import asyncio
import base64
from uuid import uuid4

import pytest
from meerkat_mobkit.models import SessionBuildOptions

from launchkit.acceptance import SemanticAcceptance
from launchkit.acceptance_reviewer import AcceptanceTools
from launchkit.blueprint import starter_blueprint
from launchkit.guide_attachments import GuideAttachments
from launchkit.guide_store import GuideStore
from launchkit.implementation_worker import BuildTools
from launchkit.templates.storage_setup import write_private


@pytest.fixture
def evidence(tmp_path):
    store = GuideStore(tmp_path)
    guide = store.create({"messages": [], "brief": {}})
    uploads = GuideAttachments(tmp_path)
    text = "target,priority,exact_rule\n" + "M31,7,Keep MiXeD spelling and a separate context\n" * 300
    ref, = uploads.save(guide["id"], [("observing-rules.csv", "text/csv", text.encode())])
    directory = tmp_path / "implementations" / guide["id"] / str(uuid4())
    directory.mkdir(parents=True, mode=0o700)
    frozen = {"guide_id": guide["id"], "blueprint": starter_blueprint().model_dump(mode="json"),
              "brief": {"summary": "Use the uploaded observing rules, summarized here."},
              "messages": [{"role": "user", "content": "Implement these exact rules.", "attachments": [ref]}]}
    write_private(directory / "input.json", frozen)
    tools = BuildTools(directory)
    acceptance = SemanticAcceptance(directory / "acceptance", tools.workspace, frozen)
    reviewer = AcceptanceTools(directory, acceptance, tools.reference)
    return {"root": tmp_path, "guide": guide, "uploads": uploads, "ref": ref,
            "text": text, "directory": directory, "frozen": frozen, "builder": tools, "reviewer": reviewer}


def handler(agent):
    draft = SessionBuildOptions()
    agent.register(draft)
    assert "read_build_attachment" in draft.tool_handlers, "Original uploaded evidence must be readable beyond the guide's summary"
    return draft.tool_handlers["read_build_attachment"]


@pytest.mark.parametrize("role", ["builder", "reviewer"])
def test_registered_builder_and_reviewer_read_exact_original_evidence_in_pages(evidence, role):
    read = handler(evidence[role])
    offset, pages = 0, []
    while True:
        result = asyncio.run(read({"attachment_id": evidence["ref"]["id"], "offset": offset, "max_chars": 12000}))
        assert "error" not in result, result
        assert result["attachment"] == evidence["ref"]
        assert result["offset"] == offset
        assert "not" in result["authority"] and "authorization" in result["authority"]
        pages.append(result["text"])
        if result["complete"]:
            assert result["next_offset"] is None
            break
        offset = result["next_offset"]
    assert len(pages) > 1 and "".join(pages) == evidence["text"]
    assert not list(evidence["builder"].workspace.root.rglob("observing-rules.csv")), "Reading does not copy private uploads into application source"


@pytest.mark.parametrize("role", ["builder", "reviewer"])
def test_evidence_reader_refuses_later_uploads_other_setups_and_tool_selected_roots(evidence, role):
    read = handler(evidence[role])
    later, = evidence["uploads"].save(evidence["guide"]["id"], [("later.md", "text/markdown", b"Not in this frozen build")])
    other = GuideStore(evidence["root"]).create({"messages": []})
    foreign, = evidence["uploads"].save(other["id"], [("foreign.md", "text/markdown", b"Other setup's original context")])
    for ref in (later, foreign):
        result = asyncio.run(read({"attachment_id": ref["id"]}))
        assert result["ok"] is False and "not submitted" in result["error"]
        assert "text" not in result
    for extra in ({"guide_id": other["id"]}, {"path": "/private/another-file"}, {"url": foreign["url"]}):
        result = asyncio.run(read({"attachment_id": evidence["ref"]["id"], **extra}))
        assert result["ok"] is False and "text" not in result


@pytest.mark.parametrize("change", ["filename", "guide", "bytes", "assistant_only"])
def test_evidence_reader_refuses_changed_or_non_user_context(evidence, change):
    tools = evidence["builder"]
    if change == "filename":
        tools.input["messages"][0]["attachments"][0]["filename"] = "invented.csv"
    elif change == "guide":
        tools.input["guide_id"] = str(uuid4())
    elif change == "assistant_only":
        tools.input["messages"][0]["role"] = "assistant"
    else:
        path = evidence["root"] / "guide-attachments" / evidence["guide"]["id"] / (evidence["ref"]["sha256"] + ".bin")
        path.write_bytes(b"Replaced source must not be used")
    result = asyncio.run(handler(tools)({"attachment_id": evidence["ref"]["id"]}))
    assert result["ok"] is False and "text" not in result


@pytest.mark.parametrize("paging", [{"offset": -1}, {"offset": True}, {"offset": 1_000_001},
                                    {"max_chars": 0}, {"max_chars": True}, {"max_chars": 12001}])
def test_evidence_reader_retains_existing_strict_paging_bounds(evidence, paging):
    result = asyncio.run(handler(evidence["builder"])({"attachment_id": evidence["ref"]["id"], **paging}))
    assert result["ok"] is False and "text" not in result


def test_restarted_tools_keep_frozen_evidence_scope(evidence):
    changed = {"role": "user", "content": "A later user message is not this build's original scope."}
    store = GuideStore(evidence["root"])
    current = store.get(evidence["guide"]["id"])
    store.save({**current, "messages": [changed]}, current["version"])
    fresh = BuildTools(evidence["directory"])
    result = asyncio.run(handler(fresh)({"attachment_id": evidence["ref"]["id"]}))
    assert result["attachment"] == evidence["ref"] and result["text"] == evidence["text"][:12000]


def test_reader_does_not_claim_extracted_image_text(evidence):
    png = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jGd0AAAAASUVORK5CYII=")
    ref, = evidence["uploads"].save(evidence["guide"]["id"], [("diagram.png", "image/png", png)])
    evidence["builder"].input["messages"][0]["attachments"].append(ref)
    result = asyncio.run(handler(evidence["builder"])({"attachment_id": ref["id"]}))
    assert result["ok"] is False and "image" in result["error"] and "text" not in result


@pytest.mark.parametrize("role", ["builder", "reviewer"])
def test_registered_evidence_reader_returns_original_pdf_extraction(evidence, role):
    from test_guide_attachments import pdf_bytes
    text = "Keep this exact source rule: require independent weather and target expertise."
    ref, = evidence["uploads"].save(evidence["guide"]["id"], [
        ("operating-rules.pdf", "application/pdf", pdf_bytes(text=text))])
    frozen = evidence["builder"].input if role == "builder" else evidence["reviewer"].acceptance.frozen
    frozen["messages"][0]["attachments"].append(ref)
    result = asyncio.run(handler(evidence[role])({"attachment_id": ref["id"]}))
    assert result["attachment"] == ref and result["text"].strip() == text
    assert result["complete"] and result["next_offset"] is None
    assert result["extraction"] == "PDF text only; no OCR or image analysis."
