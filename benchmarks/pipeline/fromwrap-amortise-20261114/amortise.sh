#!/bin/bash
# Measure the from-wrap amortisation: prove one saved wrap proof N times in ONE
# process. The Go prover caches the R1CS and the proving key in package globals
# (prove_groth16.go:19-23), so proof 1 pays the reads and 2..N should not.
#
# Needs: /workspace mounted with the populated volume, source at
# /workspace/src/mizu, and a wrap proof uploaded to /workspace/out/wrap.bin.
set -uo pipefail
V=/workspace
SELF=$(cd "$(dirname "$0")" && pwd)
RUN=${RUN:-amortise}
OUT=$V/out/$RUN
mkdir -p "$OUT"
export PATH=/root/.cargo/bin:/usr/local/go/bin:/usr/local/cuda/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
export CARGO_TARGET_DIR=$V/target
export CUDA_ARCHS=86
export RUST_LOG=info
unset SP1_CIRCUIT_MODE               # production path: use the released artifacts on the volume
N=${N:-3}
WRAP=${WRAP:-$V/out/wrap.bin}
FEATURES=${FEATURES:-native-gnark}
BIN=$V/target/release/pipeline-runner

{
  echo "begin=$(date -u +%FT%TZ)"
  echo "cores=$(nproc) quota=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null || echo unknown)"
  nvidia-smi --query-gpu=name,memory.total,ecc.mode.current,compute_cap --format=csv,noheader 2>/dev/null || echo "no nvidia-smi"
  echo "features=$FEATURES n=$N wrap=$WRAP"
  $SELF/vol-setup.sh
  echo "artifacts_complete=$([ -f $V/sp1/circuits/groth16/v6.1.0/.complete ] && echo yes || echo NO)"
  [ -d $V/go ] || cp -a /usr/local/go $V/go 2>/dev/null || true
  cd $V/src/mizu/vendor/sp1/crates/recursion/gnark-ffi/go && go mod download && echo "go mod ok"
  cd $V/src/mizu
  # Same guard as vol-run.sh: a failed build must not leave the old binary to be
  # used silently. That cost a run on 2026-11-13.
  rm -f $BIN
  cargo build --release -p pipeline-runner --bin pipeline-runner --features "$FEATURES" 2>&1 \
    | grep -E "^error|-->|Finished|could not compile|Compiling (sp1-prover|pipeline-runner)"
  echo "build_exit=${PIPESTATUS[0]}"
  echo "BUILD_DONE=$(date -u +%FT%TZ)"
} > "$OUT/build.log" 2>&1
[ -x $BIN ] || { echo "CHAIN_DONE=build-failed" > "$OUT/chain.log"; cat "$OUT/build.log" | tail -20; exit 1; }

# N copies of the same wrap proof. Same input every time, so any difference
# between proof 1 and proofs 2..N is the cached artifacts and nothing else.
ARGS=()
for i in $(seq 1 "$N"); do ARGS+=(--wrap-in "$WRAP"); done
{
  echo "begin=$(date -u +%FT%TZ)"
  /usr/bin/time -v $BIN --elf /dev/null --input-dir $V/src/mizu-gpu-input \
    --mode from-wrap "${ARGS[@]}" \
    --output "$OUT/amortise.json" --proof-out "$OUT/p.bin"
  echo "exit=$?"
  echo "done=$(date -u +%FT%TZ)"
} > "$OUT/run.log" 2>&1
{
  echo "=== the numbers ==="
  grep -E "groth16_first|groth16_rest_mean|saved_per_proof|groth16_total" "$OUT/amortise.json" 2>/dev/null
  echo "=== gnark's own stage lines (first vs later) ==="
  grep -nE "Reading R1CS|Reading proving key|Generating proof|Generating witness" "$OUT/run.log" | head -20
  echo "CHAIN_DONE=$(date -u +%FT%TZ)"
} > "$OUT/chain.log" 2>&1
cat "$OUT/chain.log"
