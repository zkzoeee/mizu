#!/bin/bash
# Two very different workloads, one merged map, one vk_root.
set -uo pipefail
V=/workspace
BIN=$V/target/release/pipeline-cuda
MAP=$V/vkmap/union.bin
OUT=$V/out/block3
mkdir -p $OUT
exec > /root/stage4.log 2>&1
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib CARGO_TARGET_DIR=$V/target CUDA_ARCHS=86 RUST_LOG=warn
unset SP1_CIRCUIT_MODE
say(){ echo "[$(date -u +%H:%M:%S)] $*"; }
say "union map: $(sha256sum $MAP | cut -c1-16) $(stat -c%s $MAP) bytes"

say "=== PROOF A: Poseidon ELF, retention ON, union map ==="
t0=$(date +%s)
$BIN --elf $V/elf/o1-verifier-poseidon --input-dir $V/src/mizu-gpu-input \
  --mode groth16 --vk-map $MAP --retain-pasta true \
  --output $OUT/proofA.json --proof-out $OUT/proofA.bin
say "proofA_exit=$? seconds=$(( $(date +%s) - t0 ))"

say "=== PROOF B: default ELF 0cfb3eaf, retention OFF, same union map ==="
t0=$(date +%s)
$BIN --elf $V/src/o1-verifier-elf --input-dir $V/src/mizu-gpu-input \
  --mode groth16 --vk-map $MAP --retain-pasta false \
  --output $OUT/proofB.json --proof-out $OUT/proofB.bin
say "proofB_exit=$? seconds=$(( $(date +%s) - t0 ))"
say "STAGE4_DONE"
