#!/usr/bin/env python3
"""
Setup script for Rust CI/CD Pipeline

This script initializes a CI/CD pipeline for a Rust project by:
1. Creating necessary configuration files from templates
2. Setting up pre-commit hooks
3. Configuring cargo-deny for security auditing

Usage:
    python setup_cicd.py --crate-name my_crate
    python setup_cicd.py --crate-name my_crate --rust-version 1.75.0

The script reads templates from the references/ directory and customizes them
based on the provided arguments.
"""

import argparse
import sys
from pathlib import Path


def get_skill_dir() -> Path:
    """Get the skill directory (parent of scripts/)."""
    return Path(__file__).parent.parent


def read_template(name: str) -> str:
    """Read a template file from references/."""
    template_path = get_skill_dir() / "references" / name
    if not template_path.exists():
        print(f"Error: Template not found: {template_path}")
        sys.exit(1)
    return template_path.read_text()


def extract_makefile(template: str) -> str:
    """Extract makefile content from markdown template."""
    lines = template.split("```makefile")[1].split("```")[0].strip()
    return lines


def customize_template(content: str, replacements: dict) -> str:
    """Replace placeholders in template content."""
    for placeholder, value in replacements.items():
        content = content.replace(f"{{{{{placeholder}}}}}", value)
    return content


def write_file(path: Path, content: str, dry_run: bool = False, force: bool = False):
    """Write content to file, creating directories as needed."""
    if path.exists() and not force:
        print(f"Skipped (exists): {path}")
        return False

    if dry_run:
        status = "Would overwrite" if path.exists() else "Would create"
        print(f"{status}: {path}")
        return True

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    print(f"Created: {path}")
    return True


def create_makefile(project_dir: Path, crate_name: str, dry_run: bool = False, force: bool = False):
    """Create Makefile from template."""
    template = read_template("makefile-template.md")
    content = extract_makefile(template)
    content = customize_template(content, {"CRATE_NAME": crate_name})
    write_file(project_dir / "Makefile", content, dry_run, force)


def create_pre_commit_config(project_dir: Path, dry_run: bool = False, force: bool = False):
    """Create .pre-commit-config.yaml from template."""
    content = read_template("pre-commit-config.yaml")
    write_file(project_dir / ".pre-commit-config.yaml", content, dry_run, force)


def create_deny_config(project_dir: Path, dry_run: bool = False, force: bool = False):
    """Create deny.toml from template."""
    content = read_template("deny-config.toml")
    write_file(project_dir / "deny.toml", content, dry_run, force)


def create_rust_toolchain(project_dir: Path, rust_version: str, dry_run: bool = False, force: bool = False):
    """Create rust-toolchain.toml from template."""
    content = read_template("rust-toolchain-example.toml")
    content = content.replace('channel = "1.75.0"', f'channel = "{rust_version}"')
    write_file(project_dir / "rust-toolchain.toml", content, dry_run, force)


def create_changelog(project_dir: Path, dry_run: bool = False, force: bool = False):
    """Create initial CHANGELOG.md."""
    content = """# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Initial project setup

"""
    write_file(project_dir / "CHANGELOG.md", content, dry_run, force)


def create_test_directories(project_dir: Path, dry_run: bool = False):
    """Create test directory structure."""
    dirs = [
        project_dir / "tests",
        project_dir / "benches",
    ]

    for d in dirs:
        if dry_run:
            if not d.exists():
                print(f"Would create directory: {d}")
        else:
            if not d.exists():
                d.mkdir(parents=True, exist_ok=True)
                print(f"Created directory: {d}")


def check_prerequisites(project_dir: Path):
    """Check for required files and tools."""
    warnings = []

    # Check for Cargo.toml
    if not (project_dir / "Cargo.toml").exists():
        warnings.append("No Cargo.toml found. Create one or run 'cargo init' first.")

    # Check for rustup
    import shutil
    if not shutil.which("rustup"):
        warnings.append("rustup not found. Install from https://rustup.rs/")

    if not shutil.which("cargo"):
        warnings.append("cargo not found. Install Rust toolchain first.")

    return warnings


def main():
    parser = argparse.ArgumentParser(
        description="Set up CI/CD pipeline for a Rust project",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    %(prog)s --crate-name my_crate
    %(prog)s --crate-name my_crate --rust-version 1.80.0
    %(prog)s --crate-name my_crate --dry-run
    %(prog)s --crate-name my_crate --force
        """
    )
    parser.add_argument(
        "--project-dir",
        type=Path,
        default=Path.cwd(),
        help="Project directory (default: current directory)"
    )
    parser.add_argument(
        "--crate-name",
        type=str,
        required=True,
        help="Name of the crate (used in Makefile)"
    )
    parser.add_argument(
        "--rust-version",
        type=str,
        default="1.75.0",
        help="Rust version to pin (default: 1.75.0)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be created without creating files"
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Overwrite existing files"
    )
    parser.add_argument(
        "--skip-makefile",
        action="store_true",
        help="Skip Makefile creation"
    )
    parser.add_argument(
        "--skip-changelog",
        action="store_true",
        help="Skip CHANGELOG.md creation"
    )
    parser.add_argument(
        "--skip-toolchain",
        action="store_true",
        help="Skip rust-toolchain.toml creation"
    )

    args = parser.parse_args()
    project_dir = args.project_dir.resolve()

    print(f"Setting up Rust CI/CD pipeline in: {project_dir}")
    print(f"Crate name: {args.crate_name}")
    print(f"Rust version: {args.rust_version}")
    if args.dry_run:
        print("(Dry run - no files will be created)")
    if args.force:
        print("(Force mode - existing files will be overwritten)")
    print()

    # Check prerequisites
    warnings = check_prerequisites(project_dir)
    for warning in warnings:
        print(f"Warning: {warning}")
    if warnings:
        print()

    # Create files
    if not args.skip_makefile:
        create_makefile(project_dir, args.crate_name, args.dry_run, args.force)

    create_pre_commit_config(project_dir, args.dry_run, args.force)
    create_deny_config(project_dir, args.dry_run, args.force)

    if not args.skip_toolchain:
        create_rust_toolchain(project_dir, args.rust_version, args.dry_run, args.force)

    if not args.skip_changelog:
        create_changelog(project_dir, args.dry_run, args.force)

    create_test_directories(project_dir, args.dry_run)

    print()
    print("Setup complete! Next steps:")
    print("1. Review and customize the generated files")
    print("2. Install required tools:")
    print("   rustup component add clippy rustfmt")
    print("   cargo install cargo-deny cargo-audit cargo-tarpaulin")
    print("3. Install pre-commit: pip install pre-commit")
    print("4. Install hooks: make install-hooks")
    print("5. Run CI locally: make ci")


if __name__ == "__main__":
    main()
