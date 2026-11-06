// Who a gateway's console acts as in this browser, shared by its tabs.
//
// A tab that learns its caller (`meta/console/whoami`'s `caller_scope`, an
// opaque digest) writes it under one localStorage record per gateway base
// URL; the other tabs of that gateway's console read the write through a
// `storage` event. A tab whose caller differs reloads its authority at once,
// so a user switch made in one tab blanks the previous caller's views in
// every tab. Signing out writes no caller: every tab leaves its views. The
// record names no person; it only lets tabs compare digests.

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
 * or the gateway now serves another caller (`caller_changed`). */
export function consoleCallerFailure(error: unknown): "sign_in" | "caller_changed" | null {
  if (!error || typeof error !== "object") return null;
  const typed = error as {
    httpStatus?: unknown;
    transportFailure?: unknown;
    rpcError?: { data?: { kind?: unknown } | null } | null;
    responseRpcError?: { data?: { kind?: unknown } | null } | null;
  };
  const kind = typed.rpcError?.data?.kind ?? typed.responseRpcError?.data?.kind;
  if (kind === "caller_changed") return "caller_changed";
  if (typed.httpStatus === 401 || typed.transportFailure === "sign_in_redirect"
    || kind === "unauthenticated" || kind === "caller_not_current") {
    return "sign_in";
  }
  return null;
}
