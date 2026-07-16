#!/usr/bin/env node
import assert from "node:assert/strict";
import { moduleReferences } from "./rust-test-selector.mjs";

const refs = moduleReferences(`
//! A doc example with #[path = "support/not_a_module.rs"] must not affect
//! the next real module declaration.
// mod commented_out;
mod plain;
pub mod public_mod;
pub(crate) mod scoped_mod;
#[path = "support/two_line.rs"]
mod two_line;
#[path = "support/same_line.rs"] mod same_line;
`);

assert.deepEqual(refs, [
  { name: "plain" },
  { name: "public_mod" },
  { name: "scoped_mod" },
  { path: "support/two_line.rs" },
  { path: "support/same_line.rs" },
]);

console.log("rust selector selftest ok");
