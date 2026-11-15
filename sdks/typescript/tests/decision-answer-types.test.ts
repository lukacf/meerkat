// Compile-time contract: one entry of a decision's typed answer to a
// question set is the generated `WireAskUserQuestionAnswer`, with its
// question id and its typed answer, and not an untyped record.
import type {
  WireAskUserAnswer,
  WireAskUserQuestionAnswer,
  WireDecisionAnswer,
  WireDecisionEvidence,
} from "../src/generated/types.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;

type AskUserAnswers = Extract<WireDecisionAnswer, { kind: "ask_user" }>["answers"];

const keepsTheNamedEntry: Equal<AskUserAnswers[number], WireAskUserQuestionAnswer> = true;
const entryNamesItsQuestion: Equal<AskUserAnswers[number]["question_id"], string> = true;
const entryCarriesTheTypedAnswer: Equal<AskUserAnswers[number]["answer"], WireAskUserAnswer> = true;

// A reader narrows the evidence to the answers and reads each entry without
// a cast.
function answersInWords(evidence: WireDecisionEvidence): string[] {
  if (evidence.kind !== "human" || evidence.answer?.kind !== "ask_user") {
    return [];
  }
  const words: string[] = [];
  for (const entry of evidence.answer.answers) {
    const questionId: string = entry.question_id;
    if (entry.answer.kind === "choice") {
      words.push(`${questionId}: ${entry.answer.choice_id}`);
    } else if (entry.answer.kind === "free_text") {
      words.push(`${questionId}: ${entry.answer.text}`);
    } else if (entry.answer.kind === "unanswered") {
      words.push(`${questionId}: not answered`);
    }
    // @ts-expect-error An entry holds its question and its answer, nothing else.
    void entry.text;
  }
  return words;
}

void [keepsTheNamedEntry, entryNamesItsQuestion, entryCarriesTheTypedAnswer, answersInWords];
