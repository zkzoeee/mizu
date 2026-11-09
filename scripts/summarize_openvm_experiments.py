#!/usr/bin/env python3
"""Summarize frozen CPU experiments; preserve partial/failed status explicitly."""
from pathlib import Path
import argparse, hashlib, json, os, re
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--directory', required=True, type=Path)
parser.add_argument('--output', type=Path)
args=parser.parse_args()
ROOT=Path(__file__).resolve().parents[1]
OUT=args.directory.resolve()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def load(p):return json.loads(p.read_text())
summary={'schema_version':1,'name':'mizu-openvm-inspired-sp1-6.10','date':'2026-11-09','status':'partial','proof_generated':False,'source_revision':None,'dependency_pins':load(ROOT/'benchmarks/core-proof-systems-0.7-sp1-6.10.json')['dependency_pins'],'raw_evidence_directory_relative_to_repository':os.path.relpath(OUT,ROOT),'hardware':load(OUT/'hardware.json'),'native_validation':load(OUT/'native-summary.json'),'variants':{},'comparisons':[],'limits':['All guest runs are CPU execution only; no fresh cryptographic proof, verification, reload or CUDA measurements.','Batch executions repeat one valid mainnet proof and do not represent diverse proof batches.','Digest output changes the ABI; its difference is functionality overhead, not equivalent-output optimization.','Precompile rows are unpadded row estimates, not complete trace cells or proof time.','Runner/JIT RSS is not prover memory.']}
for variant in ['control','allocation','fused','digest','batch-control','batch-combined']:
 directory=OUT/variant
 build=directory/'build'/'build.json'
 if not build.exists():continue
 b=load(build)
 assert sha(build.parent/b['config']['guest']['binary']) == b['elf_sha256'], 'frozen ELF changed'
 effective_abi = 'batch' if b['config']['guest']['binary']=='o1-verifier-batch' else ('statement-digest' if 'statement-digest' in b['config']['guest'].get('features',[]) else 'boolean')
 summary['source_revision']=b['revision']
 v={'build_receipt_sha256':sha(build),'elf_sha256':b['elf_sha256'],'tools':b['tools'],'config':b['config'],'source_manifest_sha256':hashlib.sha256(json.dumps(b['source_sha256'],sort_keys=True).encode()).hexdigest(),'effective_guest_abi':effective_abi,'runs':{}}
 for p in sorted(directory.glob('*.run.json')):
  raw=p.with_name(p.name.replace('.run.json','.json'))
  r=load(raw); receipt=load(p)
  assert sha(raw)==receipt['result_sha256']
  assert r['elf_sha256']==b['elf_sha256']
  assert r['fixture_sha256']==b['config']['fixture_sha256']
  assert r['proof_generated'] is False
  assert type(r['cycles']) is int and r['cycles']>0
  assert r['guest_abi'].replace('-','')==effective_abi.replace('-','')
  if raw.stem.startswith(('bad-','invalid-count-')):
   assert r['accepted'] is False and receipt['exit_code']!=0 and r['guest_exit_code']!=0 and r['public_values_len']==0
  else:
   assert r['accepted'] is True and receipt['exit_code']==0 and r['guest_exit_code']==0
  log=raw.with_suffix('.log')
  run={k:r.get(k) for k in ['guest_abi','proof_valid','expected_statement_digest','cycles','accepted','guest_exit_code','public_values_len','public_values_sha256','public_values_hex','stdin_items_sha256','fixture_sha256','pasta_syscall_counts','uint256_mul_count','stage_cycles','elapsed_seconds','batch_size','corrupt_index','corrupt_statement','corrupt_accumulator_challenge','corrupt_wrap_opening','precompile_profile']}
  run.update(result_sha256=sha(raw),run_receipt_sha256=sha(p),log_sha256=sha(log),max_runner_rss_bytes=receipt.get('max_runner_rss_bytes'))
  v['runs'][raw.stem]=run
 summary['variants'][variant]=v

def compare(a,al,b,bl,kind):
 try:
  control=summary['variants'][a]['runs'][al]; candidate=summary['variants'][b]['runs'][bl]
 except KeyError:return
 assert control['stdin_items_sha256']==candidate['stdin_items_sha256']
 assert control['fixture_sha256']==candidate['fixture_sha256']
 if kind!='changed-output-functionality':
  assert control['public_values_sha256']==candidate['public_values_sha256']
  assert control['guest_abi']==candidate['guest_abi']
 delta=candidate['cycles']-control['cycles']
 summary['comparisons'].append({'control':a+'/'+al,'candidate':b+'/'+bl,'kind':kind,'control_cycles':control['cycles'],'candidate_cycles':candidate['cycles'],'delta_cycles':delta,'delta_percent':100*delta/control['cycles'],'speedup':control['cycles']/candidate['cycles']})
