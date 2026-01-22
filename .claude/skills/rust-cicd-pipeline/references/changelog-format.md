# Changelog Format

This project uses [Keep a Changelog](https://keepachangelog.com/) format with [Semantic Versioning](https://semver.org/).

## Template

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- New features that have been added

### Changed
- Changes in existing functionality

### Deprecated
- Features that will be removed in future versions

### Removed
- Features that have been removed

### Fixed
- Bug fixes

### Security
- Security vulnerability fixes

## [1.0.0] - 2024-12-10

### Added
- Initial release
- Core functionality X
- Feature Y

### Security
- Implemented authentication system

[Unreleased]: https://github.com/user/repo/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/user/repo/releases/tag/v1.0.0
```

## Section Guidelines

### Added
New features or capabilities. Use for:
- New API endpoints
- New CLI commands
- New configuration options
- New integrations

### Changed
Changes to existing functionality. Use for:
- Modified behavior
- Updated defaults
- Improved performance
- Refactored internals (if user-visible)

### Deprecated
Features marked for future removal. Use for:
- Old API versions
- Legacy configuration options
- Features being replaced

### Removed
Features that have been removed. Use for:
- Deleted functionality
- Removed dependencies
- Dropped platform support

### Fixed
Bug fixes. Use for:
- Corrected behavior
- Fixed crashes
- Resolved edge cases

### Security
Security-related changes. Use for:
- Vulnerability patches
- Security improvements
- Dependency updates for CVEs

## Version Extraction for Releases

The release process extracts changelog entries using this pattern:

```bash
# Extract section for version 1.2.0
sed -n '/^## \[1.2.0\]/,/^## \[/p' CHANGELOG.md | sed '$d'
```

Ensure your changelog follows the exact format for automated extraction:
- Version headers: `## [X.Y.Z] - YYYY-MM-DD`
- Section headers: `### Added`, `### Changed`, etc.
- List items: `- Description of change`

## Best Practices

1. **Update as you go**: Add entries when making changes, not at release time
2. **Be specific**: "Fixed login bug" → "Fixed session timeout during OAuth flow"
3. **User-focused**: Describe impact, not implementation details
4. **Link issues**: Reference issue numbers when relevant: "Fixed crash on startup (#123)"
5. **Group related changes**: Keep related items together under the same section
