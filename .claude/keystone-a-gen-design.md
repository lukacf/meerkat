# Keystone A Generalization — Design (from scoping wf wy4pf1yym)

Goal: emit the per-machine parity mirrors FROM the MachineSchema so a machine fold
is a one-file catalog edit, not a 9-failure cascade. Validates the user's thesis:
the "needs-annotation" mirrors are catalog FACTS, not downstream hand tables.

## Mirror verdicts
- M1 CatalogInput enum + ALL + input_variant() + as_str(): FULLY derivable from
  schema.inputs.variants. Hand at meerkat-mob/src/mob_machine.rs:298-869 AND
  meerkat-runtime/src/meerkat_machine_types.rs:1110-1336 (Mob + Meerkat). Generated
  *InputVariant enums already exist (precedent). catalog_input() on CommandVariant
  stays hand (policy). GO NOW.
- M4 field-evaluator: FULLY derivable from schema.state.fields. tests.rs snapshot
  summary + field_value. GO NOW.
- M2 runtime-internal reason: NEEDS ANNOTATION. reason=authority-domain, NOT from
  effect seam. Add a catalog per-input `reason <X>` clause (or a
  mob_machine_runtime_internal_input_reasons! macro alongside runtime_internal_inputs!).
- M3 flow-authority partition: NEEDS ANNOTATION. Add `@flow_authority` marker on the
  9 flow inputs in the catalog input block.
- M5 probe partition: NEEDS (minimal) ANNOTATION. probe_required already derived
  (¬surface ∧ ¬runtime-internal); bless the partition.

## Emission seam (M6)
codegen already emits per-machine *InputVariant enums via render_named_type_definition
into meerkat-machine-kernels/src/generated/<machine>.rs. Add a render for the richer
CatalogInput mirror (enum+ALL+input_variant+as_str) into a generated module consumed
by mob + meerkat; wire into xtask machine-codegen --all loop.

## Sequencing
1. M1 CatalogInput generation (biggest tax, GO) -> delete mob+meerkat hand enums.
2. M4 field-evaluator generation (GO).
3. Catalog annotations (reason/flow-authority/probe) -> generate M2/M3/M5.
Then PRIMITIVE 1 (MachineInit-from-config) + PRIMITIVE 2 (schema->wire-projection, collapses D2).

## M1 DESIGN CORRECTION (discovered during impl — agent ad70ba8 draft, reverted)
The CatalogInput mirror CANNOT be emitted into the kernel (meerkat-machine-kernels) referencing
catalog::dsl::<m>::<Prefix>InputVariant, because:
 (1) the catalog-dsl module name doesn't derive cleanly from machine_slug (mob_machine/meerkat_machine
     have the _machine suffix; session_document/occurrence_lifecycle/etc. do NOT) — broke 8 machines; AND
 (2) MORE FUNDAMENTAL: consumers (meerkat-mob, meerkat-runtime) use the PRODUCTION <Prefix>InputVariant
     emitted into THEIR crate by the production macro (e.g. mob_catalog_machine_dsl!("meerkat-mob",
     "machines::mob_machine") emits meerkat-mob's own MobMachineInputVariant) — a DIFFERENT type from the
     catalog-dsl one. A kernel-emitted CatalogInput.input_variant() -> catalog-dsl InputVariant would
     type-mismatch the consumers' production InputVariant.
CORRECTED TARGET: emit CatalogInput into the CONSUMER crate (meerkat-mob/src/generated/, meerkat-runtime/
src/generated/) referencing the PRODUCTION InputVariant in that crate — i.e. via the protocol-codegen
per-consumer emitter pattern (same machinery that emits meerkat-core/src/generated/* and
meerkat-session/src/generated/* wrappers), NOT machine-codegen's kernel render. Gate to the machines that
actually have a CatalogInput consumer (mob + meerkat) — the per-consumer protocol-codegen registration is
the natural gate (only those crates register a CatalogInput emission), avoiding over-generation for the
other 8 machines.
NEXT: build the CatalogInput emitter in xtask/src/protocol_codegen.rs (mirror render_session_document_authority /
the meerkat-core wrapper emitters), emit into meerkat-mob/src/generated/catalog_input.rs +
meerkat-runtime/src/generated/catalog_input.rs, delete the hand enums, repoint consumers.
