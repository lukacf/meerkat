// Hand-maintained aggregator for files written by `xtask protocol-codegen`.
// Each sibling module carries its own `@generated` provenance header. This
// aggregator is not itself a codegen output: it is a stable `pub mod` index,
// and `xtask audit-generated-headers` forbids the codegen marker here to keep
// that honest.

pub mod session_turn_admission;
