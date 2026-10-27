import type { GmailProviderReason as RootReason, GmailFailureKind } from "@rkat/toolkit";
import { GmailError } from "@rkat/toolkit/connectors/gmail";
import type { GmailProviderReason } from "@rkat/toolkit/connectors/gmail";

const reason: RootReason = "dailyLimitExceeded";
const same: GmailProviderReason = reason;
const kind: GmailFailureKind = "quota_exceeded";
const error = new GmailError(kind, "unknown", 403, null, same);
const observed: GmailProviderReason | null = error.providerReason;
void observed;
