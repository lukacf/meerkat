import { subscribeTimelineWithRecovery } from "./timeline-recovery";
import type { ConsoleTimelineSubscriptionOptions, ConsoleTransportState } from "./timeline-subscription";
import {
  type ConsoleWorkbenchTarget,
  type MetaWorkbenchTarget,
} from "./targets";
import {
  CONSOLE_BLOB_PATH_PREFIX,
  CONSOLE_REST_PATHS,
  CONSOLE_RPC_METHODS,
} from "./contract";
import {
  callConsoleRpc,
  fetchJson,
  queryTimeline,
  sendConsole,
  sendConsoleMultipart,
  subscribeTimelineEvents,
  readConsoleBlob,
  uploadConsoleBlobMultipart,
} from "./network";
import type {
  ConsoleExperience,
  ConsoleFrame,
  ConsoleModulesResponse,
  ConsoleTimelineAccepted,
  ConsoleTimelinePage,
} from "./runtime-types";
import type { ConsoleTopologyControlCapabilities } from "./topology";

export type ConsoleFactSource =
  | "meta-protocol"
  | "controller-derived"
  | "optimistic"
  | "host-adapter";

export interface ConsoleFact<T> {
  value: T;
  provenance: {
    source: ConsoleFactSource;
    contractVersion?: string;
    routeOrMethod?: string;
    cursor?: string;
    capabilityVersion?: string;
    timestampMs?: number;
    correlationId?: string;
  };
}

export interface ConsoleCapabilities {
  methods: string[];
  version?: string;
  readOnly?: boolean;
  runtime_capabilities?: unknown;
  method_capabilities?: unknown;
  topologyControl?: ConsoleTopologyControlCapabilities;
}

export interface ConsoleTimelineQueryInput {
  signal?: AbortSignal;
  identity?: string;
  conversationId?: string;
  after?: string;
  before?: string;
  mode?: "since" | "recent";
  limit?: number;
}

export interface ConsoleTimelineSubscribeInput {
  signal?: AbortSignal;
  onTransportState?: (state: ConsoleTransportState) => void;
  identity?: string;
  conversationId?: string;
  after?: string;
  limit?: number;
}

export interface ConsoleSendInput {
  identity: string;
  content: string | Array<Record<string, unknown>>;
  origin: string;
  idempotencyKey: string;
  handlingMode?: "queue" | "steer";
  attachments?: File[];
}

export interface ConsoleUploadInput {
  blobId?: string;
  file?: File;
  mediaType?: string;
}

export interface ConsoleBlobReadInput {
  blobId: string;
  signal?: AbortSignal;
}

export interface ConsoleUploadResult {
  blob_id: string;
  url?: string;
}

export const CONSOLE_COMMAND_NAMES = {
  inspectIdentity: "inspectIdentity",
  retireIdentity: "retireIdentity",
  respawnIdentity: "respawnIdentity",
  resetIdentity: "resetIdentity",
  stopMemberRun: "stopMemberRun",
  listRoutingRoutes: "listRoutingRoutes",
  listDeliveryHistory: "listDeliveryHistory",
  listGatingPending: "listGatingPending",
  listGatingAudit: "listGatingAudit",
  decideGating: "decideGating",
  accessStatus: "accessStatus",
  getAccessConfig: "getAccessConfig",
  setAccessConfig: "setAccessConfig",
  enableAccess: "enableAccess",
  upsertAccessRule: "upsertAccessRule",
  deleteAccessRule: "deleteAccessRule",
  setAccessGroup: "setAccessGroup",
  deleteAccessGroup: "deleteAccessGroup",
  previewAccess: "previewAccess",
  listMemoryRecords: "listMemoryRecords",
  getMemoryRecord: "getMemoryRecord",
  listMemoryQuarantine: "listMemoryQuarantine",
  listMemoryDreams: "listMemoryDreams",
  getMemoryOverview: "getMemoryOverview",
  listMemoryProposals: "listMemoryProposals",
  listMemoryInjections: "listMemoryInjections",
  listMemoryHarvests: "listMemoryHarvests",
  listMemoryDreamRuns: "listMemoryDreamRuns",
  listMemoryAuditVerdicts: "listMemoryAuditVerdicts",
  decideMemoryQuarantine: "decideMemoryQuarantine",
  workgraphSnapshot: "workgraphSnapshot",
  workgraphGet: "workgraphGet",
  workgraphEvents: "workgraphEvents",
  workgraphGoalStatus: "workgraphGoalStatus",
  workgraphClaim: "workgraphClaim",
  workgraphRelease: "workgraphRelease",
  workgraphClose: "workgraphClose",
  workgraphGoalConfirm: "workgraphGoalConfirm",
  workgraphGoalRequestClose: "workgraphGoalRequestClose",
  workgraphAttentionPause: "workgraphAttentionPause",
  workgraphAttentionResume: "workgraphAttentionResume",
  workgraphAttentionReassign: "workgraphAttentionReassign",
  topologyQuery: "topologyQuery",
  topologyPlan: "topologyPlan",
  topologyApply: "topologyApply",
  topologyOperationGet: "topologyOperationGet",
  topologyAuditQuery: "topologyAuditQuery",
} as const;

