"""Frozen builds and checked CPU observations, adapted to Mizu's SP1 6.10 ABI."""
import hashlib
import re
from pathlib import Path
import shutil
import subprocess
import uuid

from . import store as s
from .compare import validate as validate_record
from .poseidon_reference import reference

FILES = ('app_statement.json', 'proof.serde.json', 'public_input_skeleton.json', 'vk.serde.json')
MUTATIONS = ('app-state', 'rehashed-app-state', 'previous-step-commitment', 'accumulator', 'public-input', 'wrap-opening')
FIXTURES = ('mainnet-blockchain-snark', 'ocaml-compat', 'nrr', 'simplechain/wrap0', 'simplechain/wrap1', 'simplechain/wrap2', 'treeproofreturn/wrap0', 'treeproofreturn/wrap1', 'treeproofreturn/wrap2')


def recipe(path):
    result = s.read_record(path)
    if not isinstance(result.get('optimization_env'), dict):
        raise ValueError('recipe needs an explicit optimization_env')
    for name, value in result['optimization_env'].items():
        if not name.startswith(('ZEKO_', 'MIZU_')) or not isinstance(value, str):
            raise ValueError('recipe optimization settings must be named compile-time experiment flags')
    guest = result.get('guest', {})
    if not (s.ROOT / guest.get('directory','')).resolve().is_relative_to(s.ROOT.resolve()):
        raise ValueError('guest source must be inside this repository')
    for name in ('package','binary'):
        if not isinstance(guest.get(name),str) or not re.fullmatch(r'[A-Za-z0-9_-]+',guest[name]):
            raise ValueError('invalid guest package/binary name')
    for name in result.get('native_tests',[]) + result.get('native_lib_filters',[]):
        if not isinstance(name,str) or not re.fullmatch(r'[A-Za-z0-9_:.-]+',name):
            raise ValueError('invalid native test selection')
    if not re.fullmatch('[0-9a-f]{7,40}',result.get('builder_commit','')):
        raise ValueError('recipe must pin the SP1 builder commit')
    return result


def fixture_hashes(fixture):
    return {name: s.sha(fixture / name) for name in FILES}


def tools(env):
    builder = shutil.which('cargo-prove') or str(Path.home() / '.sp1/bin/cargo-prove')
    versions = {name: subprocess.check_output(command, cwd=s.ROOT, env=env, text=True).strip()
                for name, command in {'host': ['rustc', '--version'], 'guest': ['rustc', '+succinct', '--version'],
                                      'builder': [builder, 'prove', '--version']}.items()}
    return {'builder_path': builder, 'builder_sha256': s.sha(builder), **versions}


def build(config, fixture=None):
    env = s.environment(config['optimization_env'])
    guest = config['guest']
    fixture = Path(fixture or s.ROOT / config['fixture_dir']).resolve()
    pins = fixture_hashes(fixture)
    if fixture == (s.ROOT / config['fixture_dir']).resolve() and pins != config['fixture_sha256']:
        raise ValueError('fixture differs from the recipe pins')
    env['VK_JSON'] = str(fixture / 'vk.serde.json')
    source = s.source_snapshot()
    # Host runner edits do not invalidate guest builds; different proofs with
    # byte-identical VKs share one guest. All library/fork sources stay pinned.
    inputs = {p: h for p, h in source.items() if not p.startswith(('crates/cycle-runner/', 'crates/o1-verifier-host/', 'fixtures/', 'scripts/tests/'))}
    tool = tools(env)
    if config['builder_commit'] not in tool['builder']:
        raise ValueError('SP1 builder differs from recipe')
    flags = ['--locked', '--rustflags=-C,passes=lower-atomic', '--warning-level', 'minimal',
             '--packages', guest['package'], '--binaries', guest['binary']]
    if guest.get('features'): flags += ['--features', ','.join(guest['features'])]
    identity = {'schema': 1, 'sources': inputs, 'settings': config['optimization_env'],
                'guest': {k: v for k, v in guest.items() if k != 'batch_size'},
                'vk_sha256': pins['vk.serde.json'], 'tools': tool,
                'ambient': s.ambient_identity(env, s.ROOT / guest['directory']), 'flags': flags,
                'runtime_revision': next(d['revision'] for d in s.read_record(s.ROOT / 'dependencies/lock.json')['dependencies'] if d['path']=='vendor/sp1'),
                'runtime_source_sha256': s.key({p:h for p,h in source.items() if p.startswith('vendor/sp1/')})}
    folder = s.STORE / 'builds' / s.key(identity)
    prior = s.checked_entry(folder, identity)
    if prior:
        archive = prior['source_archive']
        if not Path(archive['path']).is_file() or s.sha(archive['path']) != archive['sha256']:
            raise ValueError('corrupt cached source archive')
        print('Reusing checked guest build', flush=True)
        return folder, prior, True
    folder.mkdir(parents=True, exist_ok=True)
    archive = s.archive_source(source)
    s.command([tool['builder_path'], 'prove', 'build', *flags, '--output-directory', str(folder)],
              env, folder / 'build.log', cwd=s.ROOT / guest['directory'])
    if s.source_snapshot() != source or s.ambient_identity(env, s.ROOT / guest['directory']) != identity['ambient'] or tools(env) != tool:
        raise ValueError('source changed during guest build')
    prior = s.seal(folder, identity, [guest['binary'], 'build.log', 'build.command.json'],
                   source_archive=archive, revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=s.ROOT, text=True).strip(),
                   fixture_at_build=pins, proof_generated=False)
    return folder, prior, False


