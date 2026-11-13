#!/bin/bash
# Probe, gate, build block 3, then prove two different workloads against it.
set -uo pipefail
V=/workspace
BIN=$V/target/release/pipeline-cuda
MAP=$V/vkmap/block3.bin
OUT=$V/out/block3
mkdir -p $OUT
exec > /root/stage2.log 2>&1
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib CARGO_TARGET_DIR=$V/target CUDA_ARCHS=86 RUST_LOG=info
unset SP1_CIRCUIT_MODE
say(){ echo "[$(date -u +%H:%M:%S)] $*"; }

say "=== PROBE: 500 indices spread over block 3, both ends included ==="
t0=$(date +%s)
$BIN --elf $V/elf/o1-verifier-poseidon --input-dir $V/src/mizu-gpu-input \
  --mode vk-map-block --block 3 --sample 500 --chunk 4 --allow-panics true \
  --vk-map $V/vkmap/probe500.bin --output $OUT/probe500.json
probe_exit=$?
probe_s=$(( $(date +%s) - t0 ))
say "probe_exit=$probe_exit probe_seconds=$probe_s"
[ $probe_exit -eq 0 ] || { say "GATE FAIL: probe did not finish"; exit 1; }
panics=$(python3 -c "import json;print(len(json.load(open('$OUT/probe500.json'))['panic_indices']))")
per=$(python3 -c "import json;print('%.4f'%json.load(open('$OUT/probe500.json'))['seconds_per_program'])")
say "panics=$panics seconds_per_program=$per"
[ "$panics" = "0" ] || { say "GATE FAIL: $panics shapes panicked -- decide the policy before building the block"; exit 1; }
proj=$(python3 -c "print(int(8176*$per))")
say "projected full build = ${proj}s"
[ "$proj" -lt 3000 ] || { say "GATE FAIL: projected ${proj}s exceeds the 50 min budget"; exit 1; }

say "=== BUILD: all 8,170 shapes of block 3 + 6 fixed ==="
t0=$(date +%s)
$BIN --elf $V/elf/o1-verifier-poseidon --input-dir $V/src/mizu-gpu-input \
  --mode vk-map-block --block 3 --chunk 4 \
  --vk-map $MAP --output $OUT/block3-map.json
build_exit=$?
say "build_exit=$build_exit build_seconds=$(( $(date +%s) - t0 ))"
[ $build_exit -eq 0 ] || { say "STOP: block build failed"; exit 1; }
say "map: $(ls -la $MAP | awk '{print $5}') bytes"

say "=== PROOF A: Poseidon ELF, retention ON (the production config) ==="
t0=$(date +%s)
$BIN --elf $V/elf/o1-verifier-poseidon --input-dir $V/src/mizu-gpu-input \
  --mode groth16 --vk-map $MAP --retain-pasta true \
  --output $OUT/proofA.json --proof-out $OUT/proofA.bin
say "proofA_exit=$? seconds=$(( $(date +%s) - t0 ))"

say "=== PROOF B: default ELF 0cfb3eaf, retention OFF -- different program, different sharding ==="
t0=$(date +%s)
$BIN --elf $V/src/o1-verifier-elf --input-dir $V/src/mizu-gpu-input \
  --mode groth16 --vk-map $MAP --retain-pasta false \
  --output $OUT/proofB.json --proof-out $OUT/proofB.bin
say "proofB_exit=$? seconds=$(( $(date +%s) - t0 ))"

cp -f /root/.sp1/circuits/groth16/v6.1.0/*.sol $OUT/ 2>/dev/null
say "STAGE2_DONE"
