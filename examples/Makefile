ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
CARGO ?= $(ROOT)/scripts/repo-cargo
PYTHON ?= python3
RKAT_TARGET = $(shell cd "$(ROOT)" && "$(CARGO)" --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')
RKAT ?= $(RKAT_TARGET)/debug/rkat
RKAT_RPC ?= $(RKAT_TARGET)/debug/rkat-rpc

.DEFAULT_GOAL := verify
.NOTPARALLEL:
.PHONY: verify rust standalone codemob mdm sdk shell web realtime hosts web-runtime wasm-comms browsers

# These are deterministic checks, not external-provider or physical-device tests.
verify: rust standalone sdk shell web realtime

rust:
	cd "$(ROOT)" && "$(CARGO)" test --locked --no-fail-fast -p meerkat -p meerkat-mob --examples \
		--features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills
	cd "$(ROOT)" && $(PYTHON) -m unittest discover -s examples/011-hooks-guardrails-rs -p 'test_*.py' -v

standalone: codemob mdm

codemob:
	cd "$(ROOT)" && "$(CARGO)" test --locked --manifest-path examples/034-codemob-mcp/Cargo.toml

mdm:
	cd "$(ROOT)" && "$(CARGO)" test --locked --manifest-path examples/035-mdm-tux-rs/Cargo.toml
	cd "$(ROOT)" && "$(CARGO)" test --locked -p meerkat-schedule --test exact_catalog
	cd "$(ROOT)" && $(PYTHON) scripts/test_example_locks.py

sdk:
	cd "$(ROOT)" && npm --prefix sdks/typescript run build
	cd "$(ROOT)" && npm --prefix examples run check:sdk
	cd "$(ROOT)" && npm --prefix examples run test:sdk

hosts:
	cd "$(ROOT)" && "$(CARGO)" build --locked -p rkat -p meerkat-rpc -p meerkat-rest \
		--bins --features meerkat-rpc/live-webrtc

web-runtime:
	cd "$(ROOT)" && CARGO="$(CARGO)" npm --prefix sdks/web run build

shell: hosts web-runtime
	cd "$(ROOT)/examples" && git ls-files --cached --others --exclude-standard -z -- '*.sh' | xargs -0 -n 1 bash -n
	cd "$(ROOT)" && RKAT="$(RKAT)" $(PYTHON) examples/010-mcp-tool-server-sh/test_examples.py
	cd "$(ROOT)" && RKAT="$(RKAT)" $(PYTHON) examples/028-mobpack-release-triage-sh/test_packaging.py
	cd "$(ROOT)" && RKAT="$(RKAT)" node --test --test-force-exit examples/029-web-incident-war-room-sh/test_browser_skills.mjs

# Browser suites run Playwright's bundled headless Chromium; no Google Chrome
# install is needed. Each package installs the build its own Playwright pins.
browsers:
	cd "$(ROOT)" && npm --prefix examples/031-wasm-mini-diplomacy-sh/web exec -- playwright install chromium-headless-shell
	cd "$(ROOT)" && npm --prefix examples/032-wasm-webcm-agent/web exec -- playwright install chromium-headless-shell
	cd "$(ROOT)" && npm --prefix examples/033-the-office-demo-sh/web exec -- playwright install chromium
	cd "$(ROOT)" && npm --prefix examples/037-live-webrtc-web exec -- playwright install chromium-headless-shell

wasm-comms: web-runtime
	cd "$(ROOT)" && node --test --test-force-exit sdks/web/tests/wasm_mob_comms.test.mjs

web: browsers wasm-comms
	cd "$(ROOT)" && npm --prefix examples/031-wasm-mini-diplomacy-sh/web test
	cd "$(ROOT)" && npm --prefix examples/031-wasm-mini-diplomacy-sh/web run build
	cd "$(ROOT)" && npm --prefix examples/032-wasm-webcm-agent/web test
	cd "$(ROOT)" && npm --prefix examples/032-wasm-webcm-agent/web run build
	cd "$(ROOT)" && npm --prefix examples/032-wasm-webcm-agent/web run test:guest
	cd "$(ROOT)" && npm --prefix examples/033-the-office-demo-sh/web run typecheck
	cd "$(ROOT)" && npm --prefix examples/033-the-office-demo-sh/web test
	cd "$(ROOT)" && npm --prefix examples/033-the-office-demo-sh/web run build
	cd "$(ROOT)" && npm --prefix examples/033-the-office-demo-sh/web run test:offline

realtime: browsers hosts
	cd "$(ROOT)" && PYTHONPATH=sdks/python $(PYTHON) -m unittest discover -s examples/036-realtime-audio-py -p 'test_*.py' -v
	cd "$(ROOT)" && PYTHONPATH=sdks/python $(PYTHON) examples/036-realtime-audio-py/main.py --help
	cd "$(ROOT)" && npm --prefix examples/037-live-webrtc-web run check
	cd "$(ROOT)" && npm --prefix examples/037-live-webrtc-web run build
	cd "$(ROOT)" && npm --prefix examples/037-live-webrtc-web test
	cd "$(ROOT)" && RKAT_RPC="$(RKAT_RPC)" node examples/037-live-webrtc-web/tests/local-host-smoke.cjs
