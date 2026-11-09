#!/usr/bin/env python3
"""Fast, CPU-only native / operation / paired-guest development workflow."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import uuid

from devloop import engine as e, store as s
from devloop.compare import compare, validate


def native_tool_identity(env):
    return {'host':subprocess.check_output(['rustc','--version'],env=env,cwd=s.ROOT,text=True).strip(),
            'ambient':s.ambient_identity(env)}


def new_output(path):
    path = Path(path).resolve() if path else s.ROOT / 'benchmarks/runs' / ('dev-' + uuid.uuid4().hex)
    if path.exists() and any(path.iterdir()): raise ValueError(f'output must be new or empty: {path}')
    path.mkdir(parents=True, exist_ok=True)
    return path


def native(config, args, output):
    if args.all and (args.test or args.filter):
        raise ValueError('--all cannot be combined with --test or --filter')
    env = s.environment(config['optimization_env'])
    tool = native_tool_identity(env)
    source = s.source_snapshot()
    base = ['cargo', 'test', '--offline', '--profile', 'native-test', '--locked', '-p', 'pickles-verifier']
    targets = args.test or config.get('native_tests', ['canonical_ipa'])
    jobs = [('all', base + ['-p', 'cycle-runner'])] if args.all else [(name, base + ['--test', name]) for name in targets]
    for name in ([] if args.test or args.all else config.get('native_lib_filters', [])):
        jobs.append(('lib-' + name, base + ['--lib', name]))
    passed = 0
    for label, command in jobs:
        if args.filter: command += [args.filter]
        log = output / (label + '.log')
        s.command(command + ['--', '--test-threads=1'], env, log)
        counts = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed', log.read_text())
        total = sum(int(a) for a, _ in counts)
        if not total or any(int(b) for _, b in counts): raise ValueError(f'{label}: no tests passed; empty selections are not a gate')
        passed += total
    s.command(['cargo', 'check', '--offline', '--profile', 'native-test', '--locked', '-p', 'pickles-verifier', '--no-default-features'], env, output / 'nostd.log')
    if s.source_snapshot() != source: raise ValueError('source changed during native checks')
    s.write(output / 'native.json', {'source': source, 'recipe': config, 'passed': passed, 'full_suite': args.all,
            'no_std': True, 'tools':tool, 'proof_generated': False, 'logs_sha256': {p.name: s.sha(p) for p in output.glob('*.log')}})
    return {'passed': passed, 'no_std': True}


def bundle(config, built, runners, executed, output):
    record = e.observation(config, built, executed)
    value = {'recipe': config, 'build_folder': str(built[0]), 'runner_folder': str(runners),
             'execution_folder': str(executed[0]), 'build_entry_sha256': s.sha(built[0] / 'entry.json'),
             'execution_entry_sha256': s.sha(executed[0] / 'entry.json'), 'record': record}
    s.write(output, value)
    return value


def frozen(path):
    value = s.read_record(path)
    validate(value['record'], 'frozen control')
    build_path, execution_path, runner_path = (Path(value[k]) for k in ('build_folder', 'execution_folder', 'runner_folder'))
    for folder, field in ((build_path, 'build_entry_sha256'), (execution_path, 'execution_entry_sha256')):
        if s.sha(folder / 'entry.json') != value[field]: raise ValueError('frozen receipt changed')
    b = s.read_record(build_path / 'entry.json'); x = s.read_record(execution_path / 'entry.json')
    r = s.read_record(runner_path / 'entry.json')
    s.checked_entry(build_path, b['identity']); s.checked_entry(execution_path, x['identity']); s.checked_entry(runner_path, r['identity'])
    if s.sha(b['source_archive']['path']) != b['source_archive']['sha256']: raise ValueError('frozen source archive changed')
    raw = s.read_record(execution_path / 'result.json')
    e.validate_execution(raw, x['identity'], x['command_status'], x['validated_output_sha256'])
    rebuilt = e.observation(value['recipe'], (build_path, b, True), (execution_path, raw, x, True))
    if {k:v for k,v in rebuilt.items() if k != 'cache_hit'} != {k:v for k,v in value['record'].items() if k != 'cache_hit'}:
        raise ValueError('frozen comparison record differs from its artifacts')
    return value, (build_path, b, True), runner_path


def baseline(config, args, output):
    runner = e.runners(); built = e.build(config)
    executed = e.execute(config, built, runner, fresh=args.fresh)
    value = bundle(config, built, runner, executed, output / 'baseline.json')
    return {'baseline': str(output / 'baseline.json'), 'cycles': value['record']['measurement']['cycles'], 'cache_hit': executed[3]}


def bench(config, args, output):
    if args.control_record:
        control, _, runner = frozen(args.control_record)
        control = {**control, 'record': {**control['record'], 'cache_hit': True}}
        s.write(output / 'control.json', control)
    else:
        runner = e.runners(); control_config = e.recipe(args.control)
        built = e.build(control_config); executed = e.execute(control_config, built, runner, fresh=args.fresh)
        control = bundle(control_config, built, runner, executed, output / 'control.json')
    built = e.build(config)
    # A changed runtime needs a new matched control/runner, never an old cache.
    if built[1]['identity']['runtime_source_sha256'] != control['record']['compatibility']['runtime_source_sha256']:
        raise ValueError('runtime source changed; establish a new matched baseline')
    executed = e.execute(config, built, runner, fresh=args.fresh)
    candidate = bundle(config, built, runner, executed, output / 'candidate.json')
    result = compare(control['record'], candidate['record'])
    if config['guest'].get('abi') != 'poseidon':
        negative = e.execute(config, built, runner, mutation=args.negative, fresh=args.fresh)
        # Prove the negative actually changes the input, with the same ELF.
        if negative[1]['stdin_items_sha256'] == executed[1]['stdin_items_sha256']:
            raise ValueError('negative mutation did not change guest input')
        result['negative'] = {'case': args.negative, 'receipt': str(negative[0] / 'entry.json'), 'cache_hit': negative[3]}
    else:
        result['negative'] = {'scope': 'operation screening; native oracle checks; full guest rejection belongs in gate'}
    result['bundles_sha256'] = {name: s.sha(output / (name + '.json')) for name in ('control', 'candidate')}
    result.update(proof_generated=False, control_cache_hit=control['record']['cache_hit'], candidate_cache_hit=executed[3])
    s.write(output / 'comparison.json', result)
    return result


def validate_native_receipt(path, config, source):
    record = s.read_record(path)
    if record.get('source') != source or record.get('recipe', {}).get('optimization_env') != config['optimization_env'] or record.get('full_suite') is not True or record.get('no_std') is not True or record.get('proof_generated') is not False:
        raise ValueError('native gate source, flags or completeness differs')
    if record.get('tools') != native_tool_identity(s.environment(config['optimization_env'])):
        raise ValueError('native gate compiler/configuration differs')
    logs = record.get('logs_sha256', {})
    if not {'all.log', 'nostd.log'} <= set(logs) or type(record.get('passed')) is not int or record['passed'] <= 0:
        raise ValueError('native gate lacks full-suite logs/count')
    for name, digest in logs.items():
        log = path.parent / name
        if not log.resolve().is_relative_to(path.parent.resolve()) or s.sha(log) != digest:
            raise ValueError('native gate log changed')
    counts = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed', (path.parent / 'all.log').read_text())
    if not counts or any(int(b) for _, b in counts) or sum(int(a) for a, _ in counts) != record['passed']:
        raise ValueError('native gate test count differs from logs')
    return record


def gate(config, args, output):
    if config['guest'].get('abi', 'boolean') not in ('boolean', 'statement-digest'):
        raise ValueError('gate uses the single-proof verifier; screen micro/batch work with bench and native tests')
    s.command([sys.executable, '-B', 'dependencies/verify.py'], s.environment(), output / 'pins.log')
    source = s.source_snapshot()
    tools_env = s.environment(); tools_env['PYTHONPATH'] = str(s.ROOT/'scripts')
    s.command([sys.executable,'-B','-m','unittest','discover','-s','scripts/tests','-v'],tools_env,output/'tools.log')
    native_result = None
    if args.native_record:
        validate_native_receipt(args.native_record, config, source)
        native_result = {'record':str(args.native_record),'sha256':s.sha(args.native_record)}
    elif not args.skip_native:
        (output/'native').mkdir()
        native_result = native(config,argparse.Namespace(test=None,all=True,filter=None),output/'native')
    runner = e.runners(); built = e.build(config)
    positive = e.execute(config, built, runner, fresh=args.fresh)
    results = {'positive': str(positive[0] / 'entry.json')}
    selected_cases = args.case or [*e.MUTATIONS, 'wrong-vk']
    for case in selected_cases:
        if case == 'wrong-vk':
            wrong = e.build(config, s.ROOT / 'fixtures/nrr')
            if wrong[1]['artifacts'][config['guest']['binary']] == built[1]['artifacts'][config['guest']['binary']]:
                raise ValueError('wrong VK did not change program identity')
            run = e.execute(config, wrong, runner, wrong_vk=True, fresh=args.fresh)
            if run[1]['stdin_items_sha256'] != positive[1]['stdin_items_sha256']: raise ValueError('wrong-key control changed proof input')
        else:
            run = e.execute(config, built, runner, mutation=case, fresh=args.fresh)
            if run[1]['stdin_items_sha256'] == positive[1]['stdin_items_sha256']: raise ValueError('mutation left input unchanged')
        results[case] = str(run[0] / 'entry.json')
    selected_fixtures = args.fixture or list(e.FIXTURES)
    fixtures = {}
    for name in selected_fixtures:
        fixture = s.ROOT / 'fixtures' / name
        image = e.build(config, fixture)
        run = e.execute(config, image, runner, fixture=fixture, fresh=args.fresh)
        fixtures[name] = str(run[0] / 'entry.json')
    profile = None
    if not args.skip_profile:
        trace = e.execute(config,built,runner,fresh=args.fresh,profile=True)
        if trace[1]['cycles'] != positive[1]['cycles'] or trace[1]['stdin_items_sha256'] != positive[1]['stdin_items_sha256'] or trace[1]['public_values_sha256'] != positive[1]['public_values_sha256']:
            raise ValueError('profile differs from ordinary execution')
        profile = str(trace[0]/'entry.json')
    if s.source_snapshot()!=source: raise ValueError('source changed across promotion gates')
    result = {'cpu_gate_passed': True, 'full_cpu_matrix': set(selected_cases) == {*e.MUTATIONS, 'wrong-vk'} and set(selected_fixtures) == set(e.FIXTURES) and native_result is not None and profile is not None,
              'cases': results, 'fixtures': fixtures, 'recipe': config, 'proof_generated': False,
              'native':native_result,'profile':profile,
              'scope': 'CPU gates only; cryptographic SP1 proof generation/verification remains separate'}
    s.write(output / 'gate.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['check', 'baseline', 'bench', 'gate', 'pins', 'tools-test'])
    parser.add_argument('--recipe', type=Path, default=s.ROOT / 'benchmarks/dev/retained.json')
    parser.add_argument('--control', type=Path)
    parser.add_argument('--control-record', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--test', action='append')
    parser.add_argument('--filter')
    parser.add_argument('--all', action='store_true')
    parser.add_argument('--fresh', action='store_true', help='new independent execution; never relabel a cache hit')
    parser.add_argument('--negative', choices=e.MUTATIONS, default='accumulator')
    parser.add_argument('--case', choices=[*e.MUTATIONS, 'wrong-vk'], action='append')
    parser.add_argument('--fixture', choices=e.FIXTURES, action='append')
    parser.add_argument('--native-record',type=Path,help='reuse only an exact source/flags/full-suite native receipt')
    parser.add_argument('--skip-native',action='store_true',help='partial gate; never marks the full CPU matrix complete')
    parser.add_argument('--skip-profile',action='store_true',help='partial gate; leave trace replay for promotion')
    args = parser.parse_args()
    if args.mode == 'bench' and bool(args.control) == bool(args.control_record): parser.error('bench needs exactly one of --control or --control-record')
    if args.all and (args.test or args.filter): parser.error('--all cannot be combined with --test or --filter')
    with s.serial():
        if args.mode in ('pins', 'tools-test'):
            command = [sys.executable, '-B', 'dependencies/verify.py'] if args.mode == 'pins' else [sys.executable, '-B', '-m', 'unittest', 'discover', '-s', 'scripts/tests', '-v']
            env = s.environment(); env['PYTHONPATH'] = str(s.ROOT / 'scripts')
            subprocess_result = subprocess.run(command, env=env, cwd=s.ROOT)
            return subprocess_result.returncode
        config = e.recipe(args.recipe); output = new_output(args.output)
        result = {'check': native, 'baseline': baseline, 'bench': bench, 'gate': gate}[args.mode](config, args, output)
        print(json.dumps({'output': str(output), **result}, indent=2))
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.CalledProcessError) as error:
        print(json.dumps({'status': 'failed', 'error': str(error)}), file=sys.stderr)
        raise SystemExit(2)
