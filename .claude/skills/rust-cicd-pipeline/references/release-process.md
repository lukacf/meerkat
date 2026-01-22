# Release Process

This document describes the complete release workflow for Rust projects without GitHub Actions.

## Overview

```
Development → Update Changelog → Bump Version → Tag → Push → Build Release
```

## Step-by-Step Process

### 1. Ensure All Tests Pass

Before releasing, verify the full CI pipeline passes:

```bash
make ci
```

This runs:
- Format check
- Clippy linting
- Full test suite
- Security audit

### 2. Update CHANGELOG.md

Move items from `[Unreleased]` to a new version section:

```markdown
## [Unreleased]

## [1.2.0] - 2024-12-10
### Added
- New feature description

### Fixed
- Bug fix description
```

### 3. Bump Version in Cargo.toml

Update the version field to match your planned release:

```toml
[package]
version = "1.2.0"
```

For workspace projects, also update `Cargo.lock`:

```bash
cargo update --workspace
```

### 4. Commit Version Bump

```bash
git add CHANGELOG.md Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.2.0 and update changelog"
```

### 5. Create and Push Tag

```bash
git tag -a v1.2.0 -m "Release v1.2.0 - Brief description"
git push origin main
git push origin v1.2.0
```

### 6. Build Release Artifacts

```bash
make release
```

This creates optimized binaries in `target/release/`.

### 7. Create Release Archive (Optional)

For distribution, create a release archive:

```bash
VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')
CRATE=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name')

# Create archive
tar -czvf "${CRATE}-${VERSION}-$(uname -s)-$(uname -m).tar.gz" \
    -C target/release "${CRATE}" \
    -C ../.. README.md LICENSE
```

## Version Numbering (SemVer)

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR** (1.x.x): Breaking API changes
- **MINOR** (x.1.x): New features, backwards compatible
- **PATCH** (x.x.1): Bug fixes, backwards compatible

### Pre-release Versions

For pre-releases, use standard suffixes:

```toml
version = "1.2.0-alpha.1"
version = "1.2.0-beta.1"
version = "1.2.0-rc.1"
```

## Verify Version Consistency

The Makefile includes a target to verify version matches:

```bash
make verify-version
```

This checks that `Cargo.toml` version matches the current git tag.

## Troubleshooting

### Version Mismatch Error

```
Version mismatch: Cargo.toml has X but tag is Y
```

**Fix**: Ensure Cargo.toml version matches your tag (without 'v' prefix).

### Failed Release, Need to Retry

If something fails after creating a tag:

```bash
# Delete the tag locally and remotely
git tag -d v1.2.0
git push origin :refs/tags/v1.2.0

# Fix the issue, then re-create
git tag -a v1.2.0 -m "Release v1.2.0"
git push origin v1.2.0
```

### Cargo.lock Not Updated

If you see dependency version mismatches:

```bash
cargo update --workspace
git add Cargo.lock
git commit --amend --no-edit
```

## Pre-Release Checklist

- [ ] All tests passing (`make ci`)
- [ ] CHANGELOG.md updated with all changes
- [ ] Version bumped in Cargo.toml
- [ ] No uncommitted changes (`git status`)
- [ ] On main branch with latest changes (`git pull`)
- [ ] Version verified (`make verify-version`)
- [ ] Release builds successfully (`make release`)

## CI System Integration

### Jenkins Release Pipeline

```groovy
pipeline {
    agent any

    stages {
        stage('Verify') {
            steps {
                sh 'make verify-version'
            }
        }

        stage('CI') {
            steps {
                sh 'make ci'
            }
        }

        stage('Build Release') {
            when {
                tag pattern: "v\\d+\\.\\d+\\.\\d+", comparator: "REGEXP"
            }
            steps {
                sh 'make release'
                archiveArtifacts artifacts: 'target/release/*', fingerprint: true
            }
        }
    }
}
```

### GitLab CI Release Job

```yaml
release:
  stage: release
  only:
    - tags
  script:
    - make verify-version
    - make ci
    - make release
  artifacts:
    paths:
      - target/release/
```

## Publishing to crates.io (Optional)

If publishing to crates.io:

```bash
# Dry run first
cargo publish --dry-run

# Publish
cargo publish
```

Ensure your `Cargo.toml` has all required metadata:
- `license` or `license-file`
- `description`
- `repository`
