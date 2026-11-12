#!/usr/bin/env bash
# The job, run by the container rather than by a person over SSH.
#
# This is the other half of the 2026-11-11 overhead: a cache-hit run spent
# ~112 s waiting for the image to pull and ssh.direct to appear, then ~70 s of
# launcher round trips, before 139 s of proof. An entrypoint starts the work as
# soon as the container is up, so SSH becomes a convenience for collection
# rather than a precondition for starting.
#
# Progress is read with RunPod's own log stream (`stream-pod-logs`), so nothing
# here depends on a shell being attached. The pod cannot terminate itself — no
# API key goes on a rented host — so it halts at its deadline and the operator
# deletes it.
#
# Environment, all optional except the cap-enforcing deadline:
#   ARMS              manifest arms to prove, default retain-pasta-and-uint
#   CUDA_ARCHS        the architectures the baked binary was built for; this
#                     must match, because it is part of the cache key
#   DEADLINE_MINUTES  self-halt deadline; the cost ceiling
#   RESULTS           default /root/results (container disk)
#   PASTA_R5_INFLIGHT default 8, the measured knee
set -euo pipefail

HOME_DIR="${MIZU_HOME:-/opt/mizu}"
RESULTS="${RESULTS:-/root/results}"
ARMS="${ARMS:-retain-pasta-and-uint}"
DEADLINE_MINUTES="${DEADLINE_MINUTES:-30}"
BIN="$HOME_DIR/bin/pasta-precompile-proof-6-10"
ELF="$HOME_DIR/gpu/elf/guest-program"
MANIFEST="$HOME_DIR/gpu/run-manifest.json"
INPUTS="$HOME_DIR/gpu/input"

mkdir -p "$RESULTS"
exec > >(tee -a "$RESULTS/run.log") 2>&1
stamp() { echo "=== $* $(date -u +%FT%TZ) ==="; }
fail() { stamp "FAILED: $*"; echo "$*" > "$RESULTS/FAILED"; sleep 2; exit 1; }
# One nvidia-smi field, trimmed, never fatal. ECC mode is the reason this exists:
# the A40 pool ships both ECC-on and ECC-off parts and the ECC-off ones proved
# 11.8% faster, so every run records the card's ECC state as identity, not as a
# gate -- a card that cannot report it ("[N/A]", "Not Supported", an absent
# nvidia-smi) is still a card we proved on. See docs/running-a-proof.md.
smi() {
  nvidia-smi --query-gpu="$1" --format=csv,noheader 2>/dev/null \
    | head -1 | sed 's/^ *//; s/ *$//' || echo unknown
}

stamp "entrypoint started"

# The deadline halts the container even if everything else wedges. It is a
# backstop, not a schedule, and it is armed before any work.
(
  sleep $(( DEADLINE_MINUTES * 60 ))
  echo "=== deadline of ${DEADLINE_MINUTES}m reached, halting $(date -u +%FT%TZ) ===" \
    | tee -a "$RESULTS/watchdog.log"
  sync
  poweroff 2>/dev/null || kill -TERM 1 2>/dev/null || kill -KILL 1
) &
echo "watchdog armed for ${DEADLINE_MINUTES} minutes"

# ---------------------------------------------------------------- gates
nvidia-smi --query-gpu=name,compute_cap,memory.total,driver_version --format=csv \
  | tee "$RESULTS/telemetry-probe.csv"
if grep -qi 'Insufficient Permissions' "$RESULTS/telemetry-probe.csv"; then
  fail "nvidia-smi will not report memory/utilization; this is a MIG slice, not a whole card"
fi
# torch is deliberately absent from this image, so the context gate is the
# harness's own: it initialises CUDA and refuses a device reporting no memory.
"$BIN" --mode cuda-probe --output "$RESULTS/cuda-probe.json" \
  || fail "the GPU is visible but no process can create a CUDA context; discard this host"

for required in "$BIN" "$ELF" "$MANIFEST" "$INPUTS/proof-input.bin" \
                "$INPUTS/receipt-input.bin" "$INPUTS/public-values.bin"; do
  test -f "$required" || fail "the image is missing $required"
done

if [ -z "${CUDA_ARCHS:-}" ]; then
  CUDA_ARCHS=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d ' .')
  echo "WARNING: CUDA_ARCHS not set; recording the card's own $CUDA_ARCHS."
