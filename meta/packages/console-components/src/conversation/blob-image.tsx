import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { displayableImageMediaType, type ConversationRichImageBlock } from "@console-core";

/// Reads a stored blob's bytes with the host's request authority: the
/// Console transport's `readBlob`. Resolves to a Blob typed with its image
/// media type.
export type ConversationBlobReader = (input: { blobId: string; signal: AbortSignal }) => Promise<Blob>;

const ConversationBlobReaderContext = createContext<ConversationBlobReader | undefined>(undefined);

/// One read's result, bound to the reader, blob and source it was read for.
type LoadedImage = { reader: ConversationBlobReader; blobId: string; src: string; objectUrl?: string; unavailable: boolean };

/// Gives the conversation the host transport's blob reader. Without one,
/// stored images keep their direct URL. Pass a stable reader: a new reader
/// restarts every stored image's read.
export function ConversationBlobReaderProvider({ reader, children }: { reader?: ConversationBlobReader; children: ReactNode }) {
  return <ConversationBlobReaderContext.Provider value={reader}>{children}</ConversationBlobReaderContext.Provider>;
}

/// The source to show for an image. A stored image (`/blobs/{id}`) is read
/// through the host transport and shown from one object URL, which serves
/// both the thumbnail and "open full size". The URL is revoked when the image
/// unmounts or its source changes; a read the retired Console started is
/// refused by the transport. Only raster image types are shown, since an
/// object URL carries the Console's origin. A refused or non-image read is
/// `unavailable`.
///
/// Limits: an image already read stays shown until its view or the Console
/// instance goes away, so a token expiry or logout the host does not signal
/// by retiring the Console does not hide it. Revoking the object URL cannot
/// erase an image already open in another tab or saved.
export function useConversationImageSource(image: ConversationRichImageBlock): { src?: string; unavailable: boolean } {
  const reader = useContext(ConversationBlobReaderContext);
  const blobId = image.blobId;
  const stored = Boolean(reader && blobId && image.src.endsWith(`/blobs/${encodeURIComponent(blobId)}`));
  const [loaded, setLoaded] = useState<LoadedImage | null>(null);
  useEffect(() => {
    // Every restart (another source, blob or reader) drops the previous
    // result, so a replaced reader's image is never shown while the new read
    // is pending.
    setLoaded(null);
    if (!stored || !reader || !blobId) return undefined;
    const controller = new AbortController();
    let objectUrl: string | undefined;
    void (async () => {
      try {
        const bytes = await reader({ blobId, signal: controller.signal });
        if (controller.signal.aborted) return;
        if (!displayableImageMediaType(bytes.type)) throw new Error("not a displayable image");
        objectUrl = URL.createObjectURL(bytes);
        setLoaded({ reader, blobId, src: image.src, objectUrl, unavailable: false });
      } catch {
        if (!controller.signal.aborted) setLoaded({ reader, blobId, src: image.src, unavailable: true });
      }
    })();
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [stored, reader, blobId, image.src]);
  if (!stored) return { src: image.src, unavailable: false };
  // Only the current reader's result for this exact blob and source counts.
  const current = loaded && loaded.reader === reader && loaded.blobId === blobId && loaded.src === image.src
    ? loaded
    : null;
  return current ? { src: current.objectUrl, unavailable: current.unavailable } : { unavailable: false };
}
