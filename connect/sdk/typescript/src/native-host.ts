/** Ordinary Meta builder composition for a trusted local Toolkit host.
 * Meta retains its runtime, callback reader, init watch and shutdown owner.
 */
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";
import { createHash } from "node:crypto";
import { Readable, Writable } from "node:stream";
import { GatewayStartupCleanupError, MetaBuilder, PersistentTransport } from "@rkat/meta-sdk";
import type { GatewayTransportFactory, MetaRuntime } from "@rkat/meta-sdk";
import { copyApplicationPreparation, copyApplicationUpdateStartup, copyHostBootstrapSelection, HostControlError,
  HostTransportError, PrivateHostClient } from "./private-host.js";
import type { ApplicationPreparation, ApplicationPreparationAttempt, ApplicationUpdateStartup,
  HostBootstrapSelection, SelectedHostArtifact } from "./private-host.js";

const MAX_SELECTION = 16 * 1024 * 1024;
const MAX_PLAN = 256 * 1024;
type PrivateSelection = void | ApplicationPreparationAttempt;

function requireNativeHooks(builder: MetaBuilder): void {
  if (process.platform === "win32") throw new HostControlError("Toolkit native host requires POSIX");
  if (!(builder instanceof MetaBuilder) || typeof builder.gatewayTransport !== "function"
    || typeof PersistentTransport.prototype.startAsync !== "function"
    || typeof PersistentTransport.prototype.extraStdioEndpoint !== "function"
    || typeof PersistentTransport.prototype.requireHostStartup !== "function"
    || typeof PersistentTransport.prototype.lastTermination !== "function") {
    throw new HostControlError("Toolkit native host requires the matching Meta transport SDK");
  }
}

/** One transport for one native-owned connect. It never sends meta/init itself. */
class ToolkitPersistentTransport extends PersistentTransport {
  #client: PrivateHostClient | null = null;
  #started = false;
  #settled = false;
  readonly #select: (client: PrivateHostClient) => Promise<PrivateSelection>;
  #preparation: ApplicationPreparationAttempt | undefined;
  #initSent = false;
  readonly #checkInit: ((request: Record<string, unknown>) => void) | undefined;
  readonly #privateTimeoutMs: number | undefined;
  readonly #release: (transport: ToolkitPersistentTransport) => void;

  constructor(gatewayBin: string, timeoutMs: number | undefined,
    select: (client: PrivateHostClient) => Promise<PrivateSelection>,
    release: (transport: ToolkitPersistentTransport) => void,
    checkInit?: (request: Record<string, unknown>) => void) {
    super(gatewayBin, { ...(timeoutMs === undefined ? {} : { timeout: timeoutMs }),
      extraArgs: ["--host-control-read-fd", "3", "--host-control-write-fd", "4"],
      extraStdio: ["pipe", "pipe"] });
    this.#select = select; this.#privateTimeoutMs = timeoutMs;
    this.#release = release;
    this.#checkInit = checkInit;
  }

