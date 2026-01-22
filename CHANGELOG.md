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

## [0.1.0] - Unreleased

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
