#!/usr/bin/env bash
# Verify version parity across Rust, Bazel, Python, and TypeScript packages.
# Exit 0 if everything is in sync, exit 1 with diagnostics on any mismatch.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO="${ROOT}/scripts/repo-cargo"
FAIL=0

red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }

# 1. Package version parity

CARGO_VER=$("$CARGO" metadata --manifest-path "$ROOT/Cargo.toml" \
    --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "meerkat-meta") | .version')

PY_VER=$(python3 -c "
import pathlib
import re
text = pathlib.Path('$ROOT/sdk/python/pyproject.toml').read_text()
project = re.search(r'(?ms)^\[project\][ \t]*$\n(.*?)(?=^\[|\Z)', text)
version = project and re.search(r'(?m)^[ \t]*version[ \t]*=[ \t]*\"([^\"]+)\"', project.group(1))
if not version:
    raise SystemExit('pyproject.toml [project].version is missing or unparsable')
print(version.group(1))
")

TS_VER=""
if [ -f "$ROOT/sdk/typescript/package.json" ]; then
    TS_VER=$(node -p "require('$ROOT/sdk/typescript/package.json').version")
fi

MODULE_VER=""
MODULE_FILE="$ROOT/MODULE.bazel"
if [ -f "$MODULE_FILE" ]; then
    MODULE_VER=$(sed -n '/^module(/,/^)/s/.*version = "\([^"]*\)".*/\1/p' "$MODULE_FILE")
fi

echo "Package versions:"
echo "  Cargo (meerkat-meta):  $CARGO_VER"
echo "  Python SDK:                   $PY_VER"
if [ -n "$TS_VER" ]; then
    echo "  TypeScript SDK:               $TS_VER"
fi
if [ -f "$MODULE_FILE" ]; then
    if [ -n "$MODULE_VER" ]; then
        echo "  Bazel module:                 $MODULE_VER"
    else
        echo "  Bazel module:                 <missing or unparsable>"
    fi
fi

PKG_OK=true
if [ "$CARGO_VER" != "$PY_VER" ]; then
    red "FAIL: Python SDK version mismatch ($PY_VER != $CARGO_VER)"
    PKG_OK=false
    FAIL=1
fi
if [ -n "$TS_VER" ] && [ "$CARGO_VER" != "$TS_VER" ]; then
    red "FAIL: TypeScript SDK version mismatch ($TS_VER != $CARGO_VER)"
    PKG_OK=false
    FAIL=1
fi
if [ -f "$MODULE_FILE" ]; then
    if [ -z "$MODULE_VER" ]; then
        red "FAIL: MODULE.bazel module version is missing or unparsable"
        PKG_OK=false
        FAIL=1
    elif [ "$CARGO_VER" != "$MODULE_VER" ]; then
        red "FAIL: Bazel module version mismatch ($MODULE_VER != $CARGO_VER)"
        PKG_OK=false
        FAIL=1
    fi
fi
if $PKG_OK; then
    green "  Package versions: OK"
fi

# 2. Bazel rustc_env parity — drives env!("CARGO_PKG_VERSION") in the
# bazel-built RELEASE binaries (and their --version). Drifted to 0.7.4 across
# 0.7.5–0.7.7 because nothing checked it; now it does.
BAZEL_FILE="$ROOT/crates/meerkat-meta/BUILD.bazel"
if [ -f "$BAZEL_FILE" ]; then
    BAD_BAZEL=$(grep -oE '"CARGO_PKG_VERSION": "[^"]*"' "$BAZEL_FILE" \
        | grep -v "\"$CARGO_VER\"" | sort -u || true)
    if [ -n "$BAD_BAZEL" ]; then
        red "FAIL: BUILD.bazel CARGO_PKG_VERSION entries != $CARGO_VER:"
        printf '%s\n' "$BAD_BAZEL"
        FAIL=1
    else
        green "  BUILD.bazel CARGO_PKG_VERSION: OK"
    fi
fi

# 3. TypeScript SDK lockfile root version parity. npm lockfiles repeat the
# package version at the document root and at packages[""]. Both fields are
# release inputs and both are owned by bump-sdk-versions.sh.
LOCK_FILE="$ROOT/sdk/typescript/package-lock.json"
if [ -f "$LOCK_FILE" ]; then
    LOCK_VER=$(node -p "require('$LOCK_FILE').version" 2>/dev/null || echo "")
    LOCK_ROOT_PKG_VER=$(node -p "require('$LOCK_FILE').packages?.['']?.version" 2>/dev/null || echo "")
    if [ -z "$LOCK_VER" ] || [ -z "$LOCK_ROOT_PKG_VER" ]; then
        red "FAIL: package-lock.json root versions are missing or unparsable"
        FAIL=1
    elif [ "$LOCK_VER" != "$CARGO_VER" ] || [ "$LOCK_ROOT_PKG_VER" != "$CARGO_VER" ]; then
        red "FAIL: package-lock.json version mismatch (top-level=$LOCK_VER, packages[\"\"]=$LOCK_ROOT_PKG_VER, expected=$CARGO_VER)"
        FAIL=1
    else
        green "  package-lock.json root versions: OK"
    fi
fi

# 4. Canonical Rust installation docs. Cargo's 0.x caret semantics do not
# cross minor versions, so an old snippet silently keeps users off the release.
for DOC in "$ROOT/docs/quickstart.mdx" "$ROOT/docs/sdks/rust.mdx"; do
    if [ -f "$DOC" ]; then
        DOC_VERSIONS=$(sed -n 's/.*meerkat-meta = "\([^"]*\)".*/\1/p' "$DOC" | sort -u)
        if [ -z "$DOC_VERSIONS" ]; then
            red "FAIL: $DOC has no meerkat-meta installation version"
            FAIL=1
        elif [ "$DOC_VERSIONS" != "$CARGO_VER" ]; then
            red "FAIL: $DOC install version mismatch ($DOC_VERSIONS != $CARGO_VER)"
            FAIL=1
        else
            green "  $(basename "$DOC") Rust install version: OK"
        fi
    fi
done

echo ""
if [ $FAIL -ne 0 ]; then
    red "Version parity check FAILED"
    exit 1
else
    green "All version parity checks passed"
fi
