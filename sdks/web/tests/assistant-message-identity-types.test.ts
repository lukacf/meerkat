import type {
  AssistantMessageId,
  ReasoningDeltaEvent,
  RetryingEvent,
  RunCompletedEvent,
  TextCompleteEvent,
  TextDeltaEvent,
  TurnCompletedEvent,
  TurnStartedEvent,
} from "../src/index.js";

const id: AssistantMessageId = "0190f5c2-4a1e-7c3d-8e2f-00000000a001";
type MessageScoped =
  | TurnStartedEvent
  | TextDeltaEvent
  | TextCompleteEvent
  | ReasoningDeltaEvent
  | TurnCompletedEvent
  | RetryingEvent
  | RunCompletedEvent;
function joinKey(event: MessageScoped): AssistantMessageId | null | undefined {
  return event.assistant_message_id;
}
// Pre-0.8.45 payloads omit the id entirely.
const legacy: TurnStartedEvent = { type: "turn_started", turn_number: 0 };
const stamped: TextDeltaEvent = {
  type: "text_delta",
  delta: "he",
  assistant_message_id: id,
};
// @ts-expect-error The id is an opaque string, never a number.
const invalid: TextDeltaEvent = { type: "text_delta", delta: "he", assistant_message_id: 7 };
void [joinKey, legacy, stamped, invalid];
