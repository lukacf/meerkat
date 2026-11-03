import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { KitGallery, collectGallerySections, mergeGallerySections } from "./kit-gallery";
import type { GallerySection } from "./types";

// The dev-only kit gallery (console v2 spec B23, K1) and the console app's
// sections beside the kit's.

const APP: GallerySection = {
  id: "app-probe",
  title: "App probe",
  order: 85,
  render: () => <div data-testid="app-probe-body">Near the memory limit</div>,
};

const KIT: GallerySection[] = [
  { id: "tokens", title: "Tokens", order: 10, render: () => null },
  { id: "charts", title: "Micro charts", order: 45, render: () => null },
  { id: "graph", title: "Graph", order: 90, render: () => null },
];

describe("kit gallery", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
  });

  it("merges the app's sections into the kit's sorted list, under the same filter", () => {
    expect(mergeGallerySections(KIT, [APP]).map((section) => section.id)).toEqual(["tokens", "charts", "app-probe", "graph"]);
    expect(mergeGallerySections(KIT, [APP], "app-probe").map((section) => section.id)).toEqual(["app-probe"]);
    expect(mergeGallerySections(KIT, [APP], "charts").map((section) => section.id)).toEqual(["charts"]);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      // A repeated id keeps its first section.
      const merged = mergeGallerySections(KIT, [{ ...APP, id: "tokens", title: "Duplicate" }]);
      expect(merged.filter((section) => section.id === "tokens").map((section) => section.title)).toEqual(["Tokens"]);
      expect(warn).toHaveBeenCalledTimes(1);
    } finally {
      warn.mockRestore();
    }
  });

  it("flattens the sections a glob found, in path order", () => {
    const sections = collectGallerySections({
      "b/roster.gallery.tsx": { gallerySections: [APP] },
      "a/memory.gallery.tsx": { gallerySections: [{ ...APP, id: "memory-near-limit" }] },
      "c/plain.gallery.tsx": {},
    });
    expect(sections.map((section) => section.id)).toEqual(["memory-near-limit", "app-probe"]);
  });

  it("renders an app section in the page, alone under its section filter", () => {
    window.history.replaceState(null, "", "/?kit_gallery=1&section=app-probe");
    const view = render(<KitGallery extraSections={[APP]} />);
    const section = view.getByTestId("kit-gallery-section:app-probe");
    expect(section.textContent?.includes("Near the memory limit")).toBe(true);
    expect(view.container.querySelectorAll('[data-testid^="kit-gallery-section:"]').length).toBe(1);
    expect(view.getByRole("navigation", { name: "Sections" }).textContent).toBe("App probe");
  });
});
