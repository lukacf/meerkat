import type { ReactNode } from "react";

/** One section of the dev-only kit gallery. Any package adds sections by
 * exporting `gallerySections` from a `*.gallery.tsx` file anywhere under
 * packages/console-components/src, and the console app from one anywhere
 * under console/src (console/src/dev/app-gallery.ts collects those): the
 * gallery finds them with Vite's import.meta.glob, so no gallery file needs
 * an edit. Ids are unique across both. */
export interface GallerySection {
  id: string;
  title: string;
  /** Sort key: K1 uses 10-80, the graph kit 90. */
  order?: number;
  description?: string;
  render: () => ReactNode;
}
