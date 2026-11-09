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


if __name__=='__main__': unittest.main()
