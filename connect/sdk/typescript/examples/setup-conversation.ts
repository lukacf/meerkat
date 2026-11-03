import type { Session } from "@rkat/sdk";
import { NativeSetupConversation } from "@rkat/toolkit/setup-session";
import { mountSetupConversation, mountToolkitView } from "@rkat/toolkit/ui";
import type { ControlRequest, ToolkitSnapshot } from "@rkat/toolkit/ui";

/** Called inside an embedding application after its authenticated host has
 * created/selected the setup Session through the released SDK. For example,
 * `await hostClient.createSession(fullUserRequest, hostSelectedSessionOptions)`
 * returns the real Session used here. This example does not open a runtime,
 * accept a browser-supplied session ID, or decide session tools/permissions.
 *
 * The Node-oriented Session remains privileged host state. A browser-only
 * console needs the application's admitted bridge and optional console slot;
 * those are not implemented by this same-process embedding example.
 */
export function mountAdmittedSetup(options: {
  conversationContainer: HTMLElement;
  managementContainer: HTMLElement;
  session: Session;
  scope: string;
  invalidated: AbortSignal;
  revalidate: () => boolean | Promise<boolean>;
  dispatch: (request: ControlRequest) => Promise<void>;
  initialProjection: ToolkitSnapshot;
}) {
  const binding = new NativeSetupConversation({ session: options.session,
    scope: options.scope, invalidated: options.invalidated, revalidate: options.revalidate });
  const chat = mountSetupConversation(options.conversationContainer, {
    scope: options.scope, conversation: binding,
  });
  const management = mountToolkitView(options.managementContainer, {
    scope: options.scope, dispatch: options.dispatch,
  });
  management.update(options.initialProjection);
  // Service readiness and frontend configuration are still separate native/host
  // observations. A chat result never manufactures one of these projections.
  const retireManagement = () => {
    management.dispose();
    options.managementContainer.replaceChildren();
  };
  options.invalidated.addEventListener("abort", retireManagement, { once: true });
  if (options.invalidated.aborted) retireManagement();
  return {
    update: (projection: ToolkitSnapshot) => management.update(projection),
    dispose() {
      options.invalidated.removeEventListener("abort", retireManagement);
      chat.dispose(); management.dispose();
      // No native interrupt, archive, stop or credential operation.
    },
  };
}