export type ConsoleCommandName = typeof CONSOLE_COMMAND_NAMES[keyof typeof CONSOLE_COMMAND_NAMES];

type ConsoleCommandSpec = {
  method: typeof CONSOLE_RPC_METHODS[keyof typeof CONSOLE_RPC_METHODS];
  targetKinds: ReadonlySet<MetaWorkbenchTarget["kind"]>;
};

const LEGACY_INSPECT_IDENTITY_METHOD = "meta/inspect_identity";

const CONSOLE_COMMAND_SPECS: Record<ConsoleCommandName, ConsoleCommandSpec> = {
  [CONSOLE_COMMAND_NAMES.inspectIdentity]: {
    method: CONSOLE_RPC_METHODS.inspectIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.retireIdentity]: {
    method: CONSOLE_RPC_METHODS.retireIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.respawnIdentity]: {
    method: CONSOLE_RPC_METHODS.respawnIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.resetIdentity]: {
    method: CONSOLE_RPC_METHODS.resetIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  // Run-fenced Stop: params carry `member_id`, `run_id` and `reason`
  // explicitly (the run id comes from the member's `run_started`).
  [CONSOLE_COMMAND_NAMES.stopMemberRun]: {
    method: CONSOLE_RPC_METHODS.stopMemberRun,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.listRoutingRoutes]: {
    method: CONSOLE_RPC_METHODS.routingRoutesList,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/routing"]),
  },
  [CONSOLE_COMMAND_NAMES.listDeliveryHistory]: {
    method: CONSOLE_RPC_METHODS.deliveryHistory,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/routing"]),
  },
  [CONSOLE_COMMAND_NAMES.listGatingPending]: {
    method: CONSOLE_RPC_METHODS.gatingPending,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.listGatingAudit]: {
    method: CONSOLE_RPC_METHODS.gatingAudit,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.decideGating]: {
    method: CONSOLE_RPC_METHODS.gatingDecide,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.accessStatus]: {
    method: CONSOLE_RPC_METHODS.accessStatus,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.getAccessConfig]: {
    method: CONSOLE_RPC_METHODS.accessGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.setAccessConfig]: {
    method: CONSOLE_RPC_METHODS.accessSet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.enableAccess]: {
    method: CONSOLE_RPC_METHODS.accessEnable,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.upsertAccessRule]: {
    method: CONSOLE_RPC_METHODS.accessRuleUpsert,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.deleteAccessRule]: {
    method: CONSOLE_RPC_METHODS.accessRuleDelete,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.setAccessGroup]: {
    method: CONSOLE_RPC_METHODS.accessGroupSet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.deleteAccessGroup]: {
    method: CONSOLE_RPC_METHODS.accessGroupDelete,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.previewAccess]: {
    method: CONSOLE_RPC_METHODS.accessPreview,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryRecords]: {
    method: CONSOLE_RPC_METHODS.memoryPanelRecords,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.getMemoryRecord]: {
    method: CONSOLE_RPC_METHODS.memoryPanelRecord,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryQuarantine]: {
    method: CONSOLE_RPC_METHODS.memoryPanelQuarantine,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryDreams]: {
    method: CONSOLE_RPC_METHODS.memoryPanelDreams,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.getMemoryOverview]: {
    method: CONSOLE_RPC_METHODS.memoryPanelOverview,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryProposals]: {
    method: CONSOLE_RPC_METHODS.memoryPanelProposals,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryInjections]: {
    method: CONSOLE_RPC_METHODS.memoryPanelInjections,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryHarvests]: {
    method: CONSOLE_RPC_METHODS.memoryPanelHarvests,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryDreamRuns]: {
    method: CONSOLE_RPC_METHODS.memoryPanelDreamRuns,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryAuditVerdicts]: {
    method: CONSOLE_RPC_METHODS.memoryPanelAuditVerdicts,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.decideMemoryQuarantine]: {
    method: CONSOLE_RPC_METHODS.memoryQuarantineDecide,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphSnapshot]: {
    method: CONSOLE_RPC_METHODS.workgraphSnapshot,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGet]: {
    method: CONSOLE_RPC_METHODS.workgraphGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphEvents]: {
    method: CONSOLE_RPC_METHODS.workgraphEvents,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalStatus]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalStatus,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphClaim]: {
    method: CONSOLE_RPC_METHODS.workgraphClaim,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphRelease]: {
    method: CONSOLE_RPC_METHODS.workgraphRelease,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphClose]: {
    method: CONSOLE_RPC_METHODS.workgraphClose,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalConfirm]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalConfirm,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalRequestClose]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalRequestClose,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionPause]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionPause,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionResume]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionResume,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionReassign]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionReassign,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyQuery]: {
    method: CONSOLE_RPC_METHODS.topologyQuery,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyPlan]: {
    method: CONSOLE_RPC_METHODS.topologyPlan,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyApply]: {
    method: CONSOLE_RPC_METHODS.topologyApply,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyOperationGet]: {
    method: CONSOLE_RPC_METHODS.topologyOperationGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyAuditQuery]: {
    method: CONSOLE_RPC_METHODS.topologyAuditQuery,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
};

