# Meerkat CI/CD Makefile
# Single source of truth for all build, test, and lint commands

CRATE_NAME := meerkat
FAST_TARGET_DIR ?= target/fast

# Colors for terminal output
GREEN := \033[0;32m
YELLOW := \033[0;33m
RED := \033[0;31m
NC := \033[0m

.PHONY: all build test test-unit test-int test-int-real test-e2e test-all test-minimal test-feature-matrix-lib test-feature-matrix-surface test-feature-matrix test-surface-modularity lint lint-feature-matrix fmt fmt-check audit ci clean doc release install-hooks coverage check help legacy-surface-gate legacy-surface-inventory verify-version-parity verify-schema-freshness bump-sdk-versions release-preflight publish-dry-run

# Default target
all: ci

# Build the project
build:
	@echo "$(GREEN)Building $(CRATE_NAME)...$(NC)"
	cargo build --workspace

# Build release version
release:
	@echo "$(GREEN)Building release version...$(NC)"
	cargo build --workspace --release

# Fast test suite (for commit/push hooks)
# Unit + integration-fast, skips doctests and ignored tests
test:
	@echo "$(GREEN)Running fast tests (unit + integration-fast)...$(NC)"
	cargo test --target-dir $(FAST_TARGET_DIR) --workspace --lib --bins --tests

# Unit tests only
test-unit:
	@echo "$(GREEN)Running unit tests...$(NC)"
	cargo test --target-dir $(FAST_TARGET_DIR) --workspace --lib --bins

# Integration-fast tests only (no unit tests)
test-int:
	@echo "$(GREEN)Running integration-fast tests...$(NC)"
	cargo test --target-dir $(FAST_TARGET_DIR) --workspace --tests

# Integration-real tests (ignored by default)
test-int-real:
	@echo "$(YELLOW)Running integration-real tests (ignored by default)...$(NC)"
	cargo test --workspace integration_real -- --ignored --test-threads=1

# End-to-end tests (ignored by default)
test-e2e:
	@echo "$(YELLOW)Running e2e tests (ignored by default)...$(NC)"
	cargo test --workspace e2e_ -- --ignored --test-threads=1

# Full test suite (for CI)
# Includes all tests with all features
test-all:
	@echo "$(GREEN)Running full test suite...$(NC)"
	cargo test --workspace --all-features --all-targets

# Minimal builds without optional features
test-minimal:
	@echo "$(GREEN)Running minimal build checks...$(NC)"
	cargo check -p meerkat-core
	cargo check -p meerkat-client --no-default-features
	cargo check -p meerkat-store --no-default-features
	cargo check -p meerkat-tools --no-default-features
	cargo check -p meerkat --no-default-features
	cargo test -p meerkat-core --lib --bins --tests

# Library crate feature combinations
test-feature-matrix-lib:
	@echo "$(GREEN)Running library feature matrix checks...$(NC)"
	cargo check -p meerkat-tools --no-default-features --features sub-agents
	cargo check -p meerkat-tools --no-default-features --features comms
	cargo check -p meerkat-tools --no-default-features --features mcp
	cargo check -p meerkat-tools --no-default-features --features sub-agents,comms
	cargo check -p meerkat-tools --no-default-features --features comms,mcp
	cargo check -p meerkat --no-default-features --features openai,memory-store
	cargo check -p meerkat --no-default-features --features gemini,jsonl-store
	cargo check -p meerkat --features all-providers,comms,mcp,sub-agents
	cargo test -p meerkat --features all-providers,comms,mcp --lib --bins --tests

# Surface crate feature combinations
test-feature-matrix-surface:
	@echo "$(GREEN)Running surface feature matrix checks...$(NC)"
	cargo check -p meerkat-rpc --no-default-features
	cargo check -p meerkat-rpc --no-default-features --features comms,mcp
	cargo check -p meerkat-rest --no-default-features
	cargo check -p meerkat-rest --no-default-features --features comms
	cargo check -p meerkat-mcp-server --no-default-features
	cargo check -p meerkat-mcp-server --no-default-features --features comms
	cargo check -p rkat --no-default-features --features session-store
	cargo check -p rkat --no-default-features --features session-store,mcp
	cargo test -p rkat --no-default-features --features session-store,mcp -- --nocapture
	cargo check -p rkat --no-default-features --features session-store,comms,mcp

# Session capability matrix (A-F builds from spec)
test-session-matrix:
	@echo "$(GREEN)Running session capability matrix...$(NC)"
	@echo "  Build A: no-default-features (ephemeral only)"
	cargo check -p meerkat-session --no-default-features
	cargo test -p meerkat-session --no-default-features --lib --tests
	@echo "  Build B: session-store"
	cargo check -p meerkat-session --no-default-features --features session-store
	@echo "  Build C: (memory-store — Phase 6)"
	@echo "  Build D: session-compaction"
	cargo check -p meerkat-session --no-default-features --features session-compaction
	@echo "  Build E: session-store (Phase 6 combo)"
	@echo "  Build F: all session features"
	cargo check -p meerkat-session --no-default-features --features session-store,session-compaction

# Full feature matrix
test-feature-matrix: test-feature-matrix-lib test-feature-matrix-surface

# Surface modularity guardrail: minimal feature builds + binary smoke checks.
test-surface-modularity:
	@echo "$(GREEN)Running surface modularity checks...$(NC)"
	@scripts/check_surface_modularity.sh

# Run clippy linter
lint:
	@echo "$(GREEN)Running clippy...$(NC)"
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run clippy across key feature combinations (not just --all-features)
lint-feature-matrix:
	@echo "$(GREEN)Running clippy feature matrix...$(NC)"
	cargo clippy -p meerkat-tools --no-default-features --features sub-agents
	cargo clippy -p meerkat-tools --no-default-features --features comms,mcp
	cargo clippy -p meerkat --no-default-features --features openai,memory-store
	cargo clippy -p meerkat --features all-providers,comms,mcp,sub-agents
	cargo clippy -p rkat --no-default-features --features session-store,mcp
	cargo clippy -p meerkat-rpc --no-default-features

# Check formatting
fmt-check:
	@echo "$(GREEN)Checking formatting...$(NC)"
	cargo fmt --all -- --check

# Fix formatting
fmt:
	@echo "$(GREEN)Fixing formatting...$(NC)"
	cargo fmt --all

# Security audit using cargo-deny
audit:
	@echo "$(GREEN)Running security audit...$(NC)"
	cargo deny check

# Alternative audit using cargo-audit (if cargo-deny not available)
audit-alt:
	@echo "$(GREEN)Running cargo-audit...$(NC)"
	cargo audit

# Full CI pipeline - runs everything
ci: fmt-check legacy-surface-gate verify-version-parity lint lint-feature-matrix test-all test-minimal test-feature-matrix test-surface-modularity audit
	@echo "$(GREEN)CI pipeline complete!$(NC)"

# Milestone 0 gate: ensure legacy public surface names are either removed
# or explicitly whitelisted during migration.
legacy-surface-gate:
	@echo "$(GREEN)Checking legacy public surface names...$(NC)"
	@scripts/m0_legacy_surface_scan.sh

# Capture or refresh the baseline inventory file used by the M0 gate.
legacy-surface-inventory:
	@echo "$(GREEN)Generating legacy surface inventory baseline...$(NC)"
	@scripts/m0_legacy_surface_scan.sh --no-fail --output=artifacts/m0_legacy_surface_inventory.txt

# Quick check - compile without producing output
check:
	@echo "$(GREEN)Running cargo check...$(NC)"
	cargo check --workspace --all-targets --all-features

# Generate documentation
doc:
	@echo "$(GREEN)Generating documentation...$(NC)"
	cargo doc --workspace --no-deps --all-features

# Open documentation in browser
doc-open:
	@echo "$(GREEN)Opening documentation...$(NC)"
	cargo doc --workspace --no-deps --all-features --open

# Test coverage using cargo-tarpaulin
coverage:
	@echo "$(GREEN)Generating test coverage...$(NC)"
	cargo tarpaulin --workspace --all-features --timeout 120 --out Html

# Clean build artifacts
clean:
	@echo "$(YELLOW)Cleaning build artifacts...$(NC)"
	cargo clean

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
	cargo update

# Show outdated dependencies
outdated:
	@echo "$(GREEN)Checking for outdated dependencies...$(NC)"
	cargo outdated

# Run benchmarks (if you have benches/)
bench:
	@echo "$(GREEN)Running benchmarks...$(NC)"
	cargo bench --workspace

# ── Version parity & release targets ────────────────────────────────────────

# Hard gate: Rust workspace, Python SDK, TypeScript SDK, and contract versions must match
verify-version-parity:
	@scripts/verify-version-parity.sh

# Verify committed schema artifacts match freshly emitted ones
verify-schema-freshness:
	@scripts/verify-schema-freshness.sh

# Bump Python + TypeScript SDK versions to match Cargo workspace version
bump-sdk-versions:
	@scripts/bump-sdk-versions.sh

# Re-emit schemas and regenerate SDK types from Rust source of truth
regen-schemas:
	@echo "$(GREEN)Emitting schemas...$(NC)"
	cargo run -p meerkat-contracts --features schema --bin emit-schemas
	@echo "$(GREEN)Running SDK codegen...$(NC)"
	python3 tools/sdk-codegen/generate.py
	@echo "$(GREEN)Schemas and SDK types regenerated$(NC)"

# Full pre-release checklist
release-preflight: ci verify-schema-freshness
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
	@echo "  cargo release <patch|minor|major>"

# Dry-run cargo publish for all publishable crates
publish-dry-run:
	@echo "$(GREEN)Checking publish readiness...$(NC)"
	@FAIL=0; \
	for pkg in meerkat-core meerkat-contracts meerkat-client meerkat-store \
	           meerkat-tools meerkat-session meerkat-memory meerkat-mcp \
	           meerkat-mcp-server meerkat-hooks meerkat-skills meerkat-comms \
	           meerkat-rpc meerkat-rest meerkat; do \
		printf "  %-25s" "$$pkg..."; \
		if cargo publish -p $$pkg --dry-run 2>&1 | grep -q "error"; then \
			echo "$(RED)FAIL$(NC)"; \
			FAIL=1; \
		else \
			echo "$(GREEN)OK$(NC)"; \
		fi; \
	done; \
	if [ $$FAIL -ne 0 ]; then \
		echo "$(RED)Some crates are not publish-ready$(NC)"; \
		exit 1; \
	fi
	@echo "$(GREEN)All crates are publish-ready$(NC)"

# Verify version matches tag (for release validation)
verify-version:
	@echo "$(GREEN)Verifying version...$(NC)"
	@VERSION=$$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "meerkat") | .version'); \
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
	@echo "  $(GREEN)build$(NC)         - Build the project (debug)"
	@echo "  $(GREEN)release$(NC)       - Build optimized release version"
	@echo "  $(GREEN)test$(NC)          - Run fast tests (unit + integration-fast)"
	@echo "  $(GREEN)test-unit$(NC)     - Run unit tests only"
	@echo "  $(GREEN)test-int$(NC)      - Run integration-fast tests only"
	@echo "  $(GREEN)test-int-real$(NC) - Run integration-real tests (ignored)"
	@echo "  $(GREEN)test-e2e$(NC)      - Run e2e tests (ignored)"
	@echo "  $(GREEN)test-all$(NC)      - Run full test suite (CI)"
	@echo "  $(GREEN)test-surface-modularity$(NC) - Minimal surface build + binary smoke checks"
	@echo "  $(GREEN)lint$(NC)          - Run clippy linter"
	@echo "  $(GREEN)lint-feature-matrix$(NC)- Run clippy across key feature combinations"
	@echo "  $(GREEN)fmt$(NC)           - Fix code formatting"
	@echo "  $(GREEN)fmt-check$(NC)     - Check code formatting"
	@echo "  $(GREEN)audit$(NC)         - Run security audit (cargo-deny)"
	@echo "  $(GREEN)ci$(NC)            - Run full CI pipeline"
	@echo "  $(GREEN)check$(NC)         - Quick compilation check"
	@echo "  $(GREEN)doc$(NC)           - Generate documentation"
	@echo "  $(GREEN)coverage$(NC)      - Generate test coverage report"
	@echo "  $(GREEN)clean$(NC)         - Remove build artifacts"
	@echo "  $(GREEN)install-hooks$(NC) - Install git hooks"
	@echo "  $(GREEN)verify-version$(NC)- Verify Cargo.toml version matches git tag"
	@echo ""
	@echo "Release targets:"
	@echo "  $(GREEN)verify-version-parity$(NC) - Check Rust/Python/TS version + contract parity"
	@echo "  $(GREEN)verify-schema-freshness$(NC)- Check committed schemas match Rust source"
	@echo "  $(GREEN)bump-sdk-versions$(NC)     - Bump Python + TS versions to match Cargo"
	@echo "  $(GREEN)regen-schemas$(NC)         - Re-emit schemas + run SDK codegen"
	@echo "  $(GREEN)release-preflight$(NC)     - Full pre-release checklist (CI + freshness)"
	@echo "  $(GREEN)publish-dry-run$(NC)       - Dry-run cargo publish for all crates"
	@echo "  $(GREEN)help$(NC)          - Show this help message"
