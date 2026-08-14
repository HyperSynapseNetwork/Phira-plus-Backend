#!/usr/bin/env python3
"""Validate PPB's standalone Auth Gateway product-language contract subset."""
from pathlib import Path
import hashlib,json
root=Path(__file__).resolve().parents[1]
g=root/'contracts/auth-gateway'
required=['tokens.json','strings.zh.json','strings.en.json','errors.zh.json','errors.en.json','consent.json','logo.png']
missing=[x for x in required if not (g/x).exists()]
if missing: raise SystemExit(f'FAIL missing auth-gateway contract files: {missing}')
for name in required:
    if name.endswith('.json'):
        json.loads((g/name).read_text())
zh=json.loads((g/'strings.zh.json').read_text()); en=json.loads((g/'strings.en.json').read_text())
if set(zh)!=set(en): raise SystemExit('FAIL auth-gateway zh/en string keys differ')
expected_strings={'documentTitle','product','title','subtitle','email','password','signIn','github','or','consentPrefix','terms','privacy','consentJoin','consentRequired','legalUnavailable','networkError','genericError','requestId','githubHint','clientPpf','clientPanel','language','zh','en'}
if set(zh)!=expected_strings: raise SystemExit(f'FAIL auth-gateway string schema drift: missing={sorted(expected_strings-set(zh))} extra={sorted(set(zh)-expected_strings)}')
tokens=json.loads((g/'tokens.json').read_text())
expected_tokens={'canvas','surface','surfaceStrong','border','textPrimary','textSecondary','accent','accentText','danger','focus','radiusControlPx','radiusWindowPx','maxWidthPx'}
if set(tokens)!=expected_tokens: raise SystemExit(f'FAIL auth-gateway token schema drift: missing={sorted(expected_tokens-set(tokens))} extra={sorted(set(tokens)-expected_tokens)}')
ez=json.loads((g/'errors.zh.json').read_text()); ee=json.loads((g/'errors.en.json').read_text())
if set(ez)!=set(ee): raise SystemExit('FAIL auth-gateway zh/en error keys differ')
server=set(json.loads((root/'contracts/error-codes.json').read_text())['codes'])
unknown=sorted(set(ez)-server)
if unknown: raise SystemExit(f'FAIL auth-gateway error locale has unknown ErrorCode(s): {unknown}')
consent=json.loads((g/'consent.json').read_text())
required_fields={'accepted','terms_version','privacy_version'}
if not required_fields.issubset(set(consent.get('fields',[]))): raise SystemExit('FAIL auth-gateway consent field contract incomplete')
for key in ['persistence','publicAuthRule','cookieConsentIsNotAccountLegalAcceptance']:
    if key not in consent: raise SystemExit(f'FAIL auth-gateway consent contract missing {key}')
# Gateway runtime must consume generated contract data safely; future localized
# strings must not be able to terminate the inline <script>.
gateway=(root/'crates/ppb-server/src/auth/gateway.rs').read_text()
for literal in ['\\\\u0026','\\\\u003c','\\\\u003e','\\\\u2028','\\\\u2029']:
    if f'\"{literal}\"' not in gateway:
        raise SystemExit(f'FAIL auth-gateway inline JSON escape invariant missing: {literal}')
if 'const messages={error_map};' not in gateway:
    raise SystemExit('FAIL auth-gateway runtime no longer consumes generated ErrorCode locale map')
if '<html lang="{locale}">' not in gateway:
    raise SystemExit('FAIL auth-gateway runtime lost locale-aware document language')
for invariant in [
    'content-security-policy',
    'no-store, max-age=0',
    'no-referrer',
    'nosniff',
    'x-frame-options',
    'permissions-policy',
    '<style nonce="{nonce}">',
    '<script nonce="{nonce}">',
    'aria-busy="false"',
    'function setBusy(value)',
]:
    if invariant not in gateway:
        raise SystemExit(f'FAIL auth-gateway security/busy invariant missing: {invariant}')
if "'unsafe-inline'" in gateway:
    raise SystemExit('FAIL auth-gateway CSP must not rely on unsafe-inline')
github=(root/'crates/ppb-server/src/auth/github.rs').read_text()
for raw in ['format!("github token:', 'format!("github user:', 'ErrorCode::PhiraApiUnavailable', 'ErrorCode::PhiraAuthFailed']:
    if raw in github:
        raise SystemExit(f'FAIL GitHub auth reuses/raw-leaks legacy provider semantics: {raw}')
for code in ['GithubOauthFailed','GithubApiUnavailable','GithubOauthStateInvalid']:
    if code not in github:
        raise SystemExit(f'FAIL GitHub auth missing domain ErrorCode use: {code}')
# Stable local subset digest for diagnostics/release provenance.
h=hashlib.sha256()
for name in required:
    h.update(name.encode()); h.update(b'\0'); h.update((g/name).read_bytes())
print(f'auth-gateway contract gate passed: {len(zh)} shared strings; {len(ez)} ErrorCode strings; sha256={h.hexdigest()}')
