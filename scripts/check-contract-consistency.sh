#!/usr/bin/env bash
# Contract consistency check (DESIGN/CONTRACT_CONSISTENCY_TEST.md).
#
# Usage:
#   scripts/check-contract-consistency.sh <openapi.json> <src_dir>
#
# Scans a frontend src dir for HTTP/WS call sites (string/template literals
# starting with /api/v1 or /ws/v1), normalizes template params (`${...}` ->
# `{param}`), derives the HTTP method from the enclosing call (`.get()/.post()`
# chains, `method: 'X'` options, or GET default), and verifies every
# (method, path) exists in the OpenAPI document (same method + path,
# param-agnostic). Unmatched calls are printed as FAIL (file:line METHOD path);
# any FAIL makes the script exit non-zero.
#
# Known WS endpoints are checked against a built-in allowlist (the REST
# OpenAPI document does not list WS paths).
#
# Examples:
#   scripts/check-contract-consistency.sh contracts/openapi.json ../ppf/src
#   scripts/check-contract-consistency.sh contracts/openapi.json ../panel/src
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <openapi.json> <src_dir>" >&2
  exit 2
fi

OPENAPI="$1"
SRC_DIR="$2"
test -f "$OPENAPI" || { echo "openapi file not found: $OPENAPI" >&2; exit 2; }
test -d "$SRC_DIR" || { echo "src dir not found: $SRC_DIR" >&2; exit 2; }

python3 - "$OPENAPI" "$SRC_DIR" <<'PYEOF'
import json, os, pathlib, re, sys

openapi_path = sys.argv[1]
src_dir = pathlib.Path(sys.argv[2])

doc = json.loads(open(openapi_path).read())
paths = doc.get("paths", {})

# Built-in WS endpoint allowlist (contract §1/§4): REST OpenAPI does not list WS.
WS_ALLOW = [
    "/ws/v1/rooms/{room_uuid}/live",
    "/ws/v1/replays/{round_uuid}",
]

# Runtime alias paths PPB serves for the same handler as a canonical path
# (spec: "PPB 别名不计为必需匹配"). These are accepted (WARN) so frontends
# can migrate to canonical paths without breaking the gate.
ALIAS_ALLOW = {
    "/api/v1/admin/auth/reauth",        # canonical: /api/v1/auth/phira/reauth
    "/api/v1/admin/server",              # canonical: /api/v1/admin/server/status
    "/api/v1/admin/permissions",         # canonical: /api/v1/admin/permissions/manifest
    "/api/v1/admin/runbook-runs",        # canonical: /api/v1/admin/automation/runbook-runs
    "/api/v1/admin/commands/history",    # canonical: /api/v1/admin/commands
}

def norm_params(p: str) -> str:
    return re.sub(r"\{[^}]+\}", "{}", p)

def seg_loose_match(call: str, op: str) -> bool:
    c = call.split("/")
    o = op.split("/")
    if len(c) != len(o):
        return False
    for a, b in zip(c, o):
        if a == b:
            continue
        if a == "{param}":
            continue          # frontend variable matches any one segment
        if b.startswith("{") and b.endswith("}"):
            continue          # OpenAPI param matches any literal
        return False
    return True

def path_matches(call: str, op: str) -> bool:
    if norm_params(call) == norm_params(op):
        return True
    if "{param}" in call:
        return seg_loose_match(call, op)
    return False

HTTP_METHODS = ("get", "post", "put", "patch", "delete", "head", "options", "trace")

def openapi_methods(item) -> set:
    if not isinstance(item, dict):
        return set()
    return {k for k in item.keys() if k in HTTP_METHODS}

# ── lightweight tokenizer: skips comments, tracks strings/templates and parens
def tokenize(text):
    tokens = []  # (kind, value, start, end); kind in {str, tmpl, open, close, ident, punct}
    open_stack = []  # indices into tokens of open parens
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1; continue
        if text.startswith("//", i):
            j = text.find("\n", i); i = n if j == -1 else j + 1; continue
        if text.startswith("/*", i):
            j = text.find("*/", i + 2); i = n if j == -1 else j + 2; continue
        if c in "'\"`":
            quote = c
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2; continue
                if text[j] == quote:
                    break
                j += 1
            kind = "tmpl" if quote == "`" else "str"
            tokens.append((kind, text[i:j + 1], i, j + 1))
            i = j + 1; continue
        if c in "([{":
            idx = len(tokens)
            tokens.append(("open", c, i, i + 1))
            if c == "(":
                open_stack.append(idx)
            i += 1; continue
        if c in ")]}":
            if c == ")" and open_stack:
                open_stack.pop()
            tokens.append(("close", c, i, i + 1))
            i += 1; continue
        m = re.match(r"[A-Za-z_$][A-Za-z0-9_$]*", text[i:])
        if m:
            tokens.append(("ident", m.group(0), i, i + len(m.group(0))))
            i += len(m.group(0)); continue
        tokens.append(("punct", c, i, i + 1))
        i += 1
    return tokens, open_stack

