const failureMessages = {
  capture: "The agent's history could not be captured.",
  generation: "The context summary could not be generated.",
  timed_out: "Context preparation timed out.",
  input_too_large: "The agent's history exceeds the context limit.",
  output_too_large: "The context summary exceeds the size limit.",
  empty: "The context summary was empty.",
  stale_snapshot: "The agent's history changed before context could be supplied.",
  unsupported: "Initial context is not supported by this voice configuration.",
  source_read: "The agent's history could not be read.",
  producer_panicked: "The context summary service failed.",
  delivery_rejected: "The voice provider rejected the initial context.",
  delivery_ambiguous: "The voice provider did not confirm the initial context.",
  cancelled: "Initial context preparation was cancelled.",
  authority_rejected: "The session authority refused the initial context; no fallback was attempted.",
} as const;

export type VoiceContextFailure = keyof typeof failureMessages;
export type VoiceContextPreparation =
  | { readonly phase: "not_requested" }
  | { readonly phase: "preparing"; readonly stage: "capturing" | "generating" | "delivering" }
  | { readonly phase: "provider_acknowledged" }
  | { readonly phase: "failed"; readonly reason: VoiceContextFailure };

interface VoiceContextScope {
  readonly identity: string;
  readonly requestId: string;
  readonly channelId: string;
}

function record(raw: unknown): Record<string, unknown> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Invalid voice context status.");
  }
  return raw as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, keys: string[]) {
  if (Object.keys(value).length !== keys.length || keys.some((key) => !Object.hasOwn(value, key))) {
    throw new Error("Invalid voice context status.");
  }
}

function isFailure(value: unknown): value is VoiceContextFailure {
  return typeof value === "string" && Object.hasOwn(failureMessages, value);
}

export function parseVoiceContextStatus(raw: unknown, scope: VoiceContextScope): VoiceContextPreparation {
  const result = record(raw);
  exactKeys(result, ["identity", "request_id", "channel_id", "context_preparation"]);
  if (
    result.identity !== scope.identity ||
    result.request_id !== scope.requestId ||
    result.channel_id !== scope.channelId
  ) throw new Error("Voice context status does not match the active call.");
  const preparation = record(result.context_preparation);
  switch (preparation.phase) {
    case "not_requested":
    case "provider_acknowledged":
      exactKeys(preparation, ["phase"]);
      return { phase: preparation.phase };
    case "preparing":
      exactKeys(preparation, ["phase", "stage"]);
      if (preparation.stage === "capturing" || preparation.stage === "generating" || preparation.stage === "delivering") {
        return { phase: "preparing", stage: preparation.stage };
      }
      break;
    case "failed":
      exactKeys(preparation, ["phase", "reason"]);
      if (isFailure(preparation.reason)) return { phase: "failed", reason: preparation.reason };
      break;
  }
  throw new Error("Invalid voice context status.");
}

export function voiceContextFailureMessage(reason: VoiceContextFailure): string {
  return `${failureMessages[reason]} Voice remains connected. End voice and start again to retry.`;
}
