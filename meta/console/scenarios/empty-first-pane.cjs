"use strict";

// A first load with no saved layout opens Today, and Today's backfill fills
// the most active agents' logs. A scenario about the chat's own first page
// and paging starts instead from a saved layout whose one pane is empty: the
// experience gets a storage scope so the layout persists, and the layout is
// in place before the console loads.
async function startFromEmptyFirstPane(page, storageScope) {
  await page.route("**/console/experience", async (route) => {
    const response = await route.fetch();
    const value = await response.json();
    await route.fulfill({ response, json: { ...value, storage_scope: storageScope } });
  });
  await page.addInitScript((scope) => {
    const key = `meta-console-dock-state:${JSON.stringify([window.location.origin, scope])}`;
    window.localStorage.setItem(key, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: null }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
  }, storageScope);
}

module.exports = { startFromEmptyFirstPane };
