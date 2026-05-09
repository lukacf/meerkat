import { EventSubscription } from './events.js';
import type {
  AuthBindingRef,
  ContentInput,
  HandlingMode,
  SpawnSpec,
  SpawnResult,
  MobMember,
  MobPeerTarget,
  MobStatus,
  MobLifecycleAction,
  FlowStatus,
  AttributedEventItem,
  MemberEventItem,
  MobEvent,
  AppendSystemContextOptions,
  MobAppendSystemContextResult,
  RenderMetadata,
  ContentBlock,
  MemberDeliveryReceipt,
  MobRespawnResult,
  MobMemberSnapshot,
  ResolvedModelCapabilities,
  MobHelperResult,
  EventEnvelope,
  EventSourceIdentity,
  AgentRuntimeId,
  SubscriptionLaggedEvent,
} from './types.js';
import type {
  MobAppendSystemContextResult as WireMobAppendSystemContextResult,
  MobFlowStatusResult as WireMobFlowStatusResult,
  MobHelperResult as WireMobHelperResult,
  MobMemberSendResult as WireMobMemberSendResult,
  MobMemberStatusResult as WireMobMemberStatusResult,
  MobRespawnResult as WireMobRespawnResult,
  MobStatusResult as WireMobStatusResult,
  WireMobMemberStatus,
} from './generated/mob.js';

// WASM function signatures (bound at construction)
interface MobWasmBindings {
  mob_spawn: (mobId: string, specs: string) => Promise<string>;
  mob_retire: (mobId: string, agentIdentity: string) => Promise<void>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_unwire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_wire_peer?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_unwire_peer?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_wire_target?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_unwire_target?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_list_members: (mobId: string) => Promise<string>;
  mob_append_system_context: (
    mobId: string,
    agentIdentity: string,
    requestJson: string,
  ) => Promise<string>;
  mob_member_send: (mobId: string, agentIdentity: string, requestJson: string) => Promise<string>;
  mob_member_status: (mobId: string, agentIdentity: string) => Promise<string>;
  mob_respawn: (mobId: string, agentIdentity: string, initialMessage?: string) => Promise<string>;
  mob_force_cancel: (mobId: string, agentIdentity: string) => Promise<void>;
  mob_spawn_helper: (mobId: string, requestJson: string) => Promise<string>;
  mob_fork_helper: (mobId: string, requestJson: string) => Promise<string>;
  mob_status: (mobId: string) => Promise<string>;
  mob_lifecycle: (mobId: string, action: string) => Promise<void>;
  mob_events: (mobId: string, afterCursor: number, limit: number) => Promise<string>;
  mob_run_flow: (mobId: string, flowId: string, params: string) => Promise<string>;
  mob_flow_status: (mobId: string, runId: string) => Promise<string>;
  mob_cancel_flow: (mobId: string, runId: string) => Promise<void>;
  mob_member_subscribe: (mobId: string, agentIdentity: string) => Promise<number>;
  mob_subscribe_events: (mobId: string) => Promise<number>;
  poll_subscription: (handle: number) => string;
  close_subscription: (handle: number) => void;
}

function encodeBase64UrlJson(payload: Record<string, unknown>): string {
  const bytes = new TextEncoder().encode(JSON.stringify(payload));
  let binary = '';
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  const b64 = btoa(binary);
  return b64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function encodeMemberRef(mobId: string, agentIdentity: string): string {
  return encodeBase64UrlJson({ m: mobId, a: agentIdentity });
}

function spawnSpecPayload(spec: SpawnSpec): Record<string, unknown> {
  return {
    profile: spec.profile,
    agent_identity: spec.agent_identity,
    runtime_mode: spec.runtime_mode,
    initial_message: spec.initial_message,
    labels: spec.labels,
    context: spec.context,
    additional_instructions: spec.additional_instructions,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function requireOnlyKeys(
  value: Record<string, unknown>,
  allowedKeys: readonly string[],
  message: string,
): void {
  const allowed = new Set(allowedKeys);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(message);
    }
  }
}

function parseJsonPayload(json: string, context: string): unknown {
  try {
    return JSON.parse(json) as unknown;
  } catch (error) {
    throw new Error(`${context}: invalid JSON`);
  }
}

function requireRecord(value: unknown, message: string): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new Error(message);
  }
  return value;
}

function requireStringField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): string {
  const raw = value[field];
  if (typeof raw !== 'string' || raw.length === 0) {
    throw new Error(message);
  }
  return raw;
}

