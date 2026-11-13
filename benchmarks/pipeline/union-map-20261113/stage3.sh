#!/bin/bash
# Salvage: per-ELF maps for two very different workloads, to be merged offline.
set -uo pipefail
V=/workspace
BIN=$V/target/release/pipeline-cuda
OUT=$V/out/block3
mkdir -p $OUT
exec > /root/stage3.log 2>&1
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib CARGO_TARGET_DIR=$V/target CUDA_ARCHS=86 RUST_LOG=warn
unset SP1_CIRCUIT_MODE
say(){ echo "[$(date -u +%H:%M:%S)] $*"; }

say "=== MAP A: Poseidon ELF, retention ON ==="
t0=$(date +%s)
$BIN --elf $V/elf/o1-verifier-poseidon --input-dir $V/src/mizu-gpu-input \
  --mode vk-map --retain-pasta true --chunk 4 \
  --vk-map $V/vkmap/mapA.bin --output $OUT/mapA.json
say "mapA_exit=$? seconds=$(( $(date +%s) - t0 ))"

say "=== MAP B: default ELF 0cfb3eaf, retention OFF ==="
t0=$(date +%s)
$BIN --elf $V/src/o1-verifier-elf --input-dir $V/src/mizu-gpu-input \
  --mode vk-map --retain-pasta false --chunk 4 \
  --vk-map $V/vkmap/mapB.bin --output $OUT/mapB.json
say "mapB_exit=$? seconds=$(( $(date +%s) - t0 ))"
cp -f /root/.sp1/circuits/groth16/v6.1.0/*.sol $OUT/ 2>/dev/null
say "STAGE3_DONE"
