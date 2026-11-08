/** Trusted application adapters for the existing local Connect pipe.
 * Native owns identity/held settlement; the application owns its durable row.
 */
import { durableAgentSpecToDict } from "@rkat/meta-sdk";
import type { DurableAgentSpec } from "@rkat/meta-sdk";
import { HostControlError, HostObservationTimeout, HostRefusedError, HostTransportError, PrivateHostClient } from "./private-host.js";
import type { HostRefusal } from "./private-host.js";
import { receiptJson, serviceBody } from "./connectors/json.js";

type Data = Record<string, unknown>;
const refusals = ["source_conflict", "invalid_selection", "unavailable", "denied", "wrong_owner", "not_settled", "observation_changed"];
function invalid(): never { throw new HostControlError("invalid member class protocol data"); }
function object(value: unknown, keys: readonly string[], optional: readonly string[] = []): Data {
  if (!value || typeof value !== "object" || Array.isArray(value)) invalid();
  const row = value as Data;
  if (keys.some(key => !(key in row)) || Object.keys(row).some(key => !keys.includes(key) && !optional.includes(key))) invalid();
  return row;
}
function id(value: unknown): string {
  if (typeof value !== "string" || !value || value.length > 512 || value.trim() !== value || /[\u0000-\u001f\u007f]/u.test(value)) invalid();
  return value;
}
function integer(value: unknown, min = 0): number { if (typeof value !== "number" || !Number.isSafeInteger(value) || value < min) invalid(); return value; }
function copy<T>(value: T): T { try { return receiptJson(serviceBody(value)) as T; } catch { return invalid(); } }
function equal(a: unknown, b: unknown): boolean {
  const canonical = (value: unknown): unknown => Array.isArray(value) ? value.map(canonical) : value && typeof value === "object"
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical((value as Data)[key])])) : value;
  return JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
}
function refusal(value: unknown): HostRefusal { if (typeof value !== "string" || !refusals.includes(value)) invalid(); return value as HostRefusal; }
export interface MemberBinding { readonly mob_id: string; readonly member: string; readonly role: string }
export interface MemberClassCheckpoint { readonly admission: string; readonly revision: number }
export interface MemberMcpAdmissionSelection { readonly admission_id: string; readonly identity: string; readonly class: string }
export interface RetainedMemberMcpBindings { readonly selection: MemberMcpAdmissionSelection; readonly bindings: readonly string[] }
/** The native lifetime wire shape; only native evaluates the restrictions. */
export interface MemberMcpLifetime {
  readonly bound: { readonly bound: "unrestricted" | "empty" }
    | { readonly bound: "window"; readonly window: { readonly not_before_ms: number; readonly expires_at_ms: number } };
  readonly unresolved: readonly unknown[];
}
export interface MemberMcpAddIntent {
  readonly origin: "operator_self";
  readonly console_enrollment_id: number;
  readonly services: readonly { readonly server: string; readonly operations: readonly string[]; readonly lifetime: MemberMcpLifetime }[];
}
export interface MemberClassAddRequest {
  readonly id: string;
  readonly instance: { readonly class: string; readonly member: { readonly member: MemberBinding; readonly logical_owner: Readonly<Data>; readonly context_generation: number; readonly audience: Readonly<Data> } };
  readonly spec: DurableAgentSpec;
  readonly route: Readonly<Data>;
  readonly controller: Readonly<Data>;
  readonly holdTimeoutMs?: number;
  readonly expectedPredecessor?: MemberClassCheckpoint;
  readonly mcp?: MemberMcpAddIntent;
}
export type MemberClassControlOutcome = { readonly state: "succeeded" } | { readonly state: "refused"; readonly error: HostRefusal };
export type MemberClassObservation =
  | { readonly state: "reservation_not_accepted"; readonly error: HostRefusal }
  | { readonly state: "preflight_refused"; readonly error: HostRefusal; readonly cleanup: MemberClassControlOutcome }
  | { readonly state: "unsettled"; readonly settlement_id: number }
  | { readonly state: "settled"; readonly native: { readonly state: "result"; readonly result: { readonly outcomes: Readonly<Data>; readonly managed_edges: readonly unknown[] } } | { readonly state: "refused"; readonly message: string; readonly data: unknown }; readonly authorization: MemberClassControlOutcome; readonly cleanup: MemberClassControlOutcome }
  | { readonly state: "native_refused"; readonly message: string; readonly data: unknown }
  | { readonly state: "observed"; readonly record: Readonly<Data>; readonly checkpoint: MemberClassCheckpoint; readonly authorization: MemberClassControlOutcome }
  | { readonly state: "retired" };
