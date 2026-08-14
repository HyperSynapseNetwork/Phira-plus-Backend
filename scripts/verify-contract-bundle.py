#!/usr/bin/env python3
"""Verify the committed Phira+ contract bundle as one unit.

Checks canonical PPB OpenAPI/ErrorCode/generated types metadata plus, when a
bundle root is available, the frozen copies and locale coverage in PPF/Panel.
This is deliberately stricter than path-only contract scans.
"""
from __future__ import annotations
import argparse, hashlib, json, re, sys
from pathlib import Path

HTTP_METHODS={"get","put","post","delete","patch","options","head","trace"}
def sha(p:Path)->str: return hashlib.sha256(p.read_bytes()).hexdigest()
def load(p:Path): return json.loads(p.read_text())
def auth_gateway_sha(bundle:Path)->str:
    base=bundle/'frontend-contract/auth-gateway'
    names=['tokens.json','strings.zh.json','strings.en.json','errors.zh.json','errors.en.json','consent.json','logo.png']
    h=hashlib.sha256()
    for name in names:
        h.update(name.encode()); h.update(b'\0'); h.update((base/name).read_bytes())
    return h.hexdigest()
def fail(msg:str): print(f"FAIL {msg}"); raise SystemExit(1)

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--bundle-root', type=Path)
    args=ap.parse_args()
    ppb=Path(__file__).resolve().parents[1]
    bundle=args.bundle_root.resolve() if args.bundle_root else None
    openapi_p=ppb/'contracts/openapi.json'
    types_p=ppb/'contracts/types.ts'
    errors_p=ppb/'contracts/error-codes.json'
    version_p=ppb/'contracts/contract-version.json'
    api=load(openapi_p); errors=load(errors_p).get('codes',[]); version=load(version_p)
    api_codes=api.get('components',{}).get('schemas',{}).get('ErrorCode',{}).get('enum',[])
    if api_codes != errors: fail(f"ErrorCode enum != error-codes manifest: api={len(api_codes)} manifest={len(errors)}")
    if version.get('openapi_sha256') != sha(openapi_p) or version.get('sha256') != sha(openapi_p): fail('contract-version OpenAPI hash mismatch')
    if version.get('generated_types_sha256') != sha(types_p): fail('contract-version generated types hash mismatch')
    if version.get('error_codes_sha256') != sha(errors_p): fail('contract-version error-codes hash mismatch')
    if version.get('error_code_count') != len(errors): fail('contract-version error_code_count mismatch')
    if version.get('path_count') != len(api.get('paths',{})): fail('contract-version path_count mismatch')
    ops=sum(1 for item in api.get('paths',{}).values() for method in item if method.lower() in HTTP_METHODS)
    if version.get('operation_count') != ops: fail('contract-version operation_count mismatch')
    if version.get('schema_count') != len(api.get('components',{}).get('schemas',{})): fail('contract-version schema_count mismatch')
    # Generated TS must contain every ErrorCode literal in its ErrorCode union/schema.
    ts=types_p.read_text(errors='ignore')
    missing=[c for c in errors if c not in ts]
    if missing: fail(f"generated types missing ErrorCode values: {missing}")
    if bundle:
        copies=[
            bundle/'contracts/openapi.json',
            bundle/'repos/ppb/Phira-plus-Backend-main/contracts/openapi.json',
            bundle/'repos/ppf/Phira-plus-frontend-main/contracts/openapi.json',
            bundle/'repos/panel/Phira-plus-panel-main/contracts/openapi.json',
        ]
        canonical=openapi_p.read_bytes()
        for p in copies:
            if not p.exists() or p.read_bytes()!=canonical: fail(f"frozen OpenAPI copy mismatch: {p}")
        versions=[
            bundle/'contracts/contract-version.json',
            bundle/'repos/ppb/Phira-plus-Backend-main/contracts/contract-version.json',
            bundle/'repos/ppf/Phira-plus-frontend-main/contracts/contract-version.json',
            bundle/'repos/panel/Phira-plus-panel-main/contracts/contract-version.json',
        ]
        canonical_version=version_p.read_bytes()
        for p in versions:
            if not p.exists() or p.read_bytes()!=canonical_version: fail(f"contract-version copy mismatch: {p}")
        error_manifests=[
            bundle/'contracts/error-codes.json',
            bundle/'repos/ppb/Phira-plus-Backend-main/contracts/error-codes.json',
            bundle/'repos/ppf/Phira-plus-frontend-main/contracts/error-codes.json',
            bundle/'repos/panel/Phira-plus-panel-main/contracts/error-codes.json',
        ]
        canonical_errors=errors_p.read_bytes()
        for p in error_manifests:
            if not p.exists() or p.read_bytes()!=canonical_errors: fail(f"error-codes copy mismatch: {p}")
        root_types=bundle/'contracts/types.ts'
        if not root_types.exists() or root_types.read_bytes()!=types_p.read_bytes(): fail('root canonical generated types copy mismatch')
        bundle_manifest_p=bundle/'contracts/contract-bundle.json'
        if not bundle_manifest_p.exists(): fail('exact bundle contract manifest missing')
        bm=load(bundle_manifest_p)
        expected_hashes={
            'openapi_sha256': sha(openapi_p),
            'canonical_generated_types_sha256': sha(types_p),
            'error_codes_sha256': sha(errors_p),
            'ppf_generated_types_sha256': sha(bundle/'repos/ppf/Phira-plus-frontend-main/src/utils/api/generated.ts'),
            'panel_generated_types_sha256': sha(bundle/'repos/panel/Phira-plus-panel-main/types/generated.ts'),
        }
        for key,value in expected_hashes.items():
            if bm.get(key)!=value: fail(f'exact bundle manifest hash mismatch: {key}')
        expected_counts={
            'path_count': len(api.get('paths',{})),
            'operation_count': ops,
            'schema_count': len(api.get('components',{}).get('schemas',{})),
            'error_code_count': len(errors),
        }
        for key,value in expected_counts.items():
            if bm.get(key)!=value: fail(f'exact bundle manifest count mismatch: {key}')
        design_version=load(bundle/'frontend-contract/version.json')
        if bm.get('frontend_design_sha256')!=design_version.get('sha256'): fail('exact bundle frontend design hash mismatch')
        if bm.get('auth_gateway_sha256')!=auth_gateway_sha(bundle): fail('exact bundle auth gateway hash mismatch')
        if bm.get('route_surface_sha256')!=sha(ppb/'contracts/route-surface.json'): fail('exact bundle route surface hash mismatch')
        if bm.get('realtime_contract_sha256')!=sha(ppb/'contracts/realtime.json'): fail('exact bundle realtime contract hash mismatch')
        root_route=bundle/'contracts/route-surface.json'
        root_rt=bundle/'contracts/realtime.json'
        if not root_route.exists() or root_route.read_bytes()!=(ppb/'contracts/route-surface.json').read_bytes(): fail('root route-surface copy mismatch')
        if not root_rt.exists() or root_rt.read_bytes()!=(ppb/'contracts/realtime.json').read_bytes(): fail('root realtime contract copy mismatch')
        # Product-language subset is copied into PPB; exact bundle must not drift.
        for name in ['tokens.json','strings.zh.json','strings.en.json','errors.zh.json','errors.en.json','consent.json','logo.png']:
            a=bundle/'frontend-contract/auth-gateway'/name
            b=bundle/'repos/ppb/Phira-plus-Backend-main/contracts/auth-gateway'/name
            if not b.exists() or a.read_bytes()!=b.read_bytes(): fail(f'Auth Gateway contract copy mismatch: {name}')
        # PPF/Panel Design Contract mirrors must be byte-identical to the root contract files.
        for app in ['ppf/Phira-plus-frontend-main','panel/Phira-plus-panel-main']:
            mirror=bundle/'repos'/app/'contracts/frontend-design'
            if not (mirror/'version.json').exists() or (mirror/'version.json').read_bytes()!=(bundle/'frontend-contract/version.json').read_bytes(): fail(f'frontend design version mirror mismatch: {app}')
            for name in design_version.get('files',[]):
                if not (mirror/name).exists() or (mirror/name).read_bytes()!=(bundle/'frontend-contract'/name).read_bytes(): fail(f'frontend design mirror mismatch {app}: {name}')
        for gp in [bundle/'repos/ppf/Phira-plus-frontend-main/src/utils/api/generated.ts', bundle/'repos/panel/Phira-plus-panel-main/types/generated.ts']:
            gtext=gp.read_text(errors='ignore')
            miss=[c for c in errors if c not in gtext]
            if miss: fail(f'consumer generated types missing ErrorCode values {gp}: {miss}')

        locale_groups={
            'ppf_locale_sha256': {
                'zh': bundle/'repos/ppf/Phira-plus-frontend-main/src/i18n/zh.json',
                'en': bundle/'repos/ppf/Phira-plus-frontend-main/src/i18n/en.json',
            },
            'panel_locale_sha256': {
                'zh': bundle/'repos/panel/Phira-plus-panel-main/i18n/zh.json',
                'en': bundle/'repos/panel/Phira-plus-panel-main/i18n/en.json',
            },
        }
        for manifest_key, group in locale_groups.items():
            manifest_hashes=bm.get(manifest_key,{})
            for locale,p in group.items():
                if manifest_hashes.get(locale)!=sha(p): fail(f'exact bundle locale hash mismatch: {manifest_key}.{locale}')
                keys=load(p).get('errors',{}).get('api',{})
                miss=[c for c in errors if c not in keys]
                extra=[c for c in keys if c not in errors]
                if miss or extra: fail(f"locale ErrorCode coverage mismatch {p}: missing={miss} extra={extra}")
    print(f"contract-bundle gate passed: {len(api.get('paths',{}))} paths; {ops} operations; {len(api.get('components',{}).get('schemas',{}))} schemas; {len(errors)} ErrorCodes")

if __name__=='__main__': main()