/** Return the typed RPC method for a modeled console command. */
export function consoleCommandMethod(command: ConsoleCommandName): string {
  return CONSOLE_COMMAND_SPECS[command].method;
}

export interface ConsoleCommandRequest {
  signal?: AbortSignal;
  command: ConsoleCommandName;
  target: ConsoleWorkbenchTarget;
  params?: Record<string, unknown>;
}

export interface ConsoleCommandResult {
  command: ConsoleCommandName;
  accepted: boolean;
  result?: unknown;
}

export interface MetaConsoleTransport {
  loadExperience(): Promise<ConsoleExperience>;
  loadModules?(): Promise<ConsoleModulesResponse>;
  capabilities(): Promise<ConsoleCapabilities>;
  queryTimeline(input: ConsoleTimelineQueryInput): Promise<ConsoleTimelinePage>;
  subscribeTimeline(
    input: ConsoleTimelineSubscribeInput,
    onFrame: (frame: ConsoleFrame) => void,
    options?: ConsoleTimelineSubscriptionOptions,
  ): () => void;
  send(input: ConsoleSendInput): Promise<ConsoleTimelineAccepted>;
  executeCommand?(input: ConsoleCommandRequest): Promise<ConsoleCommandResult>;
  upload?(input: ConsoleUploadInput): Promise<ConsoleUploadResult>;
  /// Reads a stored blob's bytes with this transport's own request
  /// authority, the credentials the host gives every Console request.
  /// The Console shows stored images from these bytes; a bare
  /// `/blobs/{id}` load carries no Authorization. Resolves to a Blob typed
  /// with its image media type. `blobUrl` stays a projection-only resolver.
  readBlob?(input: ConsoleBlobReadInput): Promise<Blob>;
  blobUrl?(blobId: string): string;
}

