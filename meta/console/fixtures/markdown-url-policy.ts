import type { MarkdownUrlPolicy } from "@console-components";

/** Explicit policy supplied by the acceptance host, never by transcript content. */
export function acceptanceMarkdownUrlPolicy(
  host: Pick<Location, "origin" | "search"> = window.location,
): MarkdownUrlPolicy | undefined {
  const options = new URLSearchParams(host.search);
  if (options.get("markdownPolicy") !== "custom") return undefined;
  const approvedBlob = options.get("approvedImage");
  const approvedImage = approvedBlob ? `${host.origin}/blobs/${encodeURIComponent(approvedBlob)}` : null;
  return {
    resolveLink: url => {
      if (url === "/release/review" || url === "meta:review") return `${host.origin}/console#release-review`;
      return url === "https://example.com/reference" ? url : null;
    },
    resolveImage: url => approvedImage !== null && url === approvedImage ? url : null,
  };
}
