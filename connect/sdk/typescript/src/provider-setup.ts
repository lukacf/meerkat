/** Trusted-host provider-only Connect. Never starts a model or installed host. */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { isAbsolute } from "node:path";

const PROTOCOL = "connect-provider-setup/1";
const MAX_FRAME = 1024 * 1024;
// Node timers cannot represent a larger single observation delay.
const MAX_OBSERVATION_MS = 2_147_483_647;
const ENVIRONMENT = new Set(["PATH", "LANG", "LC_ALL", "TMPDIR", "TMP", "TEMP", "SYSTEMROOT"]);
export interface ProviderSetupBinding {
  application_id: string; instance_id: string; principal_id: string; login_scope_id: string;
  expires_at_ms: number; realm_id: string; context_root: string; state_root: string;
  user_config_root: string; credential_scope: { kind: "native_default" } | { kind: "file"; root: string }; browser: "local_loopback" | "manual_return";
}
export interface ProviderSelection {
  interaction_id: string; model: string; provider: string;
  auth_binding: { realm: string; binding: string; profile?: string | null; origin?: string };
  credential: string; live_entitlement: "unobserved";
}
export type ProviderAnswer = { kind: "choice"; choice_id: string } | { kind: "confirm" }
  | { kind: "action"; action: "use" | "enter_key" | "sign_in" | "back" }
  | { kind: "key"; key: string } | { kind: "callback"; url: string } | { kind: "cancel" };
export class ProviderSetupError extends Error {}
type ObjectValue = Record<string, unknown>;
function object(value: unknown): ObjectValue {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new ProviderSetupError("invalid provider object");
  return value as ObjectValue;
}
function nonempty(value: unknown): value is string { return typeof value === "string" && value.length > 0; }
export function decodeProviderSelection(raw: unknown): ProviderSelection {
  const value = object(raw);
  const keys = ["interaction_id", "model", "provider", "auth_binding", "credential", "live_entitlement"];
  if (Object.keys(value).length !== keys.length || keys.some(key => !(key in value))
      || ["interaction_id", "model", "provider", "credential"].some(key => !nonempty(value[key]))
      || value.live_entitlement !== "unobserved") throw new ProviderSetupError("invalid provider selection");
  const auth = object(value.auth_binding);
  if (!nonempty(auth.realm) || !nonempty(auth.binding)
      || Object.keys(auth).some(key => !["realm", "binding", "profile", "origin"].includes(key))
      || (auth.profile != null && !nonempty(auth.profile))
      || ("origin" in auth && auth.origin !== "configured")) throw new ProviderSetupError("invalid native provider binding");
  return value as unknown as ProviderSelection;
}
function bindingData(binding: ProviderSetupBinding): ObjectValue {
  const fields = ["application_id", "instance_id", "principal_id", "login_scope_id", "expires_at_ms", "realm_id",
    "context_root", "state_root", "user_config_root", "credential_scope", "browser"];
  const value = object(binding);
  if (Object.keys(value).length !== fields.length || fields.some(key => !(key in value))) throw new ProviderSetupError("invalid trusted binding fields");
  for (const name of ["application_id", "instance_id", "principal_id", "login_scope_id", "realm_id"]) {
    const v = value[name];
    if (!nonempty(v) || v.length > 256 || /[\x00-\x1f\x7f]/u.test(v)) throw new ProviderSetupError("invalid trusted viewer binding");
  }
  for (const name of ["context_root", "state_root", "user_config_root"]) {
    const v = value[name];
    if (typeof v !== "string" || !isAbsolute(v) || v.includes("\0")) throw new ProviderSetupError("provider roots must be trusted absolute paths");
  }
  const scope = object(binding.credential_scope);
  if (!(scope.kind === "native_default" && Object.keys(scope).length === 1)
      && !(scope.kind === "file" && Object.keys(scope).length === 2 && typeof scope.root === "string" && isAbsolute(scope.root) && !scope.root.includes("\0"))) throw new ProviderSetupError("invalid trusted credential scope");
  if (!Number.isSafeInteger(binding.expires_at_ms) || binding.expires_at_ms <= Date.now()
      || !["local_loopback", "manual_return"].includes(binding.browser)) throw new ProviderSetupError("invalid provider deadline or transport");
  return { protocol: PROTOCOL, ...binding };
}

