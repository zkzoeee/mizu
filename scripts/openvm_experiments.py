#!/usr/bin/env python3
"""Matched CPU-only experiments. No SDK, prover, network or GPU APIs.

Run from any directory with an empty --output. Later phases can reuse the
frozen builds. A phase always refuses stale source or runner identities.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import benchmark

ROOT = benchmark.ROOT
VARIANTS = {
    'control': ({}, [], 'o1-verifier'),
    'allocation': ({'MIZU_CIP_MODE': 'borrowed', 'MIZU_STACK_CROSS_FIELD': '1'}, [], 'o1-verifier'),
    'fused': ({'MIZU_CIP_MODE': 'fused', 'MIZU_STACK_CROSS_FIELD': '1'}, [], 'o1-verifier'),
    'digest': ({}, ['statement-digest'], 'o1-verifier'),
    'batch-control': ({}, ['batch'], 'o1-verifier-batch'),
    'batch-combined': ({'MIZU_BATCH_ACCUMULATOR': '1'}, ['batch'], 'o1-verifier-batch'),
}

def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + '\n')

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def environment():
    env = {k: v for k, v in os.environ.items() if not k.startswith(('ZEKO_', 'MIZU_'))}
    env.update(CARGO_BUILD_JOBS='1', RAYON_NUM_THREADS='1', CARGO_TARGET_DIR=str(ROOT/'target'), RUSTUP_TOOLCHAIN='1.92.0')
    return env

def build(output):
    assert not (output/'identity.json').exists(), 'build receipt already exists; use another directory'
    env = environment()
    command = ['cargo', 'build', '--offline', '--profile', 'native-test', '--locked', '-p', 'cycle-runner', '--bin', 'cycle-runner']
    benchmark.run(command, env, output/'runner-build.log')
    runner = output/'cycle-runner'
    shutil.copy2(ROOT/'target/native-test/cycle-runner', runner)
    source = benchmark.snapshot()
    for name, (settings, features, binary) in VARIANTS.items():
        config = json.loads((ROOT/'benchmarks/retained.json').read_text())
        config['name'] = f'mizu-openvm-experiment-{name}'
        config['optimization_env'].update(settings)
        config['guest'].update(features=features, binary=binary)
        path = output/name/'config.json'
        write(path, config)
        print(f'Building {name}', flush=True)
        subprocess.run([sys.executable, str(ROOT/'scripts/benchmark.py'), 'build', '--config', str(path), '--output', str(path.parent/'build')], env=env, cwd=ROOT, check=True)
    assert benchmark.snapshot() == source, 'source changed during experiment builds'
    write(output/'identity.json', dict(schema_version=1, source_sha256=source,
          runner_sha256=sha(runner), script_sha256=sha(Path(__file__)),
          proof_generated=False, fixture_policy='N repetitions of one valid mainnet proof; not a diverse batch workload'))

def check_identity(output):
    identity = json.loads((output/'identity.json').read_text())
    assert benchmark.snapshot() == identity['source_sha256'], 'source differs from frozen builds'
    assert sha(output/'cycle-runner') == identity['runner_sha256'], 'runner changed'
    assert sha(Path(__file__)) == identity['script_sha256'], 'experiment script changed'

def execute(output, variant, label, abi='boolean', count=1, extra=(), reject=False):
    check_identity(output)
    directory = output/variant
    build = directory/'build'
    receipt = json.loads((build/'build.json').read_text())
    binary = receipt['config']['guest']['binary']
    assert sha(build/binary) == receipt['elf_sha256'], 'ELF changed'
    target = directory/(label+'.json')
    command = [str(output/'cycle-runner'), '--elf', str(build/binary), '--fixture-dir', str(ROOT/'fixtures/mainnet-blockchain-snark'), '--guest-abi', abi, '--batch-size', str(count), '--output', str(target), *extra]
    completed = target.with_suffix('.run.json')
    if completed.exists():
        previous = json.loads(completed.read_text())
        assert previous['command'] == command and previous['result_sha256'] == sha(target), 'completed run changed'
        assert previous['runner_sha256'] == sha(output/'cycle-runner'), 'completed runner differs'
        result = json.loads(target.read_text())
        assert result['elf_sha256'] == receipt['elf_sha256']
        assert result['fixture_sha256'] == receipt['config']['fixture_sha256']
        assert result['accepted'] is (not reject) and result['proof_generated'] is False
        if reject:
            assert previous['exit_code'] != 0 and result['guest_exit_code'] != 0 and result['public_values_len'] == 0
        else:
            assert previous['exit_code'] == 0 and result['guest_exit_code'] == 0
        print(f'Reusing checked {variant}/{label}', flush=True)
        return
    assert not target.exists(), f'incomplete run at {target}; inspect before retrying'
    start = time.monotonic()
    print(f'Running {variant}/{label}', flush=True)
    with target.with_suffix('.log').open('w') as log:
        process = subprocess.run(['/usr/bin/time', '-l', *command], env=environment(), cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    result = json.loads(target.read_text())
    assert result['elf_sha256'] == receipt['elf_sha256']
    assert result['fixture_sha256'] == receipt['config']['fixture_sha256']
    assert result['proof_generated'] is False
    if reject:
        assert process.returncode != 0 and result['accepted'] is False
        assert result['guest_exit_code'] != 0 and result['public_values_len'] == 0, 'must reject in the guest before output'
    else:
        assert process.returncode == 0 and result['accepted'] is True
        assert result['guest_exit_code'] == 0
    write(target.with_suffix('.run.json'), dict(command=command, exit_code=process.returncode,
          wall_seconds=time.monotonic()-start, result_sha256=sha(target), runner_sha256=sha(output/'cycle-runner'),
          max_rss_note='Darwin maximum resident set size in bytes is recorded by /usr/bin/time -l in the adjacent log'))
    check_identity(output)
    print(f"  {result['cycles']:,} cycles; accepted={result['accepted']}", flush=True)

def singles(output):
    for name in ['control', 'allocation', 'fused', 'digest']:
        abi = 'statement-digest' if name == 'digest' else 'boolean'
        execute(output, name, 'honest', abi)
        execute(output, name, 'bad-accumulator', abi, extra=['--corrupt-accumulator-challenge'], reject=True)
        execute(output, name, 'bad-opening', abi, extra=['--corrupt-wrap-opening'], reject=True)
        if name == 'digest':
            execute(output, name, 'bad-statement', abi, extra=['--corrupt-statement'], reject=True)

def batches(output):
    for name in ['batch-control', 'batch-combined']:
        for n in [1, 2, 4, 8]:
            execute(output, name, f'n{n}', 'batch', n)
        for bad in range(3):
            execute(output, name, f'bad-accumulator-{bad}', 'batch', 3,
                    ['--corrupt-accumulator-challenge', '--corrupt-index', str(bad)], True)
        execute(output, name, 'bad-opening-last', 'batch', 2,
                ['--corrupt-wrap-opening', '--corrupt-index', '1'], True)
        for n in [0, 9]:
            execute(output, name, f'invalid-count-{n}', 'batch', n, reject=True)

def profile(output):
    for name in ['control', 'allocation', 'fused']:
        execute(output, name, 'profile', extra=['--profile-precompiles'])

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=['build', 'single', 'batch', 'profile', 'all'])
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.phase in ('build', 'all'): build(output)
    if args.phase in ('single', 'all'): singles(output)
    if args.phase in ('batch', 'all'): batches(output)
    if args.phase in ('profile', 'all'): profile(output)

if __name__ == '__main__':
    main()
