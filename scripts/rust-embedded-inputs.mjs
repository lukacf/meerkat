#!/usr/bin/env node
// Prints every repo path a workspace crate embeds through a tracked symlink,
// one `<path>\t<package>\t<file|dir>` row per line. The agent gates use this to
// treat edits to those targets (for example `.claude/skills/meerkat-platform/`
// behind `meerkat/embedded_skills/`) as build-relevant crate inputs instead of
// docs-only changes. See `embeddedInputs` in rust-test-selector.mjs.
import { embeddedInputs, packageDirs, readMetadata, workspacePackages } from "./rust-test-selector.mjs";

const dirs = packageDirs(workspacePackages(readMetadata()));
for (const input of embeddedInputs(dirs)) {
  process.stdout.write(`${input.path}\t${input.pkg.name}\t${input.isDirectory ? "dir" : "file"}\n`);
}
