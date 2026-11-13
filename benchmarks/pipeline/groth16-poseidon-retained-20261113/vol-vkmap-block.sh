#!/bin/bash
# Build an ELF-scoped VK map on the pod: a sample first, then the whole block.
#
# Why a sample first. `build_vks` has no resume. The controller submits every
# chunk task up front, creates an input and an output artifact for each, waits
# for all of them, then downloads and merges (`worker/controller/vk_tree.rs:95`).
# At 8,170 programs and chunk 4 that is ~2,043 tasks and ~4,086 live artifacts,
# and a failure at 90% loses everything. The only figure we have is 0.285
# s/program from a 67-program run; extrapolating that 122x without checking is
# how the stale-binary run happened.
#
# So: SAMPLE=n measures the rate, the memory and which shapes panic, including
# both ends of the block, which are the shapes most likely to do so. Then
# SAMPLE= (empty) builds the block for real.
#
# Needs the volume populated by vol-setup.sh and a build in $V/target.
# Usage:  ELF=... MAP=/workspace/vkmap/block3.bin SAMPLE=500 ./vol-vkmap-block.sh
set -uo pipefail
V=/workspace
SELF=$(cd "$(dirname "$0")" && pwd)
RUN=${RUN:-vkmap-block}
OUT=$V/out/$RUN
mkdir -p "$OUT"
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
export CARGO_TARGET_DIR=$V/target
export CUDA_ARCHS=86
export RUST_LOG=info
BLOCK=${BLOCK:-3}
CHUNK=${CHUNK:-4}
MAP=${MAP:?set MAP}
BIN=$V/target/release/pipeline-cuda

{
  echo "begin=$(date -u +%FT%TZ)"
  nvidia-smi --query-gpu=name,memory.total,ecc.mode.current,compute_cap --format=csv,noheader
  echo "cores=$(nproc) quota=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null || echo unknown)"
  echo "block=$BLOCK chunk=$CHUNK sample=${SAMPLE:-<all>}"
  $SELF/vol-setup.sh
  cd $V/src/mizu/tools/pipeline-cuda
  # Same guard as vol-run.sh: a failed build must not leave the previous binary
  # in place to be used silently.
  rm -f $BIN
  cargo build --release 2>&1 | grep -E "^error|-->|Finished|could not compile|Compiling pipeline-cuda"
  echo "build_exit=${PIPESTATUS[0]}"
} > "$OUT/build.log" 2>&1
[ -x $BIN ] || { echo "CHAIN_DONE=build-failed" > "$OUT/chain.log"; exit 1; }

( while true; do nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits; sleep 5; done > "$OUT/vram.txt" ) & S1=$!
( while true; do cat /sys/fs/cgroup/memory.current 2>/dev/null \
    || cat /sys/fs/cgroup/memory/memory.usage_in_bytes 2>/dev/null; sleep 5; done > "$OUT/rss.txt" ) & S2=$!
{
  echo "begin=$(date -u +%FT%TZ)"
  # --elf and --input-dir are ignored by this mode but the parser wants them.
  /usr/bin/time -v $BIN --elf "${ELF:-$V/elf/o1-verifier-poseidon}" \
    --input-dir $V/src/mizu-gpu-input \
    --mode vk-map-block --block "$BLOCK" --chunk "$CHUNK" \
    ${SAMPLE:+--sample "$SAMPLE"} \
    --allow-panics "${ALLOW_PANICS:-false}" \
    --vk-map "$MAP" --output "$OUT/vkmap.json"
  echo "exit=$?"
  echo "done=$(date -u +%FT%TZ)"
} > "$OUT/run.log" 2>&1
kill $S1 $S2 2>/dev/null
{
  echo "peak_vram_mib=$(sort -n "$OUT/vram.txt" | tail -1)"
  echo "peak_cgroup_gib=$(sort -n "$OUT/rss.txt" | tail -1 | awk '{printf "%.1f", $1/1073741824}')"
  grep -E "seconds_per_program|programs_built|vk_map_keys|panic_indices" "$OUT/vkmap.json" 2>/dev/null
  echo "CHAIN_DONE=$(date -u +%FT%TZ)"
} > "$OUT/chain.log" 2>&1
cat "$OUT/chain.log"
