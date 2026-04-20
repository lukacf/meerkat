#!/usr/bin/env bash
# pre-push-bridge-no-responsestatus.sh — grep gate for W2-F.
#
# The supervisor bridge transports typed commands/replies. It must not
# re-interpret `ResponseStatus` — all "is this terminal?" questions go
# through `classify_response_terminality` / `TerminalityClass`, so the
# canonical classifier in `meerkat-core::interaction` stays the single
# source of truth. See issue #264 (W2-F).
#
# Exit 0 = clean, exit 1 = violations found.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

VIOLATIONS=0

# Files that must not match `ResponseStatus` on their non-test, non-import
# lines. The production path is limited to the bridge sender, the bridge
# wire types, and the bridge receiver used by local members. The bridge
# receiver in `supervisor_bridge.rs` is allowed to name
# `ResponseStatus` in test-only code (see `#[cfg(test)]` block) because
# the test module exercises the canonical classifier directly.
BRIDGE_FILES=(
    "meerkat-mob/src/runtime/supervisor_bridge.rs"
    "meerkat-mob/src/runtime/local_bridge.rs"
    "meerkat-contracts/src/wire/supervisor_bridge.rs"
)

for file in "${BRIDGE_FILES[@]}"; do
    if [[ ! -f "$file" ]]; then
        continue
    fi

    # Strip everything after `#[cfg(test)]` / `mod tests` so test code can
    # still name `ResponseStatus`. The production bridge must not `match`
    # on `ResponseStatus`.
    production=$(awk '
        /^#\[cfg\(test\)\]/ { in_test = 1 }
        /^mod tests/        { in_test = 1 }
        !in_test            { print NR ": " $0 }
    ' "$file")

    # The W2-F contract: no `match` on `ResponseStatus` in bridge code.
    # Doc comments and imports naming the type are fine — the point is to
    # ban re-deriving "is this terminal?" locally.
    hits=$(printf '%s\n' "$production" \
        | grep -v '^\s*[0-9]*:\s*//' \
        | grep -v '^\s*[0-9]*:\s*///' \
        | grep -E 'match[^;]*ResponseStatus|ResponseStatus::(Completed|Failed|Accepted)' \
        || true)

    if [[ -n "$hits" ]]; then
        echo -e "${RED}W2-F violation:${NC} bridge file interprets \`ResponseStatus\` directly — route through \`classify_response_terminality\` + \`TerminalityClass\`:"
        echo -e "  file: ${file}"
        echo "$hits" | sed 's/^/    /'
        VIOLATIONS=$((VIOLATIONS + 1))
    fi
done

if [[ $VIOLATIONS -eq 0 ]]; then
    echo -e "${GREEN}W2-F bridge-classifier gate clean.${NC}"
    exit 0
fi

echo ""
echo -e "${RED}${VIOLATIONS} file(s) violate the W2-F contract.${NC}"
echo "All terminal-vs-progress decisions in the bridge layer must go"
echo "through \`meerkat_core::interaction::classify_response_terminality\`"
echo "and match \`TerminalityClass\` exhaustively."
exit 1
