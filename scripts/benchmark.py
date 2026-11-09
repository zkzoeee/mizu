#!/usr/bin/env python3
"""Build/measure the retained Pickles configuration on CPU. No proving APIs."""
import argparse, hashlib, json, os, shutil, subprocess, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def sha(path): return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def snapshot():
    lock = json.loads((ROOT/'dependencies/lock.json').read_text())
    result = {}
    for repo, prefix in [(ROOT, '')]+[(ROOT/d['path'], d['path']+'/') for d in lock['dependencies']]:
        for name in subprocess.check_output(['git', '-C', str(repo), 'ls-files', '-z']).split(b'\0'):
            if not name: continue
            key = prefix+os.fsdecode(name); p = ROOT/key
            if p.is_file() and (key.startswith(('crates/', 'vendor/', 'fixtures/')) or key in ('Cargo.toml','Cargo.lock','rust-toolchain.toml')):
                result[key] = sha(p)
    return result

def run(command, env, output, cwd=ROOT):
    start = time.monotonic()
    with output.open('w') as log:
        subprocess.run(command, cwd=cwd, env=env, check=True, stdout=log, stderr=subprocess.STDOUT)
    return {'command':command, 'seconds':time.monotonic()-start}

def tool_identity(builder, env):
    commands = {'host_rust': ['rustc', '--version'], 'guest_rust': ['rustc', '+succinct', '--version'], 'sp1_builder': [builder, 'prove', '--version']}
    return {'builder_sha256': sha(builder), **{name: subprocess.check_output(command, cwd=ROOT, env=env, text=True).strip() for name, command in commands.items()}}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('mode', choices=['build','run'])
    p.add_argument('--config', type=Path, default=ROOT/'benchmarks/retained.json')
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args(); args.output = args.output.resolve()
    if args.output.exists() and any(args.output.iterdir()): p.error('--output must be a new or empty directory')
    args.output.mkdir(parents=True, exist_ok=True)
    config = json.loads(args.config.read_text())
    fixture = ROOT/config['fixture_dir']
    assert {name:sha(fixture/name) for name in config['fixture_sha256']} == config['fixture_sha256'], 'fixture differs from retained configuration'
    env = dict(os.environ, CARGO_BUILD_JOBS='1', RAYON_NUM_THREADS='1', CARGO_TARGET_DIR=str(ROOT/'target'), RUSTUP_TOOLCHAIN='1.92.0')
    env.update(config['optimization_env'])
    source = snapshot(); timings = []
    neutral = {k:v for k,v in env.items() if not k.startswith(('ZEKO_','MIZU_'))}
    runner = ROOT/'target/native-test/cycle-runner'
    if args.mode == 'run':
        timings.append(run(['cargo','build','--profile','native-test','--locked','-p','cycle-runner','--bin','cycle-runner'],neutral,args.output/'runner-build.log'))
        shutil.copy2(runner,args.output/'cycle-runner');runner=args.output/'cycle-runner'
    builder = shutil.which('cargo-prove') or str(Path.home()/'.sp1/bin/cargo-prove')
    tools = tool_identity(builder, neutral)
    assert config['builder_commit'] in tools['sp1_builder'], 'builder commit differs from configuration'
    guest = config['guest'];elf=args.output/guest['binary']
    timings.append(run([builder,'prove','build','--locked','--rustflags=-C,passes=lower-atomic','--warning-level','minimal','--packages',guest['package'],'--binaries',guest['binary'],'--output-directory',str(args.output)],env,args.output/'guest-build.log',ROOT/guest['directory']))
    assert snapshot() == source, 'source changed during build'
    receipt={'schema_version':1,'config':config,'tools':tools,'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'source_sha256':source,'elf_sha256':sha(elf),'timings':timings,'proof_generated':False}
    if args.mode == 'run':
        receipt['runner_sha256']=sha(runner)
        result_path=args.output/'result.json'
        timings.append(run([str(runner),'--elf',str(elf),'--fixture-dir',str(ROOT/config['fixture_dir']),'--output',str(result_path)],neutral,args.output/'execute.log'))
        result=json.loads(result_path.read_text())
        assert result['accepted'] and result['guest_exit_code']==0
        assert result['public_values_sha256']==config['expected_public_values_sha256']
        assert result['fixture_sha256']==config['fixture_sha256']
        assert result['elf_sha256']==receipt['elf_sha256'] and sha(runner)==receipt['runner_sha256']
        assert snapshot()==source, 'source changed during execution'
        receipt['result']=result
        print(json.dumps({k:result[k] for k in ['cycles','guest_abi','accepted','public_values_len','elapsed_seconds']},indent=2))
    (args.output/'build.json').write_text(json.dumps(receipt,indent=2)+'\n')
if __name__ == '__main__': main()
