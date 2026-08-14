#!/usr/bin/env python3
"""Provider-side Runtime Route Surface gate.

Proves that every mounted Axum method/path is either:
  A) an exact Frozen OpenAPI operation, or
  B) an exact declared non-REST/infrastructure exception, or
  C) an explicit compatibility alias.

Exact means exact parameter names too: {room_id} != {room_uuid}.
This parser intentionally targets this repository's explicit Router builder
functions; adding a new router module requires adding one ROUTER_GROUP entry,
which is itself reviewable provider-boundary governance.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "crates/ppb-server/src"
OPENAPI = ROOT / "contracts/openapi.json"
SURFACE = ROOT / "contracts/route-surface.json"
METHODS = {"get", "post", "put", "delete", "patch"}

# (relative source file, router function, mounted prefix, special crop)
ROUTER_GROUPS = [
    ("app.rs", "build_router", "/api/v1", "api"),
    ("public/routes.rs", "routes", "/api/v1/public", None),
    ("auth/routes.rs", "routes", "/api/v1/auth", None),
    ("auth/routes.rs", "root_routes", "/api/v1/admin", None),
    ("admin/routes.rs", "routes", "/api/v1/admin", None),
    ("rooms/routes.rs", "routes", "/api/v1", None),
    ("rooms/routes.rs", "admin_routes", "/api/v1/admin", None),
    ("phira/routes.rs", "routes", "/api/v1", None),
    ("replay/routes.rs", "routes", "/api/v1", None),
    ("social/routes.rs", "routes", "/api/v1", None),
    ("notifications/routes.rs", "routes", "/api/v1", None),
    ("preferences/routes.rs", "routes", "/api/v1", None),
    ("admin/coupons.rs", "user_routes", "/api/v1", None),
    ("audit/routes.rs", "routes", "/api/v1/admin", None),
    ("config/routes.rs", "routes", "/api/v1/admin", None),
    ("logs/routes.rs", "routes", "/api/v1/admin", None),
    ("admin/server.rs", "routes", "/api/v1/admin", None),
    ("admin/plugins.rs", "routes", "/api/v1/admin", None),
    ("admin/notifications.rs", "routes", "/api/v1/admin", None),
    ("admin/coupons.rs", "routes", "/api/v1/admin", None),
    ("automation/routes.rs", "routes", "/api/v1/admin/automation", None),
    ("jobs/routes.rs", "routes", "/api/v1/admin", None),
    ("permissions/routes.rs", "routes", "/api/v1/admin", None),
    ("actions/routes.rs", "routes", "/api/v1/admin", None),
    ("users/routes.rs", "admin_routes", "/api/v1/admin", None),
]

ROOT_ROUTES = [
    ("GET", "/ws/v1/rooms/{room_id}/live"),
    ("GET", "/ws/v1/replays/{round_uuid}"),
    ("GET", "/auth/phira/login"),
    ("GET", "/healthz"),
]


def fail(msg: str) -> None:
    print(f"FAIL route-surface: {msg}", file=sys.stderr)
    raise SystemExit(1)


def find_function_body(text: str, name: str) -> str:
    pat = re.compile(rf"\b(?:pub\s+)?fn\s+{re.escape(name)}\s*\([^)]*\)\s*->[^{{]+{{")
    m = pat.search(text)
    if not m:
        fail(f"router function not found: {name}")
    start = text.find("{", m.start())
    depth = 0
    i = start
    in_string = False
    escaped = False
    in_line_comment = False
    block_depth = 0
    while i < len(text):
        c = text[i]
        n = text[i + 1] if i + 1 < len(text) else ""
        if in_line_comment:
            if c == "\n":
                in_line_comment = False
            i += 1
            continue
        if block_depth:
            if c == "/" and n == "*":
                block_depth += 1
                i += 2
                continue
            if c == "*" and n == "/":
                block_depth -= 1
                i += 2
                continue
            i += 1
            continue
        if in_string:
            if escaped:
                escaped = False
            elif c == "\\":
                escaped = True
            elif c == '"':
                in_string = False
            i += 1
            continue
        if c == "/" and n == "/":
            in_line_comment = True
            i += 2
            continue
        if c == "/" and n == "*":
            block_depth = 1
            i += 2
            continue
        if c == '"':
            in_string = True
            i += 1
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[start + 1 : i]
        i += 1
    fail(f"unbalanced function body: {name}")
    return ""


def extract_route_calls(body: str) -> list[tuple[str, list[str]]]:
    out: list[tuple[str, list[str]]] = []
    i = 0
    while True:
        start = body.find(".route(", i)
        if start < 0:
            return out
        pos = start + len(".route(")
        depth = 1
        in_string = False
        escaped = False
        while pos < len(body) and depth:
            c = body[pos]
            if in_string:
                if escaped:
                    escaped = False
                elif c == "\\":
                    escaped = True
                elif c == '"':
                    in_string = False
            else:
                if c == '"':
                    in_string = True
                elif c == "(":
                    depth += 1
                elif c == ")":
                    depth -= 1
            pos += 1
        if depth:
            fail("unbalanced .route(...) call")
        args = body[start + len(".route(") : pos - 1]
        # first top-level comma separates path and MethodRouter expression
        nested = 0
        quote = False
        escaped = False
        split = None
        for idx, c in enumerate(args):
            if quote:
                if escaped:
                    escaped = False
                elif c == "\\":
                    escaped = True
                elif c == '"':
                    quote = False
            else:
                if c == '"':
                    quote = True
                elif c == "(":
                    nested += 1
                elif c == ")":
                    nested -= 1
                elif c == "," and nested == 0:
                    split = idx
                    break
        if split is None:
            fail(f"cannot split route args: {args[:120]}")
        path_arg = args[:split].strip()
        expr = args[split + 1 :]
        pm = re.fullmatch(r'"([^"\\]+)"', path_arg)
        if not pm:
            fail(f"route path must be a literal for provider-boundary verification: {path_arg}")
        methods = [m.lower() for m in re.findall(r"\b(get|post|put|delete|patch)\s*\(", expr)]
        if not methods:
            fail(f"route has no recognized method: {pm.group(1)}")
        out.append((pm.group(1), methods))
        i = pos


def runtime_routes() -> set[tuple[str, str]]:
    result: set[tuple[str, str]] = set()
    for rel, fn, prefix, special in ROUTER_GROUPS:
        text = (SRC / rel).read_text(encoding="utf-8")
        body = find_function_body(text, fn)
        if special == "api":
            begin = body.find("let api =")
            end = body.find("let cors", begin)
            if begin < 0 or end < 0:
                fail("build_router api crop markers missing")
            body = body[begin:end]
        for path, methods in extract_route_calls(body):
            full = prefix.rstrip("/") + path if path.startswith("/") else prefix.rstrip("/") + "/" + path
            for method in methods:
                result.add((method.upper(), full))
    result.update(ROOT_ROUTES)
    return result


def openapi_routes() -> set[tuple[str, str]]:
    doc = json.loads(OPENAPI.read_text(encoding="utf-8"))
    out: set[tuple[str, str]] = set()
    for path, item in doc.get("paths", {}).items():
        for method in item:
            if method.lower() in METHODS:
                out.add((method.upper(), path))
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    runtime = runtime_routes()
    frozen = openapi_routes()
    manifest = json.loads(SURFACE.read_text(encoding="utf-8"))
    exceptions = {(e["method"].upper(), e["path"]) for e in manifest.get("exceptions", [])}
    aliases = {(e["method"].upper(), e["path"]) for e in manifest.get("compatibility_aliases", [])}

    if exceptions & frozen:
        fail(f"exception also exists as frozen REST: {sorted(exceptions & frozen)}")
    if aliases & frozen:
        fail(f"compatibility alias duplicates frozen operation: {sorted(aliases & frozen)}")
    if not exceptions <= runtime:
        fail(f"stale exceptions not mounted: {sorted(exceptions - runtime)}")
    if not aliases <= runtime:
        fail(f"stale aliases not mounted: {sorted(aliases - runtime)}")

    undeclared = runtime - frozen - exceptions - aliases
    missing = frozen - runtime
    if undeclared:
        fail(f"runtime operations outside frozen/exception surface: {sorted(undeclared)}")
    if missing:
        fail(f"frozen operations not mounted at runtime: {sorted(missing)}")

    if args.self_test:
        # Exact semantic parameter names matter; an old room_uuid path must not
        # be accepted merely because its placeholder shape matches room_id.
        probe = ("GET", "/ws/v1/rooms/{room_uuid}/live")
        if probe in exceptions or probe in frozen or probe in aliases:
            fail("self-test fixture unexpectedly accepted room_uuid semantic")
        print("route-surface self-test passed: semantic path parameter mismatch is rejected")
        return

    print(
        "route-surface gate passed:\n"
        f"  runtime methods: {len(runtime)}\n"
        f"  frozen REST:     {len(frozen)}\n"
        f"  exceptions:      {len(exceptions)}\n"
        f"  aliases:         {len(aliases)}\n"
        "  undeclared:      0\n"
        "  missing:         0"
    )


if __name__ == "__main__":
    main()