fi
{
  echo "{"
  echo "  \"mode\": \"entrypoint\","
  echo "  \"cuda_archs\": \"$CUDA_ARCHS\","
  echo "  \"gpu_name\": \"$(nvidia-smi --query-gpu=name --format=csv,noheader | head -1)\","
  echo "  \"gpu_uuid\": \"$(nvidia-smi --query-gpu=uuid --format=csv,noheader | head -1)\","
  echo "  \"driver\": \"$(smi driver_version)\","
  echo "  \"gpu_memory_total\": \"$(smi memory.total)\","
  echo "  \"ecc_mode_current\": \"$(smi ecc.mode.current)\","
  echo "  \"ecc_mode_pending\": \"$(smi ecc.mode.pending)\","
  echo "  \"cpu_model\": \"$(awk -F': ' '/model name/{print $2;exit}' /proc/cpuinfo)\","
  echo "  \"host_vcpu_visible\": $(nproc),"
  echo "  \"elf_sha256\": \"$(sha256sum "$ELF" | cut -d' ' -f1)\","
  echo "  \"harness_sha256\": \"$(sha256sum "$BIN" | cut -d' ' -f1)\""
  echo "}"
} > "$RESULTS/build-identity.json"
cat "$RESULTS/build-identity.json"

stamp "pre-flight: every pinned identity, before any proving"
"$BIN" --mode manifest-check --manifest "$MANIFEST" --elf "$ELF" \
  --input "$INPUTS" --output "$RESULTS/preflight.json" > /dev/null \
  || fail "manifest pre-flight failed"

export PASTA_ALLOW_REAL_PROOF=1

for ARM in $ARMS; do
  stamp "proving arm $ARM as a CUDA core proof"
  nvidia-smi --query-gpu=timestamp,utilization.gpu,utilization.memory,memory.used,memory.total \
    --format=csv,noheader,nounits -lms 200 > "$RESULTS/gpu-samples-$ARM.csv" &
  SAMPLER=$!
  set +e
  RUST_LOG="${RUST_LOG:-sp1_core_machine=info}" \
  PASTA_SPAN_TIMING="${PASTA_SPAN_TIMING:-0}" \
  PASTA_FRI_CONFIG=core PASTA_LOG_STACKING_HEIGHT=21 PASTA_MAX_LOG_ROW_COUNT=22 \
  PASTA_GPU_MAX_TRACE_SIZE=404750336 PASTA_GPU_WORKERS=2 \
  PASTA_R5_INFLIGHT="${PASTA_R5_INFLIGHT:-8}" \
  PASTA_PROVE_INFLIGHT="${PASTA_PROVE_INFLIGHT:-1}" \
  /usr/bin/time -v "$BIN" --mode manifest-core-gpu \
    --manifest "$MANIFEST" --arm "$ARM" --elf "$ELF" \
    --input "$INPUTS" --output "$RESULTS/$ARM" 2>&1 | tee "$RESULTS/$ARM.log"
  RC=${PIPESTATUS[0]}
  set -e
  kill $SAMPLER 2>/dev/null || true
  [ "$RC" -eq 0 ] || fail "arm $ARM exited $RC"
done

stamp "fresh-process reload verification of every arm proved here"
for ARM in $ARMS; do
  set +e
  "$BIN" --mode manifest-core-verify --manifest "$MANIFEST" --elf "$ELF" --arm "$ARM" \
    --input "$RESULTS" --reference "$INPUTS/public-values.bin" \
    --output "$RESULTS/verification-$ARM" 2>&1 | tee -a "$RESULTS/verification.log"
  RC=${PIPESTATUS[0]}
  set -e
  [ "$RC" -eq 0 ] || fail "on-pod reload verification of $ARM exited $RC"
done

sha256sum "$RESULTS"/*/*.proof.bin 2>/dev/null | tee "$RESULTS/proof-hashes.txt" || true
echo ok > "$RESULTS/ALL_STAGES_PASSED"
stamp "finished: proved and verified, with nothing installed and nothing uploaded"
# Keep the container alive briefly so the log stream and any collection can
# drain, then let the watchdog or the operator end it.
sleep "${LINGER_SECONDS:-120}"