function optionalStringField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): string | undefined {
  const raw = value[field];
  if (raw == null) {
    return undefined;
  }
  if (typeof raw !== 'string') {
    throw new Error(message);
  }
  return raw;
}

function optionalNonEmptyStringField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): string | undefined {
  const raw = optionalStringField(value, field, message);
  if (raw === undefined) {
    return undefined;
  }
  if (raw.length === 0) {
    throw new Error(message);
  }
  return raw;
}

function requireNumberField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): number {
  const raw = value[field];
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    throw new Error(message);
  }
  return raw;
}

function requireNonNegativeIntegerField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): number {
  const raw = requireNumberField(value, field, message);
  if (!Number.isInteger(raw) || raw < 0) {
    throw new Error(message);
  }
  return raw;
}

function optionalNumberField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): number | undefined {
  const raw = value[field];
  if (raw == null) {
    return undefined;
  }
  if (typeof raw !== 'number' || !Number.isFinite(raw)) {
    throw new Error(message);
  }
  return raw;
}

function requireBooleanField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): boolean {
  const raw = value[field];
  if (typeof raw !== 'boolean') {
    throw new Error(message);
  }
  return raw;
}

function optionalRecordField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): Record<string, unknown> | undefined {
  const raw = value[field];
  if (raw == null) {
    return undefined;
  }
  return requireRecord(raw, message);
}

function parseResolvedModelCapabilities(
  raw: unknown,
): ResolvedModelCapabilities | undefined {
  if (raw == null) {
    return undefined;
  }
  const record = requireRecord(
    raw,
    'Invalid mob member_status response: resolved_capabilities must be object',
  );
  return {
    vision: Boolean(record.vision),
    image_input: Boolean(record.image_input),
    image_tool_results: Boolean(record.image_tool_results),
    inline_video: Boolean(record.inline_video),
    realtime: Boolean(record.realtime),
    web_search: Boolean(record.web_search),
    image_generation: Boolean(record.image_generation),
  };
}

function optionalStringArrayField(
  value: Record<string, unknown>,
  field: string,
  message: string,
): string[] | undefined {
  const raw = value[field];
  if (raw == null) {
    return undefined;
  }
  if (!Array.isArray(raw) || raw.some((entry) => typeof entry !== 'string')) {
    throw new Error(message);
  }
  return [...raw];
}

const WIRE_MOB_MEMBER_STATUSES: readonly WireMobMemberStatus[] = [
  'active',
  'retiring',
  'broken',
  'completed',
  'unknown',
];

function parseWireMobMemberStatus(raw: unknown, message: string): WireMobMemberStatus {
  if (
    typeof raw === 'string' &&
    WIRE_MOB_MEMBER_STATUSES.includes(raw as WireMobMemberStatus)
  ) {
    return raw as WireMobMemberStatus;
  }
  throw new Error(message);
}

const WIRE_HANDLING_MODES: readonly HandlingMode[] = ['queue', 'steer'];

function parseWireHandlingMode(raw: unknown, message: string): HandlingMode {
  if (typeof raw === 'string' && WIRE_HANDLING_MODES.includes(raw as HandlingMode)) {
    return raw as HandlingMode;
  }
  throw new Error(message);
}

export function parseMobStatusResult(
  raw: unknown,
  context = 'Invalid mob/status response',
): MobStatus {
  const record = requireRecord(raw, `${context}: malformed envelope`) as Partial<
    WireMobStatusResult
  > &
    Record<string, unknown>;
  const mobId = requireStringField(
    record,
    'mob_id',
    `${context}: missing mob_id`,
  );
  const status = requireStringField(
    record,
    'status',
    `${context}: missing status`,
  );
  return {
    mob_id: mobId,
    status,
    state: status,
  };
}

function parseMobAppendSystemContextResult(raw: unknown): MobAppendSystemContextResult {
  const record = requireRecord(
    raw,
    'Invalid mob append_system_context response: malformed envelope',
  ) as Partial<WireMobAppendSystemContextResult> & Record<string, unknown>;
  const status = requireStringField(
    record,
    'status',
    'Invalid mob append_system_context response: missing status',
  );
  if (status !== 'staged' && status !== 'duplicate') {
    throw new Error('Invalid mob append_system_context response: invalid status');
  }
  return {
    mob_id: requireStringField(
      record,
      'mob_id',
      'Invalid mob append_system_context response: missing mob_id',
    ),
    agent_identity: requireStringField(
      record,
      'agent_identity',
      'Invalid mob append_system_context response: missing agent_identity',
    ),
    status,
  };
}

