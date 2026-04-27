# Assistant Image Generation Substrate RCT Source Of Truth

This RCT package governs the implementation of
`docs/architecture/assistant-image-generation-substrate.md`.

The ADR is normative for product semantics. Meerkat architecture doctrine remains
normative for ownership:

- `meerkat-core` owns typed contracts and traits, with no I/O dependencies.
- `meerkat-contracts` owns wire types.
- provider crates own provider mechanics.
- `meerkat-tools` may host builtin mechanics, but `generate_image` semantics are
  machine-owned and privileged.
- `MeerkatMachine` owns image operation, switch-turn, approval, scoped override,
  and model-routing status semantics.

External API facts were validated against official docs on 2026-04-26:

- OpenAI Responses supports a hosted `image_generation` tool. The tool emits
  `image_generation_call` output containing base64 image data and an optional
  `revised_prompt`. GPT Image models are backend image targets, not valid
  Responses `model` values.
- Gemini image models expose native text/image output through `generateContent`
  parts; generated text and inline image parts can be interleaved.

The implementation must keep deterministic lanes hermetic. Live provider image
tests belong only to explicit live lanes such as `e2e-smoke` / ignored tests.
