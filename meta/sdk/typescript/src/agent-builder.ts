/**
 * SessionAgentBuilder protocol and CallbackDispatcher.
 *
 * The builder protocol uses imperative mutation: buildAgent receives a
 * mutable SessionBuildOptions and modifies it in place.
 */

import {
  parseForkBuildSource,
  SessionBuildOptions,
  type ToolHandler,
} from "./models.js";
import {
  parseLiveAssistantOutputAddress,
  type LiveAssistantOutputAddress,
} from "./live.js";
import { ToolResultContent } from "./tool-content.js";
import { parseToolCallRequester } from "./tool-requester.js";
import { runAsToolCallback } from "./decisions.js";
import { ContractMismatchError } from "./errors.js";
import {
  DetachedJobContext,
  DetachedJobExecution,
  DetachedJobReporter,
  DetachedJobResult,
  jobAuthorityToDict,
  parseDetachedJobAuthority,
  sameJobAuthority,
  type DetachedJobAuthority,
  type DetachedJobHandler,
  type DetachedJobRpc,
  type DetachedJobRunner,
  type JobCredentialResolver,
} from "./jobs.js";
import {
  parseErrorEvent,
  parseAgentBuildContext,
  parseRosterContext,
  parseDurableAgentSpec,
  parseAgentBuildDraft,
  agentBuildDraftToDict,
  durableAgentSpecToDict,
  managedPeerEdgeToDict,
  type ErrorEvent,
  type SessionCreatedContext,
  type RosterProvider,
  type AgentCustomizer,
  type TopologyProvider,
  type ProviderCallbackContext,
} from "./types.js";

/**
 * A host's handler of `meta/decisions/notice`: one open durable decision,
 * noticed once per stdio connection when the host lists it. `event` is
 * `held` (nothing of its content shown) or `presented` (shown to the stdio
 * operator, with the rendering).
 */
export type DecisionNoticeHandler = (
  notice: Record<string, unknown>,
) => void | Promise<void>;

/**
 * A deterministic host runnable's `callback/schedule_fire` handler. Receives
 * the raw occurrence (`schedule_id`, `occurrence_id`, `due_at`, optional
 * `payload`). Throwing or rejecting fails the durable occurrence attempt.
 * The gateway acknowledges completion but does not persist the return value.
 */
export type ScheduleFireHandler = (
  occurrence: Record<string, unknown>,
) => unknown | Promise<unknown>;

// -- Protocol -------------------------------------------------------------

/**
 * Protocol for building agents during session creation.
 *
 * @example
 * ```ts
 * const builder: SessionAgentBuilder = {
 *   async buildAgent(opts) {
 *     opts.profileName = "assistant";
 *     opts.registerTool("search", searchHandler);
 *   },
 * };
 * ```
 */
export interface SessionAgentBuilder {
  buildAgent(options: SessionBuildOptions): Promise<void>;
  /** Called after session creation succeeds. Best-effort — errors logged, not propagated. */
  afterCreate?(sessionId: string, context: SessionCreatedContext): Promise<void>;
}

// -- Error callback type --------------------------------------------------

export type ErrorCallback = (event: ErrorEvent) => void | Promise<void>;

interface RegisteredJobRunner {
  readonly execution: DetachedJobExecution;
  readonly profileName: string | null;
}

interface ActiveJobAttempt {
  readonly authority: DetachedJobAuthority;
  readonly runnerKey: string;
  readonly runnerHandle: string;
  readonly controller: AbortController;
  task: Promise<void> | null;
  adopted: boolean;
  superseded: boolean;
}

// -- CallbackDispatcher ---------------------------------------------------

/**
 * Routes incoming JSON-RPC callbacks from the Rust runtime to the
 * registered SessionAgentBuilder and tool handlers.
 *
 * Tool handlers are scoped by a build-level scope_id to prevent
 * cross-session handler bleed in concurrent sessions.
 */
/**
 * The mob a member-building callback names: its top-level `mob_id`. `null`
 * when the host sent none (or null): a host of an earlier release, which
 * serves its own mob only. A host that names mobs names them by text, so any
 * other value is refused with `ContractMismatchError`: read as "none" it
 * would stand for the host's own mob, the wrong one for a member of another.
 */
