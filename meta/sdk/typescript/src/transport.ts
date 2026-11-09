/**
 * Persistent subprocess transport for Meta JSON-RPC.
 *
 * Keeps a long-lived gateway binary alive, communicating over stdin/stdout
 * newline-delimited JSON. Supports bidirectional callbacks from Rust.
 */

import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { closeSync, openSync } from "node:fs";
import { performance } from "node:perf_hooks";
import { createInterface } from "node:readline";
import type { ChildProcess } from "node:child_process";
import type { Readable, Writable } from "node:stream";
import type { ProviderCallbackContext } from "./types.js";
import {
  GatewayChildEndedError,
  GatewayRestartRefusedError,
  TransportReaderFailedError,
} from "./errors.js";

// -- Types ----------------------------------------------------------------

export interface JsonRpcRequest {
  readonly jsonrpc: "2.0";
  readonly id: string;
  readonly method: string;
  readonly params: Record<string, unknown>;
}

export interface JsonRpcSuccess {
  readonly jsonrpc: "2.0";
  readonly id: string;
  readonly result: unknown;
}

export interface JsonRpcErrorBody {
  readonly code: number;
  readonly message: string;
}

export interface JsonRpcErrorResponse {
  readonly jsonrpc: "2.0";
  readonly id: string;
  readonly error: JsonRpcErrorBody;
}

export type JsonRpcResponse = JsonRpcSuccess | JsonRpcErrorResponse;

export type JsonRpcTransport = (
  request: JsonRpcRequest,
) => Promise<unknown>;

export type JsonRpcSyncTransport = (request: JsonRpcRequest) => unknown;

export type CallbackHandler = (
  method: string,
  params: Record<string, unknown>,
  context: ProviderCallbackContext,
) => Promise<unknown>;

export type FetchLikeResponse = {
  ok: boolean;
  status: number;
  text(): Promise<string>;
};

export type FetchLike = (
  url: string,
  init: {
    method: "POST";
    headers: Record<string, string>;
    body: string;
    /** Optional AbortSignal — surfaced so the http transport can cancel
     * a hung server request after `timeoutMs`. Implementations that
     * don't support it can ignore the field. */
    signal?: AbortSignal;
  },
) => Promise<FetchLikeResponse>;

/**
 * Shutdown wait when no valid horizon was advertised. The stock Rust gateway
 * advertises its own `stdio_shutdown_horizon_ms` (432_000 ms in 0.9.5) in
 * its `accepted` answer to `meta/init`, before any bootstrap, and the
 * transport waits that value instead. This fallback governs only a stop
 * before that answer (nothing durable has run yet), an older or custom
 * gateway without the shutdown handshake (the stdin EOF protocol), and a
 * handshake gateway that advertises no valid horizon.
 *
 * Provider operations are publicly required to finish within 120 seconds;
 * the stock Rust gateway gives each callback a hard 130-second wire deadline.
 * For the gateways this fallback serves, it covers two such callback windows,
 * runtime event/mob drains, bounded RPC/HTTP/stdout phases, and
 * response-delivery/process-reap margin.
 *
 * `MetaRuntime.gatewayShutdownHorizonMs` is then `null`. A supervisor that
 * sizes its stop from it falls back to its own unreported horizon: Meta
 * Forge uses the stock 432_000 ms and widens an earlier systemd unit to fit.
 */
export const PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS = 337_000;
export const PROVIDER_CALLBACK_COMPLETION_MS = 125_000;

const PERSISTENT_TRANSPORT_SIGTERM_GRACE_MS = 5_000;
const PERSISTENT_TRANSPORT_SIGKILL_GRACE_MS = 5_000;
// How long a gateway that exited gets for its stdout to close before its
// waiters fail anyway: a process it started can hold that pipe open.
const CHILD_EXIT_STDOUT_GRACE_MS = 1_000;
const MAX_GATEWAY_SHUTDOWN_HORIZON_MS = 2_147_483_647;
const GATEWAY_SHUTDOWN_METHOD = "meta/shutdown";

type ChildExitWaiter = (
  child: ChildProcess,
  timeoutMs: number,
) => Promise<boolean>;

// -- Helpers --------------------------------------------------------------

export function buildJsonRpcRequest(
  id: string,
  method: string,
  params: Record<string, unknown>,
): JsonRpcRequest {
  return { jsonrpc: "2.0", id, method, params };
}

function sanitizeForJson(obj: unknown): unknown {
  if (obj === null || obj === undefined) return obj;
  if (typeof obj === "boolean" || typeof obj === "number" || typeof obj === "string") return obj;
  if (Array.isArray(obj)) return obj.map(sanitizeForJson);
  if (typeof obj === "object") {
    const result: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
      result[k] = sanitizeForJson(v);
    }
    return result;
  }
  return String(obj);
}

/** meta/init: accepted, then settled (#550). */
export const INIT_PROTOCOL_ACCEPTED_THEN_SETTLED = "accepted_then_settled";
const INIT_PROGRESS_METHOD = "meta/init_progress";
const INIT_SETTLED_METHOD = "meta/init_settled";

/**
 * Correlates one accepted-then-settled `meta/init`. Registered before the
 * init request is written, so no progress or settlement line arrives
 * unclaimed.
 */
