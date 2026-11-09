#!/usr/bin/env python3
"""Restore pinned upstream-based forks. Never build, execute or prove."""
import argparse, hashlib, json, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def git(repo, *args, check=True):
    return subprocess.run(['git', '-C', str(repo), *args], check=check, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--fork-root', type=Path, default=ROOT.parent/'dependency-forks')
    p.add_argument('--offline', action='store_true')
    args = p.parse_args()
    args.fork_root.mkdir(parents=True, exist_ok=True)
    lock = json.loads((ROOT/'dependencies/lock.json').read_text())
    for d in lock['dependencies']:
        bundle = ROOT/d['bundle']; bare = args.fork_root/(d['name']+'.git')
        assert hashlib.sha256(bundle.read_bytes()).hexdigest() == d['bundle_sha256'], bundle
        if not bare.exists():
            subprocess.run(['git', 'init', '--bare', str(bare)], check=True, stdout=subprocess.DEVNULL)
            git(bare, 'remote', 'add', 'upstream', d['upstream'])
        assert git(bare, 'rev-parse', '--is-bare-repository').stdout.strip() == 'true'
        assert git(bare, 'remote', 'get-url', 'upstream').stdout.strip() == d['upstream']
        if git(bare, 'cat-file', '-e', d['base']+'^{commit}', check=False).returncode:
            if args.offline: raise SystemExit('Missing upstream base '+d['base']+'; rerun without --offline')
            git(bare, 'fetch', '--depth=1', 'upstream', d['base'])
        git(bare, 'bundle', 'verify', str(bundle))
        # Import by immutable SHA, so a local development branch is never reset.
        git(bare, 'fetch', str(bundle), 'refs/heads/'+d['branch'])
        assert git(bare, 'rev-parse', 'FETCH_HEAD').stdout.strip() == d['revision']
        git(bare, 'update-ref', 'refs/mizu/pins/'+d['revision'], d['revision'])
        branch = 'refs/heads/'+d['branch']
        if git(bare, 'show-ref', '--verify', branch, check=False).returncode:
            git(bare, 'update-ref', branch, d['revision'])
        if git(bare, 'rev-parse', '--verify', 'HEAD', check=False).returncode:
            git(bare, 'symbolic-ref', 'HEAD', branch)
        git(ROOT, 'config', 'submodule.'+d['path']+'.url', str(bare.resolve()))
        git(ROOT, '-c', 'protocol.file.allow=always', 'submodule', 'update', '--init', '--', d['path'])
        checkout = ROOT/d['path']
        git(checkout, 'config', 'remote.origin.url', str(bare.resolve()))
        git(checkout, 'config', 'remote.upstream.url', d['upstream'])
        git(checkout, 'config', 'remote.upstream.fetch', '+refs/heads/*:refs/remotes/upstream/*')
    subprocess.run(['python3', str(ROOT/'dependencies/verify.py')], check=True)
if __name__ == '__main__': main()
