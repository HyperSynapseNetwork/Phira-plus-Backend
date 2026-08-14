#!/usr/bin/env python3
"""Validate typed REST request extractor semantics.

This gate is intentionally narrower than `cargo check`, but it protects the
mechanical migration boundary that previously let compile-broken `ApiApiJson`
binders and response-side `ApiJson(...)` wrappers pass while reporting
"0 raw extractors".

Declared raw Axum exceptions are non-JSON-REST surfaces: the standalone HTML
Auth Gateway and the Live WebSocket handshake.
"""
from __future__ import annotations

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "crates/ppb-server/src"
EXTRACTORS = SRC / "error/extractors.rs"

RAW_SAME = re.compile(r"\b(Json|Query|Path)\([^\n]*\)\s*:\s*\1\s*<")
RAW_TYPE = re.compile(r"(?<!:):\s*(Json|Query|Path)\s*<")
RAW_TYPED_MISMATCH = re.compile(
    r"\b(Json|Query|Path)\([^\n]*?\)\s*:\s*Api(Json|Query|Path)\s*<"
)
TYPED_BINDER = re.compile(
    r"\b(Api(?:Json|Query|Path))\([^\n]*?\)\s*:\s*(Api(?:Json|Query|Path))\s*<"
)
BAD_DOUBLE_PREFIX = re.compile(r"\bApiApi(?:Json|Query|Path)\b")
RESPONSE_MISUSE = re.compile(
    r"(?:\bOk\s*\(\s*|\bSome\s*\(\s*|\breturn\s+)Api(?:Json|Query|Path)\s*\("
)

ALLOWED_RAW = {
    "auth/gateway.rs": {"Query"},
    "live/routes.rs": {"Path", "Query"},
}


def exported_typed_extractors() -> set[str]:
    text = EXTRACTORS.read_text(encoding="utf-8")
    return set(re.findall(r"pub\s+struct\s+(Api(?:Json|Query|Path))\s*<", text))


def validate_text(text: str, rel: str, exported: set[str]) -> tuple[list[tuple[str, int, str]], list[str]]:
    raw: list[tuple[str, int, str]] = []
    errors: list[str] = []
    seen: set[tuple[str, int, str]] = set()

    for rx in (RAW_SAME, RAW_TYPE):
        for m in rx.finditer(text):
            kind = m.group(1)
            line = text.count("\n", 0, m.start()) + 1
            key = (rel, line, kind)
            if key not in seen:
                seen.add(key)
                raw.append(key)

    for m in RAW_TYPED_MISMATCH.finditer(text):
        line = text.count("\n", 0, m.start()) + 1
        errors.append(
            f"{rel}:{line}: raw {m.group(1)} binder paired with typed Api{m.group(2)} extractor"
        )

    for m in TYPED_BINDER.finditer(text):
        ctor, typ = m.groups()
        line = text.count("\n", 0, m.start()) + 1
        if ctor != typ:
            errors.append(f"{rel}:{line}: typed binder constructor {ctor} does not match extractor type {typ}")
        if ctor not in exported or typ not in exported:
            errors.append(f"{rel}:{line}: typed extractor {ctor}/{typ} is not exported by error/extractors.rs")

    for m in BAD_DOUBLE_PREFIX.finditer(text):
        line = text.count("\n", 0, m.start()) + 1
        errors.append(f"{rel}:{line}: undefined typed extractor constructor {m.group(0)}")

    for m in RESPONSE_MISUSE.finditer(text):
        line = text.count("\n", 0, m.start()) + 1
        snippet = m.group(0).strip()
        errors.append(f"{rel}:{line}: request extractor used as response wrapper: {snippet}")

    return raw, errors


def self_test(exported: set[str]) -> None:
    cases = {
        "good": "async fn f(ApiJson(body): ApiJson<Body>, ApiPath(id): ApiPath<Id>) {}",
        "bad-double": "async fn f(ApiApiJson(body): ApiJson<Body>) {}",
        "bad-mismatch": "async fn f(ApiPath(body): ApiJson<Body>) {}",
        "bad-response": "fn f() { Ok(ApiJson(value)); }",
        "bad-raw": "async fn f(Json(body): Json<Body>) {}",
    }
    raw, errors = validate_text(cases["good"], "fixture.rs", exported)
    if raw or errors:
        raise SystemExit(f"SELF-TEST FAIL: valid fixture rejected: raw={raw}, errors={errors}")
    for name in ("bad-double", "bad-mismatch", "bad-response"):
        _raw, errs = validate_text(cases[name], "fixture.rs", exported)
        if not errs:
            raise SystemExit(f"SELF-TEST FAIL: {name} fixture was not rejected")
    raw, _errs = validate_text(cases["bad-raw"], "fixture.rs", exported)
    if not raw:
        raise SystemExit("SELF-TEST FAIL: raw Axum fixture was not detected")
    print("rest-extractor semantic self-test passed")


def main() -> int:
    exported = exported_typed_extractors()
    expected_exports = {"ApiJson", "ApiQuery", "ApiPath"}
    if exported != expected_exports:
        print(f"FAIL typed extractor exports changed: expected={sorted(expected_exports)} actual={sorted(exported)}")
        return 1

    if "--self-test" in sys.argv:
        self_test(exported)
        return 0

    found_raw: list[tuple[str, int, str]] = []
    errors: list[str] = []
    for path in SRC.rglob("*.rs"):
        rel = path.relative_to(SRC).as_posix()
        text = path.read_text(encoding="utf-8", errors="ignore")
        raw, errs = validate_text(text, rel, exported)
        found_raw.extend(raw)
        errors.extend(errs)

    for rel, line, kind in found_raw:
        if kind not in ALLOWED_RAW.get(rel, set()):
            errors.append(f"{rel}:{line}: raw {kind} extractor")

    expected = {(rel, kind) for rel, kinds in ALLOWED_RAW.items() for kind in kinds}
    actual = {(rel, kind) for rel, _, kind in found_raw if rel in ALLOWED_RAW}
    missing = sorted(expected - actual)

    if errors or missing:
        if errors:
            print("FAIL REST typed extractor semantics:")
            print(*errors, sep="\n  ")
        if missing:
            print("FAIL declared non-REST extractor exception disappeared; update the registry intentionally:")
            print(*[f"{rel}:{kind}" for rel, kind in missing], sep="\n  ")
        return 1

    print(
        "rest-extractor gate passed: 0 raw REST input extractors; "
        f"{len(found_raw)} declared non-REST extractor occurrences; typed binder/response semantics valid"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
