/**
 * Meerkat client — spawns rkat-rpc and communicates via JSON-RPC.
 *
 * @example
 * ```ts
 * import { MeerkatClient } from "@rkat/sdk";
 *
 * const client = new MeerkatClient();
 * await client.connect();
 *
 * const session = await client.createSession("Hello!");
 * console.log(session.text);
 *
 * const result = await session.turn("Tell me more");
 * console.log(result.text);
 *
 * for await (const event of session.stream("Explain in detail")) {
 *   if (event.type === "text_delta") {
 *     process.stdout.write(event.delta);
 *   }
 * }
 *
 * await session.archive();
 * await client.close();
 * ```
 */

import { spawn, type ChildProcess } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";
import { MeerkatError, CapabilityUnavailableError } from "./generated/errors.js";
import {
  CONTRACT_VERSION,
  type McpAddParams,
  type McpLiveOpResponse,
  type McpReloadParams,
  type McpRemoveParams,
} from "./generated/types.js";
import { DeferredSession, Session } from "./session.js";
import { Mob } from "./mob.js";
import { parseCoreEvent } from "./events.js";
import { EventStream, AsyncQueue } from "./streaming.js";
import { EventSubscription } from "./subscription.js";
import type {
  AgentEventEnvelope,
  AttributedMobEvent,
  Capability,
  MobCreateOptions,
  MobFlowStatus,
  MobMember,
  MobStatus,
  MobSummary,
  RunResult,
  SchemaWarning,
  SessionInfo,
  SessionOptions,
  SkillKey,
  SkillRef,
  SkillRuntimeDiagnostics,
  TurnToolOverlay,
  Usage,
} from "./types.js";

const MEERKAT_REPO = "lukacf/meerkat";
const MEERKAT_RELEASE_BINARY = "rkat-rpc";
const MEERKAT_BINARY_CACHE_ROOT = path.join(
  os.homedir(),
  ".cache",
  "meerkat",
  "bin",
  MEERKAT_RELEASE_BINARY,
);

interface ResolvedBinary {
  command: string;
  useLegacySubcommand: boolean;
}

interface PlatformTarget {
  target: string;
  archiveExt: "tar.gz" | "zip";
  binaryName: string;
}

/** Options for connecting to the rkat-rpc runtime. */
export interface ConnectOptions {
  realmId?: string;
  instanceId?: string;
  realmBackend?: "jsonl" | "redb";
  isolated?: boolean;
  stateRoot?: string;
  contextRoot?: string;
  userConfigRoot?: string;
}

/**
 * Normalize a SkillRef to the wire format { source_uuid, skill_name }.
 *
 * SkillKey objects are converted from camelCase to snake_case.
 * Legacy strings are parsed and emit a console warning.
 */
function normalizeSkillRef(ref: SkillRef): { source_uuid: string; skill_name: string } {
  if (typeof ref !== "string") {
    return { source_uuid: ref.sourceUuid, skill_name: ref.skillName };
  }
  const value = ref.startsWith("/") ? ref.slice(1) : ref;
  const [sourceUuid, ...rest] = value.split("/");
  if (!sourceUuid || rest.length === 0) {
    throw new Error(
      `Invalid skill reference '${ref}'. Expected '<source_uuid>/<skill_name>'.`,
    );
  }
  console.warn(
    `[meerkat-sdk] legacy skill reference '${ref}' is deprecated; pass { sourceUuid, skillName } instead.`,
  );
  return { source_uuid: sourceUuid, skill_name: rest.join("/") };
}

function skillRefsToWire(refs: SkillRef[] | undefined): Array<{ source_uuid: string; skill_name: string }> | undefined {
  if (!refs) return undefined;
  return refs.map(normalizeSkillRef);
}

export class MeerkatClient {
  private process: ChildProcess | null = null;
  private requestId = 0;
  private _capabilities: Capability[] = [];
  private _methods = new Set<string>();
  private rkatPath: string;
  private pendingRequests = new Map<
    number,
    { resolve: (value: Record<string, unknown>) => void; reject: (reason: unknown) => void }
  >();
  private eventQueues = new Map<string, AsyncQueue<Record<string, unknown> | null>>();
  private streamQueues = new Map<string, AsyncQueue<Record<string, unknown> | null>>();
  private pendingStreamQueue: AsyncQueue<Record<string, unknown> | null> | null = null;
  private pendingStreamRequestId: number | null = null;
  private unmatchedStreamBuffer = new Map<string, Record<string, unknown>[]>();
  private unmatchedStandaloneStreamBuffer = new Map<string, Record<string, unknown>[]>();
  private unmatchedStandaloneStreamEnd = new Map<string, Record<string, unknown>>();
  private streamTerminalOutcomes = new Map<string, Record<string, unknown>>();
  private rl: ReadlineInterface | null = null;
  private toolHandlers = new Map<
    string,
    {
      description: string;
      inputSchema: Record<string, unknown>;
      handler: (args: Record<string, unknown>) => Promise<string>;
    }
  >();

  constructor(rkatPath = "rkat-rpc") {
    this.rkatPath = rkatPath;
  }

  /**
   * Register a callback tool handler.
   *
   * @example
   * ```ts
   * client.registerTool("search", "Search the web", { type: "object" }, async (args) => {
   *   return `Results for ${args.q}`;
   * });
   * ```
   */
  registerTool(
    name: string,
    description: string,
    inputSchema: Record<string, unknown>,
    handler: (args: Record<string, unknown>) => Promise<string>,
  ): void {
    this.toolHandlers.set(name, { description, inputSchema, handler });
    // If already connected, register the new tool with the server immediately.
    if (this.process?.stdin) {
      this.request("tools/register", {
        tools: [{ name, description, input_schema: inputSchema }],
      }).catch(() => {
        // Best-effort: tool registration may fail if connection is closing.
      });
    }
  }

