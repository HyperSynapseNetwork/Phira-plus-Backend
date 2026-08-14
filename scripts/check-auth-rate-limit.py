#!/usr/bin/env python3
from pathlib import Path
import re,sys
ROOT=Path(__file__).resolve().parents[1]
routes=(ROOT/'crates/ppb-server/src/auth/routes.rs').read_text()
github=(ROOT/'crates/ppb-server/src/auth/github.rs').read_text()
config=(ROOT/'crates/ppb-server/src/config/mod.rs').read_text()
fail=[]
for field in ['github_start_per_minute','github_callback_per_minute','github_provider_per_minute']:
    if field not in config: fail.append(f'missing rate config field {field}')
if 'github-login-start:' not in routes or 'github_start_per_minute' not in routes: fail.append('GitHub login start lacks per-network/client rate limit')
if 'github-provider:start' not in routes or 'github-provider:callback' not in routes: fail.append('GitHub provider global bucket missing')
if re.search(r'github-callback:[^\n]*params\.code',routes): fail.append('callback bucket must not be keyed by OAuth code')
if 'MAX_PENDING_STATES' not in github or 'self.states.len() >= MAX_PENDING_STATES' not in github: fail.append('OAuth state store has no hard pending-state cap')
if fail:
    print('auth-rate-limit gate failed:\n'+'\n'.join(fail),file=sys.stderr); sys.exit(1)
print('auth-rate-limit gate passed: GitHub start/callback use endpoint/network + provider buckets; OAuth code is not a rate key; pending states capped')
