#!/usr/bin/env python3
"""Summarize retained development-loop validation; never run a prover."""
import argparse
import hashlib
import json
from pathlib import Path
import re

ROOT=Path(__file__).resolve().parents[1]
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def read(path):return json.loads(path.read_text())
def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--directory',type=Path,default=ROOT/'benchmarks/runs/dev-loop-validation')
 p.add_argument('--output',type=Path,default=ROOT/'benchmarks/dev-loop-validation.json')
 args=p.parse_args(); folder=args.directory.resolve()
 result={'schema_version':1,'kind':'cached-development-workflow-validation','status':'partial','proof_generated':False,
         'implementation_revision':read(folder/'full-baseline/baseline.json')['record']['source']['revision'],
         'dependency_pins':{d['name']:d['revision'] for d in read(ROOT/'dependencies/lock.json')['dependencies']},
         'raw_directory_relative_to_repository':str(folder.relative_to(ROOT)),'checks':{},'source_references':{},
         'limits':['Checked cache reuse is not an independent measurement.','Poseidon kernel comparison validates a restored benchmark for an existing optimization; no new whole-verifier gain is claimed.','The CPU gate does not generate or verify an SP1 proof; runtime AIR/proof gates remain separate.']}
 artifacts={'operation_cold':'operation-cold/comparison.json','operation_warm':'operation-warm/comparison.json',
            'targeted_native':'targeted-native/native.json','full_pair':'full-pair/comparison.json','full_pair_warm':'full-pair-warm/comparison.json',
            'native_all':'native-all/native.json','promotion':'promotion/gate.json','promotion_warm':'promotion-warm/gate.json'}
 for label,relative in artifacts.items():
  path=folder/relative
  if not path.exists():continue
  data=read(path);entry={'artifact':str(path.relative_to(ROOT)),'sha256':sha(path)}
  if 'comparison_valid' in data:
   assert data['comparison_valid'] is True and data['proof_generated'] is False
   entry.update({k:data.get(k) for k in ['control','candidate','delta','reduction_percent','control_cache_hit','candidate_cache_hit','negative']})
  elif 'cpu_gate_passed' in data:
   assert data['cpu_gate_passed'] is True and data['proof_generated'] is False
   entry.update(full_cpu_matrix=data['full_cpu_matrix'],case_count=len(data['cases']),positive_fixture_count=len(data['fixtures']),profile=data['profile'],native=data['native'])
  else:
   assert data['passed']>0 and data['no_std'] is True and data['proof_generated'] is False
   entry.update(passed=data['passed'],full_suite=data.get('full_suite',False),no_std=True)
  result['checks'][label]=entry
 timing=folder/'operation-warm-timing.json'
 if timing.exists():result['checked_operation_reuse']=read(timing)
 for relative in ['../wazabi/spikes/experiment.py','../wazabi/spikes/EXPERIMENTS.md','../wazabi/spikes/poseidon_reference.py','../wazabi/program/pasta-selftest/src/bin/poseidon-operation-program.rs',
                  '../miza-o1js-to-zkvm/miza/compare_runs.py','../miza-o1js-to-zkvm/miza/test_compare_runs.py','../miza-o1js-to-zkvm/miza/native.py','../miza-o1js-to-zkvm/miza/run_native_gate.py','../miza-o1js-to-zkvm/miza/guest_regressions.py']:
  source=ROOT/relative
  if source.exists():result['source_references'][relative]=sha(source)
 result['source_references_scope']='Working-file fingerprints; original repositories read only. No old experiment manifests, application integrations or experimental SP1 runtime were imported.'
 result['status']='cpu-workflow-validated-no-proofs' if len(result['checks'])==len(artifacts) and result['checks']['promotion']['full_cpu_matrix'] and result['checks']['promotion_warm']['full_cpu_matrix'] else 'partial'
 hardening=folder/'hardening'
 required=['frozen-operation/comparison.json','frozen-operation/check.log',
           'argument-rejection.json','tools-final.log','validation-scope.json']
 if all((hardening/name).exists() for name in required):
  data=read(hardening/'frozen-operation/comparison.json')
  assert data['comparison_valid'] is True and data['proof_generated'] is False
  tests=re.search(r'Ran (\d+) tests', (hardening/'tools-final.log').read_text())
  assert tests and '\nOK\n' in (hardening/'tools-final.log').read_text()
  assert read(hardening/'argument-rejection.json')['returncode']==2
  result['evidence_storage_hardening']={
   'implementation_revision':data['controller_revision'],'tooling_tests_passed':int(tests.group(1)),
   'frozen_operation_storage_and_reuse_passed':True,
   'operation_comparison':{k:data[k] for k in ['control','candidate','delta','reduction_percent','bundles_sha256']},
   'artifacts_sha256':{name:sha(hardening/name) for name in required},
   'validation_scope':read(hardening/'validation-scope.json'),
   'scope':'Full CPU matrix above ran before evidence-only hardening. Current tooling and frozen-executable input storage/reuse passed. Concurrent standalone harness/runtime edits are separate and are not validated by these results.',
   'proof_generated':False}
 args.output.write_text(json.dumps(result,indent=2)+'\n')
 print(json.dumps({'status':result['status'],'completed_checks':list(result['checks'])},indent=2))
if __name__=='__main__':main()