  // -- Connection ---------------------------------------------------------

  async connect(options?: ConnectOptions): Promise<this> {
    if (options?.realmId && options?.isolated) {
      throw new MeerkatError("INVALID_ARGS", "realmId and isolated cannot both be set");
    }

    const resolved = await MeerkatClient.resolveBinaryPath(this.rkatPath);
    this.rkatPath = resolved.command;

    const args = MeerkatClient.buildArgs(resolved.useLegacySubcommand, options);
    if (resolved.useLegacySubcommand && args.length > 1) {
      throw new MeerkatError(
        "LEGACY_BINARY_UNSUPPORTED",
        "Realm/context options require the standalone rkat-rpc binary. Install rkat-rpc and retry.",
      );
    }

    this.process = spawn(this.rkatPath, args, {
      stdio: ["pipe", "pipe", "pipe"],
    });

    this.rl = createInterface({ input: this.process.stdout! });
    this.rl.on("line", (line: string) => this.handleLine(line));

    // Handshake
    const initResult = await this.request("initialize", {});
    const serverVersion = String(initResult.contract_version ?? "");
    if (!MeerkatClient.checkVersionCompatible(serverVersion, CONTRACT_VERSION)) {
      throw new MeerkatError(
        "VERSION_MISMATCH",
        `Server version ${serverVersion} incompatible with SDK ${CONTRACT_VERSION}`,
      );
    }
    this._methods = new Set(
      Array.isArray(initResult.methods)
        ? initResult.methods.map((method) => String(method))
        : [],
    );

    // Fetch capabilities
    const capsResult = await this.request("capabilities/get", {});
    const rawCaps = (capsResult.capabilities as Array<Record<string, unknown>>) ?? [];
    this._capabilities = rawCaps.map(
      (cap): Capability => ({
        id: String(cap.id ?? ""),
        description: String(cap.description ?? ""),
        status: MeerkatClient.normalizeStatus(cap.status),
      }),
    );

    // Register callback tools if any were declared.
    if (this.toolHandlers.size > 0) {
      const tools = Array.from(this.toolHandlers.entries()).map(
        ([name, { description, inputSchema }]) => ({
          name,
          description,
          input_schema: inputSchema,
        }),
      );
      await this.request("tools/register", { tools });
    }

    return this;
  }

  async close(): Promise<void> {
    if (this.rl) {
      this.rl.close();
      this.rl = null;
    }
    if (this.process) {
      this.process.kill();
      this.process = null;
    }
    for (const [, pending] of this.pendingRequests) {
      pending.reject(new MeerkatError("CLIENT_CLOSED", "Client closed"));
    }
    this.pendingRequests.clear();
    for (const [, queue] of this.eventQueues) {
      queue.put(null);
    }
    this.eventQueues.clear();
    for (const [, queue] of this.streamQueues) {
      queue.put(null);
    }
    this.streamQueues.clear();
    this.streamTerminalOutcomes.clear();
    if (this.pendingStreamQueue) {
      this.pendingStreamQueue.put(null);
      this.pendingStreamQueue = null;
      this.pendingStreamRequestId = null;
      this.unmatchedStreamBuffer.clear();
    }
  }

  // -- Session lifecycle --------------------------------------------------

  /**
   * Create a new session, run the first turn, and return a {@link Session}.
   *
   * @example
   * ```ts
   * const session = await client.createSession("Summarise this project", {
   *   model: "claude-sonnet-4-5",
   * });
   * console.log(session.text);
   * ```
   */
  async createSession(
    prompt: string,
    options?: SessionOptions,
  ): Promise<Session> {
    const params = MeerkatClient.buildCreateParams(prompt, options);
    const raw = await this.request("session/create", params);
    const result = MeerkatClient.parseRunResult(raw);
    return new Session(this, result);
  }

  /**
   * Create a new session and stream typed events from the first turn.
   *
   * Returns an {@link EventStream} `AsyncIterable<AgentEvent>`.
   */
  createSessionStreaming(
    prompt: string,
    options?: SessionOptions,
  ): EventStream {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    const params = MeerkatClient.buildCreateParams(prompt, options);
    this.requestId++;
    const requestId = this.requestId;

    if (this.pendingStreamQueue) {
      throw new MeerkatError(
        "INVALID_STATE",
        "Only one createSessionStreaming request can be pending at a time",
      );
    }
    const queue = new AsyncQueue<Record<string, unknown> | null>();
    this.pendingStreamQueue = queue;
    this.pendingStreamRequestId = requestId;
    const responsePromise = this.registerRequest(requestId);

    // When response arrives, bind the queue to the session_id
    const wrappedPromise = responsePromise.then((result) => {
      const sid = String(result.session_id ?? "");
      if (sid) {
        const buffered = this.unmatchedStreamBuffer.get(sid) ?? [];
        for (const evt of buffered) {
          queue.put(evt);
        }
        this.unmatchedStreamBuffer.delete(sid);
        this.eventQueues.set(sid, queue);
      }
      this.pendingStreamQueue = null;
      this.pendingStreamRequestId = null;
      this.unmatchedStreamBuffer.clear();
      return result;
    });

    const rpcRequest = { jsonrpc: "2.0", id: requestId, method: "session/create", params };
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");

    return new EventStream({
      sessionId: "",
      eventQueue: queue,
      responsePromise: wrappedPromise,
      parseResult: MeerkatClient.parseRunResult,
    });
  }

  /**
   * Create a new session without running the first turn.
   *
   * Returns a {@link DeferredSession} whose `startTurn()` executes the first
   * turn with optional per-turn overrides.
   */
  async createDeferredSession(
    prompt: string,
    options?: SessionOptions,
  ): Promise<DeferredSession> {
    const params = MeerkatClient.buildCreateParams(prompt, options);
    params.initial_turn = "deferred";
    const raw = await this.request("session/create", params);
    return new DeferredSession(
      this,
      String(raw.session_id ?? ""),
      raw.session_ref != null ? String(raw.session_ref) : undefined,
    );
  }