function normalizeSpawnManyEntry(raw: unknown, mobId: string): SpawnResult {
  if (!isRecord(raw)) {
    throw new Error('Invalid mob spawn response: malformed result entry');
  }
  if ('ok' in raw) {
    throw new Error('Invalid mob spawn response: legacy ok result row');
  }
  requireOnlyKeys(raw, ['status', 'result'], 'Invalid mob spawn response: malformed result entry');

  const status = raw.status;
  if (status !== 'spawned' && status !== 'failed') {
    throw new Error('Invalid mob spawn response: invalid result status');
  }
  if (!isRecord(raw.result)) {
    throw new Error('Invalid mob spawn response: missing result payload');
  }

  if (status === 'failed') {
    requireOnlyKeys(
      raw.result,
      ['message'],
      'Invalid mob spawn response: malformed failed result payload',
    );
    const message = raw.result.message;
    if (typeof message !== 'string' || message.length === 0) {
      throw new Error('Invalid mob spawn response: failed result missing message');
    }
    throw new Error(`Mob spawn failed: ${message}`);
  }

  requireOnlyKeys(
    raw.result,
    ['agent_identity', 'member_ref'],
    'Invalid mob spawn response: malformed spawned result payload',
  );
  const agentIdentity = raw.result.agent_identity;
  const memberRef = raw.result.member_ref;
  if (typeof agentIdentity !== 'string' || agentIdentity.length === 0) {
    throw new Error('Invalid mob spawn response: spawned result missing agent_identity');
  }
  if (typeof memberRef !== 'string' || memberRef.length === 0) {
    throw new Error('Invalid mob spawn response: spawned result missing member_ref');
  }
  return {
    mob_id: mobId,
    agent_identity: agentIdentity,
    member_ref: memberRef,
  };
}

function parseSubscriptionLaggedEvent(
  record: Record<string, unknown>,
  context: string,
): SubscriptionLaggedEvent {
  return {
    type: 'lagged',
    skipped: requireNumberField(record, 'skipped', `${context}: lagged skipped must be number`),
  };
}

function normalizeEventSourceIdentity(raw: unknown, context: string): EventSourceIdentity {
  const source = requireRecord(raw, `${context}: missing source`);
  const sourceType = requireStringField(source, 'type', `${context}: source missing type`);
  switch (sourceType) {
    case 'session': {
      requireOnlyKeys(source, ['type', 'session_id', 'sessionId'], `${context}: malformed source`);
      const sessionId =
        typeof source.session_id === 'string' ? source.session_id : source.sessionId;
      if (typeof sessionId !== 'string' || sessionId.length === 0) {
        throw new Error(`${context}: source missing session_id`);
      }
      return { type: 'session', session_id: sessionId };
    }
    case 'runtime': {
      requireOnlyKeys(source, ['type', 'runtime_id', 'runtimeId'], `${context}: malformed source`);
      const runtimeId =
        typeof source.runtime_id === 'string' ? source.runtime_id : source.runtimeId;
      if (typeof runtimeId !== 'string' || runtimeId.length === 0) {
        throw new Error(`${context}: source missing runtime_id`);
      }
      return { type: 'runtime', runtime_id: runtimeId };
    }
    case 'interaction': {
      requireOnlyKeys(
        source,
        ['type', 'interaction_id', 'interactionId'],
        `${context}: malformed source`,
      );
      const interactionId =
        typeof source.interaction_id === 'string' ? source.interaction_id : source.interactionId;
      if (typeof interactionId !== 'string' || interactionId.length === 0) {
        throw new Error(`${context}: source missing interaction_id`);
      }
      return { type: 'interaction', interaction_id: interactionId };
    }
    case 'callback':
      requireOnlyKeys(source, ['type'], `${context}: malformed source`);
      return { type: 'callback' };
    case 'external': {
      requireOnlyKeys(source, ['type', 'source_id', 'sourceId'], `${context}: malformed source`);
      const sourceId = typeof source.source_id === 'string' ? source.source_id : source.sourceId;
      if (typeof sourceId !== 'string' || sourceId.length === 0) {
        throw new Error(`${context}: source missing source_id`);
      }
      return { type: 'external', source_id: sourceId };
    }
    default:
      throw new Error(`${context}: unsupported source type`);
  }
}

