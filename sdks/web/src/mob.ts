import { EventSubscription } from './events.js';
import type {
  ConnectionRef,
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
  MemberRespawnReceipt,
  MobRespawnResult,
  MobMemberSnapshot,
  MobHelperResult,
  EventEnvelope,
} from './types.js';

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

function normalizeEventEnvelope(raw: unknown, mobId: string): MemberEventItem {
  if (!raw || typeof raw !== 'object') {
    return raw as MemberEventItem;
  }
  const record = raw as Record<string, unknown>;
  if (record.type === 'lagged') {
    return raw as MemberEventItem;
  }
  const agentIdentity =
    typeof record.agent_identity === 'string' && record.agent_identity.length > 0
      ? record.agent_identity
      : undefined;
  return {
    agent_identity: agentIdentity,
    member_ref: agentIdentity ? encodeMemberRef(mobId, agentIdentity) : undefined,
    cursor:
      typeof record.cursor === 'string' || typeof record.cursor === 'number'
        ? record.cursor
        : undefined,
    event:
      record.event && typeof record.event === 'object'
        ? (record.event as EventEnvelope['event'])
        : { type: 'unknown' },
  };
}

function normalizeAttributedEvent(raw: unknown, mobId: string): AttributedEventItem {
  if (!raw || typeof raw !== 'object') {
    return raw as AttributedEventItem;
  }
  const record = raw as Record<string, unknown>;
  if (record.type === 'lagged') {
    return raw as AttributedEventItem;
  }
  const envelope = normalizeEventEnvelope(record.envelope, mobId);
  return {
    source: typeof record.source === 'string' ? record.source : '',
    role: typeof record.role === 'string' ? record.role : '',
    envelope: envelope && 'event' in envelope ? envelope : { event: { type: 'unknown' } },
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
    const receipt = JSON.parse(json) as Partial<MemberDeliveryReceipt>;
    const memberRef =
      typeof receipt.member_ref === 'string' && receipt.member_ref.length > 0
        ? receipt.member_ref
        : undefined;
    if (!memberRef) {
      throw new Error('Invalid mob member delivery response: missing member_ref');
    }
    if (typeof receipt.agent_identity !== 'string' || receipt.agent_identity.length === 0) {
      throw new Error('Invalid mob member delivery response: missing agent_identity');
    }
    return {
      agent_identity: receipt.agent_identity,
      member_ref: memberRef,
      handling_mode: receipt.handling_mode ?? handlingMode,
    };
  }

  async subscribe(): Promise<EventSubscription<MemberEventItem>> {
    const handle = await this.bindings.mob_member_subscribe(this.mobId, this.agentIdentity);
    return new EventSubscription<MemberEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) =>
        Array.isArray(raw)
          ? raw.map((item) => normalizeEventEnvelope(item, this.mobId))
          : [],
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
    return (JSON.parse(json) as Array<Partial<SpawnResult> & Record<string, unknown>>).map((entry) => {
      if (typeof entry.agent_identity !== 'string' || entry.agent_identity.length === 0) {
        throw new Error('Invalid mob spawn response: missing agent_identity');
      }
      if (typeof entry.member_ref !== 'string' || entry.member_ref.length === 0) {
        throw new Error('Invalid mob spawn response: missing member_ref');
      }
      return {
        mob_id: typeof entry.mob_id === 'string' ? entry.mob_id : this.mobId,
        agent_identity: entry.agent_identity,
        member_ref: entry.member_ref,
      };
    });
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
    return (JSON.parse(json) as Array<Record<string, unknown>>).map((member) => {
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
      return {
        agent_identity: agentIdentity,
        member_ref: memberRef,
        profile: String(member.profile_name ?? member.profile ?? member.role ?? ''),
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
    const result = JSON.parse(json) as Partial<MobAppendSystemContextResult>;
    return {
      mob_id: typeof result.mob_id === 'string' ? result.mob_id : this.mobId,
      agent_identity:
        typeof result.agent_identity === 'string' ? result.agent_identity : agentIdentity,
      status: result.status === 'duplicate' ? 'duplicate' : 'staged',
    };
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
    const result = JSON.parse(json) as Partial<MobRespawnResult> & {
      receipt?: Partial<MemberRespawnReceipt>;
      failed_peer_ids?: unknown;
    };
    if (!result.receipt || typeof result.receipt !== 'object') {
      throw new Error('Invalid mob respawn response: missing receipt');
    }
    const receipt = result.receipt as unknown as Partial<MemberRespawnReceipt> &
      Record<string, unknown>;
    const memberRef =
      typeof receipt.member_ref === 'string' && receipt.member_ref.length > 0
        ? receipt.member_ref
        : undefined;
    if (!memberRef) {
      throw new Error('Invalid mob respawn response: receipt missing member_ref');
    }
    if (typeof receipt.agent_identity !== 'string' || receipt.agent_identity.length === 0) {
      throw new Error('Invalid mob respawn response: receipt missing agent_identity');
    }
    return {
      status: result.status === 'topology_restore_failed' ? 'topology_restore_failed' : 'completed',
      receipt: {
        agent_identity: receipt.agent_identity,
        member_ref: memberRef,
      },
      failed_peer_ids: Array.isArray(result.failed_peer_ids)
        ? result.failed_peer_ids.map((peerId) => String(peerId))
        : undefined,
    };
  }

  /** Force-cancel an active member turn. */
  async forceCancel(agentIdentity: string): Promise<void> {
    await this.bindings.mob_force_cancel(this.mobId, agentIdentity);
  }

  /** Read the current execution snapshot for a member. */
  async memberStatus(agentIdentity: string): Promise<MobMemberSnapshot> {
    const json = await this.bindings.mob_member_status(this.mobId, agentIdentity);
    const snapshot = JSON.parse(json) as Record<string, unknown>;
    return {
      status: typeof snapshot.status === 'string' ? snapshot.status : 'unknown',
      member_ref: encodeMemberRef(this.mobId, agentIdentity),
      output_preview:
        typeof snapshot.output_preview === 'string' ? snapshot.output_preview : undefined,
      error: typeof snapshot.error === 'string' ? snapshot.error : undefined,
      tokens_used: typeof snapshot.tokens_used === 'number' ? snapshot.tokens_used : 0,
      is_final: Boolean(snapshot.is_final),
      kickoff:
        snapshot.kickoff && typeof snapshot.kickoff === 'object'
          ? (snapshot.kickoff as Record<string, unknown>)
          : undefined,
      peer_connectivity:
        snapshot.peer_connectivity && typeof snapshot.peer_connectivity === 'object'
          ? (snapshot.peer_connectivity as MobMemberSnapshot['peer_connectivity'])
          : undefined,
    };
  }

  /** Spawn a short-lived helper and return its terminal result. */
  async spawnHelper(
    prompt: string,
    options?: {
      agentIdentity?: string;
      profileName?: string;
      connectionRef?: ConnectionRef;
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
          connection_ref: options?.connectionRef,
          runtime_mode: options?.runtimeMode,
          backend: options?.backend,
        }),
    );
    const result = JSON.parse(json) as Partial<MobHelperResult>;
    const memberRef =
      typeof result.member_ref === 'string' && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new Error('Invalid mob spawn_helper response: missing member_ref');
    }
    if (typeof result.agent_identity !== 'string' || result.agent_identity.length === 0) {
      throw new Error('Invalid mob spawn_helper response: missing agent_identity');
    }
    return {
      output: typeof result.output === 'string' ? result.output : undefined,
      tokens_used: typeof result.tokens_used === 'number' ? result.tokens_used : 0,
      agent_identity: result.agent_identity,
      member_ref: memberRef,
    };
  }

  /** Fork a helper from an existing member and return its terminal result. */
  async forkHelper(
    sourceMemberId: string,
    prompt: string,
    options?: {
      agentIdentity?: string;
      profileName?: string;
      connectionRef?: ConnectionRef;
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
          connection_ref: options?.connectionRef,
          fork_context: options?.forkContext,
          runtime_mode: options?.runtimeMode,
          backend: options?.backend,
        }),
    );
    const result = JSON.parse(json) as Partial<MobHelperResult>;
    const memberRef =
      typeof result.member_ref === 'string' && result.member_ref.length > 0
        ? result.member_ref
        : undefined;
    if (!memberRef) {
      throw new Error('Invalid mob fork_helper response: missing member_ref');
    }
    if (typeof result.agent_identity !== 'string' || result.agent_identity.length === 0) {
      throw new Error('Invalid mob fork_helper response: missing agent_identity');
    }
    return {
      output: typeof result.output === 'string' ? result.output : undefined,
      tokens_used: typeof result.tokens_used === 'number' ? result.tokens_used : 0,
      agent_identity: result.agent_identity,
      member_ref: memberRef,
    };
  }

  /** Get mob status. */
  async status(): Promise<MobStatus> {
    const json = await this.bindings.mob_status(this.mobId);
    return JSON.parse(json) as MobStatus;
  }

  /** Perform a lifecycle action (stop, resume, complete, destroy). */
  async lifecycle(action: MobLifecycleAction): Promise<void> {
    await this.bindings.mob_lifecycle(this.mobId, action);
  }

  /** Get mob events after a cursor. */
  async events(afterCursor = '', limit = 100): Promise<MobEvent[]> {
    const numericCursor = afterCursor === '' ? 0 : Number(afterCursor);
    const json = await this.bindings.mob_events(this.mobId, Number.isFinite(numericCursor) ? numericCursor : 0, limit);
    return JSON.parse(json) as MobEvent[];
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
  async flowStatus(runId: string): Promise<FlowStatus> {
    const json = await this.bindings.mob_flow_status(this.mobId, runId);
    return JSON.parse(json) as FlowStatus;
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
        Array.isArray(raw)
          ? raw.map((item) => normalizeEventEnvelope(item, this.mobId))
          : [],
      () => this.bindings.close_subscription(handle),
    );
  }

  /** Subscribe to all mob-wide attributed events. */
  async subscribeEvents(): Promise<EventSubscription<AttributedEventItem>> {
    const handle = await this.bindings.mob_subscribe_events(this.mobId);
    return new EventSubscription<AttributedEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) =>
        Array.isArray(raw)
          ? raw.map((item) => normalizeAttributedEvent(item, this.mobId))
          : [],
      () => this.bindings.close_subscription(handle),
    );
  }

}
