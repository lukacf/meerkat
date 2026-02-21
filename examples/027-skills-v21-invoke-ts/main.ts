/**
 * 027 — Skills V2.1 Invocation (TypeScript SDK)
 *
 * Invoke a specific skill using canonical refs (`{ source_uuid, skill_name }`)
 * from the rebuilt skill system.
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... \
 *   MEERKAT_SKILL_SOURCE_UUID=dc256086-0d2f-4f61-a307-320d4148107f \
 *   MEERKAT_SKILL_NAME=shell-patterns \
 *   npx tsx main.ts
 */

import { MeerkatClient, SkillHelper } from "@rkat/sdk";

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
    const skills = new SkillHelper(client);
    skills.requireSkills();

    const result = await skills.invokeNewSession(
      {
        source_uuid: sourceUuid,
        skill_name: skillName,
      },
      "Use this skill to review this shell command for safety: rm -rf /tmp/build-cache",
      "claude-sonnet-4-5",
    );

    console.log(`Skill: ${sourceUuid}/${skillName}`);
    console.log(`Session: ${result.session_id}`);
    console.log(result.text);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
