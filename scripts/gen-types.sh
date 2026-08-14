#!/usr/bin/env bash
# Regenerate the exact PPB contract bundle from Rust source.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[1/7] dumping OpenAPI JSON from Rust source..."
cargo run --quiet --bin ppb-server -- --openapi > /tmp/ppb-openapi.json
test -s /tmp/ppb-openapi.json || { echo "empty OpenAPI JSON" >&2; exit 1; }
grep -q '"/api/v1/me"' /tmp/ppb-openapi.json || { echo "core path /api/v1/me missing" >&2; exit 1; }
cp /tmp/ppb-openapi.json contracts/openapi.json

echo "[2/7] syncing ErrorCode manifest from Rust..."
python3 scripts/sync-contract-metadata.py --codes-only

echo "[3/7] proving Rust ErrorCode == dumped OpenAPI enum before type generation..."
python3 scripts/check-error-contract.py

echo "[4/7] generating TypeScript from the exact dumped OpenAPI..."
npx --yes openapi-typescript@latest /tmp/ppb-openapi.json -o contracts/types.ts

echo "[5/7] refreshing exact contract metadata..."
python3 scripts/sync-contract-metadata.py

echo "[6/7] verifying joined PPB contract artifacts..."
python3 scripts/check-error-contract.py
python3 scripts/verify-contract-bundle.py

echo "[7/7] done: openapi/types/error-codes/contract-version are one source bundle"
