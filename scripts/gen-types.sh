#!/usr/bin/env bash
# Regenerate contracts/types.ts from the PPB OpenAPI document (contract §21).
#   ./scripts/gen-types.sh
# Requires: Rust toolchain + Node (npx). Produces `contracts/types.ts` (snake_case, §20).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[1/3] dumping OpenAPI JSON (ppb-server --openapi)..."
cargo run --quiet --bin ppb-server -- --openapi > /tmp/ppb-openapi.json
test -s /tmp/ppb-openapi.json || { echo "empty OpenAPI JSON" >&2; exit 1; }
grep -q '"/api/v1/me"' /tmp/ppb-openapi.json || { echo "core path /api/v1/me missing" >&2; exit 1; }

echo "[2/3] generating TS types (openapi-typescript)..."
# Diagnostic: what components/refs are present (helps debug $ref resolution).
echo "--- component schema keys ---"
jq -r '.components.schemas | keys[]' /tmp/ppb-openapi.json 2>/dev/null | head -40 || true
echo "--- sample \$ref values ---"
grep -oE '"\\$ref": *"[^"]*"' /tmp/ppb-openapi.json 2>/dev/null | sort -u | head -20 || true
echo "--- /me 200 schema ---"
jq -c '.paths["/api/v1/me"].get.responses["200"].content["application/json"].schema' /tmp/ppb-openapi.json 2>/dev/null || true
echo "--- ErrorEnvelope component ---"
jq -c '.components.schemas.ErrorEnvelope' /tmp/ppb-openapi.json 2>/dev/null || true
npx --yes openapi-typescript@latest /tmp/ppb-openapi.json -o contracts/types.ts

echo "[3/3] done: contracts/types.ts"
