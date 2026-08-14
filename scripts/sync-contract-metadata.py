#!/usr/bin/env python3
"""Regenerate ErrorCode manifest and contract-version metadata from source artifacts."""
from __future__ import annotations
import argparse, hashlib, json, os, re, subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]

def sha(p:Path)->str: return hashlib.sha256(p.read_bytes()).hexdigest()
def rust_codes()->list[str]:
    text=(root/'crates/ppb-server/src/error/mod.rs').read_text()
    block=re.search(r'pub fn as_str\(&self\).*?match self \{(.*?)\n\s*\}\n\s*\}', text, re.S)
    if not block: raise SystemExit('ErrorCode::as_str block not found')
    codes=re.findall(r'=>\s*"([A-Z][A-Z0-9_]*)"', block.group(1))
    if len(codes)!=len(set(codes)): raise SystemExit('duplicate serialized ErrorCode')
    return codes

def source_commit():
    if os.getenv('GITHUB_SHA'): return os.environ['GITHUB_SHA']
    try: return subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'], text=True, stderr=subprocess.DEVNULL).strip()
    except Exception: return None

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--codes-only',action='store_true'); args=ap.parse_args()
    codes=rust_codes()
    (root/'contracts/error-codes.json').write_text(json.dumps({'version':'1.1','source':'PPB crates/ppb-server/src/error/mod.rs::ErrorCode','codes':codes},indent=2)+"\n")
    if args.codes_only:
        print(f'synced error-codes.json: {len(codes)} codes'); return
    api_p=root/'contracts/openapi.json'; types_p=root/'contracts/types.ts'
    api=json.loads(api_p.read_text())
    methods={'get','put','post','delete','patch','options','head','trace'}
    meta={
      'source':'PPB contracts/openapi.json',
      'sha256':sha(api_p),
      'openapi_sha256':sha(api_p),
      'generated_types_sha256':sha(types_p),
      'error_codes_sha256':sha(root/'contracts/error-codes.json'),
      'path_count':len(api.get('paths',{})),
      'operation_count':sum(1 for item in api.get('paths',{}).values() for m in item if m.lower() in methods),
      'schema_count':len(api.get('components',{}).get('schemas',{})),
      'error_code_count':len(codes),
      'freeze':'v0',
      'evidence_level':'STATIC_VALIDATED',
      'source_commit':source_commit(),
    }
    (root/'contracts/contract-version.json').write_text(json.dumps(meta,indent=2)+"\n")
    print(f"synced contract-version: {meta['operation_count']} ops / {meta['schema_count']} schemas / {len(codes)} errors")
if __name__=='__main__': main()
