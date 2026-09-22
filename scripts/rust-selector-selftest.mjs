#!/usr/bin/env node
import assert from "node:assert/strict";
import { embeddedInputs, moduleReferences, packageForFile } from "./rust-test-selector.mjs";

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

// Package directories sorted longest-first, as packageDirs() produces them.
const dirs = [
  ["meerkat-core", { name: "meerkat-core" }],
  ["meerkat", { name: "meerkat" }],
];

assert.equal(packageForFile("meerkat-core/src/lib.rs", dirs)?.name, "meerkat-core");
assert.equal(packageForFile("meerkat/Cargo.toml", dirs)?.name, "meerkat");

// The facade embeds the platform and CLI reference skills through tracked
// symlinks under meerkat/embedded_skills; edits to the symlink targets are
// facade inputs and must select the facade, not fall through as docs-only.
assert.equal(packageForFile(".claude/skills/meerkat-platform/SKILL.md", dirs)?.name, "meerkat");
assert.equal(
  packageForFile("./.claude/skills/meerkat-platform/references/mobs.md", dirs)?.name,
  "meerkat",
);
assert.equal(packageForFile(".claude/skills/meerkat-cli-reference/SKILL.md", dirs)?.name, "meerkat");

// Unrelated repo-root documents still select nothing.
assert.equal(packageForFile(".claude/skills/meerkat-architecture/SKILL.md", dirs), null);
assert.equal(packageForFile(".claude/agents/rust-quality-gate.md", dirs), null);
assert.equal(packageForFile("docs/reference/build-and-ci.mdx", dirs), null);

const embedded = embeddedInputs(dirs);
assert.ok(embedded.length > 0, "expected tracked embedded-input symlinks under crate directories");
assert.ok(embedded.every((input) => input.pkg.name === "meerkat"));
assert.ok(embedded.some((input) => input.path === ".claude/skills/meerkat-platform/SKILL.md"));

console.log("rust selector selftest ok");
