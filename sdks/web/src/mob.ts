import { EventSubscription } from './events.js';
import type {
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
} from './types.js';

// WASM function signatures (bound at construction)
interface MobWasmBindings {
  mob_spawn: (mobId: string, specs: string) => Promise<string>;
  mob_retire: (mobId: string, meerkatId: string) => Promise<void>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_unwire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_wire_target?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_unwire_target?: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_list_members: (mobId: string) => Promise<string>;
  mob_append_system_context: (
    mobId: string,
    meerkatId: string,
    requestJson: string,
  ) => Promise<string>;
  mob_member_send: (mobId: string, meerkatId: string, requestJson: string) => Promise<string>;
  mob_respawn: (mobId: string, meerkatId: string, initialMessage?: string) => Promise<string>;
  mob_status: (mobId: string) => Promise<string>;
  mob_lifecycle: (mobId: string, action: string) => Promise<void>;
  mob_events: (mobId: string, afterCursor: number, limit: number) => Promise<string>;
  mob_run_flow: (mobId: string, flowId: string, params: string) => Promise<string>;
  mob_flow_status: (mobId: string, runId: string) => Promise<string>;
  mob_cancel_flow: (mobId: string, runId: string) => Promise<void>;
  mob_member_subscribe: (mobId: string, meerkatId: string) => Promise<number>;
  mob_subscribe_events: (mobId: string) => Promise<number>;
  poll_subscription: (handle: number) => string;
  close_subscription: (handle: number) => void;
}

/** Capability-bearing handle for one mob member. */
export class Member {
  private mobId: string;
  private meerkatId: string;
  private bindings: MobWasmBindings;

  constructor(mobId: string, meerkatId: string, bindings: MobWasmBindings) {
    this.mobId = mobId;
    this.meerkatId = meerkatId;
    this.bindings = bindings;
  }

  async send(
    content: ContentInput,
    handlingMode: HandlingMode = 'queue',
    renderMetadata?: RenderMetadata,
  ): Promise<{ member_id: string; session_id: string; handling_mode: HandlingMode }> {
    const sessionId = await this.bindings.mob_member_send(
      this.mobId,
      this.meerkatId,
      JSON.stringify({
        content,
        handling_mode: handlingMode,
        render_metadata: renderMetadata,
      }),
    );
    if (typeof sessionId !== 'string' || sessionId.length === 0) {
      throw new Error('Invalid mob/send response: missing session_id');
    }
    return {
      member_id: this.meerkatId,
      session_id: sessionId,
      handling_mode: handlingMode,
    };
  }

  async subscribe(): Promise<EventSubscription<MemberEventItem>> {
    const handle = await this.bindings.mob_member_subscribe(this.mobId, this.meerkatId);
    return new EventSubscription<MemberEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) => Array.isArray(raw) ? (raw as MemberEventItem[]) : [],
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
      JSON.stringify(specs),
    );
    return JSON.parse(json) as SpawnResult[];
  }

  /** Retire an agent from the mob. */
  async retire(meerkatId: string): Promise<void> {
    await this.bindings.mob_retire(this.mobId, meerkatId);
  }

  /** Wire two agents for comms trust. */
  async wire(member: string, peer: MobPeerTarget): Promise<void> {
    if (typeof peer === 'string') {
      await this.bindings.mob_wire(this.mobId, member, peer);
      return;
    }
    if (!this.bindings.mob_wire_target) {
      throw new Error('This runtime does not support external peer wiring');
    }
    await this.bindings.mob_wire_target(this.mobId, member, JSON.stringify(peer));
  }

  /** Remove comms trust between two agents. */
  async unwire(member: string, peer: MobPeerTarget): Promise<void> {
    if (typeof peer === 'string') {
      await this.bindings.mob_unwire(this.mobId, member, peer);
      return;
    }
    if (!this.bindings.mob_unwire_target) {
      throw new Error('This runtime does not support external peer unwiring');
    }
    await this.bindings.mob_unwire_target(this.mobId, member, JSON.stringify(peer));
  }

  /** List all members in the mob. */
  async listMembers(): Promise<MobMember[]> {
    const json = await this.bindings.mob_list_members(this.mobId);
    return JSON.parse(json) as MobMember[];
  }

  /** Stage runtime system context for a specific member session. */
  async appendSystemContext(
    meerkatId: string,
    options: AppendSystemContextOptions,
  ): Promise<MobAppendSystemContextResult> {
    const json = await this.bindings.mob_append_system_context(
      this.mobId,
      meerkatId,
      JSON.stringify({
        text: options.text,
        source: options.source,
        idempotency_key: options.idempotencyKey,
      }),
    );
    return JSON.parse(json) as MobAppendSystemContextResult;
  }

  /** Get a capability-bearing handle for one member. */
  member(meerkatId: string): Member {
    return new Member(this.mobId, meerkatId, this.bindings);
  }

  /**
   * Compatibility facade for direct member turns.
   *
   * Canonical 0.5 callers should prefer `member(meerkatId).send(...)`. This
   * method intentionally adds no runtime semantics of its own and preserves the
   * legacy browser contract of returning the session ID.
   */
  async sendMessage(
    meerkatId: string,
    content: ContentInput,
    handlingMode: HandlingMode = 'queue',
    renderMetadata?: RenderMetadata,
  ): Promise<string> {
    const receipt = await this.member(meerkatId).send(
      content,
      handlingMode,
      renderMetadata,
    );
    return receipt.session_id;
  }

  /** Retire and re-spawn an agent with the same profile. Returns a result envelope with receipt. */
  async respawn(
    meerkatId: string,
    initialMessage?: string | ContentBlock[],
  ): Promise<Record<string, unknown>> {
    const payload =
      initialMessage != null
        ? typeof initialMessage === 'string'
          ? initialMessage
          : JSON.stringify(initialMessage)
        : undefined;
    const json = await this.bindings.mob_respawn(this.mobId, meerkatId, payload);
    return JSON.parse(json) as Record<string, unknown>;
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
  async subscribe(meerkatId: string): Promise<EventSubscription<MemberEventItem>> {
    const handle = await this.bindings.mob_member_subscribe(this.mobId, meerkatId);
    return new EventSubscription<MemberEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) => Array.isArray(raw) ? (raw as MemberEventItem[]) : [],
      () => this.bindings.close_subscription(handle),
    );
  }

  /** Subscribe to all mob-wide attributed events. */
  async subscribeAll(): Promise<EventSubscription<AttributedEventItem>> {
    const handle = await this.bindings.mob_subscribe_events(this.mobId);
    return new EventSubscription<AttributedEventItem>(
      () => this.bindings.poll_subscription(handle),
      (raw) => Array.isArray(raw) ? (raw as AttributedEventItem[]) : [],
      () => this.bindings.close_subscription(handle),
    );
  }

}