function parseEventPayload(raw: unknown, context: string): EventEnvelope['payload'] {
  const payload = requireRecord(raw, `${context}: missing payload`);
  requireStringField(payload, 'type', `${context}: payload missing type`);
  return payload as EventEnvelope['payload'];
}

function parseEventEnvelope(raw: unknown, context: string): EventEnvelope {
  const record = requireRecord(raw, `${context}: malformed envelope`);
  const payload = parseEventPayload(record.payload, context);
  const envelope: EventEnvelope = {
    event_id: requireStringField(
      record,
      'event_id',
      `${context}: missing event_id`,
    ),
    source: normalizeEventSourceIdentity(record.source, context),
    source_id: requireStringField(
      record,
      'source_id',
      `${context}: missing source_id`,
    ),
    seq: requireNumberField(record, 'seq', `${context}: missing seq`),
    timestamp_ms: requireNumberField(
      record,
      'timestamp_ms',
      `${context}: missing timestamp_ms`,
    ),
    payload,
  };
  const mobId = optionalNonEmptyStringField(record, 'mob_id', `${context}: mob_id must be string`);
  if (mobId !== undefined) envelope.mob_id = mobId;
  const agentIdentity = optionalNonEmptyStringField(
    record,
    'agent_identity',
    `${context}: agent_identity must be string`,
  );
  if (agentIdentity !== undefined) envelope.agent_identity = agentIdentity;
  const memberRef = optionalNonEmptyStringField(
    record,
    'member_ref',
    `${context}: member_ref must be string`,
  );
  if (memberRef !== undefined) envelope.member_ref = memberRef;
  const cursor = record.cursor;
  if (cursor != null) {
    if (typeof cursor !== 'string' && typeof cursor !== 'number') {
      throw new Error(`${context}: cursor must be string or number`);
    }
    envelope.cursor = cursor;
  }
  return envelope;
}

function parseMemberEventItem(raw: unknown, context: string): MemberEventItem {
  const record = requireRecord(raw, `${context}: malformed event item`);
  if (record.type === 'lagged') {
    return parseSubscriptionLaggedEvent(record, context);
  }
  return parseEventEnvelope(record, context);
}

function parseAttributedSource(raw: unknown, context: string): AgentRuntimeId {
  const source = requireRecord(raw, `${context}: missing source`);
  const identity = requireStringField(source, 'identity', `${context}: source missing identity`);
  const generation = requireNonNegativeIntegerField(
    source,
    'generation',
    `${context}: source generation must be a non-negative integer`,
  );
  return { identity, generation };
}

function parseAttributedEventItem(raw: unknown, context: string): AttributedEventItem {
  const record = requireRecord(raw, `${context}: malformed attributed event`);
  if (record.type === 'lagged') {
    return parseSubscriptionLaggedEvent(record, context);
  }
  const source = parseAttributedSource(record.source, context);
  const role = requireStringField(record, 'role', `${context}: missing role`);
  const attributed = {
    source,
    role,
    envelope: parseEventEnvelope(record.envelope, `${context}: envelope`),
  };
  const sourceFenceToken = optionalNumberField(
    record,
    'source_fence_token',
    `${context}: source_fence_token must be number`,
  );
  return sourceFenceToken === undefined
    ? attributed
    : { ...attributed, source_fence_token: sourceFenceToken };
}

function parseEventItems<T>(
  raw: unknown,
  context: string,
  parseItem: (item: unknown, itemContext: string) => T,
): T[] {
  if (!Array.isArray(raw)) {
    throw new Error(`${context}: events must be a list`);
  }
  return raw.map((item, index) => parseItem(item, `${context}[${index}]`));
}

function parseMobEvent(raw: unknown, context: string): MobEvent {
  const record = requireRecord(raw, `${context}: malformed mob event`);
  return {
    cursor: requireNumberField(record, 'cursor', `${context}: cursor must be number`),
    timestamp: requireStringField(record, 'timestamp', `${context}: missing timestamp`),
    mob_id: requireStringField(record, 'mob_id', `${context}: missing mob_id`),
    kind: requireRecord(record.kind, `${context}: missing kind`),
  };
}

function parseMobEvents(raw: unknown): MobEvent[] {
  return parseEventItems(raw, 'Invalid mob/events response', parseMobEvent);
}

