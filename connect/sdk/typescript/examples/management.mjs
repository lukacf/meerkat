import { mountToolkitView } from "../dist/ui/index.js";
import { fixture } from "./management-fixture.mjs";
const panel = mountToolkitView(document.querySelector("#toolkit"), {
  scope: "fixture-viewer",
  async dispatch() {
    // Deliberately no navigation or fake success transition in this preview.
    // Only actual owner projections may change account, role or setup status.
  },
});
panel.update(fixture());
