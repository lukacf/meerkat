import { createRoot } from "react-dom/client";

import { KitGallery } from "./kit-gallery";
import type { GallerySection } from "./types";
// The kit's stylesheets, as the console app loads them (its host sheet
// follows from browser.tsx, after these, as in the app).
import "../../styles/index.css";
import "./gallery.css";

/**
 * Mounts the kit gallery (console v2 spec B23, K1) in place of the console,
 * with the console app's own sections (`extraSections`) beside the kit's.
 * Dev only: console/src/browser.tsx loads this module behind a branch the
 * production build drops, and it refuses to mount outside Vite's dev mode.
 */
export function mountKitGallery(root: HTMLElement | null, extraSections?: ReadonlyArray<GallerySection>): void {
  // @ts-ignore Vite defines import.meta.env; nothing else loads this file.
  const dev = Boolean(import.meta.env?.DEV);
  if (!dev || !root) return;
  document.title = "Console kit gallery";
  createRoot(root).render(<KitGallery extraSections={extraSections} />);
}