function parseMobMemberSnapshot(raw: unknown, mobId: string, agentIdentity: string): MobMemberSnapshot {
  const snapshot = requireRecord(
    raw,
    'Invalid mob member_status response: malformed envelope',
  ) as Partial<WireMobMemberStatusResult> & Record<string, unknown>;
  const currentSessionId = optionalStringField(
    snapshot,
    'current_session_id',
    'Invalid mob member_status response: current_session_id must be string',
  );
  const result: MobMemberSnapshot = {
    status: parseWireMobMemberStatus(
      snapshot.status,
      'Invalid mob member_status response: missing status',
    ),
    member_ref: encodeMemberRef(mobId, agentIdentity),
    output_preview: optionalStringField(
      snapshot,
      'output_preview',
      'Invalid mob member_status response: output_preview must be string',
    ),
    error: optionalStringField(
      snapshot,
      'error',
      'Invalid mob member_status response: error must be string',
    ),
    tokens_used: requireNonNegativeIntegerField(
      snapshot,
      'tokens_used',
      'Invalid mob member_status response: tokens_used must be number',
    ),
    is_final: requireBooleanField(
      snapshot,
      'is_final',
      'Invalid mob member_status response: is_final must be boolean',
    ),
    kickoff: optionalRecordField(
      snapshot,
      'kickoff',
      'Invalid mob member_status response: kickoff must be object',
    ),
    peer_connectivity: optionalRecordField(
      snapshot,
      'peer_connectivity',
      'Invalid mob member_status response: peer_connectivity must be object',
    ) as MobMemberSnapshot['peer_connectivity'],
  };
  if (currentSessionId !== undefined) {
    result.current_session_id = currentSessionId;
  }
  const resolvedCapabilities = parseResolvedModelCapabilities(
    snapshot.resolved_capabilities,
  );
  if (resolvedCapabilities !== undefined) {
    result.resolved_capabilities = resolvedCapabilities;
  }
  if (Object.prototype.hasOwnProperty.call(snapshot, 'external_member')) {
    result.external_member = snapshot.external_member;
  }
  return result;
}

function parseMobRespawnResult(raw: unknown): MobRespawnResult {
  const result = requireRecord(raw, 'Invalid mob respawn response: malformed envelope') as Partial<
    WireMobRespawnResult
  > &
    Record<string, unknown>;
  const status = requireStringField(
    result,
    'status',
    'Invalid mob respawn response: missing status',
  );
  if (status !== 'completed' && status !== 'topology_restore_failed') {
    throw new Error('Invalid mob respawn response: invalid status');
  }
  const receipt = requireRecord(result.receipt, 'Invalid mob respawn response: missing receipt');
  const memberRef = requireStringField(
    receipt,
    'member_ref',
    'Invalid mob respawn response: receipt missing member_ref',
  );
  const identity = requireStringField(
    receipt,
    'identity',
    'Invalid mob respawn response: receipt missing identity',
  );
  return {
    status,
    receipt: {
      agent_identity: identity,
      member_ref: memberRef,
    },
    failed_peer_ids: optionalStringArrayField(
      result,
      'failed_peer_ids',
      'Invalid mob respawn response: failed_peer_ids must be string list',
    ),
  };
}

function parseMemberDeliveryReceipt(
  raw: unknown,
  expectedMobId: string,
  expectedAgentIdentity: string,
): MemberDeliveryReceipt {
  const receipt = requireRecord(
    raw,
    'Invalid mob member delivery response: malformed envelope',
  ) as Partial<WireMobMemberSendResult> & Record<string, unknown>;
  const mobId = requireStringField(
    receipt,
    'mob_id',
    'Invalid mob member delivery response: missing mob_id',
  );
  if (mobId !== expectedMobId) {
    throw new Error('Invalid mob member delivery response: mob_id mismatch');
  }
  const agentIdentity = requireStringField(
    receipt,
    'agent_identity',
    'Invalid mob member delivery response: missing agent_identity',
  );
  if (agentIdentity !== expectedAgentIdentity) {
    throw new Error('Invalid mob member delivery response: agent_identity mismatch');
  }
  return {
    agent_identity: agentIdentity,
    member_ref: requireStringField(
      receipt,
      'member_ref',
      'Invalid mob member delivery response: missing member_ref',
    ),
    handling_mode: parseWireHandlingMode(
      receipt.handling_mode,
      'Invalid mob member delivery response: missing handling_mode',
    ),
  };
}

