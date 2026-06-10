#!/usr/bin/env bash

set -euo pipefail

printf '%s\n' \
  meerkat-machine-derive \
  meerkat-machine-dsl-core \
  meerkat-agent-build-authority \
  meerkat-core \
  meerkat-capabilities \
  meerkat-machine-dsl \
  meerkat-machine-schema \
  meerkat-machine-kernels \
  meerkat-skills \
  meerkat-schedule \
  meerkat-workgraph \
  meerkat-contracts \
  meerkat-store \
  meerkat-llm-core \
  meerkat-live \
  meerkat-auth-core \
  meerkat-memory \
  meerkat-mcp \
  meerkat-hooks \
  meerkat-comms \
  meerkat-anthropic \
  meerkat-gemini \
  meerkat-providers \
  meerkat-runtime \
  meerkat-openai \
  meerkat-tools \
  meerkat-session \
  meerkat-client \
  meerkat \
  meerkat-mob \
  meerkat-mob-mcp \
  meerkat-mob-pack \
  meerkat-mcp-server \
  meerkat-rpc \
  meerkat-rest \
  rkat