function callbackMob(params: Record<string, unknown>): string | null {
  const mobId = params.mob_id;
  if (mobId === undefined || mobId === null) return null;
  if (typeof mobId !== "string") {
    throw new ContractMismatchError("a callback's mob_id must be a string");
  }
  return mobId;
}

/**
 * The incarnation of the mob a customize callback names: its top-level
 * `mob_incarnation`. `null` when the host sent none (or null): a host of an
 * earlier release. A host that names it names it by a non-empty text, so any
 * other value is refused with `ContractMismatchError`: read as "none" it
 * would keep the tools of a mob that may be gone.
 */
function callbackMobIncarnation(params: Record<string, unknown>): string | null {
  const incarnation = params.mob_incarnation;
  if (incarnation === undefined || incarnation === null) return null;
  if (typeof incarnation !== "string" || incarnation === "") {
    throw new ContractMismatchError("a callback's mob_incarnation must be a non-empty string");
  }
  return incarnation;
}

/** One key for a member: its mob (`null` for the host's own) and its identity. */
function memberKey(mobId: string | null, identity: string): string {
  return JSON.stringify([mobId, identity]);
}

export class CallbackDispatcher {
  private _builder: SessionAgentBuilder | null = null;
  private _errorCallback: ErrorCallback | null = null;
  private readonly _toolHandlers = new Map<string, ToolHandler>();
  private readonly _scopeTools = new Map<string, string[]>();
  // Latest customizer scope per member, so a restore can release the prior
  // scope (the gateway has no scope-release signal: newest wins). A member is
  // its mob and its identity: two mobs of one host can each have a member of
  // the same name, and a build of one must not release the other's tools.
  // Keyed by `memberKey`; the value keeps the member's mob beside its scope,
  // so a mob's scopes are found without reading a key.
  private readonly _customizerScopeByMember = new Map<
    string,
    { mobId: string | null; scopeId: string }
  >();
  // The incarnations the gateway named for each mob id, from the customize
  // callback, in the order they were first named: the last is the mob's, the
  // ones before it are of mobs that are gone. A mob created under the id of a
  // destroyed one is another incarnation: its first build releases every
  // scope kept under that mob id, which are the destroyed mob's. One entry
  // per incarnation named in this process; nothing is stored.
  private readonly _customizerIncarnationsByMob = new Map<string, string[]>();
  private _rosterProvider: RosterProvider | null = null;
  private _agentCustomizer: AgentCustomizer | null = null;
  private _topologyProvider: TopologyProvider | null = null;
  private _jobRpc: DetachedJobRpc | null = null;
  private _jobCredentialResolver: JobCredentialResolver | null = null;
  private readonly _jobRunners = new Map<string, RegisteredJobRunner>();
  private readonly _jobAttempts = new Map<string, ActiveJobAttempt>();
  private readonly _jobHighestFence = new Map<string, number>();
  private readonly _jobTasks = new Set<Promise<void>>();
  private readonly _jobMutationTails = new Map<string, Promise<void>>();
  private readonly _liveOutputConsumers = new Map<
    string,
    (output: LiveAssistantOutputAddress) => void
  >();
  private _decisionNoticeHandler: DecisionNoticeHandler | null = null;
  private readonly _scheduleFireHandlers = new Map<string, ScheduleFireHandler>();

  registerBuilder(builder: SessionAgentBuilder): void {
    this._builder = builder;
  }

  registerErrorCallback(callback: ErrorCallback): void {
    this._errorCallback = callback;
  }

  /** Receive each `meta/decisions/notice` the stdio gateway writes. */
  registerDecisionNoticeHandler(handler: DecisionNoticeHandler): void {
    this._decisionNoticeHandler = handler;
  }

  /** Register or replace the handler for a named host runnable. */
  registerScheduleFireHandler(name: string, handler: ScheduleFireHandler): void {
    if (typeof name !== "string" || !name.trim()) {
      throw new TypeError("runnable name must be a non-empty string");
    }
    if (typeof handler !== "function") {
      throw new TypeError("handler must be callable");
    }
    this._scheduleFireHandlers.set(name, handler);
  }

