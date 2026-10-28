// Compile-time contract: the hand-written comms/send types follow the
// generated wire contracts.
import type {
  CommsCommand,
  CommsSendErrorData,
  CommsSendReceipt,
  PeerDeliveryOutcome,
  PeerIngressTerminalOutcomeKind,
} from "../src/index.js";

type LifecycleKind = Extract<CommsCommand, { kind: "peer_lifecycle" }>["lifecycle_kind"];

const dismiss: LifecycleKind = "mob.dismiss";
// @ts-expect-error lifecycle kinds are the generated closed vocabulary.
const unknownLifecycle: LifecycleKind = "mob.unknown";

void [dismiss, unknownLifecycle];

// The comms/send receipt is the snake_case wire result, with no camelCase copy.
declare const receipt: CommsSendReceipt;
if (receipt.kind === "peer_request_sent") {
  const requestId: string = receipt.request_id;
  void requestId;
}
// @ts-expect-error the SDK never populated camelCase receipt fields.
void receipt.requestId;

// Every peer receipt's delivery is the named, fully typed PeerDeliveryOutcome.
type Delivery = Extract<CommsSendReceipt, { kind: "peer_message_sent" }>["delivery"];
const durable: Delivery = { durably_resolved: { outcome: "deduplicated" } };
const named: PeerDeliveryOutcome = durable;
const outcome: PeerIngressTerminalOutcomeKind = "rejected";
// @ts-expect-error terminal ingress outcomes are the generated closed vocabulary.
const unknownOutcome: Delivery = { durably_resolved: { outcome: "dropped" } };
void [named, outcome, unknownOutcome];

// comms/send failures carry the generated CommsSendErrorData in `details`.
const unreachable: CommsSendErrorData = {
  code: "peer_unreachable",
  peer: "agent-a",
  reason: "offline_or_no_ack",
  message: "peer is unreachable",
};
// An ambiguous delivery is its own code with the envelope id, not send_failed.
const ambiguous: CommsSendErrorData = {
  code: "send_ambiguous",
  envelope_id: "00000000-0000-4000-8000-000000000001",
  message: "delivery outcome is unknown",
  retry_safe: false,
  required_action: "reconcile",
};
// A seated member whose wake was refused is its own code with the closed,
// generated refusal vocabulary, never peer_not_found_or_not_trusted.
const wakeRefused: CommsSendErrorData = {
  code: "peer_wake_refused",
  peer: "agent-a",
  envelope_id: "00000000-0000-4000-8000-000000000001",
  refusal: { kind: "wake_timed_out" },
  retryable: true,
  message: "peer was not woken",
};
const wakeRefusedUnknownKind: CommsSendErrorData = {
  code: "peer_wake_refused",
  peer: "agent-a",
  envelope_id: "00000000-0000-4000-8000-000000000001",
  // @ts-expect-error wake refusals are the generated closed vocabulary.
  refusal: { kind: "not_a_refusal" },
  retryable: true,
  message: "peer was not woken",
};
// @ts-expect-error error codes are the generated closed vocabulary.
const unknownCode: CommsSendErrorData = { code: "not_a_code", message: "m" };
void [unreachable, ambiguous, wakeRefused, wakeRefusedUnknownKind, unknownCode];
