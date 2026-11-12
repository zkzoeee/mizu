#!/usr/bin/env bash
# Prove the pinned core guest on a rented GPU pod, one arm at a time.
#
# Ported from wazabi r123 (`spikes/artifacts/r123-sp1-6.10-5090-validation/
# pod-run.sh`), which ran this same harness binary on an RTX 5090. Every gate,
# every ordering rule and every hard-won environment fix is kept. What is new
# here is the build cache (section 4): if a network volume is mounted and
# already holds a binary built from these exact sources, for this GPU
# architecture, on this image and toolchain, the build is skipped entirely.
#
# Lessons from r114/r120/r123, all applied below:
#   1. nvcc is installed but not on PATH; set CUDACXX explicitly.
#   2. Gates do not get `|| true`. That masked a missing nvcc and cost r114
#      4.5 minutes of billed time.
#   3. `nproc` reports the HOST's cores, not the pod's allocation (120 visible
#      against 15 allocated on r123's pod, 152 against 25 on the Groth16 one),
#      so CARGO_BUILD_JOBS is pinned rather than derived.
#   4. cmake >= 3.30 is required and Ubuntu 24.04 ships 3.28.
#   5. A whole card reports memory and utilization; a MIG slice answers
#      "[Insufficient Permissions]" and cost r114 its VRAM measurement.
#   6. /workspace is a MooseFS network mount, so `target/` is built on local
#      container disk and only the finished binary is copied to the volume.
#   7. A community host can report the GPU through nvidia-smi while no process
#      can create a CUDA context. That is gated in seconds, before any build.
#
# CARGO_BUILD_JOBS is 8 here, not the repository's usual 1. That rule exists
# for a shared 16 GB Mac; a pod has 50+ GB and 15+ vCPU, and a cold release
# build of the SP1 fork at one job would spend roughly an hour of billed time.
# It touches no arithmetic constraint, fixture, ELF identity, FRI parameter or
# verifier policy.
#
# Generates real proofs, so it is gated on PASTA_ALLOW_REAL_PROOF, which this
# script sets only after every identity check has passed.
set -euo pipefail

BUNDLE_SHA="${BUNDLE_SHA:?BUNDLE_SHA must be exported by the launcher}"
BUNDLE="${BUNDLE:-/workspace/mizu-bundle.tar.gz}"
# Overridable so a relaunch after a fixed gate unpacks into a fresh directory
# instead of needing a removal on the pod.
ROOT="${ROOT:-/workspace/mizu}"
RESULTS="${RESULTS:-/workspace/results}"
# The build cache, normally a mounted network volume. Set CACHE= (empty) to
# ignore any volume and always build from scratch.
CACHE="${CACHE-/workspace/mizu-cache}"
# target/ lives on local container disk: see lesson 6.
BUILD_DIR="${BUILD_DIR:-/root/mizu-target}"
TOOLCHAIN="${TOOLCHAIN:-1.92.0}"
ARMS="${ARMS:-retain-pasta-and-uint default}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"

CRATE="$ROOT/tools/pasta-proof"
MANIFEST="$ROOT/gpu/run-manifest.json"
ELF="$ROOT/gpu/elf/guest-program"
INPUTS="$ROOT/gpu/input"
BINDIR=/workspace/bin
BIN_NAME=pasta-precompile-proof-6-10

ulimit -n 65535 || true
mkdir -p "$RESULTS" "$BINDIR"
# A relaunch after a fixed gate keeps the same results directory, so a marker
# and a log from the previous attempt are still here. Rotate the marker: on
# 2026-11-10 a stale FAILED file made a healthy relaunch read as a failure.
if [ -f "$RESULTS/FAILED" ]; then
  mv "$RESULTS/FAILED" "$RESULTS/FAILED.before-$(date -u +%Y%m%dT%H%M%SZ)"
fi
exec > >(tee -a "$RESULTS/run.log") 2>&1
stamp() { echo "=== $* $(date -u +%FT%TZ) ==="; }
fail() { stamp "FAILED: $*"; echo "$*" > "$RESULTS/FAILED"; exit 1; }
stamp "run started"