  registerRosterProvider(provider: RosterProvider): void {
    this._rosterProvider = provider;
  }

  registerAgentCustomizer(customizer: AgentCustomizer): void {
    this._agentCustomizer = customizer;
  }

  registerTopologyProvider(provider: TopologyProvider): void {
    this._topologyProvider = provider;
  }

  registerJobRpc(rpc: DetachedJobRpc): void {
    if (typeof rpc !== "function") {
      throw new TypeError("job rpc must be callable");
    }
    this._jobRpc = rpc;
  }

  registerJobCredentialResolver(resolver: JobCredentialResolver): void {
    if (typeof resolver !== "function") {
      throw new TypeError("job credential resolver must be callable");
    }
    this._jobCredentialResolver = resolver;
  }

  async waitForJobTasks(): Promise<void> {
    while (this._jobTasks.size > 0) {
      await Promise.allSettled([...this._jobTasks]);
    }
  }

  /** Remove all tool handlers for a scope. Call when a session ends. */
  releaseScope(scopeId: string): void {
    const tools = this._scopeTools.get(scopeId);
    if (tools) {
      for (const toolName of tools) {
        this._toolHandlers.delete(`${scopeId}:${toolName}`);
      }
      this._scopeTools.delete(scopeId);
    }
  }

  /**
   * Record the incarnation a customize callback names for a mob, and answer
   * whether the build is of the mob as it is named now.
   *
   * An incarnation named for the first time, for a mob id that was named by
   * another: the mob of the earlier builds is gone (a mob keeps its
   * incarnation for as long as it exists), so every customizer scope kept
   * under that mob id is released and the later mob starts with none of them.
   * An incarnation that was named before a later one: the build is of a mob
   * that is gone and was handled late; it releases nothing and the answer is
   * `false`.
   */
  private _noteMobIncarnation(mobId: string, incarnation: string): boolean {
    let named = this._customizerIncarnationsByMob.get(mobId);
    if (named === undefined) {
      named = [];
      this._customizerIncarnationsByMob.set(mobId, named);
    }
    if (named.length > 0 && named[named.length - 1] === incarnation) return true;
    if (named.includes(incarnation)) return false;
    if (named.length > 0) {
      for (const [member, kept] of [...this._customizerScopeByMember]) {
        if (kept.mobId === mobId) {
          this.releaseScope(kept.scopeId);
          this._customizerScopeByMember.delete(member);
        }
      }
    }
    named.push(incarnation);
    return true;
  }

  /** Whether `incarnation` is the one last named for `mobId`. */
  private _isMobIncarnation(mobId: string, incarnation: string): boolean {
    const named = this._customizerIncarnationsByMob.get(mobId);
    return named !== undefined && named.length > 0 && named[named.length - 1] === incarnation;
  }

  registerLiveOutputConsumer(
    channelId: string,
    consumer: (output: LiveAssistantOutputAddress) => void,
  ): () => void {
    if (channelId.trim().length === 0) {
      throw new TypeError("channelId must be non-empty");
    }
    if (this._liveOutputConsumers.has(channelId)) {
      throw new Error(`live output consumer already registered for ${channelId}`);
    }
    this._liveOutputConsumers.set(channelId, consumer);
    return () => {
      if (this._liveOutputConsumers.get(channelId) === consumer) {
        this._liveOutputConsumers.delete(channelId);
      }
    };
  }

