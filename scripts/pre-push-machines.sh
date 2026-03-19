#!/usr/bin/env bash
# If machine-related files changed, run codegen + verify
changed=$(git diff --cached --name-only HEAD | grep -E '(machine-schema/src/catalog|_authority\.rs|machine-kernels/src/generated)')
if [ -n "$changed" ]; then
    echo "Machine files changed, running codegen + verify..."
    cargo xtask machine-codegen --all || exit 1
    cargo xtask machine-verify --all || exit 1
fi
