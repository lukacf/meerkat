CARGO   ?= RUST_LANE_ID=examples-audit-H CARGO_BUILD_JOBS=2 ../../scripts/repo-cargo
PROFILE ?= release
# repo-cargo changes to the repository root before invoking Cargo.
MANIFEST_PATH := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/Cargo.toml)
TARGET_DIR = $(shell $(CARGO) --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/$(PROFILE)

BINS := mdm-tux mdm-target mdm-kennel

.PHONY: all build check lint test sign clean docker-smoke docker-tux docker-tmux docker-live-suite docker-architecture-suite docker-logs docker-down docker-clean

all: build sign

build:
ifeq ($(PROFILE),release)
	$(CARGO) build --manifest-path "$(MANIFEST_PATH)" --locked --release
else
	$(CARGO) build --manifest-path "$(MANIFEST_PATH)" --locked
endif

check:
	$(CARGO) check --manifest-path "$(MANIFEST_PATH)" --locked --all-targets

lint:
	$(CARGO) clippy --manifest-path "$(MANIFEST_PATH)" --locked --all-targets --all-features -- -D warnings

test:
	$(CARGO) test --manifest-path "$(MANIFEST_PATH)" --locked
	python3 scripts/test-scripts.py

sign: build
	@for bin in $(BINS); do \
		echo "codesign $$bin"; \
		codesign --force --sign - $(TARGET_DIR)/$$bin; \
	done

clean:
	$(CARGO) clean --manifest-path "$(MANIFEST_PATH)"

docker-smoke:
	./scripts/docker-smoke.sh smoke

docker-tux:
	./scripts/docker-smoke.sh tux

docker-tmux:
	./scripts/docker-smoke.sh tmux

docker-live-suite:
	./scripts/tmux-live-suite.sh

docker-architecture-suite:
	./scripts/tmux-architecture-suite.sh

docker-logs:
	./scripts/docker-smoke.sh logs

docker-down:
	./scripts/docker-smoke.sh down

docker-clean:
	./scripts/docker-smoke.sh clean