export class InitWatch {
  /** The last `meta/init_progress` phase seen, for diagnostics. */
  lastPhase: string | null = null;
  private _settle!: (params: Record<string, unknown>) => void;
  private _fail!: (error: Error) => void;
  private _done = false;
  private readonly _settlement: Promise<Record<string, unknown>>;

  constructor(readonly initId: string) {
    this._settlement = new Promise((resolve, reject) => {
      this._settle = resolve;
      this._fail = reject;
    });
    // A failure nobody awaits must not become an unhandled rejection.
    this._settlement.catch(() => undefined);
  }

  /** @internal */
  deliver(method: string, params: Record<string, unknown>): void {
    if (method === INIT_PROGRESS_METHOD) {
      if (typeof params.phase === "string") this.lastPhase = params.phase;
    } else if (method === INIT_SETTLED_METHOD && !this._done) {
      this._done = true;
      this._settle(params);
    }
  }

  /** @internal */
  fail(error: Error): void {
    if (this._done) return;
    this._done = true;
    this._fail(error);
  }

  /**
   * Wait for `meta/init_settled`. `deadlineMs` is the caller's own cap
   * (`null`: wait until the gateway settles, exits, or the reader fails);
   * exceeding it rejects with `InitDeadlineExceeded`.
   */
  settlement(deadlineMs: number | null): Promise<Record<string, unknown>> {
    if (deadlineMs === null) return this._settlement;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new InitDeadlineExceeded(deadlineMs)), deadlineMs);
      this._settlement.then(
        (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        (error: unknown) => {
          clearTimeout(timer);
          reject(error);
        },
      );
    });
  }
}

/** The caller's init deadline ran out before the init settled. */
export class InitDeadlineExceeded extends Error {
  constructor(readonly deadlineMs: number) {
    super(`the init deadline of ${deadlineMs}ms ran out before init settled`);
    this.name = "InitDeadlineExceeded";
  }
}

function containsNonFinite(obj: unknown): boolean {
  if (typeof obj === "number") return !Number.isFinite(obj);
  if (Array.isArray(obj)) return obj.some(containsNonFinite);
  if (typeof obj === "object" && obj !== null) {
    return Object.values(obj as Record<string, unknown>).some(containsNonFinite);
  }
  return false;
}

function childHasExited(child: ChildProcess): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

function validateGatewayShutdownResponse(
  response: unknown,
): "acknowledged" | "retained_until_exit" {
  if (typeof response !== "object" || response === null) {
    throw new Error("gateway shutdown returned a malformed response");
  }
  const envelope = response as Record<string, unknown>;
  if (typeof envelope.error === "object" && envelope.error !== null) {
    const message = String(
      (envelope.error as Record<string, unknown>).message ?? "unknown gateway error",
    );
    throw new Error(`gateway shutdown failed: ${message}`);
  }
  const result = envelope.result;
  if (typeof result !== "object" || result === null) {
    throw new Error("gateway shutdown did not complete runtime-owned cleanup");
  }
  const fields = result as Record<string, unknown>;
  if (fields.shutdown === true && fields.runtime_cleanup_completed === true) {
    return "acknowledged";
  }
  // Cleanup is still incomplete, but the only cause is durable child mobs
  // held until the gateway process exits; reaping it (next, with its own
  // failure path) releases them. Not a failed phase.
  if (
    fields.runtime_cleanup_completed === false &&
    fields.runtime_cleanup_retained_until_exit === true
  ) {
    return "retained_until_exit";
  }
  throw new Error("gateway shutdown did not complete runtime-owned cleanup");
}

async function waitForChildExit(
  child: ChildProcess,
  timeoutMs: number,
): Promise<boolean> {
  if (childHasExited(child)) return true;

  return new Promise<boolean>((resolve) => {
    let settled = false;
    let timer: NodeJS.Timeout | null = null;

    const finish = (exited: boolean): void => {
      if (settled) return;
      settled = true;
      child.removeListener("exit", onExit);
      child.removeListener("close", onExit);
      if (timer !== null) clearTimeout(timer);
      resolve(exited);
    };
    const onExit = (): void => finish(true);

    child.once("exit", onExit);
    child.once("close", onExit);

    // Close the check/listener race: the process may have exited between the
    // initial fast path and listener registration.
    if (childHasExited(child)) {
      finish(true);
      return;
    }

    timer = setTimeout(() => finish(childHasExited(child)), timeoutMs);
    timer.unref?.();
    if (settled) clearTimeout(timer);
  });
}

/**
 * The highest signal {@link stopChildProcess} attempted: none (it only closed
 * stdin), SIGTERM, or SIGKILL. An attempt is not proof that it caused the
 * exit.
 */
export type ChildStopEscalation = "none" | "sigterm" | "sigkill";

/**
 * @internal Exported for deterministic lifecycle policy tests. `progress`
 * records each escalation before it is attempted, so a caller still knows it
 * when this rejects. It only rises: an earlier stop's attempt against the
 * same child is kept.
 */
