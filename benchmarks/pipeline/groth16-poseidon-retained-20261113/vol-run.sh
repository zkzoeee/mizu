#!/bin/bash
# Build (cached) and prove Groth16, using the network volume for everything
# expensive. Needs: /workspace mounted, source at /workspace/src/mizu, a guest
# ELF at /workspace/elf/, and a VK map at /workspace/vkmap/.
set -uo pipefail
V=/workspace
SELF=$(cd "$(dirname "$0")" && pwd)
RUN=${RUN:-run}
OUT=$V/out/$RUN
mkdir -p "$OUT"
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
export CARGO_TARGET_DIR=$V/target
export CUDA_ARCHS=86                 # A40/A6000 are sm_86; skips five other kernel sets
export RUST_LOG=info
unset SP1_CIRCUIT_MODE               # production path: install the released artifacts
ELF=${ELF:?set ELF}
MAP=${MAP:?set MAP}
BIN=$V/target/release/pipeline-cuda

{
  echo "begin=$(date -u +%FT%TZ)"
  nvidia-smi --query-gpu=name,memory.total,ecc.mode.current,compute_cap --format=csv,noheader
  # Cores decide the gnark solver and prover: 7.65 -> 16.15 halved both. Log
  # what we actually got, since RunPod gives a fraction of the host's cores.
  echo "cores=$(nproc) quota=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null || echo unknown)"
  echo "go_tags=${SP1_GNARK_GO_TAGS-<upstream default: debug>}"
  $SELF/vol-setup.sh
  [ -d $V/go ] || cp -a /usr/local/go $V/go 2>/dev/null || true
  cd $V/src/mizu/vendor/sp1/crates/recursion/gnark-ffi/go && go mod download && echo "go mod ok"
  cd $V/src/mizu/tools/pipeline-cuda
  # Delete the binary first. The guard below only tests that it exists, so
  # without this a failed build silently proves with the previous one -- which
  # is exactly what happened on 2026-11-13, when a driver edit did not compile
  # and the run looked normal.
  rm -f $BIN
  cargo build --release --features native-gnark 2>&1 | grep -E "^error|-->|Finished|could not compile|Compiling (sp1-gpu-prover|sp1-prover|pipeline-cuda)"
  echo "build_exit=${PIPESTATUS[0]}"
  echo "BUILD_DONE=$(date -u +%FT%TZ)"
} > "$OUT/build.log" 2>&1
[ -x $BIN ] || { echo "CHAIN_DONE=build-failed" > "$OUT/chain.log"; exit 1; }

( while true; do nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits; sleep 5; done > "$OUT/vram.txt" ) & S1=$!
# cgroup v2 first, then v1: not every RunPod host gives the same one.
( while true; do cat /sys/fs/cgroup/memory.current 2>/dev/null \
    || cat /sys/fs/cgroup/memory/memory.usage_in_bytes 2>/dev/null; sleep 5; done > "$OUT/rss.txt" ) & S2=$!
{
  echo "begin=$(date -u +%FT%TZ)"
  /usr/bin/time -v $BIN --elf "$ELF" --input-dir $V/src/mizu-gpu-input \
    --mode groth16 --vk-map "$MAP" --retain-pasta true \
    --output "$OUT/groth16.json" --proof-out "$OUT/groth16.proof.bin"
  echo "exit=$?"
  echo "done=$(date -u +%FT%TZ)"
} > "$OUT/run.log" 2>&1
kill $S1 $S2 2>/dev/null
{
  echo "peak_vram_mib=$(sort -n "$OUT/vram.txt" | tail -1)"
  echo "peak_cgroup_gib=$(sort -n "$OUT/rss.txt" | tail -1 | awk '{printf "%.1f", $1/1073741824}')"
  grep oom_kill /sys/fs/cgroup/memory.events 2>/dev/null \
    || grep oom_kill /sys/fs/cgroup/memory/memory.oom_control 2>/dev/null
  du -sh $V/sp1 $V/target $V/cargo-registry 2>/dev/null
  df -h $V | tail -1
} >> "$OUT/run.log" 2>&1
echo "CHAIN_DONE=$(date -u +%FT%TZ)" > "$OUT/chain.log"
