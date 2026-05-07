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
import type { SpawnOptions } from "node:child_process";
import { once } from "node:events";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";
import { Buffer } from "node:buffer";
import { MeerkatError, CapabilityUnavailableError } from "./generated/errors.js";
import {
  CONTRACT_VERSION,
  type RealtimeCapabilitiesResult,
  type RealtimeOpenInfo,
  type RealtimeOpenRequest,
  type RealtimeStatusResult,
  type MobTurnStartParams,
  type MobRotateSupervisorResult,
  type RuntimeRealtimeAttachmentStatusResult,
  type McpAddParams,
  type McpLiveOpResponse,
  type McpReloadParams,
  type McpRemoveParams,
  type MobSpawnManyFailureCause,
  type MobSpawnManyResultEntry,
} from "./generated/types.js";
import { DeferredSession, Session } from "./session.js";
import {
  Mob,
  type MemberDeliveryReceipt,
  type MemberSendOptions,
  type MobKickoffMemberSnapshot,
  type MobKickoffWaitOptions,
  type MobReadyMemberSnapshot,
  type MobReadyWaitOptions,
  type MobPeerTarget,
} from "./mob.js";
import { parseCoreEvent } from "./events.js";
import { EventStream, AsyncQueue } from "./streaming.js";
import { EventSubscription } from "./subscription.js";
import type {
  AgentEventEnvelope,
  AttributedMobEvent,
  BlobPayload,
  Capability,
  CommsCommand,
  CommsSendReceipt,
  ConfigEnvelope,
  ContentInput,
  ContentBlock,
  CreateScheduleRequest,
  EventSourceIdentity,
  HelpOptions,
  ModelsCatalog,
  MobEventsOptions,
  MobEventsResult,
  MobCreateOptions,
  MobFlowStatus,
  MobLifecycleAction,
  MobMember,
  MobMemberRef,
  MobProfile,
  MobProfileDeleteResult,
  MobProfileLookupResult,
  MobSpawnResult,
  MobStatus,
  MobSummary,
  MobTurnStartOptions,
  PeerCorrelationId,
  PeerId,
  PeerResponseTerminalOptions,
  RunResult,
  Schedule,
  ScheduleListOptions,
  ScheduleOccurrencesOptions,
  ScheduleOccurrencesResult,
  ScheduleToolCallRequest,
  ScheduleToolsResult,
  SchemaWarning,
  SessionAssistantBlock,
  SessionForkResult,
  SessionHistory,
  SessionIngressOptions,
  SessionInfo,
  SessionListOptions,
  SessionMessage,
  SessionOptions,
  SessionToolCall,
  SessionToolResult,
  SkillKey,
  SkillRef,
  SkillRuntimeDiagnostics,
  SpawnManySpec,
  SpawnSpec,
  TranscriptEditOptions,
  TranscriptReplacement,
  UpdateScheduleRequest,
  TurnOptions,
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
const MOB_SPAWN_MANY_FAILURE_CAUSES = new Set<string>([
  "profile_not_found",
  "member_not_found",
  "member_already_exists",
  "not_externally_addressable",
  "invalid_transition",
  "wiring_error",
  "bridge_command_rejected",
  "member_restore_failed",
  "kickoff_wait_timed_out",
  "ready_wait_timed_out",
  "definition_error",
  "flow_not_found",
  "flow_failed",
  "run_not_found",
  "run_canceled",
  "flow_turn_timed_out",
  "frame_depth_limit_exceeded",
  "frame_atomic_persistence_unavailable",
  "spec_revision_conflict",
  "schema_validation",
  "insufficient_targets",
  "topology_violation",
  "bridge_delivery_rejected",
  "supervisor_escalation",
  "unsupported_for_mode",
  "reset_barrier",
  "storage_error",
  "session_error",
  "comms_error",
  "callback_pending",
  "stale_fence_token",
  "stale_event_cursor",
  "work_not_found",
  "internal",
]);

function isMobSpawnManyFailureCause(value: unknown): value is MobSpawnManyFailureCause {
  return typeof value === "string" && MOB_SPAWN_MANY_FAILURE_CAUSES.has(value);
}

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
  realmBackend?: "jsonl" | "sqlite";
  isolated?: boolean;
  stateRoot?: string;
  contextRoot?: string;
  userConfigRoot?: string;
}

interface WireSkillKey {
  [key: string]: string;
  source_uuid: string;
  skill_name: string;
}

interface WireSkillRef extends WireSkillKey {
  kind: "structured";
}

/**
 * Normalize a structured SkillRef to the bare SkillKey wire format.
 */
function normalizeSkillKey(ref: SkillRef): WireSkillKey {
  if (
    ref === null ||
    typeof ref !== "object" ||
    typeof ref.sourceUuid !== "string" ||
    typeof ref.skillName !== "string"
  ) {
    throw new Error("Skill references must be SkillKey objects");
  }
  return { source_uuid: ref.sourceUuid, skill_name: ref.skillName };
}

function skillKeysToWire(refs: SkillRef[] | undefined): WireSkillKey[] | undefined {
  if (!refs) return undefined;
  return refs.map(normalizeSkillKey);
}

function skillRefsToWire(refs: SkillRef[] | undefined): WireSkillRef[] | undefined {
  const keys = skillKeysToWire(refs);
  return keys?.map((key) => ({ kind: "structured", ...key }));
}

function setIfDefined<T extends object, K extends keyof T>(
  payload: T,
  key: K,
  value: T[K] | undefined,
): void {
  if (value !== undefined) {
    payload[key] = value;
  }
}

function mobSpawnPayload(mobId: string, spec: SpawnSpec): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    mob_id: mobId,
    profile: spec.profile,
    agent_identity: spec.agentIdentity,
  };
  setIfDefined(payload, "initial_message", spec.initialMessage);
  setIfDefined(payload, "runtime_mode", spec.runtimeMode);
  setIfDefined(payload, "backend", spec.backend);
  setIfDefined(payload, "labels", spec.labels);
  setIfDefined(payload, "context", spec.context);
  setIfDefined(payload, "additional_instructions", spec.additionalInstructions);
  setIfDefined(payload, "binding", spec.binding);
  setIfDefined(payload, "shell_env", spec.shellEnv);
  setIfDefined(payload, "auto_wire_parent", spec.autoWireParent);
  setIfDefined(payload, "launch_mode", spec.launchMode);
  setIfDefined(payload, "tool_access_policy", spec.toolAccessPolicy);
  setIfDefined(payload, "budget_split_policy", spec.budgetSplitPolicy);
  setIfDefined(payload, "inherited_tool_filter", spec.inheritedToolFilter);
  setIfDefined(payload, "override_profile", spec.overrideProfile);
  setIfDefined(payload, "auth_binding", spec.authBinding);
  return payload;
}

function mobSpawnManySpecPayload(spec: SpawnManySpec): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    profile: spec.profile,
    agent_identity: spec.agentIdentity,
  };
  setIfDefined(payload, "initial_message", spec.initialMessage);
  setIfDefined(payload, "runtime_mode", spec.runtimeMode);
  setIfDefined(payload, "backend", spec.backend);
  setIfDefined(payload, "labels", spec.labels);
  setIfDefined(payload, "context", spec.context);
  setIfDefined(payload, "additional_instructions", spec.additionalInstructions);
  setIfDefined(payload, "auth_binding", spec.authBinding);
  return payload;
}

function mobTurnStartPayload(
  mobId: string,
  agentIdentity: string,
  prompt: ContentInput,
  options?: MobTurnStartOptions,
): MobTurnStartParams {
  const payload: MobTurnStartParams = {
    mob_id: mobId,
    agent_identity: agentIdentity,
    prompt: typeof prompt === "string"
      ? prompt
      : prompt.map((block) => ({ ...block })) as MobTurnStartParams["prompt"],
  };
  const wireRefs = skillRefsToWire(options?.skillRefs);
  if (wireRefs) {
    payload.skill_refs = wireRefs as MobTurnStartParams["skill_refs"];
  }
  if (options?.flowToolOverlay) {
    payload.flow_tool_overlay = {
      allowed_tools: options.flowToolOverlay.allowedTools,
      blocked_tools: options.flowToolOverlay.blockedTools,
    } as MobTurnStartParams["flow_tool_overlay"];
  }
  setIfDefined(payload, "additional_instructions", options?.additionalInstructions);
  setIfDefined(payload, "keep_alive", options?.keepAlive);
  setIfDefined(payload, "model", options?.model);
  setIfDefined(payload, "provider", options?.provider);
  setIfDefined(payload, "max_tokens", options?.maxTokens);
  setIfDefined(payload, "system_prompt", options?.systemPrompt);
  setIfDefined(payload, "output_schema", options?.outputSchema);
  setIfDefined(payload, "structured_output_retries", options?.structuredOutputRetries);
  setIfDefined(payload, "provider_params", options?.providerParams);
  setIfDefined(payload, "clear_provider_params", options?.clearProviderParams);
  setIfDefined(payload, "auth_binding", options?.authBinding);
  setIfDefined(payload, "clear_auth_binding", options?.clearAuthBinding);
  return payload;
}

