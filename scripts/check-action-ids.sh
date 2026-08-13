#!/usr/bin/env bash
# Action ID consistency check (contract §23 #2).
#
# The PPB Action Registry (`crates/ppb-server/src/actions/registry.rs`) is the
# single source of Action IDs. PPF/Panel must reference only Registry IDs
# (`room.lock` / `room.kick` / `player.ban` / `server.connections` /
# `pmp.cli.execute` ...), never hand-written IDs.
#
# Usage:
#   scripts/check-action-ids.sh <registry.rs> <src_dir>
#
# Scans a frontend src dir for Action IDs referenced in two places:
#   1. an `action:` / `action =` (or JSON `"action":`) call context, and
#   2. central Action-ID definition files (basename contains `action`,
#      e.g. Panel `config/action-ids.ts`).
#
# Any dotted ID found that is NOT in the Registry is a FAIL and makes the
# script exit non-zero (contract §23 #2: frontends must not invent Action IDs).
#
# Examples:
#   scripts/check-action-ids.sh crates/ppb-server/src/actions/registry.rs ../ppf/src
#   scripts/check-action-ids.sh crates/ppb-server/src/actions/registry.rs ../panel
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <registry.rs> <src_dir>" >&2
  exit 2
fi

REGISTRY="$1"
SRC_DIR="$2"
test -f "$REGISTRY" || { echo "registry file not found: $REGISTRY" >&2; exit 2; }
test -d "$SRC_DIR" || { echo "src dir not found: $SRC_DIR" >&2; exit 2; }

python3 - "$REGISTRY" "$SRC_DIR" <<'PYEOF'
import os, pathlib, re, sys

registry_path = sys.argv[1]
src_dir = pathlib.Path(sys.argv[2])

# 1) canonical Action IDs extracted from the PPB registry source.
text = open(registry_path, encoding="utf-8").read()
registry_ids = set(re.findall(r'ActionDescriptor::new\(\s*"([^"]+)"', text))

def is_action_id(s: str) -> bool:
    return bool(re.fullmatch(r"[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*", s))

registry_ids = {i for i in registry_ids if is_action_id(i)}
if not registry_ids:
    print("ERROR: no Action IDs extracted from registry source", file=sys.stderr)
    sys.exit(2)

# 2) scan frontend source.
action_field = re.compile(r"\baction\b\s*[:=]\s*['\"]([a-z][a-z0-9_]*\.[a-z][a-z0-9_]*)['\"]")
json_action = re.compile(r"['\"]action['\"]\s*:\s*['\"]([a-z][a-z0-9_]*\.[a-z][a-z0-9_]*)['\"]")
dotted_string = re.compile(r"['\"]([a-z][a-z0-9_]*\.[a-z][a-z0-9_]*)['\"]")

skip_dirs = {"node_modules", ".nuxt", ".output", "dist", "coverage", ".git", ".next", "tests", "__tests__"}
found = {}
for dirpath, dirnames, filenames in os.walk(src_dir):
    dirnames[:] = [d for d in dirnames if d not in skip_dirs]
    for fn in filenames:
        f = pathlib.Path(dirpath) / fn
        if f.suffix not in {".ts", ".vue", ".js"}:
            continue
        if "types/generated.ts" in str(f) or fn.endswith((".spec.ts", ".test.ts", ".spec.js", ".test.js")):
            continue
        try:
            t = f.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue
        line_of = lambda pos: t.count("\n", 0, pos) + 1
        # inline `action:` call contexts (PPF sends these directly)
        for rx in (action_field, json_action):
            for m in rx.finditer(t):
                found.setdefault(m.group(1), set()).add((str(f), line_of(m.start())))
        # central Action-ID definition files (Panel `config/action-ids.ts`)
        if "action" in f.stem.lower():
            for m in dotted_string.finditer(t):
                if is_action_id(m.group(1)):
                    found.setdefault(m.group(1), set()).add((str(f), line_of(m.start())))

fails = []
for aid, locs in sorted(found.items()):
    if aid not in registry_ids:
        for f, ln in sorted(locs):
            fails.append(f"{f}:{ln}: action id `{aid}` not in PPB Action Registry")

if fails:
    print("ACTION ID CONSISTENCY FAIL:")
    for fl in fails:
        print("  " + fl)
    print(f"checked {len(found)} action ids, {len(fails)} locations unknown")
    sys.exit(1)
else:
    print(f"action ids OK: {len(found)} used, all in PPB Action Registry")
    sys.exit(0)
PYEOF
