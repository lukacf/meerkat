import { MeerkatClient } from "../src/index.js";
import type { RequesterPrincipal, ToolCallRequester } from "../src/index.js";

// A callback handler may take the run's requester as an optional third
// argument; one- and two-argument handlers stay valid. Checked with
// `tsc --noEmit`.
const client = new MeerkatClient();
client.registerTool("plain", "Plain", { type: "object" }, async (args) => String(args.q));
client.registerTool(
  "person_scoped",
  "Person scoped",
  { type: "object" },
  async (_args, _dedupeKey, requester?: ToolCallRequester) =>
    requester !== undefined && !requester.carried ? requester.principal.id : "refused",
);

const principal: RequesterPrincipal = {
  kind: "human",
  id: "alice",
  trust_domain_id: "oidc-issuer:https://issuer.example",
};
const requester: ToolCallRequester = { principal, carried: false };
// @ts-expect-error The requester is read-only.
requester.carried = true;
const parsed: ToolCallRequester | undefined = MeerkatClient.parseToolCallRequester(requester);

void [principal, requester, parsed];
