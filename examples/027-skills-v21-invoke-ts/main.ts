/**
 * 027 — Skills V2.1 Invocation (TypeScript SDK)
 *
 * Invoke a specific skill using canonical refs (`SkillKey`) from the rebuilt
 * skill system.
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "@rkat/sdk";
import type { SkillKey } from "@rkat/sdk";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_SOURCE_UUID = "dc256086-0d2f-4f61-a307-320d4148107f";
const DEFAULT_SKILL_NAME = "shell-patterns";

const SKILL_BODY = `---
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
`;

function prepareLocalSkillSource(sourceUuid: string, skillName: string) {
  const root = dirname(fileURLToPath(import.meta.url));
  const work = join(root, ".work");
  const contextRoot = join(work, "project");
  const stateRoot = join(work, "state");
  const userConfigRoot = join(work, "user");
  const skillDir = join(contextRoot, ".rkat", "skills", skillName);
  mkdirSync(skillDir, { recursive: true });
  mkdirSync(stateRoot, { recursive: true });
  mkdirSync(userConfigRoot, { recursive: true });
  writeFileSync(join(skillDir, "SKILL.md"), SKILL_BODY, "utf8");
  writeFileSync(
    join(contextRoot, ".rkat", "skills.toml"),
    [
      "enabled = true",
      "",
      "[[repositories]]",
      'name = "example-local"',
      `source_uuid = "${sourceUuid}"`,
      'type = "filesystem"',
      'path = ".rkat/skills"',
      "",
    ].join("\n"),
    "utf8",
  );
  return { contextRoot, stateRoot, userConfigRoot };
}

async function main() {
  const sourceUuid = process.env.MEERKAT_SKILL_SOURCE_UUID ?? DEFAULT_SOURCE_UUID;
  const skillName = process.env.MEERKAT_SKILL_NAME ?? DEFAULT_SKILL_NAME;
  const roots = prepareLocalSkillSource(sourceUuid, skillName);

  const client = new MeerkatClient();
  await client.connect({ isolated: true, ...roots });

  try {
    client.requireCapability("skills");

    // Create a session, then invoke the skill on it.
    const skill: SkillKey = { sourceUuid, skillName };
    const session = await client.createSession("Hello", {
      model: "claude-sonnet-4-6",
    });
    const result = await session.invokeSkill(
      skill,
      "Use this skill to review this shell command for safety: rm -rf /tmp/build-cache",
    );

    console.log(`Skill: ${sourceUuid}/${skillName}`);
    console.log(`Session: ${session.id}`);
    console.log(result.text);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
