#!/usr/bin/env bash
# Write only committed inputs. Never archive the checkout, its .git directory,
# or the pack's local environment, bindings, state, and build caches.
set -euo pipefail
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
# The lockfile covers the unified workspace. Include crate manifests, declared
# Cargo target entrypoints and sources, plus compile-time embedded assets.
# The two tracked skill trees supply the targets of embedded_skills symlinks.
exec git -C "$root" archive --format=tar.gz HEAD -- \
  .claude/skills/meerkat-cli-reference/SKILL.md .claude/skills/meerkat-platform/SKILL.md \
  ':(glob).claude/skills/meerkat-platform/references/*.md' \
  ':(glob)crates/*/tests/*.rs' ':(glob)crates/meerkat/examples/*.rs' \
  ':(glob)examples/*-rs/main.rs' ':(glob)meta/examples/*/server.rs' \
  ':(glob)connect/crates/*/src/bin/*.rs' connect/crates/meerkat-connect/tests/mobkit_setup_member.rs \
  ':(glob)connect/crates/meerkat-connect/tests/support/*.rs' ':(glob)tests/integration/tests/*.rs' \
  meta/examples/004-mdm-console-pack/scripts/build-gcp-target.sh \
  Cargo.toml Cargo.lock rust-toolchain.toml .cargo/config.toml \
  ':(glob)crates/*/Cargo.toml' ':(glob)crates/*/build.rs' ':(glob)crates/*/src/**' \
  ':(glob)crates/*/skills/**' crates/meerkat/embedded_skills crates/meerkat-sandbox/third_party \
  ':(glob)meta/crates/*/Cargo.toml' ':(glob)meta/crates/*/src/**' \
  meta/crates/meerkat-meta/assets meta/crates/meerkat-meta/console-dist \
  meta/crates/meerkat-meta/flow-editor-dist meta/examples/004-mdm-console-pack/target.rs \
  ':(glob)connect/crates/*/Cargo.toml' ':(glob)connect/crates/*/src/lib.rs' \
  ':(glob)connect/crates/*/src/main.rs' \
  tests/integration/Cargo.toml tests/integration/src/lib.rs \
  ':(glob)tests/fixtures/*/Cargo.toml' ':(glob)tests/fixtures/*/src/**/*.rs'