  async handleCallback(
    method: string,
    params: Record<string, unknown>,
    callbackContext?: ProviderCallbackContext,
  ): Promise<unknown> {
    if (method === "callback/schedule_fire") {
      const runnable = params.runnable ?? "";
      const handler = this._scheduleFireHandlers.get(runnable as string);
      if (handler === undefined) {
        throw new Error(`no schedule-fire handler registered for runnable: ${runnable}`);
      }
      const occurrence = "occurrence" in params ? params.occurrence : {};
      const result = await handler(occurrence as Record<string, unknown>);
      // A void JS handler is Python's None: keep a JSON-RPC result field.
      return result === undefined ? null : result;
    }
    if (method === "meta/decisions/notice") {
      // One open durable decision, noticed once per gateway connection; a
      // host without a handler is told nothing more.
      if (this._decisionNoticeHandler !== null) {
        await this._decisionNoticeHandler(params);
      }
      return null;
    }
    if (method === "meta/live/assistant_output_available") {
      const output = parseLiveAssistantOutputAddress(params);
      const consumer = this._liveOutputConsumers.get(output.channelId);
      if (consumer === undefined) {
        throw new Error(
          `no live output consumer registered for ${output.channelId}`,
        );
      }
      consumer(output);
      return { accepted: true };
    }

    if (method === "meta/on_error") {
      if (this._errorCallback !== null) {
        const event = parseErrorEvent(params);
        try {
          await this._errorCallback(event);
        } catch {
          // Fire-and-forget — swallow error callback failures
        }
      }
      return null;
    }

    if (method === "callback/after_create") {
      if (this._builder !== null && typeof this._builder.afterCreate === "function") {
        const sessionId = String(params.session_id ?? "");
        const context: SessionCreatedContext = {
          model: String(params.model ?? ""),
          labels: typeof params.labels === "object" && params.labels !== null
            ? (params.labels as Record<string, string>)
            : {},
          systemPrompt: typeof params.system_prompt === "string"
            ? params.system_prompt
            : null,
        };
        try {
          await this._builder.afterCreate(sessionId, context);
        } catch {
          // Best-effort — swallow afterCreate failures.
        }
      }
      return null;
    }

    if (method === "callback/build_agent") {
      if (this._builder === null) {
        throw new Error("no SessionAgentBuilder registered");
      }
      const rawOptions = {
        ...(typeof params.options === "object" && params.options !== null
          ? (params.options as Record<string, unknown>)
          : {}),
      };
      const scopeId = String(rawOptions.scope_id ?? "");
      if (!scopeId) {
        throw new Error("callback/build_agent requires scope_id in options");
      }
      delete rawOptions.scope_id;

      const forkSourceIdentity = rawOptions.fork_source_identity;
      if (
        forkSourceIdentity !== undefined &&
        forkSourceIdentity !== null &&
        typeof forkSourceIdentity !== "string"
      ) {
        throw new TypeError(
          `fork_source_identity must be a string, got ${typeof forkSourceIdentity}`,
        );
      }
      const opts = new SessionBuildOptions({
        forkSource: parseForkBuildSource(rawOptions.fork_source),
        forkSourceIdentity: forkSourceIdentity ?? null,
      });
      if (rawOptions.app_context !== undefined) {
        opts.appContext = rawOptions.app_context;
      }
      if (Array.isArray(rawOptions.additional_instructions)) {
        opts.additionalInstructions = rawOptions.additional_instructions.filter(
          (v): v is string => typeof v === "string",
        );
      }
      if (typeof rawOptions.session_id === "string") {
        opts.sessionId = rawOptions.session_id;
      }
      if (
        typeof rawOptions.labels === "object" &&
        rawOptions.labels !== null
      ) {
        opts.labels = rawOptions.labels as Record<string, string>;
      }
      if (typeof rawOptions.profile_name === "string") {
        opts.profileName = rawOptions.profile_name;
      }
      if (typeof rawOptions.resume_session_id === "string") {
        opts.resumeSessionId = rawOptions.resume_session_id;
      }

      await this._builder.buildAgent(opts);

      // Capture tool handlers scoped to this build
      const toolNames: string[] = [];
      for (const [name, handler] of opts.toolHandlers) {
        this._toolHandlers.set(`${scopeId}:${name}`, handler);
        toolNames.push(name);
      }
      this._scopeTools.set(scopeId, toolNames);
      for (const execution of opts.jobExecutions.values()) {
        this._registerJobRunner(execution, opts.profileName);
      }

      return opts.toDict();
    }

    if (method === "callback/call_tool") {
      const scopeId = String(params.scope_id ?? "");
      if (!scopeId) {
        throw new Error("callback/call_tool requires scope_id");
      }
      const toolName = String(params.tool ?? "");
      const args = (
        typeof params.arguments === "object" && params.arguments !== null
          ? params.arguments
          : {}
      ) as Record<string, unknown>;

      const handler = this._toolHandlers.get(`${scopeId}:${toolName}`);
      if (!handler) {
        throw new Error(
          `no handler registered for tool: ${toolName} (scope: ${scopeId})`,
        );
      }

      // The handler serves an agent's tool call: a durable decision's decide
      // refuses inside it, and inside the work it starts. The gateway names
      // the run's requester here, never in the arguments; absent or
      // malformed is no requester.
      const result = await runAsToolCallback(async () =>
        handler(args, parseToolCallRequester(params.requester)),
      );
      // Rich content is opt-in: only an explicit ToolResultContent is delivered
      // as content blocks (images / multi-block). Any other return keeps the
      // legacy single-text-block behavior.
      if (result instanceof ToolResultContent) {
        return { content_blocks: result.blocks };
      }
      return { content: result };
    }

    if (method === "callback/job/start") {
      return this._startJob(params);
    }

    if (method === "callback/job/reconcile") {
      return this._reconcileJobs(params);
    }

    if (method === "callback/job/cancel") {
      return this._cancelJob(params);
    }

    // -- Roster callback ----------------------------------------------------

    if (method === "callback/roster_provider/roster") {
      if (this._rosterProvider === null) {
        throw new Error("no RosterProvider registered");
      }
      // The gateway nests the Rust `RosterContext` under `context`, the same
      // envelope as the topology and customizer callbacks.
      const context = parseRosterContext(params.context ?? {});
      const specs = await this._rosterProvider.roster(context);
      return specs.map(durableAgentSpecToDict);
    }

    // -- Topology callback --------------------------------------------------

    if (method === "callback/topology_provider/compute_edges") {
      if (this._topologyProvider === null) {
        throw new Error("no TopologyProvider registered");
      }
      const targetIdentities = Array.isArray(params.target_identities)
        ? params.target_identities.map(String)
        : [];
      const mobId = callbackMob(params);
      const sent = params.context ?? {};
      // The provider's context names the mob whose edges are asked.
      const context =
        mobId !== null && typeof sent === "object" && sent !== null && !Array.isArray(sent)
          ? { ...(sent as Record<string, unknown>), mob_id: mobId }
          : sent;
      const edges = await this._topologyProvider.computeEdges(
        targetIdentities,
        context,
      );
      return edges.map(managedPeerEdgeToDict);
    }

    // -- Customizer callbacks -----------------------------------------------

    if (method === "callback/agent_customizer/customize_build") {
      if (this._agentCustomizer === null) {
        throw new Error("no AgentCustomizer registered");
      }
      const scopeId = String(params.scope_id ?? "");
      const mobId = callbackMob(params);
      const incarnation = callbackMobIncarnation(params);
      // The context has a `mobId` only when the host names a mob.
      const context = {
        ...parseAgentBuildContext(params.context),
        ...(mobId === null ? {} : { mobId }),
      };
      const spec = parseDurableAgentSpec(params.spec);
      const draft = parseAgentBuildDraft(params.draft);
      // Before the customizer is asked, and whatever it does with the build:
      // a build that names another incarnation of its mob releases what was
      // kept for the earlier one. A build of an incarnation that a later one
      // superseded is of a mob that is gone: the customizer is asked and the
      // draft answered, and its tools are not registered.
      let ofTheMobAsNamed = true;
      if (mobId !== null && incarnation !== null) {
        ofTheMobAsNamed = this._noteMobIncarnation(mobId, incarnation);
      }
      await this._agentCustomizer.customizeBuild(context, spec, draft);
      // While the customizer ran, a build of a later incarnation of this mob
      // may have been named: this build is then of a mob that is gone too.
      if (ofTheMobAsNamed && mobId !== null && incarnation !== null) {
        ofTheMobAsNamed = this._isMobIncarnation(mobId, incarnation);
      }
      // Capture any tool handlers the customizer registered via
      // draft.registerTool(), keyed by this build's scope — the same
      // (scope, tool) map build_agent uses, so callback/call_tool dispatches
      // to them. Guard on scopeId for compat with a gateway not yet sending it.
      if (scopeId && draft.toolHandlers.size > 0 && ofTheMobAsNamed) {
        // Release the previous scope for this member before registering the
        // new one: customize_build runs again at each seat, fresh life, role
        // migration and build of the identity, and the gateway never signals
        // scope release, so this bounds growth to one live scope per member
        // (newest wins). The member of another mob with the same identity
        // keeps its own.
        const member = memberKey(mobId, context.identity);
        const prior = this._customizerScopeByMember.get(member);
        if (prior && prior.scopeId !== scopeId) {
          this.releaseScope(prior.scopeId);
        }
        this._customizerScopeByMember.set(member, { mobId, scopeId });
        const toolNames: string[] = [];
        for (const [name, handler] of draft.toolHandlers) {
          this._toolHandlers.set(`${scopeId}:${name}`, handler);
          toolNames.push(name);
        }
        this._scopeTools.set(scopeId, toolNames);
      }
      return agentBuildDraftToDict(draft);
    }

    if (method === "callback/agent_customizer/after_create") {
      // Read first: a mob_id that is not text is refused whoever listens.
      const mobId = callbackMob(params);
      if (
        this._agentCustomizer !== null &&
        typeof this._agentCustomizer.afterCreate === "function"
      ) {
        const identity = String(params.identity ?? "");
        const sessionId = String(params.session_id ?? "");
        const rawLabels = (params.labels != null && typeof params.labels === "object")
          ? Object.fromEntries(
              Object.entries(params.labels as Record<string, unknown>).map(([k, v]) => [k, String(v)])
            )
          : {};
        const context: SessionCreatedContext = {
          model: String(params.model ?? ""),
          labels: rawLabels,
          systemPrompt: typeof params.system_prompt === "string" ? params.system_prompt : null,
          // A `mobId` only when the host names a mob.
          ...(mobId === null ? {} : { mobId }),
        };
        try {
          await this._agentCustomizer.afterCreate(
            identity,
            sessionId,
            context,
          );
        } catch {
          // Best-effort — swallow afterCreate failures.
        }
      }
      return null;
    }

    throw new Error(`unknown callback method: ${method}`);
  }

