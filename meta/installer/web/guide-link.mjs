const guideIdentity = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const validIdentity = value => typeof value === "string" && value.length === 36 && guideIdentity.test(value);

// A link selects only a product guide in the current authenticated workspace.
// It supplies neither native identity nor an action to submit.
export function readGuideLink(search = "") {
  const values = new URLSearchParams(search).getAll("guide");
  if (!values.length) return null;
  if (values.length !== 1 || !validIdentity(values[0]))
    return { id: null, error: "This setup link is invalid. Your saved setups have not changed." };
  return { id: values[0] };
}

export function replaceGuideLink(id, browser = globalThis.window) {
  if (!browser?.location || !browser?.history) return;
  const url = new URL(browser.location.href);
  if (!url.searchParams.has("guide")) return;
  if (validIdentity(id)) url.searchParams.set("guide", id);
  else url.searchParams.delete("guide");
  // Address-bar failure must not discard an otherwise successful selection.
  try { browser.history.replaceState(browser.history.state, "", url); } catch {}
}