export async function stopChildProcess(
  child: ChildProcess,
  waitForExit: ChildExitWaiter = waitForChildExit,
  shutdownGraceMs = PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS,
  progress: { escalation: ChildStopEscalation } = { escalation: "none" },
): Promise<ChildStopEscalation> {
  try {
    child.stdin?.end();
  } catch {
    // Continue with signal-based cleanup if stdin is already unavailable.
  }

  if (
    await waitForExit(child, shutdownGraceMs)
  ) {
    return progress.escalation;
  }

  progress.escalation = progress.escalation === "sigkill" ? "sigkill" : "sigterm";
  try {
    child.kill("SIGTERM");
  } catch {
    // A concurrent exit is observed by the bounded wait below.
  }

  if (await waitForExit(child, PERSISTENT_TRANSPORT_SIGTERM_GRACE_MS)) {
    return progress.escalation;
  }

  progress.escalation = "sigkill";
  try {
    child.kill("SIGKILL");
  } catch {
    // Best effort: never make SDK shutdown unbounded on a failed kill call.
  }

  if (!(await waitForExit(child, PERSISTENT_TRANSPORT_SIGKILL_GRACE_MS))) {
    throw new Error(
      "persistent transport: gateway process did not terminate after bounded cleanup",
    );
  }
  return progress.escalation;
}

/**
 * What one `stop()` observed for the gateway process it stopped. Read it with
 * {@link PersistentTransport.lastTermination}.
 *
 * `"exited"` with `exitCode` 0 and escalation `"none"` is the process outcome
 * only. It does not prove the gateway's own cleanup (see `nativeShutdown`), a
 * host's private settlement, or drained stdio: the stop's wait can finish on
 * the process exit before its streams close.
 */
export interface TransportTermination {
  /**
   * `"exited"`: the stop observed the process end (`exitCode` or `signal`).
   * `"not_started"`: there was no started process to stop (never started,
   * or its spawn failed). `"unknown"`: the bounded cleanup ended without an
   * observed exit; the transport keeps the process and a later `stop()`
   * tries again.
   */
  readonly state: "exited" | "not_started" | "unknown";
  /** The process's own exit code; `null` when a signal ended it or no exit was observed. */
  readonly exitCode: number | null;
  /** The signal that ended the process (for example `"SIGKILL"`); `null` for a normal exit. */
  readonly signal: string | null;
  /**
   * The highest signal any stop attempted against this process. An attempted
   * signal is not proof that it caused the exit, nor that the process ended
   * at all.
   */
  readonly escalation: ChildStopEscalation;
  /**
   * The gateway's own answer to the shutdown request, apart from the process
   * outcome: `"acknowledged"` (runtime cleanup completed),
   * `"retained_until_exit"` (cleanup completes only with the process exit),
   * `"refused"` (an error or non-attesting answer), `"unanswered"` (no answer:
   * a timeout, a failed reader or a gone process), or `"not_negotiated"`
   * (the gateway offered no shutdown handshake, so the stop only closed its
   * stdin).
   */
  readonly nativeShutdown:
    | "acknowledged"
    | "retained_until_exit"
    | "refused"
    | "unanswered"
    | "not_negotiated";
}

const NOT_STARTED: TransportTermination = Object.freeze({
  state: "not_started",
  exitCode: null,
  signal: null,
  escalation: "none",
  nativeShutdown: "not_negotiated",
});

/** What a stop observed for `child`, read from the child itself. */
function terminationOf(
  child: ChildProcess,
  escalation: ChildStopEscalation,
  nativeShutdown: TransportTermination["nativeShutdown"],
): TransportTermination {
  // A failed spawn has no pid; Node then reports a negative errno as its
  // exit code, which is not an exit of a started gateway.
  if (child.pid === undefined) return NOT_STARTED;
  const exited = childHasExited(child);
  return Object.freeze({
    state: exited ? "exited" : "unknown",
    exitCode: exited ? child.exitCode : null,
    signal: exited ? child.signalCode : null,
    escalation,
    nativeShutdown,
  });
}

// -- PersistentTransport --------------------------------------------------

/**
 * One extra stdio slot of the gateway process, at child descriptor
 * `3 + index`: `"pipe"` gives the parent an endpoint
 * ({@link PersistentTransport.extraStdioEndpoint}), `"ignore"` leaves it
 * closed, and a number passes that open parent descriptor (3 or above).
 */
export type PersistentTransportStdioSlot = "pipe" | "ignore" | number;

export interface PersistentTransportOptions {
  env?: Record<string, string>;
  timeout?: number;
  /** Arguments appended after `--persistent`. */
  extraArgs?: readonly string[];
  /**
   * Extra stdio slots of the gateway process. Every other parent descriptor
   * stays closed in the child.
   */
  extraStdio?: readonly PersistentTransportStdioSlot[];
}

/**
 * Builds the transport one `connect()` starts. It runs on every connect,
 * before any `meta/init`.
 */
export type GatewayTransportFactory = (gateway: {
  gatewayBin: string;
  timeoutMs: number | undefined;
}) => PersistentTransport;

function validateExtraArgs(extraArgs: readonly string[]): readonly string[] {
  if (extraArgs.some((arg) => typeof arg !== "string" || arg.includes("\0"))) {
    throw new Error("extraArgs must be strings without NUL");
  }
  return [...extraArgs];
}

