import type {
  WireConnectorAccountSelection,
  WireConnectorAuthTarget,
} from "../src/generated/types.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
const keepsTheNamedAlias: Equal<
  WireConnectorAuthTarget["account_selection"],
  WireConnectorAccountSelection
> = true;

const slot = { realm_id: "tenant-a", slot_id: "drive-work" };
const base = {
  slot,
  issuer: "https://accounts.example.com",
  client: "client",
  resource: "https://api.example.com",
  scopes: ["openid"],
  strategy_id: "oidc-userinfo-v1",
};

const discover: WireConnectorAuthTarget = {
  ...base,
  account_selection: { mode: "discover" },
};
const known: WireConnectorAuthTarget = {
  ...base,
  account_selection: { mode: "known", account: "subject-7" },
};
const knownWithoutAccount: WireConnectorAuthTarget = {
  ...base,
  // @ts-expect-error A known selection names the account.
  account_selection: { mode: "known" },
};
const invalidMode: WireConnectorAuthTarget = {
  ...base,
  // @ts-expect-error Only known and discover are selection modes.
  account_selection: { mode: "any" },
};

function selectedAccount(selection: WireConnectorAccountSelection): string | null {
  if (selection.mode === "known") {
    return selection.account;
  }
  // @ts-expect-error A discover selection carries no account.
  void selection.account;
  return null;
}

void [keepsTheNamedAlias, discover, known, knownWithoutAccount, invalidMode, selectedAccount];
