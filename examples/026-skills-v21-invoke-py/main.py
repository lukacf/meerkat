#!/usr/bin/env python3
"""026 — Skills V2.1 Invocation (Python SDK)

Invoke a specific skill using canonical refs (`SkillKey`) from the rebuilt
skill system.

Run:
    ANTHROPIC_API_KEY=sk-... \
    MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f \
    MEERKAT_SKILL_NAME=shell-patterns \
    python main.py
"""

import asyncio
import os
from meerkat import MeerkatClient, SkillHelper, SkillKey


async def main() -> None:
    source_uuid = os.environ.get("MEERKAT_SKILL_SOURCE_UUID")
    skill_name = os.environ.get("MEERKAT_SKILL_NAME")

    if not source_uuid or not skill_name:
        print("Missing required env vars:")
        print("  MEERKAT_SKILL_SOURCE_UUID=<source uuid>")
        print("  MEERKAT_SKILL_NAME=<skill name>")
        return

    client = MeerkatClient()
    await client.connect()

    try:
        skills = SkillHelper(client)
        skills.require_skills()

        skill = SkillKey(source_uuid=source_uuid, skill_name=skill_name)
        result = await skills.invoke_new_session(
            skill,
            "Use this skill to review this shell command for safety: rm -rf /tmp/build-cache",
            model="claude-sonnet-4-5",
        )

        print(f"Skill: {source_uuid}/{skill_name}")
        print(f"Session: {result.session_id}")
        print(result.text)
    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
