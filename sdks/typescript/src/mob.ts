import { EventSubscription } from "./subscription.js";
import type {
  AgentEventEnvelope,
  AttributedMobEvent,
  ContentBlock,
  MobFlowStatus,
  MobLifecycleAction,
  MobMember,
  MobStatus,
  SpawnSpec,
} from "./types.js";
import type { MeerkatClient } from "./client.js";

export type MobHandlingMode = "queue" | "steer";
export type MobRenderClass =
  | "user_prompt"
  | "peer_message"
  | "peer_request"
  | "peer_response"
  | "external_event"
  | "flow_step"
  | "continuation"
  | "system_notice"
  | "tool_scope_notice"
  | "ops_progress";

export interface MobRenderMetadata extends Record<string, unknown> {
  class: MobRenderClass;
  salience?: "background" | "normal" | "important" | "urgent";
}

export interface MemberSendOptions {
  handlingMode?: MobHandlingMode;
  renderMetadata?: MobRenderMetadata;
}

export interface MemberDeliveryReceipt {
  memberId: string;
  sessionId: string;
  handlingMode: MobHandlingMode;
}

export interface MobRespawnResult {
  status: "completed" | "topology_restore_failed";
  receipt: Record<string, unknown>;
  failedPeerIds?: string[];
}

export interface MobMemberSnapshot {
  status: string;
  outputPreview?: string;
  error?: string;
  tokensUsed: number;
  isFinal: boolean;
  currentSessionId?: string;
  peerConnectivity?: {
    reachablePeerCount: number;
    unknownPeerCount: number;
    unreachablePeers: Array<{
      peer: string;
      reason?: string;
    }>;
  };
}

export interface ExternalPeerTarget {
  readonly external: {
    readonly name: string;
    readonly peer_id: string;
    readonly address: string;
  };
}

export type MobPeerTarget = string | ExternalPeerTarget;

export interface MobHelperResult {
  output?: string;
  tokensUsed: number;
  sessionId?: string;
}

export class Member {
  private readonly client: MeerkatClient;
  readonly mobId: string;
  readonly meerkatId: string;

  /** @internal */
  constructor(client: MeerkatClient, mobId: string, meerkatId: string) {
    this.client = client;
    this.mobId = mobId;
    this.meerkatId = meerkatId;
  }

  async send(
    content: string | ContentBlock[],
    options?: MemberSendOptions,
  ): Promise<MemberDeliveryReceipt> {
    return this.client.sendMobMemberContent(this.mobId, this.meerkatId, content, options);
  }

  async events(): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.client.subscribeMobMemberEvents(this.mobId, this.meerkatId);
  }
}

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

  async respawn(
    meerkatId: string,
    initialMessage?: string | ContentBlock[],
  ): Promise<MobRespawnResult> {
    return this.client.respawnMobMember(this.mobId, meerkatId, initialMessage);
  }

  async forceCancel(meerkatId: string): Promise<void> {
    await this.client.forceCancelMobMember(this.mobId, meerkatId);
  }

  async memberStatus(meerkatId: string): Promise<MobMemberSnapshot> {
    return this.client.mobMemberStatus(this.mobId, meerkatId);
  }

  async spawnHelper(
    prompt: string,
    options?: { meerkatId?: string; profileName?: string },
  ): Promise<MobHelperResult> {
    return this.client.spawnMobHelper(this.mobId, prompt, options);
  }

  async forkHelper(
    sourceMemberId: string,
    prompt: string,
    options?: { meerkatId?: string; profileName?: string; forkContext?: Record<string, unknown> },
  ): Promise<MobHelperResult> {
    return this.client.forkMobHelper(this.mobId, sourceMemberId, prompt, options);
  }

  async wire(member: string, peer: MobPeerTarget): Promise<void> {
    await this.client.wireMobMembers(this.mobId, member, peer);
  }

  async unwire(member: string, peer: MobPeerTarget): Promise<void> {
    await this.client.unwireMobMembers(this.mobId, member, peer);
  }

  async listMembers(): Promise<MobMember[]> {
    return this.client.listMobMembers(this.mobId);
  }

  member(meerkatId: string): Member {
    return new Member(this.client, this.mobId, meerkatId);
  }

  /**
   * Compatibility facade for direct member turns.
   *
   * Canonical 0.5 callers should prefer `member(meerkatId).send(...)`. This
   * method intentionally adds no runtime semantics of its own.
   */
  async sendMessage(
    meerkatId: string,
    content: string | ContentBlock[],
    options?: MemberSendOptions,
  ): Promise<void> {
    await this.member(meerkatId).send(content, options);
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
