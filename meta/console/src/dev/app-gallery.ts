import type { GallerySection } from "@console-components";

// Dev only (console v2 spec B23, K1). The console app's own kit gallery
// sections: every `gallerySections` export of a `*.gallery.tsx` file under
// console/src. App panels cannot add them under packages/console-components
// (the package never imports console/src), so console/src/browser.tsx loads
// this module next to the gallery, behind the branch the production build
// drops, and passes these sections to mountKitGallery.

/** A module found by the gallery glob. */
export interface AppGalleryModule {
  gallerySections?: ReadonlyArray<GallerySection>;
}

/** The sections of the given modules, flattened in path order. */
export function flattenGallerySections(modules: Record<string, AppGalleryModule>): GallerySection[] {
  return Object.keys(modules)
    .sort()
    .flatMap((key) => modules[key].gallerySections ?? []);
}

/** Every app gallery section under console/src. */
export function appGallerySections(): GallerySection[] {
  // @ts-ignore import.meta.glob is Vite's (dev server and vitest); the production build never reaches this module.
  const modules = import.meta.glob("../**/*.gallery.tsx", { eager: true }) as Record<string, AppGalleryModule>;
  return flattenGallerySections(modules);
}
