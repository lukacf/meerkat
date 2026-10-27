import type { ConsoleFrame } from "./runtime-types";
import { isRealtimeHistoryMessage, realtimeMessageOrigin } from "./realtime-message-identity";

const UUID_FORM = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** An input's typed owner survives send/history source replacement. */
export function userMessageRenderKey(frame: ConsoleFrame): string | undefined {
  if (frame.event !== "user_input" && frame.event !== "interaction_started" && frame.event !== "run_started") return undefined;
  if (isRealtimeHistoryMessage(frame)) {
    const origin = realtimeMessageOrigin(frame);
    // An explicit invalid realtime carrier cannot authorize an ordinary join.
    return origin ? `user-realtime:${JSON.stringify([frame.runtimeKey ?? null, frame.identity ?? null,
      origin.sessionId, origin.channelId, origin.canonicalRowSequence])}` : undefined;
  }
  const interaction = frame.interactionId;
  if (typeof frame.sessionId !== "string" || !frame.sessionId.trim()
    || typeof interaction !== "string" || interaction.length !== 36 || !UUID_FORM.test(interaction)) return undefined;
  return `user:${JSON.stringify([frame.runtimeKey ?? null, frame.identity ?? null,
    frame.sessionId, interaction.toLowerCase()])}`;
}