  // -- Detached callback job shell ----------------------------------------

  private _registerJobRunner(
    execution: DetachedJobExecution,
    profileName: string | null,
  ): void {
    const existing = this._jobRunners.get(execution.runnerKey);
    if (existing && existing.profileName !== profileName) {
      throw new Error(
        `detached runner "${execution.runner}"@${execution.version} is already ` +
          `bound to profile "${String(existing.profileName)}"; refusing ` +
          `conflicting profile "${String(profileName)}"`,
      );
    }
    this._jobRunners.set(execution.runnerKey, { execution, profileName });
  }

  private static _runnerKey(params: Record<string, unknown>): string {
    const runner = params.runner;
    if (typeof runner !== "object" || runner === null) {
      throw new Error("job callback requires a runner object");
    }
    const raw = runner as Record<string, unknown>;
    if (typeof raw.name !== "string" || raw.name === "") {
      throw new Error("job callback runner requires a non-empty name");
    }
    if (typeof raw.version !== "string" || raw.version === "") {
      throw new Error("job callback runner requires a non-empty version");
    }
    return `${raw.name}\u0000${raw.version}`;
  }

  private async _resolveJobCredentials(
    registration: RegisteredJobRunner,
    scopes: readonly string[],
  ): Promise<Readonly<Record<string, unknown>>> {
    if (scopes.length === 0) return {};
    if (this._jobCredentialResolver === null) {
      throw new Error(
        "detached callback requires credential scopes but no execution-time " +
          "credential resolver is configured",
      );
    }
    const resolved = await this._jobCredentialResolver(
      registration.profileName,
      scopes,
    );
    if (typeof resolved !== "object" || resolved === null || Array.isArray(resolved)) {
      throw new TypeError("job credential resolver must return an object");
    }
    return resolved;
  }

