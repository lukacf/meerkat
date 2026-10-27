import { mountToolkitView } from '@rkat/toolkit/ui';

// The host supplies its authenticated, audience-filtered snapshot and existing
// dispatcher. Sign-in URLs, callbacks and the protected presenter stay in the
// host. This example adds no auth endpoint, browser launcher or lifecycle.
export function mountConnectorSignIn({ container, snapshot, dispatch }) {
  const view = mountToolkitView(container, { scope: snapshot.scope, dispatch });
  view.update(snapshot);
  // Keep host callback custody when the panel is disposed or remounted.
  return view;
}
