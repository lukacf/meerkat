/** Shared host-owned Services transport. Private events and answers must never be
 * forwarded to model messages, public RPC, telemetry or ordinary logs. Native
 * configuration, grants, credentials and resource capability remain distinct.
 */
import { spawn, type ChildProcessByStdio } from "node:child_process";
import { isAbsolute } from "node:path";
import type { Readable, Writable } from "node:stream";
import { inspect } from "node:util";
import { EventEmitter } from "node:events";
import { visit } from "jsonc-parser";

export type ObjectValue = Record<string, unknown>;
type Child = ChildProcessByStdio<Writable, Readable, null>;
// Retain the real event primitive for recovery if per-instance listener setup
// fails after spawn has already returned the owned process.
const observeOwned = EventEmitter.prototype.on;
const MAX_FRAME = 64 * 1024 * 1024;
const MAX_QUEUE = 128;
const MAX_WAIT = 2_147_483_647;
const INITIALIZE = "h:initialize";
export const PROMPTS = new Set(["approval", "provider_target", "provider_action", "provider_api_key",
  "provider_browser", "provider_device", "service_secret"]);
export const EVENTS = new Set(["state", "conversation", "prompt_closed", "unavailable", "service_finalization"]);
export const COMPLETIONS = new Set(["completed", "completed_without_result", "callback_pending", "callback_batch_pending",
  "cancelled", "abandoned", "abandoned_with_error", "completed_with_finalization_failure", "runtime_terminated", "outcome_unknown"]);
const ERROR_CODES = new Set(["invalid_binding", "unavailable", "binding_retired", "native_auth_required",
  "connector_unavailable", "connector_refused", "invalid_message", "turn_uncertain", "busy", "stale_prompt",
  "invalid_answer", "stop_uncertain", "shutdown_unconfirmed", "required_context_missing",
  "installation_metadata_required", "installation_unconfirmed", "unsupported_native_entry"]);
const ENVIRONMENT = new Set(["PATH", "LANG", "LC_ALL", "TMPDIR", "TMP", "TEMP", "SYSTEMROOT"]);

/** Captured by the authenticated host, never decoded from browser input. */
export interface ServiceSetupBinding {
  readonly application_id: string; readonly instance_id: string; readonly origin: string;
  readonly principal_id: string; readonly login_scope_id: string; readonly expires_at_ms: number;
  readonly realm_id: string; readonly context_root: string; readonly state_root: string;
  readonly user_config_root: string; readonly resource_manifest_sha256: string;
  readonly credential_scope: { readonly kind: "native_default" } | { readonly kind: "file"; readonly root: string };
  readonly adapter?: "services" | "forge_services";
  readonly model?: string;
  readonly auth_binding?: { readonly realm: string; readonly binding: string; readonly profile?: string };
}
export type ServiceSetupAnswer = { readonly kind: "cancel" }
  | { readonly kind: "approval"; readonly decision: "approve" | "deny" }
  | { readonly kind: "provider_target"; readonly choice_id: string }
  | { readonly kind: "provider_action"; readonly action: "use" | "enter_key" | "sign_in" | "back" }
  | { readonly kind: "provider_api_key"; readonly key: string }
  | { readonly kind: "service_secret"; readonly job_id: string; readonly service: string; readonly slot: string; readonly value: string };
