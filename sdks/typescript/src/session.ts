/**
 * Session — the first-class handle for multi-turn agent conversations.
 *
 * @example
 * ```ts
 * const client = await Meerkat.connect();
 * const session = await client.createSession("Summarise this repo");
 * console.log(session.text);
 *
 * // Multi-turn
 * const result = await session.turn("Now list the open issues");
 *
 * // Streaming
 * for await (const event of session.stream("Explain the CI pipeline")) {
 *   if (event.type === "text_delta") {
 *     process.stdout.write(event.delta);
 *   }
 * }
 * ```
 */

import type { AgentEventEnvelope, RunResult, SkillRef, TurnToolOverlay } from "./types.js";
import type { MeerkatClient } from "./client.js";
import type { EventStream } from "./streaming.js";
import type { EventSubscription } from "./subscription.js";

export class Session {
  /** @internal */
  _lastResult: RunResult;

  private readonly _client: MeerkatClient;
  private readonly _id: string;
  private readonly _ref: string | undefined;

  /** @internal — constructed by MeerkatClient, not directly. */
  constructor(client: MeerkatClient, result: RunResult) {
    this._client = client;
    this._id = result.sessionId;
    this._ref = result.sessionRef;
    this._lastResult = result;
  }

  get id(): string {
    return this._id;
  }

  get ref(): string | undefined {
    return this._ref;
  }

  get lastResult(): RunResult {
    return this._lastResult;
  }

  get text(): string {
    return this._lastResult.text;
  }

  get usage() {
    return this._lastResult.usage;
  }

  get turns(): number {
    return this._lastResult.turns;
  }

  get toolCalls(): number {
    return this._lastResult.toolCalls;
  }

  get structuredOutput(): unknown {
    return this._lastResult.structuredOutput;
  }

  async turn(
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
    },
  ): Promise<RunResult> {
    const result = await this._client._startTurn(this._id, prompt, options);
    this._lastResult = result;
    return result;
  }

  stream(
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
    },
  ): EventStream {
    return this._client._startTurnStreaming(this._id, prompt, options, this);
  }

  async interrupt(): Promise<void> {
    await this._client._interrupt(this._id);
  }

  async archive(): Promise<void> {
    await this._client._archive(this._id);
  }

  async subscribeEvents(): Promise<EventSubscription<AgentEventEnvelope>> {
    return this._client.subscribeSessionEvents(this._id);
  }

  async invokeSkill(skillRef: SkillRef, prompt: string): Promise<RunResult> {
    this._client.requireCapability("skills");
    return this.turn(prompt, { skillRefs: [skillRef] });
  }

  async send(command: Record<string, unknown>): Promise<Record<string, unknown>> {
    const { session_id: _ignored, ...rest } = command;
    return this._client._send(this._id, rest);
  }

  async peers(): Promise<Array<Record<string, unknown>>> {
    const result = await this._client._peers(this._id);
    return (result.peers ?? []) as Array<Record<string, unknown>>;
  }

  toString(): string {
    const ref = this._ref ? ` ref=${this._ref}` : "";
    return `Session(id=${this._id}${ref})`;
  }
}