export class MeerkatClient {
  private process: ChildProcess | null = null;
  private processStderr = "";
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
    const child = this.process;
    this.processStderr = "";
    child.stderr?.on("data", (chunk: string | Buffer) => {
      this.processStderr = (this.processStderr + String(chunk)).slice(-8192);
    });
    child.once("error", (error) => {
      if (this.process === child) {
        this.process = null;
        this.rejectPendingRequests(
          new MeerkatError("PROCESS_ERROR", `Failed to start ${this.rkatPath}: ${error.message}`),
        );
        this.closeQueues();
      }
    });
    child.once("close", (code, signal) => {
      const expectedClose = this.process !== child;
      if (this.process === child) {
        this.process = null;
        this.rl?.close();
        this.rl = null;
      }
      if (!expectedClose) {
        const suffix = this.processStderr.trim() ? `: ${this.processStderr.trim()}` : "";
        this.rejectPendingRequests(
          new MeerkatError(
            "PROCESS_EXITED",
            `${this.rkatPath} exited before replying (code ${code ?? "null"}, signal ${signal ?? "null"})${suffix}`,
          ),
        );
        this.closeQueues();
      }
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
    const process = this.process;
    this.process = null;
    if (process) {
      process.stdin?.end();
      const closed = once(process, "close").catch(() => []);
      process.kill();
      const timeout = delay(5000).then(() => "timeout");
      const outcome = await Promise.race([closed, timeout]);
      if (outcome === "timeout" && process.exitCode == null && process.signalCode == null) {
        process.kill("SIGKILL");
        await closed.catch(() => {});
      }
      process.stdin?.destroy();
      process.stdout?.destroy();
      process.stderr?.destroy();
    }
    this.rejectPendingRequests(new MeerkatError("CLIENT_CLOSED", "Client closed"));
    this.closeQueues();
  }

  private rejectPendingRequests(reason: unknown): void {
    for (const [, pending] of this.pendingRequests) {
      pending.reject(reason);
    }
    this.pendingRequests.clear();
  }

  private closeQueues(): void {
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
    prompt: string | ContentBlock[],
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
    prompt: string | ContentBlock[],
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
    prompt: string | ContentBlock[],
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

  async askHelp(question: string, options?: HelpOptions): Promise<RunResult> {
    const params: Record<string, unknown> = { question };
    if (options?.prompt !== undefined) params.prompt = options.prompt;
    if (options?.executionMode !== undefined) {
      params.execution_mode = options.executionMode;
    }
    if (options?.model !== undefined) params.model = options.model;
    if (options?.provider !== undefined) params.provider = options.provider;
    if (options?.maxTokens !== undefined) params.max_tokens = options.maxTokens;
    const raw = await this.request("help/ask", params);
    return MeerkatClient.parseRunResult(raw);
  }

  // -- Session queries ----------------------------------------------------

  async listSessions(options?: SessionListOptions): Promise<SessionInfo[]> {
    const params: Record<string, unknown> = {};
    if (options?.labels) params.labels = options.labels;
    if (options?.limit !== undefined) params.limit = options.limit;
    if (options?.offset !== undefined) params.offset = options.offset;
    const result = await this.request("session/list", params);
    const sessions = (result.sessions as Array<Record<string, unknown>>) ?? [];
    return sessions.map((s) => MeerkatClient.parseSessionInfo(s));
  }

  async readSession(sessionId: string): Promise<SessionInfo> {
    const raw = await this.request("session/read", { session_id: sessionId });
    return MeerkatClient.parseSessionInfo(raw);
  }

  async sendExternalEvent(
    sessionId: string,
    eventType: string,
    payload: unknown,
    options?: { blocks?: ContentBlock[] },
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = {
      session_id: sessionId,
      kind: "generic_json",
      event_type: eventType,
      payload,
    };
    if (options?.blocks !== undefined) {
      params.blocks = options.blocks;
    }
    return this.request("session/external_event", params);
  }

  async sendPeerResponseTerminal(
    sessionId: string,
    peerId: PeerId,
    requestId: PeerCorrelationId,
    status: "completed" | "failed" | "cancelled",
    result: unknown,
    options?: PeerResponseTerminalOptions,
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = {
      session_id: sessionId,
      peer_id: peerId,
      request_id: requestId,
      status,
      result,
    };
    if (options?.displayName !== undefined) {
      params.display_name = options.displayName;
    }
    return this.request("session/peer_response_terminal", params);
  }

  async injectContext(
    sessionId: string,
    text: string,
    options?: SessionIngressOptions,
  ): Promise<{ status: string }> {
    const params: Record<string, unknown> = { session_id: sessionId, text };
    if (options?.source !== undefined) {
      params.source = options.source;
    }
    if (options?.idempotencyKey !== undefined) {
      params.idempotency_key = options.idempotencyKey;
    }
    const result = await this.request("session/inject_context", params);
    return { status: String(result.status ?? "") };
  }

  async readSessionHistory(
    sessionId: string,
    options?: { offset?: number; limit?: number },
  ): Promise<SessionHistory> {
    const params: Record<string, unknown> = {
      session_id: sessionId,
      offset: options?.offset ?? 0,
    };
    if (options?.limit !== undefined) {
      params.limit = options.limit;
    }
    const raw = await this.request("session/history", params);
    return MeerkatClient.parseSessionHistory(raw);
  }

  async forkSessionAt(
    sessionId: string,
    messageIndex: number,
    options?: TranscriptEditOptions,
  ): Promise<SessionForkResult> {
    const params: Record<string, unknown> = {
      session_id: sessionId,
      message_index: messageIndex,
    };
    if (options?.runningBehavior !== undefined) {
      params.running_behavior = options.runningBehavior;
    }
    const raw = await this.request("session/fork_at", params);
    return MeerkatClient.parseSessionForkResult(raw);
  }

  async forkSessionReplace(
    sessionId: string,
    messageIndex: number,
    replacement: TranscriptReplacement,
    options?: TranscriptEditOptions,
  ): Promise<SessionForkResult> {
    const params: Record<string, unknown> = {
      session_id: sessionId,
      message_index: messageIndex,
      replacement: MeerkatClient.serializeTranscriptReplacement(replacement),
    };
    if (options?.runningBehavior !== undefined) {
      params.running_behavior = options.runningBehavior;
    }
    const raw = await this.request("session/fork_replace", params);
    return MeerkatClient.parseSessionForkResult(raw);
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

  async getConfig(): Promise<ConfigEnvelope> {
    const raw = await this.request("config/get", {});
    return MeerkatClient.parseConfigEnvelope(raw);
  }

  async setConfig(
    config: Record<string, unknown>,
    options?: { expectedGeneration?: number },
  ): Promise<ConfigEnvelope> {
    const params: Record<string, unknown> = { config };
    if (options?.expectedGeneration !== undefined) {
      params.expected_generation = options.expectedGeneration;
    }
    const raw = await this.request("config/set", params);
    return MeerkatClient.parseConfigEnvelope(raw);
  }

  async patchConfig(
    patch: Record<string, unknown>,
    options?: { expectedGeneration?: number },
  ): Promise<ConfigEnvelope> {
    const params: Record<string, unknown> = { patch };
    if (options?.expectedGeneration !== undefined) {
      params.expected_generation = options.expectedGeneration;
    }
    const raw = await this.request("config/patch", params);
    return MeerkatClient.parseConfigEnvelope(raw);
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

  async getBlob(blobId: string): Promise<BlobPayload> {
    const result = await this.request("blob/get", { blob_id: blobId });
    return {
      blobId: String(result.blob_id ?? blobId),
      mediaType: String(result.media_type ?? ""),
      dataBase64: String(result.data ?? ""),
    };
  }

  async getRuntimeHostInfo(): Promise<Record<string, unknown>> {
    return this.request("runtime/host_info", {});
  }

  async getRuntimeHostCapabilities(): Promise<Record<string, unknown>> {
    return this.request("runtime/capabilities", {});
  }

  async getRuntimeHostHealth(): Promise<Record<string, unknown>> {
    return this.request("runtime/health", {});
  }

  async requestApproval(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("approval/request", params);
  }

  async listApprovals(
    params: Record<string, unknown> = {},
  ): Promise<Record<string, unknown>> {
    return this.request("approval/list", params);
  }

  async getApproval(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("approval/get", params);
  }

  async decideApproval(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("approval/decide", params);
  }

  async listArtifacts(
    params: Record<string, unknown> = {},
  ): Promise<Record<string, unknown>> {
    return this.request("artifact/list", params);
  }

  async getArtifact(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("artifact/get", params);
  }

  async downloadArtifact(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("artifact/download", params);
  }

  async latestEventCursor(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("events/latest_cursor", params);
  }

  async listEventsSince(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("events/list_since", params);
  }

  async eventSnapshot(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("events/snapshot", params);
  }

  async getModelsCatalog(): Promise<ModelsCatalog> {
    const result = await this.request("models/catalog", {});
    return MeerkatClient.parseModelsCatalog(result);
  }

  async createSchedule(request: CreateScheduleRequest): Promise<Schedule> {
    const result = await this.request("schedule/create", MeerkatClient.toWireCreateScheduleRequest(request));
    return MeerkatClient.parseSchedule(result);
  }

  async getSchedule(scheduleId: string): Promise<Schedule> {
    const result = await this.request("schedule/get", { schedule_id: scheduleId });
    return MeerkatClient.parseSchedule(result);
  }

  async listSchedules(_options?: ScheduleListOptions): Promise<Schedule[]> {
    const params: Record<string, unknown> = {};
    if (_options?.labels) params.labels = _options.labels;
    if (_options?.limit !== undefined) params.limit = _options.limit;
    if (_options?.offset !== undefined) params.offset = _options.offset;
    const result = await this.request("schedule/list", params);
    const schedules = Array.isArray(result.schedules)
      ? (result.schedules as Array<Record<string, unknown>>)
      : [];
    return schedules.map((schedule) => MeerkatClient.parseSchedule(schedule));
  }

  async updateSchedule(request: UpdateScheduleRequest): Promise<Schedule> {
    const params = {
      schedule_id: request.scheduleId,
      ...MeerkatClient.toWireUpdateSchedulePatch(request.update),
    };
    const result = await this.request("schedule/update", params);
    return MeerkatClient.parseSchedule(result);
  }

  async pauseSchedule(scheduleId: string): Promise<Schedule> {
    const result = await this.request("schedule/pause", { schedule_id: scheduleId });
    return MeerkatClient.parseSchedule(result);
  }

  async resumeSchedule(scheduleId: string): Promise<Schedule> {
    const result = await this.request("schedule/resume", { schedule_id: scheduleId });
    return MeerkatClient.parseSchedule(result);
  }

  async deleteSchedule(scheduleId: string): Promise<Schedule> {
    const result = await this.request("schedule/delete", { schedule_id: scheduleId });
    return MeerkatClient.parseSchedule(result);
  }

  async listScheduleOccurrences(
    scheduleId: string,
    options?: ScheduleOccurrencesOptions,
  ): Promise<ScheduleOccurrencesResult> {
    const params: Record<string, unknown> = { schedule_id: scheduleId };
    if (options?.includeTerminal !== undefined) {
      params.include_terminal = options.includeTerminal;
    }
    const result = await this.request("schedule/occurrences", params);
    const occurrences = Array.isArray(result.occurrences)
      ? (result.occurrences as Array<Record<string, unknown>>).map(
          (occurrence) => MeerkatClient.parseScheduleOccurrence(occurrence),
        )
      : [];
    return { occurrences };
  }

  async listScheduleTools(): Promise<ScheduleToolsResult> {
    const result = await this.request("schedule/tools", {});
    const tools = Array.isArray(result.tools)
      ? (result.tools as Array<Record<string, unknown>>)
      : [];
    return { tools };
  }

  async callScheduleTool(request: ScheduleToolCallRequest): Promise<Record<string, unknown>> {
    return this.request("schedule/call", {
      name: request.name,
      arguments: request.arguments ?? {},
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
        ? Object.keys(rawStatus)[0]
        : undefined);
    if (!status) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/status response: missing status",
      );
    }
    return { mobId: String(result.mob_id ?? mobId), status };
  }

  async listMobMembers(mobId: string): Promise<MobMember[]> {
    const result = await this.request("mob/members", { mob_id: mobId });
    const members = (result.members as Array<Record<string, unknown>>) ?? [];
    return members.map((member) => {
      const agentIdentity = String(member.agent_identity ?? "");
      const memberRef =
        typeof member.member_ref === "string" && member.member_ref.length > 0
          ? member.member_ref
          : undefined;
      if (!agentIdentity || !memberRef) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/members response: missing identity-native member fields",
        );
      }
      return {
        agentIdentity,
        memberRef,
        profile: String(member.profile_name ?? member.profile ?? member.role ?? ""),
        peerId: member.peer_id != null ? String(member.peer_id) : undefined,
        externalPeerSpecs:
          member.external_peer_specs && typeof member.external_peer_specs === "object"
            ? Object.fromEntries(
                Object.entries(member.external_peer_specs as Record<string, unknown>).map(
                  ([key, value]) => [key, (value ?? {}) as Record<string, unknown>],
                ),
              )
            : undefined,
        runtimeMode: member.runtime_mode != null ? String(member.runtime_mode) : undefined,
        state: member.state != null ? String(member.state) : undefined,
        wiredTo: Array.isArray(member.wired_to)
          ? member.wired_to.map((peer) => String(peer))
          : undefined,
        labels: member.labels && typeof member.labels === "object"
          ? Object.fromEntries(
              Object.entries(member.labels as Record<string, unknown>).map(([key, value]) => [key, String(value)]),
            )
          : undefined,
        status: member.status != null ? String(member.status) : undefined,
        error: member.error != null ? String(member.error) : undefined,
        isFinal: member.is_final != null ? Boolean(member.is_final) : undefined,
      };
    });
  }

  async sendMobMemberContent(
    mobId: string,
    agentIdentity: string,
    content: string | ContentBlock[],
    options?: MemberSendOptions,
  ): Promise<MemberDeliveryReceipt> {
    const result = await this.request("mob/member_send", {
      mob_id: mobId,
      agent_identity: agentIdentity,
      content,
      handling_mode: options?.handlingMode,
      render_metadata: options?.renderMetadata,
    });
    const memberRef =
      typeof result.member_ref === "string" && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/member_send response: missing member_ref",
      );
    }
    return {
      agentIdentity:
        typeof result.agent_identity === "string" && result.agent_identity.length > 0
          ? result.agent_identity
          : agentIdentity,
      memberRef,
      handlingMode:
        result.handling_mode === "steer" || result.handling_mode === "queue"
          ? result.handling_mode
          : (options?.handlingMode ?? "queue"),
    };
  }

  async spawnMobMember(
    mobId: string,
    options: SpawnSpec,
  ): Promise<MobSpawnResult> {
    const result = await this.request("mob/spawn", mobSpawnPayload(mobId, options));
    const memberRef =
      typeof result.member_ref === "string" && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/spawn response: missing member_ref",
      );
    }
    return {
      mobId: String(result.mob_id ?? mobId),
      agentIdentity:
        typeof result.agent_identity === "string" && result.agent_identity.length > 0
          ? result.agent_identity
          : options.agentIdentity,
      memberRef,
    };
  }

  async spawnMobMembers(
    mobId: string,
    specs: SpawnManySpec[],
  ): Promise<MobSpawnManyResultEntry[]> {
    const result = await this.request("mob/spawn_many", {
      mob_id: mobId,
      specs: specs.map(mobSpawnManySpecPayload),
    });
    if (!Array.isArray(result.results)) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/spawn_many response: results must be a list",
      );
    }
    const entries = result.results as unknown[];
    return entries.map((entry) => {
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: malformed result entry",
        );
      }
      const record = entry as Record<string, unknown>;
      const entryKeys = Object.keys(record);
      if (
        entryKeys.some((key) => key !== "status" && key !== "result") ||
        "ok" in record ||
        "error" in record ||
        "agent_identity" in record ||
        "member_ref" in record
      ) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: legacy result carrier fields are not allowed",
        );
      }
      const status = record.status;
      const rawResult = record.result;
      if (status !== "spawned" && status !== "failed") {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: invalid result status",
        );
      }
      if (!rawResult || typeof rawResult !== "object" || Array.isArray(rawResult)) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: missing result payload",
        );
      }
      const resultRecord = rawResult as Record<string, unknown>;
      if (status === "spawned") {
        if (Object.keys(resultRecord).some((key) => key !== "agent_identity" && key !== "member_ref")) {
          throw new MeerkatError(
            "INVALID_RESPONSE",
            "Invalid mob/spawn_many response: spawned result has unknown fields",
          );
        }
        const agentIdentity = resultRecord.agent_identity;
        const memberRef = resultRecord.member_ref;
        if (typeof agentIdentity !== "string" || agentIdentity.length === 0) {
          throw new MeerkatError(
            "INVALID_RESPONSE",
            "Invalid mob/spawn_many response: spawned result missing agent_identity",
          );
        }
        if (typeof memberRef !== "string" || memberRef.length === 0) {
          throw new MeerkatError(
            "INVALID_RESPONSE",
            "Invalid mob/spawn_many response: spawned result missing member_ref",
          );
        }
        return {
          status,
          result: {
            agent_identity: agentIdentity,
            member_ref: memberRef,
          },
        };
      }
      if (Object.keys(resultRecord).some((key) => key !== "cause" && key !== "message")) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: failed result has unknown fields",
        );
      }
      const cause = resultRecord.cause;
      if (!isMobSpawnManyFailureCause(cause)) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: failed result has invalid cause",
        );
      }
      const message = resultRecord.message;
      if (typeof message !== "string" || message.length === 0) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/spawn_many response: failed result missing message",
        );
      }
      return {
        status,
        result: {
          cause,
          message,
        },
      };
    });
  }

  async retireMobMember(mobId: string, agentIdentity: string): Promise<void> {
    await this.request("mob/retire", {
      mob_id: mobId,
      agent_identity: agentIdentity,
    });
  }

  async respawnMobMember(
    mobId: string,
    agentIdentity: string,
    initialMessage?: string | ContentBlock[],
  ): Promise<{
    status: "completed" | "topology_restore_failed";
    receipt: {
      agentIdentity: string;
      memberRef: MobMemberRef;
    };
    failedPeerIds?: string[];
  }> {
    const result = await this.request("mob/respawn", {
      mob_id: mobId,
      agent_identity: agentIdentity,
      initial_message: initialMessage,
    });
    const status = String(result.status ?? "completed");
    const rawFailed = Array.isArray(result.failed_peer_ids)
      ? result.failed_peer_ids
      : [];
    const receipt = result.receipt as Record<string, unknown> | undefined;
    if (!receipt || typeof receipt !== "object") {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/respawn response: missing receipt",
      );
    }
    const memberRef =
      typeof receipt.member_ref === "string" && receipt.member_ref.length > 0
        ? receipt.member_ref
        : undefined;
    if (!memberRef) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/respawn response: receipt missing member_ref",
      );
    }
    return {
      status: status === "topology_restore_failed" ? "topology_restore_failed" : "completed",
      receipt: {
        agentIdentity:
          receipt.identity != null ? String(receipt.identity) : agentIdentity,
        memberRef,
      },
      failedPeerIds: rawFailed.map((peerId) => String(peerId)),
    };
  }

  async forceCancelMobMember(mobId: string, agentIdentity: string): Promise<void> {
    await this.request("mob/force_cancel", {
      mob_id: mobId,
      agent_identity: agentIdentity,
    });
  }

  async mobTurnStart(
    mobId: string,
    agentIdentity: string,
    prompt: ContentInput,
    options?: MobTurnStartOptions,
  ): Promise<Record<string, unknown>> {
    return await this.request("mob/turn_start", mobTurnStartPayload(
      mobId,
      agentIdentity,
      prompt,
      options,
    ));
  }

  async mobMemberStatus(
    mobId: string,
    agentIdentity: string,
  ): Promise<{
    status: string;
    outputPreview?: string;
    error?: string;
    tokensUsed: number;
    isFinal: boolean;
    liveAttachmentStatus?:
      | "unattached"
      | "intent_present_unbound"
      | "binding_not_ready"
      | "binding_ready"
      | "replacement_pending"
      | "reattach_required";
    peerConnectivity?: {
      reachablePeerCount: number;
      unknownPeerCount: number;
      unreachablePeers: Array<{ peer: string; reason?: string }>;
    };
    /**
     * Phase 5G/T5i identity-first realtime routing: session id of the
     * member's current bridge session. Consumers navigate
     * `mob/member_status.currentSessionId → realtime/open_info
     * (session_target)`. Absent when the member is not yet bound to a
     * session.
     */
    currentSessionId?: string;
  }> {
    const result = await this.request("mob/member_status", {
      mob_id: mobId,
      agent_identity: agentIdentity,
    });
    const rawConnectivity =
      result.peer_connectivity && typeof result.peer_connectivity === "object"
        ? (result.peer_connectivity as Record<string, unknown>)
        : undefined;
    return {
      status: String(result.status ?? "unknown"),
      outputPreview: result.output_preview != null ? String(result.output_preview) : undefined,
      error: result.error != null ? String(result.error) : undefined,
      tokensUsed: Number(result.tokens_used ?? 0),
      isFinal: Boolean(result.is_final),
      liveAttachmentStatus:
        typeof result.realtime_attachment_status === "string"
          ? (result.realtime_attachment_status as
              | "unattached"
              | "intent_present_unbound"
              | "binding_not_ready"
              | "binding_ready"
              | "replacement_pending"
              | "reattach_required")
          : undefined,
      peerConnectivity: rawConnectivity
        ? {
            reachablePeerCount: Number(rawConnectivity.reachable_peer_count ?? 0),
            unknownPeerCount: Number(rawConnectivity.unknown_peer_count ?? 0),
            unreachablePeers: Array.isArray(rawConnectivity.unreachable_peers)
              ? rawConnectivity.unreachable_peers.map((peer) => {
                  const rawPeer =
                    peer && typeof peer === "object" ? (peer as Record<string, unknown>) : {};
                  return {
                    peer: String(rawPeer.peer ?? ""),
                    reason: rawPeer.reason != null ? String(rawPeer.reason) : undefined,
                  };
                })
              : [],
          }
        : undefined,
      currentSessionId:
        typeof result.current_session_id === "string" && result.current_session_id.length > 0
          ? result.current_session_id
          : undefined,
    };
  }

  /**
   * Point-in-time aggregate of a mob's status plus its member list.
   * Wraps the `mob/snapshot` RPC (DELETE_ME C2).
   */
  async mobSnapshot(mobId: string): Promise<{
    mobId: string;
    status: string;
    members: unknown[];
  }> {
    const result = await this.request("mob/snapshot", { mob_id: mobId });
    return {
      mobId: String(result.mob_id ?? mobId),
      status: String(result.status ?? "unknown"),
      members: Array.isArray(result.members) ? result.members : [],
    };
  }

  /**
   * Destroy a mob and surface the structured `MobDestroyReport`.
   * Wraps the `mob/destroy` RPC (DELETE_ME C3). Unlike `mob/lifecycle`
   * with `action: "destroy"`, this dedicated endpoint has a predictable
   * response shape that does not require branching on an action string.
   */
  async mobDestroy(mobId: string): Promise<{
    mobId: string;
    ok: boolean;
    destroyReport: Record<string, unknown>;
  }> {
    const result = await this.request("mob/destroy", { mob_id: mobId });
    const report =
      result.destroy_report && typeof result.destroy_report === "object"
        ? (result.destroy_report as Record<string, unknown>)
        : {};
    return {
      mobId: String(result.mob_id ?? mobId),
      ok: Boolean(result.ok ?? false),
      destroyReport: report,
    };
  }

  /**
   * Rotate the supervisor bridge for all members of a mob.
   * Wraps the `mob/rotate_supervisor` RPC (DELETE_ME C10). Returns the
   * full `SupervisorRotationReport` so operators can inspect per-member
   * rotation outcomes instead of getting a bare `ok: true`.
   */
  async mobRotateSupervisor(mobId: string): Promise<MobRotateSupervisorResult> {
    const result = await this.request("mob/rotate_supervisor", { mob_id: mobId });
    return result as unknown as MobRotateSupervisorResult;
  }

  /**
   * Submit a unit of work to a mob member through the work lane.
   * Wraps the `mob/submit_work` RPC (DELETE_ME C4). Work lane was
   * Rust-only prior to this; `origin` is `"external"` for
   * user-originated turns and `"internal"` for mob-orchestration work.
   * When `workRef` is omitted the server generates a fresh UUID.
   */
  async mobSubmitWork(args: {
    memberRef: string;
    content: unknown;
    workRef?: string;
    origin?: "external" | "internal";
  }): Promise<{ mobId: string; workRef: string; memberRef: string }> {
    const params: Record<string, unknown> = {
      member_ref: args.memberRef,
      content: args.content,
      origin: args.origin ?? "external",
    };
    if (args.workRef !== undefined) {
      params.work_ref = args.workRef;
    }
    const result = await this.request("mob/submit_work", params);
    return {
      mobId: String(result.mob_id ?? ""),
      workRef: String(result.work_ref ?? ""),
      memberRef: String(result.member_ref ?? args.memberRef),
    };
  }

  /**
   * Cancel a previously submitted unit of work.
   * Wraps the `mob/cancel_work` RPC (DELETE_ME C4).
   */
  async mobCancelWork(mobId: string, workRef: string): Promise<{ ok: boolean }> {
    const result = await this.request("mob/cancel_work", {
      mob_id: mobId,
      work_ref: workRef,
    });
    return { ok: Boolean(result.ok ?? false) };
  }

  /**
   * Cancel all in-flight work for a specific mob member.
   * Wraps the `mob/cancel_all_work` RPC (DELETE_ME C4).
   */
  async mobCancelAllWork(args: {
    memberRef: string;
  }): Promise<{ ok: boolean }> {
    const result = await this.request("mob/cancel_all_work", {
      member_ref: args.memberRef,
    });
    return { ok: Boolean(result.ok ?? false) };
  }

  async waitMobKickoff(
    mobId: string,
    options?: MobKickoffWaitOptions,
  ): Promise<MobKickoffMemberSnapshot[]> {
    const params: Record<string, unknown> = { mob_id: mobId };
    if (options?.memberIds !== undefined) {
      params.member_ids = options.memberIds;
    }
    if (options?.timeoutMs !== undefined) {
      params.timeout_ms = options.timeoutMs;
    }
    const result = await this.request("mob/wait_kickoff", params);
    const members = Array.isArray(result.members) ? result.members : [];
    return members.map((entry) => {
      const member =
        entry && typeof entry === "object" ? (entry as Record<string, unknown>) : {};
      const agentIdentity = String(member.agent_identity ?? "");
      if (!agentIdentity) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/wait_kickoff response: member missing agent_identity",
        );
      }
      const status = MeerkatClient.requireStringField(
        member,
        "status",
        "Invalid mob/wait_kickoff response",
      );
      const tokensUsed = MeerkatClient.requireNumberField(
        member,
        "tokens_used",
        "Invalid mob/wait_kickoff response",
      );
      const isFinal = MeerkatClient.requireBooleanField(
        member,
        "is_final",
        "Invalid mob/wait_kickoff response",
      );
      const rawConnectivity =
        member.peer_connectivity && typeof member.peer_connectivity === "object"
          ? (member.peer_connectivity as Record<string, unknown>)
          : undefined;
      return {
        agentIdentity,
        status,
        outputPreview:
          member.output_preview != null ? String(member.output_preview) : undefined,
        error: member.error != null ? String(member.error) : undefined,
        tokensUsed,
        isFinal,
        peerConnectivity: rawConnectivity
          ? {
              reachablePeerCount: Number(rawConnectivity.reachable_peer_count ?? 0),
              unknownPeerCount: Number(rawConnectivity.unknown_peer_count ?? 0),
              unreachablePeers: Array.isArray(rawConnectivity.unreachable_peers)
                ? rawConnectivity.unreachable_peers.map((peer) => {
                    const rawPeer =
                      peer && typeof peer === "object" ? (peer as Record<string, unknown>) : {};
                    return {
                      peer: String(rawPeer.peer ?? ""),
                      reason: rawPeer.reason != null ? String(rawPeer.reason) : undefined,
                    };
                  })
                : [],
            }
          : undefined,
      };
    });
  }

  async waitMobReady(
    mobId: string,
    options?: MobReadyWaitOptions,
  ): Promise<MobReadyMemberSnapshot[]> {
    const params: Record<string, unknown> = { mob_id: mobId };
    if (options?.memberIds !== undefined) {
      params.member_ids = options.memberIds;
    }
    if (options?.timeoutMs !== undefined) {
      params.timeout_ms = options.timeoutMs;
    }
    const result = await this.request("mob/wait_ready", params);
    const members = Array.isArray(result.members) ? result.members : [];
    return members.map((entry) => {
      const member =
        entry && typeof entry === "object" ? (entry as Record<string, unknown>) : {};
      const agentIdentity = String(member.agent_identity ?? "");
      if (!agentIdentity) {
        throw new MeerkatError(
          "INVALID_RESPONSE",
          "Invalid mob/wait_ready response: member missing agent_identity",
        );
      }
      const status = MeerkatClient.requireStringField(
        member,
        "status",
        "Invalid mob/wait_ready response",
      );
      const tokensUsed = MeerkatClient.requireNumberField(
        member,
        "tokens_used",
        "Invalid mob/wait_ready response",
      );
      const isFinal = MeerkatClient.requireBooleanField(
        member,
        "is_final",
        "Invalid mob/wait_ready response",
      );
      const rawConnectivity =
        member.peer_connectivity && typeof member.peer_connectivity === "object"
          ? (member.peer_connectivity as Record<string, unknown>)
          : undefined;
      return {
        agentIdentity,
        status,
        outputPreview:
          member.output_preview != null ? String(member.output_preview) : undefined,
        error: member.error != null ? String(member.error) : undefined,
        tokensUsed,
        isFinal,
        peerConnectivity: rawConnectivity
          ? {
              reachablePeerCount: Number(rawConnectivity.reachable_peer_count ?? 0),
              unknownPeerCount: Number(rawConnectivity.unknown_peer_count ?? 0),
              unreachablePeers: Array.isArray(rawConnectivity.unreachable_peers)
                ? rawConnectivity.unreachable_peers.map((peer) => {
                    const rawPeer =
                      peer && typeof peer === "object" ? (peer as Record<string, unknown>) : {};
                    return {
                      peer: String(rawPeer.peer ?? ""),
                      reason: rawPeer.reason != null ? String(rawPeer.reason) : undefined,
                    };
                  })
                : [],
            }
          : undefined,
      };
    });
  }

  async wait_mob_kickoff(
    mobId: string,
    options?: MobKickoffWaitOptions,
  ): Promise<MobKickoffMemberSnapshot[]> {
    return this.waitMobKickoff(mobId, options);
  }

  async wait_mob_ready(
    mobId: string,
    options?: MobReadyWaitOptions,
  ): Promise<MobReadyMemberSnapshot[]> {
    return this.waitMobReady(mobId, options);
  }

  async spawnMobHelper(
    mobId: string,
    prompt: string,
    options?: {
      agentIdentity?: string;
      roleName?: string;
      profileName?: string;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<{
    output?: string;
    tokensUsed: number;
    agentIdentity: string;
    memberRef: MobMemberRef;
  }> {
    const roleName = options?.roleName ?? options?.profileName;
    const result = await this.request("mob/spawn_helper", {
      mob_id: mobId,
      prompt,
      agent_identity: options?.agentIdentity,
      role_name: roleName,
      runtime_mode: options?.runtimeMode,
      backend: options?.backend,
    });
    const resultIdentity =
      typeof result.agent_identity === "string" && result.agent_identity.length > 0
        ? result.agent_identity
        : options?.agentIdentity;
    if (!resultIdentity) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/spawn_helper response: missing agent identity",
      );
    }
    const memberRef =
      typeof result.member_ref === "string" && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/spawn_helper response: missing member_ref",
      );
    }
    return {
      output: result.output != null ? String(result.output) : undefined,
      tokensUsed: Number(result.tokens_used ?? 0),
      agentIdentity: resultIdentity,
      memberRef,
    };
  }

  async forkMobHelper(
    mobId: string,
    sourceMemberId: string,
    prompt: string,
    options?: {
      agentIdentity?: string;
      roleName?: string;
      profileName?: string;
      forkContext?: Record<string, unknown>;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<{
    output?: string;
    tokensUsed: number;
    agentIdentity: string;
    memberRef: MobMemberRef;
  }> {
    const roleName = options?.roleName ?? options?.profileName;
    const result = await this.request("mob/fork_helper", {
      mob_id: mobId,
      source_member_id: sourceMemberId,
      prompt,
      agent_identity: options?.agentIdentity,
      role_name: roleName,
      fork_context: options?.forkContext,
      runtime_mode: options?.runtimeMode,
      backend: options?.backend,
    });
    const resultIdentity =
      typeof result.agent_identity === "string" && result.agent_identity.length > 0
        ? result.agent_identity
        : options?.agentIdentity;
    if (!resultIdentity) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/fork_helper response: missing agent identity",
      );
    }
    const memberRef =
      typeof result.member_ref === "string" && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid mob/fork_helper response: missing member_ref",
      );
    }
    return {
      output: result.output != null ? String(result.output) : undefined,
      tokensUsed: Number(result.tokens_used ?? 0),
      agentIdentity: resultIdentity,
      memberRef,
    };
  }

  async wireMobMembers(mobId: string, member: string, peer: MobPeerTarget): Promise<void> {
    const payload =
      typeof peer === "string"
        ? { member, peer: { local: peer } }
        : { member, peer };
    await this.request("mob/wire", { mob_id: mobId, ...payload });
  }

  async unwireMobMembers(mobId: string, member: string, peer: MobPeerTarget): Promise<void> {
    const payload =
      typeof peer === "string"
        ? { member, peer: { local: peer } }
        : { member, peer };
    await this.request("mob/unwire", { mob_id: mobId, ...payload });
  }

  async mobLifecycle(mobId: string, action: MobLifecycleAction): Promise<void> {
    await this.request("mob/lifecycle", { mob_id: mobId, action });
  }

  async appendMobSystemContext(
    mobId: string,
    agentIdentity: string,
    text: string,
    options?: { source?: string; idempotencyKey?: string },
  ): Promise<Record<string, unknown>> {
    return this.request("mob/append_system_context", {
      mob_id: mobId,
      agent_identity: agentIdentity,
      text,
      source: options?.source,
      idempotency_key: options?.idempotencyKey,
    });
  }

  async readMobEvents(
    mobId: string,
    options?: MobEventsOptions,
  ): Promise<MobEventsResult> {
    const params: Record<string, unknown> = { mob_id: mobId };
    if (options?.afterCursor !== undefined) {
      params.after_cursor = options.afterCursor;
    }
    if (options?.limit !== undefined) {
      params.limit = options.limit;
    }
    const result = await this.request("mob/events", params);
    const events = Array.isArray(result.events)
      ? (result.events as Array<Record<string, unknown>>)
      : [];
    return { events };
  }

  async mobIngressInteraction(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("mob/ingress_interaction", params);
  }

  async createMobProfile(name: string, profile: MobProfile): Promise<MobProfileLookupResult> {
    const raw = await this.request("mob/profile/create", {
      name,
      profile,
    });
    return MeerkatClient.parseMobProfileLookup(raw);
  }

  async getMobProfile(name: string): Promise<MobProfileLookupResult> {
    const raw = await this.request("mob/profile/get", { name });
    return MeerkatClient.parseMobProfileLookup(raw);
  }

  async listMobProfiles(): Promise<MobProfileLookupResult[]> {
    const raw = await this.request("mob/profile/list", {});
    const profiles = Array.isArray(raw.profiles)
      ? (raw.profiles as Array<Record<string, unknown>>)
      : [];
    return profiles.map((profile) => MeerkatClient.parseMobProfileLookup(profile));
  }

  async updateMobProfile(
    name: string,
    profile: MobProfile,
    expectedRevision: number,
  ): Promise<MobProfileLookupResult> {
    const raw = await this.request("mob/profile/update", {
      name,
      profile,
      expected_revision: expectedRevision,
    });
    return MeerkatClient.parseMobProfileLookup(raw);
  }

  async deleteMobProfile(
    name: string,
    expectedRevision: number,
  ): Promise<MobProfileDeleteResult> {
    const raw = await this.request("mob/profile/delete", {
      name,
      expected_revision: expectedRevision,
    });
    return {
      name: String(raw.name ?? name),
      deletedRevision: Number(raw.deleted_revision ?? expectedRevision),
    };
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

  async subscribeMobMemberEvents(mobId: string, agentIdentity: string): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.openEventSubscription(
      "mob/stream_open",
      { mob_id: mobId, agent_identity: agentIdentity },
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
    const eventId = MeerkatClient.parseOptionalString(raw.event_id ?? raw.eventId);
    const source = MeerkatClient.parseEventSourceIdentity(raw.source);
    const sourceId = MeerkatClient.parseOptionalString(raw.source_id ?? raw.sourceId);
    const seq = MeerkatClient.parseOptionalNumber(raw.seq);
    const timestampMs = MeerkatClient.parseOptionalNumber(raw.timestamp_ms ?? raw.timestampMs);
    const payloadRaw = raw.payload;
    const payload = payloadRaw && typeof payloadRaw === "object"
      ? parseCoreEvent(payloadRaw as Record<string, unknown>)
      : undefined;
    return {
      ...(eventId != null ? { eventId } : {}),
      ...(source != null ? { source } : {}),
      ...(sourceId != null ? { sourceId } : {}),
      ...(seq != null ? { seq } : {}),
      ...(timestampMs != null ? { timestampMs } : {}),
      ...(payload ? { payload } : {}),
    };
  }

  private static parseEventSourceIdentity(raw: unknown): EventSourceIdentity | undefined {
    if (!raw || typeof raw !== "object") {
      return undefined;
    }
    const record = raw as Record<string, unknown>;
    const type = MeerkatClient.parseOptionalString(record.type);
    switch (type) {
      case "session": {
        const sessionId = MeerkatClient.parseOptionalString(record.session_id ?? record.sessionId);
        return sessionId != null ? { type: "session", sessionId } : undefined;
      }
      case "runtime": {
        const runtimeId = MeerkatClient.parseOptionalString(record.runtime_id ?? record.runtimeId);
        return runtimeId != null ? { type: "runtime", runtimeId } : undefined;
      }
      case "interaction": {
        const interactionId = MeerkatClient.parseOptionalString(
          record.interaction_id ?? record.interactionId,
        );
        return interactionId != null ? { type: "interaction", interactionId } : undefined;
      }
      case "callback":
        return { type: "callback" };
      case "external": {
        const sourceId = MeerkatClient.parseOptionalString(record.source_id ?? record.sourceId);
        return sourceId != null ? { type: "external", sourceId } : undefined;
      }
      default:
        return undefined;
    }
  }

  private static parseOptionalString(raw: unknown): string | undefined {
    return typeof raw === "string" ? raw : undefined;
  }

  private static parseOptionalNumber(raw: unknown): number | undefined {
    return typeof raw === "number" && Number.isFinite(raw) ? raw : undefined;
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
    prompt: string | ContentBlock[],
    options?: TurnOptions,
  ): Promise<RunResult> {
    const params: Record<string, unknown> = { session_id: sessionId, prompt };
    const wireRefs = skillRefsToWire(options?.skillRefs);
    if (wireRefs) {
      params.skill_refs = wireRefs;
    }
    if (options?.flowToolOverlay) {
      params.flow_tool_overlay = {
        allowed_tools: options.flowToolOverlay.allowedTools,
        blocked_tools: options.flowToolOverlay.blockedTools,
      };
    }
    if (options?.additionalInstructions != null) {
      params.additional_instructions = options.additionalInstructions;
    }
    if (options?.keepAlive != null) params.keep_alive = options.keepAlive;
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
    prompt: string | ContentBlock[],
    options?: TurnOptions,
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
    if (options?.flowToolOverlay) {
      params.flow_tool_overlay = {
        allowed_tools: options.flowToolOverlay.allowedTools,
        blocked_tools: options.flowToolOverlay.blockedTools,
      };
    }
    if (options?.additionalInstructions != null) {
      params.additional_instructions = options.additionalInstructions;
    }
    if (options?.keepAlive != null) params.keep_alive = options.keepAlive;
    if (options?.model) params.model = options.model;
    if (options?.provider) params.provider = options.provider;
    if (options?.maxTokens) params.max_tokens = options.maxTokens;
    if (options?.systemPrompt) params.system_prompt = options.systemPrompt;
    if (options?.outputSchema) params.output_schema = options.outputSchema;
    if (options?.structuredOutputRetries != null) {
      params.structured_output_retries = options.structuredOutputRetries;
    }
    if (options?.providerParams) params.provider_params = options.providerParams;

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

  private static retiredRuntimeSessionControlError(): MeerkatError {
    return new MeerkatError(
      "METHOD_NOT_FOUND",
      "Retired runtime session control methods are no longer supported by the public RPC surface.",
    );
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeStatus(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeSubmit(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeSubmission(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeSubmissions(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeRetire(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /**
   * @internal
   * @deprecated Retired runtime/session control RPC method; always fails before transport.
   */
  async _runtimeReset(_params: Record<string, unknown>): Promise<never> {
    throw MeerkatClient.retiredRuntimeSessionControlError();
  }

  /** @internal */
  async _send(sessionId: string, command: CommsCommand): Promise<CommsSendReceipt> {
    return this.send(sessionId, command);
  }

  /** @internal */
  async _peers(
    sessionId: string,
  ): Promise<Record<string, unknown>> {
    return this.peers(sessionId);
  }

  /**
   * Send a typed comms command. Invalid discriminators (`source`, `stream`,
   * `handling_mode`, `status`) are rejected at the server's typed-serde
   * boundary.
   */
  async send(sessionId: string, command: CommsCommand): Promise<CommsSendReceipt> {
    const result = await this.request("comms/send", { session_id: sessionId, ...command });
    return MeerkatClient.parseCommsSendReceipt(result);
  }

  async peers(
    sessionId: string,
  ): Promise<Record<string, unknown>> {
    return this.request("comms/peers", { session_id: sessionId });
  }

  async runtimeRealtimeAttachmentStatus(
    sessionId: string,
  ): Promise<RuntimeRealtimeAttachmentStatusResult> {
    const result = await this.request("session/realtime_attachment_status", {
      session_id: sessionId,
    });
    if (typeof result.status !== "string" || result.status.length === 0) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid session/realtime_attachment_status response: missing status",
      );
    }
    return result as unknown as RuntimeRealtimeAttachmentStatusResult;
  }

  /** Idempotent spawn: spawns or returns the existing member entry. */
  async mobEnsureMember(
    mobId: string,
    spec: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return await this.request("mob/ensure_member", {
      mob_id: mobId,
      spec,
    });
  }

  /** Declarative reconcile: converge roster to the desired spec list. */
  async mobReconcile(
    mobId: string,
    desired: Record<string, unknown>[],
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = { mob_id: mobId, desired };
    if (options !== undefined) params.options = options;
    return await this.request("mob/reconcile", params);
  }

  /** Label-filtered member listing. */
  async mobListMembersMatching(
    mobId: string,
    filter: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return await this.request("mob/list_members_matching", {
      mob_id: mobId,
      filter,
    });
  }

  async realtimeOpenInfo(
    request: RealtimeOpenRequest,
  ): Promise<RealtimeOpenInfo> {
    const result = await this.request("realtime/open_info", request);
    if (typeof result.ws_url !== "string" || result.ws_url.length === 0) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid realtime/open_info response: missing ws_url",
      );
    }
    return result as unknown as RealtimeOpenInfo;
  }

  async realtimeStatus(
    params: { target: Record<string, unknown> },
  ): Promise<RealtimeStatusResult> {
    const result = await this.request("realtime/status", params);
    if (typeof result.status !== "object" || result.status === null) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid realtime/status response: missing status",
      );
    }
    return result as unknown as RealtimeStatusResult;
  }

  async realtimeCapabilities(
    params: { target: Record<string, unknown> },
  ): Promise<RealtimeCapabilitiesResult> {
    const result = await this.request("realtime/capabilities", params);
    if (typeof result.capabilities !== "object" || result.capabilities === null) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "Invalid realtime/capabilities response: missing capabilities",
      );
    }
    return result as unknown as RealtimeCapabilitiesResult;
  }

  // -- Auth + realm (Phase 4d) --------------------------------------------
  //
  // These wrappers cover the RPC catalog's auth/* and realm/* methods.
  // Write-side methods (auth/profile/create, delete, login/start,
  // login/complete, login/device_start, logout) are currently server-stubbed
  // with typed INVALID_REQUEST pointing to the CLI; the wrappers surface
  // whatever the server returns so they stay honest about the state.

  async realmList(): Promise<unknown> {
    return this.request("realm/list", {});
  }

  async realmGet(realmId: string): Promise<unknown> {
    return this.request("realm/get", { realm_id: realmId });
  }

  async authProfileList(realmId: string): Promise<unknown> {
    return this.request("auth/profile/list", { realm_id: realmId });
  }

  async authProfileGet(
    realmId: string,
    bindingId: string,
    profileId?: string,
  ): Promise<unknown> {
    const params: Record<string, unknown> = {
      realm_id: realmId,
      binding_id: bindingId,
    };
    if (profileId) params.profile_id = profileId;
    return this.request("auth/profile/get", params);
  }

  async authProfileCreate(params: Record<string, unknown>): Promise<unknown> {
    return this.request("auth/profile/create", params);
  }

  async authProfileDelete(
    realmId: string,
    bindingId: string,
    profileId?: string,
  ): Promise<unknown> {
    const params: Record<string, unknown> = {
      realm_id: realmId,
      binding_id: bindingId,
    };
    if (profileId) params.profile_id = profileId;
    return this.request("auth/profile/delete", params);
  }

  async authLoginStart(params: Record<string, unknown>): Promise<unknown> {
    return this.request("auth/login/start", params);
  }

  async authLoginComplete(params: Record<string, unknown>): Promise<unknown> {
    return this.request("auth/login/complete", params);
  }

  async authLoginDeviceStart(
    params: Record<string, unknown>,
  ): Promise<unknown> {
    return this.request("auth/login/device_start", params);
  }

  async authLoginDeviceComplete(
    params: Record<string, unknown>,
  ): Promise<unknown> {
    return this.request("auth/login/device_complete", params);
  }

  async authLoginProvisionApiKey(
    params: Record<string, unknown>,
  ): Promise<unknown> {
    return this.request("auth/login/provision_api_key", params);
  }

  async authStatusGet(
    realmId: string,
    bindingId: string,
    profileId?: string,
  ): Promise<unknown> {
    const params: Record<string, unknown> = {
      realm_id: realmId,
      binding_id: bindingId,
    };
    if (profileId !== undefined) params.profile_id = profileId;
    return this.request("auth/status/get", params);
  }

  async authLogout(
    realmId: string,
    bindingId: string,
    profileId?: string,
  ): Promise<unknown> {
    const params: Record<string, unknown> = {
      realm_id: realmId,
      binding_id: bindingId,
    };
    if (profileId !== undefined) params.profile_id = profileId;
    return this.request("auth/logout", params);
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
          const normalized = MeerkatClient.parseRpcErrorPayload(error);
          pending.reject(
            new MeerkatError(
              normalized.code,
              normalized.message,
              normalized.details,
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
        // Preserve scope fields when present (delegated-branch / mob-member scoped events).
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

  private static parseRpcErrorPayload(error: Record<string, unknown>): {
    code: string;
    message: string;
    details?: unknown;
  } {
    const rawData = error.data;
    if (typeof rawData === "object" && rawData !== null) {
      const parsed = rawData as Record<string, unknown>;
      return {
        code: String(parsed.code ?? error.code ?? "UNKNOWN"),
        message: String(parsed.message ?? error.message ?? "Unknown error"),
        details: parsed.details ?? parsed.reason ?? rawData,
      };
    }
    const rawMessage = error.message;
    if (typeof rawMessage === "string") {
      try {
        const parsed = JSON.parse(rawMessage) as Record<string, unknown>;
        if (parsed && typeof parsed === "object") {
          return {
            code: String(parsed.code ?? error.code ?? "UNKNOWN"),
            message: String(parsed.message ?? rawMessage),
            details: parsed.details ?? parsed.reason ?? error.data,
          };
        }
      } catch {
        // Fall back to the outer JSON-RPC error payload.
      }
    }
    return {
      code: String(error.code ?? "UNKNOWN"),
      message: String(rawMessage ?? "Unknown error"),
      details: error.data,
    };
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

  private static requireRecord(
    raw: unknown,
    field: string,
    context: string,
  ): Record<string, unknown> {
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
      throw new MeerkatError("INVALID_RESPONSE", `${context}: missing ${field}`);
    }
    return raw as Record<string, unknown>;
  }

  private static requireStringField(
    raw: Record<string, unknown>,
    field: string,
    context: string,
  ): string {
    const value = raw[field];
    if (typeof value !== "string" || value.length === 0) {
      throw new MeerkatError("INVALID_RESPONSE", `${context}: missing ${field}`);
    }
    return value;
  }

  private static requireNumberField(
    raw: Record<string, unknown>,
    field: string,
    context: string,
    displayField = field,
  ): number {
    const value = raw[field];
    if (typeof value !== "number" || !Number.isFinite(value)) {
      throw new MeerkatError("INVALID_RESPONSE", `${context}: ${displayField} must be number`);
    }
    return value;
  }

  private static requireBooleanField(
    raw: Record<string, unknown>,
    field: string,
    context: string,
  ): boolean {
    const value = raw[field];
    if (typeof value !== "boolean") {
      throw new MeerkatError("INVALID_RESPONSE", `${context}: ${field} must be boolean`);
    }
    return value;
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
    const context = "Invalid run result";
    const usageRaw = MeerkatClient.requireRecord(data.usage, "usage", context);
    const usage: Usage = {
      inputTokens: MeerkatClient.requireNumberField(
        usageRaw,
        "input_tokens",
        context,
        "usage.input_tokens",
      ),
      outputTokens: MeerkatClient.requireNumberField(
        usageRaw,
        "output_tokens",
        context,
        "usage.output_tokens",
      ),
      cacheCreationTokens: usageRaw?.cache_creation_tokens != null
        ? MeerkatClient.requireNumberField(
            usageRaw,
            "cache_creation_tokens",
            context,
            "usage.cache_creation_tokens",
          )
        : undefined,
      cacheReadTokens: usageRaw?.cache_read_tokens != null
        ? MeerkatClient.requireNumberField(
            usageRaw,
            "cache_read_tokens",
            context,
            "usage.cache_read_tokens",
          )
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
    const rawExtractionError = data.extraction_error;
    const extractionError =
      rawExtractionError && typeof rawExtractionError === "object"
        ? {
            lastOutput: String((rawExtractionError as Record<string, unknown>).last_output ?? ""),
            attempts: MeerkatClient.requireNumberField(
              rawExtractionError as Record<string, unknown>,
              "attempts",
              context,
              "extraction_error.attempts",
            ),
            reason: String((rawExtractionError as Record<string, unknown>).reason ?? ""),
          }
        : undefined;

    return {
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      text: String(data.text ?? ""),
      turns: MeerkatClient.requireNumberField(data, "turns", context),
      toolCalls: MeerkatClient.requireNumberField(data, "tool_calls", context),
      usage,
      terminalCauseKind:
        typeof data.terminal_cause_kind === "string"
          ? (data.terminal_cause_kind as RunResult["terminalCauseKind"])
          : undefined,
      structuredOutput: data.structured_output,
      extractionError,
      schemaWarnings,
      skillDiagnostics: MeerkatClient.parseSkillDiagnostics(data.skill_diagnostics),
    };
  }

  static parseSessionInfo(data: Record<string, unknown>): SessionInfo {
    const labelsRaw =
      data.labels && typeof data.labels === "object"
        ? (data.labels as Record<string, unknown>)
        : {};
    const labels = Object.fromEntries(
      Object.entries(labelsRaw).map(([key, value]) => [key, String(value)]),
    );
    return {
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      createdAt: Number(data.created_at ?? 0),
      updatedAt: Number(data.updated_at ?? 0),
      messageCount: Number(data.message_count ?? 0),
      isActive: Boolean(data.is_active),
      totalTokens: data.total_tokens != null ? Number(data.total_tokens) : undefined,
      model: data.model != null ? String(data.model) : undefined,
      provider: data.provider != null ? String(data.provider) : undefined,
      lastAssistantText:
        data.last_assistant_text != null ? String(data.last_assistant_text) : undefined,
      labels,
    };
  }

  static parseConfigEnvelope(data: Record<string, unknown>): ConfigEnvelope {
    const rawConfig =
      data.config && typeof data.config === "object"
        ? (data.config as Record<string, unknown>)
        : {};
    const rawResolvedPaths =
      data.resolved_paths && typeof data.resolved_paths === "object"
        ? (data.resolved_paths as Record<string, unknown>)
        : undefined;
    const resolvedPaths = rawResolvedPaths
      ? Object.fromEntries(
          Object.entries(rawResolvedPaths).map(([key, value]) => [key, String(value)]),
        )
      : undefined;
    return {
      config: rawConfig,
      generation: Number(data.generation ?? 0),
      realmId: data.realm_id != null ? String(data.realm_id) : undefined,
      instanceId: data.instance_id != null ? String(data.instance_id) : undefined,
      backend: data.backend != null ? String(data.backend) : undefined,
      resolvedPaths,
    };
  }

  static parseCommsSendReceipt(data: Record<string, unknown>): CommsSendReceipt {
    return {
      ...data,
      requestId: data.request_id != null ? String(data.request_id) : undefined,
      interactionId: data.interaction_id != null ? String(data.interaction_id) : undefined,
      inputId: data.input_id != null ? String(data.input_id) : undefined,
    };
  }

  static parseModelsCatalog(data: Record<string, unknown>): ModelsCatalog {
    const providersRaw = Array.isArray(data.providers)
      ? (data.providers as Array<Record<string, unknown>>)
      : [];
    let contractVersion = { major: 0, minor: 0, patch: 0 };
    if (data.contract_version && typeof data.contract_version === "object") {
      const contractVersionRaw = data.contract_version as Record<string, unknown>;
      contractVersion = {
        major: Number(contractVersionRaw.major ?? 0),
        minor: Number(contractVersionRaw.minor ?? 0),
        patch: Number(contractVersionRaw.patch ?? 0),
      };
    } else if (typeof data.contract_version === "string") {
      const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(data.contract_version);
      if (match) {
        contractVersion = {
          major: Number(match[1]),
          minor: Number(match[2]),
          patch: Number(match[3]),
        };
      }
    }
    return {
      contractVersion,
      providers: providersRaw.map((provider) => ({
        provider: String(provider.provider ?? ""),
        defaultModelId: String(provider.default_model_id ?? ""),
        models: Array.isArray(provider.models)
          ? (provider.models as Array<Record<string, unknown>>).map((model) => ({
              id: String(model.id ?? ""),
              displayName: String(model.display_name ?? ""),
              tier:
                String(model.tier ?? "supported") === "recommended"
                  ? "recommended"
                  : "supported",
              contextWindow:
                model.context_window != null ? Number(model.context_window) : undefined,
              maxOutputTokens:
                model.max_output_tokens != null ? Number(model.max_output_tokens) : undefined,
              serverId: model.server_id != null ? String(model.server_id) : undefined,
              profile:
                model.profile && typeof model.profile === "object"
                  ? {
                      modelFamily: String(
                        (model.profile as Record<string, unknown>).model_family ?? "",
                      ),
                      supportsTemperature: Boolean(
                        (model.profile as Record<string, unknown>).supports_temperature,
                      ),
                      supportsThinking: Boolean(
                        (model.profile as Record<string, unknown>).supports_thinking,
                      ),
                      supportsReasoning: Boolean(
                        (model.profile as Record<string, unknown>).supports_reasoning,
                      ),
                      inlineVideo: Boolean(
                        (model.profile as Record<string, unknown>).inline_video,
                      ),
                      paramsSchema: (model.profile as Record<string, unknown>).params_schema,
                    }
                  : undefined,
            }))
          : [],
      })),
    };
  }

  static parseSchedule(data: Record<string, unknown>): Schedule {
    const labelsRaw =
      data.labels && typeof data.labels === "object"
        ? (data.labels as Record<string, unknown>)
        : {};
    return {
      scheduleId: String(data.schedule_id ?? ""),
      phase: String(data.phase ?? ""),
      revision:
        typeof data.revision === "object" && data.revision !== null
          ? Number((data.revision as Record<string, unknown>)["0"] ?? 0)
          : Number(data.revision ?? 0),
      name: data.name != null ? String(data.name) : undefined,
      description: data.description != null ? String(data.description) : undefined,
      trigger:
        data.trigger && typeof data.trigger === "object"
          ? (data.trigger as Record<string, unknown>)
          : {},
      target:
        data.target && typeof data.target === "object"
          ? (data.target as Record<string, unknown>)
          : {},
      misfirePolicy:
        data.misfire_policy != null
          ? (data.misfire_policy as Record<string, unknown> | string)
          : undefined,
      overlapPolicy: data.overlap_policy != null ? String(data.overlap_policy) : undefined,
      missingTargetPolicy:
        data.missing_target_policy != null ? String(data.missing_target_policy) : undefined,
      planningHorizonDays:
        data.planning_horizon_days != null ? Number(data.planning_horizon_days) : undefined,
      planningHorizonOccurrences:
        data.planning_horizon_occurrences != null
          ? Number(data.planning_horizon_occurrences)
          : undefined,
      nextOccurrenceOrdinal:
        typeof data.next_occurrence_ordinal === "object"
          ? Number((data.next_occurrence_ordinal as Record<string, unknown>)["0"] ?? 0)
          : data.next_occurrence_ordinal != null
            ? Number(data.next_occurrence_ordinal)
            : undefined,
      planningCursorUtc:
        data.planning_cursor_utc != null ? String(data.planning_cursor_utc) : undefined,
      createdAtUtc: data.created_at_utc != null ? String(data.created_at_utc) : undefined,
      updatedAtUtc: data.updated_at_utc != null ? String(data.updated_at_utc) : undefined,
      deletedAtUtc: data.deleted_at_utc != null ? String(data.deleted_at_utc) : undefined,
      labels: Object.fromEntries(
        Object.entries(labelsRaw).map(([key, value]) => [key, String(value)]),
      ),
    };
  }

  static parseScheduleOccurrence(data: Record<string, unknown>): ScheduleOccurrencesResult["occurrences"][number] {
    return {
      occurrenceId: String(data.occurrence_id ?? ""),
      scheduleId: String(data.schedule_id ?? ""),
      scheduleRevision:
        typeof data.schedule_revision === "object" && data.schedule_revision !== null
          ? Number((data.schedule_revision as Record<string, unknown>)["0"] ?? 0)
          : Number(data.schedule_revision ?? 0),
      occurrenceOrdinal:
        typeof data.occurrence_ordinal === "object" && data.occurrence_ordinal !== null
          ? Number((data.occurrence_ordinal as Record<string, unknown>)["0"] ?? 0)
          : Number(data.occurrence_ordinal ?? 0),
      phase: String(data.phase ?? ""),
      dueAtUtc: String(data.due_at_utc ?? ""),
      triggerSnapshot:
        data.trigger_snapshot && typeof data.trigger_snapshot === "object"
          ? (data.trigger_snapshot as Record<string, unknown>)
          : {},
      targetSnapshot:
        data.target_snapshot && typeof data.target_snapshot === "object"
          ? (data.target_snapshot as Record<string, unknown>)
          : {},
      misfirePolicy:
        data.misfire_policy != null
          ? (data.misfire_policy as Record<string, unknown> | string)
          : undefined,
      overlapPolicy: data.overlap_policy != null ? String(data.overlap_policy) : undefined,
      missingTargetPolicy:
        data.missing_target_policy != null ? String(data.missing_target_policy) : undefined,
      claimedBy: data.claimed_by != null ? String(data.claimed_by) : undefined,
      leaseExpiresAtUtc:
        data.lease_expires_at_utc != null ? String(data.lease_expires_at_utc) : undefined,
      deliveryCorrelationId:
        data.delivery_correlation_id != null ? String(data.delivery_correlation_id) : undefined,
      lastReceipt:
        data.last_receipt && typeof data.last_receipt === "object"
          ? (data.last_receipt as Record<string, unknown>)
          : undefined,
      failureClass: data.failure_class != null ? String(data.failure_class) : undefined,
      failureDetail: data.failure_detail != null ? String(data.failure_detail) : undefined,
      attemptCount: data.attempt_count != null ? Number(data.attempt_count) : undefined,
      createdAtUtc: data.created_at_utc != null ? String(data.created_at_utc) : undefined,
      claimedAtUtc: data.claimed_at_utc != null ? String(data.claimed_at_utc) : undefined,
      dispatchedAtUtc:
        data.dispatched_at_utc != null ? String(data.dispatched_at_utc) : undefined,
      completedAtUtc: data.completed_at_utc != null ? String(data.completed_at_utc) : undefined,
      supersededByRevision:
        typeof data.superseded_by_revision === "object" && data.superseded_by_revision !== null
          ? Number((data.superseded_by_revision as Record<string, unknown>)["0"] ?? 0)
          : data.superseded_by_revision != null
            ? Number(data.superseded_by_revision)
            : undefined,
    };
  }

  static parseMobProfileLookup(data: Record<string, unknown>): MobProfileLookupResult {
    if (Boolean(data.not_found)) {
      return {
        notFound: true,
        name: String(data.name ?? ""),
      };
    }
    return {
      notFound: false,
      name: String(data.name ?? ""),
      profile:
        data.profile && typeof data.profile === "object"
          ? (data.profile as MobProfile)
          : undefined,
      revision: data.revision != null ? Number(data.revision) : undefined,
      createdAt: data.created_at != null ? String(data.created_at) : undefined,
      updatedAt: data.updated_at != null ? String(data.updated_at) : undefined,
    };
  }

  private static toWireCreateScheduleRequest(request: CreateScheduleRequest): Record<string, unknown> {
    return {
      name: request.name,
      description: request.description,
      trigger: request.trigger,
      target: request.target,
      misfire_policy: request.misfirePolicy,
      overlap_policy: request.overlapPolicy,
      missing_target_policy: request.missingTargetPolicy,
      labels: request.labels,
      planning_horizon_days: request.planningHorizonDays,
      planning_horizon_occurrences: request.planningHorizonOccurrences,
    };
  }

  private static toWireUpdateSchedulePatch(
    update: UpdateScheduleRequest["update"],
  ): Record<string, unknown> {
    return {
      expected_revision: update.expectedRevision,
      name: update.name,
      description: update.description,
      trigger: update.trigger,
      target: update.target,
      misfire_policy: update.misfirePolicy,
      overlap_policy: update.overlapPolicy,
      missing_target_policy: update.missingTargetPolicy,
      planning_horizon_days: update.planningHorizonDays,
      planning_horizon_occurrences: update.planningHorizonOccurrences,
      labels: update.labels,
    };
  }

  static parseSessionHistory(data: Record<string, unknown>): SessionHistory {
    const rawMessages = Array.isArray(data.messages)
      ? (data.messages as Array<Record<string, unknown>>)
      : [];
    return {
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      messageCount: Number(data.message_count ?? 0),
      offset: Number(data.offset ?? 0),
      limit: data.limit != null ? Number(data.limit) : undefined,
      hasMore: Boolean(data.has_more ?? false),
      messages: rawMessages.map((message) => MeerkatClient.parseSessionMessage(message)),
    };
  }

  static parseSessionForkResult(data: Record<string, unknown>): SessionForkResult {
    return {
      sourceSessionId: String(data.source_session_id ?? ""),
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      messageCount: Number(data.message_count ?? 0),
    };
  }

  static serializeTranscriptReplacement(
    replacement: TranscriptReplacement,
  ): Record<string, unknown> {
    switch (replacement.type) {
      case "message":
        return {
          type: "message",
          message: replacement.message,
        };
      case "user_content_block":
        return {
          type: "user_content_block",
          block_index: replacement.blockIndex,
          block: replacement.block,
        };
      case "assistant_block":
        return {
          type: "assistant_block",
          block_index: replacement.blockIndex,
          block: replacement.block,
        };
      case "tool_result_content_block":
        return {
          type: "tool_result_content_block",
          result_index: replacement.resultIndex,
          block_index: replacement.blockIndex,
          block: replacement.block,
        };
    }
    throw new Error(
      `Unsupported transcript replacement type: ${(replacement as { type: string }).type}`,
    );
  }

  static parseSessionMessage(data: Record<string, unknown>): SessionMessage {
    const role = String(data.role ?? "");
    const contentValue =
      role === "system_notice" && data.content == null && data.body != null
        ? String(data.body)
        : data.content;
    const rawToolCalls = Array.isArray(data.tool_calls)
      ? (data.tool_calls as Array<Record<string, unknown>>)
      : [];
    const rawBlocks = Array.isArray(data.blocks)
      ? (data.blocks as Array<Record<string, unknown>>)
      : [];
    const rawResults = Array.isArray(data.results)
      ? (data.results as Array<Record<string, unknown>>)
      : [];
    return {
      role,
      createdAt: String(data.created_at ?? ""),
      content:
        contentValue != null ? MeerkatClient.parseContentInput(contentValue) : undefined,
      toolCalls: rawToolCalls.map(
        (toolCall): SessionToolCall => ({
          id: String(toolCall.id ?? ""),
          name: String(toolCall.name ?? ""),
          args: toolCall.args,
        }),
      ),
      stopReason: data.stop_reason != null ? String(data.stop_reason) : undefined,
      blocks: rawBlocks.map((block) => MeerkatClient.parseSessionAssistantBlock(block)),
      results: rawResults.map(
        (result): SessionToolResult => ({
          toolUseId: String(result.tool_use_id ?? ""),
          content: MeerkatClient.parseContentInput(result.content),
          isError: Boolean(result.is_error ?? false),
        }),
      ),
    };
  }

  static parseContentInput(value: unknown): ContentInput {
    if (Array.isArray(value)) {
      return value
        .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
        .map((block) => MeerkatClient.parseContentBlock(block));
    }
    return String(value ?? "");
  }

  static parseContentBlock(data: Record<string, unknown>): ContentBlock {
    const type = String(data.type ?? "");
    if (type === "text") {
      return { type: "text", text: String(data.text ?? "") };
    }
    if (type === "image") {
        const source = String(data.source ?? "inline");
        if (source === "blob") {
        return {
          type: "image",
          media_type: String(data.media_type ?? ""),
          source: "blob",
          blob_id: String(data.blob_id ?? ""),
        };
      }
      return {
        type: "image",
        media_type: String(data.media_type ?? ""),
        source: "inline",
        data: String(data.data ?? ""),
      };
    }
    if (type === "video") {
      return {
        type: "video",
        media_type: String(data.media_type ?? ""),
        duration_ms: Number(data.duration_ms ?? 0),
        source: "inline",
        data: String(data.data ?? ""),
      };
    }
    return { type: "text", text: "" };
  }

  static parseSessionAssistantBlock(data: Record<string, unknown>): SessionAssistantBlock {
    const blockData = (data.data as Record<string, unknown> | undefined) ?? {};
    const blobRef = blockData.blob_ref as Record<string, unknown> | undefined;
    const revisedPrompt =
      blockData.revised_prompt != null && typeof blockData.revised_prompt === "object"
        ? (blockData.revised_prompt as Record<string, unknown>)
        : undefined;
    return {
      blockType: String(data.block_type ?? ""),
      text: blockData.text != null ? String(blockData.text) : undefined,
      id: blockData.id != null ? String(blockData.id) : undefined,
      name: blockData.name != null ? String(blockData.name) : undefined,
      args: blockData.args,
      imageId: blockData.image_id != null ? String(blockData.image_id) : undefined,
      blobId: blobRef?.blob_id != null ? String(blobRef.blob_id) : undefined,
      mediaType: blockData.media_type != null ? String(blockData.media_type) : undefined,
      width: blockData.width != null ? Number(blockData.width) : undefined,
      height: blockData.height != null ? Number(blockData.height) : undefined,
      revisedPrompt,
      meta: blockData.meta as Record<string, unknown> | undefined,
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
    prompt: string | ContentBlock[],
    options?: SessionOptions,
  ): Record<string, unknown> {
    const params: Record<string, unknown> = { prompt };
    if (!options) return params;

    if (options.model) params.model = options.model;
    if (options.provider) params.provider = options.provider;
    if (options.systemPrompt) params.system_prompt = options.systemPrompt;
    if (options.maxTokens) params.max_tokens = options.maxTokens;
    if (options.outputSchema != null) params.output_schema = options.outputSchema;
    if (options.structuredOutputRetries != null) {
      params.structured_output_retries = options.structuredOutputRetries;
    }
    if (options.hooksOverride != null) params.hooks_override = options.hooksOverride;
    if (options.enableBuiltins != null) params.enable_builtins = options.enableBuiltins;
    if (options.enableShell != null) params.enable_shell = options.enableShell;
    if (options.enableMemory != null) params.enable_memory = options.enableMemory;
    if (options.enableMob != null) params.enable_mob = options.enableMob;
    if (options.keepAlive != null) params.keep_alive = options.keepAlive;
    if (options.commsName) params.comms_name = options.commsName;
    if (options.peerMeta != null) params.peer_meta = options.peerMeta;
    if (options.budgetLimits != null) params.budget_limits = options.budgetLimits;
    if (options.providerParams != null) params.provider_params = options.providerParams;
    if (options.preloadSkills != null) {
      params.preload_skills = skillKeysToWire(options.preloadSkills);
    }
    const wireRefs = skillRefsToWire(options.skillRefs);
    if (wireRefs) params.skill_refs = wireRefs;
    if (options.labels != null) params.labels = options.labels;
    if (options.additionalInstructions != null) {
      params.additional_instructions = options.additionalInstructions;
    }
    if (options.appContext !== undefined) params.app_context = options.appContext;
    if (options.shellEnv != null) params.shell_env = options.shellEnv;
    if (options.externalTools != null) params.external_tools = options.externalTools;
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
      const options: SpawnOptions = { stdio: ["ignore", "inherit", "pipe"] };
      const proc = spawn(command, args, options);
      let stderr = "";
      proc.stderr?.on("data", (chunk: string | Buffer) => { stderr += String(chunk); });
      proc.on("error", (error: Error) => { reject(error); });
      proc.on("close", (code: number | null) => {
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
    const args: string[] = ["--realtime-ws", "127.0.0.1:0"];
    if (!options) return args;
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