  private async _startJob(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const authority = parseDetachedJobAuthority(params.authority);
    return this._withJobMutation(
      authority.jobId,
      () => this._startJobLocked(params, authority),
    );
  }

  private async _withJobMutation<T>(
    jobId: string,
    operation: () => Promise<T>,
  ): Promise<T> {
    const previous = this._jobMutationTails.get(jobId) ?? Promise.resolve();
    let release!: () => void;
    const current = new Promise<void>((resolve) => {
      release = resolve;
    });
    const tail = previous.then(() => current);
    this._jobMutationTails.set(jobId, tail);
    await previous;
    try {
      return await operation();
    } finally {
      release();
      if (this._jobMutationTails.get(jobId) === tail) {
        this._jobMutationTails.delete(jobId);
      }
    }
  }

  private async _startJobLocked(
    params: Record<string, unknown>,
    authority: DetachedJobAuthority,
  ): Promise<Record<string, unknown>> {
    const runnerKey = CallbackDispatcher._runnerKey(params);
    const registration = this._jobRunners.get(runnerKey);
    const runnerHandle = params.runner_handle;
    if (typeof runnerHandle !== "string" || runnerHandle === "") {
      throw new Error("callback/job/start requires a non-empty runner_handle");
    }
    if (registration === undefined || this._jobRpc === null) {
      return { accepted: false, runner_handle: runnerHandle };
    }

    const highest = this._jobHighestFence.get(authority.jobId);
    const active = this._jobAttempts.get(authority.jobId);
    if (highest !== undefined && authority.fence < highest) {
      return { accepted: false, runner_handle: runnerHandle };
    }
    if (highest === authority.fence) {
      if (
        active !== undefined &&
        sameJobAuthority(active.authority, authority) &&
        active.runnerHandle === runnerHandle &&
        active.task !== null
      ) {
        return { accepted: true, runner_handle: runnerHandle };
      }
      return { accepted: false, runner_handle: runnerHandle };
    }

    const scopes = params.credential_scopes ?? [];
    if (
      !Array.isArray(scopes) ||
      !scopes.every((scope) => typeof scope === "string" && scope !== "")
    ) {
      throw new Error("credential_scopes must be a list of non-empty strings");
    }
    let credentials: Readonly<Record<string, unknown>>;
    try {
      credentials = await this._resolveJobCredentials(
        registration,
        scopes as string[],
      );
    } catch {
      return { accepted: false, runner_handle: runnerHandle };
    }

    if (active !== undefined) {
      active.superseded = true;
      active.controller.abort();
    }

    const controller = new AbortController();
    const reporter = new DetachedJobReporter(authority, this._jobRpc);
    const context = new DetachedJobContext(
      authority,
      runnerHandle,
      params.arguments,
      credentials,
      typeof params.resume_checkpoint === "string"
        ? params.resume_checkpoint
        : null,
      controller.signal,
      reporter,
      parseToolCallRequester(params.requester),
    );
    const attempt: ActiveJobAttempt = {
      authority,
      runnerKey,
      runnerHandle,
      controller,
      task: null,
      adopted: false,
      superseded: false,
    };
    this._jobHighestFence.set(authority.jobId, authority.fence);
    this._jobAttempts.set(authority.jobId, attempt);
    const task = this._runJob(
      attempt,
      registration.execution.handler,
      context,
      reporter,
    );
    attempt.task = task;
    this._jobTasks.add(task);
    void task.finally(() => this._jobTasks.delete(task));
    return { accepted: true, runner_handle: runnerHandle };
  }

