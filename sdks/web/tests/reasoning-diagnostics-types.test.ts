import type { ReasoningEffortRefusal } from "../src/index.js";
import {
  REASONING_CAUSE_NEW_SIGN_IN_REQUIRED,
  REASONING_CAUSE_TOKEN_EXCHANGE_UNAVAILABLE,
} from "../src/index.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
const openCause: Equal<ReasoningEffortRefusal["cause"], string | undefined> = true;
const retainedTime: Equal<ReasoningEffortRefusal["notBefore"], string | undefined> = true;
const signInWire: "new_sign_in_required" = REASONING_CAUSE_NEW_SIGN_IN_REQUIRED;
const outageWire: "token_exchange_unavailable" = REASONING_CAUSE_TOKEN_EXCHANGE_UNAVAILABLE;

declare const refusal: ReasoningEffortRefusal;
const futureCause: ReasoningEffortRefusal = { ...refusal, cause: "future_cause" };
const opaqueTime: ReasoningEffortRefusal = { ...refusal, notBefore: "not-a-timestamp" };
// @ts-expect-error advisory causes are open strings, not arbitrary JSON
const invalidCause: ReasoningEffortRefusal = { ...refusal, cause: 7 };
// @ts-expect-error timestamps are retained strings at this SDK boundary
const invalidTime: ReasoningEffortRefusal = { ...refusal, notBefore: {} };
void [openCause, retainedTime, signInWire, outageWire, futureCause, opaqueTime, invalidCause, invalidTime];