function validateExtraStdio(
  extraStdio: readonly PersistentTransportStdioSlot[],
): readonly PersistentTransportStdioSlot[] {
  const descriptors: number[] = [];
  for (const slot of extraStdio) {
    if (slot === "pipe" || slot === "ignore") continue;
    if (typeof slot !== "number" || !Number.isSafeInteger(slot) || slot < 3) {
      throw new Error(
        'extraStdio slots must be "pipe", "ignore" or an open non-stdio descriptor',
      );
    }
    descriptors.push(slot);
  }
  if (new Set(descriptors).size !== descriptors.length) {
    throw new Error("extraStdio must not contain duplicate descriptors");
  }
  if (descriptors.length > 0 && process.platform === "win32") {
    throw new Error("inherited descriptors require a POSIX host");
  }
  return [...extraStdio];
}

/**
 * Long-lived gateway subprocess communicating over stdin/stdout JSON-RPC.
 *
 * Uses a readline reader to multiplex responses and callbacks. Unlike
 * per-call subprocess transports, this keeps the process alive so mob
 * state persists across calls.
 */
export class PersistentTransport {
  private _process: ChildProcess | null = null;
  private _stopping: Promise<void> | null = null;
  private _stderrFd: number | null = null;
  private readonly _env: Record<string, string>;
  private readonly _timeout: number;
  // Process-local startup capabilities, never serialized in RPC. Their
  // creator keeps ownership of the parent descriptors.
  private readonly _extraArgs: readonly string[];
  private readonly _extraStdio: readonly PersistentTransportStdioSlot[];
  // Set by the runtime for a factory-built transport. A host-composed
  // transport (this, or one with its own startAsync) never respawns its
  // process on a request: the respawn would skip the host's trusted startup.
  private _hostComposed = false;
  private _callbackHandler: CallbackHandler | null = null;
  // Mutable only so deterministic tests can scale the 125-second contract.
  private _providerCallbackCompletionMs = PROVIDER_CALLBACK_COMPLETION_MS;
  // Mutable only so deterministic tests can scale the bounded stop waits.
  private _waitForExit: ChildExitWaiter = waitForChildExit;
  // What the last completed stop() observed for the current or last child;
  // null while that child is live or its stop has not completed.
  private _lastTermination: TransportTermination | null = null;
  // The highest signal any stop attempted against the current child.
  private _attemptedEscalation: ChildStopEscalation = "none";
  // The current child's answer to its one shutdown request (and the error
  // that request ended with), or null while it was not asked: a later stop
  // of the same child keeps them and never asks it again.
  private _nativeShutdown: TransportTermination["nativeShutdown"] | null = null;
  private _nativeShutdownError: Error | null = null;
  private _supportsShutdownHandshake = false;
  private _shutdownHorizonMs = PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS;
  private _advertisedShutdownHorizonMs: number | null = null;
  private readonly _pending = new Map<
    string,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  // Set once the reader for the current gateway process has stopped; every
  // waiter and every later request fails with it.
  private _readerFailure: TransportReaderFailedError | null = null;
  // The accepted-then-settled init in flight on this process, if any.
  private _initWatch: InitWatch | null = null;

  constructor(
    readonly gatewayBin: string,
    options?: PersistentTransportOptions,
  ) {
    this._extraArgs = validateExtraArgs(options?.extraArgs ?? []);
    this._extraStdio = validateExtraStdio(options?.extraStdio ?? []);
    this._env = { ...process.env, ...(options?.env ?? {}) } as Record<string, string>;
    this._timeout = options?.timeout ?? 60_000;
  }

  setCallbackHandler(handler: CallbackHandler): void {
    this._callbackHandler = handler;
  }

  /** Register the watch for `initId` before writing `meta/init`. */
  openInitWatch(initId: string): InitWatch {
    const watch = new InitWatch(initId);
    this._initWatch = watch;
    if (this._readerFailure !== null) watch.fail(this._readerFailure);
    return watch;
  }

  closeInitWatch(watch: InitWatch): void {
    if (this._initWatch === watch) this._initWatch = null;
  }

  /**
   * Start the gateway process and finish any host-owned startup before the
   * runtime sends `meta/init`. The default only spawns. A subclass that
   * passes private startup channels overrides it to complete its own trusted
   * bootstrap over them; a rejection fails `connect()`, which then stops this
   * transport.
   */
  async startAsync(): Promise<void> {
    this.start();
  }

  /**
   * Mark this transport host-composed: once its gateway process is gone, a
   * request fails with `GatewayRestartRefusedError` instead of respawning
   * it. A transport with its own `startAsync()` is host-composed already.
   */
  requireHostStartup(): void {
    this._hostComposed = true;
  }

  /**
   * The parent end of extra stdio slot `index` (child descriptor
   * `3 + index`) of the running gateway process, for a `"pipe"` slot; `null`
   * for another slot or when no process runs. The transport never reads or
   * writes it; it closes with the process.
   */
  extraStdioEndpoint(index: number): Readable | Writable | null {
    if (!Number.isSafeInteger(index) || index < 0 || index >= this._extraStdio.length) {
      throw new RangeError(`no extra stdio slot ${index}`);
    }
    const child = this._process;
    if (this._extraStdio[index] !== "pipe" || child === null || childHasExited(child)) {
      return null;
    }
    return (child.stdio[3 + index] as Readable | Writable | null | undefined) ?? null;
  }

