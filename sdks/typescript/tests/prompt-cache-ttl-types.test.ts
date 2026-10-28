import type {
  ForkAuthoredCacheBreakpoint,
  ForkCacheTtl,
  ForkCacheTtlUnknown,
} from "../src/index.js";
import type { OpenAiPromptCacheTtl } from "../src/generated/event_types.js";
import type { WireOpenAiPromptCacheTtl } from "../src/generated/types.js";

// The OpenAI prompt-cache TTL vocabularies carry the 12-hour lifetime, and
// authored cache evidence can name it or, tolerantly, a lifetime this SDK
// does not know. Checked with `tsc --noEmit`; unrun until the candidate's
// validation.
type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;

const wireTtls: Equal<WireOpenAiPromptCacheTtl, "30m" | "12h"> = true;
const eventTtls: Equal<OpenAiPromptCacheTtl, "30m" | "12h"> = true;
const twelveHours: WireOpenAiPromptCacheTtl = "12h";
// @ts-expect-error `24h` is a prompt_cache_retention value, not a TTL.
const retentionValue: WireOpenAiPromptCacheTtl = "24h";
const evidence: ForkCacheTtl = "twelve_hours";
const evidenceTtl: Equal<
  ForkAuthoredCacheBreakpoint["ttl"],
  ForkCacheTtl | ForkCacheTtlUnknown
> = true;
const unknownEvidence: ForkAuthoredCacheBreakpoint["ttl"] = {
  unknown: "forty_eight_hours",
};

void [
  wireTtls,
  eventTtls,
  twelveHours,
  retentionValue,
  evidence,
  evidenceTtl,
  unknownEvidence,
];
