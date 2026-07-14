#!/usr/bin/env python3
"""026 — Skills V2.1 Invocation (Python SDK)

Invoke a specific skill using canonical refs (`SkillKey`) from the rebuilt
skill system.

Run:
    ANTHROPIC_API_KEY=sk-... python3 main.py
"""

import asyncio
import os
from pathlib import Path

from meerkat import MeerkatClient, SkillKey

PROJECT_LOCAL_SOURCE_UUID = "00000000-0000-4b11-8111-000000000002"
DEFAULT_SKILL_NAME = "shell-patterns"


def skill_body(skill_name: str) -> str:
    return f"""---
name: {skill_name}
description: Review shell commands for safety and portability.
---

# {skill_name}

Review shell commands before execution. Flag destructive operations, missing
quoting, non-portable flags, and places where a safer dry-run or explicit path
would reduce risk.

Respond with:
- Risk level
- Specific concern
- Safer command or mitigation
"""


def prepare_local_skill_source(
    skill_name: str,
) -> tuple[Path, Path, Path]:
    root = Path(__file__).resolve().parent
    work = root / ".work"
    context_root = work / "project"
    state_root = work / "state"
    user_config_root = work / "user"
    skill_dir = context_root / ".rkat" / "skills" / skill_name
    skill_dir.mkdir(parents=True, exist_ok=True)
    state_root.mkdir(parents=True, exist_ok=True)
    user_config_root.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_text(skill_body(skill_name), encoding="utf-8")
    return context_root, state_root, user_config_root


async def main() -> None:
    source_uuid = PROJECT_LOCAL_SOURCE_UUID
    skill_name = os.environ.get("MEERKAT_SKILL_NAME", DEFAULT_SKILL_NAME)
    context_root, state_root, user_config_root = prepare_local_skill_source(skill_name)

    client = MeerkatClient()
    await client.connect(
        isolated=True,
        context_root=str(context_root),
        state_root=str(state_root),
        user_config_root=str(user_config_root),
    )

    try:
        client.require_capability("skills")

        # Create a session, then invoke the skill on it.
        skill = SkillKey(source_uuid=source_uuid, skill_name=skill_name)
        session = await client.create_session(
            prompt="Hello",
            model="claude-sonnet-4-6",
        )
        result = await session.invoke_skill(
            skill,
            "Use this skill to review this shell command for safety: rm -rf /tmp/build-cache",
        )

        print(f"Skill: {source_uuid}/{skill_name}")
        print(f"Session: {session.id}")
        print(result.text)
    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