# ---------------------------------------------------------------- 0. gates
# Fail before spending build time if the GPU is not usable. No `|| true`.
nvidia-smi
nvidia-smi -L
test "$(nvidia-smi --query-gpu=name --format=csv,noheader | wc -l)" -ge 1
nvidia-smi --query-gpu=name,memory.used,memory.total,utilization.gpu --format=csv \
  | tee "$RESULTS/telemetry-probe.csv"
if grep -qi 'Insufficient Permissions' "$RESULTS/telemetry-probe.csv"; then
  fail "nvidia-smi will not report memory/utilization; this is a MIG slice, not a whole card"
fi

# The cheap gate that catches a broken community host in seconds rather than
# after a full build. The PyTorch images carry torch; if a leaner image does
# not, say so loudly rather than pretending the gate ran.
if python3 -c 'import torch' 2>/dev/null; then
  python3 -c "import torch; assert torch.cuda.is_available(), 'no CUDA context'; \
print('cuda context ok:', torch.cuda.get_device_name(0))" \
    || fail "the GPU is visible but no process can create a CUDA context; discard this host"
else
  echo "WARNING: torch is absent, so the CUDA-context gate did not run."
  echo "WARNING: a broken host will now surface only at the first proof."
fi

# Architecture from the card itself, rather than hardcoded as in r123. A
# cached binary built for another architecture is useless here, which is why
# this value is part of the cache key. The comparison is containment, not
# equality: a fat build covering this card is reusable, a narrower one is not.
if [ -z "${CUDA_ARCHS:-}" ]; then
  CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d ' .')
  [ -n "$CC" ] || fail "could not read compute capability; set CUDA_ARCHS explicitly"
  CUDA_ARCHS="$CC"
fi
export CUDA_ARCHS
echo "building for CUDA_ARCHS=$CUDA_ARCHS"

echo "$BUNDLE_SHA  $BUNDLE" | sha256sum -c -

# ------------------------------------------------------------- 1. sources
test ! -e "$ROOT" || fail "$ROOT already exists; refusing to overwrite a previous unpack"
mkdir -p "$ROOT"
# --no-same-owner: the tarball is built on macOS and /workspace is a MooseFS
# mount where chown is refused even as root. Without this, tar reports "Cannot
# change ownership to uid 501" for every member and exits non-zero, and
# `set -e` kills the run before anything is built. That is exactly what
# happened on r123's first launch. --no-xattrs is the other half of the fix.
tar xzf "$BUNDLE" -C "$ROOT" --no-same-owner --no-xattrs
for required in "$ELF" "$MANIFEST" "$INPUTS/proof-input.bin" \
                "$INPUTS/receipt-input.bin" "$INPUTS/public-values.bin" \
                "$CRATE/Cargo.lock"; do
  test -f "$required" || fail "the bundle is missing $required"
done

# ------------------------------------------------------- 2. prerequisites
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev git curl ca-certificates \
  python3 python3-pip tmux time procps

python3 -m pip install --quiet --break-system-packages 'cmake>=3.30'
hash -r
cmake --version | head -1

export CUDA_HOME=/usr/local/cuda
export PATH="$CUDA_HOME/bin:$PATH"
export CUDACXX="$CUDA_HOME/bin/nvcc"
if [ ! -x "$CUDACXX" ]; then
  FOUND=$(find /usr/local -maxdepth 6 -type f -name nvcc 2>/dev/null | head -1)
  [ -n "$FOUND" ] || fail "no nvcc anywhere under /usr/local"
  export CUDACXX="$FOUND"
  export PATH="$(dirname "$FOUND"):$PATH"
fi
test -x "$CUDACXX"
nvcc --version | tail -2

# A volume that already holds a cargo and rustup home saves the toolchain
# download and every crate fetch on top of the build itself.
CACHE_USABLE=0
if [ -n "$CACHE" ] && mkdir -p "$CACHE" 2>/dev/null && [ -w "$CACHE" ]; then
  CACHE_USABLE=1
  export CARGO_HOME="$CACHE/cargo-home"
  export RUSTUP_HOME="$CACHE/rustup-home"
  mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
  echo "build cache at $CACHE ($(df -h "$CACHE" | awk 'NR==2{print $4" free on "$1}'))"
