import test from "node:test";
import assert from "node:assert/strict";
import {
  dedupeComposerImageFiles,
  consoleBlobReferencesFromText,
  normalizeConsoleBlobUrl,
  selectImageTransferFiles,
  stripConsoleBlobReferencesFromText,
} from "./composer-attachment-text";

const BASE = "http://127.0.0.1:49551/console";
const ORIGIN = "http://127.0.0.1:49551";

test("normalizeConsoleBlobUrl accepts same-origin absolute and relative blob URLs", () => {
  assert.equal(
    normalizeConsoleBlobUrl(
      "http://127.0.0.1:49551/blobs/sha256%3Aabc",
      BASE,
      ORIGIN,
    ),
    "http://127.0.0.1:49551/blobs/sha256%3Aabc",
  );
  assert.equal(
    normalizeConsoleBlobUrl("/blobs/sha256%3Aabc", BASE, ORIGIN),
    "http://127.0.0.1:49551/blobs/sha256%3Aabc",
  );
});

test("normalizeConsoleBlobUrl rejects cross-origin and non-blob URLs", () => {
  assert.equal(normalizeConsoleBlobUrl("https://example.com/blobs/sha256%3Aabc", BASE, ORIGIN), null);
  assert.equal(normalizeConsoleBlobUrl("http://127.0.0.1:49551/console", BASE, ORIGIN), null);
});

test("consoleBlobReferencesFromText extracts unique image references from text and HTML", () => {
  const refs = consoleBlobReferencesFromText(
    `Describe this /blobs/sha256%3Aabc <img src="http://127.0.0.1:49551/blobs/sha256%3Aabc"> <a href="/blobs/sha256%3Adef">second</a>`,
    BASE,
    ORIGIN,
  );
  assert.deepEqual(
    refs.map((ref) => ref.href),
    [
      "http://127.0.0.1:49551/blobs/sha256%3Aabc",
      "http://127.0.0.1:49551/blobs/sha256%3Adef",
    ],
  );
});

test("stripConsoleBlobReferencesFromText removes blob URLs while preserving prompt text", () => {
  const draft = "Describe this image http://127.0.0.1:49551/blobs/sha256%3Aabc and call out rollback status.";
  const refs = consoleBlobReferencesFromText(draft, BASE, ORIGIN);
  assert.equal(
    stripConsoleBlobReferencesFromText(draft, refs),
    "Describe this image and call out rollback status.",
  );
});

test("dedupeComposerImageFiles collapses duplicate drag/drop file surfaces", () => {
  const file = { name: "incident.png", type: "image/png", size: 42, lastModified: 7 };
  const duplicate = { name: "incident.png", type: "image/png", size: 42, lastModified: 7 };
  const other = { name: "other.png", type: "image/png", size: 42, lastModified: 7 };

  assert.deepEqual(
    dedupeComposerImageFiles([file, duplicate, other]),
    [file, other],
  );
});

test("selectImageTransferFiles prefers direct clipboard files over item duplicates", () => {
  const direct = { name: "image.png", type: "image/png", size: 42, lastModified: 1 };
  const sameClipboardItem = { name: "pasted-image.png", type: "image/png", size: 42, lastModified: 2 };

  assert.deepEqual(
    selectImageTransferFiles([direct], [sameClipboardItem]),
    [direct],
  );
  assert.deepEqual(
    selectImageTransferFiles([], [sameClipboardItem]),
    [sameClipboardItem],
  );
});
