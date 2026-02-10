/**
 * Skill invocation helpers for the Meerkat SDK.
 */

import type { MeerkatClient } from "./client.js";
import type { WireRunResult } from "./generated/types.js";
import { CapabilityUnavailableError } from "./generated/errors.js";

/**
 * Helpers for working with Meerkat skills.
 *
 * Skills are loaded by the agent from filesystem and embedded sources.
 * The agent's system prompt contains an inventory of available skills.
 * To invoke a skill, include its reference (e.g. `/shell-patterns`)
 * in the user prompt.
 *
 * @example
 * ```ts
 * const helper = new SkillHelper(client);
 * helper.requireSkills();
 * const result = await helper.invoke(sessionId, "/shell-patterns", "How do I run a background job?");
 * ```
 */
export class SkillHelper {
  private client: MeerkatClient;

  constructor(client: MeerkatClient) {
    this.client = client;
  }

  /** Check if the skills capability is available in this runtime. */
  isAvailable(): boolean {
    return this.client.hasCapability("skills");
  }

  /** Throw if skills capability is not available. */
  requireSkills(): void {
    if (!this.isAvailable()) {
      throw new CapabilityUnavailableError(
        "CAPABILITY_UNAVAILABLE",
        "Skills capability is not available in this runtime. " +
          "Build with --features skills to enable.",
      );
    }
  }

  /**
   * Invoke a skill within an existing session.
   *
   * The skill reference (e.g. `/shell-patterns`) is prepended to the prompt.
   * The agent's system prompt already contains the skill inventory, so it
   * knows how to resolve the reference.
   */
  async invoke(
    sessionId: string,
    skillReference: string,
    prompt: string,
  ): Promise<WireRunResult> {
    this.requireSkills();
    const fullPrompt = `${skillReference} ${prompt}`;
    return this.client.startTurn(sessionId, fullPrompt);
  }

  /**
   * Create a new session and invoke a skill in the first turn.
   */
  async invokeNewSession(
    skillReference: string,
    prompt: string,
    model?: string,
  ): Promise<WireRunResult> {
    this.requireSkills();
    const fullPrompt = `${skillReference} ${prompt}`;
    return this.client.createSession({ prompt: fullPrompt, model });
  }
}
