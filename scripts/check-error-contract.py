#!/usr/bin/env python3
"""Static Error Contract v1.1 gate: Rust enum -> manifest -> OpenAPI enum."""
import json,re,sys
from pathlib import Path
root=Path(__file__).resolve().parents[1]
text=(root/'crates/ppb-server/src/error/mod.rs').read_text()
# Use as_str match arms as canonical serialized values, excluding test literals.
block=re.search(r'pub fn as_str\(&self\).*?match self \{(.*?)\n\s*\}\n\s*\}', text, re.S)
if not block: raise SystemExit('FAIL: ErrorCode::as_str block not found')
codes=re.findall(r'=>\s*"([A-Z][A-Z0-9_]*)"',block.group(1))
if len(codes)!=len(set(codes)): raise SystemExit('FAIL: duplicate ErrorCode serialized values')
manifest=json.loads((root/'contracts/error-codes.json').read_text())
manifest_codes=manifest.get('codes',[])
if codes!=manifest_codes:
 print('FAIL Rust ErrorCode != manifest')
 print('rust-only',sorted(set(codes)-set(manifest_codes)))
 print('manifest-only',sorted(set(manifest_codes)-set(codes)))
 raise SystemExit(1)
openapi=json.loads((root/'contracts/openapi.json').read_text())
schemas=openapi.get('components',{}).get('schemas',{})
api_codes=schemas.get('ErrorCode',{}).get('enum',[])
if codes!=api_codes:
 print('FAIL manifest != OpenAPI ErrorCode enum')
 print('manifest-only',sorted(set(codes)-set(api_codes)))
 print('openapi-only',sorted(set(api_codes)-set(codes)))
 raise SystemExit(1)
code_schema=schemas.get('ErrorBody',{}).get('properties',{}).get('code',{})
if code_schema.get('$ref')!='#/components/schemas/ErrorCode': raise SystemExit('FAIL ErrorBody.code is not ErrorCode ref')
# Forbid direct construction of INTERNAL from exception formatting in product source.
bad=[]
for p in (root/'crates/ppb-server/src').rglob('*.rs'):
 if p.name=='mod.rs' and p.parent.name=='error': continue
 t=p.read_text(errors='ignore')
 if re.search(r'ErrorCode::InternalError\s*,\s*(?:e\.to_string\(\)|format!\([^\n]*\{e\})',t): bad.append(str(p.relative_to(root)))
if bad:
 print('FAIL raw INTERNAL exception construction:',*bad,sep='\n  '); raise SystemExit(1)
print(f'error-contract gate passed: {len(codes)} server codes; ErrorBody.code typed; INTERNAL raw-exception pattern 0')
