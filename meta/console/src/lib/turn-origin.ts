// Who sent a user turn, in words people use. A console send carries a typed
// caller kind (`origin_kind`: operator, operator_probe, connector,
// scheduler, policy, flow, system), a free-form `origin`, and the sender the
// gateway recorded from the caller it authenticated (`sender`: kind,
// display name and the caller scope `meta/console/whoami` reports). "You"
// means a send whose sender is this console's caller, in any tab; another
// caller's send reads with its sender's display name. Every anonymous caller
// of an Open console shares one caller scope, so an anonymous sender is
// "You" only for the sends this tab made. A frame that names no
// sender (an older gateway) falls back to the sends this console made in
// this browser session. Pure module: panels whose test bundles cannot
// resolve React use it too.

export type TurnSender =
  | "you"
  | "named"
  | "operator"
  | "scheduler"
  | "connector"
  | "probe"
  | "flow"
  | "policy"
  | "system"
  | "unknown";

const OWN_SEND_LIMIT = 500;
const ownSendKeys = new Set<string>();
// The caller scope `meta/console/whoami` reports for this console's caller.
let ownCallerScope: string | undefined;

/** Remember the idempotency key of a send this console made. */
export function rememberOwnSend(key: string | null | undefined): void {
  const trimmed = typeof key === "string" ? key.trim() : "";
  if (!trimmed) return;
  ownSendKeys.delete(trimmed);
  ownSendKeys.add(trimmed);
  if (ownSendKeys.size > OWN_SEND_LIMIT) {
    const oldest = ownSendKeys.values().next().value;
    if (oldest !== undefined) ownSendKeys.delete(oldest);
  }
}

/** Forget every remembered send and the caller scope. After an account
 * switch in the same tab the earlier sends belong to the previous account,
 * not to "You". */
export function forgetOwnSends(): void {
  ownSendKeys.clear();
  ownCallerScope = undefined;
}

/** Remember the caller scope `meta/console/whoami` reports for this
 * console's caller: a frame whose sender names it is the caller's own, in
 * any tab. */
export function rememberOwnCallerScope(scope: string | null | undefined): void {
  const trimmed = typeof scope === "string" ? scope.trim() : "";
  ownCallerScope = trimmed || undefined;
}

export function isOwnSend(key: unknown): boolean {
  return typeof key === "string" && ownSendKeys.has(key.trim());
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function senderOf(data: Record<string, unknown> | null | undefined): Record<string, unknown> | undefined {
  const sender = data?.sender;
  return sender && typeof sender === "object" && !Array.isArray(sender)
    ? sender as Record<string, unknown>
    : undefined;
}

/** The display name of the sender a `user_input` frame records, if any. */
export function turnSenderName(data: Record<string, unknown> | null | undefined): string {
  return text(senderOf(data)?.display_name);
}

/** The sender of a `user_input` frame from its payload. */
export function turnSender(data: Record<string, unknown> | null | undefined): TurnSender {
  const kind = text(data?.origin_kind);
  const origin = text(data?.origin);
  if (kind === "scheduler" || origin === "console:schedule") return "scheduler";
  if (kind === "connector") return "connector";
  if (kind === "operator_probe") return "probe";
  if (kind === "flow") return "flow";
  if (kind === "policy") return "policy";
  if (kind === "system") return "system";
  if (kind === "operator" || origin.startsWith("console:")) {
    const sender = senderOf(data);
    const senderScope = text(sender?.caller_scope);
    // Every anonymous caller of an Open console shares one caller scope, so
    // the scope cannot tell this tab's caller from another anonymous one:
    // an anonymous message is "You" only when this tab sent it.
    if (text(sender?.kind) === "anonymous") {
      if (isOwnSend(data?.idempotency_key)) return "you";
      return text(sender?.display_name) ? "named" : "operator";
    }
    if (senderScope) {
      if (ownCallerScope) {
        if (senderScope === ownCallerScope) return "you";
      } else if (isOwnSend(data?.idempotency_key)) {
        // whoami has not answered yet: this tab's own send is still "You".
        return "you";
      }
      return text(sender?.display_name) ? "named" : "operator";
    }
    return isOwnSend(data?.idempotency_key) ? "you" : "operator";
  }
  return "unknown";
}

/** The words around the agent's name: "You asked {agent}", "Scheduled run
 * for {agent}", "A connector sent {agent} a message". `name` is the
 * sender's display name, for a `named` sender. */
export function turnSenderWords(sender: TurnSender, name?: string): { before: string; after: string } {
  switch (sender) {
    case "you":
      return { before: "You asked ", after: "" };
    case "named":
      return name ? { before: `${name} asked `, after: "" } : { before: "An operator asked ", after: "" };
    case "operator":
      return { before: "An operator asked ", after: "" };
    case "scheduler":
      return { before: "Scheduled run for ", after: "" };
    case "connector":
      return { before: "A connector sent ", after: " a message" };
    case "probe":
      return { before: "An operator check asked ", after: "" };
    case "flow":
      return { before: "A flow step asked ", after: "" };
    case "policy":
      return { before: "A policy started a turn for ", after: "" };
    case "system":
      return { before: "The system started a turn for ", after: "" };
    default:
      return { before: "Message to ", after: "" };
  }
}

/** The whole line for a known agent name. */
export function turnSenderLine(data: Record<string, unknown> | null | undefined, agentName: string): string {
  const words = turnSenderWords(turnSender(data), turnSenderName(data));
  return `${words.before}${agentName}${words.after}`;
}
