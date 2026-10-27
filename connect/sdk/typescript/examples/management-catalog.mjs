import { mountToolkitView } from '@rkat/toolkit/ui';

// A host already owns these DOM surfaces, the authenticated scope, projection
// and OwnerControl dispatcher. This example only reveals the supplied manual
// panel. That panel must enforce its own admission before any subsequent setup.
// No authorization, installation or attempt is started by this navigation.
export function mountCatalogManagement({ container, manualPanel, manualHeading, snapshot, dispatch }) {
  if (!manualPanel.contains(manualHeading) || manualPanel.ownerDocument !== container.ownerDocument) {
    throw new TypeError('Manual navigation requires the host-owned panel and heading');
  }
  manualHeading.tabIndex = -1;
  const view = mountToolkitView(container, {
    scope: snapshot.scope,
    dispatch,
    onManualConfiguration() {
      manualPanel.hidden = false;
      manualHeading.focus();
    },
  });
  view.update(snapshot);
  return view;
}
