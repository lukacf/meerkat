export interface ConsoleBlobReference {
  href: string;
  raw: string;
}

export interface ComposerImageFileLike {
  name: string;
  type: string;
  size: number;
  lastModified?: number;
}

export function composerImageFileKey(file: ComposerImageFileLike): string {
  return [
    file.name || "",
    file.type || "",
    String(file.size),
    String(file.lastModified ?? 0),
  ].join("\u0000");
}

export function dedupeComposerImageFiles<T extends ComposerImageFileLike>(files: readonly T[]): T[] {
  const seen = new Set<string>();
  const deduped: T[] = [];
  for (const file of files) {
    const key = composerImageFileKey(file);
    if (seen.has(key)) continue;
    seen.add(key);
    deduped.push(file);
  }
  return deduped;
}

export function selectImageTransferFiles<T extends ComposerImageFileLike>(
  directFiles: readonly T[],
  itemFiles: readonly T[],
): T[] {
  // Browsers can expose the same pasted image through both
  // DataTransfer.files and DataTransfer.items. Prefer the direct file list
  // when present; it is the canonical browser surface for file transfers.
  return dedupeComposerImageFiles(directFiles.length > 0 ? directFiles : itemFiles);
}

function defaultBaseHref(): string {
  if (typeof window !== "undefined") return window.location.href;
  return "http://localhost/";
}

function defaultOrigin(baseHref: string): string {
  if (typeof window !== "undefined") return window.location.origin;
  return new URL(baseHref).origin;
}

export function normalizeConsoleBlobUrl(
  raw: string,
  baseHref = defaultBaseHref(),
  origin = defaultOrigin(baseHref),
): string | null {
  try {
    const url = new URL(raw.trim(), baseHref);
    if (url.origin !== origin) return null;
    if (!url.pathname.startsWith("/blobs/")) return null;
    return url.href;
  } catch {
    return null;
  }
}

export function consoleBlobReferencesFromText(
  value: string,
  baseHref = defaultBaseHref(),
  origin = defaultOrigin(baseHref),
): ConsoleBlobReference[] {
  const normalized = value.replace(/&amp;/g, "&");
  const candidates = [
    ...Array.from(normalized.matchAll(/\b(?:src|href)=["']([^"']+)["']/gi)).map((match) => match[1]),
    ...Array.from(normalized.matchAll(/(?:https?:\/\/[^\s"'<>]+|\/blobs\/[^\s"'<>]+)/gi)).map((match) => match[0]),
  ];
  const refs: ConsoleBlobReference[] = [];
  const seen = new Set<string>();
  for (const candidate of candidates) {
    const href = normalizeConsoleBlobUrl(candidate, baseHref, origin);
    if (!href || seen.has(href)) continue;
    seen.add(href);
    refs.push({ href, raw: candidate });
  }
  return refs;
}

export function consoleBlobUrlsFromText(value: string): string[] {
  return consoleBlobReferencesFromText(value).map((ref) => ref.href);
}

export function stripConsoleBlobReferencesFromText(
  value: string,
  references: ConsoleBlobReference[] = consoleBlobReferencesFromText(value),
): string {
  let next = value;
  for (const ref of references) {
    next = next.split(ref.raw).join("");
    next = next.split(ref.href).join("");
  }
  return next
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
}
