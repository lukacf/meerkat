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

export interface RealtimeMobMemberTarget {
  readonly type: "mob_member";
  readonly mob_id: string;
  readonly agent_identity: string;
}

// W3-H: `RealtimeChannelTarget` carries identity as a first-class wire
// fact for mob-member channels via the `mob_member` variant. The server
// resolves `(mob_id, agent_identity)` against the MobMachine's
// canonical `member_realtime_bindings` map on every tick, so respawn
// atomically rotates the bound session without any SDK round-trip and
// without any client-side session-id pin. A terminal retire surfaces
// as `RealtimeErrorCode::BindingReleased`.
export type RealtimeChannelTarget =
  | RealtimeSessionTarget
  | RealtimeMobMemberTarget;

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
  // The wire target that crosses the RPC boundary. Carries identity
  // directly for `mob_member` channels (W3-H); the server resolves the
  // current bridge session on every tick from the MobMachine's
  // canonical binding map, so the SDK never pins a session id.
  readonly target: RealtimeChannelTarget;
  readonly role: "primary" | "observer";
  readonly turningMode: "provider_managed" | "explicit_commit";
  readonly reconnectPolicy?: RealtimeReconnectPolicy;

  private constructor(
    client: MeerkatClient,
    target: RealtimeChannelTarget,
    options?: RealtimeChannelOptions,
  ) {
    this.client = client;
    this.target = target;
    this.role = options?.role ?? "primary";
    this.turningMode = options?.turningMode ?? "provider_managed";
    this.reconnectPolicy = options?.reconnectPolicy;
  }

  static session(
    client: MeerkatClient,
    sessionId: string,
    options?: RealtimeChannelOptions,
  ): RealtimeChannel {
    return new RealtimeChannel(
      client,
      { type: "session_target", session_id: sessionId },
      options,
    );
  }

  static mobMember(
    client: MeerkatClient,
    mobId: string,
    agentIdentity: string,
    options?: RealtimeChannelOptions,
  ): RealtimeChannel {
    // W3-H: identity is a first-class wire fact. The channel target is
    // `mob_member { mob_id, agent_identity }`; the server resolves the
    // current bridge session from the MobMachine's canonical binding
    // map on every tick, so respawn rotates the bound session without
    // any SDK round-trip or session-id pin.
    return new RealtimeChannel(
      client,
      { type: "mob_member", mob_id: mobId, agent_identity: agentIdentity },
      options,
    );
  }

  private wireReconnectPolicy(): Record<string, unknown> | undefined {
    if (!this.reconnectPolicy) {
      return undefined;
    }
    return { ...this.reconnectPolicy };
  }

  openRequest(): RealtimeOpenRequest {
    return {
      target: { ...this.target },
      role: this.role,
      turning_mode: this.turningMode,
      reconnect_policy: this.wireReconnectPolicy(),
    };
  }

  async openInfo(): Promise<RealtimeOpenInfo> {
    return this.client.realtimeOpenInfo(this.openRequest());
  }

  async status(): Promise<RealtimeStatusResult> {
    return this.client.realtimeStatus({ target: { ...this.target } });
  }

  async capabilities(): Promise<RealtimeCapabilitiesResult> {
    return this.client.realtimeCapabilities({ target: { ...this.target } });
  }

  async connectWithOpenInfo(openInfo: RealtimeOpenInfo): Promise<RealtimeConnection> {
    if (JSON.stringify(openInfo.target) !== JSON.stringify(this.target)) {
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