export interface MetaConsoleController {
  transport: MetaConsoleTransport;
  commands: ConsoleCommandSurface;
  timeline: ConsoleTimelineController;
  facts: {
    meta<T>(value: T, meta?: Partial<ConsoleFact<T>["provenance"]>): ConsoleFact<T>;
    derived<T>(value: T, meta?: Partial<ConsoleFact<T>["provenance"]>): ConsoleFact<T>;
    optimistic<T>(value: T, correlationId: string): ConsoleFact<T>;
    host<T>(value: T): ConsoleFact<T>;
  };
}

export interface ConsoleCommandSurface {
  sendMessage(
    target: ConsoleWorkbenchTarget,
    input: Omit<ConsoleSendInput, "identity">,
  ): Promise<{
    optimistic: ConsoleFact<{ idempotencyKey: string; targetId: string }>;
    accepted: ConsoleFact<ConsoleTimelineAccepted>;
  }>;
  uploadBlob(input: ConsoleUploadInput): Promise<ConsoleFact<ConsoleUploadResult>>;
  execute(input: ConsoleCommandRequest): Promise<ConsoleCommandResult>;
}

export interface ConsoleTimelineController {
  query(input: ConsoleTimelineQueryInput): Promise<ConsoleFact<ConsoleTimelinePage>>;
  subscribeWithBackfill(
    input: ConsoleTimelineSubscribeInput,
    onFrame: (frame: ConsoleFact<ConsoleFrame>) => void,
    onReplayGap?: () => void,
  ): Promise<() => void>;
}

export function createHttpConsoleTransport({
  baseUrl,
  fetchTimeoutMs,
}: {
  baseUrl: string;
  fetchTimeoutMs?: number | (() => number);
}): MetaConsoleTransport {
  const timeout = () => typeof fetchTimeoutMs === "function" ? fetchTimeoutMs() : fetchTimeoutMs;
  return {
    loadExperience: () => fetchJson<ConsoleExperience>(baseUrl, CONSOLE_REST_PATHS.experience, timeout()),
    loadModules: () => fetchJson<ConsoleModulesResponse>(baseUrl, CONSOLE_REST_PATHS.modules, timeout()),
    capabilities: async () => normalizeCapabilities(
      await callConsoleRpc<unknown>(baseUrl, CONSOLE_RPC_METHODS.capabilities, {}, timeout()),
    ),
    queryTimeline: (input) => queryTimeline(baseUrl, input, input.limit, timeout(), input.signal),
    subscribeTimeline: (input, onFrame, options) => subscribeTimelineEvents(baseUrl, input, onFrame, {
      signal: input.signal, onTransportState: input.onTransportState, ...options,
    }),
    send: (input) => {
      const handlingMode = input.handlingMode ?? "queue";
      if (input.attachments?.length) {
        return sendConsoleMultipart(
          baseUrl,
          input.identity,
          input.content,
          input.attachments,
          input.origin,
          input.idempotencyKey,
          handlingMode,
          timeout(),
        );
      }
      return sendConsole(
        baseUrl,
        input.identity,
        input.content,
        input.origin,
        input.idempotencyKey,
        handlingMode,
        timeout(),
      );
    },
    executeCommand: async (input) => {
      const spec = commandSpec(input.command);
      const params = { ...(input.params || {}) };
      if (identityCommandMethods.has(spec.method)) {
        const identity = stringValue(params.identity) || identityForCommandTarget(input.target);
        if (!identity) {
          throw new Error(`${input.command} requires an identity-addressed target`);
        }
        params.identity = identity;
      }
      let result: unknown;
      try {
        result = await callConsoleRpc<unknown>(baseUrl, spec.method, params, timeout(), input.signal);
      } catch (error) {
        if (
          spec.method !== CONSOLE_RPC_METHODS.inspectIdentity ||
          !isJsonRpcMethodNotFoundError(error)
        ) {
          throw error;
        }
        result = await callConsoleRpc<unknown>(baseUrl, LEGACY_INSPECT_IDENTITY_METHOD, params, timeout(), input.signal);
      }
      return {
        command: input.command,
        accepted: true,
        result,
      };
    },
    upload: (input) => uploadConsoleBlobMultipart(baseUrl, input, timeout()),
    readBlob: (input) => readConsoleBlob(baseUrl, input.blobId, timeout(), input.signal),
    blobUrl: (blobId) => `${baseUrl}${CONSOLE_BLOB_PATH_PREFIX}${encodeURIComponent(blobId)}`,
  };
}

