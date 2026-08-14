#!/usr/bin/env python3
from __future__ import annotations
from pathlib import Path
import json,re,sys
ROOT=Path(__file__).resolve().parents[1]
files=[ROOT/'README.md']
docs=ROOT/'docs'
if docs.exists(): files += [p for p in docs.rglob('*.md') if 'history' not in p.parts]
files=[p for p in files if p.exists()]
patterns=[
 (re.compile(r'已由\s*CI\s*全量验证'), 'permanent CI-verified claim'),
 (re.compile(r'\bPhase\s+[A-Z]\b',re.I), 'historical Phase used as current status'),
 (re.compile(r'\bPhase[0-9][A-Za-z0-9_-]*\b',re.I), 'historical Phase marker used as current status'),
 (re.compile(r'OWNER\s+LATER|This page is a placeholder|本页面为占位内容',re.I), 'placeholder/owner-later current documentation'),
 (re.compile(r'/ws/v1/rooms/\{room_uuid\}/live'), 'wrong Live WS identity; must use room_id'),
 (re.compile(r'OpenAPI[^\n]{0,30}(?:落地后|生成后|以后).{0,20}(?:为准|source)',re.I), 'stale future-OpenAPI wording'),
]
fail=[]
for p in sorted(set(files)):
 for n,line in enumerate(p.read_text(errors='ignore').splitlines(),1):
  if 'docs/history/' in line or '历史计划' in line: continue
  for rx,label in patterns:
   if rx.search(line): fail.append(f'{p.relative_to(ROOT)}:{n}: {label}: {line.strip()}')
# Contract-backed method/path rows in docs/api.md.
api=json.loads((ROOT/'contracts/openapi.json').read_text())
methods={'get','put','post','delete','patch','options','head','trace'}
known={(m.upper(),path) for path,item in api.get('paths',{}).items() for m in item if m.lower() in methods}
surface=json.loads((ROOT/'contracts/route-surface.json').read_text())
known |= {(e['method'].upper(),e['path']) for e in surface.get('exceptions',[])}
known |= {(e['method'].upper(),e['path']) for e in surface.get('compatibility_aliases',[])}
row_re=re.compile(r'^\|\s*(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s*\|\s*`([^`]+)`\s*\|')
api_doc=ROOT/'docs/api.md'
for n,line in enumerate(api_doc.read_text().splitlines(),1):
 m=row_re.match(line)
 if m and (m.group(1),m.group(2)) not in known:
  fail.append(f'docs/api.md:{n}: method/path not present in OpenAPI or route-surface exception: {m.group(1)} {m.group(2)}')
# Realtime paths in docs must exactly match typed realtime registry.
rt=json.loads((ROOT/'contracts/realtime.json').read_text())['channels']
canonical={v['path'] for v in rt.values() if v['kind'] in {'sse','websocket'}}
for p in sorted(set(files)):
 for n,line in enumerate(p.read_text(errors='ignore').splitlines(),1):
  for path in re.findall(r'/(?:ws|api)/[^\s`|)>,]+', line):
   if ('/ws/' in path or path.endswith('/events') or path.endswith('/logs/stream')) and ('{' in path or '/events' in path or '/logs/stream' in path):
    clean=path.rstrip('.,;:')
    if clean.startswith('/ws/') and clean not in canonical:
     fail.append(f'{p.relative_to(ROOT)}:{n}: realtime path not in typed contract: {clean}')
if fail:
 print('current-docs contract-backed gate failed:\n'+'\n'.join(fail),file=sys.stderr); sys.exit(1)
print(f'current-docs contract-backed gate passed: {len(set(files))} current documents; docs/api method/path rows map to current contracts')