compare('control','honest','allocation','honest','same-input-output')
compare('allocation','honest','fused','honest','same-input-output')
compare('control','honest','fused','honest','same-input-output')
compare('control','honest','digest','honest','changed-output-functionality')
for n in [1,2,4,8]:compare('batch-control',f'n{n}','batch-combined',f'n{n}','same-input-output')
expected_single={name: {'honest','bad-accumulator','bad-opening'} | ({'bad-statement'} if name=='digest' else set()) for name in ['control','allocation','fused','digest']}
expected_batch={name:{'n1','n2','n4','n8','bad-accumulator-0','bad-accumulator-1','bad-accumulator-2','bad-opening-last','invalid-count-0','invalid-count-9'} for name in ['batch-control','batch-combined']}
expected={**expected_single,**expected_batch}
for name in ['control','allocation','fused']: expected[name].add('profile')
summary['expected_runs']=sum(map(len,expected.values()))
summary['completed_runs']=sum(len(v['runs']) for v in summary['variants'].values())
summary['missing_runs']={name:sorted(names-set(summary['variants'].get(name,{}).get('runs',{}))) for name,names in expected.items() if names-set(summary['variants'].get(name,{}).get('runs',{}))}
summary['status']='cpu-experiments-complete-no-proofs' if not summary['missing_runs'] else 'partial'
identity=load(OUT/'identity.json')
summary['runner_sha256']=identity['runner_sha256']
summary['launcher_sha256']=identity['script_sha256']
summary['harness_repair']=identity.get('script_repair')
summary['known_reporting_issues']=['Original frozen build configs inherited the boolean ABI/expected-output metadata for optional guests. Actual features, binary, runner ABI and exact accepted output identify the contracts; effective_guest_abi records them.', 'Original frozen runner proof_valid interprets the first output byte as a boolean, which is invalid for the digest guest. accepted and the exact expected digest comparison establish verification; retain the raw field unchanged. Metadata-only corrections are tracked separately.']
summary['identity_receipt_sha256']=sha(OUT/'identity.json')
post = {}
for filename, label in [('distinct-batch-tests.log','distinct_producer_native'),('receipt-native.log','receipt_native'),('no-std-check.log','no_std')]:
 file=OUT/filename
 if file.exists():
  content=file.read_text()
  counts=[tuple(map(int,x)) for x in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed',content)]
  post[label]={'log_sha256':sha(file),'passed_tests':sum(p for p,f in counts),'failed_tests':sum(f for p,f in counts),'completed':bool(counts) if label!='no_std' else 'Finished' in content and 'error:' not in content}
recheck=OUT/'digest-postfix-recheck'
if (recheck/'build.json').exists():
 b=load(recheck/'build.json'); r=load(recheck/'result.json')
 assert r['accepted'] is True and r['proof_valid'] is True and r['guest_abi']=='statement-digest'
 assert r['elf_sha256']==summary['variants']['digest']['elf_sha256'], 'metadata recheck changed the guest ELF'
 assert r['cycles']==summary['variants']['digest']['runs']['honest']['cycles']
 assert r['public_values_sha256']==summary['variants']['digest']['runs']['honest']['public_values_sha256']
 assert not any(k.startswith('MIZU_') for k in b['config']['optimization_env'])
 post['digest_reporting_recheck']={'completed':True,'build_sha256':sha(recheck/'build.json'),'result_sha256':sha(recheck/'result.json'),'elf_sha256':r['elf_sha256'],'runner_sha256':b['runner_sha256'],'cycles':r['cycles'],'guest_abi':r['guest_abi'],'proof_valid':r['proof_valid'],'public_values_sha256':r['public_values_sha256'],'experiment_environment_injected':['MIZU_CIP_MODE=fused','MIZU_STACK_CROSS_FIELD=1','MIZU_BATCH_ACCUMULATOR=1'],'config_selects_experiments':False,'scope':'Generic benchmark run with corrected ABI/output fields and neutralized ambient experiment flags; same digest guest cycles and exact output as original.'}
summary['post_validation']=post
summary['post_validation_complete']=len(post)==4 and all(v['completed'] for v in post.values())
summary['native_core_and_runner_tests_passed']=summary['native_validation']['candidate_tests_passed']+post.get('distinct_producer_native',{}).get('passed_tests',0)
path=args.output or ROOT/'benchmarks/openvm-experiments-sp1-6.10.json'
path.write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({'variants':{k:list(v['runs']) for k,v in summary['variants'].items()},'comparisons':summary['comparisons']},indent=2))