export function createMetaConsoleController({
  transport,
}: {
  transport: MetaConsoleTransport;
}): MetaConsoleController {
  const facts = createFactFactory();
  return {
    transport,
    facts,
    timeline: createTimelineController(transport, facts),
    commands: createConsoleCommandSurface(transport, facts),
  };
}

function isJsonRpcMethodNotFoundError(error: unknown): boolean {
  const rpcError = (error as { rpcError?: { code?: unknown } } | null)?.rpcError;
  return rpcError?.code === -32601;
}

function createConsoleCommandSurface(
  transport: MetaConsoleTransport,
  facts: MetaConsoleController["facts"],
): ConsoleCommandSurface {
  let cachedCapabilities: ConsoleCapabilities | null = null;
  let capabilitiesRequest: Promise<ConsoleCapabilities> | null = null;
  const capabilities = async (force = false) => {
    if (force || !cachedCapabilities) {
      if (!capabilitiesRequest) {
        capabilitiesRequest = transport.capabilities().finally(() => {
          capabilitiesRequest = null;
        });
      }
      cachedCapabilities = await capabilitiesRequest;
    }
    return cachedCapabilities;
  };
  const requireFreshCapability = async (method: string) => {
    let currentCapabilities = await capabilities(true);
    if (!hasCapability(currentCapabilities, method)) {
      currentCapabilities = await capabilities(true);
    }
    requireCapability(currentCapabilities, method);
    return currentCapabilities;
  };
  return {
    async sendMessage(target, input) {
      const identity = identityForSendTarget(target);
      if (!identity) {
        throw new Error(`target ${target.kind} cannot send Meta console messages`);
      }
      const currentCapabilities = await requireFreshCapability(CONSOLE_RPC_METHODS.send);
      const optimistic = facts.optimistic({
        idempotencyKey: input.idempotencyKey,
        targetId: target.id,
      }, input.idempotencyKey);
      const accepted = await transport.send({
        ...input,
        identity,
      });
      return {
        optimistic,
        accepted: facts.meta(accepted, {
          routeOrMethod: CONSOLE_RPC_METHODS.send,
          capabilityVersion: currentCapabilities.version,
          correlationId: input.idempotencyKey,
          cursor: accepted.cursor,
        }),
      };
    },
    async uploadBlob(input) {
      const currentCapabilities = await requireFreshCapability(CONSOLE_RPC_METHODS.blobUpload);
      if (!transport.upload) {
        throw new Error(`transport does not implement ${CONSOLE_RPC_METHODS.blobUpload}`);
      }
      const uploaded = await transport.upload(input);
      return facts.meta(uploaded, {
        routeOrMethod: CONSOLE_RPC_METHODS.blobUpload,
        capabilityVersion: currentCapabilities.version,
      });
    },
    async execute(input) {
      input.signal?.throwIfAborted();
      if (!isMetaTarget(input.target)) {
        throw new Error(`host target ${input.target.kind} cannot execute Meta commands`);
      }
      const spec = commandSpec(input.command);
      if (!spec.targetKinds.has(input.target.kind)) {
        throw new Error(`target ${input.target.kind} cannot execute command ${input.command}`);
      }
      await requireFreshCapability(spec.method);
      input.signal?.throwIfAborted();
      if (!transport.executeCommand) {
        throw new Error(`transport does not implement command ${input.command}`);
      }
      return transport.executeCommand(input);
    },
  };
}