  private async _runJob(
    attempt: ActiveJobAttempt,
    runner: DetachedJobHandler,
    context: DetachedJobContext,
    reporter: DetachedJobReporter,
  ): Promise<void> {
    try {
      const result = typeof runner === "function"
        ? await runner(context)
        : await runner.run(context);
      if (attempt.superseded) return;
      if (context.signal.aborted) {
        await reporter.cancelAck();
      } else if (result instanceof DetachedJobResult) {
        await reporter.complete(result.resultRef);
      } else if (result === undefined || result === null) {
        await reporter.complete(null);
      } else {
        throw new TypeError(
          "detached runner must return DetachedJobResult or undefined; " +
            "persist result bytes and return only their reference",
        );
      }
    } catch {
      if (!attempt.superseded) {
        try {
          if (context.signal.aborted) {
            await reporter.cancelAck();
          } else {
            await reporter.fail("host_runner_failed");
          }
        } catch {
          // The durable runtime remains authoritative and reconciles an
          // ambiguous report after transport recovery.
        }
      }
    } finally {
      const current = this._jobAttempts.get(attempt.authority.jobId);
      if (current === attempt && !attempt.adopted) {
        this._jobAttempts.delete(attempt.authority.jobId);
      }
    }
  }

  private async _renewReconciledAttempt(
    authority: DetachedJobAuthority,
  ): Promise<boolean> {
    if (this._jobRpc === null) return false;
    const nowMs = Date.now();
    try {
      await new DetachedJobReporter(authority, this._jobRpc).heartbeat({
        heartbeatAtMs: nowMs,
        leaseExpiresAtMs: nowMs + 120_000,
      });
      return true;
    } catch {
      return false;
    }
  }

