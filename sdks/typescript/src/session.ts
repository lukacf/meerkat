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

import type { AgentEvent } from "./events.js";
import type { RunResult, SkillRef, TurnToolOverlay } from "./types.js";
import type { MeerkatClient } from "./client.js";
import type { CommsEventStream, EventStream } from "./streaming.js";

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

  // -- Identity -----------------------------------------------------------

  /** The stable UUID for this session. */
  get id(): string {
    return this._id;
  }

  /** Optional human-readable session reference. */
  get ref(): string | undefined {
    return this._ref;
  }

  // -- Last result shortcuts ----------------------------------------------

  /** The most recent {@link RunResult}. */
  get lastResult(): RunResult {
    return this._lastResult;
  }

  /** The assistant's text from the last turn. */
  get text(): string {
    return this._lastResult.text;
  }

  /** Token usage from the last turn. */
  get usage() {
    return this._lastResult.usage;
  }

  /** Number of LLM turns in the last run. */
  get turns(): number {
    return this._lastResult.turns;
  }

  /** Number of tool calls in the last run. */
  get toolCalls(): number {
    return this._lastResult.toolCalls;
  }

  /** Structured output from the last run, if requested. */
  get structuredOutput(): unknown {
    return this._lastResult.structuredOutput;
  }

  // -- Multi-turn ---------------------------------------------------------

  /** Run another turn on this session (non-streaming). */
  async turn(
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
    },
  ): Promise<RunResult> {
    const result = await this._client._startTurn(
      this._id,
      prompt,
      options,
    );
    this._lastResult = result;
    return result;
  }

  /**
   * Run another turn on this session with streaming events.
   *
   * Returns an {@link EventStream} `AsyncIterable<AgentEvent>`.
   *
   * @example
   * ```ts
   * for await (const event of session.stream("prompt")) {
   *   // ...
   * }
   * ```
   */
  stream(
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
    },
  ): EventStream {
    return this._client._startTurnStreaming(
      this._id,
      prompt,
      options,
      this,
    );
  }

  // -- Lifecycle ----------------------------------------------------------

  /** Cancel the currently running turn, if any. */
  async interrupt(): Promise<void> {
    await this._client._interrupt(this._id);
  }

  /** Archive (remove) this session from the server. */
  async archive(): Promise<void> {
    await this._client._archive(this._id);
  }

  // -- Skills convenience -------------------------------------------------

  /**
   * Invoke a skill in this session.
   *
   * Accepts a {@link SkillKey} or a legacy string reference.
   * Sends the structured `skill_refs` parameter to the runtime.
   */
  async invokeSkill(
    skillRef: SkillRef,
    prompt: string,
  ): Promise<RunResult> {
    this._client.requireCapability("skills");
    return this.turn(prompt, { skillRefs: [skillRef] });
  }

  // -- Comms convenience --------------------------------------------------

  /** Send a comms command scoped to this session. */
  async send(command: Record<string, unknown>): Promise<Record<string, unknown>> {
    return this._client._send(this._id, command);
  }

  /** List peers visible to this session's comms runtime. */
  async peers(): Promise<Array<Record<string, unknown>>> {
    const result = await this._client._peers(this._id);
    return (result.peers ?? []) as Array<Record<string, unknown>>;
  }

  async openCommsStream(
    options?: { scope?: "session" | "interaction"; interactionId?: string },
  ): Promise<CommsEventStream> {
    return this._client.openCommsStream(this._id, options);
  }

  toString(): string {
    const ref = this._ref ? ` ref=${this._ref}` : "";
    return `Session(id=${this._id}${ref})`;
  }
}
