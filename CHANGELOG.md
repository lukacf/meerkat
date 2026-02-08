# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Initial project structure with workspace layout
- Core agent loop with budget enforcement and checkpointing
- LLM client support for Anthropic, OpenAI, and Gemini providers
- MCP client for tool server integration
- Session persistence with JSONL storage
- CLI (`rkat`) with `run` and `resume` commands
- Provider auto-detection from model name prefixes
- `--provider` flag for explicit provider selection
- MCP server for exposing Meerkat as tools to other agents
- REST API server (meerkat-rest)

### Changed
- Project renamed from "raik" to "Meerkat" with CLI binary "rkat"

### Changed - Feature defaults
- `meerkat-tools`: comms, mcp, and sub-agents are now optional features (default: on)
  - `--no-default-features` builds tools with zero optional deps
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat` facade: comms, mcp, and sub-agents are now optional features (default: on)
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat-rpc`: minimal by default — no comms/mcp/sub-agents unless explicitly enabled
- `meerkat-rest`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-mcp-server`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-cli`: comms and mcp are opt-in (default: on), all inline code cfg-gated
- `agent_spawn` tool: `host_mode` field removed from schema when comms feature is off

### Removed
- Dead files in meerkat-core: `comms_runtime.rs`, `comms_bootstrap.rs`, `comms_config.rs`, `agent/comms.rs`
- Duplicate LlmClientAdapter/DynLlmClientAdapter in meerkat-tools (uses canonical from meerkat-client)

## [0.1.0] - Unreleased

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