  // -- Session queries ----------------------------------------------------

  async listSessions(): Promise<SessionInfo[]> {
    const result = await this.request("session/list", {});
    const sessions = (result.sessions as Array<Record<string, unknown>>) ?? [];
    return sessions.map(
      (s): SessionInfo => ({
        sessionId: String(s.session_id ?? ""),
        sessionRef: s.session_ref != null ? String(s.session_ref) : undefined,
        createdAt: String(s.created_at ?? ""),
        updatedAt: String(s.updated_at ?? ""),
        messageCount: Number(s.message_count ?? 0),
        totalTokens: Number(s.total_tokens ?? 0),
        isActive: Boolean(s.is_active),
      }),
    );
  }

  async readSession(sessionId: string): Promise<Record<string, unknown>> {
    return this.request("session/read", { session_id: sessionId });
  }

  // -- Capabilities -------------------------------------------------------

  get capabilities(): readonly Capability[] {
    return this._capabilities;
  }

  hasCapability(capabilityId: string): boolean {
    if (capabilityId === "mob") {
      return (
        this._methods.has("mob/create")
        || this._methods.has("mob/list")
        || this._methods.has("mob/call")
      );
    }
    return this._capabilities.some(
      (c) => c.id === capabilityId && c.status === "Available",
    );
  }

  requireCapability(capabilityId: string): void {
    if (!this.hasCapability(capabilityId)) {
      throw new CapabilityUnavailableError(
        "CAPABILITY_UNAVAILABLE",
        `Capability '${capabilityId}' is not available`,
      );
    }
  }

  // -- Config -------------------------------------------------------------

  async getConfig(): Promise<Record<string, unknown>> {
    return this.request("config/get", {});
  }

  async setConfig(
    config: Record<string, unknown>,
    options?: { expectedGeneration?: number },
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = { config };
    if (options?.expectedGeneration !== undefined) {
      params.expected_generation = options.expectedGeneration;
    }
    return this.request("config/set", params);
  }

  async patchConfig(
    patch: Record<string, unknown>,
    options?: { expectedGeneration?: number },
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = { patch };
    if (options?.expectedGeneration !== undefined) {
      params.expected_generation = options.expectedGeneration;
    }
    return this.request("config/patch", params);
  }

  async mcpAdd(params: McpAddParams): Promise<McpLiveOpResponse> {
    const raw = await this.request("mcp/add", params as unknown as Record<string, unknown>);
    return MeerkatClient.parseMcpLiveOpResponse(raw);
  }

  async mcpRemove(params: McpRemoveParams): Promise<McpLiveOpResponse> {
    const raw = await this.request("mcp/remove", params as unknown as Record<string, unknown>);
    return MeerkatClient.parseMcpLiveOpResponse(raw);
  }

  async mcpReload(params: McpReloadParams): Promise<McpLiveOpResponse> {
    const raw = await this.request(
      "mcp/reload",
      params as unknown as Record<string, unknown>,
    );
    return MeerkatClient.parseMcpLiveOpResponse(raw);
  }

  // snake_case aliases for parity with other SDKs/surfaces
  async mcp_add(params: McpAddParams): Promise<McpLiveOpResponse> {
    return this.mcpAdd(params);
  }

  async mcp_remove(params: McpRemoveParams): Promise<McpLiveOpResponse> {
    return this.mcpRemove(params);
  }

  async mcp_reload(params: McpReloadParams): Promise<McpLiveOpResponse> {
    return this.mcpReload(params);
  }

  // -- Skills ---------------------------------------------------------------

  async listSkills(): Promise<Array<Record<string, unknown>>> {
    const result = await this.request("skills/list", {});
    return (result.skills as Array<Record<string, unknown>>) ?? [];
  }

  async inspectSkill(
    id: string,
    options?: { source?: string },
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = { id };
    if (options?.source !== undefined) {
      params.source = options.source;
    }
    return this.request("skills/inspect", params);
  }

  async listMobPrefabs(): Promise<Array<Record<string, unknown>>> {
    const result = await this.request("mob/prefabs", {});
    return (result.prefabs as Array<Record<string, unknown>>) ?? [];
  }

  async list_mob_prefabs(): Promise<Array<Record<string, unknown>>> {
    return this.listMobPrefabs();
  }

  async listMobTools(): Promise<Array<Record<string, unknown>>> {
    const result = await this.request("mob/tools", {});
    return (result.tools as Array<Record<string, unknown>>) ?? [];
  }

  async callMobTool(
    name: string,
    argumentsPayload?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("mob/call", {
      name,
      arguments: argumentsPayload ?? {},
    });
  }

