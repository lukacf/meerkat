#!/usr/bin/env bash
# pre-push-bridge-no-responsestatus.sh — W2-F bridge-classifier gate.
#
# The structural implementation lives in `xtask bridge-classifier` (syn AST
# rules in xtask/src/bridge_classifier.rs); this wrapper only execs it so the
# pre-commit hook and Makefile keep a stable entry point.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec ./scripts/repo-cargo run -p xtask -- bridge-classifier
