// Hand-maintained aggregator for files written by xtask codegen passes.
// Each sibling module carries its own generated header. This index is not a
// generated artifact so the generated-header audit can distinguish codegen
// output from stable module wiring.

pub mod meerkat_mob_seam;
