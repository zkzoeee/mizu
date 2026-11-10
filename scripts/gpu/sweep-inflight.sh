#!/usr/bin/env bash
# Sweep the producer/consumer channel depth on one pod, interleaved.
#
# Why this exists: the 2026-11-10 A40 receipts show a single proof is bounded
# by the handshake between the record producer and the shard prover, not by
# either side's work. At PASTA_R5_INFLIGHT=2 the producer blocked 73.60 s
# inside the consumer callback (a full channel) while the consumer starved
# 40.38 s (an empty one), with at most one shard proof in flight. wazabi's
# r128 reached the same conclusion from the other side and left the deeper
# channel untested.
#
# Method follows r124: passes alternate between the depths rather than running
# all of one then all of the other, so a drift in the host shows up as spread
# within each side instead of as an effect. Everything else is held fixed —
# same binary, same host, same statement, same arm, same prover options.
#
# Runs on the pod, after pod-run.sh has built the harness and proved pass 1.
# Usage: sweep-inflight.sh [DEPTHS...]   (default: 8 2 8)
set -euo pipefail

ROOT="${ROOT:-/workspace/mizu}"
RESULTS="${RESULTS:-/workspace/results}"
BIN="${BIN:-/workspace/bin/pasta-precompile-proof-6-10}"
ARM="${ARM:-retain-pasta-and-uint}"
MANIFEST="$ROOT/gpu/run-manifest.json"
ELF="$ROOT/gpu/elf/guest-program"
INPUTS="$ROOT/gpu/input"
DEPTHS=("$@")
[ ${#DEPTHS[@]} -gt 0 ] || DEPTHS=(8 2 8)

test -x "$BIN" || { echo "no harness at $BIN; run pod-run.sh first" >&2; exit 1; }
stamp() { echo "=== $* $(date -u +%FT%TZ) ==="; }
exec > >(tee -a "$RESULTS/sweep.log") 2>&1

pass=1
for depth in "${DEPTHS[@]}"; do
  tag="inflight$depth-pass$pass"
  out="$RESULTS/sweep-$tag"
  if [ -e "$out" ]; then
    echo "skipping $tag: $out exists"
    pass=$((pass + 1))
    continue
  fi
  stamp "pass $pass: PASTA_R5_INFLIGHT=$depth"
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,memory.used,memory.total \
    --format=csv,noheader,nounits -lms 200 > "$RESULTS/gpu-samples-$tag.csv" &
  sampler=$!
  set +e
  PASTA_ALLOW_REAL_PROOF=1 \
  PASTA_FRI_CONFIG=core PASTA_LOG_STACKING_HEIGHT=21 PASTA_MAX_LOG_ROW_COUNT=22 \
  PASTA_GPU_MAX_TRACE_SIZE=404750336 PASTA_GPU_WORKERS=2 \
  PASTA_R5_INFLIGHT="$depth" PASTA_PROVE_INFLIGHT=1 \
  RUST_LOG=sp1_core_machine=info \
  /usr/bin/time -v "$BIN" --mode manifest-core-gpu \
    --manifest "$MANIFEST" --arm "$ARM" --elf "$ELF" \
    --input "$INPUTS" --output "$out" 2>&1 | tail -25
  rc=${PIPESTATUS[0]}
  set -e
  kill "$sampler" 2>/dev/null || true
  [ "$rc" -eq 0 ] || { echo "pass $pass (depth $depth) exited $rc" > "$RESULTS/SWEEP_FAILED"; exit 1; }
  python3 -I -c "
import json, sys
d = json.load(open('$out/result.json'))
p = d.get('producer_profile') or {}
c = d.get('consumer') or {}
print(json.dumps({
    'tag': '$tag', 'depth': $depth,
    'total_s': d['total_seconds'], 'executor_s': d['executor_seconds'],
    'gpu_prove_s': d['gpu_prove_seconds'], 'gpu_setup_s': d['gpu_setup_seconds'],
    'producer_blocked_s': p.get('executor_blocked_seconds'),
    'producer_on_record_s': p.get('on_record_seconds'),
    'producer_deps_s': p.get('dependencies_seconds'),
    'consumer_starved_s': c.get('starved_seconds'),
    'consumer_proving_s': c.get('proving_seconds'),
    'consumer_max_in_flight': c.get('max_in_flight'),
    'shards': d['generated_shards'], 'proof_bytes': d['proof_bytes'],
    'accepted': d['complete_verifier_accepted'],
}))" | tee -a "$RESULTS/sweep-summary.jsonl"
  pass=$((pass + 1))
done

stamp "sweep finished"
cat "$RESULTS/sweep-summary.jsonl"
