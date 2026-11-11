#!/usr/bin/env bash
# Prove from a cached build: no bundle, no compiler, no apt.
#
# `pod-run.sh` is the build-and-populate path and stays the authority on
# identity and gates. This is the path for a pod whose mounted volume already
# holds the binary and the artefacts, which after the 2026-11-11 measurements
# is the common case: the run itself is 3 minutes and everything else was
# overhead.
#
# What it does NOT do, deliberately, because none of it is needed when nothing
# is compiled: install build-essential, pkg-config, libssl-dev, git, curl or
# cmake; install a Rust toolchain; unpack a 72 MB source bundle. Those cost
# 22 s of apt plus 29 s of upload per run and produce nothing a prebuilt
# binary uses. Nor does it need a compiler of any kind: the cache is compared
# in RUNTIME SCOPE (`cache-key.py --runtime`), which requires the OS,
# CUDA_ARCHS and source fingerprint to match and ignores nvcc, rustc and the
# toolchain — they are build identity, already recorded in the stored key, and
# demanding them installed is what would force a 10 GB toolchain image to
# re-derive a fact that is written down. That is what lets this path run on
# `nvidia/cuda:…-base`, 101 MB, which has no nvcc at all.
#
# The cache key is checked, not assumed. A hit requires the runtime components
# above, the recorded sha256 to match, and the binary to answer
# `--mode registration` and `--mode cuda-probe` on this host.
# Anything else is a hard failure here rather than a silent fallback to a
# build: this script exists to be fast, and a surprise build would hide that
# the cache stopped working.
#
# Usage: SOURCE_FP=<fingerprint> pod-prove.sh
# Environment:
#   SOURCE_FP     source fingerprint from `cache-key.py --fingerprint-only`,
#                 computed by the launcher, which has the sources
#   CACHE         default /workspace/mizu-cache (the mounted volume)
#   RESULTS       default /root/results (container disk)
#   CUDA_ARCHS    must match what built the cached binary; the key is exact,
#                 so an auto-detected 86 misses an 86,89,120 cache
#   ARMS          manifest arms to prove, default retain-pasta-and-uint
set -euo pipefail

SOURCE_FP="${SOURCE_FP:?SOURCE_FP must be exported by the launcher}"
CACHE="${CACHE:-/workspace/mizu-cache}"
RESULTS="${RESULTS:-/root/results}"
TOOLCHAIN="${TOOLCHAIN:-1.92.0}"
ARMS="${ARMS:-retain-pasta-and-uint}"
BIN_NAME=pasta-precompile-proof-6-10
BINDIR=/workspace/bin
GPU_BIN="$BINDIR/$BIN_NAME"

mkdir -p "$RESULTS" "$BINDIR"
if [ -f "$RESULTS/FAILED" ]; then
  mv "$RESULTS/FAILED" "$RESULTS/FAILED.before-$(date -u +%Y%m%dT%H%M%SZ)"
fi
exec > >(tee -a "$RESULTS/run.log") 2>&1
stamp() { echo "=== $* $(date -u +%FT%TZ) ==="; }
fail() { stamp "FAILED: $*"; echo "$*" > "$RESULTS/FAILED"; exit 1; }
stamp "prove-from-cache started"

# This path installs nothing by design, but it does need python3 for the cache
# key and the JSON checks, and a slim CUDA image carries none. `python3`, not
# `python3-minimal`: the minimal package has no `json` module, which on
# 2026-11-11 made the cache probe fail with ModuleNotFoundError and cost a
# launch. A few tens of megabytes against the ~110 s of image pull a PyTorch
# image costs is the trade this path exists to make.
if ! python3 -c 'import json' 2>/dev/null; then
  echo "python3 with a stdlib is absent; installing it (the only install on this path)"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq && apt-get install -y --no-install-recommends python3 \
    || fail "no usable python3 and apt could not provide one"
fi

# This path installs nothing, so /usr/bin/time may not exist: pod-run.sh gets it
# from apt, and the stock image does not carry it. Use it when present and drop
# it otherwise rather than failing the proof over a stopwatch.
TIMER=""
if [ -x /usr/bin/time ]; then
  TIMER="/usr/bin/time -v"
else
  echo "note: /usr/bin/time is absent; timing comes from the harness's own report"
fi

# ---------------------------------------------------------------- 0. gates
nvidia-smi --query-gpu=name,memory.used,memory.total,utilization.gpu --format=csv \
  | tee "$RESULTS/telemetry-probe.csv"
if grep -qi 'Insufficient Permissions' "$RESULTS/telemetry-probe.csv"; then
  fail "nvidia-smi will not report memory/utilization; this is a MIG slice, not a whole card"
