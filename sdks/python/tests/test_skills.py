import warnings

import pytest

from meerkat.skills import SkillHelper


class _MockClient:
    def has_capability(self, capability: str) -> bool:
        return capability == "skills"

    async def start_turn(self, session_id: str, prompt: str, skill_refs=None):
        return {"session_id": session_id, "prompt": prompt, "skill_refs": skill_refs}


@pytest.mark.asyncio
async def test_legacy_string_skill_ref_emits_deprecation_warning():
    helper = SkillHelper(_MockClient())
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        await helper.invoke(
            "session-1",
            "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor",
            "run",
        )

    assert len(caught) >= 1
    assert any(item.category is DeprecationWarning for item in caught)