  start(): void {
    if (this._stopping !== null) {
      throw new Error("persistent transport is stopping");
    }
    if (this.isRunning()) {
      return;
    }

    if (this._process !== null) {
      // Process exit can precede stdout EOF. Retire its waiters before a new
      // child can reuse request ids, and clear their old request timers.
      this._onReaderClosed("the previous gateway process exited", this._process);
      this._initWatch = null;
    }

    // Capabilities are process-scoped and must be renegotiated on init after
    // a child restart.
    this._supportsShutdownHandshake = false;
    this._shutdownHorizonMs = PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS;
    this._advertisedShutdownHorizonMs = null;
    // A new process gets a new reader.
    this._readerFailure = null;
    // A new process has no termination yet; a previous child's never stands
    // in for it.
    this._lastTermination = null;
    this._attemptedEscalation = "none";
    this._nativeShutdown = null;
    this._nativeShutdownError = null;
    this._process = spawn(this.gatewayBin, ["--persistent", ...this._extraArgs], {
      env: this._env,
      stdio: ["pipe", "pipe", this._stderrDisposition(), ...this._extraStdio],
    });

    const child = this._process;

    // Background reader on stdout
    let readerClosed = child.stdout == null;
    let exitFailed = false;
    let exitGrace: ReturnType<typeof setTimeout> | undefined;
    if (child.stdout) {
      const rl = createInterface({ input: child.stdout });
      rl.on("line", (line: string) => this._handleLine(line, child));
      rl.on("close", () => {
        readerClosed = true;
        clearTimeout(exitGrace);
        // The exit grace below already failed this child with its exit and
        // let go of stdout: keep that failure.
        if (exitFailed) return;
        this._onReaderClosed("the gateway closed its stdout", child);
      });
    }
    // A gateway that exits while a process it started still holds its
    // stdout never closes the reader. After a short grace, fail its waiters
    // with the exit instead of leaving them to their request timeouts, and
    // let go of the pipe so it cannot keep this process alive.
    child.once("exit", (code, signal) => {
      if (readerClosed) return;
      exitGrace = setTimeout(() => {
        exitFailed = true;
        this._failExitedChildWaiters(
          child,
          new GatewayChildEndedError(
            `the gateway exited (code ${code ?? "null"}, signal ${signal ?? "null"}) ` +
              "while a process it started still held its stdout",
            code,
            signal,
          ),
        );
        // A reply still unread here (the event loop was blocked for the whole
        // grace) is dropped; its waiter already has the exit.
        child.stdout?.destroy();
      }, CHILD_EXIT_STDOUT_GRACE_MS);
    });

    child.on("error", () => {
      if (this._process !== child) return;
      // Process spawn error — fail all pending
      for (const [id, pending] of this._pending) {
        this._pending.delete(id);
        pending.reject(new Error("gateway process failed to start"));
      }
    });
  }

  /** Handle one stdout line. A line that is not a JSON object is skipped. */
  private _handleLine(line: string, child = this._process): void {
    if (this._process !== child) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return;
    const msg = parsed as Record<string, unknown>;

    if ("method" in msg) {
      const method = String(msg.method);
      if (!("id" in msg) && (method === INIT_PROGRESS_METHOD || method === INIT_SETTLED_METHOD)) {
        const params = msg.params;
        const watch = this._initWatch;
        // Only the init this process is running; a late or foreign line is ignored.
        if (
          watch !== null &&
          typeof params === "object" &&
          params !== null &&
          (params as Record<string, unknown>).init_id === watch.initId
        ) {
          watch.deliver(method, params as Record<string, unknown>);
        }
        return;
      }
      this._handleCallback(msg, child);
    } else if ("id" in msg) {
      const msgId = String(msg.id);
      const pending = this._pending.get(msgId);
      if (pending) {
        this._pending.delete(msgId);
        pending.resolve(msg);
      }
    }
  }

  /** No response can arrive any more: fail every waiter, typed. */
  private _onReaderClosed(reason: string, child = this._process): void {
    if (this._process !== child) return;
    const failure = new TransportReaderFailedError(reason);
    this._readerFailure = failure;
    for (const [id, pending] of this._pending) {
      this._pending.delete(id);
      pending.reject(failure);
    }
    this._initWatch?.fail(failure);
  }

  /**
   * The gateway exited and its stdout outlived the grace, held by a process
   * it started: fail its waiters as a closed reader would, with the exit. A
   * replacement child's waiters are never touched; once a stop let go of the
   * child, no other child's waiter can be pending.
   */
  private _failExitedChildWaiters(child: ChildProcess, failure: GatewayChildEndedError): void {
    if (this._process !== null && this._process !== child) return;
    if (this._process === child) this._readerFailure = failure;
    for (const [id, pending] of this._pending) {
      this._pending.delete(id);
      pending.reject(failure);
    }
    this._initWatch?.fail(failure);
  }

