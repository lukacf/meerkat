import type { Session } from "@rkat/sdk";
import { NativeSetupConversation } from "@rkat/toolkit/setup-session";
import type { SetupConversationPort } from "@rkat/toolkit/setup-session";
import { mountSetupConversation } from "@rkat/toolkit/ui";
declare const session: Session;
declare const scopeInvalidated: AbortSignal;
const conversation = new NativeSetupConversation({ session, scope: "host-selected",
  invalidated: scopeInvalidated, revalidate: () => true });
const safePort: SetupConversationPort = conversation;
declare const element: HTMLElement;
mountSetupConversation(element, { scope: "host-selected", conversation: safePort });
new NativeSetupConversation({
  // @ts-expect-error a user-supplied session ID is not an SDK Session
  session: "arbitrary-session-id", scope: "host-selected", invalidated: scopeInvalidated, revalidate: () => true,
});