def call_name_before(text, open_idx, tokens):
    # walk back from open_idx collecting idents/dots/'()' until a stop token;
    # skip TypeScript generic type args (`<T>`) so `api.post<T>(` -> `api.post`
    parts = []
    j = open_idx - 1
    while j >= 0:
        kind, val, s, e = tokens[j]
        if kind == "punct" and val == ">":
            # skip back to the matching '<' (generics)
            depth = 1
            j -= 1
            while j >= 0 and depth > 0:
                k2, v2, _, _ = tokens[j]
                if v2 == ">":
                    depth += 1
                elif v2 == "<":
                    depth -= 1
                j -= 1
            continue
        if kind == "ident":
            parts.append(val)
        elif kind == "punct" and val in ".?:$":
            parts.append(val)
        elif kind == "close" and val == ")":
            parts.append("()")
        elif kind == "open" and val == "(":
            parts.append("()")
        else:
            break
        j -= 1
    return "".join(reversed(parts))

def args_text(text, open_idx, tokens):
    # find matching close paren for the open paren token
    depth = 0
    for t in tokens[open_idx:]:
        kind, val, s, e = t
        if kind == "open" and val == "(":
            depth += 1
        elif kind == "close" and val == ")":
            depth -= 1
            if depth == 0:
                return text[tokens[open_idx][2] + 1:e - 1]
    return ""

def detect_method(text, open_idx, tokens, call):
    # explicit method in options
    atext = args_text(text, open_idx, tokens)
    m = re.search(r"method\s*[:=]\s*['\"](GET|POST|PUT|PATCH|DELETE)['\"]", atext, re.I)
    if m:
        return m.group(1).upper()
    # accessor chain: useApi().get / .post / .put / .patch / .delete / .fetch
    mm = re.search(r"\.(get|post|put|patch|delete|fetch)\s*$", call, re.I)
    if mm:
        return "GET" if mm.group(1).lower() == "fetch" else mm.group(1).upper()
    # data hooks default to GET
    if re.search(r"(useFetch|useApiFetch|useApiData|fetch)$", call):
        return "GET"
    return "GET"

def extract_calls(text):
    tokens, _ = tokenize(text)
    calls = []
    for idx, (kind, val, s, e) in enumerate(tokens):
        if kind not in ("str", "tmpl"):
            continue
        content = val[1:-1]  # strip quotes
        if content.startswith("/admin/"):
            # Panel calls `/admin/...` relative to the `/api/v1` base (useApi).
            content = "/api/v1" + content
        if not (content.startswith("/api/v1") or content.startswith("/ws/v1")):
            continue
        path = content
        path = re.sub(r"\$\{[^}]*\}", "{param}", path)
        # skip doc/comment placeholders that slipped through (e.g. "...", "*")
        if "..." in path or "*" in path:
            continue
        # find innermost enclosing call paren
        open_idx = None
        depth = 0
        for j in range(idx - 1, -1, -1):
            k, v, _, _ = tokens[j]
            if k == "open" and v == "(":
                if depth == 0:
                    open_idx = j
                    break
                depth -= 1
            elif k == "close" and v == ")":
                depth += 1
        method = "GET"
        if open_idx is not None:
            call = call_name_before(text, open_idx, tokens)
            method = detect_method(text, open_idx, tokens, call)
        line = text.count("\n", 0, s) + 1
        calls.append((method, path, line))
    return calls

# Collect + dedupe calls
calls = {}
skip_dirs = {"node_modules", ".nuxt", ".output", "dist", "coverage", ".git", ".next", "tests", "__tests__"}
for dirpath, dirnames, filenames in os.walk(src_dir):
    dirnames[:] = [d for d in dirnames if d not in skip_dirs]
    for fn in filenames:
        f = pathlib.Path(dirpath) / fn
        # only app source; ignore generated types and test/mock files
        if f.suffix not in {".ts", ".vue", ".js"}:
            continue
        if "types/generated.ts" in str(f) or fn.endswith((".spec.ts", ".test.ts", ".spec.js", ".test.js")):
            continue
        try:
            text = f.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue
        for method, path, line in extract_calls(text):
            path = path.split("?", 1)[0]
            if not path or path == "/api/v1" or path == "/ws/v1":
                continue
            calls.setdefault((method, path), set()).add((str(f), line))

fails = []
warns = []
total = 0
for (method, path), locs in sorted(calls.items()):
    total += 1
    if path.startswith("/ws/v1/"):
        ok = any(path_matches(path, w) for w in WS_ALLOW)
        label = "ws"
    else:
        item = paths.get(path)
        if item is not None and method.lower() in openapi_methods(item):
            ok = True
        else:
            ok = any(
                path_matches(path, op) and method.lower() in openapi_methods(it)
                for op, it in paths.items()
            )
        label = "http"
    if not ok and path in ALIAS_ALLOW:
        ok = True
        for f, ln in sorted(locs):
            warns.append(f"{f}:{ln}: {method} {path}  (alias -> canonical)")
    if not ok:
        for f, ln in sorted(locs):
            fails.append(f"{f}:{ln}: {method} {path}  ({label})")

for w in warns:
    print("WARN " + w)
if fails:
    print("CONTRACT CONSISTENCY FAIL:")
    for fl in fails:
        print("  " + fl)
    print(f"checked {total} call sites, {len(fails)} FAIL")
    sys.exit(1)
else:
    print(f"contract consistency OK: {total} call sites all present in {openapi_path}")
    sys.exit(0)
PYEOF