def runners():
    env = s.environment()
    source = s.source_snapshot()
    inputs = {p: h for p, h in source.items() if not p.startswith(('crates/o1-verifier/', 'crates/poseidon-bench/', 'crates/o1-verifier-host/', 'fixtures/', 'scripts/tests/'))}
    identity = {'sources': inputs, 'ambient': s.ambient_identity(env), 'profile': 'native-test',
                'host': subprocess.check_output(['rustc', '--version'], cwd=s.ROOT, env=env, text=True).strip()}
    folder = s.STORE / 'runners' / s.key(identity)
    prior = s.checked_entry(folder, identity)
    if prior:
        print('Reusing checked CPU runners', flush=True)
        return folder
    folder.mkdir(parents=True, exist_ok=True)
    s.command(['cargo', 'build', '--offline', '--profile', 'native-test', '--locked', '-p', 'cycle-runner', '--bins'], env, folder / 'build.log')
    for name in ('cycle-runner', 'operation-bench-runner'):
        shutil.copy2(s.ROOT / 'target/native-test' / name, folder / name)
    if s.source_snapshot() != source or s.ambient_identity(env) != identity['ambient']: raise ValueError('source/configuration changed during runner build')
    s.seal(folder, identity, ['cycle-runner', 'operation-bench-runner', 'build.log', 'build.command.json'])
    return folder


def validate_execution(raw, identity, status, expected):
    if type(raw.get('cycles')) is not int or not 0 < raw['cycles'] <= 2**64-1:
        raise ValueError('execution needs positive uint64 cycles')
    if raw.get('elf_sha256') != identity['elf_sha256']:
        raise ValueError('execution ELF mismatch')
    if raw.get('proof_generated') is not False:
        raise ValueError('CPU observation unexpectedly claims a proof')
    if identity['abi'] == 'poseidon':
        if raw.get('input_sha256') != identity['operation_input_sha256'] or raw.get('iterations_per_field') != identity['iterations']:
            raise ValueError('operation input mismatch')
        if raw.get('pasta_syscall_counts') != [0, 0, 0, 0]:
            raise ValueError('Poseidon operation unexpectedly called Pasta point syscalls')
    else:
        if raw.get('fixture_sha256') != identity['fixtures'] or raw.get('guest_abi') != identity['abi']:
            raise ValueError('execution fixture or ABI mismatch')
        if not raw.get('stdin_items_sha256'): raise ValueError('missing serialized guest input identity')
        if raw.get('mutation', 'none') != identity['mutation']:
            raise ValueError('wrong mutation was executed')
    if identity['expect_reject']:
        if status == 0 or raw.get('accepted') is not False or type(raw.get('guest_exit_code')) is not int or raw['guest_exit_code'] == 0 or raw.get('public_values_len') != 0:
            raise ValueError('negative must fail in the guest before public output')
    elif status != 0 or raw.get('accepted') is not True or raw.get('guest_exit_code') != 0 or raw.get('public_values_sha256') != expected:
        raise ValueError('honest execution/output validation failed')


