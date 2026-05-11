/**
 * LiveChannel — session lifecycle wrapper for the `live/*` RPC surface.
 *
 * Replaces the old `RealtimeChannel` that was removed in the live-adapter
 * MVP. Unlike the old class (which managed a WebSocket connection directly),
 * `LiveChannel` delegates transport ownership to the caller: `open()`
 * returns a {@link LiveOpenResult} containing a transport bootstrap
 * (URL + token) that the caller connects independently.
 *
 * The class wraps the `MeerkatClient.live*` methods, binding a session id
 * at construction time so callers do not need to thread `session_id` /
 * `channel_id` through every call.
 *
 * @example
 * ```ts
 * import { MeerkatClient, LiveChannel } from "@rkat/sdk";
 *
 * const client = new MeerkatClient();
 * await client.connect();
 *
 * const session = await client.createSession("Hello");
 * const channel = LiveChannel.session(client, session.id);
 *
 * const openResult = await channel.open();
 * // Connect to openResult.transport.url externally (WebSocket, etc.)
 *
 * await channel.commitInput();
 * const status = await channel.status();
 * console.log(status.status);
 *
 * await channel.close();
 * ```
 */

import type {
  LiveChannelParams,
  LiveCommitInputParams,
  LiveOpenParams,
  LiveOpenResult,
  LiveRefreshResult,
  LiveSendInputParams,
  LiveStatusResult,
  LiveTruncateParams,
  LiveInputChunkWire,
  RealtimeTurningMode,
  WireLiveResponseModality,
} from "./generated/types.js";
import type { MeerkatClient } from "./client.js";

export type LiveOpenTransport = NonNullable<LiveOpenParams["transport"]>;

/** Options controlling how the live channel is opened. */
export interface LiveChannelOptions {
  /**
   * Turning mode for the channel. `"provider_managed"` lets the provider's
   * VAD decide when to commit; `"explicit_commit"` requires the caller to
   * invoke {@link LiveChannel.commitInput} explicitly.
   *
   * Text-only input via `commitInput({ responseModality: "text" })` requires
   * `"explicit_commit"` because the OpenAI Realtime API rejects
   * `input_audio_buffer.commit` unless the session was opened with
   * explicit-commit semantics.
   */
  readonly turningMode?: RealtimeTurningMode;
  /** Transport to request from `live/open`. Missing uses the server default. */
  readonly transport?: LiveOpenTransport;
}

export class LiveChannel {
  readonly client: MeerkatClient;
  readonly sessionId: string;
  readonly turningMode?: RealtimeTurningMode;
  readonly transport?: LiveOpenTransport;

  /** The channel id assigned by the server after `open()`. */
  private _channelId: string | undefined;

  private constructor(
    client: MeerkatClient,
    sessionId: string,
    options?: LiveChannelOptions,
  ) {
    this.client = client;
    this.sessionId = sessionId;
    this.turningMode = options?.turningMode;
    this.transport = options?.transport;
  }

  /**
   * Create a `LiveChannel` bound to a standalone session.
   */
  static session(
    client: MeerkatClient,
    sessionId: string,
    options?: LiveChannelOptions,
  ): LiveChannel {
    return new LiveChannel(client, sessionId, options);
  }

  /** The channel id, available after {@link open} completes. */
  get channelId(): string | undefined {
    return this._channelId;
  }

  /**
   * Open the live channel. Calls `live/open` and stores the returned
   * `channel_id` for subsequent operations.
   *
   * The returned {@link LiveOpenResult} contains `transport` (URL + token
   * for the caller to connect externally), `capabilities`, and `continuity`.
   */
  async open(): Promise<LiveOpenResult> {
    const params: LiveOpenParams = { session_id: this.sessionId };
    if (this.turningMode != null) {
      params.turning_mode = this.turningMode;
    }
    if (this.transport != null) {
      params.transport = this.transport;
    }
    const result = await this.client.liveOpen(params);
    this._channelId = result.channel_id;
    return result;
  }