export interface ServiceConnectorTarget {
  readonly slot_id: string; readonly issuer: string; readonly client: string; readonly resource: string;
  readonly scopes: readonly string[]; readonly strategy_id: string;
  readonly account_selection: string | { readonly mode: "unverified" } | null;
  readonly client_auth_profile?: string;
}
export interface ServiceConnectorOperation {
  readonly destination_origin: string; readonly required_scopes?: readonly string[]; readonly read_only?: boolean;
}
export interface ServiceSetupClose {
  readonly process_joined: true;
  readonly exit_code: number | null;
  /** Requires the exact native close receipt, successful child exit and no
   * transport failure. Does not attest required-resource capability. */
  readonly native_resources_closed: boolean;
}
export class ServiceSetupError extends Error {}
export class ServiceSetupTimeout extends ServiceSetupError {
  constructor() { super("service observation pending; same native owner retained"); }
}
export class ServiceSetupEnded extends ServiceSetupError {
  constructor() { super("native service process ended"); }
}
export class ServiceSetupLaunchError extends ServiceSetupError {
  constructor(readonly owner: ServiceTransport) { super("service transport startup failed; native owner retained"); }
}
/** Payload access is explicit; inspection and JSON serialization omit it. */
export class ServiceSetupEvent {
  readonly #payload: Readonly<ObjectValue>;
  constructor(readonly kind: string, readonly request_id: string | null, payload: ObjectValue) {
    this.#payload = freeze(payload);
    Object.freeze(this);
  }
  get payload(): Readonly<ObjectValue> { return this.#payload; }
  toJSON(): ObjectValue { return { kind: this.kind, request_id: this.request_id }; }
  [inspect.custom](): ObjectValue { return this.toJSON(); }
}

export function object(value: unknown): value is ObjectValue {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && [Object.prototype, null].includes(Object.getPrototypeOf(value));
}
export function keys(value: ObjectValue, required: readonly string[], optional: readonly string[] = []): boolean {
  return required.every(key => Object.hasOwn(value, key))
    && Object.keys(value).every(key => required.includes(key) || optional.includes(key));
}
export function utf8(value: unknown, limit: number): value is string {
  if (typeof value !== "string" || !value.length || Buffer.byteLength(value) > limit) return false;
  // Buffer silently replaces isolated UTF-16 surrogates; native JSON does not.
  for (let i = 0; i < value.length; i++) {
    const c = value.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff) {
      const next = value.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
    } else if (c >= 0xdc00 && c <= 0xdfff) return false;
  }
  return true;
}
export function text(value: unknown, limit = 256): value is string {
  return utf8(value, limit) && !/[\x00-\x1f\x7f-\x9f]/u.test(value);
}
export function path(value: unknown): value is string {
  return utf8(value, MAX_FRAME) && isAbsolute(value) && !value.includes("\0");
}
function waitTime(value: number): number {
  if (!Number.isFinite(value) || value <= 0 || value > MAX_WAIT) throw new ServiceSetupError("invalid observation timeout");
  return value;
}
function freeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  }
  return value;
}
export function jsonSafe(value: unknown, depth = 0): void {
  if (depth > 256) throw new ServiceSetupError("invalid private JSON value");
  if (value === null || typeof value === "boolean") return;
  if (typeof value === "string" && (value === "" || utf8(value, MAX_FRAME))) return;
  if (typeof value === "number" && Number.isFinite(value)) return;
  if (Array.isArray(value)) { for (const child of value) jsonSafe(child, depth + 1); return; }
  if (object(value)) {
    for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(value))) {
      if (!(name === "" || utf8(name, MAX_FRAME)) || !Object.hasOwn(descriptor, "value")) throw new ServiceSetupError("invalid private JSON object");
      jsonSafe(descriptor.value, depth + 1);
    }
    return;
  }
  throw new ServiceSetupError("invalid private JSON value");
}
function encode(value: ObjectValue): Buffer {
  try {
    jsonSafe(value);
    const result = Buffer.from(JSON.stringify(value) + "\n");
    if (result.length > MAX_FRAME) throw new Error();
    return result;
  } catch { throw new ServiceSetupError("invalid or oversized private request"); }
}
function decode(line: Buffer): unknown {
  try {
    const source = new TextDecoder("utf-8", { fatal: true }).decode(line);
    const value: unknown = JSON.parse(source);
    const stack: Set<string>[] = [];
    visit(source, {
      onObjectBegin() { stack.push(new Set()); },
      onObjectProperty(name) { const current = stack.at(-1); if (!current || current.has(name)) throw new Error(); current.add(name); },
      onObjectEnd() { stack.pop(); },
      onError() { throw new Error(); },
    });
    jsonSafe(value);
    return value;
  } catch { throw new ServiceSetupError("invalid native service frame"); }
}
export function copy(value: ObjectValue): ObjectValue { return JSON.parse(encode(value).toString("utf8")) as ObjectValue; }
function equalTarget(value: unknown, target: ObjectValue): boolean {
  return object(value) && keys(value, Object.keys(target)) && Object.keys(target).every(key => value[key] === target[key]);
}
export function bindingWire(binding: ServiceSetupBinding): ObjectValue {
  const b: unknown = binding;
  const required = ["application_id", "instance_id", "origin", "principal_id", "login_scope_id", "expires_at_ms",
    "realm_id", "context_root", "state_root", "user_config_root", "resource_manifest_sha256", "credential_scope"];
  if (!object(b) || !keys(b, required, ["adapter", "model", "auth_binding"])) throw new Error();
  jsonSafe(b);
  if (![b.application_id, b.instance_id, b.principal_id, b.login_scope_id, b.realm_id].every(v => text(v))) throw new Error();
  if (!Number.isSafeInteger(b.expires_at_ms) || (b.expires_at_ms as number) <= Date.now()) throw new Error();
  if (!text(b.origin, 4096)) throw new Error();
  const origin = new URL(b.origin);
  if (!["http:", "https:"].includes(origin.protocol) || !origin.hostname || origin.username || origin.password
      || origin.search || origin.hash || origin.pathname !== "/") throw new Error();
  if (![b.context_root, b.state_root, b.user_config_root].every(v => path(v))) throw new Error();
  if (typeof b.resource_manifest_sha256 !== "string" || !/^[0-9a-f]{64}$/u.test(b.resource_manifest_sha256)) throw new Error();
  if (b.adapter !== undefined && b.adapter !== "services" && b.adapter !== "forge_services") throw new Error();
  if (b.model !== undefined && !text(b.model)) throw new Error();
  if (b.auth_binding !== undefined && (!object(b.auth_binding) || !keys(b.auth_binding, ["realm", "binding"], ["profile"])
      || !Object.values(b.auth_binding).every(v => text(v)))) throw new Error();
  const scope = b.credential_scope;
  if (!object(scope) || !(keys(scope, ["kind"]) && scope.kind === "native_default"
      || keys(scope, ["kind", "root"]) && scope.kind === "file" && path(scope.root))) throw new Error();
  return copy({ protocol_version: 5, adapter: b.adapter ?? "services",
    application: { application_id: b.application_id, instance_id: b.instance_id, origin: b.origin },
    viewer: { principal_id: b.principal_id, login_scope_id: b.login_scope_id, expires_at_ms: b.expires_at_ms },
    realm: { realm_id: b.realm_id, context_root: b.context_root, state_root: b.state_root, user_config_root: b.user_config_root },
    services: { resource_manifest_sha256: b.resource_manifest_sha256, preparation: "permitted" },
    ...(b.model === undefined ? {} : { model: b.model }), ...(b.auth_binding === undefined ? {} : { auth_binding: b.auth_binding }) });
}
function scopes(value: unknown): value is string[] { return Array.isArray(value) && value.length <= 256 && value.every(v => text(v, 4096)); }
function connectorTarget(value: unknown): ObjectValue {
  const required = ["slot_id", "issuer", "client", "resource", "scopes", "strategy_id", "account_selection"];
  if (!object(value) || !keys(value, required, ["client_auth_profile"])) throw new ServiceSetupError("closed native connector target required");
  if (!Object.keys(value).filter(k => !["scopes", "account_selection"].includes(k)).every(k => text(value[k], 4096)) || !scopes(value.scopes)) throw new ServiceSetupError("invalid native connector selector");
  const account = value.account_selection;
  if (account !== null && !text(account, 4096) && !(object(account) && keys(account, ["mode"]) && account.mode === "unverified")) throw new ServiceSetupError("closed native account selection required");
  return copy(value);
}
function connectorOperations(value: unknown): ObjectValue {
  if (!object(value) || !Object.keys(value).length || Object.keys(value).length > 256) throw new ServiceSetupError("explicit connector operations required");
  for (const [name, row] of Object.entries(value)) {
    if (!text(name, 4096) || !object(row) || !keys(row, ["destination_origin"], ["required_scopes", "read_only"])
        || !text(row.destination_origin, 4096) || Object.hasOwn(row, "required_scopes") && !scopes(row.required_scopes)
        || Object.hasOwn(row, "read_only") && typeof row.read_only !== "boolean") throw new ServiceSetupError("closed native connector operation required");
  }
  return copy(value);
}
function answerMatches(prompt: ObjectValue, answer: unknown): answer is ObjectValue {
  if (!object(answer)) return false;
  if (keys(answer, ["kind"]) && answer.kind === "cancel") return true;
  if (answer.kind !== prompt.kind) return false;
  switch (answer.kind) {
    case "service_secret": return keys(answer, ["kind", "job_id", "service", "slot", "value"])
      && ["job_id", "service", "slot"].every(k => text(answer[k], 4096) && answer[k] === prompt[k])
      && utf8(answer.value, 16384) && !answer.value.includes("\0");
    case "approval": return keys(answer, ["kind", "decision"]) && typeof answer.decision === "string" && ["approve", "deny"].includes(answer.decision);
    case "provider_target": return keys(answer, ["kind", "choice_id"]) && text(answer.choice_id, 4096)
      && Array.isArray(prompt.choices) && prompt.choices.some(c => object(c) && c.choice_id === answer.choice_id);
    case "provider_action": return keys(answer, ["kind", "action"]) && typeof answer.action === "string" && ["use", "enter_key", "sign_in", "back"].includes(answer.action)
      && Array.isArray(prompt.actions) && prompt.actions.includes(answer.action);
    case "provider_api_key": return keys(answer, ["kind", "key"]) && text(answer.key, 8192) && Boolean(answer.key.trim());
    default: return false;
  }
}

