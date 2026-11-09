import { SlackOperations } from "@rkat/toolkit";
import type { SlackUser, SlackUserResult, SlackUserGetOptions } from "@rkat/toolkit";
import type { SlackUserResult as DirectResult } from "@rkat/toolkit/connectors/slack";

declare const slack: SlackOperations;
const options: SlackUserGetOptions = { includeLocale: false, signal: new AbortController().signal };
const result: Promise<SlackUserResult> = slack.getUser("U123", options);
const direct: Promise<DirectResult> = result;
void direct.then((value) => { const user: SlackUser = value.user; const id: string = user.id; return id; });
// @ts-expect-error IDs are exact strings, not numbers.
void slack.getUser(123);
// @ts-expect-error Locale is a boolean.
void slack.getUser("U123", { includeLocale: "true" });
