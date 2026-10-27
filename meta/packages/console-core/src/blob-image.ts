/// Image media types the Console shows from a stored blob. SVG is not one of
/// them: the bytes are shown through an object URL, which carries the
/// Console's origin, so a scriptable image opened full size would run as the
/// Console.
export const CONSOLE_DISPLAYABLE_IMAGE_TYPES: ReadonlySet<string> = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);

/// The displayable image media type a blob or response declares, or
/// `undefined` when it declares anything else.
export function displayableImageMediaType(contentType: string | null | undefined): string | undefined {
  const mediaType = contentType?.split(";")[0]?.trim().toLowerCase() ?? "";
  return CONSOLE_DISPLAYABLE_IMAGE_TYPES.has(mediaType) ? mediaType : undefined;
}