  private _handleCallback(msg: Record<string, unknown>, child = this._process): void {
    // Callback ids and host deadlines belong to this exact child, even when
    // the transport starts another child before the handler finishes.
    const respond = (response: Record<string, unknown>): void => this._writeLine(response, child);
    const handler = this._callbackHandler;
    const method = String(msg.method ?? "");
    if (!handler) {
      // Answer now: the gateway would otherwise wait out its full callback
      // deadline for a response that never comes.
      if (msg.id !== undefined) {
        respond({
          jsonrpc: "2.0",
          id: String(msg.id),
          error: {
            code: -32000,
            message: `no callback handler is registered for ${method}`,
            data: { kind: "callback_handler_unavailable", method },
          },
        });
      }
      return;
    }

    const params = (
      typeof msg.params === "object" && msg.params !== null
        ? msg.params
        : {}
    ) as Record<string, unknown>;
    const callbackId = msg.id !== undefined ? String(msg.id) : null;
    const controller = new AbortController();
    const deadlineMs = Date.now() + this._providerCallbackCompletionMs;
    let completed = false;
    const timer = setTimeout(() => {
      if (completed) return;
      completed = true;
      const error = new Error(
        "provider callback exceeded its 125s host completion deadline",
      );
      controller.abort(error);
      if (callbackId !== null) {
        respond({
          jsonrpc: "2.0",
          id: callbackId,
          error: { code: -32000, message: error.message },
        });
      }
    }, this._providerCallbackCompletionMs);
    timer.unref?.();

    Promise.resolve()
      .then(() => {
        if (this._process !== child) return;
        return handler(method, params, {
          signal: controller.signal,
          deadlineMs,
        });
      })
      .then((result) => {
        if (completed) return;
        completed = true;
        clearTimeout(timer);
        if (callbackId === null) return; // Notification — no response
        const sanitized = sanitizeForJson(result);
        if (containsNonFinite(sanitized)) {
          // JSON has no NaN or Infinity; JSON.stringify would silently turn
          // them into null and change the provider's answer. Refuse it.
          respond({
            jsonrpc: "2.0",
            id: callbackId,
            error: {
              code: -32000,
              message: `${method} returned a non-finite number (NaN or Infinity), which JSON cannot carry`,
              data: { kind: "non_finite_result" },
            },
          });
          return;
        }
        respond({
          jsonrpc: "2.0",
          id: callbackId,
          result: sanitized,
        });
      })
      .catch((err: unknown) => {
        if (completed) return;
        completed = true;
        clearTimeout(timer);
        if (callbackId === null) return;
        respond({
          jsonrpc: "2.0",
          id: callbackId,
          error: { code: -32000, message: String(err instanceof Error ? err.message : err) },
        });
      });
  }

  private _writeLine(obj: Record<string, unknown>, child = this._process): void {
    if (this._process === child && child?.stdin?.writable) {
      child.stdin.write(JSON.stringify(obj) + "\n");
    }
  }

  /**
   * Send one request and await its response. `options.timeoutMs` overrides
   * the transport's request timeout for this request only (a server-side
   * wait whose own deadline is longer than the default).
   */
  async sendAsync(
    request: Record<string, unknown>,
    options: { timeoutMs?: number } = {},
  ): Promise<unknown> {
    const response = await this._sendAsyncWithTimeout(
      request,
      options.timeoutMs ?? this._timeout,
    );
    if (request.method === "meta/init") {
      const result =
        typeof response === "object" && response !== null
          ? (response as Record<string, unknown>).result
          : null;
      this._supportsShutdownHandshake =
        typeof result === "object" &&
        result !== null &&
        (result as Record<string, unknown>).stdio_shutdown_handshake === true;
      this._shutdownHorizonMs = PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS;
      this._advertisedShutdownHorizonMs = null;
      if (this._supportsShutdownHandshake) {
        const horizonMs = (result as Record<string, unknown>).stdio_shutdown_horizon_ms;
        if (
          typeof horizonMs === "number" &&
          Number.isSafeInteger(horizonMs) &&
          horizonMs > 0 &&
          horizonMs <= MAX_GATEWAY_SHUTDOWN_HORIZON_MS
        ) {
          this._shutdownHorizonMs = horizonMs;
          this._advertisedShutdownHorizonMs = horizonMs;
        }
      }
    }
    return response;
  }