function member(value: unknown): MemberBinding {
  const row = object(value, ["mob_id", "member", "role"]);
  return { mob_id: id(row.mob_id), member: id(row.member), role: id(row.role) };
}
function checkpoint(value: unknown): MemberClassCheckpoint { const row = object(value, ["admission", "revision"]); return { admission: id(row.admission), revision: integer(row.revision) }; }
function selection(value: unknown): MemberMcpAdmissionSelection {
  const row = object(value, ["admission_id", "identity", "class"]);
  return { admission_id: id(row.admission_id), identity: id(row.identity), class: id(row.class) };
}
function retained(value: unknown): RetainedMemberMcpBindings {
  const row = object(value, ["selection", "bindings"]);
  if (!Array.isArray(row.bindings) || row.bindings.length > 32) invalid();
  const bindings = row.bindings.map(id); if (new Set(bindings).size !== bindings.length) invalid();
  return { selection: selection(row.selection), bindings };
}
function addOperation(request: MemberClassAddRequest): Data {
  const row = object(request, ["id", "instance", "spec", "route", "controller"], ["holdTimeoutMs", "expectedPredecessor", "mcp"]);
  const instance = object(copy(row.instance), ["class", "member"]);
  const destination = object(instance.member, ["member", "logical_owner", "context_generation", "audience"]);
  const binding = member(destination.member);
  id(instance.class); integer(destination.context_generation, 1);
  const spec = copy(durableAgentSpecToDict(request.spec));
  id(spec.identity); id(spec.profile);
  if (request.spec.initialMessage != null || request.spec.placement != null || binding.role !== spec.profile) invalid();
  const route = object(copy(row.route), ["id", "origin", "logical_executor", "represented_subject", "source_authority", "authentication_domain", "event_domain"], ["application_parent", "controller_parent"]);
  if (route.application_parent != null || route.controller_parent != null) invalid();
  const origin = object(route.origin, ["kind", "member", "requester", "ingress_actor"]);
  if (origin.kind !== "local_console" || !equal(member(origin.member), binding)) invalid();
  const controller = object(copy(row.controller), ["target", "issuance_id"], ["controller_route"]);
  if (!equal(controller.target, { kind: "original_route", route: id(route.id) })) invalid();
  id(controller.issuance_id);
  const hold = integer(request.holdTimeoutMs ?? 60_000, 1); if (hold > 900_000) invalid();
  const result: Data = { kind: "add", id: id(row.id), instance, spec, route, controller, hold_timeout_ms: hold };
  if (request.expectedPredecessor !== undefined) result.expected_predecessor = checkpoint(request.expectedPredecessor);
  if (request.mcp !== undefined) {
    const mcp = object(copy(request.mcp), ["origin", "console_enrollment_id", "services"]);
    if (mcp.origin !== "operator_self" || !Array.isArray(mcp.services) || !mcp.services.length || mcp.services.length > 32) invalid();
    integer(mcp.console_enrollment_id, 1);
    const servers = new Set<string>();
    for (const value of mcp.services) {
      const service = object(value, ["server", "operations", "lifetime"]); const server = id(service.server);
      if (servers.has(server) || !Array.isArray(service.operations) || !service.operations.length || service.operations.length > 256) invalid();
      servers.add(server); const operations = service.operations.map(id); if (new Set(operations).size !== operations.length) invalid();
      const lifetime = object(service.lifetime, ["bound", "unresolved"]);
      const bound = object(lifetime.bound, ["bound"], ["window"]);
      if (bound.bound === "window") {
        const window = object(bound.window, ["not_before_ms", "expires_at_ms"]);
        integer(window.not_before_ms); integer(window.expires_at_ms);
      } else if ((bound.bound !== "unrestricted" && bound.bound !== "empty") || "window" in bound) invalid();
      if (!Array.isArray(lifetime.unresolved)) invalid();
    }
    result.mcp = mcp;
  }
  return copy(result);
}
function control(value: unknown): void {
  const row = value as Data;
  if (row?.state === "succeeded") object(row, ["state"]);
  else if (row?.state === "refused") { object(row, ["state", "error"]); refusal(row.error); }
  else invalid();
}
export function decodeMemberClassObservation(reply: unknown, operation: Data, correlation: number, states: readonly string[]): MemberClassObservation {
  const outer = object(reply, ["kind", "observation"]); if (outer.kind !== "member_class") invalid();
  const value = outer.observation as Data;
  if (!value || typeof value.state !== "string" || !states.includes(value.state)) invalid();
  switch (value.state) {
    case "reservation_not_accepted": object(value, ["state", "error"]); refusal(value.error); break;
    case "preflight_refused": object(value, ["state", "error", "cleanup"]); refusal(value.error); control(value.cleanup); break;
    case "retired": object(value, ["state"]); break;
    case "unsettled": object(value, ["state", "settlement_id"]); if (integer(value.settlement_id, 1) !== correlation) invalid(); break;
    case "native_refused": object(value, ["state", "message", "data"]); if (typeof value.message !== "string") invalid(); break;
    case "settled": {
      object(value, ["state", "native", "authorization", "cleanup"]); control(value.authorization); control(value.cleanup);
      const native = value.native as Data;
      if (native?.state === "result") {
        object(native, ["state", "result"]); const result = object(native.result, ["outcomes", "managed_edges"]);
        if (!result.outcomes || typeof result.outcomes !== "object" || Array.isArray(result.outcomes) || !Array.isArray(result.managed_edges)) invalid();
        if (operation.kind === "add" && !equal(Object.keys(result.outcomes), [(operation.spec as Data).identity])) invalid();
        if ("identity" in operation && !equal(Object.keys(result.outcomes), [operation.identity])) invalid();
      } else if (native?.state === "refused") { object(native, ["state", "message", "data"]); if (typeof native.message !== "string") invalid(); }
      else invalid();
      break;
    }
    case "observed": {
      object(value, ["state", "record", "checkpoint", "authorization"]); control(value.authorization);
      const prior = checkpoint(value.checkpoint); const record = value.record as Data;
      const instance = record?.instance as Data; const selected = instance?.member as Data;
      if (record?.id !== operation.id || prior.admission !== operation.id || !equal(selected?.member, operation.member)) invalid();
      break;
    }
  }
  return copy(value) as unknown as MemberClassObservation;
}
function deferred<T>(): { promise: Promise<T>; resolve(value: T): void; reject(error: unknown): void } {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); void promise.catch(() => {});
  return { promise, resolve, reject };
}
export class MemberClassObservationUnknown extends Error {
  constructor(readonly attempt: MemberClassAddAttempt<unknown>) { super("member observation unavailable; native settlement is unknown"); }
}
export class MemberClassObservationTimeout extends MemberClassObservationUnknown {}
export class MemberClassListingUnknown extends Error {}