  /**
   * Close the live channel. Calls `live/close`.
   *
   * @throws if the channel has not been opened yet.
   */
  async close(): Promise<void> {
    await this.client.liveClose(this.requireChannelParams());
  }

  /**
   * Commit buffered input on the channel. Calls `live/commit_input`.
   *
   * @param responseModality - Optional modality override for this single
   *   response. Pass `"text"` to suppress audio output on an audio-first
   *   channel, or `"audio"` explicitly. `undefined` keeps the channel default.
   */
  async commitInput(responseModality?: string): Promise<void> {
    const params: LiveCommitInputParams = {
      channel_id: this.requireChannelId(),
    };
    if (responseModality != null) {
      params.response_modality = {
        modality: responseModality,
      } as WireLiveResponseModality;
    }
    await this.client.liveCommitInput(params);
  }

  /**
   * Get the current status of the live channel. Calls `live/status`.
   */
  async status(): Promise<LiveStatusResult> {
    return this.client.liveStatus(this.requireChannelParams());
  }

  /**
   * Apply mutable session config to the open channel. Calls `live/refresh`.
   *
   * Does NOT replay canonical history. Refresh enqueues a single
   * `session.update` carrying the latest projection snapshot's mutable
   * config fields. Identity changes (model/provider swaps) require
   * close + reopen.
   */
  async refresh(): Promise<LiveRefreshResult> {
    return this.client.liveRefresh(this.requireChannelParams());
  }

  /**
   * Send an input chunk to the live channel. Calls `live/send_input`.
   */
  async sendInput(chunk: LiveInputChunkWire): Promise<void> {
    const params: LiveSendInputParams = {
      channel_id: this.requireChannelId(),
      chunk,
    };
    await this.client.liveSendInput(params);
  }

  /**
   * Send a text input chunk to the live channel.
   */
  async sendInputText(text: string): Promise<void> {
    await this.sendInput({ kind: "text", text });
  }

  /**
   * Send a base64-encoded audio chunk to the live channel.
   */
  async sendInputAudio(
    dataBase64: string,
    sampleRateHz: number,
    channels: number,
  ): Promise<void> {
    await this.sendInput({
      kind: "audio",
      data: dataBase64,
      sample_rate_hz: sampleRateHz,
      channels,
    });
  }

  /**
   * Send a base64-encoded image input chunk to the live channel.
   */
  async sendInputImage(mime: string, dataBase64: string): Promise<void> {
    await this.sendInput({ kind: "image", mime, data: dataBase64 });
  }

  /**
   * Send a base64-encoded video-frame input chunk to the live channel.
   */
  async sendInputVideoFrame(
    codec: string,
    dataBase64: string,
    timestampMs: number,
  ): Promise<void> {
    await this.sendInput({
      kind: "video_frame",
      codec,
      data: dataBase64,
      timestamp_ms: timestampMs,
    });
  }

  /**
   * Interrupt the in-progress assistant turn. Calls `live/interrupt`.
   */
  async interrupt(): Promise<void> {
    await this.client.liveInterrupt(this.requireChannelParams());
  }

  /**
   * Truncate assistant output at the client-tracked playback cursor.
   * Calls `live/truncate`.
   */
  async truncate(
    itemId: string,
    contentIndex: number,
    audioPlayedMs: number,
  ): Promise<void> {
    const params: LiveTruncateParams = {
      channel_id: this.requireChannelId(),
      item_id: itemId,
      content_index: contentIndex,
      audio_played_ms: audioPlayedMs,
    };
    await this.client.liveTruncate(params);
  }

  // -- Internal helpers ---------------------------------------------------

  private requireChannelId(): string {
    if (this._channelId == null) {
      throw new Error(
        "LiveChannel has not been opened yet — call open() first",
      );
    }
    return this._channelId;
  }

  private requireChannelParams(): LiveChannelParams {
    return { channel_id: this.requireChannelId() };
  }
}