function parseMobHelperResult(raw: unknown, context: string): MobHelperResult {
  const result = requireRecord(raw, `${context}: malformed envelope`) as Partial<
    WireMobHelperResult
  > &
    Record<string, unknown>;
  return {
    output: optionalStringField(result, 'output', `${context}: output must be string`),
    tokens_used: requireNonNegativeIntegerField(
      result,
      'tokens_used',
      `${context}: tokens_used must be number`,
    ),
    agent_identity: requireStringField(
      result,
      'agent_identity',
      `${context}: missing agent_identity`,
    ),
    member_ref: requireStringField(result, 'member_ref', `${context}: missing member_ref`),
  };
}

function parseMobFlowStatusResult(raw: unknown): FlowStatus | null {
  const context = 'Invalid mob flow_status response';
  const record = requireRecord(raw, `${context}: malformed envelope`) as Partial<
    WireMobFlowStatusResult
  > &
    Record<string, unknown>;
  if (!Object.prototype.hasOwnProperty.call(record, 'run')) {
    throw new Error(`${context}: missing run`);
  }
  if (record.run == null) {
    return null;
  }
  const run = requireRecord(record.run, `${context}: run must be object`);
  return {
    ...run,
    run_id: requireStringField(run, 'run_id', `${context}: run missing run_id`),
    status: requireStringField(run, 'status', `${context}: run missing status`),
  };
}

/** Capability-bearing handle for one mob member. */
export class Member {
  private mobId: string;
  private agentIdentity: string;
  private bindings: MobWasmBindings;

  constructor(mobId: string, agentIdentity: string, bindings: MobWasmBindings) {
    this.mobId = mobId;
    this.agentIdentity = agentIdentity;
    this.bindings = bindings;
  }

  async send(
    content: ContentInput,
    handlingMode: HandlingMode = 'queue',
    renderMetadata?: RenderMetadata,
  ): Promise<MemberDeliveryReceipt> {
    const json = await this.bindings.mob_member_send(
      this.mobId,
      this.agentIdentity,
      JSON.stringify({
        content,
        handling_mode: handlingMode,
        render_metadata: renderMetadata,
      }),
    );
    return parseMemberDeliveryReceipt(
      parseJsonPayload(json, 'Invalid mob member delivery response'),
      this.mobId,
      this.agentIdentity,
    );
  }

  async subscribe(): Promise<EventSubscription<MemberEventItem>> {
    const handle = await this.bindings.mob_member_subscribe(this.mobId, this.agentIdentity);
    return new EventSubscription<MemberEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) =>
        parseEventItems(raw, 'Invalid mob member subscription event', parseMemberEventItem),
      () => this.bindings.close_subscription(handle),
    );
  }
}

/** A mob instance — a group of agents with shared orchestration. */
export class Mob {
  /** The mob's unique identifier. */
  readonly mobId: string;

  private bindings: MobWasmBindings;

  /** @internal — use MeerkatRuntime.createMob() instead. */
  constructor(mobId: string, bindings: MobWasmBindings) {
    this.mobId = mobId;
    this.bindings = bindings;
  }

  /** Spawn one or more agents into the mob. */
  async spawn(specs: SpawnSpec[]): Promise<SpawnResult[]> {
    const json = await this.bindings.mob_spawn(
      this.mobId,
      JSON.stringify(specs.map(spawnSpecPayload)),
    );
    const parsed = parseJsonPayload(json, 'Invalid mob spawn response');
    if (!Array.isArray(parsed)) {
      throw new Error('Invalid mob spawn response: results must be a list');
    }
    return parsed.map((entry) => normalizeSpawnManyEntry(entry, this.mobId));
  }

  /** Retire an agent from the mob. */
  async retire(agentIdentity: string): Promise<void> {
    await this.bindings.mob_retire(this.mobId, agentIdentity);
  }

  /** Wire two agents for comms trust. */
  async wire(member: string, peer: MobPeerTarget): Promise<void> {
    if (typeof peer === 'string') {
      await this.bindings.mob_wire(this.mobId, member, peer);
      return;
    }
    const wirePeer = this.bindings.mob_wire_peer ?? this.bindings.mob_wire_target;
    if (!wirePeer) {
      throw new Error('This runtime does not support external peer wiring');
    }
    await wirePeer(this.mobId, member, JSON.stringify(peer));
  }

  /** Remove comms trust between two agents. */
  async unwire(member: string, peer: MobPeerTarget): Promise<void> {
    if (typeof peer === 'string') {
      await this.bindings.mob_unwire(this.mobId, member, peer);
      return;
    }
    const unwirePeer = this.bindings.mob_unwire_peer ?? this.bindings.mob_unwire_target;
    if (!unwirePeer) {
      throw new Error('This runtime does not support external peer unwiring');
    }
    await unwirePeer(this.mobId, member, JSON.stringify(peer));
  }

