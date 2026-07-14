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

const PROJECT_LOCAL_SOURCE_UUID = "00000000-0000-4b11-8111-000000000002";
const DEFAULT_SKILL_NAME = "shell-patterns";

function skillBody(skillName: string): string {
  return `---
name: ${skillName}
description: Review shell commands for safety and portability.
---

# ${skillName}

Review shell commands before execution. Flag destructive operations, missing
quoting, non-portable flags, and places where a safer dry-run or explicit path
would reduce risk.

Respond with:
- Risk level
- Specific concern
- Safer command or mitigation
`;
}

function prepareLocalSkillSource(skillName: string) {
  const root = dirname(fileURLToPath(import.meta.url));
  const work = join(root, ".work");
  const contextRoot = join(work, "project");
  const stateRoot = join(work, "state");
  const userConfigRoot = join(work, "user");
  const skillDir = join(contextRoot, ".rkat", "skills", skillName);
  mkdirSync(skillDir, { recursive: true });
  mkdirSync(stateRoot, { recursive: true });
  mkdirSync(userConfigRoot, { recursive: true });
  writeFileSync(join(skillDir, "SKILL.md"), skillBody(skillName), "utf8");
  return { contextRoot, stateRoot, userConfigRoot };
}

async function main() {
  const sourceUuid = PROJECT_LOCAL_SOURCE_UUID;
  const skillName = process.env.MEERKAT_SKILL_NAME ?? DEFAULT_SKILL_NAME;
  const roots = prepareLocalSkillSource(skillName);

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
