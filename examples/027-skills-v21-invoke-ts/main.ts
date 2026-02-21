/**
 * 027 — Skills V2.1 Invocation (TypeScript SDK)
 *
 * Invoke a specific skill using canonical refs (`SkillKey`) from the rebuilt
 * skill system.
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... \
 *   MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f \
 *   MEERKAT_SKILL_NAME=shell-patterns \
 *   npx tsx main.ts
 */

import { MeerkatClient } from "@rkat/sdk";
import type { SkillKey } from "@rkat/sdk";

async function main() {
  const sourceUuid = process.env.MEERKAT_SKILL_SOURCE_UUID;
  const skillName = process.env.MEERKAT_SKILL_NAME;

  if (!sourceUuid || !skillName) {
    console.error("Missing required env vars:");
    console.error("  MEERKAT_SKILL_SOURCE_UUID=<source uuid>");
    console.error("  MEERKAT_SKILL_NAME=<skill name>");
    process.exit(1);
  }

  const client = new MeerkatClient();
  await client.connect();

  try {
    client.requireCapability("skills");

    // Create a session, then invoke the skill on it.
    const skill: SkillKey = { sourceUuid, skillName };
    const session = await client.createSession("Hello", {
      model: "claude-sonnet-4-5",
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
