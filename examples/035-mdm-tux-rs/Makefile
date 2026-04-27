CARGO   ?= cargo
PROFILE ?= release
TARGET_DIR := target/$(PROFILE)

BINS := mdm-tux mdm-target mdm-kennel

.PHONY: all build sign clean docker-smoke docker-tux docker-tmux docker-live-suite docker-architecture-suite docker-logs docker-down docker-clean

all: build sign

build:
ifeq ($(PROFILE),release)
	$(CARGO) build --release
else
	$(CARGO) build
endif

sign: build
	@for bin in $(BINS); do \
		echo "codesign $$bin"; \
		codesign --force --sign - $(TARGET_DIR)/$$bin; \
	done

clean:
	$(CARGO) clean

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
