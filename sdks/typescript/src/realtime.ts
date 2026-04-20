import type {
  RealtimeCapabilitiesResult,
  RealtimeClientFrame,
  RealtimeOpenInfo,
  RealtimeOpenRequest,
  RealtimeReconnectPolicy,
  RealtimeServerFrame,
  RealtimeStatusResult,
} from "./generated/types.js";
import type { MeerkatClient } from "./client.js";
import { MeerkatError } from "./generated/errors.js";
import { AsyncQueue } from "./streaming.js";
import WebSocket, { type RawData } from "ws";

export interface RealtimeSessionTarget {
  readonly type: "session_target";
  readonly session_id: string;
}

// Phase 5G/T5i: `mob_member_target` was removed from the realtime wire
// contract. SDK callers can still construct channels via
// `RealtimeChannel.mobMember(...)` for ergonomics; the helper resolves
// `mob/member_status → current_session_id` internally and opens a
// `session_target`. `RealtimeChannelTarget` is the single wire-level
// shape that crosses the RPC boundary.
export type RealtimeChannelTarget = RealtimeSessionTarget;

interface MobMemberBinding {
  readonly mob_id: string;
  readonly agent_identity: string;
}

export interface RealtimeChannelOptions {
  readonly role?: "primary" | "observer";
  readonly turningMode?: "provider_managed" | "explicit_commit";
  readonly reconnectPolicy?: RealtimeReconnectPolicy;
}

export class RealtimeConnection {
  private readonly socket: WebSocket;
  private readonly frames = new AsyncQueue<RealtimeServerFrame | null>();

  constructor(socket: WebSocket) {
    this.socket = socket;
    this.socket.on("message", (data: RawData) => {
      const payload =
        typeof data === "string"
          ? data
          : (Buffer.isBuffer(data) ? data.toString("utf8") : String(data));
      this.frames.put(JSON.parse(payload) as RealtimeServerFrame);
    });
    this.socket.on("close", () => {
      this.frames.put(null);
    });
  }

  static async open(
    openInfo: RealtimeOpenInfo,
    options?: Pick<RealtimeChannelOptions, "role" | "turningMode">,
  ): Promise<RealtimeConnection> {
    const role = options?.role ?? "primary";
    const turningMode = options?.turningMode ?? "provider_managed";
    const socket = await new Promise<WebSocket>((resolve, reject) => {
      const ws = new WebSocket(openInfo.ws_url);
      ws.once("open", () => resolve(ws));
      ws.once("error", (error: Error) => reject(error));
    });

    const connection = new RealtimeConnection(socket);
    await connection.sendFrame({
      type: "channel.open",
      protocol_version: openInfo.default_protocol_version,
      open_token: openInfo.open_token,
      role,
      turning_mode: turningMode,
    } as RealtimeClientFrame);

    const opened = await connection.nextFrame();
    if (opened === null) {
      throw new MeerkatError(
        "REALTIME_OPEN_FAILED",
        "realtime websocket closed before channel.open completed",
      );
    }
    if ((opened as Record<string, unknown>).type === "channel.error") {
      throw new MeerkatError(
        "REALTIME_OPEN_FAILED",
        String((opened as Record<string, unknown>).message ?? "realtime websocket rejected channel.open"),
      );
    }
    if ((opened as Record<string, unknown>).type !== "channel.opened") {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        `Expected channel.opened after channel.open, got ${String((opened as Record<string, unknown>).type)}`,
      );
    }

    return connection;
  }

  async sendFrame(frame: RealtimeClientFrame): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.socket.send(JSON.stringify(frame), (error?: Error) => {
        if (error) {
          reject(error);
          return;
        }
        resolve();
      });
    });
  }

  async sendInput(chunk: Record<string, unknown>): Promise<void> {
    await this.sendFrame({
      type: "channel.input",
      chunk,
    } as RealtimeClientFrame);
  }

  async commitTurn(): Promise<void> {
    await this.sendFrame({ type: "channel.commit_turn" } as RealtimeClientFrame);
  }

  async interrupt(): Promise<void> {
    await this.sendFrame({ type: "channel.interrupt" } as RealtimeClientFrame);
  }

  async close(): Promise<void> {
    await this.sendFrame({ type: "channel.close" } as RealtimeClientFrame);
  }

  async nextFrame(): Promise<RealtimeServerFrame | null> {
    return this.frames.get();
  }
}

export class RealtimeChannel {
  readonly client: MeerkatClient;
  // `null` until a pending mob-member binding resolves into a concrete
  // `session_target` on the first wire call.
  private resolvedTarget: RealtimeSessionTarget | null;
  private readonly mobBinding: MobMemberBinding | null;
  readonly role: "primary" | "observer";
  readonly turningMode: "provider_managed" | "explicit_commit";
  readonly reconnectPolicy?: RealtimeReconnectPolicy;

