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

DEFAULT_SOURCE_UUID = "dc256086-0d2f-4f61-a307-320d4148107f"
DEFAULT_SKILL_NAME = "shell-patterns"

SKILL_BODY = """---
name: shell-patterns
description: Review shell commands for safety and portability.
---

# Shell Patterns

Review shell commands before execution. Flag destructive operations, missing
quoting, non-portable flags, and places where a safer dry-run or explicit path
would reduce risk.

Respond with:
- Risk level
- Specific concern
- Safer command or mitigation
"""


def prepare_local_skill_source(source_uuid: str, skill_name: str) -> tuple[Path, Path, Path]:
    root = Path(__file__).resolve().parent
    work = root / ".work"
    context_root = work / "project"
    state_root = work / "state"
    user_config_root = work / "user"
    skill_dir = context_root / ".rkat" / "skills" / skill_name
    skill_dir.mkdir(parents=True, exist_ok=True)
    state_root.mkdir(parents=True, exist_ok=True)
    user_config_root.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_text(SKILL_BODY, encoding="utf-8")
    (context_root / ".rkat" / "skills.toml").write_text(
        "\n".join(
            [
                "enabled = true",
                "",
                "[[repositories]]",
                'name = "example-local"',
                f'source_uuid = "{source_uuid}"',
                'type = "filesystem"',
                'path = ".rkat/skills"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    return context_root, state_root, user_config_root


async def main() -> None:
    source_uuid = os.environ.get("MEERKAT_SKILL_SOURCE_UUID", DEFAULT_SOURCE_UUID)
    skill_name = os.environ.get("MEERKAT_SKILL_NAME", DEFAULT_SKILL_NAME)
    context_root, state_root, user_config_root = prepare_local_skill_source(source_uuid, skill_name)

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
