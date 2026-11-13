#!/bin/bash
# Fuzz the block map by spot-rebuilding a spread of it, then the positive and
# negative proof controls.
set -uo pipefail
R=/root/mizu
BIN=$R/tools/pipeline-cuda/target/release/pipeline-cuda
OUT=/root/verify
mkdir -p $OUT
exec > /root/verify.log 2>&1
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib CUDA_ARCHS=86 RUST_LOG=warn
unset SP1_CIRCUIT_MODE
say(){ echo "[$(date -u +%H:%M:%S)] $*"; }

say "=== FUZZ: spot-rebuild 50 shapes spread across block 3 ==="
$BIN --elf /root/dummy/elf.bin --input-dir /root/dummy \
  --mode vk-map-block --block 3 --sample 50 --chunk 4 \
  --vk-map $OUT/spot50.bin --output $OUT/spot50.json
say "spot50_exit=$?"

say "=== POSITIVE: Groth16 against the block map ==="
t0=$(date +%s)
$BIN --elf /root/elf/o1-verifier-poseidon --input-dir /root/input \
  --mode groth16 --vk-map /root/vkmap/block3.bin --retain-pasta true \
  --output $OUT/proof-block3.json --proof-out $OUT/proof-block3.bin
say "positive_exit=$? seconds=$(( $(date +%s) - t0 ))"

say "=== NEGATIVE: same ELF against mapB, which lacks its shapes ==="
t0=$(date +%s)
$BIN --elf /root/elf/o1-verifier-poseidon --input-dir /root/input \
  --mode groth16 --vk-map /root/vkmap/mapB.bin --retain-pasta true \
  --output $OUT/proof-negative.json --proof-out $OUT/proof-negative.bin
say "negative_exit=$? seconds=$(( $(date +%s) - t0 ))  (non-zero is the PASS here)"

say "=== WRAP: save the wrap proof for later CPU-only gnark runs ==="
t0=$(date +%s)
$BIN --elf /root/elf/o1-verifier-poseidon --input-dir /root/input \
  --mode wrap --vk-map /root/vkmap/block3.bin --retain-pasta true \
  --output $OUT/wrap.json --wrap-out $OUT/wrap-proof.bin --proof-out $OUT/compressed.bin
say "wrap_exit=$? seconds=$(( $(date +%s) - t0 ))"
say "VERIFY_DONE"