/** Internal shared transport contract. Not exported as a package subpath. */
export interface PreparedServiceLaunch {
  readonly executable: string;
  readonly args: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly initial: ObjectValue;
  readonly deadline: number;
  readonly version: 5 | 6 | 7;
}
export interface ServiceProtocol {
  readonly errorCodes?: ReadonlySet<string>;
  readonly events: ReadonlySet<string>;
  beforeRequest(method: string, params: ObjectValue): void;
  queued(id: string, method: string, params: ObjectValue): void;
  cancellable(id: string): boolean;
  retires(params: ObjectValue): boolean;
  observe(event: ServiceSetupEvent, viewerCurrent: boolean): void;
}
export function serviceEnvironment(binding: ServiceSetupBinding, environment?: Readonly<Record<string, string>>): Record<string, string> {
  const nativeDefault = binding.credential_scope.kind === "native_default";
  if (nativeDefault && environment === undefined) throw new Error();
  if (environment !== undefined) { jsonSafe(environment); if (!object(environment)) throw new Error(); }
  const env: Record<string, string> = environment === undefined ? { PATH: "/usr/bin:/bin", LANG: "C" } : { ...environment };
  const allowed = new Set([...ENVIRONMENT, ...(nativeDefault ? ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"] : [])]);
  if (Object.entries(env).some(([key, value]) => !allowed.has(key) || typeof value !== "string" || value.includes("\0"))) throw new Error();
  if (nativeDefault && !path(env.HOME)) throw new Error();
  return env;
}
export function setupLaunch(executable: string, binding: ServiceSetupBinding, environment?: Readonly<Record<string, string>>): PreparedServiceLaunch {
  try {
    const initial = bindingWire(binding);
    if (!path(executable)) throw new Error();
    const env = serviceEnvironment(binding, environment);
    const args = ["--stdio"];
    if (binding.credential_scope.kind === "file") args.push("--provider-store-root", binding.credential_scope.root);
    // Validate before spawning. The shared owner repeats encoding only for its
    // actual retained output frame; it never reconstructs host authority.
    encode({ jsonrpc: "2.0", id: INITIALIZE, method: "connect/initialize", params: initial });
    return { executable, args, env, initial, deadline: binding.expires_at_ms, version: 5 };
  } catch { throw new ServiceSetupError("invalid trusted service launch binding or environment"); }
}

/** One retained process. Request IDs acknowledge only local queuing. A timeout
 * never authorizes replay; close can observe the same owner again. */
export class ServiceTransport {
  #protocol: ServiceProtocol | undefined;
  #version: 5 | 6 | 7;
  #child: Child | undefined;
  #deadline: number;
  #outbound: Buffer[] = [];
  #outboundBytes = 0;
  #writing = false;
  #stopping = false;
  #input = Buffer.alloc(0);
  #inputBytes = 0;
  #events: { event: ServiceSetupEvent; bytes: number }[] = [];
  #eventBytes = 0;
  #requests = new Map<string, string>([[INITIALIZE, "connect/initialize"]]);
  #sendReferences = new Map<string, string>();
  #counter = 0;
  #scope: string | undefined;
  #prompt: { id: string; value: ObjectValue; answered: boolean } | undefined;
  #retired = false;
  #closing = false;
  #failed = false;
  #closed = false;
  #nativeClosed = false;
  #closeTarget: ObjectValue | undefined;
  #exitCode: number | null = null;
  #joined: Promise<void>;
  #resolveJoined!: () => void;
  #wake: (() => void) | undefined;
  #onClosed = (code: number | null): void => {
    if (this.#closed) return;
    this.#exitCode = code; this.#closed = true; this.#retired = true;
    this.#endInput(); this.#resolveJoined(); this.#wake?.();
  };

  protected constructor(launch: PreparedServiceLaunch, protocol?: ServiceProtocol) {
    this.#protocol = protocol;
    const { executable, args, env } = launch;
    this.#version = launch.version;
    this.#deadline = launch.deadline;
    const frame = encode({ jsonrpc: "2.0", id: INITIALIZE, method: "connect/initialize", params: launch.initial });
    this.#joined = new Promise(resolve => { this.#resolveJoined = resolve; });
    this.#enqueue(frame);
    try {
      this.#child = spawn(executable, args, { env, stdio: ["pipe", "pipe", "ignore"] });
      this.#child.once("error", () => this.#fail());
      this.#child.once("close", this.#onClosed);
      this.#child.stdin.on("error", () => this.#fail());
      this.#child.stdout.on("error", () => this.#fail());
      this.#child.stdout.on("data", (chunk: Buffer) => this.#read(chunk));
      this.#child.stdout.once("end", () => {
        if (this.#inputBytes) this.#fail();
        this.#input.fill(0); this.#input = Buffer.alloc(0); this.#inputBytes = 0; this.#endInput();
      });
      this.#pump();
    } catch {
      if (!this.#child) throw new ServiceSetupError("service setup launch failed");
      observeOwned.call(this.#child, "error", () => this.#fail());
      observeOwned.call(this.#child, "close", this.#onClosed);
      observeOwned.call(this.#child.stdout, "error", () => this.#fail());
      observeOwned.call(this.#child.stdin, "error", () => this.#fail());
      // Failed startup still drains the actual output pipe until close. Native
      // frames cannot establish cleanup after this transport failure.
      this.#child.stdout.resume();
      this.#fail();
      throw new ServiceSetupLaunchError(this);
    }
  }
  /** Diagnostic process identity only, never authority or completion. */
  get pid(): number | undefined { return this.#child?.pid; }
  /** Local transport/lifetime observation only. The native host remains the
   * authority for actual caller admission and resource access. */
  protected transportCurrent(): boolean {
    return !this.#retired && !this.#closing && !this.#closed && !this.#failed && this.#deadline > Date.now();
  }
  #target(): ObjectValue { return this.#scope === undefined ? { initialize_request_id: INITIALIZE } : { scope: this.#scope }; }
  #current(): void {
    if (this.#retired || this.#closing || this.#closed || this.#failed || this.#deadline <= Date.now()) throw new ServiceSetupError("service binding unavailable");
  }
  #enqueue(frame: Buffer): void {
    if (this.#stopping || this.#outbound.length + Number(this.#writing) >= MAX_QUEUE || this.#outboundBytes + frame.length > MAX_FRAME) throw new ServiceSetupError("private transport queue unavailable");
    this.#outbound.push(frame); this.#outboundBytes += frame.length;
  }
  #pump(): void {
    if (!this.#child || this.#writing || this.#stopping) return;
    const frame = this.#outbound.shift();
    if (!frame) return;
    this.#writing = true;
    let settled = false;
    const finish = (error?: Error | null): void => {
      if (settled) return;
      settled = true; this.#writing = false; this.#outboundBytes -= frame.length;
      if (error) this.#fail(); else this.#pump();
    };
    try { this.#child.stdin.write(frame, finish); } catch { finish(new Error()); }
  }
  #endInput(): void {
    if (this.#stopping) return;
    this.#stopping = true;
    this.#outboundBytes -= this.#outbound.reduce((sum, frame) => sum + frame.length, 0);
    this.#outbound = [];
    try { this.#child?.stdin.end(); } catch { this.#failed = true; }
  }
  #fail(): void { this.#failed = true; this.#retired = true; this.#endInput(); this.#wake?.(); }
  #request(method: string, params: ObjectValue, control = false): string {
    if (!control) this.#current();
    if (this.#closed || this.#failed || this.#requests.size >= 256) throw new ServiceSetupError("service request unavailable");
    if (method === "connect/send" && [...this.#requests.values()].includes(method)) throw new ServiceSetupError("a native setup send is already owned");
    const id = `h:${++this.#counter}`;
    this.#protocol?.beforeRequest(method, params);
    this.#enqueue(encode({ jsonrpc: "2.0", id, method, params }));
    this.#requests.set(id, method);
    if (method === "connect/cancel") this.#sendReferences.set(id, params.send_request_id as string);
    try { this.#protocol?.queued(id, method, params); }
    catch { this.#fail(); throw new ServiceSetupError("service request outcome unknown"); }
    this.#pump();
    return id;
  }
  submit(message: string): string {
    this.#current();
    if (!utf8(message, MAX_FRAME) || !message.trim()) throw new ServiceSetupError("nonempty UTF-8 setup message required");
    if (this.#scope === undefined) throw new ServiceSetupError("service initialization pending");
    return this.#request("connect/send", { scope: this.#scope, text: message });
  }
  cancel(sendRequestId: string): string {
    this.#current();
    if (this.#scope === undefined || (this.#requests.get(sendRequestId) !== "connect/send" && !this.#protocol?.cancellable(sendRequestId))) throw new ServiceSetupError("setup send is not current");
    try { return this.#request("connect/cancel", { scope: this.#scope, send_request_id: sendRequestId }); }
    catch { this.#fail(); throw new ServiceSetupError("service cancellation outcome unknown"); }
    finally { this.#retired = true; }
  }
  protected scopedRequest(method: string, params: ObjectValue): string {
    this.#current();
    if (this.#scope === undefined) throw new ServiceSetupError("service initialization pending");
    return this.#request(method, { ...params, scope: this.#scope });
  }
  connectorStart(selection: { readonly job_id: string; readonly connector: ServiceConnectorTarget;
    readonly redirect_uri: string; readonly operations: Readonly<Record<string, ServiceConnectorOperation>> }): string {
    let params: ObjectValue;
    try {
      jsonSafe(selection);
      if (!object(selection) || !keys(selection, ["job_id", "connector", "redirect_uri", "operations"])
          || !text(selection.job_id, 4096) || !text(selection.redirect_uri, 4096)) throw new Error();
      params = { job_id: selection.job_id, connector: connectorTarget(selection.connector),
        redirect_uri: selection.redirect_uri, operations: connectorOperations(selection.operations) };
    } catch { throw new ServiceSetupError("closed native connector start selection required"); }
    return this.scopedRequest("connect/connector/login/start", params);
  }
  connectorComplete(selection: { readonly state: string; readonly code: string; readonly host_attempt_handle?: string }): string {
    let params: ObjectValue;
    try {
      jsonSafe(selection);
      if (!object(selection) || !keys(selection, ["state", "code"], ["host_attempt_handle"]) || !text(selection.state, 16384)
          || !text(selection.code, 16384) || Object.hasOwn(selection, "host_attempt_handle") && !text(selection.host_attempt_handle, 4096)) throw new Error();
      params = copy(selection);
    } catch { throw new ServiceSetupError("closed connector completion required"); }
    return this.scopedRequest("connect/connector/login/complete", params);
  }
  connectorCancel(): string { return this.scopedRequest("connect/connector/login/cancel", {}); }
  answer(promptId: string, answer: ServiceSetupAnswer): void {
    this.#current();
    if (!this.#prompt || this.#prompt.id !== promptId || this.#prompt.answered) throw new ServiceSetupError("protected prompt is not current");
    try { jsonSafe(answer); if (!answerMatches(this.#prompt.value, answer)) throw new Error(); }
    catch { throw new ServiceSetupError("invalid protected answer"); }
    const frame = encode({ jsonrpc: "2.0", id: promptId, result: { answer } });
    const cancelling = answer.kind === "cancel";
    if (cancelling) this.#retired = true;
    try { this.#enqueue(frame); } catch {
      if (cancelling) this.#fail();
      throw new ServiceSetupError("private transport queue unavailable");
    }
    this.#prompt.answered = true; this.#pump();
  }
  #read(chunk: Buffer): void {
    if (this.#failed) return;
    try {
      // Split before concatenation so one chunk containing many legal frames
      // cannot bypass the per-frame bound or allocate an unbounded accumulator.
      let start = 0;
      while (start < chunk.length) {
        const newline = chunk.indexOf(10, start);
        const end = newline < 0 ? chunk.length : newline + 1;
        const part = chunk.subarray(start, end);
        const size = this.#inputBytes + part.length;
        if (size > MAX_FRAME) throw new Error();
        if (size > this.#input.length) {
          // Geometric growth bounds total copying for fragmented large frames.
          const grown = Buffer.allocUnsafe(Math.min(MAX_FRAME, Math.max(size, 4096, this.#input.length * 2)));
          this.#input.copy(grown, 0, 0, this.#inputBytes); this.#input.fill(0); this.#input = grown;
        }
        part.copy(this.#input, this.#inputBytes); this.#inputBytes = size;
        if (newline >= 0) {
          try { this.#accept(decode(this.#input.subarray(0, size)), size); }
          finally { this.#input.fill(0, 0, size); this.#inputBytes = 0; }
        }
        start = end;
      }
    } catch { this.#input.fill(0); this.#input = Buffer.alloc(0); this.#inputBytes = 0; this.#fail(); }
  }
  #accept(value: unknown, bytes: number): void {
    if (!object(value) || value.jsonrpc !== "2.0") throw new Error();
    const method = value.method, id = value.id;
    let event: ServiceSetupEvent;
    if (method === "connect/prompt" || method === "connect/event") {
      const params = value.params;
      if (!keys(value, method === "connect/prompt" ? ["jsonrpc", "method", "id", "params"] : ["jsonrpc", "method", "params"])
          || !object(params) || !equalTarget(params.target, this.#target())) throw new Error();
      if (method === "connect/prompt") {
        const prompt = params.prompt;
        if (!keys(params, ["target", "prompt"]) || !object(prompt) || typeof prompt.kind !== "string" || !PROMPTS.has(prompt.kind)
            || typeof id !== "string" || !/^p:[A-Za-z0-9_-]{1,64}$/u.test(id) || this.#prompt) throw new Error();
        const inactive = this.#retired || this.#closing || this.#deadline <= Date.now();
        this.#prompt = { id, value: inactive ? {} : prompt, answered: false };
        event = new ServiceSetupEvent(inactive ? "retired_prompt" : "prompt", id, inactive ? { prompt_kind: prompt.kind } : prompt);
      } else {
        if (typeof params.kind !== "string" || !(this.#protocol?.events ?? EVENTS).has(params.kind)) throw new Error();
        if (params.kind === "prompt_closed") {
          if (!this.#prompt || params.prompt_id !== this.#prompt.id) throw new Error();
          this.#prompt = undefined;
        } else if (params.kind === "unavailable" && (this.#protocol?.retires(params) ?? true)) this.#retired = true;
        event = new ServiceSetupEvent(params.kind, null, params);
      }
    } else {
      if (Object.hasOwn(value, "method") || typeof id !== "string") throw new Error();
      const request = this.#requests.get(id);
      if (!request || !(keys(value, ["jsonrpc", "id", "result"]) || keys(value, ["jsonrpc", "id", "error"]))) throw new Error();
      this.#requests.delete(id);
      const reference = this.#sendReferences.get(id); this.#sendReferences.delete(id);
      if (Object.hasOwn(value, "error")) {
        if (!object(value.error) || !Number.isSafeInteger(value.error.code)) throw new Error();
        const code = object(value.error.data) ? value.error.data.code : undefined;
        event = new ServiceSetupEvent("refused", id, { method: request, code: typeof code === "string" && (ERROR_CODES.has(code) || this.#protocol?.errorCodes?.has(code)) ? code : "native_refused" });
      } else {
        const result = value.result;
        if (!object(result)) throw new Error();
        if (request === "connect/initialize") {
          const caps = result.capabilities;
          if (!keys(result, ["protocol_version", "scope", "session_id", "capabilities", "state"]) || result.protocol_version !== this.#version
              || !text(result.scope) || !text(result.session_id) || !object(result.state) || !object(caps) || caps.service_preparation !== true
              || !Array.isArray(caps.prompt_kinds) || caps.prompt_kinds.length !== PROMPTS.size
              || new Set(caps.prompt_kinds).size !== PROMPTS.size || !caps.prompt_kinds.every(p => typeof p === "string" && PROMPTS.has(p))) throw new Error();
          this.#scope = result.scope;
        } else if (request === "connect/close") {
          if (!keys(result, ["target", "resources_closed"]) || !this.#closeTarget || !equalTarget(result.target, this.#closeTarget) || result.resources_closed !== true) throw new Error();
          this.#nativeClosed = true;
        } else {
          if (result.scope !== this.#scope) throw new Error();
          if (request === "connect/send" && (!keys(result, ["scope", "send_request_id", "completion", "state"])
              || result.send_request_id !== id || typeof result.completion !== "string" || !COMPLETIONS.has(result.completion) || !object(result.state))) throw new Error();
          if (request === "connect/cancel" && (!keys(result, ["scope", "send_request_id", "status"])
              || result.send_request_id !== reference || typeof result.status !== "string" || !["cancellation_requested", "not_current"].includes(result.status))) throw new Error();
          if (request === "connect/connector/login/start" && (!keys(result, ["scope", "started"]) || !object(result.started))) throw new Error();
          if (request === "connect/connector/login/complete" && (!keys(result, ["scope", "completed"]) || !object(result.completed))) throw new Error();
          if (request === "connect/connector/login/cancel" && (!keys(result, ["scope", "cancelled"]) || result.cancelled !== true)) throw new Error();
        }
        event = new ServiceSetupEvent("result", id, { method: request, result });
      }
    }
    this.#protocol?.observe(event, this.transportCurrent());
    if (this.#events.length >= MAX_QUEUE || this.#eventBytes + bytes > MAX_FRAME) throw new Error();
    this.#events.push({ event, bytes }); this.#eventBytes += bytes; this.#wake?.();
  }
  async nextEvent(timeoutMs = 60_000): Promise<ServiceSetupEvent> {
    waitTime(timeoutMs);
    if (this.#wake) throw new ServiceSetupError("one event observer already owns this channel");
    const take = (): ServiceSetupEvent | undefined => {
      const next = this.#events.shift();
      if (next) { this.#eventBytes -= next.bytes; return next.event; }
      if (this.#failed) throw new ServiceSetupError("service transport failed; retain native owner");
      if (this.#closed) throw new ServiceSetupEnded();
      return undefined;
    };
    const ready = take();
    if (ready) return ready;
    return new Promise<ServiceSetupEvent>((resolve, reject) => {
      const done = (): void => { clearTimeout(timer); this.#wake = undefined; };
      const timer = setTimeout(() => { done(); reject(new ServiceSetupTimeout()); }, timeoutMs);
      this.#wake = () => {
        try { const next = take(); if (next) { done(); resolve(next); } }
        catch (error) { done(); reject(error); }
      };
    });
  }
  async close(timeoutMs = 150_000): Promise<ServiceSetupClose> {
    waitTime(timeoutMs);
    const first = !this.#closing && !this.#closed;
    this.#closing = true;
    if (first) {
      this.#closeTarget = this.#target();
      try { this.#request("connect/close", { target: this.#closeTarget }, true); } catch { this.#endInput(); }
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([this.#joined, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new ServiceSetupTimeout()), timeoutMs); })]);
    } finally { if (timer) clearTimeout(timer); }
    return Object.freeze({ process_joined: true, exit_code: this.#exitCode,
      native_resources_closed: this.#nativeClosed && !this.#failed && this.#exitCode === 0 });
  }
}
