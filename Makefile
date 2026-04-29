# Meerkat CI/CD Makefile
# Single source of truth for all build, test, and lint commands

CRATE_NAME := meerkat
XTASK_TARGET_DIR ?= /tmp/meerkat-xtask-target
XTASK_BIN := $(XTASK_TARGET_DIR)/debug/xtask
CARGO ?= ./scripts/repo-cargo
PYTHON ?= $(shell command -v python3.11 2>/dev/null || command -v python3)

# Colors for terminal output
GREEN := \033[0;32m
YELLOW := \033[0;33m
RED := \033[0;31m
NC := \033[0m

.PHONY: all install-build-deps build test test-unit test-int e2e-fast e2e-build e2e-system e2e-live e2e-smoke test-int-real test-e2e test-all test-minimal test-feature-matrix-lib test-feature-matrix-surface test-feature-matrix test-surface-modularity test-sdk-python test-sdk-typescript test-sdk-suites lint lint-feature-matrix fmt fmt-check audit rust-lane-doctor agent-gate cargo-agent-gate buildbuddy-install buildbuddy-generate buildbuddy-generate-check buildbuddy-doctor buildbuddy-build buildbuddy-check buildbuddy-clippy buildbuddy-lint buildbuddy-test buildbuddy-test-all buildbuddy-test-unit buildbuddy-test-int buildbuddy-e2e-fast buildbuddy-e2e-system buildbuddy-e2e-live buildbuddy-e2e-smoke buildbuddy-agent-gate buildbuddy-ci-dispatch buildbuddy-fast buildbuddy-benchmark buildbuddy-ci buildbuddy-ci-warm buildbuddy-ci-full buildbuddy-ci-full-warm ci ci-smoke release-preflight release-preflight-smoke publish-dry-run publish-dry-run-python publish-dry-run-typescript release-dry-run release-dry-run-smoke clean doc release install-hooks coverage check help legacy-surface-gate legacy-surface-inventory session-control-gate deprecated-backend-gate deprecated-backend-inventory verify-version-parity verify-schema-freshness verify-rpc-surface-alignment verify-sdk-wrapper-freshness check-rust-release-packaging check-mini-skill-size bump-sdk-versions smoke-sdk-python-artifact smoke-sdk-typescript-artifact xtask-build machine-codegen machine-verify machine-check-drift seam-inventory rmat-audit audit-generated-headers

# Default target
all: ci

install-build-deps:
	@echo "$(GREEN)Installing local Rust build dependencies...$(NC)"
	@scripts/install-build-deps

# Build the project
build:
	@echo "$(GREEN)Building $(CRATE_NAME)...$(NC)"
	@scripts/run-build-backend-lane build

# Build release version
release:
	@echo "$(GREEN)Building release version...$(NC)"
	$(CARGO) build --workspace --release

# Fast test suite (unit + integration-fast, skips doctests and ignored)
test:
	@echo "$(GREEN)Running fast tests (unit + integration-fast)...$(NC)"
	@scripts/run-build-backend-lane test

# Unit tests only
test-unit:
	@echo "$(GREEN)Running unit tests...$(NC)"
	@scripts/run-build-backend-lane test-unit

# Integration-fast tests only (no unit tests). The `int` cargo alias excludes
# the dedicated e2e lane binaries since those have their own Makefile targets
# and scenario-test timeouts incompatible with the integration-fast lane's
# default nextest deadline.
test-int:
	@echo "$(GREEN)Running integration-fast tests...$(NC)"
	@scripts/run-build-backend-lane test-int

# Deterministic end-to-end lane (canonical integration harness)
e2e-fast:
	@echo "$(GREEN)Running e2e-fast lane...$(NC)"
	@scripts/run-build-backend-lane e2e-fast

e2e-build:
	@echo "$(YELLOW)Running e2e-build lane (ignored by default)...$(NC)"
	$(CARGO) e2e-build

# Real local resources only (binaries, sockets, filesystems; no live providers)
e2e-system:
	@echo "$(GREEN)Running e2e-system lane...$(NC)"
	@scripts/run-build-backend-lane e2e-system

# Targeted live-provider boundary checks
e2e-live:
	@echo "$(YELLOW)Running e2e-live lane (ignored by default)...$(NC)"
	@scripts/run-build-backend-lane e2e-live

# Compound live-provider smoke scenarios
e2e-smoke:
	@echo "$(YELLOW)Running e2e-smoke lane (ignored by default)...$(NC)"
	@TEST="$(TEST)" SCENARIO="$(SCENARIO)" SUITE="$(SUITE)" scripts/run-build-backend-lane e2e-smoke

# Live per-model catalog validation (ignored by default; on-demand / pre-release)
e2e-models:
	@echo "$(YELLOW)Running e2e-models lane (ignored by default)...$(NC)"
	$(CARGO) e2e-models

# Temporary compatibility shims during lane migration.
test-int-real: e2e-system

test-e2e:
	@echo "$(YELLOW)Running legacy e2e shim (e2e-live + e2e-smoke)...$(NC)"
	@$(MAKE) e2e-live
	@$(MAKE) e2e-smoke

# Full deterministic fast suite with all features. Dedicated e2e lanes remain
# explicit so broad workspace commands do not accidentally run local-resource or
# live-provider wrappers.
test-all:
	@echo "$(GREEN)Running all-feature fast suite...$(NC)"
	@scripts/run-build-backend-lane test-all

# Python SDK test suite
test-sdk-python:
	@echo "$(GREEN)Running Python SDK tests...$(NC)"
	@(cd sdks/python && \
		$(PYTHON) -m pip install --upgrade pip && \
		$(PYTHON) -m pip install -e ".[dev]" && \
		$(PYTHON) -m pytest -q tests)

# TypeScript SDK test suite
test-sdk-typescript:
	@echo "$(GREEN)Running TypeScript SDK tests...$(NC)"
	@(cd sdks/typescript && \
		npm install --ignore-scripts && \
		npm run build && \
		npm test)

# Combined SDK test suites
test-sdk-suites: test-sdk-python test-sdk-typescript

# Minimal builds without optional features
test-minimal:
	@echo "$(GREEN)Running minimal build checks...$(NC)"
	$(CARGO) check -p meerkat-core
	$(CARGO) check -p meerkat-client --no-default-features
	$(CARGO) check -p meerkat-store --no-default-features
	$(CARGO) check -p meerkat-tools --no-default-features
	$(CARGO) check -p meerkat --no-default-features
	$(CARGO) nextest run -p meerkat-core

# Library crate feature combinations
test-feature-matrix-lib:
	@echo "$(GREEN)Running library feature matrix checks...$(NC)"
	$(CARGO) check -p meerkat-tools --no-default-features --features comms
	$(CARGO) check -p meerkat-tools --no-default-features --features mcp
	$(CARGO) check -p meerkat-tools --no-default-features --features comms,mcp
	$(CARGO) check -p meerkat --no-default-features --features openai,memory-store
	$(CARGO) check -p meerkat --no-default-features --features gemini,jsonl-store
	$(CARGO) check -p meerkat --features all-providers,comms,mcp
	$(CARGO) check -p meerkat-mob --no-default-features
	$(CARGO) check -p meerkat-mob --no-default-features --features runtime-adapter
	$(CARGO) nextest run -p meerkat --features all-providers,comms,mcp

# Surface crate feature combinations
test-feature-matrix-surface:
	@echo "$(GREEN)Running surface feature matrix checks...$(NC)"
	CARGO="$(CARGO)" ./scripts/run-surface-feature-matrix
	$(CARGO) nextest run -p rkat --no-default-features --features session-store,mcp --no-capture

# Session capability matrix (A-F builds from spec)
test-session-matrix:
	@echo "$(GREEN)Running session capability matrix...$(NC)"
	@echo "  Build A: no-default-features (ephemeral only)"
	$(CARGO) check -p meerkat-session --no-default-features
	$(CARGO) nextest run -p meerkat-session --no-default-features
	@echo "  Build B: session-store"
	$(CARGO) check -p meerkat-session --no-default-features --features session-store
	@echo "  Build C: (memory-store — Phase 6)"
	@echo "  Build D: session-compaction"
	$(CARGO) check -p meerkat-session --no-default-features --features session-compaction
	@echo "  Build E: session-store (Phase 6 combo)"
	@echo "  Build F: all session features"
	$(CARGO) check -p meerkat-session --no-default-features --features session-store,session-compaction

# Full feature matrix
test-feature-matrix: test-feature-matrix-lib test-feature-matrix-surface

# Surface modularity guardrail: minimal feature builds + binary smoke checks.
test-surface-modularity:
	@echo "$(GREEN)Running surface modularity checks...$(NC)"
	@scripts/check_surface_modularity.sh

# Run clippy linter
lint:
	@echo "$(GREEN)Running clippy...$(NC)"
	@scripts/run-build-backend-lane lint

# Run clippy across key feature combinations (not just --all-features)
lint-feature-matrix:
	@echo "$(GREEN)Running clippy feature matrix...$(NC)"
	$(CARGO) clippy -p meerkat-tools --no-default-features --features comms,mcp
	$(CARGO) clippy -p meerkat --no-default-features --features openai,memory-store
	$(CARGO) clippy -p meerkat --features all-providers,comms,mcp
	$(CARGO) clippy -p rkat --no-default-features --features session-store,mcp
	$(CARGO) clippy -p meerkat-rpc --no-default-features

# Check formatting
fmt-check:
	@echo "$(GREEN)Checking formatting...$(NC)"
	$(CARGO) fmt --all -- --check

# Fix formatting
fmt:
	@echo "$(GREEN)Fixing formatting...$(NC)"
	$(CARGO) fmt --all

# Security audit using cargo-deny
audit:
	@echo "$(GREEN)Running security audit...$(NC)"
	$(CARGO) deny check

# Alternative audit using cargo-audit (if cargo-deny not available)
audit-alt:
	@echo "$(GREEN)Running cargo-audit...$(NC)"
	$(CARGO) audit

rust-lane-doctor:
	@echo "$(GREEN)Checking Rust lane isolation and test-lane shape...$(NC)"
	@scripts/rust-lane-doctor

cargo-agent-gate: rust-lane-doctor
	@echo "$(GREEN)Running Cargo agent changed-path gate...$(NC)"
	@scripts/cargo-agent-gate $(AGENT_GATE_ARGS)

agent-gate:
	@. ./scripts/build-backend-env; \
	if meerkat_buildbuddy_enabled; then \
		$(MAKE) buildbuddy-doctor; \
	else \
		$(MAKE) rust-lane-doctor; \
	fi
	@echo "$(GREEN)Running agent changed-path gate...$(NC)"
	@scripts/agent-gate $(AGENT_GATE_ARGS)

buildbuddy-install:
	@echo "$(GREEN)Installing pinned BuildBuddy CLI...$(NC)"
	@scripts/install-buildbuddy-cli

# Optional BuildBuddy/remote-cache lanes. Cargo remains the default path.
buildbuddy-generate:
	@echo "$(GREEN)Regenerating optional Bazel BUILD files...$(NC)"
	@node scripts/generate-bazel-rust-builds.mjs

buildbuddy-generate-check:
	@echo "$(GREEN)Checking generated optional Bazel BUILD files...$(NC)"
	@node scripts/generate-bazel-rust-builds.mjs --check

buildbuddy-doctor:
	@echo "$(GREEN)Checking optional BuildBuddy setup...$(NC)"
	@scripts/buildbuddy-doctor

buildbuddy-build:
	@echo "$(GREEN)Building workspace with BuildBuddy...$(NC)"
	@scripts/buildbuddy-dev build $(BUILDBUDDY_ARGS)

buildbuddy-check:
	@echo "$(GREEN)Checking workspace with BuildBuddy...$(NC)"
	@scripts/buildbuddy-dev check $(BUILDBUDDY_ARGS)

buildbuddy-clippy buildbuddy-lint:
	@echo "$(GREEN)Running BuildBuddy clippy...$(NC)"
	@scripts/buildbuddy-dev clippy $(BUILDBUDDY_ARGS)

buildbuddy-test:
	@echo "$(GREEN)Running BuildBuddy fast tests (unit + integration-fast)...$(NC)"
	@scripts/buildbuddy-dev test $(BUILDBUDDY_ARGS)

buildbuddy-test-all:
	@echo "$(GREEN)Running BuildBuddy all-feature fast tests...$(NC)"
	@scripts/buildbuddy-dev test-all $(BUILDBUDDY_ARGS)

buildbuddy-test-unit:
	@echo "$(GREEN)Running BuildBuddy unit tests...$(NC)"
	@scripts/buildbuddy-dev test-unit $(BUILDBUDDY_ARGS)

buildbuddy-test-int:
	@echo "$(GREEN)Running BuildBuddy integration-fast tests...$(NC)"
	@scripts/buildbuddy-dev test-int $(BUILDBUDDY_ARGS)

buildbuddy-e2e-fast:
	@echo "$(GREEN)Running BuildBuddy e2e-fast lane...$(NC)"
	@scripts/buildbuddy-dev e2e-fast $(BUILDBUDDY_ARGS)

buildbuddy-e2e-system:
	@echo "$(GREEN)Running BuildBuddy e2e-system lane...$(NC)"
	@scripts/buildbuddy-dev e2e-system $(BUILDBUDDY_ARGS)

buildbuddy-e2e-live:
	@echo "$(YELLOW)Running BuildBuddy e2e-live lane (requires provider keys)...$(NC)"
	@scripts/buildbuddy-dev e2e-live $(BUILDBUDDY_ARGS)

buildbuddy-e2e-smoke:
	@echo "$(YELLOW)Building BuildBuddy e2e-smoke foundation...$(NC)"
	@TEST="$(TEST)" SCENARIO="$(SCENARIO)" SUITE="$(SUITE)" scripts/buildbuddy-dev e2e-smoke $(BUILDBUDDY_ARGS)

buildbuddy-agent-gate: buildbuddy-doctor
	@echo "$(GREEN)Running BuildBuddy agent changed-path gate...$(NC)"
	@scripts/buildbuddy-agent-gate $(AGENT_GATE_ARGS)

buildbuddy-ci-dispatch: buildbuddy-doctor
	@echo "$(GREEN)Dispatching optional BuildBuddy CI gate...$(NC)"
	@scripts/buildbuddy-ci-dispatch $(BUILDBUDDY_CI_ARGS)

buildbuddy-fast: buildbuddy-doctor
	@echo "$(GREEN)Running BuildBuddy fast workspace test lane...$(NC)"
	@BUILDBUDDY_BAZEL_COMMAND=workspace-fast-rbe scripts/buildbuddy-bazel-poc --jobs=64

buildbuddy-benchmark:
	@echo "$(GREEN)Benchmarking Cargo and BuildBuddy fast lanes...$(NC)"
	@scripts/buildbuddy-benchmark-fast-lanes

buildbuddy-ci: buildbuddy-doctor
	@echo "$(GREEN)Running BuildBuddy workspace CI gate...$(NC)"
	@scripts/buildbuddy-ci-workspace --fresh

buildbuddy-ci-warm: buildbuddy-doctor
	@echo "$(GREEN)Running warmed BuildBuddy workspace CI gate...$(NC)"
	@scripts/buildbuddy-ci-workspace --warm

buildbuddy-ci-full: buildbuddy-doctor
	@echo "$(GREEN)Running full BuildBuddy CI gate...$(NC)"
	@scripts/buildbuddy-ci-full --fresh

buildbuddy-ci-full-warm: buildbuddy-doctor
	@echo "$(GREEN)Running warmed full BuildBuddy CI gate...$(NC)"
	@scripts/buildbuddy-ci-full --warm

# Full CI pipeline - runs the required deterministic lanes plus build policy checks
ci: fmt-check legacy-surface-gate session-control-gate deprecated-backend-gate bridge-no-responsestatus-gate verify-version-parity verify-rpc-surface-alignment verify-sdk-wrapper-freshness check-rust-release-packaging lint lint-feature-matrix test-unit test-int e2e-fast e2e-system test-minimal test-feature-matrix test-surface-modularity seam-inventory rmat-audit audit-generated-headers audit
	@echo "$(GREEN)CI pipeline complete!$(NC)"

# Developer smoke CI pipeline for faster pre-release iteration.
# Keeps core validation, skips full feature matrix clippy/test expansion.
ci-smoke: fmt-check legacy-surface-gate session-control-gate deprecated-backend-gate bridge-no-responsestatus-gate verify-version-parity verify-rpc-surface-alignment verify-sdk-wrapper-freshness check-rust-release-packaging lint test-unit test-int e2e-fast e2e-system test-minimal seam-inventory rmat-audit audit-generated-headers audit
	@echo "$(GREEN)CI smoke pipeline complete!$(NC)"

# Milestone 0 gate: ensure legacy public surface names are either removed
# or explicitly whitelisted during migration.
legacy-surface-gate:
	@echo "$(GREEN)Checking legacy public surface names...$(NC)"
	@scripts/m0_legacy_surface_scan.sh

# Capture or refresh the baseline inventory file used by the M0 gate.
legacy-surface-inventory:
	@echo "$(GREEN)Generating legacy surface inventory baseline...$(NC)"
	@scripts/m0_legacy_surface_scan.sh --no-fail --output=artifacts/m0_legacy_surface_inventory.txt

session-control-gate:
	@echo "$(GREEN)Checking retired session-control public names...$(NC)"
	@scripts/session_control_public_name_scan.sh

deprecated-backend-gate:
	@echo "$(GREEN)Checking for deprecated backend references...$(NC)"
	@scripts/deprecated_backend_scan.sh

# W2-F bridge-classifier gate: supervisor_bridge / local_bridge / bridge
# wire types must not re-interpret `ResponseStatus`. All terminal-vs-progress
# decisions go through `meerkat_core::interaction::classify_response_terminality`
# so the canonical classifier stays the single source of truth.
bridge-no-responsestatus-gate:
	@echo "$(GREEN)Running W2-F bridge-classifier gate...$(NC)"
	@scripts/pre-push-bridge-no-responsestatus.sh

deprecated-backend-inventory:
	@echo "$(GREEN)Generating deprecated backend inventory...$(NC)"
	@scripts/deprecated_backend_scan.sh --no-fail --output=artifacts/deprecated_backend_scan.txt

# Quick check - compile without producing output
check:
	@echo "$(GREEN)Running workspace check...$(NC)"
	@scripts/run-build-backend-lane check

# Build xtask in an isolated target dir so machine-authority commands do not
# block behind unrelated workspace cargo activity.
xtask-build:
	@echo "$(GREEN)Building xtask in $(XTASK_TARGET_DIR)...$(NC)"
	CARGO_TARGET_DIR="$(XTASK_TARGET_DIR)" $(CARGO) build -p xtask --features machine-authority

# Generate all machine/composition authority artifacts.
machine-codegen: xtask-build
	@echo "$(GREEN)Running machine-codegen...$(NC)"
	$(XTASK_BIN) machine-codegen --all

# Verify all machine/composition authority artifacts.
machine-verify: xtask-build
	@echo "$(GREEN)Running machine-verify...$(NC)"
	$(XTASK_BIN) machine-verify --all

# Check generated machine/composition authority artifacts for drift.
machine-check-drift: xtask-build
	@echo "$(GREEN)Running machine-check-drift...$(NC)"
	$(XTASK_BIN) machine-check-drift --all

# RMAT structural seam audit: protocol coverage, feedback constraints,
# terminal mapping, ownership-ledger drift, and heuristic authority hygiene checks.
rmat-audit:
	@echo "$(GREEN)Running RMAT structural seam audit...$(NC)"
	$(CARGO) run -p xtask -- ownership-ledger --check-drift
	$(CARGO) run -p xtask -- rmat-audit --strict

# Seam inventory (strict): every Local/External effect must have an
# explicit classification and every routed effect must resolve via
# the typed Route table in the composition schema.
seam-inventory:
	@echo "$(GREEN)Running seam inventory (strict)...$(NC)"
	$(CARGO) run -p xtask -- seam-inventory --strict

# Strict `@generated` header truthfulness audit: every `@generated` marker
# must correspond to a codegen-emit path, and every codegen-emit path must
# carry the marker. No exceptions, no allowlist.
audit-generated-headers:
	@echo "$(GREEN)Running audit-generated-headers...$(NC)"
	$(CARGO) run -p xtask -- audit-generated-headers

# Generate documentation
doc:
	@echo "$(GREEN)Generating documentation...$(NC)"
	$(CARGO) doc --workspace --no-deps --all-features

# Open documentation in browser
doc-open:
	@echo "$(GREEN)Opening documentation...$(NC)"
	$(CARGO) doc --workspace --no-deps --all-features --open

# Test coverage using cargo-tarpaulin
coverage:
	@echo "$(GREEN)Generating test coverage...$(NC)"
	$(CARGO) tarpaulin --workspace --all-features --timeout 120 --out Html

# Clean build artifacts
clean:
	@echo "$(YELLOW)Cleaning build artifacts...$(NC)"
	$(CARGO) clean

# Install pre-commit hooks
install-hooks:
	@echo "$(GREEN)Installing pre-commit hooks...$(NC)"
	pre-commit install
	pre-commit install --hook-type pre-push
	@echo "$(GREEN)Hooks installed successfully!$(NC)"

# Uninstall pre-commit hooks
uninstall-hooks:
	@echo "$(YELLOW)Uninstalling pre-commit hooks...$(NC)"
	pre-commit uninstall
	pre-commit uninstall --hook-type pre-push

# Run pre-commit on all files (useful for testing hooks)
pre-commit-all:
	@echo "$(GREEN)Running pre-commit on all files...$(NC)"
	pre-commit run --all-files

# Update dependencies
update:
	@echo "$(GREEN)Updating dependencies...$(NC)"
	$(CARGO) update

# Show outdated dependencies
outdated:
	@echo "$(GREEN)Checking for outdated dependencies...$(NC)"
	$(CARGO) outdated

# Run benchmarks (if you have benches/)
bench:
	@echo "$(GREEN)Running benchmarks...$(NC)"
	$(CARGO) bench --workspace

# ── Version parity & release targets ────────────────────────────────────────

# Hard gate: Rust workspace, Python SDK, TypeScript SDK, and contract versions must match
verify-version-parity:
	@scripts/verify-version-parity.sh

# Verify committed schema artifacts match freshly emitted ones
verify-schema-freshness:
	@scripts/verify-schema-freshness.sh

# Verify router/catalog/docs method discoverability alignment
verify-rpc-surface-alignment:
	@scripts/verify-rpc-surface-alignment.sh

# Verify both SDK source trees cover canonical app-facing RPC wrappers
verify-sdk-wrapper-freshness:
	@scripts/verify-sdk-wrapper-freshness.sh

# Verify the publishable Rust workspace surface matches the release list and
# every released crate packages cleanly before we ever talk to crates.io.
check-rust-release-packaging:
	@scripts/check-rust-release-packaging.sh

check-mini-skill-size:
	@scripts/check-mini-skill-size.sh

# Bump Python + TypeScript SDK versions to match Cargo workspace version
bump-sdk-versions:
	@scripts/bump-sdk-versions.sh

# Re-emit schemas and regenerate SDK types from Rust source of truth
regen-schemas:
	@echo "$(GREEN)Emitting schemas...$(NC)"
	$(CARGO) run -p meerkat-contracts --features schema --bin emit-schemas
	@echo "$(GREEN)Running SDK codegen...$(NC)"
	$(PYTHON) tools/sdk-codegen/generate.py
	@echo "$(GREEN)Schemas and SDK types regenerated$(NC)"

# Full pre-release checklist
release-preflight: ci verify-schema-freshness check-rust-release-packaging check-mini-skill-size
	@echo ""
	@echo "$(GREEN)Pre-release checklist:$(NC)"
	@echo "  1. CHANGELOG.md [Unreleased] section populated?"
	@grep -q '## \[Unreleased\]' CHANGELOG.md && echo "     [Unreleased] section exists" || echo "     $(RED)WARNING: no [Unreleased] section$(NC)"
	@if git diff --quiet CHANGELOG.md 2>/dev/null; then \
		echo "  $(YELLOW)   WARNING: CHANGELOG.md has no uncommitted changes$(NC)"; \
	else \
		echo "     CHANGELOG.md has pending changes"; \
	fi
	@echo "  2. All CI checks passed (above)"
	@echo "  3. Schema artifacts are fresh (above)"
	@echo ""
	@echo "$(GREEN)Ready to release. Run:$(NC)"
	@echo "  $(CARGO) release <patch|minor|major>"

# Smoke pre-release checklist.
# Useful for local iteration; skips full feature-matrix expansion.
release-preflight-smoke: ci-smoke verify-schema-freshness check-rust-release-packaging check-mini-skill-size
	@echo ""
	@echo "$(GREEN)Pre-release checklist (smoke):$(NC)"
	@echo "  1. CHANGELOG.md [Unreleased] section populated?"
	@grep -q '## \[Unreleased\]' CHANGELOG.md && echo "     [Unreleased] section exists" || echo "     $(RED)WARNING: no [Unreleased] section$(NC)"
	@if git diff --quiet CHANGELOG.md 2>/dev/null; then \
		echo "  $(YELLOW)   WARNING: CHANGELOG.md has no uncommitted changes$(NC)"; \
	else \
		echo "     CHANGELOG.md has pending changes"; \
	fi
	@echo "  2. Core preflight checks passed (smoke)"
	@echo "  3. Schema artifacts are fresh (above)"
	@echo ""
	@echo "$(GREEN)Ready for smoke dry-run. Use release-preflight for full checks.$(NC)"

# Dry-run publish for Python SDK (build + twine check only)
publish-dry-run-python:
	@echo "$(GREEN)Checking Python SDK publish readiness...$(NC)"
	@(cd sdks/python && \
		$(PYTHON) -m pip install --upgrade build twine && \
		rm -rf dist *.egg-info && \
		$(PYTHON) -m build && \
		$(PYTHON) -m twine check dist/* && \
		rm -rf dist *.egg-info build)

# Dry-run publish for TypeScript SDK (npm --dry-run)
publish-dry-run-typescript:
	@echo "$(GREEN)Checking TypeScript SDK publish readiness...$(NC)"
	@(cd sdks/typescript && \
		npm install --ignore-scripts && \
		npm run build && \
		npm publish --access public --dry-run && \
		rm -rf dist)

# Dry-run publish for Web SDK (npm --dry-run)
publish-dry-run-web:
	@echo "$(GREEN)Checking Web SDK publish readiness...$(NC)"
	@(cd sdks/web && \
		npm install --ignore-scripts && \
		npm run build:ts && \
		npm publish --access public --dry-run && \
		rm -rf dist)

smoke-sdk-python-artifact:
	@echo "$(GREEN)Running Python SDK artifact smoke test...$(NC)"
	@(cd sdks/python && \
		$(PYTHON) -m pip install --upgrade build twine && \
		rm -rf dist *.egg-info build && \
		$(PYTHON) -m build && \
		$(PYTHON) -m twine check dist/* && \
		VENV_DIR=$$(mktemp -d) && \
		$(PYTHON) -m venv "$$VENV_DIR" && \
		"$$VENV_DIR/bin/python" -m pip install --upgrade pip && \
		"$$VENV_DIR/bin/python" -m pip install dist/*.whl && \
		"$$VENV_DIR/bin/python" -c "from meerkat import CONTRACT_VERSION, MeerkatClient; print(CONTRACT_VERSION); print(MeerkatClient.__name__)" && \
		rm -rf "$$VENV_DIR")

smoke-sdk-typescript-artifact:
	@echo "$(GREEN)Running TypeScript SDK artifact smoke test...$(NC)"
	@(cd sdks/typescript && \
		npm install --ignore-scripts && \
		npm run build && \
		PACKFILE=$$(npm pack | tail -n 1) && \
		SMOKE_DIR=$$(mktemp -d) && \
		cd "$$SMOKE_DIR" && \
		npm init -y >/dev/null 2>&1 && \
		npm install "$$OLDPWD/$$PACKFILE" >/dev/null 2>&1 && \
		node --input-type=module -e "const sdk = await import('@rkat/sdk'); if (!sdk.MeerkatClient || !sdk.CONTRACT_VERSION) throw new Error('missing expected exports');" && \
		rm -rf "$$SMOKE_DIR" "$$OLDPWD/$$PACKFILE")

# Full dry-run release path: all validation + dry-run publish checks (no actual uploads)
release-dry-run: release-preflight
	@echo "$(GREEN)Running full registry dry-run (no uploads)...$(NC)"
	@$(MAKE) publish-dry-run
	@$(MAKE) publish-dry-run-python
	@$(MAKE) publish-dry-run-typescript
	@$(MAKE) smoke-sdk-python-artifact
	@$(MAKE) smoke-sdk-typescript-artifact
	@$(MAKE) publish-dry-run-web

# Smoke dry-run path for local iteration.
release-dry-run-smoke: release-preflight-smoke
	@echo "$(GREEN)Running smoke registry dry-run (no uploads)...$(NC)"
	@$(MAKE) publish-dry-run
	@$(MAKE) publish-dry-run-python
	@$(MAKE) publish-dry-run-typescript
	@$(MAKE) smoke-sdk-python-artifact
	@$(MAKE) smoke-sdk-typescript-artifact
	@$(MAKE) publish-dry-run-web

# Dry-run cargo publish for all publishable crates
publish-dry-run:
	@echo "$(GREEN)Checking publish readiness...$(NC)"; \
	MEERKAT_PUBLISH_DRY_RUN_JOBS=$${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4} scripts/publish-dry-run.sh

# Verify version matches tag (for release validation)
verify-version:
	@echo "$(GREEN)Verifying version...$(NC)"
	@VERSION=$$($(CARGO) metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "meerkat") | .version'); \
	TAG=$$(git describe --tags --exact-match 2>/dev/null | sed 's/^v//'); \
	if [ -z "$$TAG" ]; then \
		echo "$(YELLOW)No tag found on current commit$(NC)"; \
	elif [ "$$VERSION" != "$$TAG" ]; then \
		echo "$(RED)Version mismatch: Cargo.toml has $$VERSION but tag is $$TAG$(NC)"; \
		exit 1; \
	else \
		echo "$(GREEN)Version $$VERSION matches tag$(NC)"; \
	fi

# Help target
help:
	@echo "Available targets:"
	@echo "  $(YELLOW)Cargo is the default backend. Set MEERKAT_BUILDBUDDY=1 to route supported local lanes through BuildBuddy.$(NC)"
	@echo "  $(GREEN)build$(NC)         - Build the project (debug)"
	@echo "  $(GREEN)install-build-deps$(NC) - Install the pinned Rust toolchain for local builds"
	@echo "  $(GREEN)release$(NC)       - Build optimized release version"
	@echo "  $(GREEN)test$(NC)          - Run fast tests (unit + integration-fast)"
	@echo "  $(GREEN)test-unit$(NC)     - Run unit tests only"
	@echo "  $(GREEN)test-int$(NC)      - Run integration-fast tests only"
	@echo "  $(GREEN)e2e-fast$(NC)      - Run deterministic end-to-end lane"
	@echo "  $(GREEN)e2e-build$(NC)     - Run build-composition e2e lane (ignored)"
	@echo "  $(GREEN)e2e-system$(NC)    - Run real-binary / real-local-resource lane"
	@echo "  $(GREEN)e2e-live$(NC)      - Run targeted live-provider lane (ignored)"
	@echo "  $(GREEN)e2e-smoke$(NC)     - Run kitchen-sink live smoke lane (ignored)"
	@echo "                  Select with TEST=..., SCENARIO=..., or SUITE=...; set MEERKAT_E2E_EXECUTION_MODE=prebuilt for materialized artifacts"
	@echo "  $(GREEN)test-int-real$(NC) - Legacy alias for e2e-system"
	@echo "  $(GREEN)test-e2e$(NC)      - Legacy alias for e2e-live + e2e-smoke"
	@echo "  $(GREEN)test-all$(NC)      - Run all-feature fast suite"
	@echo "  $(GREEN)test-sdk-python$(NC)- Run Python SDK test suite"
	@echo "  $(GREEN)test-sdk-typescript$(NC)- Run TypeScript SDK test suite"
	@echo "  $(GREEN)test-sdk-suites$(NC)- Run both SDK suites"
	@echo "  $(GREEN)test-surface-modularity$(NC) - Minimal surface build + binary smoke checks"
	@echo "  $(GREEN)lint$(NC)          - Run clippy linter"
	@echo "  $(GREEN)lint-feature-matrix$(NC)- Run clippy across key feature combinations"
	@echo "  $(GREEN)fmt$(NC)           - Fix code formatting"
	@echo "  $(GREEN)fmt-check$(NC)     - Check code formatting"
	@echo "  $(GREEN)audit$(NC)         - Run security audit (cargo-deny)"
	@echo "  $(GREEN)rust-lane-doctor$(NC)- Check Rust lane isolation and filtered test lanes"
	@echo "  $(GREEN)agent-gate$(NC)    - Run Cargo or MEERKAT_BUILDBUDDY=1 BuildBuddy changed gate (AGENT_GATE_ARGS=...)"
	@echo "  $(GREEN)cargo-agent-gate$(NC)- Run Cargo gate for changed agent files"
	@echo "  $(GREEN)buildbuddy-install$(NC)- Install pinned optional BuildBuddy CLI"
	@echo "  $(GREEN)buildbuddy-generate$(NC)- Regenerate optional Bazel BUILD files"
	@echo "  $(GREEN)buildbuddy-generate-check$(NC)- Check optional Bazel BUILD freshness"
	@echo "  $(GREEN)buildbuddy-doctor$(NC)- Check optional BuildBuddy setup"
	@echo "  $(GREEN)buildbuddy-build$(NC)- Build workspace with BuildBuddy"
	@echo "  $(GREEN)buildbuddy-check$(NC)- Check/build workspace with BuildBuddy"
	@echo "  $(GREEN)buildbuddy-clippy$(NC)- Run BuildBuddy clippy"
	@echo "  $(GREEN)buildbuddy-test$(NC)- Run BuildBuddy unit + integration-fast lanes"
	@echo "  $(GREEN)buildbuddy-test-unit$(NC)- Run BuildBuddy unit tests"
	@echo "  $(GREEN)buildbuddy-test-int$(NC)- Run BuildBuddy integration-fast tests"
	@echo "  $(GREEN)buildbuddy-e2e-fast$(NC)- Run BuildBuddy e2e-fast lane"
	@echo "  $(GREEN)buildbuddy-e2e-system$(NC)- Run BuildBuddy e2e-system lane"
	@echo "  $(GREEN)buildbuddy-e2e-live$(NC)- Run BuildBuddy e2e-live lane (requires provider keys)"
	@echo "  $(GREEN)buildbuddy-e2e-smoke$(NC)- Build BuildBuddy e2e-smoke foundation"
	@echo "  $(GREEN)buildbuddy-agent-gate$(NC)- Run BuildBuddy gate for changed agent files (AGENT_GATE_ARGS=...)"
	@echo "  $(GREEN)buildbuddy-ci-dispatch$(NC)- Dispatch optional BuildBuddy CI mode (BUILDBUDDY_CI_ARGS=...)"
	@echo "  $(GREEN)buildbuddy-fast$(NC)- Run optional BuildBuddy fast test lane"
	@echo "  $(GREEN)buildbuddy-benchmark$(NC)- Compare Cargo and BuildBuddy fast lanes"
	@echo "  $(GREEN)buildbuddy-ci$(NC) - Run optional BuildBuddy workspace CI gate"
	@echo "  $(GREEN)buildbuddy-ci-warm$(NC) - Run warmed BuildBuddy workspace CI gate"
	@echo "  $(GREEN)buildbuddy-ci-full$(NC) - Run full optional BuildBuddy CI gate"
	@echo "  $(GREEN)buildbuddy-ci-full-warm$(NC) - Run warmed full BuildBuddy CI gate"
	@echo "  $(GREEN)ci$(NC)            - Run full CI pipeline"
	@echo "  $(GREEN)check$(NC)         - Quick compilation check"
	@echo "  $(GREEN)doc$(NC)           - Generate documentation"
	@echo "  $(GREEN)coverage$(NC)      - Generate test coverage report"
	@echo "  $(GREEN)clean$(NC)         - Remove build artifacts"
	@echo "  $(GREEN)install-hooks$(NC) - Install git hooks"
	@echo "  $(GREEN)ci-smoke$(NC)       - Run CI smoke pipeline (no full feature matrices)"
	@echo "  $(GREEN)machine-verify$(NC)- Verify machine/composition authority artifacts"
	@echo "  $(GREEN)machine-check-drift$(NC)- Check generated authority artifacts for drift"
	@echo "  $(GREEN)rmat-audit$(NC)    - Run RMAT + ownership-ledger audit gates (strict mode)"
	@echo "  $(GREEN)verify-version$(NC)- Verify Cargo.toml version matches git tag"
	@echo ""
	@echo "Release targets:"
	@echo "  $(GREEN)verify-version-parity$(NC) - Check Rust/Python/TS version + contract parity"
	@echo "  $(GREEN)verify-schema-freshness$(NC)- Check committed schemas match Rust source"
	@echo "  $(GREEN)verify-rpc-surface-alignment$(NC)- Check router/catalog/docs method parity"
	@echo "  $(GREEN)verify-sdk-wrapper-freshness$(NC)- Check SDK wrapper coverage for catalog methods"
	@echo "  $(GREEN)check-rust-release-packaging$(NC)- Verify release Rust crates package cleanly"
	@echo "  $(GREEN)bump-sdk-versions$(NC)     - Bump Python + TS versions to match Cargo"
	@echo "  $(GREEN)regen-schemas$(NC)         - Re-emit schemas + run SDK codegen"
	@echo "  $(GREEN)release-preflight$(NC)     - Full pre-release checklist (CI + freshness)"
	@echo "  $(GREEN)release-preflight-smoke$(NC)- Smoke pre-release checklist"
	@echo "  $(GREEN)publish-dry-run$(NC)       - Dry-run cargo publish for all crates"
	@echo "  $(GREEN)publish-dry-run-python$(NC) - Dry-run Python SDK publish check (build + twine check)"
	@echo "  $(GREEN)publish-dry-run-typescript$(NC)- Dry-run TypeScript SDK publish check (npm publish --dry-run)"
	@echo "  $(GREEN)smoke-sdk-python-artifact$(NC)- Build + install Python wheel smoke test"
	@echo "  $(GREEN)smoke-sdk-typescript-artifact$(NC)- Pack + install npm tarball smoke test"
	@echo "  $(GREEN)release-dry-run$(NC)       - Full preflight + dry-run registry checks (no uploads)"
	@echo "  $(GREEN)release-dry-run-smoke$(NC)  - Smoke preflight + registry dry-run checks (no uploads)"
	@echo "  $(GREEN)help$(NC)          - Show this help message"
