import { SlackOperations } from "@rkat/toolkit/connectors/slack";
const context = { baseUrl: "https://slack.invalid/", fetch: async (_request: Request) => new Response() };
const ids: readonly string[] = ["C1"];
new SlackOperations(context, { allowedChannelIds: ids });
new SlackOperations(context, { allowedChannelIds: [] });
new SlackOperations(context);
// @ts-expect-error Explicit null is not an unrestricted selection.
new SlackOperations(context, { allowedChannelIds: null });
// @ts-expect-error Channel IDs are exact strings.
new SlackOperations(context, { allowedChannelIds: [1] });
// @ts-expect-error The ceiling is not a per-operation argument.
new SlackOperations(context).historyPage("C1", { allowedChannelIds: ["C1"] });