  private async _reconcileJobs(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    if (!Array.isArray(params.attempts)) {
      throw new Error("callback/job/reconcile requires attempts");
    }
    const live: Record<string, unknown>[] = [];
    for (const candidate of params.attempts) {
      if (typeof candidate !== "object" || candidate === null) {
        throw new Error("reconcile attempts must be objects");
      }
      const raw = candidate as Record<string, unknown>;
      const authority = parseDetachedJobAuthority(raw.authority);
      const runnerKey = CallbackDispatcher._runnerKey(raw);
      const runnerHandle = raw.runner_handle;
      if (typeof runnerHandle !== "string" || runnerHandle === "") {
        throw new Error("reconcile attempt requires runner_handle");
      }
      const active = this._jobAttempts.get(authority.jobId);
      if (
        active !== undefined &&
        sameJobAuthority(active.authority, authority) &&
        active.runnerKey === runnerKey &&
        active.runnerHandle === runnerHandle
      ) {
        if (
          !active.controller.signal.aborted &&
          (active.adopted || active.task !== null) &&
          await this._renewReconciledAttempt(authority)
        ) {
          live.push(jobAuthorityToDict(authority));
        }
        continue;
      }

      const registration = this._jobRunners.get(runnerKey);
      if (raw.restart_class !== "adoptable") {
        // A reconstruction hook may adopt only work whose generated lifecycle
        // declaration permits adoption. Replay/resume requires a later
        // machine-authorized claim.
        continue;
      }
      const runner = registration?.execution.handler;
      const reconcile = typeof runner === "object" && runner !== null
        ? (runner as DetachedJobRunner).reconcile
        : undefined;
      if (reconcile === undefined || await reconcile.call(runner, raw) !== true) {
        continue;
      }
      const highest = this._jobHighestFence.get(authority.jobId);
      if (highest !== undefined && authority.fence < highest) continue;
      this._jobHighestFence.set(authority.jobId, authority.fence);
      this._jobAttempts.set(authority.jobId, {
        authority,
        runnerKey,
        runnerHandle,
        controller: new AbortController(),
        task: null,
        adopted: true,
        superseded: false,
      });
      if (!await this._renewReconciledAttempt(authority)) {
        const current = this._jobAttempts.get(authority.jobId);
        if (
          current !== undefined &&
          sameJobAuthority(current.authority, authority)
        ) {
          this._jobAttempts.delete(authority.jobId);
        }
        continue;
      }
      live.push(jobAuthorityToDict(authority));
    }
    return { live_attempts: live };
  }

  private async _cancelJob(
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const authority = parseDetachedJobAuthority(params.authority);
    const active = this._jobAttempts.get(authority.jobId);
    if (
      active === undefined ||
      !sameJobAuthority(active.authority, authority)
    ) {
      return { accepted: false };
    }
    active.controller.abort();
    const runner = this._jobRunners.get(active.runnerKey)?.execution.handler;
    const cancel = typeof runner === "object" && runner !== null
      ? (runner as DetachedJobRunner).cancel
      : undefined;
    if (cancel !== undefined) {
      await cancel.call(runner, jobAuthorityToDict(authority));
    }
    return { accepted: true };
  }
}