function createTimelineController(
  transport: MetaConsoleTransport,
  facts: MetaConsoleController["facts"],
): ConsoleTimelineController {
  return {
    async query(input) {
      const page = await transport.queryTimeline(input);
      return facts.meta(page, {
        routeOrMethod: CONSOLE_RPC_METHODS.queryTimeline,
        cursor: page.latestCursor || page.nextCursor,
      });
    },
    subscribeWithBackfill(input, onFrame, onReplayGap) {
      return subscribeTimelineWithRecovery(transport, input, (frame) => {
        return onFrame(facts.meta(frame, {
          routeOrMethod: CONSOLE_REST_PATHS.timelineStream,
          cursor: frame.cursor,
        }));
      }, onReplayGap);
    },
  };
}

function createFactFactory(): MetaConsoleController["facts"] {
  const wrap = <T>(
    source: ConsoleFactSource,
    value: T,
    meta: Partial<ConsoleFact<T>["provenance"]> = {},
  ): ConsoleFact<T> => ({
    value,
    provenance: {
      source,
      timestampMs: Date.now(),
      ...meta,
    },
  });
  return {
    meta: (value, meta) => wrap("meta-protocol", value, meta),
    derived: (value, meta) => wrap("controller-derived", value, meta),
    optimistic: (value, correlationId) => wrap("optimistic", value, { correlationId }),
    host: (value) => wrap("host-adapter", value),
  };
}

function normalizeCapabilities(value: unknown): ConsoleCapabilities {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const methods = Array.isArray(record.methods)
    ? Array.from(new Set(record.methods.filter((method): method is string => typeof method === "string" && method.trim().length > 0)))
    : [];
  return {
    methods,
    version: typeof record.version === "string" ? record.version : undefined,
    ...(typeof record.read_only === "boolean" ? { readOnly: record.read_only } : {}),
    runtime_capabilities: record.runtime_capabilities,
    method_capabilities: record.method_capabilities,
    ...(record.topology_control && typeof record.topology_control === "object"
      ? { topologyControl: record.topology_control as ConsoleTopologyControlCapabilities }
      : {}),
  };
}

/** Fresh capability evidence, distinct from an unavailable network response. */
export class ConsoleCapabilityUnavailableError extends Error {
  readonly kind = "console-capability-unavailable";
  readonly availableMethods: readonly string[];
  constructor(readonly method: string, capabilities: ConsoleCapabilities) {
    super(`Meta capability missing for ${method}`);
    this.name = "ConsoleCapabilityUnavailableError";
    this.availableMethods = [...capabilities.methods];
  }
}

function requireCapability(capabilities: ConsoleCapabilities, method: string) {
  if (!hasCapability(capabilities, method)) {
    throw new ConsoleCapabilityUnavailableError(method, capabilities);
  }
}

function hasCapability(capabilities: ConsoleCapabilities, method: string): boolean {
  return capabilities.methods.includes(method);
}

function commandSpec(command: ConsoleCommandName): ConsoleCommandSpec {
  if (!isConsoleCommandName(command)) {
    throw new Error(`unknown Meta console command ${String(command)}`);
  }
  return CONSOLE_COMMAND_SPECS[command];
}

function isConsoleCommandName(command: unknown): command is ConsoleCommandName {
  return typeof command === "string" && command in CONSOLE_COMMAND_SPECS;
}

function identityForSendTarget(target: ConsoleWorkbenchTarget): string | null {
  return target.kind === "meta/identity-chat" && "identity" in target ? target.identity : null;
}

const identityCommandMethods = new Set<string>([
  CONSOLE_RPC_METHODS.inspectIdentity,
  CONSOLE_RPC_METHODS.retireIdentity,
  CONSOLE_RPC_METHODS.respawnIdentity,
  CONSOLE_RPC_METHODS.resetIdentity,
]);

function identityForCommandTarget(target: ConsoleWorkbenchTarget): string | null {
  if (
    (target.kind === "meta/identity-chat" || target.kind === "meta/identity-inspect")
    && "identity" in target
  ) {
    return target.identity;
  }
  return null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function isMetaTarget(target: ConsoleWorkbenchTarget): target is MetaWorkbenchTarget {
  return target.kind.startsWith("meta/");
}
