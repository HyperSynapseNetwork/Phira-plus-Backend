#!/usr/bin/env python3
"""Static invariants for the canonical Config security plane.

This gate does not replace Rust/DB integration. It prevents known structural
bypasses from being reintroduced: hidden legacy routers, full RuntimeConfig
serialization, plaintext raw YAML responses, missing reauth, and missing audit
hooks for canonical save/rollback.
"""
from __future__ import annotations
import argparse
import re
import sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
SRC=ROOT/'crates/ppb-server/src/config/routes.rs'

BANNED_RUNTIME_ROUTES=(
    '/config/ppb',
    '/config/pmp',
    '/config/pmp/descriptor',
    '/config/pmp/snapshots',
    '/config/public/{key}',
)
BANNED_SECRET_RESPONSE_PATTERNS=(
    'serde_json::to_value(&*state.config)',
    'Json(state.config)',
    'Json((*state.config)',
)

def fail(msg:str)->None:
    print(f'FAIL config-security: {msg}', file=sys.stderr)
    raise SystemExit(1)

def fn_body(text:str,name:str)->str:
    m=re.search(rf'pub async fn {re.escape(name)}\s*\(',text)
    if not m: fail(f'function missing: {name}')
    start=text.find('{',m.start())
    depth=0
    for i in range(start,len(text)):
        c=text[i]
        if c=='{': depth+=1
        elif c=='}':
            depth-=1
            if depth==0: return text[start+1:i]
    fail(f'unbalanced function: {name}')
    return ''

def main()->None:
    ap=argparse.ArgumentParser(); ap.add_argument('--self-test',action='store_true'); args=ap.parse_args()
    text=SRC.read_text(encoding='utf-8')
    router=text[text.index('pub fn routes()'):text.index('// Canonical config surface')]
    for path in BANNED_RUNTIME_ROUTES:
        if f'.route("{path}"' in router:
            fail(f'legacy config route mounted: {path}')
    for pat in BANNED_SECRET_RESPONSE_PATTERNS:
        if pat in text:
            fail(f'full RuntimeConfig response pattern found: {pat}')

    save=fn_body(text,'save')
    rollback=fn_body(text,'rollback')
    raw=fn_body(text,'raw')
    for name,body in [('save',save),('rollback',rollback)]:
        if 'check_reauth_header' not in body or 'ReauthRisk::Critical' not in body:
            fail(f'{name} is not server-enforced Critical reauth')
    if '"config.save"' not in save:
        fail('config.save audit hook missing')
    if '"config.rollback"' not in rollback:
        fail('config.rollback audit hook missing')
    if 'manager.read_yaml()' not in raw or '[REDACTED]' not in raw or 'pmp_config_descriptor()' not in raw:
        fail('raw config endpoint is not a descriptor-owned redacted projection')
    if 'return manager.read_yaml()' in raw or re.search(r'\bmanager\.read_yaml\(\)\s*$',raw,re.M):
        fail('raw config endpoint returns literal on-disk YAML')

    if args.self_test:
        fixture='Router::new().route("/config/ppb", get(leak))\nserde_json::to_value(&*state.config)'
        if not any(f'.route("{p}"' in fixture for p in BANNED_RUNTIME_ROUTES):
            fail('self-test failed to detect legacy route')
        if not any(p in fixture for p in BANNED_SECRET_RESPONSE_PATTERNS):
            fail('self-test failed to detect RuntimeConfig serialization')
        print('config-security self-test passed')
        return

    print('config-security gate passed: one canonical config plane; Critical reauth; audit hooks; redacted raw projection')

if __name__=='__main__': main()