  private constructor(
    client: MeerkatClient,
    init:
      | { kind: "session"; target: RealtimeSessionTarget }
      | { kind: "mob_member"; binding: MobMemberBinding },
    options?: RealtimeChannelOptions,
  ) {
    this.client = client;
    if (init.kind === "session") {
      this.resolvedTarget = init.target;
      this.mobBinding = null;
    } else {
      this.resolvedTarget = null;
      this.mobBinding = init.binding;
    }
    this.role = options?.role ?? "primary";
    this.turningMode = options?.turningMode ?? "provider_managed";
    this.reconnectPolicy = options?.reconnectPolicy;
  }

  /**
   * The wire target for this channel. Only available after a mob-member
   * binding has been resolved — use {@link openInfo}, {@link status},
   * {@link capabilities}, or {@link connect} to trigger resolution.
   */
  get target(): RealtimeSessionTarget | null {
    return this.resolvedTarget;
  }

  static session(
    client: MeerkatClient,
    sessionId: string,
    options?: RealtimeChannelOptions,
  ): RealtimeChannel {
    return new RealtimeChannel(
      client,
      {
        kind: "session",
        target: { type: "session_target", session_id: sessionId },
      },
      options,
    );
  }

  static mobMember(
    client: MeerkatClient,
    mobId: string,
    agentIdentity: string,
    options?: RealtimeChannelOptions,
  ): RealtimeChannel {
    // Phase 5G/T5i: defer mob-member → session resolution until the
    // first wire call. The channel target that crosses the RPC
    // boundary is always `session_target`.
    return new RealtimeChannel(
      client,
      {
        kind: "mob_member",
        binding: { mob_id: mobId, agent_identity: agentIdentity },
      },
      options,
    );
  }

  private async resolveTarget(): Promise<RealtimeSessionTarget> {
    if (this.resolvedTarget !== null) {
      return this.resolvedTarget;
    }
    const binding = this.mobBinding;
    if (binding === null) {
      throw new MeerkatError(
        "INVALID_REQUEST",
        "RealtimeChannel has no target or mob binding to resolve",
      );
    }
    const status = await this.client.mobMemberStatus(
      binding.mob_id,
      binding.agent_identity,
    );
    const sessionId = status.currentSessionId;
    if (typeof sessionId !== "string" || sessionId.length === 0) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        `mob/member_status did not surface current_session_id for mob ` +
          `'${binding.mob_id}' agent '${binding.agent_identity}'; ` +
          `the member may not yet be bound to a session`,
      );
    }
    const resolved: RealtimeSessionTarget = {
      type: "session_target",
      session_id: sessionId,
    };
    this.resolvedTarget = resolved;
    return resolved;
  }

  private wireTargetFrom(target: RealtimeSessionTarget): Record<string, unknown> {
    return { ...target };
  }

  private wireReconnectPolicy(): Record<string, unknown> | undefined {
    if (!this.reconnectPolicy) {
      return undefined;
    }
    return { ...this.reconnectPolicy };
  }

  openRequest(): RealtimeOpenRequest {
    if (this.resolvedTarget === null) {
      throw new MeerkatError(
        "INVALID_REQUEST",
        "RealtimeChannel.openRequest() requires a resolved target; " +
          "await openInfo() / status() / capabilities() / connect() first " +
          "so the mob-member binding resolves to a session_target.",
      );
    }
    return {
      target: this.wireTargetFrom(this.resolvedTarget),
      role: this.role,
      turning_mode: this.turningMode,
      reconnect_policy: this.wireReconnectPolicy(),
    };
  }

  async openInfo(): Promise<RealtimeOpenInfo> {
    const target = await this.resolveTarget();
    return this.client.realtimeOpenInfo({
      target: this.wireTargetFrom(target),
      role: this.role,
      turning_mode: this.turningMode,
      reconnect_policy: this.wireReconnectPolicy(),
    });
  }

  async status(): Promise<RealtimeStatusResult> {
    const target = await this.resolveTarget();
    return this.client.realtimeStatus({ target: this.wireTargetFrom(target) });
  }

  async capabilities(): Promise<RealtimeCapabilitiesResult> {
    const target = await this.resolveTarget();
    return this.client.realtimeCapabilities({ target: this.wireTargetFrom(target) });
  }

  async connectWithOpenInfo(openInfo: RealtimeOpenInfo): Promise<RealtimeConnection> {
    const target = await this.resolveTarget();
    if (JSON.stringify(openInfo.target) !== JSON.stringify(this.wireTargetFrom(target))) {
      throw new MeerkatError(
        "INVALID_RESPONSE",
        "realtime/open_info returned a target that does not match the requested channel target",
      );
    }
    return RealtimeConnection.open(openInfo, {
      role: this.role,
      turningMode: this.turningMode,
    });
  }

  async connect(): Promise<RealtimeConnection> {
    return this.connectWithOpenInfo(await this.openInfo());
  }
}
