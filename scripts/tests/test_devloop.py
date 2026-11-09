"""Real corruption/identity failures and scientific comparison guards."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from dev import native
from devloop import engine, store
from devloop.poseidon_reference import reference


class ArtifactTests(unittest.TestCase):
    def test_cached_elf_log_and_identity_corruption_fail(self):
        for victim in ['guest', 'build.log', 'identity']:
            with self.subTest(victim=victim), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for name in ['guest', 'build.log']: (root/name).write_bytes(name.encode())
                identity = {'flags': {'MIZU_OPT': '1'}, 'source': 'a'*64}
                store.seal(root, identity, ['guest', 'build.log'])
                self.assertIsNotNone(store.checked_entry(root, identity))
                if victim == 'identity':
                    changed = copy.deepcopy(identity); changed['flags']['MIZU_OPT']='0'
                    with self.assertRaisesRegex(ValueError, 'identity'): store.checked_entry(root, changed)
                else:
                    (root/victim).write_bytes(b'corrupt')
                    with self.assertRaisesRegex(ValueError, 'corrupt'): store.checked_entry(root, identity)

    def test_incomplete_entry_cannot_be_reused(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); (root/'failed.log').write_text('interrupted')
            with self.assertRaisesRegex(ValueError, 'incomplete'): store.checked_entry(root,{})

    def test_archive_is_deterministic_and_corrupt_archive_rejects(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); (root/'source.rs').write_text('fn main() {}')
            sources={'source.rs':store.sha(root/'source.rs')}
            with patch.object(store,'ROOT',root):
                a=store.archive_source(sources,root/'a'); b=store.archive_source(sources,root/'b')
                self.assertEqual(a['sha256'],b['sha256'])
                Path(a['path']).write_bytes(b'corrupt')
                with self.assertRaisesRegex(ValueError,'corrupt'): store.archive_source(sources,root/'a')

    def test_flags_sources_tools_and_config_change_cache_key(self):
        identity={'flags':{'MIZU_OPT':'1'},'sources':{'x':'a'*64},'tool':'b'*64,'config':'c'*64}
        for name in identity:
            modified=copy.deepcopy(identity);modified[name]={'changed':True}
            self.assertNotEqual(store.key(identity),store.key(modified))

    def test_ambient_flags_are_hashed_without_disclosing_values(self):
        a=store.ambient_identity({'RUSTFLAGS':'secret-build-override-A'})
        b=store.ambient_identity({'RUSTFLAGS':'secret-build-override-B'})
        self.assertNotEqual(a,b)
        self.assertNotIn('secret-build',json.dumps(a))


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        self.identity={'elf_sha256':'a'*64,'abi':'boolean','fixtures':{'vk':'b'*64},'mutation':'none','expect_reject':False}
        self.raw={'elf_sha256':'a'*64,'guest_abi':'boolean','fixture_sha256':{'vk':'b'*64},'mutation':'none',
                  'cycles':100,'accepted':True,'guest_exit_code':0,'public_values_len':1,
                  'public_values_sha256':'c'*64,'stdin_items_sha256':['d'*64],'proof_generated':False}

    def test_invalid_cycle_types_rejected(self):
        for value in [True,0,-1,1.5,float('nan'),2**64]:
            raw={**self.raw,'cycles':value}
            with self.assertRaises(ValueError): engine.validate_execution(raw,self.identity,0,'c'*64)

    def test_host_failure_cannot_count_as_guest_rejection(self):
        identity={**self.identity,'expect_reject':True,'mutation':'accumulator'}
        raw={**self.raw,'accepted':False,'mutation':'accumulator','public_values_len':0}
        with self.assertRaisesRegex(ValueError,'guest'): engine.validate_execution(raw,identity,1,'c'*64)
        raw['guest_exit_code']=1
        engine.validate_execution(raw,identity,1,'c'*64)

    def test_wrong_output_fixture_elf_or_mutation_rejects(self):
        for name,value in [('public_values_sha256','f'*64),('elf_sha256','f'*64),('fixture_sha256',{}),('mutation','public-input')]:
            with self.assertRaises(ValueError): engine.validate_execution({**self.raw,name:value},self.identity,0,'c'*64)


class NativeGateTests(unittest.TestCase):
    def test_zero_selected_tests_is_failure(self):
        args=argparse.Namespace(test=['selected'],all=False,filter=None)
        def command(argv,env,path): path.write_text('test result: ok. 0 passed; 0 failed;\n')
        with tempfile.TemporaryDirectory() as directory, patch('dev.native_tool_identity',return_value={}), patch.object(store,'command',command), patch.object(store,'source_snapshot',return_value={}):
            with self.assertRaisesRegex(ValueError,'no tests'): native({'optimization_env':{}},args,Path(directory))

    def test_recipe_flags_and_serial_limits_reach_native_gate(self):
        args=argparse.Namespace(test=['selected'],all=False,filter=None)
        seen=[]
        def command(argv,env,path):
            seen.append((argv,env));path.write_text('test result: ok. 1 passed; 0 failed;\n')
        with tempfile.TemporaryDirectory() as directory, patch('dev.native_tool_identity',return_value={}), patch.object(store,'command',command), patch.object(store,'source_snapshot',return_value={}):
            result=native({'optimization_env':{'MIZU_CIP_MODE':'borrowed'}},args,Path(directory))
        self.assertEqual(result['passed'],1)
        self.assertTrue(any('--no-default-features' in argv for argv,_ in seen))
        for _,env in seen:
            self.assertEqual(env['MIZU_CIP_MODE'],'borrowed')
            for name in ['CARGO_BUILD_JOBS','RAYON_NUM_THREADS','RUST_TEST_THREADS']: self.assertEqual(env[name],'1')


class OracleTests(unittest.TestCase):
    def test_poseidon_integer_reference_vectors(self):
        for n,digest in [(1,'551b8112153878bcb13943956a65e5c12ccb1fd3ffb6b5f585d4fcc8ccf7eb38'),
                         (3,'2360eced3ec46f7f98a49a95627c3b9b519ce03103e23450fd6f789b95b186d1'),
                         (16,'b15468a959d2611303410a36ae21be25507eb6a0f711570b53d2bfca7dcf8a8e')]:
            self.assertEqual(reference(n)['public_values_sha256'],digest)
        for n in [True,0,65]:
            with self.assertRaises(ValueError): reference(n)


class EvidenceHardeningTests(unittest.TestCase):
    def test_consumer_scope_excludes_only_the_separate_tools_workspace(self):
        snapshot = {'tools/pasta-proof/src/main.rs': 'a', 'crates/pickles-verifier/src/lib.rs': 'b',
                    'vendor/sp1/crates/core/executor/src/lib.rs': 'c', 'scripts/dev.py': 'd'}
        with patch.object(store, 'source_snapshot', return_value=snapshot):
            self.assertEqual(engine.consumer_snapshot(), {name: digest for name, digest in snapshot.items() if not name.startswith('tools/')})

    def test_each_observation_preserves_input_bytes_and_checks_corruption(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); fixture = root/'fixture'; fixture.mkdir()
            for name in engine.FILES: (fixture/name).write_bytes(name.encode())
            identity = {'fixtures': engine.fixture_hashes(fixture), 'abi': 'boolean'}
            run = root/'run'; run.mkdir()
            names = engine.freeze_inputs(run, fixture, identity)
            store.seal(run, identity, names)
            first = engine.FILES[0]
            (fixture/first).write_bytes(b'changed after observation')
            self.assertEqual((run/'inputs'/first).read_bytes(), first.encode())
            self.assertIsNotNone(store.checked_entry(run, identity))
            (run/'inputs'/first).write_bytes(b'corrupt cached input')
            with self.assertRaisesRegex(ValueError, 'corrupt'): store.checked_entry(run, identity)
            second = root/'second'; second.mkdir()
            with self.assertRaisesRegex(ValueError, 'changed while freezing'): engine.freeze_inputs(second, fixture, identity)

    def test_operation_preserves_integer_input_and_oracle(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); fixture = root/'fixture'; fixture.mkdir()
            for name in engine.FILES: (fixture/name).write_bytes(name.encode())
            oracle = reference(1)
            identity = {'fixtures': engine.fixture_hashes(fixture), 'abi': 'poseidon', 'iterations': 1,
                        'oracle': oracle, 'operation_input_sha256': hashlib.sha256((1).to_bytes(4,'little')).hexdigest()}
            names = engine.freeze_inputs(root, fixture, identity)
            self.assertIn('inputs/oracle.json', names)
            self.assertEqual((root/'inputs/iterations.bin').read_bytes(), b'\1\0\0\0')
            self.assertEqual(store.read_record(root/'inputs/oracle.json'), oracle)

    def test_cpu_report_requires_explicit_no_proof_flag(self):
        example = ExecutionTests(); example.setUp()
        for value in ['missing', True, None, 0]:
            raw = dict(example.raw)
            if value == 'missing': raw.pop('proof_generated')
            else: raw['proof_generated'] = value
            with self.assertRaisesRegex(ValueError, 'proof'): engine.validate_execution(raw, example.identity, 0, 'c'*64)

    def test_filtered_selection_cannot_claim_all_tests(self):
        for test, selection in [(None, 'one'), (['one'], None)]:
            with self.assertRaisesRegex(ValueError, '--all'):
                native({'optimization_env': {}}, argparse.Namespace(all=True, test=test, filter=selection), Path('/unused'))

    def test_native_receipt_requires_complete_logs_and_exact_count(self):
        from dev import validate_native_receipt
        with tempfile.TemporaryDirectory() as directory, patch('dev.native_tool_identity', return_value={}):
            root = Path(directory)
            (root/'all.log').write_text('test result: ok. 2 passed; 0 failed;\n')
            (root/'nostd.log').write_text('Finished')
            original = {'source': {}, 'recipe': {'optimization_env': {}}, 'full_suite': True, 'no_std': True,
                        'proof_generated': False, 'tools': {}, 'passed': 2,
                        'logs_sha256': {name: store.sha(root/name) for name in ['all.log','nostd.log']}}
            path = root/'native.json'; store.write(path, original)
            validate_native_receipt(path, {'optimization_env': {}}, {})
            for change in [{'passed': True}, {'passed': 3}, {'logs_sha256': {}}, {'full_suite': False}]:
                store.write(path, {**original, **change})
                with self.assertRaises(ValueError): validate_native_receipt(path, {'optimization_env': {}}, {})
            store.write(path, original); (root/'all.log').write_text('changed')
            with self.assertRaisesRegex(ValueError, 'log changed'): validate_native_receipt(path, {'optimization_env': {}}, {})

    def test_frozen_comparison_labels_reuse_and_materializes_both_bundles(self):
        from dev import bench
        control = {'record': {'cache_hit': False, 'compatibility': {'runtime_source_sha256': 'r'}}}
        config = {'guest': {'abi': 'poseidon'}}
        built = (Path('/build'), {'identity': {'runtime_source_sha256': 'r'}}, True)
        args = argparse.Namespace(control_record=Path('/old.json'), fresh=False)
        def candidate_bundle(config, built, runner, executed, output):
            result = {'record': {'cache_hit': True}}; store.write(output, result); return result
        with tempfile.TemporaryDirectory() as directory, patch('dev.frozen', return_value=(control, built, Path('/runner'))), patch.object(engine, 'build', return_value=built), patch.object(engine, 'execute', return_value=(None,None,None,True)), patch('dev.bundle', side_effect=candidate_bundle), patch('dev.compare', return_value={}):
            root = Path(directory); result = bench(config, args, root)
            self.assertTrue(result['control_cache_hit'])
            self.assertFalse(control['record']['cache_hit'])
            self.assertEqual(result['bundles_sha256'], {name: store.sha(root/(name+'.json')) for name in ['control','candidate']})

    def test_command_attempt_is_recorded_before_execution_and_on_interrupt(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory)/'execute.log'; receipt = log.with_suffix('.command.json')
            def interrupt(*args, **kwargs):
                self.assertEqual(store.read_record(receipt)['status'], 'started')
                raise KeyboardInterrupt()
            with patch.object(store.subprocess, 'run', side_effect=interrupt), self.assertRaises(KeyboardInterrupt):
                store.command(['guest-runner'], {}, log)
            self.assertEqual(store.read_record(receipt)['status'], 'interrupted')


if __name__=='__main__': unittest.main()
