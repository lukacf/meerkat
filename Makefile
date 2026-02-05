# Meerkat CI/CD Makefile
# Single source of truth for all build, test, and lint commands

CRATE_NAME := meerkat

# Colors for terminal output
GREEN := \033[0;32m
YELLOW := \033[0;33m
RED := \033[0;31m
NC := \033[0m

.PHONY: all build test test-unit test-int test-int-real test-e2e test-all lint fmt fmt-check audit ci clean doc release install-hooks coverage check help

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
	cargo test --workspace --lib --bins --tests

# Unit tests only
test-unit:
	@echo "$(GREEN)Running unit tests...$(NC)"
	cargo test --workspace --lib --bins

# Integration-fast tests only (no unit tests)
test-int:
	@echo "$(GREEN)Running integration-fast tests...$(NC)"
	cargo test --workspace --tests

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

# Run clippy linter
lint:
	@echo "$(GREEN)Running clippy...$(NC)"
	cargo clippy --workspace --all-targets --all-features -- -D warnings

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
ci: fmt-check lint test-all audit
	@echo "$(GREEN)CI pipeline complete!$(NC)"

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
	@echo "  $(GREEN)lint$(NC)          - Run clippy linter"
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
	@echo "  $(GREEN)help$(NC)          - Show this help message"