/** Keeps the actual child even after an observation timeout. close() can be
 * called again; it never reports a timeout, process signal or auth click as a
 * native joined completion. Sensitive answers are never logged. */
export class ProviderSetupClient {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #joined: Promise<void>;
  #buffer = Buffer.alloc(0);
  #events: ObjectValue[] = [];
  #waiters: (() => void)[] = [];
  #failure: Error | undefined;
  #terminal: ObjectValue | undefined;
  #interaction: string | undefined;
  #pending: string | undefined;
  #counter = 0;
  #cancelled = false;
  #closed = false;

  constructor(executable: string, binding: ProviderSetupBinding, environment?: Record<string, string>) {
    const selected = bindingData(binding);
    const nativeDefault = binding.credential_scope.kind === "native_default";
    if (nativeDefault && (!environment || !environment.HOME || !isAbsolute(environment.HOME))) throw new ProviderSetupError("native-default launch requires an explicit fixed HOME environment");
    const env = environment ?? { PATH: "/usr/bin:/bin", LANG: "C" };
    const allowed = new Set([...ENVIRONMENT, ...(nativeDefault ? ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"] : [])]);
    if (!isAbsolute(executable) || Object.keys(env).some(key => !allowed.has(key))
        || Object.values(env).some(value => typeof value !== "string" || value.includes("\0"))) throw new ProviderSetupError("invalid isolated provider launch");
    const initialization = JSON.stringify({ method: "initialize", id: "initialize", binding: selected }) + "\n";
    if (Buffer.byteLength(initialization) > MAX_FRAME) throw new ProviderSetupError("provider initialization exceeds frame bound");
    // No process.env spread, HOME mutation, shell or caller-controlled argv.
    const scopeArgs = binding.credential_scope.kind === "file" ? ["--provider-store-root", binding.credential_scope.root] : ["--provider-native-default"];
    this.#child = spawn(executable, ["--provider-stdio", ...scopeArgs], { env: { ...env }, stdio: ["pipe", "pipe", "pipe"], detached: true });
    this.#joined = new Promise(resolve => this.#child.once("close", (code) => {
      this.#closed = true;
      if (code !== 0 || !this.#terminal) this.#failure ??= new ProviderSetupError("provider ended without successful transport closure");
      this.#wake(); resolve();
    }));
    try {
    this.#child.once("error", () => this.#fail("provider process failed"));
    this.#child.stdin.on("error", () => this.#fail("provider input failed"));
    this.#child.stdout.on("error", () => this.#fail("provider output failed"));
    // Drain diagnostics without retaining provider URLs, callback or secrets.
    this.#child.stderr.on("data", () => {});
    this.#child.stdout.on("data", (chunk: Buffer) => {
      try {
        this.#buffer = Buffer.concat([this.#buffer, chunk]);
        let newline: number;
        while ((newline = this.#buffer.indexOf(10)) >= 0) {
          if (newline > MAX_FRAME) throw new ProviderSetupError("provider frame too large");
          const line = this.#buffer.subarray(0, newline);
          this.#buffer = this.#buffer.subarray(newline + 1);
          this.#accept(object(JSON.parse(line.toString("utf8"))));
        }
        if (this.#buffer.length > MAX_FRAME) throw new ProviderSetupError("provider frame too large");
      } catch { this.#fail("invalid provider response"); }
    });
    this.#child.stdout.once("end", () => {
      if (this.#buffer.length) this.#fail("truncated provider response");
    });
    this.#child.stdin.write(initialization);
    } catch {
      // Preserve the returned client and its close observer after any
      // synchronous setup/write failure. EOF retires the native interaction.
      this.#fail("provider initialization transport failed");
    }
  }
  #wake(): void { for (const resolve of this.#waiters.splice(0)) resolve(); }
  #fail(message: string): void {
    this.#failure ??= new ProviderSetupError(message);
    this.#child.stdin.end(); // native EOF retirement, never unowned kill/retry
    this.#wake();
  }
  #send(value: ObjectValue): void {
    const encoded = JSON.stringify(value) + "\n";
    if (Buffer.byteLength(encoded) > MAX_FRAME || this.#closed || this.#failure) throw new ProviderSetupError("provider input unavailable");
    this.#child.stdin.write(encoded);
  }
  #accept(value: ObjectValue): void {
    if (this.#terminal) throw new ProviderSetupError("provider output after terminal result");
    switch (value.kind) {
      case "initialized":
        if (this.#interaction || value.protocol !== PROTOCOL || value.id !== "initialize" || !nonempty(value.interaction_id)) throw new ProviderSetupError("invalid provider initialization");
        this.#interaction = value.interaction_id; break;
      case "selected": {
        const selected = decodeProviderSelection(value.selection);
        if (selected.interaction_id !== this.#interaction) throw new ProviderSetupError("foreign provider selection");
        this.#terminal = value; break;
      }
      case "failed": case "cancelled":
        if (value.interaction_id !== this.#interaction || value.joined !== true || value.join_scope !== "provider_interaction") throw new ProviderSetupError("unjoined provider terminal");
        this.#terminal = value; break;
      case "prompt":
        if (value.interaction_id !== this.#interaction || this.#pending || !nonempty(value.prompt_id)) throw new ProviderSetupError("foreign or overlapping provider prompt");
        object(value.prompt); this.#pending = value.prompt_id; break;
      case "answered": case "prompt_closed":
        if (value.prompt_id !== this.#pending) throw new ProviderSetupError("foreign provider prompt closure");
        this.#pending = undefined; break;
      case "notice": case "cancelling":
        if (value.interaction_id !== this.#interaction) throw new ProviderSetupError("foreign provider notice"); break;
      default: throw new ProviderSetupError("unknown provider response");
    }
    if (this.#events.length >= 64) throw new ProviderSetupError("provider observer fell behind");
    this.#events.push(value); this.#wake();
  }
  async nextEvent(timeoutMs = 60_000): Promise<ObjectValue> {
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > MAX_OBSERVATION_MS) throw new ProviderSetupError("observation timeout must be positive and finite, at most 2147483647ms");
    const deadline = Date.now() + timeoutMs;
    while (!this.#events.length) {
      if (this.#failure) throw this.#failure;
      if (this.#closed) throw new ProviderSetupError("provider interaction ended");
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new ProviderSetupError("provider observation pending; owner retained");
      await new Promise<void>(resolve => {
        const wake = () => { clearTimeout(timer); const i = this.#waiters.indexOf(wake); if (i >= 0) this.#waiters.splice(i, 1); resolve(); };
        const timer = setTimeout(wake, remaining); this.#waiters.push(wake);
      });
    }
    return this.#events.shift()!;
  }
  answer(promptId: string, answer: ProviderAnswer): void {
    if (promptId !== this.#pending || this.#cancelled || this.#terminal) throw new ProviderSetupError("provider prompt is no longer current");
    this.#send({ method: "answer", id: `answer-${++this.#counter}`, prompt_id: promptId, answer });
  }
  async close(timeoutMs = 150_000): Promise<ObjectValue | undefined> {
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > MAX_OBSERVATION_MS) throw new ProviderSetupError("observation timeout must be positive and finite, at most 2147483647ms");
    if (!this.#cancelled && !this.#terminal && !this.#closed) {
      this.#cancelled = true;
      try { this.#send({ method: "cancel", id: "close" }); } catch { this.#child.stdin.end(); }
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([this.#joined, new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new ProviderSetupError("native provider cleanup pending; same child retained")), timeoutMs);
      })]);
    } finally { if (timer) clearTimeout(timer); }
    if (this.#failure) throw this.#failure;
    return this.#terminal;
  }
}