  private async _sendAsyncWithTimeout(
    request: Record<string, unknown>,
    timeoutMs: number,
    expectedChild?: ChildProcess,
  ): Promise<unknown> {
    if (expectedChild === undefined) {
      this._ensureRunning();
    } else if (this._process !== expectedChild || childHasExited(expectedChild)) {
      throw new Error("persistent transport: subprocess is not running");
    }
    const msgId = String(request.id ?? "");
    if (this._readerFailure !== null) throw this._readerFailure;

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this._pending.delete(msgId);
        reject(new Error(`persistent transport: timeout after ${timeoutMs}ms`));
      }, timeoutMs);

      this._pending.set(msgId, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });

      try {
        this._writeLine(request as Record<string, unknown>);
      } catch (error) {
        clearTimeout(timer);
        this._pending.delete(msgId);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  async stop(): Promise<void> {
    if (this._stopping !== null) return this._stopping;

    const child = this._process;
    if (child === null) {
      // Nothing was started to stop. A real record of an earlier child stays.
      if (this._lastTermination === null) this._lastTermination = NOT_STARTED;
      return;
    }

    const shutdownStarted = performance.now();
    let childTerminated = false;
    const progress: { escalation: ChildStopEscalation } = {
      escalation: this._attemptedEscalation,
    };
    // An earlier stop of this child already gave it the whole horizon and
    // escalated; a later stop goes straight to the signals.
    const graceSpent = this._attemptedEscalation !== "none";
    // A child gets one shutdown request. A later stop of the same child, after
    // bounded cleanup failed, only finishes reaping it: the stdin that request
    // used is already ended, and asking again could only report a false
    // failure. That stop reports the answer as the stop that reaps would have.
    const asked = this._nativeShutdown;
    let nativeShutdown: TransportTermination["nativeShutdown"] = asked ?? "not_negotiated";
    let stopping: Promise<void>;
    stopping = (async () => {
      let shutdownError: Error | null = asked === null ? null : this._nativeShutdownError;
      // Keep stdin open while the gateway shuts its runtime down: host
      // callbacks may still need round-trips.
      // Capability negotiation keeps older/custom gateways on their EOF
      // protocol instead of assuming method-not-found behavior.
      if (asked === null && this._supportsShutdownHandshake) {
        nativeShutdown = "unanswered";
        try {
          const response = await this._sendAsyncWithTimeout(
            {
              jsonrpc: "2.0",
              id: `meta-shutdown-${randomUUID()}`,
              method: GATEWAY_SHUTDOWN_METHOD,
              params: {},
            },
            this._shutdownHorizonMs,
            child,
          );
          // An answer that does not attest the gateway's cleanup is a refusal.
          nativeShutdown = "refused";
          nativeShutdown = validateGatewayShutdownResponse(response);
        } catch (error) {
          shutdownError = error instanceof Error ? error : new Error(String(error));
        }
        if (this._process === child) {
          this._nativeShutdown = nativeShutdown;
          this._nativeShutdownError = shutdownError;
        }
      }
      const elapsedMs = performance.now() - shutdownStarted;
      const remainingGraceMs = graceSpent
        ? 0
        : Math.max(0, this._shutdownHorizonMs - elapsedMs);
      await stopChildProcess(child, this._waitForExit, remainingGraceMs, progress);
      childTerminated = true;
      if (shutdownError !== null) {
        throw new Error(
          `persistent transport: gateway shutdown failed after bounded cleanup: ${shutdownError.message}`,
        );
      }
    })().finally(() => {
      if (this._process === child) {
        // Read from this exact child before it is released or kept: an exit
        // after the last bounded wait is recorded as the exit it was.
        this._attemptedEscalation = progress.escalation;
        this._lastTermination = terminationOf(child, progress.escalation, nativeShutdown);
      }
      if (
        this._process === child &&
        (childTerminated || childHasExited(child))
      ) {
        this._process = null;
        if (this._stderrFd !== null) {
          closeSync(this._stderrFd);
          this._stderrFd = null;
        }
      }
      if (this._stopping === stopping) this._stopping = null;
    });
    this._stopping = stopping;
    return stopping;
  }

  /**
   * What the most recent completed `stop()` observed for this transport's
   * current or last gateway process: its exit, the escalation it attempted
   * and the gateway's own shutdown answer (see {@link TransportTermination}).
   * `null` while the current process runs or its stop has not finished; a
   * new process never inherits an earlier one's record.
   */
  lastTermination(): TransportTermination | null {
    return this._lastTermination;
  }

  /**
   * The shutdown horizon, in milliseconds, that the current gateway process
   * advertised as `stdio_shutdown_horizon_ms` in a `meta/init` that also
   * advertised the shutdown handshake, as accepted. `null` before that
   * `meta/init`, or when the gateway advertised no valid horizon and this
   * transport waits its own default; a new process never inherits an earlier
   * one's value.
   */
  advertisedShutdownHorizonMs(): number | null {
    return this._advertisedShutdownHorizonMs;
  }

  isRunning(): boolean {
    // A process killed by a signal has a null exit code but has exited.
    return this._process !== null && !childHasExited(this._process);
  }

  /**
   * Gateway stderr disposition (tracing lines, panic hooks, migration
   * progress). Default: `"inherit"` — the child's stderr flows to the host
   * process's stderr; the old `"ignore"` default silently discarded a week
   * of panic-hook lines in one production fleet. Opt-outs, mirroring the
   * Python SDK:
   *
   * - `META_GATEWAY_STDERR_FILE=<path>`: append gateway stderr to a file.
   * - `META_GATEWAY_STDERR=devnull`: discard (the pre-0.8.9 default).
   */
  private _stderrDisposition(): "inherit" | "ignore" | number {
    if (this._stderrFd !== null) {
      closeSync(this._stderrFd);
      this._stderrFd = null;
    }
    const stderrPath = (this._env.META_GATEWAY_STDERR_FILE ?? "").trim();
    if (stderrPath) {
      this._stderrFd = openSync(stderrPath, "a");
      return this._stderrFd;
    }
    const optOut = (this._env.META_GATEWAY_STDERR ?? "").trim().toLowerCase();
    if (optOut === "devnull") {
      return "ignore";
    }
    return "inherit";
  }

  private _ensureRunning(): void {
    if (this._stopping !== null) {
      throw new Error("persistent transport is stopping");
    }
    if (!this.isRunning()) {
      // Decided on the initialized instance: a subclass's startAsync class
      // field is assigned only after this base constructor has run.
      if (this._hostComposed || this.startAsync !== PersistentTransport.prototype.startAsync) {
        throw new GatewayRestartRefusedError(
          this._process === null
            ? "its gateway process is not started"
            : "its gateway process exited",
        );
      }
      this.start();
    }
  }
}

