import json, re, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'coverage/owner-auth'
BASE = '669418ccea9b15212abb81bc8ce3db0fbd18c646'
FILES = [
'apps/frontend/src/adapters/web/account-auth.ts',
'apps/frontend/src/context/auth-context.tsx',
'apps/frontend/src/pages/auth/account-auth-page.tsx',
'apps/frontend/src/pages/auth/login-page.tsx',
'apps/server/src/account_auth.rs',
'apps/server/src/auth.rs',
'apps/server/src/config.rs',
'scripts/create-owner-invitation.mjs',
]
def parse_lcov(path, prefix=''):
    result = {}
    if not path.exists(): return result
    current = None
    for line in path.read_text().splitlines():
        if line.startswith('SF:'):
            name = line[3:]
            if name.startswith(str(ROOT) + '/'): name = name[len(str(ROOT))+1:]
            elif prefix and not name.startswith(prefix): name = prefix + name
            current = result.setdefault(name, {'lines': {}, 'branches': {}})
        elif current is not None and line.startswith('DA:'):
            n, count, *_ = line[3:].split(',')
            current['lines'][int(n)] = max(current['lines'].get(int(n), 0), int(count))
        elif current is not None and line.startswith('BRDA:'):
            n, block, branch, count = line[5:].split(',')
            key = (int(n), block, branch)
            current['branches'][key] = max(current['branches'].get(key, 0), int(count) if count != '-' else 0)
    return result
reports = {}
reports.update(parse_lcov(OUT/'frontend/lcov.info', 'apps/frontend/'))
reports.update(parse_lcov(OUT/'server.lcov'))
reports.update(parse_lcov(OUT/'script.lcov'))
def metric(values):
    values = list(values)
    hit = sum(v > 0 for v in values)
    return {'covered': hit, 'total': len(values), 'percent': round(100*hit/len(values), 2) if values else None}
rows = []
for name in FILES:
    if name not in reports:
        raise SystemExit('Missing coverage for ' + name)
    if not reports[name]['lines'] or not reports[name]['branches']:
        raise SystemExit('Missing line or branch instrumentation for ' + name)
    source = (ROOT/name).read_text().splitlines()
    cutoff = next((i for i, line in enumerate(source, 1) if line == '#[cfg(test)]'), len(source)+1)
    excluded = set(range(cutoff, len(source)+1)) if name.endswith('.rs') else set()
    # A test-only helper inside AccountAuth's production impl is also excluded.
    if name.endswith('account_auth.rs'):
        start = next(i for i, line in enumerate(source, 1) if 'pub(crate) fn test_reset_password' in line)
        end = next(i for i in range(start+1, len(source)+1) if source[i-1] == '    }')
        excluded.update(range(start-1, end+1))
    diff = subprocess.check_output(['git', 'diff', '--unified=0', BASE, 'HEAD', '--', name], cwd=ROOT, text=True)
    added = set()
    for match in re.finditer(r'^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@', diff, re.M):
        start = int(match[1]); count = int(match[2]) if match[2] is not None else 1
        added.update(range(start, start+count))
    lines = {n:v for n,v in reports[name]['lines'].items() if n not in excluded}
    branches = {k:v for k,v in reports[name]['branches'].items() if k[0] not in excluded}
    rows.append({'file': name,
      'lines': metric(lines.values()), 'branches': metric(branches.values()),
      'added_lines': metric(v for n,v in lines.items() if n in added),
      'added_branches': metric(v for k,v in branches.items() if k[0] in added),
      'uncovered_added_lines': [n for n,v in lines.items() if n in added and v == 0],
      'uncovered_branch_lines': sorted(set(k[0] for k,v in branches.items() if v == 0)),
    })
summary = {'base': BASE, 'head': subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(), 'files':rows}
for group, selected in [('all', rows),('frontend',[r for r in rows if r['file'].startswith('apps/frontend')]),('server',[r for r in rows if r['file'].startswith('apps/server')]),('script',[r for r in rows if r['file'].startswith('scripts')])]:
    summary[group] = {}
    for field in ['lines','branches','added_lines','added_branches']:
        covered = sum(r.get(field,{}).get('covered',0) for r in selected)
        total = sum(r.get(field,{}).get('total',0) for r in selected)
        summary[group][field] = {'covered':covered,'total':total,'percent':round(100*covered/total,2) if total else None}
(OUT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))

# Keep this baseline fixed: this is the owner-auth feature's added-code gate,
# not a claim that the entire repository has 95% coverage.
failed = False
for field in ['added_lines', 'added_branches']:
    value = summary['all'][field]
    print(f"{field}: {value['covered']}/{value['total']} ({value['percent']}%)")
    if not value['total'] or value['covered'] * 100 < 95 * value['total']:
        failed = True
raise SystemExit(1 if failed else 0)