  get hostControl(): PrivateHostClient {
    if (this.#client === null) throw new HostControlError("native host control is unavailable");
    this.#client.assertReady();
    return this.#client;
  }

  override async startAsync(): Promise<void> {
    if (this.#started) throw new HostControlError("Toolkit transport startup was already requested");
    this.#started = true;
    let commands: Readable | Writable | null = null;
    let responses: Readable | Writable | null = null;
    try {
      await super.startAsync();
      commands = this.extraStdioEndpoint(0); responses = this.extraStdioEndpoint(1);
      if (!(commands instanceof Writable) || !(responses instanceof Readable)) {
        throw new HostControlError("native host private endpoints are unavailable");
      }
      const client = new PrivateHostClient(commands, responses,
        this.#privateTimeoutMs === undefined ? {} : { timeoutMs: this.#privateTimeoutMs });
      this.#client = client;
      const selected = await this.#select(client);
      if (selected) {
        this.#preparation = selected;
        // Preparation needs this same child's native init and callbacks to
        // produce its manifest. Do not wait for adoption before writing init.
        client.assertAvailable();
      } else client.assertReady();
    } catch (error) {
      // Closing private admission first lets a child still waiting for request
      // 1 observe host loss. The native connect owner then joins transport.stop.
      try {
        if (this.#client !== null) await this.#client.close();
        else { commands?.destroy(); responses?.destroy(); }
      } catch { /* Preserve the original startup failure; native still joins stop. */ }
      throw error;
    }
  }

  override async sendAsync(request: Record<string, unknown>, options: { timeoutMs?: number } = {}): Promise<unknown> {
    // No await between this same-client check and the native request writer.
    // Native opens its watch synchronously after awaiting startAsync.
    // Keep native cleanup and other requests on their existing failure path.
    if (request.method === "meta/init") {
      if (this.#preparation) {
        if (this.#initSent) throw new HostControlError("application preparation init was already sent");
        this.#client!.assertAvailable();
        this.#checkInit?.(request);
        this.#initSent = true;
      } else this.hostControl.assertReady();
    }
    return await super.sendAsync(request, options);
  }

  override async stop(): Promise<void> {
    let failed = false, failure: unknown;
    try { await super.stop(); }
    catch (error) { failed = true; failure = error; }
    // Native retains the same child and records its actual stop observation.
    // Physical exit is required before closing this child's private endpoints.
    const termination = this.lastTermination();
    if (termination === null || termination.state === "unknown") {
      if (failed) throw failure;
      throw new HostTransportError("native gateway cleanup remains unknown; original child exit is unobserved");
    }
    // A prior stop may already have joined both owners and reported an
    // abnormal outcome. The native runtime's next stop acknowledges that
    // retained settlement without relabelling its historical record as clean.
    if (this.#settled) {
      if (failed) throw failure;
      this.#release(this);
      return;
    }
    try {
      if (this.#client !== null) await this.#client.close();
      this.#client = null;
    } catch (error) {
      // Exit says nothing about this private join. Keep the same client for
      // the same runtime's later shutdown, including a failed build's handle.
      if (failed) throw new AggregateError([failure, error],
        "native gateway stop and private host cleanup failed", { cause: failure });
      throw error;
    }
    this.#settled = true;
    if (failed) throw failure;
    if (termination.state === "exited" && (termination.exitCode !== 0
      || termination.signal !== null || termination.escalation !== "none"
      || termination.nativeShutdown === "refused" || termination.nativeShutdown === "unanswered")) {
      throw new HostTransportError("Toolkit gateway did not confirm clean shutdown: "
        + `exit=${termination.exitCode}, signal=${termination.signal}, `
        + `escalation=${termination.escalation}, nativeShutdown=${termination.nativeShutdown}`);
    }
    // For the selected Toolkit wrapper, a natural exit 0 follows its native
    // and private owner joins. The record alone is not application readiness.
    this.#release(this);
  }
}

class SelectedHostBinding {
  #configured = false;
  #attempted = false;
  #current: ToolkitPersistentTransport | null = null;
  readonly #select: (client: PrivateHostClient) => Promise<PrivateSelection>;
  readonly #replacement: boolean;

  constructor(select: (client: PrivateHostClient) => Promise<PrivateSelection>, replacement: boolean) {
    this.#select = select; this.#replacement = replacement;
  }

  protected configureBuilder<T extends MetaBuilder>(builder: T, project?: () => void,
    checkInit?: (request: Record<string, unknown>) => void): T {
    requireNativeHooks(builder);
    if (this.#configured) throw new HostControlError("Toolkit host binding was already configured");
    const factory: GatewayTransportFactory = ({ gatewayBin, timeoutMs }) => {
      if (this.#replacement && this.#attempted) {
        throw new HostControlError("application update already started; inspect the original transaction");
      }
      if (this.#current !== null) throw new HostControlError("Toolkit host retains a transport with unresolved cleanup");
      // Consumed before constructing or spawning a transport. A reconnect,
      // failed start or second runtime cannot replay an admitted replacement.
      this.#attempted = true;
      const transport = new ToolkitPersistentTransport(gatewayBin, timeoutMs, this.#select,
        stopped => { if (this.#current === stopped) this.#current = null; }, checkInit);
      this.#current = transport;
      return transport;
    };
    project?.();
    builder.gatewayTransport(factory);
    this.#configured = true;
    return builder;
  }

  /** Only the current process's selected private channel, never a new owner. */
  get hostControl(): PrivateHostClient {
    if (this.#current === null) throw new HostControlError("native host control is unavailable");
    return this.#current.hostControl;
  }
}

/** Select ordinary original-owner Restore on a native Meta builder. */
export class ToolkitHost extends SelectedHostBinding {
  constructor(selection: HostBootstrapSelection) {
    const selected = copyHostBootstrapSelection(selection);
    super(client => client.bootstrap(selected), false);
  }

  /** Configure a dedicated builder before build(); Meta owns connect/shutdown. */
  configure<T extends MetaBuilder>(builder: T): T { return this.configureBuilder(builder); }
}

/** One replacement process under the existing trusted application transaction.
 * The caller retains the exact source/callback lease, prior desired policy and
 * declaration/recovery decision. Matching JSON or hashes do not authenticate it.
 */
export class ToolkitApplicationUpdate extends SelectedHostBinding {
  readonly #bytes: Uint8Array;
  readonly #digest: string;

  constructor(startup: ApplicationUpdateStartup) {
    const selected = copyApplicationUpdateStartup(startup);
    if (process.platform === "win32") throw new HostControlError("Toolkit native host requires POSIX");
    const descriptor = openSync(selected.selection.path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    let bytes: Buffer;
    try {
      if (!fstatSync(descriptor).isFile()) throw new HostControlError("application update selection must be a regular file");
      const buffer = Buffer.alloc(MAX_SELECTION + 1);
      let length = 0;
      while (length < buffer.length) {
        const read = readSync(descriptor, buffer, length, buffer.length - length, null);
        if (read === 0) break;
        length += read;
      }
      bytes = Buffer.from(buffer.subarray(0, length));
      if (bytes.length > MAX_SELECTION || createHash("sha256").update(bytes).digest("hex") !== selected.selection.sha256) {
        throw new HostControlError("application update selection changed or exceeds its limit");
      }
    } finally { closeSync(descriptor); }
    super(client => client.replaceApplication(selected), true);
    this.#bytes = bytes; this.#digest = selected.selection.sha256;
  }

  /** Supplies the fixed candidate through the public native builder carrier.
   * Configure a dedicated builder, then set its ordinary callbacks/storage and
   * optional transaction-selected declareSpecUpdate. Do not replace this carrier
   * or factory afterward. Native still validates the exact private/public join.
   */
  configure<T extends MetaBuilder>(builder: T): T {
    return this.configureBuilder(builder, () => builder.preparedApplication(this.#bytes, this.#digest));
  }
}

type NotStarted = { readonly status: "not_started" };
type NotRequested = { readonly status: "not_requested" };
/** Actual outcomes of this one build, not a recovery or readiness receipt. */
export interface ApplicationPreparationSettlement {
  readonly native: PromiseSettledResult<MetaRuntime>;
  readonly finalization: PromiseSettledResult<void> | NotStarted;
  readonly privateClose: PromiseSettledResult<void> | NotRequested;
  readonly nativeClose: PromiseSettledResult<void> | NotRequested;
  readonly cancelled: boolean;
}
interface PreparationState {
  client: PrivateHostClient | null;
  attempt: ApplicationPreparationAttempt | null;
  closing: boolean;
  cancelled: boolean;
  privateClose: Promise<PromiseSettledResult<void>> | null;
  finalization: Promise<PromiseSettledResult<void> | NotStarted>;
  finish: (result: PromiseSettledResult<void> | NotStarted) => void;
}
function settled<T>(operation: Promise<T>): Promise<PromiseSettledResult<T>> {
  return operation.then(value => ({ status: "fulfilled" as const, value }),
    reason => ({ status: "rejected" as const, reason }));
}
function closePreparation(state: PreparationState): void {
  state.closing = true;
  if (state.client !== null && state.privateClose === null) {
    // Close private admission to wake the held compiler. This is not native
    // settlement; build/shutdown and any entered finalizer still remain joined.
    state.privateClose = settled(state.client.close());
  }
}

/** Fresh schema-4 preparation, final publication and native adoption in one
 * child. The application's real finalizer owns validation and publication.
 * Use build(), which joins the ordinary native build and that finalizer;
 * ordinary Restore and replacement keep their existing configure() APIs.
 */
export class ToolkitApplicationPreparation extends SelectedHostBinding {
  readonly #plan: ApplicationPreparation;
  readonly #bytes: Buffer;
  readonly #state: PreparationState;
  #built = false;
  #settlement: ApplicationPreparationSettlement | null = null;

  constructor(preparation: ApplicationPreparation,
    finalize: (manifest: SelectedHostArtifact) => Promise<HostBootstrapSelection>,
    options: { timeoutMs?: number } = {}) {
    const selected = copyApplicationPreparation(preparation);
    if (typeof finalize !== "function") throw new HostControlError("application finalizer is required");
    if (process.platform === "win32") throw new HostControlError("Toolkit native host requires POSIX");
    const descriptor = openSync(selected.plan.path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    let bytes: Buffer;
    try {
      if (!fstatSync(descriptor).isFile()) throw new HostControlError("application preparation plan must be a regular file");
      const buffer = Buffer.alloc(MAX_PLAN + 1);
      let length = 0;
      while (length < buffer.length) {
        const read = readSync(descriptor, buffer, length, buffer.length - length, null);
        if (read === 0) break;
        length += read;
      }
      bytes = Buffer.from(buffer.subarray(0, length));
      if (bytes.length > MAX_PLAN || createHash("sha256").update(bytes).digest("hex") !== selected.plan.sha256) {
        throw new HostControlError("application preparation plan changed or exceeds its limit");
      }
    } finally { closeSync(descriptor); }
    let finish!: PreparationState["finish"];
    const finalization = new Promise<PromiseSettledResult<void> | NotStarted>(resolve => { finish = resolve; });
    const state: PreparationState = { client: null, attempt: null, closing: false,
      cancelled: false, privateClose: null, finalization, finish };
    const privateOptions = options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs };
    super(async client => {
      state.client = client;
      if (state.closing) throw new HostControlError("application preparation closed before startup");
      const attempt = client.beginApplicationPreparation(selected, privateOptions);
      state.attempt = attempt;
      const work = (async () => {
        const manifest = await attempt.observe();
        if (state.closing) throw new HostControlError("application preparation closed before publication");
        // No await separates that check from entering the trusted finalizer.
        // Caller loss cannot abandon a publication that has already entered.
        const selection = await finalize(manifest);
        await attempt.adopt(selection);
      })();
      void settled(work).then(result => {
        if (result.status === "rejected") closePreparation(state);
        state.finish(result);
      });
      return attempt;
    }, true);
    this.#plan = selected; this.#bytes = bytes; this.#state = state;
  }

  get preparationAttempt(): ApplicationPreparationAttempt | null { return this.#state.attempt; }
  get settlement(): ApplicationPreparationSettlement | null { return this.#settlement; }

  /** One ordinary native build. Abort fences unentered publication and closes
   * the private channel, then joins native work and any entered publication.
   * Neither failed build nor shutdown permits this binding to start again.
   */
  async build(builder: MetaBuilder, options: { signal?: AbortSignal } = {}): Promise<MetaRuntime> {
    if (this.#built) throw new HostControlError("application preparation already started; inspect the original attempt");
    if (options.signal?.aborted) throw new HostControlError("application preparation cancelled before startup");
    this.configureBuilder(builder, () => builder.preparedApplication(this.#bytes, this.#plan.plan.sha256), request => {
      const params = request.params;
      if (typeof params !== "object" || params === null || Array.isArray(params)) {
        throw new HostControlError("application preparation init has no selected plan");
      }
      const input = params as Record<string, unknown>;
      const carrier = input.prepared_application;
      if (Object.hasOwn(input, "mob_config") || Object.hasOwn(input, "application_tool_policies")
        || typeof carrier !== "object" || carrier === null || Array.isArray(carrier)
        || Object.keys(carrier).length !== 2
        || (carrier as Record<string, unknown>).bundle_base64 !== this.#bytes.toString("base64")
        || (carrier as Record<string, unknown>).digest !== this.#plan.plan.sha256) {
        throw new HostControlError("application preparation init differs from its selected plan");
      }
    });
    this.#built = true;
    const state = this.#state;
    const abort = () => { state.cancelled = true; closePreparation(state); };
    options.signal?.addEventListener("abort", abort, { once: true });
    if (options.signal?.aborted) abort();
    try {
      const nativeOutcome = settled(builder.build()).then(result => {
        if (result.status === "rejected") closePreparation(state);
        if (state.attempt === null) state.finish({ status: "not_started" });
        return result;
      });
      const [native, finalization] = await Promise.all([nativeOutcome, state.finalization]);
      const failed = native.status === "rejected" || finalization.status !== "fulfilled" || state.cancelled;
      let nativeClose: PromiseSettledResult<void> | NotRequested = { status: "not_requested" };
      if (failed) {
        closePreparation(state);
        // If native Ready raced a private failure/cancellation, this build has
        // not returned the runtime. Join that exact runtime before reporting.
        if (native.status === "fulfilled") nativeClose = await settled(native.value.shutdown());
      }
      const privateClose = state.privateClose === null
        ? { status: "not_requested" as const } : await state.privateClose;
      this.#settlement = Object.freeze({ native, finalization, privateClose, nativeClose, cancelled: state.cancelled });
      // Actual native typed errors (including its cleanup capability) take
      // precedence over a private EOF caused by the same failure.
      if (native.status === "rejected") throw native.reason;
      const primary = finalization.status === "rejected" ? finalization.reason
        : new HostControlError(state.cancelled ? "application preparation cancelled" : "application preparation did not start");
      if (nativeClose.status === "rejected") {
        throw new GatewayStartupCleanupError(primary, nativeClose.reason, {
          shutdown: () => native.value.shutdown(), lastTermination: () => native.value.lastGatewayTermination(),
        });
      }
      if (failed) throw primary;
      return native.value;
    } finally { options.signal?.removeEventListener("abort", abort); }
  }
}
