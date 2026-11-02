"""PDF paging reuses extraction without reusing admission or storage evidence."""
from unittest.mock import Mock

import pytest

from launchkit.guide_attachments import GuideAttachments, _pdf_text
from test_guide_attachments import attachments, pdf_bytes


@pytest.fixture
def pdf_reads(monkeypatch):
    import pypdf

    _pdf_text.cache_clear()
    reader = Mock(wraps=pypdf.PdfReader)
    monkeypatch.setattr(pypdf, "PdfReader", reader)
    try:
        yield reader
    finally:
        _pdf_text.cache_clear()


def test_sequential_pages_and_new_readers_parse_pdf_once(attachments, pdf_reads):
    store, guide_id, other_id = attachments
    text = "Keep this exact workplace rule. " * 500
    data = pdf_bytes(text=text)
    ref, = store.save(guide_id, [("rules.pdf", "application/pdf", data)])
    assert pdf_reads.call_count == 1
    stored_paths = set(store.root.rglob("*"))
    offset, pieces = 0, []
    while True:
        # Build attachment calls construct a new storage reader for each page.
        result = GuideAttachments(store.root).read_content(guide_id, ref["id"], offset=offset)
        assert result["attachment"] == ref and result["offset"] == offset
        pieces.append(result["text"])
        if result["complete"]:
            assert result["next_offset"] is None
            break
        offset = result["next_offset"]
    assert len(pieces) > 1 and "".join(pieces).strip() == text.strip()
    assert pdf_reads.call_count == 1
    assert set(store.root.rglob("*")) == stored_paths
    with pytest.raises(ValueError):
        store.read_content(other_id, ref["id"])
    assert pdf_reads.call_count == 1


def test_extraction_cache_keeps_only_two_recent_documents(attachments, pdf_reads):
    store, guide_id, _ = attachments
    refs = [store.save(guide_id, [(f"rules-{i}.pdf", "application/pdf",
                                pdf_bytes(text=f"Distinct rule {i}."))])[0] for i in range(3)]
    assert pdf_reads.call_count == 3
    assert "Distinct rule 1." in store.read_content(guide_id, refs[1]["id"])["text"]
    assert "Distinct rule 2." in store.read_content(guide_id, refs[2]["id"])["text"]
    assert pdf_reads.call_count == 3
    # Returning to the evicted PDF re-reads its bytes and extracts it again.
    assert "Distinct rule 0." in store.read_content(guide_id, refs[0]["id"])["text"]
    assert pdf_reads.call_count == 4


def test_failed_pdf_extraction_is_not_retained(attachments, pdf_reads):
    store, guide_id, _ = attachments
    data = pdf_bytes(encrypted=True)
    for _ in range(2):
        with pytest.raises(ValueError, match="Encrypted"):
            store.save(guide_id, [("locked.pdf", "application/pdf", data)])
    assert pdf_reads.call_count == 2
    assert not (store.root / "guide-attachments").exists()