// -- Per-call transport factories -----------------------------------------

/**
 * Create a synchronous transport that spawns the gateway binary per call.
 */
export function createGatewaySyncTransport(
  gatewayBin: string,
): JsonRpcSyncTransport {
  return (request: JsonRpcRequest): unknown => {
    const requestJson = JSON.stringify(request);
    const out = spawnSync(gatewayBin, [], {
      env: { ...process.env, META_RPC_REQUEST: requestJson },
      encoding: "utf8",
    });

    if (out.status !== 0) {
      throw new Error(
        `gateway failed (status=${out.status}): ${String(out.stderr ?? "")}`,
      );
    }

    try {
      return JSON.parse(String(out.stdout ?? "")) as unknown;
    } catch {
      throw new Error("gateway returned non-JSON response");
    }
  };
}

/**
 * Create an async transport that spawns the gateway binary per call.
 */
export function createGatewayAsyncTransport(
  gatewayBin: string,
): JsonRpcTransport {
  return async (request: JsonRpcRequest): Promise<unknown> =>
    new Promise<unknown>((resolve, reject) => {
      const requestJson = JSON.stringify(request);
      const child = spawn(gatewayBin, [], {
        env: { ...process.env, META_RPC_REQUEST: requestJson },
        stdio: ["ignore", "pipe", "pipe"],
      });

      let stdout = "";
      let stderr = "";

      if (child.stdout) {
        child.stdout.setEncoding("utf8");
        child.stdout.on("data", (chunk: string) => {
          stdout += chunk;
        });
      }
      if (child.stderr) {
        child.stderr.setEncoding("utf8");
        child.stderr.on("data", (chunk: string) => {
          stderr += chunk;
        });
      }

      child.on("error", (error: Error) => reject(error));

      child.on("close", (code: number | null) => {
        if (code !== 0) {
          reject(new Error(`gateway failed (status=${code}): ${stderr}`));
          return;
        }
        try {
          resolve(JSON.parse(stdout) as unknown);
        } catch {
          reject(new Error("gateway returned non-JSON response"));
        }
      });
    });
}

/**
 * Default request timeout for {@link createJsonRpcHttpTransport}. A
 * server that accepts the connection but never replies would otherwise
 * leak the fetch task forever; pre-fix there was no timeout at all.
 */
export const DEFAULT_HTTP_TRANSPORT_TIMEOUT_MS = 60_000;

/**
 * Create an async HTTP POST transport.
 */
export function createJsonRpcHttpTransport(
  endpoint: string,
  options: {
    headers?: Record<string, string>;
    fetchImpl?: FetchLike;
    /** Maximum time (ms) a single request may stay pending before being
     * aborted. Defaults to 60s. Set to 0 to disable (not recommended). */
    timeoutMs?: number;
  } = {},
): JsonRpcTransport {
  const globalFetch = (globalThis as unknown as { fetch?: FetchLike }).fetch;
  const fetchImpl = options.fetchImpl ?? globalFetch;
  if (!fetchImpl) {
    throw new Error("fetch implementation not available");
  }
  const timeoutMs =
    options.timeoutMs === undefined
      ? DEFAULT_HTTP_TRANSPORT_TIMEOUT_MS
      : options.timeoutMs;

  return async (request: JsonRpcRequest): Promise<unknown> => {
    const controller =
      timeoutMs > 0 ? new AbortController() : undefined;
    const timer =
      controller !== undefined && timeoutMs > 0
        ? setTimeout(() => controller.abort(), timeoutMs)
        : undefined;

    try {
      const response = await fetchImpl(endpoint, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          accept: "application/json",
          ...(options.headers ?? {}),
        },
        body: JSON.stringify(request),
        signal: controller?.signal,
      });

      const body = await response.text();
      if (!response.ok) {
        throw new Error(
          `http transport failed (status=${response.status}): ${body}`,
        );
      }

      try {
        return JSON.parse(body) as unknown;
      } catch {
        throw new Error("http transport returned non-JSON response");
      }
    } catch (err) {
      if (
        err !== null &&
        typeof err === "object" &&
        "name" in err &&
        (err as { name: unknown }).name === "AbortError"
      ) {
        throw new Error(
          `http transport timed out after ${timeoutMs}ms; server unresponsive`,
        );
      }
      throw err;
    } finally {
      if (timer !== undefined) {
        clearTimeout(timer);
      }
    }
  };
}
