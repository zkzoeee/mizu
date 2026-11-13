#!/bin/bash
# Build one slice of preprocessed block 3, with memory sampling.
# Usage: SLICE=i COUNT=n /root/p2-slice.sh
set -uo pipefail
R=/root/mizu
BIN=$R/tools/pipeline-cuda/target/release/pipeline-cuda
OUT=/root/slices
mkdir -p $OUT /root/dummy
: > /root/dummy/elf.bin; : > /root/dummy/proof-input.bin; : > /root/dummy/receipt-input.bin
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib CUDA_ARCHS=86 RUST_LOG=warn
SLICE=${SLICE:?set SLICE}
COUNT=${COUNT:?set COUNT}
LOG=$OUT/slice-$SLICE.log
( while true; do cat /sys/fs/cgroup/memory.current 2>/dev/null; sleep 2; done > $OUT/rss-$SLICE.txt ) & S=$!
t0=$(date +%s)
$BIN --elf /root/dummy/elf.bin --input-dir /root/dummy \
  --mode vk-map-block --block 3 --slice-index "$SLICE" --slice-count "$COUNT" \
  --chunk 4 --vk-map $OUT/slice-$SLICE.bin --output $OUT/slice-$SLICE.json > $LOG 2>&1
code=$?
kill $S 2>/dev/null
peak=$(sort -n $OUT/rss-$SLICE.txt | tail -1)
echo "slice=$SLICE/$COUNT exit=$code seconds=$(( $(date +%s) - t0 )) peak_gib=$(awk "BEGIN{printf \"%.1f\", $peak/1073741824}")"
grep -E "block 3:|keys in" $LOG | tail -2
