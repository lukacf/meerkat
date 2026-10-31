// Who sent a user turn, in words people use. A console send carries a typed
// caller kind (`origin_kind`: operator, operator_probe, connector,
// scheduler, policy, flow, system) and a free-form `origin`; neither names
// the operator, so "You" means a send this console made in this browser
// session, and any other operator send reads "An operator". Pure module:
// panels whose test bundles cannot resolve React use it too.

export type TurnSender =
  | "you"
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

/** Forget every remembered send. After an account switch in the same tab
 * the earlier sends belong to the previous account, not to "You". */
export function forgetOwnSends(): void {
  ownSendKeys.clear();
}

export function isOwnSend(key: unknown): boolean {
  return typeof key === "string" && ownSendKeys.has(key.trim());
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
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
    return isOwnSend(data?.idempotency_key) ? "you" : "operator";
  }
  return "unknown";
}

/** The words around the agent's name: "You asked {agent}", "Scheduled run
 * for {agent}", "A connector sent {agent} a message". */
export function turnSenderWords(sender: TurnSender): { before: string; after: string } {
  switch (sender) {
    case "you":
      return { before: "You asked ", after: "" };
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
  const words = turnSenderWords(turnSender(data));
  return `${words.before}${agentName}${words.after}`;
}
