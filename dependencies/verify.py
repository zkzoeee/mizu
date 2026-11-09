#!/usr/bin/env python3
"""Check fork pins, ancestry, bundles and a clean dependency checkout."""
import hashlib, json, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def git(repo, *args): return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
lock = json.loads((ROOT/'dependencies/lock.json').read_text())
for d in lock['dependencies']:
    repo = ROOT/d['path']
    assert git(repo, 'rev-parse', 'HEAD') == d['revision'], d['path']
    assert git(repo, 'rev-parse', 'HEAD^{tree}') == d['tree'], d['path']
    assert not git(repo, 'status', '--porcelain'), d['path']+' has local changes'
    subprocess.run(['git', '-C', str(repo), 'merge-base', '--is-ancestor', d['base'], d['revision']], check=True)
    assert git(ROOT, 'ls-files', '--stage', '--', d['path']).split()[:2] == ['160000', d['revision']]
    assert hashlib.sha256((ROOT/d['bundle']).read_bytes()).hexdigest() == d['bundle_sha256']
    subprocess.run(['git', '-C', str(repo), 'bundle', 'verify', str(ROOT/d['bundle'])], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print('Verified three dependency forks: pins, source trees, clean state, ancestry and recovery bundles.')
