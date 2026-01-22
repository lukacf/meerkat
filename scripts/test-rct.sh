#!/bin/bash
# Run all RCT tests for Meerkat
# Provider tests run serially to avoid rate limits

set -e

echo "=== Running Meerkat RCT Tests ==="
echo ""

# Check for required API keys
if [ -z "$ANTHROPIC_API_KEY" ]; then
    echo "Warning: ANTHROPIC_API_KEY not set"
fi
if [ -z "$OPENAI_API_KEY" ]; then
    echo "Warning: OPENAI_API_KEY not set"
fi
if [ -z "$GOOGLE_API_KEY" ]; then
    echo "Warning: GOOGLE_API_KEY not set (required for Gemini)"
fi

echo ""
echo "Running meerkat-core tests..."
cargo test --package meerkat-core --lib

echo ""
echo "Running meerkat-store tests..."
cargo test --package meerkat-store --lib

echo ""
echo "Running meerkat-client tests (provider RCTs - serial)..."
RUST_TEST_THREADS=1 cargo test --package meerkat-client --lib

echo ""
echo "=== All RCT Tests Complete ==="
