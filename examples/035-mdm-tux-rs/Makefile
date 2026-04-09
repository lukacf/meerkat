CARGO   ?= cargo
PROFILE ?= release
TARGET_DIR := target/$(PROFILE)

BINS := mdm-tux mdm-target mdm-kennel

.PHONY: all build sign clean

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