fi
# The context gate, which is why a 20 GB PyTorch image was ever used: it was
# the only thing on the pod that could create a CUDA context before the real
# work. `--mode cuda-probe` does the same with the harness itself, so a slim
# image needs no torch — but the binary has to be in place first, so the probe
# runs in section 1 below. When torch happens to be present, use it here too:
# two independent gates cost nothing and disagreement is informative.
if python3 -c 'import torch' 2>/dev/null; then
  python3 -c "import torch; assert torch.cuda.is_available(), 'no CUDA context'; \
print('torch gate: cuda context ok:', torch.cuda.get_device_name(0))" \
    || fail "the GPU is visible but no process can create a CUDA context; discard this host"
else
  echo "torch absent; the context gate is --mode cuda-probe, after the binary is in place"
fi

# ------------------------------------------------- 1. the cache, and nothing else
test -d "$CACHE" || fail "no cache at $CACHE; is the volume mounted?"
test -f "$CACHE/key.json" || fail "no key.json in $CACHE; run pod-run.sh once to populate it"
ART="$CACHE/artifacts"
for required in "$ART/elf/guest-program" "$ART/run-manifest.json" \
                "$ART/input/proof-input.bin" "$ART/input/receipt-input.bin" \
                "$ART/input/public-values.bin"; do
  test -f "$required" || fail "the cache is missing $required; populate it with pod-run.sh"
done
ELF="$ART/elf/guest-program"
MANIFEST="$ART/run-manifest.json"
INPUTS="$ART/input"

# nvcc comes from the image; rustc comes from the volume. Neither is installed.
export CUDA_HOME=/usr/local/cuda
export PATH="$CUDA_HOME/bin:$PATH"
command -v nvcc >/dev/null || {
  FOUND=$(find /usr/local -maxdepth 6 -type f -name nvcc 2>/dev/null | head -1)
  [ -n "$FOUND" ] && export PATH="$(dirname "$FOUND"):$PATH"
}
export CARGO_HOME="$CACHE/cargo-home"
export RUSTUP_HOME="$CACHE/rustup-home"
export PATH="$CARGO_HOME/bin:$PATH"
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
# Not required any more, and not a failure when absent: the runtime-scoped key
# does not read it. Reported when the volume happens to provide it, because a
# record of which toolchain was on the pod costs nothing.
command -v rustc >/dev/null || echo "note: no rustc here; the runtime-scoped key does not need one"

if [ -z "${CUDA_ARCHS:-}" ]; then
  CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d ' .')
  CUDA_ARCHS="$CC"
  echo "WARNING: CUDA_ARCHS not given, using the card's own $CUDA_ARCHS."
  echo "WARNING: a cache built for several architectures will MISS on this."
fi
export CUDA_ARCHS

stamp "checking the cached build against this host and these sources"
python3 -I /workspace/cache-key.py --runtime --source-fingerprint "$SOURCE_FP" \
  --archs "$CUDA_ARCHS" --toolchain "$TOOLCHAIN" \
  --write "$RESULTS/cache-key.json" --compare "$CACHE/key.json" > /dev/null \
  || fail "cache miss; this path does not build. Relaunch with pod-run.sh and the bundle"
# The identity that matters is the BUILD's, recorded when the binary was made —
# not what this toolchain-less host would compute for itself.
KEY=$(python3 -I -c "import json;print(json.load(open('$CACHE/key.json'))['key'])")

cp "$CACHE/bin/$BIN_NAME" "$GPU_BIN"
chmod +x "$GPU_BIN"
( cd "$BINDIR" && sha256sum -c <(sed "s|$BIN_NAME|$GPU_BIN|" "$CACHE/bin/$BIN_NAME.sha256") ) \
  || fail "the cached binary does not match its recorded sha256"
"$GPU_BIN" --mode registration --output "$RESULTS/registration.json" > /dev/null \
  || fail "the cached binary does not run on this host"
# The CUDA-context gate proper: initialise the device and read its memory. This
# is what replaces torch, and it is the harness's own code path rather than a
# proxy for it.
"$GPU_BIN" --mode cuda-probe --output "$RESULTS/cuda-probe.json" \
  || fail "the GPU is visible but no process can create a CUDA context; discard this host"
cat "$RESULTS/cuda-probe.json"
echo "{\"cache_hit\": 1, \"key\": \"$KEY\", \"built_nothing\": true}" > "$RESULTS/cache-status.json"
stamp "reusing the cached CUDA build; nothing installed, nothing compiled"

