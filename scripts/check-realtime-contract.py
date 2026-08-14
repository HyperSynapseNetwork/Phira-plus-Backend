#!/usr/bin/env python3
from __future__ import annotations
import argparse, copy, json, re, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
RT=ROOT/'contracts/realtime.json'
RS=ROOT/'contracts/route-surface.json'
PARAM_RE=re.compile(r'\{([^{}]+)\}')

def load(p): return json.loads(p.read_text())
def fail(msg): print('realtime-contract gate failed: '+msg, file=sys.stderr); raise SystemExit(1)
def validate(rt, rs):
    channels=rt.get('channels',{})
    if not channels: fail('no channels')
    declared={(v.get('method','GET').upper(),v.get('path'),v.get('kind')) for v in channels.values()}
    exceptions={(e.get('method','GET').upper(),e.get('path'),e.get('kind')) for e in rs.get('exceptions',[]) if e.get('kind') in {'sse','websocket'}}
    if declared != exceptions:
        fail(f'realtime channels != route-surface realtime exceptions: only_realtime={sorted(declared-exceptions)} only_surface={sorted(exceptions-declared)}')
    for name,v in channels.items():
        path=v.get('path','')
        params=PARAM_RE.findall(path)
        spec=v.get('params',{})
        if set(params)!=set(spec): fail(f'{name}: path params {params} != params registry {sorted(spec)}')
        for p in params:
            semantic=spec[p].get('semantic')
            if not semantic: fail(f'{name}.{p}: missing semantic')
    live=channels.get('live_room',{})
    if live.get('path')!='/ws/v1/rooms/{room_id}/live': fail('live_room path must use room_id')
    if live.get('params',{}).get('room_id',{}).get('semantic')!='pmp_room_id': fail('live_room room_id semantic must be pmp_room_id')
    replay=channels.get('replay',{})
    if replay.get('path')!='/ws/v1/replays/{round_uuid}': fail('replay path must use round_uuid')

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--self-test',action='store_true'); a=ap.parse_args()
    rt=load(RT); rs=load(RS); validate(rt,rs)
    if a.self_test:
        bad=copy.deepcopy(rt); bad['channels']['live_room']['path']='/ws/v1/rooms/{room_uuid}/live'; bad['channels']['live_room']['params']={'room_uuid':{'semantic':'room_uuid'}}
        try: validate(bad,rs)
        except SystemExit: print('realtime-contract self-test passed: room_uuid semantic mismatch rejected'); return
        fail('negative fixture unexpectedly passed')
    print(f"realtime-contract gate passed: {len(rt['channels'])} channels; live room_id semantic = pmp_room_id")
if __name__=='__main__': main()
