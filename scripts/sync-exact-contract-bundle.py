#!/usr/bin/env python3
"""Write the exact-bundle Contract manifest from committed frozen artifacts.

This is packaging/CI metadata only: it never generates API semantics. The source
of truth remains PPB OpenAPI + ErrorCode; this script binds their committed
consumer mirrors and locale files into one verifiable exact-bundle record.
"""
from __future__ import annotations
import argparse, hashlib, json
from pathlib import Path

def sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()

def load(p: Path):
    return json.loads(p.read_text())

def auth_gateway_sha(bundle: Path) -> str:
    base=bundle/'frontend-contract/auth-gateway'
    names=['tokens.json','strings.zh.json','strings.en.json','errors.zh.json','errors.en.json','consent.json','logo.png']
    h=hashlib.sha256()
    for name in names:
        h.update(name.encode()); h.update(b'\0'); h.update((base/name).read_bytes())
    return h.hexdigest()

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument('--bundle-root', type=Path, required=True)
    args = ap.parse_args()
    bundle = args.bundle_root.resolve()
    ppb = bundle/'repos/ppb/Phira-plus-Backend-main'
    ppf = bundle/'repos/ppf/Phira-plus-frontend-main'
    panel = bundle/'repos/panel/Phira-plus-panel-main'
    api_p = ppb/'contracts/openapi.json'
    types_p = ppb/'contracts/types.ts'
    errors_p = ppb/'contracts/error-codes.json'
    version = load(ppb/'contracts/contract-version.json')
    api = load(api_p)
    errors = load(errors_p).get('codes', [])
    methods = {'get','put','post','delete','patch','options','head','trace'}
    manifest = {
        'version': 1,
        'evidence_level': 'STATIC_VALIDATED',
        'source_commit': version.get('source_commit'),
        'openapi_sha256': sha(api_p),
        'canonical_generated_types_sha256': sha(types_p),
        'error_codes_sha256': sha(errors_p),
        'ppf_generated_types_sha256': sha(ppf/'src/utils/api/generated.ts'),
        'panel_generated_types_sha256': sha(panel/'types/generated.ts'),
        'ppf_locale_sha256': {
            'zh': sha(ppf/'src/i18n/zh.json'),
            'en': sha(ppf/'src/i18n/en.json'),
        },
        'panel_locale_sha256': {
            'zh': sha(panel/'i18n/zh.json'),
            'en': sha(panel/'i18n/en.json'),
        },
        'path_count': len(api.get('paths', {})),
        'operation_count': sum(1 for item in api.get('paths', {}).values() for m in item if m.lower() in methods),
        'schema_count': len(api.get('components', {}).get('schemas', {})),
        'error_code_count': len(errors),
        'frontend_design_sha256': load(bundle/'frontend-contract/version.json').get('sha256'),
        'auth_gateway_sha256': auth_gateway_sha(bundle),
        'route_surface_sha256': sha(ppb/'contracts/route-surface.json'),
        'realtime_contract_sha256': sha(ppb/'contracts/realtime.json'),
    }
    out = bundle/'contracts/contract-bundle.json'
    out.write_text(json.dumps(manifest, indent=2) + '\n')
    print(f"synced exact contract bundle: {manifest['operation_count']} ops / {manifest['schema_count']} schemas / {manifest['error_code_count']} errors")

if __name__ == '__main__':
    main()
