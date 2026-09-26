import type { RunInput, RunStartedEvent } from "../src/index.js";
import type { RunInput as GeneratedRunInput } from "../src/generated/event_types.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
const usesGeneratedInput: Equal<RunInput, GeneratedRunInput> = true;
const usesInputOnEvent: Equal<RunStartedEvent["input"], GeneratedRunInput> = true;

function contentOrContinuation(event: RunStartedEvent): string {
  const input: RunInput = event.input;
  if (input.kind === "pending_tool_results") {
    // @ts-expect-error Pending tool results are not a content-bearing prompt.
    void input.content;
    return "pending tools";
  }
  return typeof input.content === "string" ? input.content : input.content[0].type;
}

// @ts-expect-error RunStartedEvent no longer exposes the obsolete prompt field.
type ObsoletePrompt = RunStartedEvent["prompt"];
void [usesGeneratedInput, usesInputOnEvent, contentOrContinuation];
