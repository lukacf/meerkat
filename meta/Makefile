# ──────────────────────────────────────────────────────────────
# Meta — single source of truth for build / test / lint
# ──────────────────────────────────────────────────────────────

CARGO  ?= ./scripts/repo-cargo

GREEN  := \033[0;32m
YELLOW := \033[0;33m
RED    := \033[0;31m
NC     := \033[0m

# ── meta ──────────────────────────────────────────────────────

.PHONY: e2e-live all build release test test-python test-flow-editor test-flow-editor-rkat test-flow-editor-rkat-deploy test-all lint fmt fmt-check \
        audit bright-line memory-evals ci ci-smoke check doc doc-open coverage clean \
        install-hooks uninstall-hooks pre-commit-all update outdated \
        verify-version-parity bump-sdk-versions publish-dry-run-python \
        release-preflight release-candidate release-promote release-registry-retry release-assets-recover help

all: ci

# ── build ─────────────────────────────────────────────────────

build: ## Build all workspace crates (debug)
	@echo "$(YELLOW)Building workspace (debug)…$(NC)"
	$(CARGO) build --workspace
	@echo "$(GREEN)Build succeeded.$(NC)"

release: ## Build all workspace crates (release)
	@echo "$(YELLOW)Building workspace (release)…$(NC)"
	$(CARGO) build --workspace --release
	@echo "$(GREEN)Release build succeeded.$(NC)"

# ── test ──────────────────────────────────────────────────────

test: ## Run Rust tests via cargo-nextest
	@echo "$(YELLOW)Running Rust tests…$(NC)"
	$(CARGO) nextest run --workspace --no-fail-fast
	@echo "$(GREEN)Rust tests passed.$(NC)"

test-python: ## Run Python SDK tests
	@echo "$(YELLOW)Running Python SDK tests…$(NC)"
	CARGO_INCREMENTAL=0 $(CARGO) build -p meerkat-meta --bin rpc_gateway --locked
	@gateway_target_dir="$$($(CARGO) --print-env | awk -F= '$$1 == "CARGO_TARGET_DIR" { print substr($$0, index($$0, "=") + 1) }')"; \
		gateway_bin="$$gateway_target_dir/debug/rpc_gateway"; \
		test -x "$$gateway_bin" || { echo "missing freshly built rpc_gateway at $$gateway_bin" >&2; exit 1; }; \
		META_GATEWAY_BIN="$$gateway_bin" PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/ -q
	@echo "$(GREEN)Python SDK tests passed.$(NC)"

e2e-live: ## Real-API end-to-end lane: real rpc_gateway + Python SDK + real Anthropic turns (needs ANTHROPIC_API_KEY)
	@echo "$(YELLOW)Running real-API e2e lane…$(NC)"
	@test -n "$${ANTHROPIC_API_KEY:-$${RKAT_ANTHROPIC_API_KEY:-}}" || { echo "e2e-live: ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY) is not set; refusing to run a lane that would skip" >&2; exit 1; }
	CARGO_INCREMENTAL=0 $(CARGO) build -p meerkat-meta --bin rpc_gateway --locked
	@gateway_target_dir="$$($(CARGO) --print-env | awk -F= '$$1 == "CARGO_TARGET_DIR" { print substr($$0, index($$0, "=") + 1) }')"; \
		gateway_bin="$$gateway_target_dir/debug/rpc_gateway"; \
		test -x "$$gateway_bin" || { echo "missing freshly built rpc_gateway at $$gateway_bin" >&2; exit 1; }; \
		META_E2E_LIVE=1 META_GATEWAY_BIN="$$gateway_bin" PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/e2e_live -q -rA
	@echo "$(GREEN)Real-API e2e lane passed.$(NC)"

test-flow-editor: ## Run Flow Editor source, projection, visual, browser, and embedded freshness contracts
	@echo "$(YELLOW)Running Flow Editor tests…$(NC)"
	CARGO_INCREMENTAL=0 $(CARGO) build -p meerkat-meta --bin meta_flow_editor
	npm --prefix flow-editor run test:controller --silent
	npm --prefix flow-editor run test:visual-contract --silent
	npm --prefix flow-editor run test:browser-source --silent
	npm --prefix flow-editor run test:browser-interactions --silent
	@echo "$(GREEN)Flow Editor tests passed.$(NC)"

test-flow-editor-rkat: ## Run live Flow Editor export/import checks against rkat mob inspect/validate
	@echo "$(YELLOW)Running live Flow Editor rkat tests…$(NC)"
	scripts/run-flow-editor-rkat-e2e.sh
	@echo "$(GREEN)Live Flow Editor rkat tests passed.$(NC)"

test-flow-editor-rkat-deploy: ## Run live Flow Editor rkat tests including rkat mob deploy
	@echo "$(YELLOW)Running live Flow Editor rkat deploy tests…$(NC)"
	scripts/run-flow-editor-rkat-e2e.sh --deploy
	@echo "$(GREEN)Live Flow Editor rkat deploy tests passed.$(NC)"

test-all: test test-python test-flow-editor ## Run all tests (Rust + Python + Flow Editor)

# ── lint / format ─────────────────────────────────────────────

lint: ## Run clippy with warnings-as-errors
	@echo "$(YELLOW)Running clippy…$(NC)"
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	@echo "$(GREEN)Clippy passed.$(NC)"

fmt: ## Format all Rust code
	@echo "$(YELLOW)Formatting code…$(NC)"
	$(CARGO) fmt --all
	@echo "$(GREEN)Formatting complete.$(NC)"

fmt-check: ## Verify Rust formatting (CI)
	@echo "$(YELLOW)Checking formatting…$(NC)"
	$(CARGO) fmt --all -- --check
	@echo "$(GREEN)Formatting OK.$(NC)"

# ── audit / CI ────────────────────────────────────────────────

audit: ## Run cargo-deny licence / advisory checks
	@echo "$(YELLOW)Running cargo deny…$(NC)"
	$(CARGO) deny check
	@echo "$(GREEN)Audit passed.$(NC)"

bright-line: ## Enforce the memory bright line (agent-memory-architecture.md §12)
	@echo "$(YELLOW)Checking memory bright line…$(NC)"
	@scripts/check-memory-bright-line
	@echo "$(GREEN)Memory bright line holds.$(NC)"

memory-evals: ## Validate memory calibration profiles/fixtures + gate mock-lane invariants (agent-memory-architecture.md §11)
	@echo "$(YELLOW)Checking memory calibration harness…$(NC)"
# Build the calibration binaries BEFORE the harness runs them. Each stage
# shells out to `repo-cargo run --bin <stage>-eval` under a fixed 600s
# subprocess budget, and those bins live in the meta crate, so a cold link
# pulls the whole meerkat graph. Without this the budget covers COMPILATION and
# the first run after any dependency bump reports a calibration timeout for a
# build that had not finished, a gate failing for a reason other than the one
# it names. Pre-building costs nothing on a warm tree and makes the 600s
# measure the eval. A build failure still fails this target, with a compiler
# error instead of a misattributed timeout.
	$(CARGO) build --quiet --bins
	@scripts/memory-evals --check
	@scripts/memory-evals --stage distiller --mode mock
	@scripts/memory-evals --stage steward --mode mock
	@scripts/memory-evals --stage hygienist --mode mock
	@echo "$(GREEN)Memory calibration harness OK (mock lanes gate deterministic invariants; judgment misses informational).$(NC)"

ci: fmt-check verify-version-parity bright-line memory-evals lint test-all audit ## Full CI pipeline
	@echo "$(GREEN)CI pipeline passed.$(NC)"

ci-smoke: fmt-check bright-line lint test test-python ## Quick smoke test (no audit / version parity)
	@echo "$(GREEN)CI smoke passed.$(NC)"

# ── misc cargo ────────────────────────────────────────────────

check: ## cargo check (fast compile check)
	@echo "$(YELLOW)Running cargo check…$(NC)"
	$(CARGO) check --workspace --all-targets
	@echo "$(GREEN)Check succeeded.$(NC)"

doc: ## Build rustdoc for all crates
	@echo "$(YELLOW)Building docs…$(NC)"
	$(CARGO) doc --workspace --no-deps
	@echo "$(GREEN)Docs built.$(NC)"

doc-open: ## Build and open rustdoc
	@echo "$(YELLOW)Building and opening docs…$(NC)"
	$(CARGO) doc --workspace --no-deps --open

coverage: ## Generate HTML coverage report (cargo-tarpaulin)
	@echo "$(YELLOW)Generating coverage report…$(NC)"
	$(CARGO) tarpaulin --workspace --timeout 120 --out Html
	@echo "$(GREEN)Coverage report generated.$(NC)"

clean: ## Remove build artefacts
	@echo "$(YELLOW)Cleaning…$(NC)"
	$(CARGO) clean
	@echo "$(GREEN)Clean complete.$(NC)"

# ── git hooks ─────────────────────────────────────────────────

install-hooks: ## Install pre-commit hooks
	@echo "$(YELLOW)Installing hooks…$(NC)"
	pre-commit install && pre-commit install --hook-type pre-push
	@echo "$(GREEN)Hooks installed.$(NC)"

uninstall-hooks: ## Uninstall pre-commit hooks
	@echo "$(YELLOW)Uninstalling hooks…$(NC)"
	pre-commit uninstall && pre-commit uninstall --hook-type pre-push
	@echo "$(GREEN)Hooks uninstalled.$(NC)"

pre-commit-all: ## Run pre-commit on all files
	@echo "$(YELLOW)Running pre-commit on all files…$(NC)"
	pre-commit run --all-files

# ── dependency management ─────────────────────────────────────

update: ## Update Cargo.lock to latest compatible versions
	@echo "$(YELLOW)Updating dependencies…$(NC)"
	$(CARGO) update
	@echo "$(GREEN)Dependencies updated.$(NC)"

outdated: ## Show outdated crates
	@echo "$(YELLOW)Checking for outdated crates…$(NC)"
	$(CARGO) outdated

# ── version / release ─────────────────────────────────────────

verify-version-parity: ## Verify version strings are in sync
	@scripts/verify-version-parity.sh

bump-sdk-versions: ## Bump SDK version strings
	@scripts/bump-sdk-versions.sh

verify-version: ## Verify Cargo.toml version matches git tag
	@VERSION=$$($(CARGO) metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "meerkat-meta") | .version'); \
	TAG=$$(git describe --tags --exact-match 2>/dev/null | sed 's/^v//'); \
	if [ -z "$$TAG" ]; then \
		echo "$(YELLOW)No tag found on current commit$(NC)"; \
	elif [ "$$VERSION" != "$$TAG" ]; then \
		echo "$(RED)Version mismatch: Cargo.toml has $$VERSION but tag is $$TAG$(NC)"; \
		exit 1; \
	else \
		echo "$(GREEN)Version $$VERSION matches tag$(NC)"; \
	fi

publish-dry-run: ## Dry-run cargo publish
	@echo "$(YELLOW)Dry-run cargo publish…$(NC)"
	$(CARGO) publish -p meerkat-meta --dry-run
	@echo "$(GREEN)Cargo dry-run succeeded.$(NC)"

publish-dry-run-python: ## Dry-run Python package build + twine check
	@echo "$(YELLOW)Building Python package (dry run)…$(NC)"
	@cd sdk/python && \
		python3 -m pip install --quiet build twine && \
		rm -rf dist && \
		python3 -m build && \
		python3 -m twine check dist/* && \
		rm -rf dist build *.egg-info
	@echo "$(GREEN)Python dry-run publish succeeded.$(NC)"

publish-dry-run-typescript: ## Dry-run TypeScript SDK build + npm pack
	@echo "$(YELLOW)Building TypeScript SDK (dry run)…$(NC)"
	@cd sdk/typescript && \
		npm install --ignore-scripts && \
		npm run build && \
		npm publish --access public --dry-run && \
		rm -rf dist
	@echo "$(GREEN)TypeScript dry-run publish succeeded.$(NC)"

release-preflight: ci ## Pre-release checks (full CI + CHANGELOG)
	@grep -q '\[Unreleased\]' CHANGELOG.md || \
		(echo "$(RED)CHANGELOG.md missing [Unreleased] section$(NC)" && exit 1)
	@echo "$(GREEN)Release preflight passed — ready to ship.$(NC)"

release-preflight-smoke: ci-smoke ## Smoke pre-release checks
	@grep -q '\[Unreleased\]' CHANGELOG.md || \
		(echo "$(RED)CHANGELOG.md missing [Unreleased] section$(NC)" && exit 1)
	@echo "$(GREEN)Smoke preflight passed.$(NC)"

release-dry-run: release-preflight publish-dry-run publish-dry-run-python publish-dry-run-typescript ## Full dry-run release (no uploads)
	@echo "$(GREEN)Full release dry-run passed.$(NC)"

release-dry-run-smoke: release-preflight-smoke publish-dry-run publish-dry-run-python publish-dry-run-typescript ## Smoke dry-run release
	@echo "$(GREEN)Smoke release dry-run passed.$(NC)"

# Protocol v1: tag pushes validate only; these dispatches never create tags.
release-candidate: export RELEASE_MODE = candidate
release-candidate: export SOURCE_SHA := $(SOURCE_SHA)
release-candidate: export RELEASE_TIER := $(TIER)
release-candidate: export PUBLISH_RELEASE_PACKAGES = false
release-candidate: export REGISTRY_DRY_RUN = false
release-candidate: ## Build one tier's immutable candidate on main (SOURCE_SHA, TIER=core|trailing required); never publish
	@python3 scripts/release-candidate.py dispatch --mode candidate

release-promote: export RELEASE_TAG := $(RELEASE_TAG)
release-promote: export RELEASE_TIER := $(TIER)
release-promote: export ARTIFACT_SELECTION_FILE = $(ARTIFACT_SELECTION)
release-promote: export PUBLISH_RELEASE_PACKAGES ?= true
release-promote: export REGISTRY_DRY_RUN ?= false
release-promote: ## Promote accepted original bytes (RELEASE_TAG, ARTIFACT_SELECTION required; no TIER)
	@python3 scripts/release-candidate.py dispatch --mode promote

release-registry-retry: export RELEASE_TAG := $(RELEASE_TAG)
release-registry-retry: export RELEASE_TIER := $(TIER)
release-registry-retry: export PUBLISH_RELEASE_PACKAGES = true
release-registry-retry: export REGISTRY_DRY_RUN ?= false
release-registry-retry: ## Retry registries only, never touch GitHub assets (RELEASE_TAG required; no TIER)
	@python3 scripts/release-candidate.py dispatch --mode existing

release-assets-recover: export RELEASE_TAG := $(RELEASE_TAG)
release-assets-recover: export RELEASE_TIER := $(TIER)
release-assets-recover: export ARTIFACT_SELECTION_FILE = $(ARTIFACT_SELECTION)
release-assets-recover: export PUBLISH_RELEASE_PACKAGES = false
release-assets-recover: export REGISTRY_DRY_RUN = false
release-assets-recover: ## Add missing original assets only (RELEASE_TAG, ARTIFACT_SELECTION required; no TIER)
	@python3 scripts/release-candidate.py dispatch --mode assets

# ── help ──────────────────────────────────────────────────────

help: ## Show this help
	@echo "$(GREEN)Meta Makefile targets:$(NC)"
	@echo "Release protocol: tag pushes validate only; release-candidate TIER=core|trailing builds one tier; release-promote publishes the accepted core tier; release-assets-recover adds the trailing tier."
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  $(YELLOW)%-24s$(NC) %s\n", $$1, $$2}'
