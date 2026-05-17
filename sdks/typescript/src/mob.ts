import { EventSubscription } from "./subscription.js";
import type {
  AgentEventEnvelope,
  AttributedMobEvent,
  ContentBlock,
  MobEventsOptions,
  MobEventsResult,
  MobFlowStatus,
  MobLifecycleAction,
  MobMember,
  MobMemberRef,
  MobWireMembersBatchEdgeInput,
  MobWireMembersBatchResult,
  ResolvedModelCapabilities,
  MobSpawnResult,
  MobStatus,
  SpawnManySpec,
  SpawnSpec,
} from "./types.js";
import type { MobSpawnManyResultEntry } from "./generated/types.js";
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
  agentIdentity: string;
  memberRef: MobMemberRef;
  handlingMode: MobHandlingMode;
}

export interface MemberRespawnReceipt {
  agentIdentity: string;
  memberRef: MobMemberRef;
}

export interface MobRespawnResult {
  status: "completed" | "topology_restore_failed";
  receipt: MemberRespawnReceipt;
  failedPeerIds?: string[];
}

export interface MobMemberSnapshot {
  status: string;
  outputPreview?: string;
  error?: string;
  tokensUsed: number;
  isFinal: boolean;
  currentSessionId?: string;
  liveAttachmentStatus?: "unattached" | "intent_present_unbound" | "binding_not_ready" | "binding_ready" | "replacement_pending" | "reattach_required";
  resolvedCapabilities?: ResolvedModelCapabilities;
  peerConnectivity?: {
    reachablePeerCount: number;
    unknownPeerCount: number;
    unreachablePeers: Array<{
      peer: string;
      reason?: string;
    }>;
  };
}

export interface MobKickoffWaitOptions {
  memberIds?: string[];
  timeoutMs?: number;
}

export interface MobKickoffMemberSnapshot extends MobMemberSnapshot {
  agentIdentity: string;
}

export type MobReadyWaitOptions = MobKickoffWaitOptions;
export type MobReadyMemberSnapshot = MobKickoffMemberSnapshot;

export interface ExternalPeerTarget {
  readonly external: {
    readonly name: string;
    readonly address: string;
    readonly identity: {
      readonly kind: "ed25519_public_key";
      readonly public_key: string;
    };
  };
}

export type MobPeerTarget = string | ExternalPeerTarget;

export interface MobHelperResult {
  output?: string;
  tokensUsed: number;
  agentIdentity: string;
  memberRef: MobMemberRef;
}

export class Member {
  private readonly client: MeerkatClient;
  readonly mobId: string;
  readonly agentIdentity: string;

  /** @internal */
  constructor(client: MeerkatClient, mobId: string, agentIdentity: string) {
    this.client = client;
    this.mobId = mobId;
    this.agentIdentity = agentIdentity;
  }

  async send(
    content: string | ContentBlock[],
    options?: MemberSendOptions,
  ): Promise<MemberDeliveryReceipt> {
    return this.client.sendMobMemberContent(this.mobId, this.agentIdentity, content, options);
  }

  async events(): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.client.subscribeMobMemberEvents(this.mobId, this.agentIdentity);
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

  async spawn(spec: SpawnSpec): Promise<MobSpawnResult> {
    return this.client.spawnMobMember(this.mobId, spec);
  }

  async spawnMany(specs: SpawnManySpec[]): Promise<MobSpawnManyResultEntry[]> {
    return this.client.spawnMobMembers(this.mobId, specs);
  }

  async retire(agentIdentity: string): Promise<void> {
    await this.client.retireMobMember(this.mobId, agentIdentity);
  }

  async respawn(
    agentIdentity: string,
    initialMessage?: string | ContentBlock[],
  ): Promise<MobRespawnResult> {
    return this.client.respawnMobMember(this.mobId, agentIdentity, initialMessage);
  }

  async forceCancel(agentIdentity: string): Promise<void> {
    await this.client.forceCancelMobMember(this.mobId, agentIdentity);
  }

  async memberStatus(agentIdentity: string): Promise<MobMemberSnapshot> {
    return this.client.mobMemberStatus(this.mobId, agentIdentity);
  }

  async waitForKickoffComplete(
    options?: MobKickoffWaitOptions,
  ): Promise<MobKickoffMemberSnapshot[]> {
    return this.client.waitMobKickoff(this.mobId, options);
  }

  async waitForReady(
    options?: MobReadyWaitOptions,
  ): Promise<MobReadyMemberSnapshot[]> {
    return this.client.waitMobReady(this.mobId, options);
  }

  async spawnHelper(
    prompt: string,
    options?: { agentIdentity?: string; roleName?: string; profileName?: string },
  ): Promise<MobHelperResult> {
    return this.client.spawnMobHelper(this.mobId, prompt, options);
  }

  async forkHelper(
    sourceMemberId: string,
    prompt: string,
    options?: {
      agentIdentity?: string;
      roleName?: string;
      profileName?: string;
      forkContext?: Record<string, unknown>;
    },
  ): Promise<MobHelperResult> {
    return this.client.forkMobHelper(this.mobId, sourceMemberId, prompt, options);
  }

  async wire(member: string, peer: MobPeerTarget): Promise<void> {
    await this.client.wireMobMembers(this.mobId, member, peer);
  }

  async wireMembersBatch(
    edges: readonly MobWireMembersBatchEdgeInput[],
  ): Promise<MobWireMembersBatchResult> {
    return this.client.mobWireMembersBatch(this.mobId, edges);
  }

  async unwire(member: string, peer: MobPeerTarget): Promise<void> {
    await this.client.unwireMobMembers(this.mobId, member, peer);
  }

  async listMembers(): Promise<MobMember[]> {
    return this.client.listMobMembers(this.mobId);
  }

  member(agentIdentity: string): Member {
    return new Member(this.client, this.mobId, agentIdentity);
  }

  async appendSystemContext(
    agentIdentity: string,
    text: string,
    options?: { source?: string; idempotencyKey?: string },
  ): Promise<Record<string, unknown>> {
    return this.client.appendMobSystemContext(this.mobId, agentIdentity, text, options);
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

  async subscribeMemberEvents(agentIdentity: string): Promise<EventSubscription<AgentEventEnvelope>> {
    return this.client.subscribeMobMemberEvents(this.mobId, agentIdentity);
  }

  async subscribeEvents(): Promise<EventSubscription<AttributedMobEvent>> {
    return this.client.subscribeMobEvents(this.mobId);
  }

  async readEvents(options?: MobEventsOptions): Promise<MobEventsResult> {
    return this.client.readMobEvents(this.mobId, options);
  }
}
