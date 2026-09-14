import type { CustomModelConfig } from "../src/generated/types.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
type Assert<T extends true> = T;
type InteractionKindIsClosed = Assert<Equal<
  Exclude<CustomModelConfig["interaction_kind"], null | undefined>,
  "text" | "turn_based_realtime" | "continuous_live"
>>;

const voice: CustomModelConfig = {
  provider: "openai",
  interaction_kind: "continuous_live",
};
const text: CustomModelConfig = { provider: "openai" };
const invalid: CustomModelConfig = {
  provider: "openai",
  // @ts-expect-error Model interaction is a closed contract, not arbitrary text.
  interaction_kind: "function_bridge",
};
void [voice, text, invalid];
