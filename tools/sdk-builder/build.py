#!/usr/bin/env python3
"""SDK Builder for Meerkat.

Given a profile manifest (meerkat-sdk.toml or profiles/*.toml), this tool:
1. Resolves features into --features flags
2. Builds a custom rkat binary
3. Emits schemas for the built capabilities
4. Runs codegen to produce typed SDK packages
5. Emits a bundle manifest

Requires a Rust toolchain (cargo).
"""

import hashlib
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

try:
    import tomllib
except ImportError:
    import tomli as tomllib  # type: ignore[no-redef]


def load_manifest(path: Path) -> dict:
    """Load a TOML manifest file."""
    with open(path, "rb") as f:
        return tomllib.load(f)


def resolve_features(manifest: dict) -> list[str]:
    """Resolve feature flags from the manifest.

    Returns CLI-level features only. Features like 'anthropic', 'jsonl-store',
    and 'sub-agents' are hardcoded in meerkat-cli's dependency on the facade
    crate and do not need to be passed as --features flags.
    """
    features = manifest.get("features", {})
    include = features.get("include", [])
    exclude = features.get("exclude", [])

    # Only features that meerkat-cli actually defines in its [features] table.
    # Facade-level features (anthropic, jsonl-store, sub-agents) are wired
    # through the CLI's dependency declaration on the meerkat facade crate.
    cli_features = ["comms", "mcp"]

    result = []
    for f in cli_features:
        if f in exclude:
            continue
        result.append(f)

    for f in include:
        if f not in result and f in cli_features:
            result.append(f)

    return result


def build_runtime(features: list[str], root: Path) -> Path:
    """Build the rkat binary with the given features."""
    cmd = ["cargo", "build", "-p", "meerkat-cli", "--release"]
    if features:
        cmd.extend(["--features", ",".join(features)])
    print(f"Building runtime: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Build failed:\n{result.stderr}", file=sys.stderr)
        sys.exit(1)

    binary = root / "target" / "release" / "rkat"
    if not binary.exists():
        print(f"Binary not found at {binary}", file=sys.stderr)
        sys.exit(1)

    return binary


def emit_schemas(root: Path) -> None:
    """Run the schema emitter."""
    cmd = [
        "cargo", "run", "-p", "meerkat-contracts",
        "--features", "schema",
        "--bin", "emit-schemas",
    ]
    print(f"Emitting schemas: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Schema emission failed:\n{result.stderr}", file=sys.stderr)
        sys.exit(1)


def run_codegen(root: Path) -> None:
    """Run the SDK code generator."""
    cmd = [sys.executable, str(root / "tools" / "sdk-codegen" / "generate.py")]
    print(f"Running codegen: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Codegen failed:\n{result.stderr}", file=sys.stderr)
        sys.exit(1)
    print(result.stdout)


def get_git_commit(root: Path) -> str:
    """Get the current git commit hash."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=root, capture_output=True, text=True
    )
    return result.stdout.strip() if result.returncode == 0 else "unknown"


def file_hash(path: Path) -> str:
    """Compute SHA-256 hash of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def emit_bundle_manifest(
    manifest: dict,
    features: list[str],
    binary: Path,
    root: Path,
    output_dir: Path,
) -> None:
    """Emit the bundle manifest."""
    version_file = root / "artifacts" / "schemas" / "version.json"
    contract_version = "0.1.0"
    if version_file.exists():
        with open(version_file) as f:
            contract_version = json.load(f).get("contract_version", "0.1.0")

    bundle = {
        "profile": manifest.get("profile", {}).get("name", "unknown"),
        "features": features,
        "contract_version": contract_version,
        "source_commit": get_git_commit(root),
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "binary_hash": file_hash(binary) if binary.exists() else "not_built",
    }

    output_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = output_dir / "bundle-manifest.json"
    with open(manifest_path, "w") as f:
        json.dump(bundle, f, indent=2)
    print(f"Bundle manifest written to {manifest_path}")


def main():
    if len(sys.argv) < 2:
        print("Usage: sdk-builder <manifest.toml>", file=sys.stderr)
        print("\nPreset profiles:")
        print("  profiles/minimal.toml")
        print("  profiles/standard.toml")
        print("  profiles/full.toml")
        sys.exit(1)

    manifest_path = Path(sys.argv[1])
    root = Path(__file__).parent.parent.parent

    manifest = load_manifest(manifest_path)
    profile_name = manifest.get("profile", {}).get("name", "custom")
    print(f"Building profile: {profile_name}")

    # 1. Resolve features
    features = resolve_features(manifest)
    print(f"Features: {features}")

    # 2. Build runtime
    binary = build_runtime(features, root)
    print(f"Runtime binary: {binary}")

    # 3. Emit schemas
    emit_schemas(root)

    # 4. Run codegen
    run_codegen(root)

    # 5. Emit bundle manifest
    output_dir = root / "dist" / profile_name
    emit_bundle_manifest(manifest, features, binary, root, output_dir)

    print(f"\nBuild complete: {profile_name}")
    print(f"  Binary: {binary}")
    print(f"  SDK artifacts: sdks/python/, sdks/typescript/")
    print(f"  Bundle manifest: {output_dir / 'bundle-manifest.json'}")


if __name__ == "__main__":
    main()
