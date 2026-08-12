# PPB contract scripts

## `gen-types.sh`

Regenerate the PPB contract artifacts from the server's OpenAPI document.

```
./scripts/gen-types.sh
```

Produces:
- `contracts/types.ts` — openapi-typescript output (snake_case, contract §20), consumed by PPF/Panel instead of hand-written types.
- `contracts/openapi.json` — the HTTP **Source of Truth** (contract §21). PPF/Panel pull this for the consistency check.

Requires a working Rust toolchain (`ppb-server --openapi`) and Node (`npx`). The CI `contract-types` job runs it on every push and commits any changes.

## `check-contract-consistency.sh`

Verify every HTTP/WS call a frontend makes exists in the PPB OpenAPI document.

```
./scripts/check-contract-consistency.sh <openapi.json> <src_dir>
```

Scans `<src_dir>` (recursively, skipping `node_modules/.nuxt/.output/dist/tests/generated types`) for call sites whose URL literal starts with `/api/v1`, `/ws/v1`, or `/admin/` (Panel's relative admin prefix, resolved to `/api/v1/admin/...`). For each call it:

1. Normalizes template params: `` `/api/v1/users/${id}` `` → `/api/v1/users/{param}`.
2. Infers the HTTP method from the enclosing call: `.get()/.post()/.put()/.patch()/.delete()` chains (incl. `api.post<T>(...)` generics), or `method: 'POST'` in call options, default `GET`.
3. Checks the OpenAPI document has the same method + path (param-agnostic; a frontend `{param}` also matches any single OpenAPI segment).

Unmatched calls are printed as `FAIL file:line METHOD path` and the script exits non-zero. Known runtime alias paths (e.g. `/api/v1/admin/auth/reauth`) are accepted with a `WARN` (spec: aliases are not required to match canonical). WS endpoints are checked against the built-in allowlist.

Examples:

```
./scripts/check-contract-consistency.sh contracts/openapi.json ../ppf/src
./scripts/check-contract-consistency.sh contracts/openapi.json ../panel
```

The CI `contract-types` job runs this against the latest `contracts/openapi.json` and the `main` branch of PPF and Panel.