  async subscribeSessionEvents(sessionId: string): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.openEventSubscription(
      "session/stream_open",
      { session_id: sessionId },
      "session/stream_close",
      MeerkatClient.parseAgentEventEnvelope,
    );
  }

  async createMob(options: MobCreateOptions): Promise<Mob> {
    this.requireCapability("mob");
    const result = await this.request("mob/create", options);
    return new Mob(this, String(result.mob_id ?? ""));
  }

  mob(mobId: string): Mob {
    return new Mob(this, mobId);
  }

  async listMobs(): Promise<MobSummary[]> {
    this.requireCapability("mob");
    const result = await this.request("mob/list", {});
    const mobs = (result.mobs as Array<Record<string, unknown>>) ?? [];
    return mobs.map((mob) => ({
      mobId: String(mob.mob_id ?? mob.mobId ?? ""),
      status: String(mob.status ?? ""),
    }));
  }

  async mobStatus(mobId: string): Promise<{ mobId: string; status: string }> {
    const result = await this.request("mob/status", { mob_id: mobId });
    const rawStatus = result.status;
    const status = typeof rawStatus === "string"
      ? rawStatus
      : (typeof rawStatus === "object" && rawStatus !== null
        ? String(Object.keys(rawStatus)[0] ?? "unknown")
        : String(rawStatus ?? "unknown"));
    return { mobId: String(result.mob_id ?? mobId), status };
  }

  async listMobMembers(mobId: string): Promise<MobMember[]> {
    const result = await this.request("mob/members", { mob_id: mobId });
    const members = (result.members as Array<Record<string, unknown>>) ?? [];
    return members.map((member) => ({
      meerkatId: String(member.meerkat_id ?? member.meerkatId ?? ""),
      profile: String(member.profile_name ?? member.profile ?? ""),
      memberRef: (member.member_ref as Record<string, unknown> | undefined),
      runtimeMode: member.runtime_mode != null ? String(member.runtime_mode) : undefined,
      state: member.state != null ? String(member.state) : undefined,
      wiredTo: Array.isArray(member.wired_to)
        ? member.wired_to.map((peer) => String(peer))
        : undefined,
      labels: member.labels && typeof member.labels === 'object'
        ? Object.fromEntries(Object.entries(member.labels as Record<string, unknown>).map(([key, value]) => [key, String(value)]))
        : undefined,
      sessionId: member.member_ref && typeof member.member_ref === 'object'
        ? (member.member_ref as Record<string, unknown>).session_id != null
          ? String((member.member_ref as Record<string, unknown>).session_id)
          : undefined
        : undefined,
    }));
  }

  async spawnMobMember(
    mobId: string,
    options: {
      profile: string;
      meerkatId: string;
      initialMessage?: string;
      runtimeMode?: string;
      backend?: string;
      resumeSessionId?: string;
      labels?: Record<string, string>;
      context?: Record<string, unknown>;
      additionalInstructions?: string[];
    },
  ): Promise<Record<string, unknown>> {
    return this.request("mob/spawn", {
      mob_id: mobId,
      profile: options.profile,
      meerkat_id: options.meerkatId,
      initial_message: options.initialMessage,
      runtime_mode: options.runtimeMode,
      backend: options.backend,
      resume_session_id: options.resumeSessionId,
      labels: options.labels,
      context: options.context,
      additional_instructions: options.additionalInstructions,
    });
  }

  async retireMobMember(mobId: string, meerkatId: string): Promise<void> {
    await this.request("mob/retire", { mob_id: mobId, meerkat_id: meerkatId });
  }

  async respawnMobMember(mobId: string, meerkatId: string, initialMessage?: string): Promise<void> {
    await this.request("mob/respawn", { mob_id: mobId, meerkat_id: meerkatId, initial_message: initialMessage });
  }

  async wireMobMembers(mobId: string, a: string, b: string): Promise<void> {
    await this.request("mob/wire", { mob_id: mobId, a, b });
  }

  async unwireMobMembers(mobId: string, a: string, b: string): Promise<void> {
    await this.request("mob/unwire", { mob_id: mobId, a, b });
  }

  async mobLifecycle(mobId: string, action: 'stop' | 'resume' | 'complete' | 'reset' | 'destroy'): Promise<void> {
    await this.request("mob/lifecycle", { mob_id: mobId, action });
  }

  async sendMobMessage(mobId: string, meerkatId: string, message: string): Promise<void> {
    await this.request("mob/send", { mob_id: mobId, meerkat_id: meerkatId, message });
  }

  async appendMobSystemContext(
    mobId: string,
    meerkatId: string,
    text: string,
    options?: { source?: string; idempotencyKey?: string },
  ): Promise<Record<string, unknown>> {
    return this.request("mob/append_system_context", {
      mob_id: mobId,
      meerkat_id: meerkatId,
      text,
      source: options?.source,
      idempotency_key: options?.idempotencyKey,
    });
  }

  async listMobFlows(mobId: string): Promise<string[]> {
    const result = await this.request("mob/flows", { mob_id: mobId });
    return (result.flows as string[]) ?? [];
  }

  async runMobFlow(mobId: string, flowId: string, params: Record<string, unknown> = {}): Promise<string> {
    const result = await this.request("mob/flow_run", { mob_id: mobId, flow_id: flowId, params });
    return String(result.run_id ?? "");
  }

  async getMobFlowStatus(mobId: string, runId: string): Promise<MobFlowStatus | null> {
    const result = await this.request("mob/flow_status", { mob_id: mobId, run_id: runId });
    return result.run == null ? null : { run: result.run as Record<string, unknown> };
  }

  async cancelMobFlow(mobId: string, runId: string): Promise<void> {
    await this.request("mob/flow_cancel", { mob_id: mobId, run_id: runId });
  }

  async subscribeMobEvents(mobId: string): Promise<EventSubscription<AttributedMobEvent>> {
    return this.openEventSubscription(
      "mob/stream_open",
      { mob_id: mobId },
      "mob/stream_close",
      MeerkatClient.parseAttributedMobEvent,
    );
  }

  async subscribeMobMemberEvents(mobId: string, meerkatId: string): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.openEventSubscription(
      "mob/stream_open",
      { mob_id: mobId, member_id: meerkatId },
      "mob/stream_close",
      MeerkatClient.parseAgentEventEnvelope,
    );
  }

  private async openEventSubscription<T>(
    openMethod: string,
    params: Record<string, unknown>,
    closeMethod: string,
    parse: (raw: Record<string, unknown>) => T,
  ): Promise<EventSubscription<T>> {
    const result = this.process?.stdin
      ? await (async () => {
          this.requestId++;
          const requestId = this.requestId;
          const responsePromise = this.registerRequest(requestId);
          const rpcRequest = { jsonrpc: "2.0", id: requestId, method: openMethod, params };
          this.process!.stdin!.write(JSON.stringify(rpcRequest) + "\n");
          return responsePromise;
        })()
      : await this.request(openMethod, params);
    const streamId = String(result.stream_id ?? "");
    if (!streamId) {
      throw new MeerkatError("INVALID_RESPONSE", `${openMethod} did not return stream_id`);
    }
    const queue = new AsyncQueue<Record<string, unknown> | null>();
    this.streamQueues.set(streamId, queue);
    const buffered = this.unmatchedStandaloneStreamBuffer.get(streamId) ?? [];
    for (const event of buffered) {
      queue.put(event);
    }
    this.unmatchedStandaloneStreamBuffer.delete(streamId);
    if (this.unmatchedStandaloneStreamEnd.delete(streamId)) {
      queue.put(null);
    }
    return new EventSubscription<T>({
      streamId,
      queue,
      closeRemote: async (id: string) => {
        this.streamQueues.delete(id);
        await this.request(closeMethod, { stream_id: id });
      },
      parseEvent: parse,
      getTerminalOutcome: () => this.streamTerminalOutcomes.get(streamId),
    });
  }

  private static parseAgentEventEnvelope(raw: Record<string, unknown>): AgentEventEnvelope {
    return {
      eventId: String(raw.event_id ?? raw.eventId ?? ""),
      sourceId: String(raw.source_id ?? raw.sourceId ?? ""),
      seq: Number(raw.seq ?? 0),
      timestampMs: Number(raw.timestamp_ms ?? raw.timestampMs ?? 0),
      payload: parseCoreEvent((raw.payload ?? {}) as Record<string, unknown>),
    };
  }

  private static parseAttributedMobEvent(raw: Record<string, unknown>): AttributedMobEvent {
    return {
      source: String(raw.source ?? ""),
      profile: String(raw.profile ?? ""),
      envelope: MeerkatClient.parseAgentEventEnvelope((raw.envelope ?? {}) as Record<string, unknown>),
    };
  }

  // -- Internal methods used by Session -----------------------------------

  /** @internal */
  async _startTurn(
    sessionId: string,
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
      hostMode?: boolean;
      model?: string;
      provider?: string;
      maxTokens?: number;
      systemPrompt?: string;
      outputSchema?: Record<string, unknown>;
      structuredOutputRetries?: number;
      providerParams?: Record<string, unknown>;
    },
  ): Promise<RunResult> {
    const params: Record<string, unknown> = { session_id: sessionId, prompt };
    const wireRefs = skillRefsToWire(options?.skillRefs);
    if (wireRefs) {
      params.skill_refs = wireRefs;
    }
    if (options?.skillReferences) {
      params.skill_references = options.skillReferences;
    }
    if (options?.flowToolOverlay) {
      params.flow_tool_overlay = {
        allowed_tools: options.flowToolOverlay.allowedTools,
        blocked_tools: options.flowToolOverlay.blockedTools,
      };
    }
    if (options?.hostMode != null) params.host_mode = options.hostMode;
    if (options?.model) params.model = options.model;
    if (options?.provider) params.provider = options.provider;
    if (options?.maxTokens) params.max_tokens = options.maxTokens;
    if (options?.systemPrompt) params.system_prompt = options.systemPrompt;
    if (options?.outputSchema) params.output_schema = options.outputSchema;
    if (options?.structuredOutputRetries != null) {
      params.structured_output_retries = options.structuredOutputRetries;
    }
    if (options?.providerParams) params.provider_params = options.providerParams;
    const raw = await this.request("turn/start", params);
    return MeerkatClient.parseRunResult(raw);
  }

  /** @internal */
  _startTurnStreaming(
    sessionId: string,
    prompt: string,
    options?: {
      skillRefs?: SkillRef[];
      skillReferences?: string[];
      flowToolOverlay?: TurnToolOverlay;
    },
    session?: Session,
  ): EventStream {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    this.requestId++;
    const requestId = this.requestId;

    const queue = new AsyncQueue<Record<string, unknown> | null>();
    this.eventQueues.set(sessionId, queue);

    const responsePromise = this.registerRequest(requestId);
    const params: Record<string, unknown> = { session_id: sessionId, prompt };
    const wireRefs = skillRefsToWire(options?.skillRefs);
    if (wireRefs) {
      params.skill_refs = wireRefs;
    }
    if (options?.skillReferences) {
      params.skill_references = options.skillReferences;
    }
    if (options?.flowToolOverlay) {
      params.flow_tool_overlay = {
        allowed_tools: options.flowToolOverlay.allowedTools,
        blocked_tools: options.flowToolOverlay.blockedTools,
      };
    }

    const rpcRequest = { jsonrpc: "2.0", id: requestId, method: "turn/start", params };
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");

    return new EventStream({
      sessionId,
      eventQueue: queue,
      responsePromise,
      parseResult: MeerkatClient.parseRunResult,
      session,
    });
  }

  /** @internal */
  async _interrupt(sessionId: string): Promise<void> {
    await this.request("turn/interrupt", { session_id: sessionId });
  }

  /** @internal */
  async _archive(sessionId: string): Promise<void> {
    await this.request("session/archive", { session_id: sessionId });
  }

  /** @internal */
  async _send(
    sessionId: string,
    command: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.send(sessionId, command);
  }

  /** @internal */
  async _peers(
    sessionId: string,
  ): Promise<Record<string, unknown>> {
    return this.peers(sessionId);
  }

  async send(
    sessionId: string,
    command: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("comms/send", { session_id: sessionId, ...command });
  }

  async peers(
    sessionId: string,
  ): Promise<Record<string, unknown>> {
    return this.request("comms/peers", { session_id: sessionId });
  }

  // -- Transport ----------------------------------------------------------

  private handleLine(line: string): void {
    let data: Record<string, unknown>;
    try {
      data = JSON.parse(line);
    } catch {
      return;
    }

    // Server→client callback request (has both id and method).
    if ("id" in data && "method" in data) {
      this.handleCallbackRequest(data);
      return;
    }

    if ("id" in data && typeof data.id === "number") {
      const pending = this.pendingRequests.get(data.id);
      if (pending) {
        this.pendingRequests.delete(data.id);
        const error = data.error as Record<string, unknown> | null | undefined;
        if (error) {
          pending.reject(
            new MeerkatError(
              String(error.code ?? "UNKNOWN"),
              String(error.message ?? "Unknown error"),
            ),
          );
        } else {
          pending.resolve((data.result ?? {}) as Record<string, unknown>);
        }
      }
    } else if ("method" in data) {
      const method = String(data.method ?? "");
      const params = (data.params ?? {}) as Record<string, unknown>;
      if (method === "session/stream_event" || method === "mob/stream_event") {
        const streamId = String(params.stream_id ?? "");
        const queue = this.streamQueues.get(streamId);
        const rawEvent = (params.event ?? params) as Record<string, unknown>;
        // Preserve scope fields when present (sub-agent / mob-member scoped events).
        const scopeId = params.scope_id as string | undefined;
        const scopePath = params.scope_path as unknown[] | undefined;
        const event: Record<string, unknown> =
          scopeId != null || scopePath != null
            ? { event: rawEvent, scope_id: scopeId, scope_path: scopePath }
            : rawEvent;
        if (queue && event) {
          queue.put(event);
        } else if (streamId && event) {
          const buffered = this.unmatchedStandaloneStreamBuffer.get(streamId) ?? [];
          buffered.push(event);
          this.unmatchedStandaloneStreamBuffer.set(streamId, buffered);
        }
        return;
      }
      if (method === "session/stream_end" || method === "mob/stream_end") {
        const streamId = String(params.stream_id ?? "");
        if (streamId) {
          this.streamTerminalOutcomes.set(streamId, params);
        }
        const queue = this.streamQueues.get(streamId);
        if (queue) {
          queue.put(null);
        } else if (streamId) {
          this.unmatchedStandaloneStreamEnd.set(streamId, params);
        }
        return;
      }
      const sessionId = String(params.session_id ?? "");
      const event = params.event as Record<string, unknown> | undefined;
      if (event) {
        const queue = this.eventQueues.get(sessionId);
        if (queue) {
          queue.put(event);
        } else if (this.pendingStreamQueue) {
          const buffered = this.unmatchedStreamBuffer.get(sessionId) ?? [];
          buffered.push(event);
          this.unmatchedStreamBuffer.set(sessionId, buffered);
        }
      }
    }
  }

  private request(method: string, params: unknown): Promise<Record<string, unknown>> {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    this.requestId++;
    const id = this.requestId;
    const rpcRequest = { jsonrpc: "2.0", id, method, params };
    const promise = this.registerRequest(id);
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");
    return promise;
  }

  private registerRequest(id: number): Promise<Record<string, unknown>> {
    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
    });
  }

  // -- Static helpers -----------------------------------------------------

  private static normalizeStatus(raw: unknown): string {
    if (typeof raw === "string") return raw;
    if (typeof raw === "object" && raw !== null) {
      // Rust can emit externally-tagged enums for status:
      // { DisabledByPolicy: { description: "..." } }
      return Object.keys(raw)[0] ?? "Unknown";
    }
    return String(raw);
  }

  private static parseSkillDiagnostics(raw: unknown): SkillRuntimeDiagnostics | undefined {
    if (!raw || typeof raw !== "object") return undefined;
    const data = raw as Record<string, unknown>;
    const sourceHealthRaw = data.source_health as Record<string, unknown> | undefined;
    const rawQuarantined = Array.isArray(data.quarantined)
      ? data.quarantined
      : [];

    const quarantined = rawQuarantined
      .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
      .map((item) => ({
        sourceUuid: String(item.source_uuid ?? ""),
        skillId: String(item.skill_id ?? ""),
        location: String(item.location ?? ""),
        errorCode: String(item.error_code ?? ""),
        errorClass: String(item.error_class ?? ""),
        message: String(item.message ?? ""),
        firstSeenUnixSecs: Number(item.first_seen_unix_secs ?? 0),
        lastSeenUnixSecs: Number(item.last_seen_unix_secs ?? 0),
      }));

    return {
      sourceHealth: {
        state: String(sourceHealthRaw?.state ?? ""),
        invalidRatio: Number(sourceHealthRaw?.invalid_ratio ?? 0),
        invalidCount: Number(sourceHealthRaw?.invalid_count ?? 0),
        totalCount: Number(sourceHealthRaw?.total_count ?? 0),
        failureStreak: Number(sourceHealthRaw?.failure_streak ?? 0),
        handshakeFailed: Boolean(sourceHealthRaw?.handshake_failed ?? false),
      },
      quarantined,
    };
  }

  private static checkVersionCompatible(server: string, client: string): boolean {
    try {
      const s = server.split(".").map(Number);
      const c = client.split(".").map(Number);
      if (s[0] === 0 && c[0] === 0) return s[1] === c[1];
      return s[0] === c[0];
    } catch {
      return false;
    }
  }

  static parseRunResult(data: Record<string, unknown>): RunResult {
    const usageRaw = data.usage as Record<string, unknown> | undefined;
    const usage: Usage = {
      inputTokens: Number(usageRaw?.input_tokens ?? 0),
      outputTokens: Number(usageRaw?.output_tokens ?? 0),
      cacheCreationTokens: usageRaw?.cache_creation_tokens != null
        ? Number(usageRaw.cache_creation_tokens)
        : undefined,
      cacheReadTokens: usageRaw?.cache_read_tokens != null
        ? Number(usageRaw.cache_read_tokens)
        : undefined,
    };

    const rawWarnings = data.schema_warnings as Array<Record<string, unknown>> | undefined;
    const schemaWarnings: SchemaWarning[] | undefined = rawWarnings?.map(
      (w): SchemaWarning => ({
        provider: String(w.provider ?? ""),
        path: String(w.path ?? ""),
        message: String(w.message ?? ""),
      }),
    );

    return {
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      text: String(data.text ?? ""),
      turns: Number(data.turns ?? 0),
      toolCalls: Number(data.tool_calls ?? 0),
      usage,
      structuredOutput: data.structured_output,
      schemaWarnings,
      skillDiagnostics: MeerkatClient.parseSkillDiagnostics(data.skill_diagnostics),
    };
  }

  private static parseMcpLiveOpResponse(
    raw: Record<string, unknown>,
  ): McpLiveOpResponse {
    const sessionId = raw.session_id;
    const operation = raw.operation;
    const status = raw.status;
    const persisted = raw.persisted;
    if (typeof sessionId !== "string" || sessionId.length === 0) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mcp response: missing session_id",
      );
    }
    if (operation !== "add" && operation !== "remove" && operation !== "reload") {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mcp response: invalid operation",
      );
    }
    if (typeof status !== "string" || status.length === 0) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mcp response: missing status",
      );
    }
    if (typeof persisted !== "boolean") {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mcp response: persisted must be boolean",
      );
    }
    return raw as unknown as McpLiveOpResponse;
  }

  private handleCallbackRequest(data: Record<string, unknown>): void {
    const requestId = data.id;
    const method = String(data.method ?? "");
    const params = (data.params ?? {}) as Record<string, unknown>;

    if (method === "tool/execute") {
      const toolName = String(params.name ?? "");
      const handler = this.toolHandlers.get(toolName);

      if (handler) {
        const args = (params.arguments ?? {}) as Record<string, unknown>;
        handler
          .handler(args)
          .then((content) => {
            this.writeCallbackResponse(requestId, { content, is_error: false });
          })
          .catch((err: unknown) => {
            this.writeCallbackResponse(requestId, {
              content: `Tool error: ${err}`,
              is_error: true,
            });
          });
      } else {
        this.writeCallbackResponse(requestId, {
          content: `Unknown tool: ${toolName}`,
          is_error: true,
        });
      }
    } else {
      // Unknown callback method.
      const response = {
        jsonrpc: "2.0",
        id: requestId,
        error: { code: -32601, message: `Method not supported: ${method}` },
      };
      this.process?.stdin?.write(JSON.stringify(response) + "\n");
    }
  }

  private writeCallbackResponse(
    requestId: unknown,
    result: { content: string; is_error: boolean },
  ): void {
    const response = { jsonrpc: "2.0", id: requestId, result };
    this.process?.stdin?.write(JSON.stringify(response) + "\n");
  }

  private static buildCreateParams(
    prompt: string,
    options?: SessionOptions,
  ): Record<string, unknown> {
    const params: Record<string, unknown> = { prompt };
    if (!options) return params;

    if (options.model) params.model = options.model;
    if (options.provider) params.provider = options.provider;
    if (options.systemPrompt) params.system_prompt = options.systemPrompt;
    if (options.maxTokens) params.max_tokens = options.maxTokens;
    if (options.outputSchema) params.output_schema = options.outputSchema;
    if (options.structuredOutputRetries != null && options.structuredOutputRetries !== 2) {
      params.structured_output_retries = options.structuredOutputRetries;
    }
    if (options.hooksOverride) params.hooks_override = options.hooksOverride;
    if (options.enableBuiltins) params.enable_builtins = true;
    if (options.enableShell) params.enable_shell = true;
    if (options.enableSubagents) params.enable_subagents = true;
    if (options.enableMemory) params.enable_memory = true;
    if (options.enableMob) params.enable_mob = true;
    if (options.hostMode) params.host_mode = true;
    if (options.commsName) params.comms_name = options.commsName;
    if (options.peerMeta != null) params.peer_meta = options.peerMeta;
    if (options.budgetLimits != null) params.budget_limits = options.budgetLimits;
    if (options.providerParams) params.provider_params = options.providerParams;
    if (options.preloadSkills != null) params.preload_skills = options.preloadSkills;
    const wireRefs = skillRefsToWire(options.skillRefs);
    if (wireRefs) params.skill_refs = wireRefs;
    if (options.skillReferences != null) params.skill_references = options.skillReferences;
    return params;
  }

  // -- Binary resolution --------------------------------------------------

  private static commandPath(command: string): string | null {
    if (path.isAbsolute(command)) {
      return existsSync(command) ? command : null;
    }
    const pathEnv = process.env.PATH ?? "";
    if (!pathEnv) return null;
    const exts =
      process.platform === "win32"
        ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM")
            .split(";")
            .filter((ext: string) => ext.length > 0)
        : [""];
    for (const dir of pathEnv.split(path.delimiter)) {
      if (!dir) continue;
      for (const ext of exts) {
        const candidate =
          process.platform === "win32" &&
          ext &&
          !command.toLowerCase().endsWith(ext.toLowerCase())
            ? `${command}${ext}`
            : command;
        const fullPath = path.join(dir, candidate);
        if (existsSync(fullPath)) return fullPath;
      }
    }
    return null;
  }

  private static resolveCandidatePath(commandOrPath: string): string | null {
    if (commandOrPath.includes(path.sep) || commandOrPath.includes("/")) {
      const candidate = path.resolve(commandOrPath);
      return existsSync(candidate) ? candidate : null;
    }
    return MeerkatClient.commandPath(commandOrPath);
  }

  private static platformTarget(): PlatformTarget {
    const architecture = os.arch();
    if (process.platform === "darwin") {
      if (architecture === "arm64") {
        return { target: "aarch64-apple-darwin", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      }
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported macOS architecture '${architecture}'.`);
    }
    if (process.platform === "linux") {
      if (architecture === "x64") return { target: "x86_64-unknown-linux-gnu", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      if (architecture === "arm64") return { target: "aarch64-unknown-linux-gnu", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported Linux architecture '${architecture}'.`);
    }
    if (process.platform === "win32") {
      if (architecture === "x64") return { target: "x86_64-pc-windows-msvc", archiveExt: "zip", binaryName: "rkat-rpc.exe" };
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported Windows architecture '${architecture}'.`);
    }
    throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported platform '${process.platform}'.`);
  }

  private static async runCommand(command: string, args: string[]): Promise<void> {
    return new Promise((resolve, reject) => {
      const proc = spawn(command, args, { stdio: ["ignore", "inherit", "pipe"] });
      let stderr = "";
      proc.stderr?.on("data", (chunk) => { stderr += String(chunk); });
      proc.on("error", (error) => { reject(error); });
      proc.on("close", (code) => {
        if (code === 0) { resolve(); return; }
        reject(new MeerkatError("ARCHIVE_EXTRACT_FAILED", `${command} failed with code ${code}: ${stderr.trim()}`));
      });
    });
  }

  private static async extractZip(archivePath: string, destinationDir: string): Promise<void> {
    try {
      await MeerkatClient.runCommand("tar", ["-xf", archivePath, "-C", destinationDir]);
    } catch {
      await MeerkatClient.runCommand("powershell", [
        "-NoProfile", "-Command",
        `Expand-Archive -LiteralPath '${archivePath.replaceAll("'", "''")}' -DestinationPath '${destinationDir.replaceAll("'", "''")}' -Force`,
      ]);
    }
  }

  private static async ensureDownloadedBinary(): Promise<string> {
    const { target, archiveExt, binaryName } = MeerkatClient.platformTarget();
    const version = CONTRACT_VERSION;
    const asset = `${MEERKAT_RELEASE_BINARY}-v${version}-${target}.${archiveExt}`;
    const url = `https://github.com/${MEERKAT_REPO}/releases/download/v${version}/${asset}`;
    const baseDir = path.join(MEERKAT_BINARY_CACHE_ROOT, `v${version}`, target);
    mkdirSync(baseDir, { recursive: true });
    const cached = path.join(baseDir, binaryName);
    if (existsSync(cached)) return cached;

    const response = await fetch(url);
    if (!response.ok) {
      throw new MeerkatError("BINARY_DOWNLOAD_FAILED", `Failed to download binary from ${url} (HTTP ${response.status})`);
    }
    const arrayBuffer = await response.arrayBuffer();
    const archivePath = path.join(baseDir, asset);
    writeFileSync(archivePath, Buffer.from(arrayBuffer));

    try {
      if (archiveExt === "tar.gz") {
        await MeerkatClient.runCommand("tar", ["-xzf", archivePath, "-C", baseDir]);
      } else {
        await MeerkatClient.extractZip(archivePath, baseDir);
      }
    } catch (error) {
      throw new MeerkatError("BINARY_DOWNLOAD_EXTRACT_FAILED", `Failed to extract ${archivePath}: ${String(error)}`);
    } finally {
      try { unlinkSync(archivePath); } catch { /* best-effort cleanup */ }
    }

    if (process.platform !== "win32") chmodSync(cached, 0o755);
    return cached;
  }

  private static async resolveBinaryPath(requestedPath: string): Promise<ResolvedBinary> {
    const overridden = process.env.MEERKAT_BIN_PATH?.trim();
    if (overridden) {
      const candidate = MeerkatClient.resolveCandidatePath(overridden);
      if (!candidate) {
        throw new MeerkatError("BINARY_NOT_FOUND", `Binary not found at MEERKAT_BIN_PATH='${overridden}'.`);
      }
      return { command: candidate, useLegacySubcommand: path.basename(candidate) === "rkat" };
    }

    if (requestedPath !== "rkat-rpc") {
      const candidate = MeerkatClient.resolveCandidatePath(requestedPath);
      if (!candidate) {
        throw new MeerkatError("BINARY_NOT_FOUND", `Binary not found at '${requestedPath}'.`);
      }
      return { command: candidate, useLegacySubcommand: path.basename(candidate) === "rkat" };
    }

    const defaultBinary = MeerkatClient.commandPath("rkat-rpc");
    if (defaultBinary) return { command: defaultBinary, useLegacySubcommand: false };

    try {
      const downloaded = await MeerkatClient.ensureDownloadedBinary();
      return { command: downloaded, useLegacySubcommand: false };
    } catch (error) {
      const legacy = MeerkatClient.commandPath("rkat");
      if (legacy) return { command: legacy, useLegacySubcommand: true };
      if (error instanceof MeerkatError) throw error;
      throw new MeerkatError("BINARY_NOT_FOUND", `Could not find '${MEERKAT_RELEASE_BINARY}' and auto-download failed.`);
    }
  }

  private static buildArgs(legacy: boolean, options?: ConnectOptions): string[] {
    if (legacy) return ["rpc"];
    if (!options) return [];
    const args: string[] = [];
    if (options.isolated) args.push("--isolated");
    if (options.realmId) args.push("--realm", options.realmId);
    if (options.instanceId) args.push("--instance", options.instanceId);
    if (options.realmBackend) args.push("--realm-backend", options.realmBackend);
    if (options.stateRoot) args.push("--state-root", options.stateRoot);
    if (options.contextRoot) args.push("--context-root", options.contextRoot);
    if (options.userConfigRoot) args.push("--user-config-root", options.userConfigRoot);
    return args;
  }
}
