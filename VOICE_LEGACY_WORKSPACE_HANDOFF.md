# Legacy voice workspace preservation

This is an emergency archival checkpoint requested before anticipated internet
loss. It preserves the original uncommitted feature workspace based on
`890e3e71cc1b68bd0fe88198f2e1b6717dd2a76d`. It is not the current integration
candidate and must not replace the newer voice implementation.

The current portable development baseline is
`73a0b869afb872d16b39d9de2ef5bd74e1c4f8a1`, published on
`luka-crnkovicfriis-abk-voice-qualified-73a0b869`. That baseline passed its
deterministic qualification but FAILED its one real S99 historical-recall run.
It is not accepted for release.

The latest ordinal/provenance repair and bounded evidence-journal WIP is
preserved separately on `luka-crnkovicfriis-abk-async-voice-context`.
Read `ASYNC_VOICE_CONTEXT_HANDOFF.md` on that branch for the authoritative
continuation instructions and `artifacts/evidence/s99-paid-73a0b869.log` for
the retained redacted failure evidence. That repair is incomplete and
unqualified; do not use its WIP ABI as a consumer pin.

All engineering, gates, and paid calls were paused for this preservation task.
Push verification hooks were bypassed only under the user's explicit emergency
authorization. No merge, release, version bump, or further paid retry is
authorized. The historical-recall failure's cause remains unproven; the
separately reproduced post-bootstrap echo defect must not be presented as its
established cause.

Coordination: MobKit Root `074d7dbf-8bf3-45e6-b888-997f540693b4`;
Meerkat coordinator `f760343a-5dc5-4e49-a23a-7648416e1b11`;
current repair owner `ddf8be20-2d38-47d0-a808-140f4b03e666`.