/** One submitted Add. Observation loss never replays it or cancels native work. */
export class MemberClassAddAttempt<T = MemberClassObservation> {
  readonly operationId: number;
  readonly #client: PrivateHostClient; readonly #operation: Data; readonly #publish: () => Promise<void>;
  readonly #listing = deferred<void>(); readonly #result: Promise<T>;
  readonly #selection: MemberMcpAdmissionSelection; readonly #commission: boolean;
  #listingFailure: unknown = null; #listingDone = false; #requested = false; #terminal = false; #unsettled = false; #closed = false; #task: Promise<void> | null = null;
  /** @internal Commissioning shares this listing owner, but sends its own command. */
  constructor(client: PrivateHostClient, operation: Data, publish: () => Promise<void>, timeoutMs?: number,
    commission?: { decode(reply: Data, correlation: number): T }) {
    this.#client = client; this.#operation = operation; this.#publish = publish; this.#commission = !!commission;
    this.#selection = commission
      ? { admission_id: id(operation.admission_id), identity: id(operation.identity), class: id((operation.instance as Data).class) }
      : { admission_id: id(operation.id), identity: id((operation.spec as Data).identity), class: id((operation.instance as Data).class) };
    const publishCorrelation = (correlation: number) => client.memberClasses._retain(correlation, this);
    const started = commission
      ? client._workflowCommissionCall<T>(operation, (reply, correlation) => {
          const value = commission.decode(reply, correlation); this._observed({ state: "commissioned" }); return value;
        }, timeoutMs, publishCorrelation)
      : client._memberCall<T>(operation, (reply, correlation) => {
          const value = decodeMemberClassObservation(reply, operation, correlation, ["reservation_not_accepted", "preflight_refused", "settled", "unsettled", "native_refused"]);
          this._observed(value); return value as T;
        }, timeoutMs, false, publishCorrelation);
    this.operationId = started.id;
    this.#result = started.result.catch(error => {
      if (error instanceof HostObservationTimeout) throw new MemberClassObservationTimeout(this);
      if (error instanceof HostTransportError) throw new MemberClassObservationUnknown(this);
      this.#terminal = true; this.#finishWithoutListing(); throw error;
    });
    void this.#result.catch(() => {});
  }
  result(): Promise<T> { return this.#result.then(copy); }
  joinListing(): Promise<void> { return this.#listing.promise; }
  _listing(reply: unknown): void {
    const row = object(reply, ["kind", "operation_id", "admission_id", "identity", "class"]);
    if (row.kind !== "member_class_listing_required" || row.operation_id !== this.operationId || row.admission_id !== this.#selection.admission_id
      || row.identity !== this.#selection.identity || row.class !== this.#selection.class
      || this.#requested || this.#terminal || this.#closed) invalid();
    this.#requested = true;
    this.#task = this.#runListing(); void this.#task.catch(() => {});
  }
  _observed(value: { state: string }): void {
    if (value.state === "unsettled") this.#unsettled = true; else this.#terminal = true;
    this.#finishWithoutListing();
    if (this.#listingDone && this.#terminal && this.#listingFailure === null) this.#client.memberClasses._forget(this.operationId);
  }
  _joined(reply: unknown, correlation: number): MemberClassObservation {
    if (this.#commission) invalid();
    const value = decodeMemberClassObservation(reply, this.#operation, correlation, ["settled", "native_refused"]);
    this._observed(value); return value;
  }
  _lost(): void { this.#closed = true; this.#finishWithoutListing(); }
  #finishWithoutListing(): void {
    if (this.#task !== null || this.#requested) return;
    if (this.#closed) this.#listing.reject(new MemberClassListingUnknown("private pipe lost; application listing is unknown"));
    else if (this.#terminal) this.#listing.resolve();
    if (this.#closed || this.#terminal) this.#client.memberClasses._forget(this.operationId);
  }
  async #runListing(): Promise<void> {
    let failure: unknown = null, accepted = false;
    try {
      this.#client.assertAvailable();
      if (!this.#terminal && !this.#unsettled && !this.#closed) { await this.#publish(); accepted = true; }
    } catch { failure = new MemberClassListingUnknown("application listing did not settle; reconcile its durable row"); }
    try {
      await this.#client._memberCall({ kind: "listing_result", operation_id: this.operationId, admission_id: this.#selection.admission_id,
        identity: this.#selection.identity, class: this.#selection.class, accepted }, reply => {
        const row = object(reply, ["kind", "operation_id", "admission_id", "identity", "class"]);
        if (row.kind !== "member_class_listing_acknowledged" || row.operation_id !== this.operationId || row.admission_id !== this.#selection.admission_id
          || row.identity !== this.#selection.identity || row.class !== this.#selection.class) invalid();
      }, undefined, true).result;
    } catch (error) {
      failure = new MemberClassListingUnknown("application listing acknowledgment is unknown; reconcile its durable row");
      if (!(error instanceof HostRefusedError)) this.#client._memberTransportFailed();
    }
    this.#listingFailure = failure; this.#listingDone = true;
    if (failure) this.#listing.reject(failure); else this.#listing.resolve();
    if ((this.#terminal || this.#closed) && failure === null) this.#client.memberClasses._forget(this.operationId);
  }
  async _joinEntered(): Promise<void> { if (this.#task) { await this.#task; await this.#listing.promise; } }
}

export class MemberClassClient {
  readonly #client: PrivateHostClient; readonly #attempts = new Map<number, MemberClassAddAttempt<unknown>>(); #closed = false;
  constructor(client: PrivateHostClient) { this.#client = client; }
  beginAdd(request: MemberClassAddRequest, publish: () => Promise<void>, options: { timeoutMs?: number } = {}): MemberClassAddAttempt {
    this.#client.assertReady(); if (this.#closed || typeof publish !== "function") invalid();
    return new MemberClassAddAttempt(this.#client, addOperation(request), publish, options.timeoutMs);
  }
  add(request: MemberClassAddRequest, publish: () => Promise<void>, options: { timeoutMs?: number } = {}): Promise<MemberClassObservation> {
    return this.beginAdd(request, publish, options).result();
  }
  join(settlementId: number, options: { timeoutMs?: number } = {}): Promise<MemberClassObservation> {
    const operation = { kind: "join", settlement_id: integer(settlementId, 1) };
    return this.#client._memberCall(operation, (reply, correlation) => {
      return this.#client.memberClasses._joined(settlementId, reply, operation, correlation);
    }, options.timeoutMs).result;
  }
  observe(admission: string, selected: MemberBinding, options: { timeoutMs?: number } = {}): Promise<MemberClassObservation> {
    const operation = { kind: "observe", id: id(admission), member: member(copy(selected)) };
    return this.#client._memberCall(operation, (reply, correlation) => decodeMemberClassObservation(reply, operation, correlation, ["observed"]), options.timeoutMs).result;
  }
  retire(admission: string, selected: MemberBinding, options: { timeoutMs?: number } = {}): Promise<MemberClassObservation> {
    const operation = { kind: "retire", id: id(admission), member: member(copy(selected)) };
    return this.#client._memberCall(operation, (reply, correlation) => decodeMemberClassObservation(reply, operation, correlation, ["retired"]), options.timeoutMs).result;
  }
  _joined(id: number, reply: unknown, operation: Data, correlation: number): MemberClassObservation {
    return this.#attempts.get(id)?._joined(reply, correlation) ?? decodeMemberClassObservation(reply, operation, correlation, ["settled", "native_refused"]);
  }
  _retain(id: number, attempt: MemberClassAddAttempt<unknown>): void { if (this.#attempts.size >= 128) invalid(); this.#attempts.set(id, attempt); }
  _refused(id: number): void { this.#attempts.get(id)?._observed({ state: "refused" }); }
  _forget(id: number): void { this.#attempts.delete(id); }
  _listing(id: number, reply: unknown): void { const attempt = this.#attempts.get(id); if (!attempt) invalid(); attempt._listing(reply); }
  _lost(): void { this.#closed = true; for (const attempt of this.#attempts.values()) attempt._lost(); }
  async _joinEntered(): Promise<void> {
    const joined = await Promise.allSettled([...this.#attempts.values()].map(attempt => attempt._joinEntered()));
    const failures = joined.filter((value): value is PromiseRejectedResult => value.status === "rejected");
    if (failures.length) throw new AggregateError(failures.map(value => value.reason), "member listing callbacks require reconciliation");
  }
}

/** One runtime's application callbacks, never another durable store.
 * Missing/retired selection must throw; [] is an explicit retained empty set.
 */
export class MemberMcpApplication {
  readonly #retain: (value: RetainedMemberMcpBindings) => Promise<void>;
  readonly #lookup: (value: MemberMcpAdmissionSelection) => Promise<RetainedMemberMcpBindings>;
  readonly #tasks = new Set<Promise<void>>(); readonly #failures: unknown[] = [];
  #client: PrivateHostClient | null = null; #lastExchange = 0; #closed = false; #joining: Promise<void> | null = null;
  constructor(callbacks: { retain(value: RetainedMemberMcpBindings): Promise<void>; lookup(value: MemberMcpAdmissionSelection): Promise<RetainedMemberMcpBindings> }) {
    if (typeof callbacks.retain !== "function" || typeof callbacks.lookup !== "function") invalid();
    this.#retain = callbacks.retain; this.#lookup = callbacks.lookup;
  }
  _request(client: PrivateHostClient, reply: unknown): void {
    const row = object(reply, ["kind", "exchange_id", "selection", "bindings"]);
    const exchange = integer(row.exchange_id, 1); const selected = selection(row.selection);
    if (row.kind !== "member_class_mcp_required" || this.#closed || (this.#client && this.#client !== client)
      || exchange <= this.#lastExchange || this.#tasks.size >= 128) invalid();
    const proposed = row.bindings === null ? null : retained({ selection: selected, bindings: row.bindings });
    this.#client = client; this.#lastExchange = exchange;
    const task = this.#run(client, exchange, selected, proposed); this.#tasks.add(task);
    void task.finally(() => this.#tasks.delete(task)).catch(() => {});
  }
  async #run(client: PrivateHostClient, exchange: number, selected: MemberMcpAdmissionSelection, proposed: RetainedMemberMcpBindings | null): Promise<void> {
    let value: RetainedMemberMcpBindings | null = null;
    try {
      client.assertAvailable(); if (this.#closed) invalid();
      if (proposed) { await this.#retain(copy(proposed)); value = proposed; }
      else value = retained(await this.#lookup(copy(selected)));
      if (!equal(value.selection, selected)) invalid();
    } catch (error) { this.#failures.push(error); value = null; }
    try {
      await client._memberCall({ kind: "mcp_result", exchange_id: exchange, selection: selected, retained: value }, reply => {
        const row = object(reply, ["kind", "exchange_id"]);
        if (row.kind !== "member_class_mcp_acknowledged" || row.exchange_id !== exchange) invalid();
      }, undefined, true).result;
    } catch (error) {
      this.#failures.push(error);
      // A decoded native refusal is a settled reply, not pipe loss. A prewrite
      // failure must wake native's waiting exchange through the existing pipe.
      if (!(error instanceof HostRefusedError)) client._memberTransportFailed();
    }
  }
  _close(): void { this.#closed = true; }
  async join(): Promise<void> {
    this._close();
    this.#joining ??= (async () => {
      await Promise.all([...this.#tasks]);
      if (this.#failures.length) throw new AggregateError([...this.#failures], "member MCP callbacks require reconciliation");
    })();
    return this.#joining;
  }
}
