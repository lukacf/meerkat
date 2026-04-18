# Realtime transcript fidelity

Status: normative reference.
Audience: platform operators, SDK authors, provider-adapter contributors.

The realtime channel carries two different kinds of transcript state:

1. What the model *generated* (the text the LLM produced for a turn).
2. What the user *actually heard* (the audio that reached the user's ear).

These two usually match. Two cases make them diverge, and only one of them
is fixable at the transport layer.

---

## Barge-in is solved

TTS replays the model's generated audio at roughly 1× real-time while the
model itself generates much faster (2× and more). When the user barges in
mid-utterance, the canonical session transcript would otherwise capture a lot
of text the user never heard.

OpenAI Realtime is explicit about the seam. When the client detects the user
speaking over the assistant, it tells the server "here is the playback
cursor":

```
client → server   conversation.item.truncate { item_id, content_index, audio_end_ms }
server → client   conversation.item.truncated { item_id, audio_end_ms, ... }
```

See the [OpenAI Realtime — Client-Side Truncation][oai-truncate] guide for the
canonical description.

Meerkat exposes the same seam on its own wire protocol so every SDK uses the
same contract:

- **Client → server:** `RealtimeClientFrame::ChannelBargeInTruncate { item_id,
  content_index, audio_played_ms }`. The client MUST send this frame the
  moment it detects the user starting to talk over the assistant's audio.
  `audio_played_ms` is the wall-clock cursor of the speaker output for the
  assistant item — it must not exceed the true elapsed duration of the audio
  content the client played back.

- **Server → client:** `RealtimeEvent::AssistantTranscriptTruncated {
  item_id, audio_played_ms, truncated_text }`. The platform emits this once
  the provider has projected the heard prefix back through the canonical
  session. `truncated_text` is the heard portion; it is `None` on providers
  that cannot yet supply a re-projected transcript — downstream projectors
  should keep whatever transcript they already have until a later emission
  fills it in.

This replaces the old "document it as a limitation" posture: the fix exists,
and every realtime client (Rust SDK, TypeScript SDK, Python SDK, ESP32
bridge, browser SDK) MUST report the playback cursor to the server on
barge-in for the session history to be accurate.

### Implementation notes

- **Client responsibility.** The client is the only party that knows exactly
  how many samples of assistant audio reached the speaker before the user
  interrupted. Reporting a cursor that overestimates the heard prefix will
  make the session history include text the user never heard; reporting
  zero will make the assistant message go blank. Report the real cursor.

- **Adapter integration.** The OpenAI adapter (`openai_live.rs`) maps
  `BargeInTruncate` onto OpenAI's `conversation.item.truncate` and watches
  for the server's `conversation.item.truncated` response to emit the
  meerkat-side event. Providers that lack equivalent server-side truncation
  should either fall back to client-local truncation (slice the in-flight
  transcript at the proportional char offset) or surface `truncated_text:
  None` and let the session projector keep the previous transcript.

- **Idempotency.** `ChannelBargeInTruncate` is safe to call multiple times
  for the same `item_id`: the adapter remembers the cursor and prefers the
  later (larger) of the client-reported and server-observed values.

## Prosody is not solved at the provider level

Tone, sarcasm, affect, pitch contour, and similar paralinguistic signals
are not present in the transcript text. OpenAI Realtime (as of 2026-04-17)
does not expose them as structured signals on the event stream, so a
compactor LLM that summarises the session history is working on a lossy
layer. Example failure mode: a user saying "that sounds great" ironically
becomes indistinguishable from an enthusiastic "that sounds great" in the
compacted summary.

Meerkat documents this as a known limitation; it is not a fixable transport
bug today. The platform does carry an **optional annotation seam** so that
providers which eventually surface structured prosody can attach it without
waiting for a protocol bump:

```rust
pub enum RealtimeEvent {
    InputTranscriptFinal {
        text: String,
        prosody_hint: Option<String>,  // additive, provider-specific
    },
    ...
}
```

Compactors may consult `prosody_hint` as a hint but must NOT treat it as
authoritative — the authoritative transcript is still `text`.

## See also

- [`docs/guides/realtime.mdx`][guide] — end-user guide, state diagram,
  authority token.
- [`.claude/skills/meerkat-architecture/references/realtime-attachment.md`][skill-ref] —
  internal architecture reference.
- OpenAI Realtime API, "Handling barge-in", [Client-Side Truncation][oai-truncate].

[oai-truncate]: https://platform.openai.com/docs/guides/realtime-conversations
[guide]: ../guides/realtime.mdx
[skill-ref]: ../../.claude/skills/meerkat-architecture/references/realtime-attachment.md
