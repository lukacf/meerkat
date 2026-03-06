import { EventSubscription } from './events.js';
import type {
  SpawnSpec,
  SpawnResult,
  MobMember,
  MobStatus,
  MobLifecycleAction,
  FlowStatus,
  EventEnvelope,
  AppendSystemContextOptions,
  MobAppendSystemContextResult,
} from './types.js';

// WASM function signatures (bound at construction)
interface MobWasmBindings {
  mob_spawn: (mobId: string, specs: string) => Promise<string>;
  mob_retire: (mobId: string, meerkatId: string) => Promise<void>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_unwire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_list_members: (mobId: string) => Promise<string>;
  mob_append_system_context: (
    mobId: string,
    meerkatId: string,
    requestJson: string,
  ) => Promise<string>;
  mob_send_message: (mobId: string, meerkatId: string, message: string) => Promise<void>;
  mob_respawn: (mobId: string, meerkatId: string, initialMessage?: string) => Promise<void>;
  mob_status: (mobId: string) => Promise<string>;
  mob_lifecycle: (mobId: string, action: string) => Promise<void>;
  mob_events: (mobId: string, afterCursor: string, limit: number) => Promise<string>;
  mob_run_flow: (mobId: string, flowId: string, params: string) => Promise<string>;
  mob_flow_status: (mobId: string, runId: string) => Promise<string>;
  mob_cancel_flow: (mobId: string, runId: string) => Promise<void>;
  mob_member_subscribe: (mobId: string, meerkatId: string) => number;
  mob_subscribe_events: (mobId: string) => number;
  mob_inject_and_subscribe: (mobId: string, meerkatId: string, message: string) => Promise<string>;
  poll_subscription: (handle: number) => string;
  close_subscription: (handle: number) => void;
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
  async wire(a: string, b: string): Promise<void> {
    await this.bindings.mob_wire(this.mobId, a, b);
  }

  /** Remove comms trust between two agents. */
  async unwire(a: string, b: string): Promise<void> {
    await this.bindings.mob_unwire(this.mobId, a, b);
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

  /** Send a message to a specific agent. */
  async sendMessage(meerkatId: string, message: string): Promise<void> {
    await this.bindings.mob_send_message(this.mobId, meerkatId, message);
  }

  /** Retire and re-spawn an agent with the same profile. */
  async respawn(meerkatId: string, initialMessage?: string): Promise<void> {
    await this.bindings.mob_respawn(this.mobId, meerkatId, initialMessage);
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
  async events(afterCursor = '', limit = 100): Promise<EventEnvelope[]> {
    const json = await this.bindings.mob_events(this.mobId, afterCursor, limit);
    return JSON.parse(json) as EventEnvelope[];
  }

  /** Run a flow. Returns the run ID. */
  async runFlow(flowId: string, params: Record<string, unknown> = {}): Promise<string> {
    const json = await this.bindings.mob_run_flow(
      this.mobId,
      flowId,
      JSON.stringify(params),
    );
    return JSON.parse(json) as string;
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
  subscribe(meerkatId: string): EventSubscription {
    const handle = this.bindings.mob_member_subscribe(this.mobId, meerkatId);
    return new EventSubscription(
      handle,
      this.bindings.poll_subscription,
      this.bindings.close_subscription,
    );
  }

  /** Subscribe to all mob-wide events. */
  subscribeAll(): EventSubscription {
    const handle = this.bindings.mob_subscribe_events(this.mobId);
    return new EventSubscription(
      handle,
      this.bindings.poll_subscription,
      this.bindings.close_subscription,
    );
  }

  /** Send a message and subscribe to the interaction. Returns the interaction ID. */
  async injectAndSubscribe(
    meerkatId: string,
    message: string,
  ): Promise<{ interactionId: string; subscription: EventSubscription }> {
    const json = await this.bindings.mob_inject_and_subscribe(
      this.mobId,
      meerkatId,
      message,
    );
    const interactionId = JSON.parse(json) as string;
    // Subscribe to the member's events to follow the interaction
    const subscription = this.subscribe(meerkatId);
    return { interactionId, subscription };
  }
}