def freeze_inputs(run_folder, fixture, identity):
    inputs = run_folder / 'inputs'
    inputs.mkdir()
    names = []
    for name in FILES:
        data = (fixture / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != identity['fixtures'][name]:
            raise ValueError('fixture changed while freezing input: ' + name)
        (inputs / name).write_bytes(data)
        names.append('inputs/' + name)
    if identity['abi'] == 'poseidon':
        data = identity['iterations'].to_bytes(4, 'little')
        if hashlib.sha256(data).hexdigest() != identity['operation_input_sha256']:
            raise ValueError('operation input differs from identity')
        (inputs / 'iterations.bin').write_bytes(data)
        s.write(inputs / 'oracle.json', identity['oracle'])
        names += ['inputs/iterations.bin', 'inputs/oracle.json']
    return names


def execute(config, built, runner_folder, fixture=None, mutation='none', wrong_vk=False, fresh=False, profile=False):
    folder, build_receipt, _ = built
    abi = config['guest'].get('abi', 'boolean')
    operation = abi == 'poseidon'
    fixture = Path(fixture or s.ROOT / config['fixture_dir']).resolve()
    runner = runner_folder / ('operation-bench-runner' if operation else 'cycle-runner')
    elf = folder / config['guest']['binary']
    if operation:
        oracle = reference(config.get('operation', {}).get('iterations', 16))
        expected = oracle['public_values_sha256']
    else:
        oracle = None
        # Digest output depends on the key and statement; the runner independently
        # computes it. For other producer families derive it from accepted output.
        expected = config.get('expected_public_values_sha256') if fixture == (s.ROOT / config['fixture_dir']).resolve() else None
    identity = {'input_storage_schema': 2, 'elf_sha256': s.sha(elf), 'runner_sha256': s.sha(runner), 'fixtures': fixture_hashes(fixture),
                'abi': abi, 'batch_size': config['guest'].get('batch_size', 1), 'mutation': mutation,
                'expect_reject': mutation != 'none' or wrong_vk, 'wrong_vk': wrong_vk, 'profile': profile,
                'expected_output_sha256': expected, 'oracle': oracle,
                'iterations': oracle['iterations_per_field'] if operation else None,
                'operation_input_sha256': hashlib.sha256(oracle['iterations_per_field'].to_bytes(4, 'little')).hexdigest() if operation else None}
    if identity['elf_sha256'] != build_receipt['artifacts'][config['guest']['binary']]: raise ValueError('guest ELF changed')
    run_folder = s.STORE / 'executions' / (s.key(identity) + ('-' + uuid.uuid4().hex if fresh else ''))
    prior = s.checked_entry(run_folder, identity)
    if prior:
        raw = s.read_record(run_folder / 'result.json')
        validate_execution(raw, identity, prior['command_status'], prior['validated_output_sha256'])
        print('Reusing checked execution (not a new sample)', flush=True)
        return run_folder, raw, prior, True
    run_folder.mkdir(parents=True, exist_ok=True)
    source = s.source_snapshot()
    input_names = freeze_inputs(run_folder, fixture, identity)
    command = [str(runner), '--elf', str(elf), '--output', str(run_folder / 'result.json')]
    if operation: command += ['--iterations', str(identity['iterations'])]
    else:
        command += ['--fixture-dir', str(run_folder / 'inputs'), '--guest-abi', abi, '--batch-size', str(identity['batch_size'])]
        if mutation != 'none': command += ['--mutation', mutation]
        if profile: command += ['--profile-precompiles']
    timing = s.command(command, s.environment(), run_folder / 'execute.log', allow_failure=True)
    raw = s.read_record(run_folder / 'result.json')
    if not expected:
        # Only a successfully checked statement digest can establish a new
        # family's digest output. Boolean public bytes are fixed by the ABI.
        expected = hashlib.sha256(b'\1').hexdigest() if abi in ('boolean', 'batch') else raw.get('public_values_sha256')
    validate_execution(raw, identity, timing['exit_code'], expected)
    if profile and raw.get('precompile_profile',{}).get('replayed_instructions') != raw['cycles']:
        raise ValueError('trace replay did not account for every instruction')
    if s.source_snapshot() != source or s.sha(elf) != identity['elf_sha256'] or s.sha(runner) != identity['runner_sha256'] or fixture_hashes(fixture) != identity['fixtures']:
        raise ValueError('execution source or inputs changed')
    prior = s.seal(run_folder, identity, ['result.json', 'execute.log', 'execute.command.json', *input_names],
                   command_status=timing['exit_code'], validated_output_sha256=expected, proof_generated=False)
    return run_folder, raw, prior, False


def observation(config, built, executed):
    folder, build_receipt, _ = built
    run_folder, raw, entry, hit = executed
    if entry['identity']['expect_reject']: raise ValueError('rejection cycles cannot be an optimization result')
    runtime = build_receipt['identity']['runtime_revision']
    runtime_source = build_receipt['identity']['runtime_source_sha256']
    stdin_hash = s.key(raw.get('stdin_items_sha256', [raw['input_sha256']] if 'input_sha256' in raw else []))
    record = {'schema_version': 1, 'run_id': run_folder.name, 'evidence_origin': 'observed_mizu',
              'source': {'repository': 'o1js-to-zkvm', 'revision': build_receipt['revision']},
              'program_sha256': entry['identity']['elf_sha256'], 'measurement_artifact_sha256': s.sha(run_folder / 'result.json'),
              'compatibility': {'protocol': 'mizu-pickles-0.7/' + entry['identity']['abi'],
                'fixture_sha256': s.key(entry['identity']['fixtures']), 'vk_sha256': entry['identity']['fixtures']['vk.serde.json'],
                'srs_sha256': s.key({p: h for p, h in build_receipt['identity']['sources'].items() if '/srs/' in p}),
                'input_sha256': stdin_hash, 'toolchain': s.key(build_receipt['identity']['tools']),
                'runtime_revision': runtime, 'runtime_source_sha256': runtime_source,
                'backend': 'minimal-executor/' + entry['identity']['runner_sha256'] + '/runtime/' + runtime_source,
                'workload': s.key({k: entry['identity'][k] for k in ('abi', 'batch_size', 'iterations', 'operation_input_sha256')})},
              'correctness': {'passed': True, 'report_sha256': s.sha(run_folder / 'entry.json'), 'output_sha256': raw['public_values_sha256']},
              'measurement': {'kind': 'execution', 'cycles': raw['cycles']}, 'cache_hit': hit,
              'raw_result': str(run_folder / 'result.json'), 'proof_generated': False}
    validate_record(record, 'observation')
    return record
