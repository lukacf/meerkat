import { EventSubscription } from "./subscription.js";
import type {
  AgentEventEnvelope,
  AttributedMobEvent,
  MobFlowStatus,
  MobLifecycleAction,
  MobMember,
  MobStatus,
  SpawnSpec,
} from "./types.js";
import type { MeerkatClient } from "./client.js";

export class Mob {
  private readonly client: MeerkatClient;
  readonly mobId: string;

  /** @internal */
  constructor(client: MeerkatClient, mobId: string) {
    this.client = client;
    this.mobId = mobId;
  }

  async status(): Promise<MobStatus> {
    return this.client.mobStatus(this.mobId);
  }

  async lifecycle(action: MobLifecycleAction): Promise<void> {
    await this.client.mobLifecycle(this.mobId, action);
  }

  async spawn(spec: SpawnSpec): Promise<Record<string, unknown>> {
    return this.client.spawnMobMember(this.mobId, spec);
  }

  async retire(meerkatId: string): Promise<void> {
    await this.client.retireMobMember(this.mobId, meerkatId);
  }

  async respawn(meerkatId: string, initialMessage?: string): Promise<Record<string, unknown>> {
    return this.client.respawnMobMember(this.mobId, meerkatId, initialMessage);
  }

  async forceCancel(meerkatId: string): Promise<void> {
    await this.client.forceCancelMobMember(this.mobId, meerkatId);
  }

  async memberStatus(meerkatId: string): Promise<Record<string, unknown>> {
    return this.client.mobMemberStatus(this.mobId, meerkatId);
  }

  async spawnHelper(
    prompt: string,
    options?: { meerkatId?: string; profileName?: string },
  ): Promise<Record<string, unknown>> {
    return this.client.spawnMobHelper(this.mobId, prompt, options);
  }

  async forkHelper(
    sourceMemberId: string,
    prompt: string,
    options?: { meerkatId?: string; profileName?: string; forkContext?: Record<string, unknown> },
  ): Promise<Record<string, unknown>> {
    return this.client.forkMobHelper(this.mobId, sourceMemberId, prompt, options);
  }

  async wire(a: string, b: string): Promise<void> {
    await this.client.wireMobMembers(this.mobId, a, b);
  }

  async unwire(a: string, b: string): Promise<void> {
    await this.client.unwireMobMembers(this.mobId, a, b);
  }

  async listMembers(): Promise<MobMember[]> {
    return this.client.listMobMembers(this.mobId);
  }

  async sendMessage(meerkatId: string, message: string): Promise<void> {
    await this.client.sendMobMessage(this.mobId, meerkatId, message);
  }

  async appendSystemContext(
    meerkatId: string,
    text: string,
    options?: { source?: string; idempotencyKey?: string },
  ): Promise<Record<string, unknown>> {
    return this.client.appendMobSystemContext(this.mobId, meerkatId, text, options);
  }

  async listFlows(): Promise<string[]> {
    return this.client.listMobFlows(this.mobId);
  }

  async runFlow(flowId: string, params: Record<string, unknown> = {}): Promise<string> {
    return this.client.runMobFlow(this.mobId, flowId, params);
  }

  async flowStatus(runId: string): Promise<MobFlowStatus | null> {
    return this.client.getMobFlowStatus(this.mobId, runId);
  }

  async cancelFlow(runId: string): Promise<void> {
    await this.client.cancelMobFlow(this.mobId, runId);
  }

  async subscribeMemberEvents(meerkatId: string): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.client.subscribeMobMemberEvents(this.mobId, meerkatId);
  }

  async subscribeEvents(): Promise<EventSubscription<AttributedMobEvent>> {
    return this.client.subscribeMobEvents(this.mobId);
  }
}