  /** List all members in the mob. */
  async listMembers(): Promise<MobMember[]> {
    const json = await this.bindings.mob_list_members(this.mobId);
    const parsed = parseJsonPayload(json, 'Invalid mob list_members response');
    if (!Array.isArray(parsed)) {
      throw new Error('Invalid mob list_members response: members must be a list');
    }
    return parsed.map((rawMember, index) => {
      const member = requireRecord(
        rawMember,
        `Invalid mob list_members entry ${index}: malformed member`,
      );
      if (typeof member.agent_identity !== 'string' || member.agent_identity.length === 0) {
        throw new Error('Invalid mob list_members entry: missing agent_identity');
      }
      const agentIdentity = member.agent_identity;
      const memberRef =
        typeof member.member_ref === 'string' && member.member_ref.length > 0
          ? member.member_ref
          : '';
      if (!memberRef) {
        throw new Error('Invalid mob list_members entry: missing member_ref');
      }
      const profile =
        typeof member.role === 'string' && member.role.length > 0
          ? member.role
          : typeof member.profile === 'string' && member.profile.length > 0
            ? member.profile
            : typeof member.profile_name === 'string' && member.profile_name.length > 0
              ? member.profile_name
              : undefined;
      if (!profile) {
        throw new Error('Invalid mob list_members entry: missing profile');
      }
      return {
        agent_identity: agentIdentity,
        member_ref: memberRef,
        profile,
        peer_id: member.peer_id != null ? String(member.peer_id) : undefined,
        external_peer_specs:
          member.external_peer_specs && typeof member.external_peer_specs === 'object'
            ? Object.fromEntries(
                Object.entries(member.external_peer_specs as Record<string, unknown>).map(
                  ([key, value]) => [key, (value ?? {}) as Record<string, unknown>],
                ),
              )
            : undefined,
        runtime_mode: member.runtime_mode != null ? String(member.runtime_mode) : undefined,
        state: member.state != null ? String(member.state) : undefined,
        wired_to: Array.isArray(member.wired_to)
          ? member.wired_to.map((peer) => String(peer))
          : undefined,
        labels:
          member.labels && typeof member.labels === 'object'
            ? Object.fromEntries(
                Object.entries(member.labels as Record<string, unknown>).map(([key, value]) => [key, String(value)]),
              )
            : undefined,
        status: member.status != null ? String(member.status) : undefined,
        error: member.error != null ? String(member.error) : undefined,
        is_final: member.is_final != null ? Boolean(member.is_final) : undefined,
        kickoff:
          member.kickoff && typeof member.kickoff === 'object'
            ? (member.kickoff as Record<string, unknown>)
            : undefined,
      };
    });
  }

  /** Stage runtime system context for a specific member session. */
  async appendSystemContext(
    agentIdentity: string,
    options: AppendSystemContextOptions,
  ): Promise<MobAppendSystemContextResult> {
    const json = await this.bindings.mob_append_system_context(
      this.mobId,
      agentIdentity,
      JSON.stringify({
        text: options.text,
        source: options.source,
        idempotency_key: options.idempotencyKey,
      }),
    );
    return parseMobAppendSystemContextResult(
      parseJsonPayload(json, 'Invalid mob append_system_context response'),
    );
  }

  /** Get a capability-bearing handle for one member. */
  member(agentIdentity: string): Member {
    return new Member(this.mobId, agentIdentity, this.bindings);
  }

  /** Retire and re-spawn an agent with the same profile. Returns a result envelope with receipt. */
  async respawn(
    agentIdentity: string,
    initialMessage?: string | ContentBlock[],
  ): Promise<MobRespawnResult> {
    const payload =
      initialMessage != null
        ? typeof initialMessage === 'string'
          ? initialMessage
          : JSON.stringify(initialMessage)
        : undefined;
    const json = await this.bindings.mob_respawn(this.mobId, agentIdentity, payload);
    return parseMobRespawnResult(parseJsonPayload(json, 'Invalid mob respawn response'));
  }

  /** Force-cancel an active member turn. */
  async forceCancel(agentIdentity: string): Promise<void> {
    await this.bindings.mob_force_cancel(this.mobId, agentIdentity);
  }

