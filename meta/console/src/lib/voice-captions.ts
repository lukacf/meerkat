/**
 * Provisional assistant captions of one console voice call, read from
 * `meta/console/voice/captions`. Meerkat publishes the in-progress text of
 * each open assistant segment keyed by the segment's item id, the same id the
 * committed assistant row later lists in `realtime_origin.provider_item_ids`.
 * A caption carries the segment's whole text so far and replaces earlier text;
 * a retraction names a segment no committed row will replace. A playback hint
 * is Meerkat's barge-in signal (`live/assistant_playback_hint`): duck the
 * assistant's playback while the user speaks over it, restore it afterwards.
 */

export const VOICE_CAPTIONS_METHOD = "meta/console/voice/captions";

export type VoicePlaybackHint = "duck" | "restore";

export type VoiceCaption =
  | { readonly kind: "caption"; readonly itemId: string; readonly text: string }
  | { readonly kind: "retracted"; readonly itemId: string }
  | { readonly kind: "playback_hint"; readonly hint: VoicePlaybackHint };

export interface VoiceCaptionBatch {
  readonly cursor: number;
  readonly captions: readonly VoiceCaption[];
}

interface VoiceCaptionScope {
  readonly identity: string;
  readonly requestId: string;
  readonly channelId: string;
}

function invalid(): never {
  throw new Error("Invalid voice captions.");
}

function record(raw: unknown): Record<string, unknown> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) invalid();
  return raw as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, keys: string[]) {
  if (Object.keys(value).length !== keys.length || keys.some((key) => !Object.hasOwn(value, key))) invalid();
}

function itemId(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) invalid();
  return value;
}

export function parseVoiceCaptions(raw: unknown, scope: VoiceCaptionScope, after: number): VoiceCaptionBatch {
  const result = record(raw);
  exactKeys(result, ["identity", "request_id", "channel_id", "cursor", "captions"]);
  if (
    result.identity !== scope.identity ||
    result.request_id !== scope.requestId ||
    result.channel_id !== scope.channelId
  ) throw new Error("Voice captions do not match the active call.");
  const cursor = result.cursor;
  if (typeof cursor !== "number" || !Number.isSafeInteger(cursor) || cursor < after) invalid();
  if (!Array.isArray(result.captions)) invalid();
  const captions = result.captions.map((raw): VoiceCaption => {
    const caption = record(raw);
    if (caption.kind === "caption") {
      exactKeys(caption, ["kind", "item_id", "text"]);
      if (typeof caption.text !== "string") invalid();
      return { kind: "caption", itemId: itemId(caption.item_id), text: caption.text };
    }
    if (caption.kind === "retracted") {
      exactKeys(caption, ["kind", "item_id"]);
      return { kind: "retracted", itemId: itemId(caption.item_id) };
    }
    if (caption.kind === "playback_hint") {
      exactKeys(caption, ["kind", "hint"]);
      if (caption.hint !== "duck" && caption.hint !== "restore") invalid();
      return { kind: "playback_hint", hint: caption.hint };
    }
    return invalid();
  });
  return { cursor, captions };
}