{
  echo "{"
  echo "  \"mode\": \"prove-from-cache\","
  echo "  \"cuda_archs\": \"$CUDA_ARCHS\","
  echo "  \"source_fingerprint\": \"$SOURCE_FP\","
  echo "  \"key\": \"$KEY\","
  echo "  \"rustc\": \"$(rustc --version 2>/dev/null || echo absent)\","
  echo "  \"nvcc\": \"$(nvcc --version 2>/dev/null | grep -oE 'release [0-9.]+' | head -1 || true)\","
  echo "  \"image\": \"$(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-unknown}")\","
  echo "  \"ldd_unresolved\": $(ldd "$GPU_BIN" 2>/dev/null | grep -c 'not found' || true),"
  echo "  \"gpu_name\": \"$(nvidia-smi --query-gpu=name --format=csv,noheader | head -1)\","
  echo "  \"gpu_uuid\": \"$(nvidia-smi --query-gpu=uuid --format=csv,noheader | head -1)\","
  echo "  \"compute_cap\": \"$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1)\","
  echo "  \"cpu_model\": \"$(awk -F': ' '/model name/{print $2;exit}' /proc/cpuinfo)\","
  echo "  \"host_vcpu_visible\": $(nproc),"
  echo "  \"elf_sha256\": \"$(sha256sum "$ELF" | cut -d' ' -f1)\","
  echo "  \"manifest_sha256\": \"$(sha256sum "$MANIFEST" | cut -d' ' -f1)\","
  echo "  \"harness_sha256\": \"$(sha256sum "$GPU_BIN" | cut -d' ' -f1)\""
  echo "}"
} > "$RESULTS/build-identity.json"
python3 -I -c "import json;json.load(open('$RESULTS/build-identity.json'))"
cat "$RESULTS/build-identity.json"

# ------------------------------------------------------------ 2. pre-flight
stamp "pre-flight: every pinned identity, before any proving"
"$GPU_BIN" --mode manifest-check --manifest "$MANIFEST" --elf "$ELF" \
  --input "$INPUTS" --output "$RESULTS/preflight.json" > /dev/null \
  || fail "manifest pre-flight failed"

export PASTA_ALLOW_REAL_PROOF=1

# ------------------------------------------------------------- 3. the proofs
for ARM in $ARMS; do
  stamp "proving arm $ARM as a CUDA core proof"
  OUT="$RESULTS/$ARM"
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,memory.used,memory.total \
    --format=csv,noheader,nounits -lms 200 > "$RESULTS/gpu-samples-$ARM.csv" &
  SAMPLER=$!
  set +e
  RUST_LOG="${RUST_LOG:-sp1_core_machine=info}" \
  PASTA_FRI_CONFIG=core PASTA_LOG_STACKING_HEIGHT=21 PASTA_MAX_LOG_ROW_COUNT=22 \
  PASTA_GPU_MAX_TRACE_SIZE=404750336 PASTA_GPU_WORKERS=2 \
  PASTA_R5_INFLIGHT="${PASTA_R5_INFLIGHT:-8}" \
  PASTA_PROVE_INFLIGHT="${PASTA_PROVE_INFLIGHT:-1}" \
  $TIMER "$GPU_BIN" --mode manifest-core-gpu \
    --manifest "$MANIFEST" --arm "$ARM" --elf "$ELF" \
    --input "$INPUTS" --output "$OUT" 2>&1 | tee "$RESULTS/$ARM.log"
  RC=${PIPESTATUS[0]}
  set -e
  kill $SAMPLER 2>/dev/null || true
  [ "$RC" -eq 0 ] || fail "arm $ARM exited $RC"
  du -sh "$OUT" | tee -a "$RESULTS/arm-sizes.txt"
done

# ------------------------------------- 4. reload verification, on this host
stamp "fresh-process reload verification of every arm proved here"
for ARM in $ARMS; do
  set +e
  "$GPU_BIN" --mode manifest-core-verify \
    --manifest "$MANIFEST" --elf "$ELF" --arm "$ARM" \
    --input "$RESULTS" --reference "$INPUTS/public-values.bin" \
    --output "$RESULTS/verification-$ARM" 2>&1 | tee -a "$RESULTS/verification.log"
  RC=${PIPESTATUS[0]}
  set -e
  [ "$RC" -eq 0 ] || fail "on-pod reload verification of $ARM exited $RC"
done

sha256sum "$RESULTS"/*/*.proof.bin 2>/dev/null | tee "$RESULTS/proof-hashes.txt" || true
echo ok > "$RESULTS/ALL_STAGES_PASSED"
stamp "finished: proved and verified from a cached build, with no compilation"