  /** Read the current execution snapshot for a member. */
  async memberStatus(agentIdentity: string): Promise<MobMemberSnapshot> {
    const json = await this.bindings.mob_member_status(this.mobId, agentIdentity);
    return parseMobMemberSnapshot(
      parseJsonPayload(json, 'Invalid mob member_status response'),
      this.mobId,
      agentIdentity,
    );
  }

  /** Spawn a short-lived helper and return its terminal result. */
  async spawnHelper(
    prompt: string,
    options?: {
      agentIdentity?: string;
      profileName?: string;
      authBinding?: AuthBindingRef;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<MobHelperResult> {
    const json = await this.bindings.mob_spawn_helper(
      this.mobId,
        JSON.stringify({
          prompt,
          agent_identity: options?.agentIdentity,
          profile_name: options?.profileName,
          auth_binding: options?.authBinding,
          runtime_mode: options?.runtimeMode,
          backend: options?.backend,
        }),
    );
    return parseMobHelperResult(
      parseJsonPayload(json, 'Invalid mob spawn_helper response'),
      'Invalid mob spawn_helper response',
    );
  }

  /** Fork a helper from an existing member and return its terminal result. */
  async forkHelper(
    sourceMemberId: string,
    prompt: string,
    options?: {
      agentIdentity?: string;
      profileName?: string;
      authBinding?: AuthBindingRef;
      forkContext?: Record<string, unknown>;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<MobHelperResult> {
    const json = await this.bindings.mob_fork_helper(
      this.mobId,
        JSON.stringify({
          source_member_id: sourceMemberId,
          prompt,
          agent_identity: options?.agentIdentity,
          profile_name: options?.profileName,
          auth_binding: options?.authBinding,
          fork_context: options?.forkContext,
          runtime_mode: options?.runtimeMode,
          backend: options?.backend,
        }),
    );
    return parseMobHelperResult(
      parseJsonPayload(json, 'Invalid mob fork_helper response'),
      'Invalid mob fork_helper response',
    );
  }

  /** Get mob status. */
  async status(): Promise<MobStatus> {
    const json = await this.bindings.mob_status(this.mobId);
    return parseMobStatusResult(parseJsonPayload(json, 'Invalid mob/status response'));
  }

  /** Perform a lifecycle action (stop, resume, complete, destroy). */
  async lifecycle(action: MobLifecycleAction): Promise<void> {
    await this.bindings.mob_lifecycle(this.mobId, action);
  }

  /** Get mob events after a cursor. */
  async events(afterCursor = '', limit = 100): Promise<MobEvent[]> {
    const numericCursor = afterCursor === '' ? 0 : Number(afterCursor);
    const json = await this.bindings.mob_events(this.mobId, Number.isFinite(numericCursor) ? numericCursor : 0, limit);
    return parseMobEvents(parseJsonPayload(json, 'Invalid mob/events response'));
  }

  /** Run a flow. Returns the run ID. */
  async runFlow(flowId: string, params: Record<string, unknown> = {}): Promise<string> {
    return this.bindings.mob_run_flow(
      this.mobId,
      flowId,
      JSON.stringify(params),
    );
  }

  /** Get flow status. */
  async flowStatus(runId: string): Promise<FlowStatus | null> {
    const json = await this.bindings.mob_flow_status(this.mobId, runId);
    return parseMobFlowStatusResult(parseJsonPayload(json, 'Invalid mob flow_status response'));
  }

  /** Cancel a running flow. */
  async cancelFlow(runId: string): Promise<void> {
    await this.bindings.mob_cancel_flow(this.mobId, runId);
  }

  /** Subscribe to events for a specific member. */
  async subscribeMemberEvents(agentIdentity: string): Promise<EventSubscription<MemberEventItem>> {
    const handle = await this.bindings.mob_member_subscribe(this.mobId, agentIdentity);
    return new EventSubscription<MemberEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) =>
        parseEventItems(raw, 'Invalid mob member subscription event', parseMemberEventItem),
      () => this.bindings.close_subscription(handle),
    );
  }

  /** Subscribe to all mob-wide attributed events. */
  async subscribeEvents(): Promise<EventSubscription<AttributedEventItem>> {
    const handle = await this.bindings.mob_subscribe_events(this.mobId);
    return new EventSubscription<AttributedEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) =>
        parseEventItems(raw, 'Invalid mob attributed subscription event', parseAttributedEventItem),
      () => this.bindings.close_subscription(handle),
    );
  }

}