else
  echo "no writable cache at ${CACHE:-<disabled>}; this run builds from scratch"
fi

if ! command -v cargo >/dev/null && [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" ]; then
  . "${CARGO_HOME:-$HOME/.cargo}/env"
fi
if ! command -v cargo >/dev/null; then
  curl -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain "$TOOLCHAIN" --profile minimal --no-modify-path
  . "${CARGO_HOME:-$HOME/.cargo}/env"
fi
rustup toolchain install "$TOOLCHAIN" --profile minimal 2>/dev/null || true
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
cargo --version
rustc --version

cd "$CRATE"

# --------------------------------------------------------- 3. identity
{
  echo "{"
  echo "  \"run\": \"$(python3 -I -c "import json;print(json.load(open('$MANIFEST'))['run'])")\","
  echo "  \"authorized_usd_cap\": $(python3 -I -c "import json;print(json.load(open('$MANIFEST'))['authorization']['total_usd_cap'])"),"
  echo "  \"rustc\": \"$(rustc --version)\","
  echo "  \"cargo\": \"$(cargo --version)\","
  echo "  \"cmake\": \"$(cmake --version | head -1)\","
  echo "  \"nvcc\": \"$(nvcc --version | grep -oE 'release [0-9.]+' | head -1)\","
  echo "  \"cuda_archs\": \"$CUDA_ARCHS\","
  echo "  \"nvidia_smi_driver\": \"$(nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -1)\","
  echo "  \"gpu_name\": \"$(nvidia-smi --query-gpu=name --format=csv,noheader | head -1)\","
  echo "  \"gpu_uuid\": \"$(nvidia-smi --query-gpu=uuid --format=csv,noheader | head -1)\","
  echo "  \"gpu_memory_total\": \"$(nvidia-smi --query-gpu=memory.total --format=csv,noheader | head -1)\","
  echo "  \"compute_cap\": \"$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1)\","
  echo "  \"host_vcpu_visible\": $(nproc),"
  echo "  \"host_mem_kb\": $(awk '/MemTotal/{print $2}' /proc/meminfo),"
  echo "  \"cpu_model\": \"$(awk -F': ' '/model name/{print $2;exit}' /proc/cpuinfo)\","
  echo "  \"disk_avail_kb\": $(df -k /workspace | awk 'NR==2{print $4}'),"
  echo "  \"cargo_build_jobs\": $CARGO_BUILD_JOBS,"
  echo "  \"bundle_sha256\": \"$BUNDLE_SHA\","
  echo "  \"elf_sha256\": \"$(sha256sum "$ELF" | cut -d' ' -f1)\","
  echo "  \"manifest_sha256\": \"$(sha256sum "$MANIFEST" | cut -d' ' -f1)\","
  echo "  \"cargo_lock_sha256\": \"$(sha256sum Cargo.lock | cut -d' ' -f1)\""
  echo "}"
} > "$RESULTS/build-identity.json"
cat "$RESULTS/build-identity.json"
python3 -I -c "import json;json.load(open('$RESULTS/build-identity.json'))"
# r124 exists because neither r120 nor r123 recorded its CPU model, and the
# 2.25x gap between them turned out to be per-core speed. Record it always.
awk -F': ' '/model name/{print $2;exit}' /proc/cpuinfo > "$RESULTS/cpu-model.txt" || true

# ------------------------------------------------- 4. the build, or the cache
GPU_BIN="$BINDIR/$BIN_NAME"
KEY_TMP="$RESULTS/cache-key.json"
CACHE_HIT=0

# SOURCE_FP lets the launcher pass the fingerprint it computed at home; the
# hash is identical either way, and it is what makes a bundle-free prove run
# possible on the next pod.
if [ -n "${SOURCE_FP:-}" ]; then
  python3 -I /workspace/cache-key.py --source-fingerprint "$SOURCE_FP" \
    --archs "$CUDA_ARCHS" --toolchain "$TOOLCHAIN" --write "$KEY_TMP" > /dev/null
else
  python3 -I /workspace/cache-key.py --root "$ROOT" --archs "$CUDA_ARCHS" \
    --toolchain "$TOOLCHAIN" --write "$KEY_TMP" > /dev/null
fi
KEY=$(python3 -I -c "import json;print(json.load(open('$KEY_TMP'))['key'])")
echo "build identity key: $KEY"

if [ "$CACHE_USABLE" = 1 ] && [ -f "$CACHE/key.json" ] && [ -f "$CACHE/bin/$BIN_NAME" ]; then
  stamp "checking the cached build against this host and these sources"
  if python3 -I /workspace/cache-key.py --root "$ROOT" --archs "$CUDA_ARCHS" \
       --toolchain "$TOOLCHAIN" --compare "$CACHE/key.json" > /dev/null \
     && ( cd "$CACHE/bin" && sha256sum -c "$BIN_NAME.sha256" ); then
    cp "$CACHE/bin/$BIN_NAME" "$GPU_BIN"
    chmod +x "$GPU_BIN"
    # A cached binary is not trusted because it exists: it has to run here.
    if "$GPU_BIN" --mode registration --output "$RESULTS/registration.json" > /dev/null; then
      CACHE_HIT=1
      stamp "reusing the cached CUDA build; no compilation this run"
      # A cache populated before artefacts existed gets them now, so the next
      # run can take the bundle-free prove path.
      if [ ! -f "$CACHE/artifacts/elf/guest-program" ]; then
        mkdir -p "$CACHE/artifacts/elf" "$CACHE/artifacts/input"
        cp "$ELF" "$CACHE/artifacts/elf/guest-program"
        cp "$MANIFEST" "$CACHE/artifacts/run-manifest.json"
        cp "$INPUTS"/proof-input.bin "$INPUTS"/receipt-input.bin \
           "$INPUTS"/public-values.bin "$CACHE/artifacts/input/"
        echo "artefacts added to the cache for the prove-only path"
      fi
      cp "$CACHE/key.json" "$RESULTS/cache-key-reused.json"
    else
      echo "the cached binary does not run on this host; rebuilding"
      rm -f "$GPU_BIN"
    fi
  else
    echo "cache not reusable for this host/sources; rebuilding"
  fi
fi

if [ "$CACHE_HIT" = 0 ]; then
  export CARGO_TARGET_DIR="$BUILD_DIR"
  mkdir -p "$CARGO_TARGET_DIR"
  if [ "$CACHE_USABLE" = 1 ] && [ -d "$CACHE/target" ]; then
    # An incremental seed, when a previous run left one and the key moved only
    # because of a source edit. Copying it in is cheaper than a cold build and
    # keeps the writes off the network mount.
    stamp "seeding $CARGO_TARGET_DIR from the volume's previous target/"
    cp -a "$CACHE/target/." "$CARGO_TARGET_DIR/" 2>/dev/null || true
  fi
  stamp "building WITH --features cuda (arch $CUDA_ARCHS, jobs $CARGO_BUILD_JOBS)"
  set +e
  /usr/bin/time -v cargo build --release --locked --features cuda --bin "$BIN_NAME" \
    2>&1 | tee "$RESULTS/cuda-build.log" | tail -40
  RC=${PIPESTATUS[0]}
  set -e
  if [ "$RC" -ne 0 ]; then
    fail "the CUDA build failed (rc $RC); see cuda-build.log. If it is a toolchain floor, relaunch with TOOLCHAIN=1.97.1, which is what wazabi r123 used for the 6.10 CUDA stack"
  fi
  cp "$CARGO_TARGET_DIR/release/$BIN_NAME" "$GPU_BIN"
  chmod +x "$GPU_BIN"
  "$GPU_BIN" --mode registration --output "$RESULTS/registration.json" > /dev/null \
    || fail "the freshly built harness cannot even report chip registration"

  if [ "$CACHE_USABLE" = 1 ]; then
    stamp "populating the cache for later runs"
    mkdir -p "$CACHE/bin" "$CACHE/artifacts/elf" "$CACHE/artifacts/input"
    cp "$GPU_BIN" "$CACHE/bin/$BIN_NAME"
    ( cd "$CACHE/bin" && sha256sum "$BIN_NAME" > "$BIN_NAME.sha256" )
    cp "$KEY_TMP" "$CACHE/key.json"
    # The artefacts too, so a later run needs no bundle at all: pod-prove.sh
    # reads the ELF, the inputs and the manifest straight from here. They are
    # pinned by hash in the manifest, so a stale copy cannot pass pre-flight.
    cp "$ELF" "$CACHE/artifacts/elf/guest-program"
    cp "$MANIFEST" "$CACHE/artifacts/run-manifest.json"
    cp "$INPUTS"/proof-input.bin "$INPUTS"/receipt-input.bin "$INPUTS"/public-values.bin \
       "$CACHE/artifacts/input/"
    if [ "${CACHE_TARGET:-0}" = 1 ]; then
      # Off by default: target/ is several GB and the volume bills per GB.
      rm -rf "$CACHE/target.new"
      cp -a "$CARGO_TARGET_DIR" "$CACHE/target.new" && rm -rf "$CACHE/target" \
        && mv "$CACHE/target.new" "$CACHE/target"
    fi
    {
      echo "Mizu GPU build cache. Safe to delete; the next run rebuilds."
      echo "key.json identifies the build (os, nvcc, rustc, CUDA_ARCHS, sources)."
      echo "bin/ holds the CUDA harness and its sha256."
      echo "cargo-home/ and rustup-home/ save the downloads, not the compile."
      echo "written $(date -u +%FT%TZ)"
    } > "$CACHE/README.txt"
    du -sh "$CACHE"/* 2>/dev/null || true
  fi
fi

ls -l "$GPU_BIN" | tee "$RESULTS/gpu-binary-size.txt"
sha256sum "$GPU_BIN" | tee "$RESULTS/gpu-binary-sha256.txt"
echo "{\"cache_hit\": $CACHE_HIT, \"key\": \"$KEY\"}" > "$RESULTS/cache-status.json"

# ------------------------------------------------------------ 5. pre-flight
# Validates the manifest, the ELF hash, both input hashes, the expected public
# values and every arm name without executing or proving, so a bad pin costs
# nothing.
stamp "pre-flight: every pinned identity, before any proving"
"$GPU_BIN" --mode manifest-check --manifest "$MANIFEST" --elf "$ELF" \
  --input "$INPUTS" --output "$RESULTS/preflight.json" > /dev/null \
  || fail "manifest pre-flight failed"
python3 -I -c "import json;d=json.load(open('$RESULTS/preflight.json'));print('arms:',[a['name'] for a in d['arms']])"

export PASTA_ALLOW_REAL_PROOF=1

# ------------------------------------------------------------- 6. the proofs
for ARM in $ARMS; do
  stamp "proving arm $ARM as a CUDA core proof"
  OUT="$RESULTS/$ARM"
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,memory.used,memory.total \
    --format=csv,noheader,nounits -lms 200 > "$RESULTS/gpu-samples-$ARM.csv" &
  SAMPLER=$!
  ( while true; do echo "$(date +%s) $(ps -eo rss= | awk '{s+=$1} END {print s}')"; sleep 0.5; done ) \
    > "$RESULTS/host-rss-$ARM.txt" &
  RSSMON=$!
  set +e
  # The manifest is authoritative for every SP1CoreOpts field; these env knobs
  # are the GPU-side ones, set explicitly and identically to r123/r124 so the
  # timings stay comparable with those runs.
  RUST_LOG="${RUST_LOG:-sp1_core_machine=info}" \
  PASTA_FRI_CONFIG=core PASTA_LOG_STACKING_HEIGHT=21 PASTA_MAX_LOG_ROW_COUNT=22 \
  PASTA_GPU_MAX_TRACE_SIZE=404750336 PASTA_GPU_WORKERS=2 \
  PASTA_R5_INFLIGHT="${PASTA_R5_INFLIGHT:-8}" \
  PASTA_PROVE_INFLIGHT="${PASTA_PROVE_INFLIGHT:-1}" \
  /usr/bin/time -v "$GPU_BIN" --mode manifest-core-gpu \
    --manifest "$MANIFEST" --arm "$ARM" --elf "$ELF" \
    --input "$INPUTS" --output "$OUT" 2>&1 | tee "$RESULTS/$ARM.log"
  RC=${PIPESTATUS[0]}
  set -e
  kill $SAMPLER $RSSMON 2>/dev/null || true
  [ "$RC" -eq 0 ] || fail "arm $ARM exited $RC"

  # The mean matters, not the peak: r120 recorded only a peak of 100% and
  # three runs then read as GPU-bound while the card was idle most of the time.
  python3 -I - "$RESULTS/gpu-samples-$ARM.csv" "$ARM" <<'PY' | tee -a "$RESULTS/telemetry-summary.txt"
import csv, sys
from datetime import datetime
rows = []
with open(sys.argv[1]) as fh:
    for r in csv.reader(fh):
        if len(r) < 5:
            continue
        try:
            t = datetime.strptime(r[0].strip()[:23], "%Y/%m/%d %H:%M:%S.%f")
            rows.append((t, int(r[1]), int(r[3])))
        except (ValueError, IndexError):
            continue
if not rows:
    print(f"{sys.argv[2]} NO SAMPLES")
    raise SystemExit(0)
t0 = rows[0][0]
s = [((t - t0).total_seconds(), u, m) for t, u, m in rows]
span = s[-1][0]
busy = sum((b[0] - a[0]) * (a[1] + b[1]) / 200.0 for a, b in zip(s, s[1:]))
util = [u for _, u, _ in s]
print(f"{sys.argv[2]} samples={len(s)} span_s={span:.1f} gpu_busy_s={busy:.1f} "
      f"gpu_idle_s={span-busy:.1f} util_mean_pct={sum(util)/len(util):.1f} "
      f"util_median_pct={sorted(util)[len(util)//2]} "
      f"zero_samples_pct={100*sum(1 for u in util if u==0)/len(util):.1f} "
      f"peak_vram_mib={max(m for _, _, m in s)}")
PY
  du -sh "$OUT" | tee -a "$RESULTS/arm-sizes.txt"
done

# ------------------------------------- 7. reload verification, on this host
# A proof file is not acceptance. This reloads every arm's proof from disk in
# a fresh process, derives the key from the pinned ELF and checks the
# committed digest against the expected public values exported before any
# proving. It generates nothing.
stamp "fresh-process reload verification of every arm proved here"
# Per arm, with --arm, because a run that proves a subset of the manifest's
# arms must be able to pass: verifying the whole manifest fails on the arms it
# never proved, which is how the 2026-11-10 Montreal run ended with a passing
# proof and an exit code of 1.
for ARM in $ARMS; do
  set +e
  "$GPU_BIN" --mode manifest-core-verify \
    --manifest "$MANIFEST" --elf "$ELF" --arm "$ARM" \
    --input "$RESULTS" \
    --reference "$INPUTS/public-values.bin" \
    --output "$RESULTS/verification-$ARM" 2>&1 | tee -a "$RESULTS/verification.log"
  RC=${PIPESTATUS[0]}
  set -e
  [ "$RC" -eq 0 ] || fail "on-pod reload verification of $ARM exited $RC"
done

nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv > "$RESULTS/gpu-info.csv"
sha256sum "$RESULTS"/*/*.proof.bin 2>/dev/null | tee "$RESULTS/proof-hashes.txt" || true
ls -l "$RESULTS"/*/*.proof.bin 2>/dev/null | tee -a "$RESULTS/proof-hashes.txt" || true
cat "$RESULTS/telemetry-summary.txt" 2>/dev/null || true
echo ok > "$RESULTS/ALL_STAGES_PASSED"
stamp "finished: every arm proved and verified from disk"
