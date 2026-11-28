// The three vocabularies of `mob/standing` may grow: a later server can send
// a word this SDK does not list. The generated types say so: each alias is
// its known words or any other string, so an answer that carries such a
// word is a `MobStandingResult` and code that handles it type-checks.
import type {
  MobStandingResult,
  WireMobStanding,
  WireMobStandingReason,
  WireMobUnknownCause,
} from "../src/generated/types.js";

const standing: WireMobStanding = "a_word_from_a_later_server";
const reason: WireMobStandingReason = "a_reason_from_a_later_server";
const cause: WireMobUnknownCause = "a_cause_from_a_later_server";

const later: MobStandingResult = {
  mob_id: "quiet",
  standing: "a_word_from_a_later_server",
  reason: "a_reason_from_a_later_server",
  causes: ["a_cause_from_a_later_server"],
};

// The known words are still the type's own: they stay assignable.
const known: WireMobStanding = "unknown";
const knownReason: WireMobStandingReason = "completed";
const knownCause: WireMobUnknownCause = "observation_timed_out";

/** A reader's handling: a word it does not know is treated as unknown. */
function isQuietOrUnknown(answer: MobStandingResult): boolean {
  switch (answer.standing) {
    case "busy":
      return false;
    case "unknown":
      return true;
    default:
      return answer.standing !== "finished";
  }
}

// @ts-expect-error A standing is a word, not a number.
const notAWord: WireMobStanding = 7;
void [standing, reason, cause, later, known, knownReason, knownCause, isQuietOrUnknown, notAWord];
