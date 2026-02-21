/**
 * Skill invocation helpers for the Meerkat SDK.
 */

import type { MeerkatClient } from "./client.js";
import type { WireRunResult } from "./generated/types.js";
import { CapabilityUnavailableError } from "./generated/errors.js";

export type SkillKey = {
  source_uuid: string;
  skill_name: string;
};

export type SkillRef = SkillKey | string;

function isStructured(ref: SkillRef): ref is SkillKey {
  return typeof ref !== "string";
}

function normalizeRef(ref: SkillRef): SkillKey {
  if (isStructured(ref)) return ref;
  const value = ref.startsWith("/") ? ref.slice(1) : ref;
  const [source_uuid, ...rest] = value.split("/");
  if (!source_uuid || rest.length === 0) {
    throw new Error(
      `Invalid legacy skill reference '${ref}'. Expected '<source_uuid>/<skill_name>'.`,
    );
  }
  console.warn(
    `[meerkat-sdk] legacy skill reference '${ref}' is deprecated; pass { source_uuid, skill_name } instead.`,
  );
  return { source_uuid, skill_name: rest.join("/") };
}

/**
 * Helpers for working with Meerkat skills.
 */
export class SkillHelper {
  private client: MeerkatClient;

  constructor(client: MeerkatClient) {
    this.client = client;
  }

  isAvailable(): boolean {
    return this.client.hasCapability("skills");
  }

  requireSkills(): void {
    if (!this.isAvailable()) {
      throw new CapabilityUnavailableError(
        "CAPABILITY_UNAVAILABLE",
        "Skills capability is not available in this runtime. " +
          "Build with --features skills to enable.",
      );
    }
  }

  async invoke(
    sessionId: string,
    skillRef: SkillRef,
    prompt: string,
  ): Promise<WireRunResult> {
    this.requireSkills();
    return this.client.startTurn(sessionId, prompt, {
      skill_refs: [normalizeRef(skillRef)],
    });
  }

  async invokeNewSession(
    skillRef: SkillRef,
    prompt: string,
    model?: string,
  ): Promise<WireRunResult> {
    this.requireSkills();
    return this.client.createSession({
      prompt,
      model,
      skill_refs: [normalizeRef(skillRef)],
    });
  }

  async listResources(sessionId: string, skillRef: SkillRef): Promise<WireRunResult> {
    this.requireSkills();
    const canonical = normalizeRef(skillRef);
    const id = `${canonical.source_uuid}/${canonical.skill_name}`;
    return this.client.startTurn(
      sessionId,
      `Use skill_list_resources for id '${id}' and return only the tool result JSON.`,
      { skill_refs: [canonical] },
    );
  }

  async readResource(
    sessionId: string,
    skillRef: SkillRef,
    path: string,
  ): Promise<WireRunResult> {
    this.requireSkills();
    const canonical = normalizeRef(skillRef);
    const id = `${canonical.source_uuid}/${canonical.skill_name}`;
    return this.client.startTurn(
      sessionId,
      `Use skill_read_resource for id '${id}' and path '${path}', then return only the tool result JSON.`,
      { skill_refs: [canonical] },
    );
  }

  async invokeFunction(
    sessionId: string,
    skillRef: SkillRef,
    functionName: string,
    argumentsValue: unknown,
  ): Promise<WireRunResult> {
    this.requireSkills();
    const canonical = normalizeRef(skillRef);
    const id = `${canonical.source_uuid}/${canonical.skill_name}`;
    return this.client.startTurn(
      sessionId,
      `Use skill_invoke_function for id '${id}', function '${functionName}', arguments: ${JSON.stringify(argumentsValue)}. Return only the tool result JSON.`,
      { skill_refs: [canonical] },
    );
  }
}
