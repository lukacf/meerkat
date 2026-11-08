"""Source regression for the native service-call boundary, not E2E authority proof."""
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[3]
COLLECTOR_SOURCES = (
    ROOT / "crates/toolkit-mobkit/src/private_originals/collector_setup.rs",
    ROOT / "crates/toolkit-mobkit/src/installed_host/operator_setup/collector.rs",
)


def test_collector_does_not_construct_or_bypass_native_call_authority():
    prohibited = (
        r"\bwith_work_authorization\s*\(",
        r"\bWorkAuthorizationContext\s*::\s*new\s*\(",
        r"\bToolDispatchContext\s*::",
        r"\badmit_call_credential\s*\(",
        r"\bMcpRouter\s*::",
        r"\.\s*call_tool\s*\(",
    )
    for path in COLLECTOR_SOURCES:
        source = path.read_text()
        for pattern in prohibited:
            assert not re.search(pattern, source), (path, pattern)


def test_collector_enrollment_cannot_delegate_an_agent_grant():
    for path in COLLECTOR_SOURCES:
        source = path.read_text()
        assert not re.search(r"\bissue_child\s*\(", source), path
        assert not re.search(r"\benroll_new_authenticated(?:_async)?\s*\(", source), path
