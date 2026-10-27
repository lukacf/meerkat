import { SlackOperations } from "@rkat/toolkit";
import type { SlackAuthObservation, SlackConversation, SlackConversationGetOptions,
  SlackConversationListOptions, SlackConversationResult, SlackConversationsPage,
  SlackConversationType, SlackOperationOptions } from "@rkat/toolkit";
import type { SlackAuthObservation as SubpathAuthObservation,
  SlackConversationListOptions as SubpathListOptions } from "@rkat/toolkit/connectors/slack";

declare const slack: SlackOperations;
declare const signal: AbortSignal;
const types: readonly SlackConversationType[] = ["public_channel", "im"];
const operation: SlackOperationOptions = { signal };
const listing: SlackConversationListOptions = { ...operation, types, teamId: "T1", limit: 7 };
const sameListing: SubpathListOptions = listing;
const detail: SlackConversationGetOptions = { signal, includeLocale: false, includeNumMembers: true };
const auth: Promise<SlackAuthObservation> = slack.authTest(operation);
const subpathAuth: Promise<SubpathAuthObservation> = auth;
const page: Promise<SlackConversationsPage> = slack.listConversationsPage(sameListing);
const info: Promise<SlackConversationResult> = slack.getConversation("C1", detail);
void subpathAuth;
void page.then((value) => { const row: SlackConversation | undefined = value.channels[0]; return row?.id; });
void info.then((value) => value.channel.id);
// @ts-expect-error Unknown conversation types are not advertised.
slack.listConversationsPage({ types: ["all"] });
// @ts-expect-error Boolean flags remain actual booleans.
slack.getConversation("C1", { includeLocale: "true" });
// @ts-expect-error No token or account selection parameter exists.
slack.authTest({ token: "synthetic-unused" });
// @ts-expect-error Exact IDs are strings, not numeric guesses.
slack.getConversation(12);
