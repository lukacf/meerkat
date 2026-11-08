// Who a gateway's console acts as in this browser, shared by its tabs.
//
// A tab that learns its caller (`meta/console/whoami`'s `caller_scope`, an
// opaque digest) writes it under one localStorage record per gateway base
// URL; the other tabs of that gateway's console read the write through a
// `storage` event. A tab whose caller differs reloads its authority at once,
// so a user switch made in one tab blanks the previous caller's views in
// every tab. Signing out writes no caller: every tab leaves its views. The
// record names no person; it only lets tabs compare digests.

import type { ConsoleWhoami } from "./headless";
import { consoleCallerUnknownFailure } from "./network";

export const CONSOLE_CALLER_KEY_PREFIX = "meta-console-caller:v1:";

/** The record that carries a gateway's console caller in this browser. */
export function consoleCallerKey(baseUrl: string): string {
  return `${CONSOLE_CALLER_KEY_PREFIX}${encodeURIComponent(baseUrl)}`;
}

/** A tab's word on who the console acts as: a caller scope, or `null` for
 * a sign-out. `at` makes every announcement a change, so a tab that
 * announces the same caller again still reaches the others. */
export interface ConsoleCallerAnnouncement {
  caller: string | null;
  at: number;
}

export function parseConsoleCallerAnnouncement(raw: string | null): ConsoleCallerAnnouncement | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as { caller?: unknown; at?: unknown };
    const caller = value.caller === null ? null : typeof value.caller === "string" && value.caller.trim() ? value.caller : undefined;
    if (caller === undefined || typeof value.at !== "number" || !Number.isFinite(value.at)) return null;
    return { caller, at: value.at };
  } catch {
    return null;
  }
}

/** Tell this browser's other tabs of the gateway who the console acts as. */
export function announceConsoleCaller(
  storage: Pick<Storage, "setItem"> | null,
  baseUrl: string,
  caller: string | null,
  now = Date.now(),
): void {
  try {
    storage?.setItem(consoleCallerKey(baseUrl), JSON.stringify({ caller, at: now }));
  } catch {
    // Storage full or denied: the other tabs notice at their own next read.
  }
}

/** Follow the other tabs' announcements for this gateway's console. */
export function subscribeConsoleCaller(
  baseUrl: string,
  onAnnouncement: (announcement: ConsoleCallerAnnouncement) => void,
): () => void {
  if (typeof window === "undefined") return () => {};
  const key = consoleCallerKey(baseUrl);
  const listener = (event: StorageEvent) => {
    if (event.key !== key) return;
    const announcement = parseConsoleCallerAnnouncement(event.newValue);
    if (announcement) onAnnouncement(announcement);
  };
  window.addEventListener("storage", listener);
  return () => window.removeEventListener("storage", listener);
}

/** What a failed console request says about the caller: the login ended
 * (a 401, a sign-in redirect, `unauthenticated` or `caller_not_current`),
 * or the gateway now serves another caller (`caller_changed`). A REST
 * refusal carries its kind at the top (`kind`, the body's `error`). */
export function consoleCallerFailure(error: unknown): "sign_in" | "caller_changed" | null {
  if (!error || typeof error !== "object") return null;
  const typed = error as {
    httpStatus?: unknown;
    transportFailure?: unknown;
    kind?: unknown;
    rpcError?: { data?: { kind?: unknown } | null } | null;
    responseRpcError?: { data?: { kind?: unknown } | null } | null;
  };
  const kind = typed.rpcError?.data?.kind ?? typed.responseRpcError?.data?.kind ?? typed.kind;
  if (kind === "caller_changed") return "caller_changed";
  if (typed.httpStatus === 401 || typed.transportFailure === "sign_in_redirect"
    || kind === "unauthenticated" || kind === "caller_not_current") {
    return "sign_in";
  }
  return null;
}

/** The caller one console instance acts as: the first answer of the
 * instance's own `meta/console/whoami` read. A later answer never rebinds
 * it: when someone else signs in to this browser, the gateway refuses this
 * instance's changes (`caller_changed`) and the console loads again for
 * them. Every change names the bound caller; while none is bound, a change
 * is refused before it leaves (`caller_unknown`). */
export interface ConsoleInstanceCaller {
  /** The instance's read: the first call starts it, every later one shares
   * it. `null`: the connection has no whoami (method not found), and the
   * instance's changes name no caller. */
  read(): Promise<ConsoleWhoami | null>;
  /** The caller a change names (`null`: none, the connection has no
   * whoami). Waits for the instance's read; refused (`caller_unknown`) when
   * no read answered. */
  forChange(): Promise<string | null>;
  /** After the instance's read failed: one more read, one at a time. Its
   * answer is bound only through `bind`, once the experience loaded after it
   * keeps the instance's scope. `undefined` while a caller is bound or the
   * instance's first read has not failed. */
  readAgain(): Promise<ConsoleWhoami | null> | undefined;
  /** Bind an answer of `readAgain`; the first bound answer stays. */
  bind(answer: ConsoleWhoami | null): void;
  /** Whether a caller is bound (a connection without whoami included). */
  known(): boolean;
}

export function createConsoleInstanceCaller(
  whoami: (() => Promise<ConsoleWhoami>) | undefined,
): ConsoleInstanceCaller {
  let bound: { answer: ConsoleWhoami | null } | undefined;
  let first: Promise<ConsoleWhoami | null> | undefined;
  let firstFailed = false;
  let again: Promise<ConsoleWhoami | null> | undefined;
  const readOnce = async (): Promise<ConsoleWhoami | null> => {
    if (!whoami) return null;
    try {
      return await whoami();
    } catch (error) {
      // A gateway without whoami has no caller to name.
      if ((error as { rpcError?: { code?: unknown } } | null)?.rpcError?.code === -32601) return null;
      throw error;
    }
  };
  const caller: ConsoleInstanceCaller = {
    read() {
      if (!first) {
        first = readOnce();
        void first.then((answer) => caller.bind(answer), () => { firstFailed = true; });
      }
      return first;
    },
    async forChange() {
      if (!bound) await caller.read().catch(() => undefined);
      if (!bound) throw consoleCallerUnknownFailure();
      return bound.answer?.callerScope ?? null;
    },
    readAgain() {
      if (bound || !firstFailed) return undefined;
      if (!again) {
        const read = readOnce();
        again = read;
        void read.then(() => undefined, () => undefined).finally(() => { if (again === read) again = undefined; });
      }
      return again;
    },
    bind(answer) {
      bound ??= { answer };
    },
    known: () => bound !== undefined,
  };
  return caller;
}
