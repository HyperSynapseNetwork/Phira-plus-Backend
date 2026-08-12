#!/usr/bin/env bash
# Regenerate contracts/types.ts + contracts/openapi.json from the PPB OpenAPI
# document (contract §21).
#   ./scripts/gen-types.sh
# Requires: Rust toolchain + Node (npx). Produces `contracts/types.ts`
# (snake_case, §20) and `contracts/openapi.json` (the HTTP Source of Truth that
# PPF/Panel pull for the consistency check).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[1/4] dumping OpenAPI JSON (ppb-server --openapi)..."
cargo run --quiet --bin ppb-server -- --openapi > /tmp/ppb-openapi.json
test -s /tmp/ppb-openapi.json || { echo "empty OpenAPI JSON" >&2; exit 1; }
grep -q '"/api/v1/me"' /tmp/ppb-openapi.json || { echo "core path /api/v1/me missing" >&2; exit 1; }

echo "[2/4] storing the OpenAPI document (contracts/openapi.json)..."
cp /tmp/ppb-openapi.json contracts/openapi.json

echo "[3/4] generating TS types (openapi-typescript)..."
npx --yes openapi-typescript@latest /tmp/ppb-openapi.json -o contracts/types.ts

echo "[4/4] done: contracts/types.ts + contracts/openapi.json"
