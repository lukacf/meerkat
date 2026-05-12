# Machine Posters

Generates large-format machine posters for the canonical machines from the
generated formal artifacts.

## Output

Running the generator writes self-contained HTML posters to:

- `docs-internal/machine-posters/index.html`
- `docs-internal/machine-posters/meerkat_machine.html`
- `docs-internal/machine-posters/mob_machine.html`

## Source of Truth

The posters are driven from the canonical generated machine specs:

- `specs/machines/meerkat_machine/model.tla`
- `specs/machines/meerkat_machine/contract.md`
- `specs/machines/mob_machine/model.tla`
- `specs/machines/mob_machine/contract.md`

The TLA models provide the transition bodies, variable writes, and invariant
formulae. The generated contracts provide the normalized `INPUTS` / `SIGNALS`
split, effects, and human-readable transition metadata.

## Usage

```bash
node scripts/machine-posters/generate-machine-posters.mjs
```

Regenerate after machine/schema/spec changes so the posters stay aligned with
the canonical formal machine definitions.
