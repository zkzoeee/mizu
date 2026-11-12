#!/usr/bin/env bash
# Upload the bundle and the driver scripts over DIRECT ssh, then launch.
#
# Ported from wazabi r123. Its warning is worth repeating: r114 lost a pod to
# a transfer mistake, because the RunPod PROXY host is PTY-only (no scp, no
# sftp), so the bundle had to go through a base64-over-PTY push, and widening
# that push's `fold` width from the proven 1000 characters to 8000 made the
# PTY silently accept nothing — zero bytes in ten minutes. The fix is not a
# better PTY push: it is direct SSH, which supports scp.
#
# Direct SSH needs a 22/tcp port and startSsh on the pod, and its address only
# appears in the pod's `ssh.direct` once runtime is non-null. r114 read that
# field while the pod was still provisioning and wrongly concluded direct SSH
# was unavailable. Wait for it; on r120's pod it appeared at 143 s of uptime.
#
# Usage: push-and-launch.sh <host> <port> <bundle-path>
#
# Environment:
#   DEADLINE_MINUTES  watchdog deadline (required: it is the cost ceiling)
#   RATE_PER_HOUR     the rate the CREATED pod reported, not the catalogue's
#   CAP_USD           the cap this run is authorized against, for the log
#   CACHE             volume path to reuse/populate ("" disables the cache)
#   ARMS              which manifest arms to prove
#   TOOLCHAIN         Rust toolchain (default 1.92.0, this repo's pin)
#   CUDA_ARCHS        override the architecture read from the card. Rarely
#                     needed now: the cache probe reads the card's own
#                     compute capability and a stored build counts as a hit
#                     when its arch list covers that.
#   CARGO_BUILD_JOBS  build parallelism (default 8; never $(nproc), see pod-run)
set -euo pipefail

HOST="${1:?usage: push-and-launch.sh <host> <port> <bundle-path>}"
PORT="${2:?missing port}"
# Optional since 2026-11-11: a prove-only run on a warm cache ships no bundle,
# so requiring a 72 MB tarball on disk to start one was a leftover. Still
# required for a build, and checked once the probe has decided which it is.
BUNDLE="${3:-}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

DEADLINE_MINUTES="${DEADLINE_MINUTES:?DEADLINE_MINUTES is required; it is the cost ceiling}"
RATE_PER_HOUR="${RATE_PER_HOUR:?RATE_PER_HOUR is required; use the rate the created pod reported}"
CAP_USD="${CAP_USD:-unset}"
JOBS="${CARGO_BUILD_JOBS:-8}"
# Container disk by default, not the volume: when a network volume is mounted
# at /workspace, an unpacked tree and a results directory there survive the pod
# and block the next run ("$ROOT already exists", "use a fresh output
# directory"), besides eating the volume's quota. Only the cache belongs on the
# volume.
POD_ROOT="${POD_ROOT:-/root/mizu}"
POD_RESULTS="${POD_RESULTS:-/root/results}"
CACHE="${CACHE-/workspace/mizu-cache}"
ARMS="${ARMS:-retain-pasta-and-uint default}"
TOOLCHAIN="${TOOLCHAIN:-1.92.0}"

SSHOPTS=(-p "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)
SCPOPTS=(-P "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)

echo "=== probing direct ssh to $HOST:$PORT ==="
ssh "${SSHOPTS[@]}" "root@$HOST" \
  'echo ok; nvidia-smi --query-gpu=name,compute_cap,memory.total,ecc.mode.current,driver_version --format=csv,noheader; nproc; free -g | head -2; df -h /workspace | tail -1; ls -la /workspace/mizu-cache 2>/dev/null || echo "no cache on this pod"'

# The scripts go first and cost nothing (about 30 KB). The 72 MB bundle goes
# only if the pod cannot already prove this exact build. The source
# fingerprint is the one part of the cache key that needs the sources, so it is
# computed here, where they are, and the pod answers with its own OS, nvcc,
# rustc and CUDA_ARCHS. On a hit, the pod needs no bundle, no toolchain and no
# apt: 2026-11-11 measured those at 29 s of upload and 22 s of prerequisites
# per run, both producing nothing a prebuilt binary uses.
scp "${SCPOPTS[@]}" "$HERE/pod-run.sh" "$HERE/pod-prove.sh" "$HERE/watchdog.sh" \
  "$HERE/cache-key.py" "root@$HOST:/workspace/"

REPO="$(cd "$HERE/../.." && pwd)"
SOURCE_FP="$(python3 -I "$HERE/cache-key.py" --root "$REPO" --fingerprint-only)"
echo "=== source fingerprint: ${SOURCE_FP:0:16} ==="

# The architecture to probe with is the CARD's, read from the pod, not a
# constant. r123's hardcoded 86 happened to be right for an A40 and still cost
# a launch on 2026-11-12: the cache on the volume was built fat for 86,89,120,
# the probe compared the string, and a warm cache read as a miss. cache-key.py
# now accepts a stored build whose arch list COVERS what this card needs, so
# the honest value to ask with is the capability the card reports.
if [ -n "${CUDA_ARCHS:-}" ]; then
  PROBE_ARCHS="$CUDA_ARCHS"
else
  PROBE_ARCHS="$(ssh "${SSHOPTS[@]}" "root@$HOST" \
    'nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d " ."' \
    2>/dev/null || true)"
  if [ -z "$PROBE_ARCHS" ]; then
    echo "WARNING: could not read compute_cap from the pod; probing with 86" >&2
    PROBE_ARCHS=86
  fi
fi
echo "=== probing the cache for architecture $PROBE_ARCHS ==="
MODE=build
if [ -n "$CACHE" ]; then
  echo "=== asking the pod whether it already holds this build ==="
  # The artefacts must be there too, or the prove path would fail on a pod it
  # has already paid to boot. A cache populated before artefacts existed
  # therefore takes the build path once more, which populates them.
  PROBE="test -f $CACHE/key.json && test -f $CACHE/artifacts/elf/guest-program"
  PROBE="$PROBE && test -f $CACHE/bin/pasta-precompile-proof-6-10"
  PROBE="$PROBE && CARGO_HOME=$CACHE/cargo-home RUSTUP_HOME=$CACHE/rustup-home"
  PROBE="$PROBE PATH=$CACHE/cargo-home/bin:/usr/local/cuda/bin:\$PATH"
  PROBE="$PROBE RUSTUP_TOOLCHAIN=$TOOLCHAIN"
  PROBE="$PROBE python3 -I /workspace/cache-key.py --source-fingerprint $SOURCE_FP"
  PROBE="$PROBE --archs $PROBE_ARCHS --toolchain $TOOLCHAIN --compare $CACHE/key.json"
  if ssh "${SSHOPTS[@]}" "root@$HOST" "$PROBE" > /dev/null 2>&1; then
    MODE=prove
    echo "=== cache hit: skipping the bundle, the toolchain and apt ==="
  # A slim image has no nvcc, so the full key cannot match there however right
  # the binary is. Ask the narrower question the prove path actually needs: the
  # same OS, CUDA_ARCHS and sources. A hit means this pod can prove but could
  # not rebuild, which is exactly the trade a 101 MB image makes.
  elif ssh "${SSHOPTS[@]}" "root@$HOST" "$PROBE --runtime" > /dev/null 2>&1; then
    MODE=prove
    echo "=== cache hit in runtime scope: the build toolchain is absent or differs here,"
    echo "    and the binary still matches the OS, architectures and sources it was built for ==="
  else
    echo "=== cache miss or empty volume: shipping the bundle and building ==="
  fi
fi

# Fail before the upload, not after it: a build needs nvcc, and the image that
# makes the prove path cheap does not have one.
if [ "$MODE" = build ]; then
  if ! ssh "${SSHOPTS[@]}" "root@$HOST" \
    'command -v nvcc >/dev/null || ls /usr/local/cuda*/bin/nvcc >/dev/null 2>&1' 2>/dev/null; then
    echo "this pod has no nvcc, so it cannot build, and the cache does not hold this" >&2
    echo "build. Either relaunch on an image with the CUDA toolkit (the PyTorch image" >&2
    echo "or nvidia/cuda:...-devel) or populate the cache from one first." >&2
    exit 1
  fi
fi

WANT=none
[ -n "$BUNDLE" ] && WANT=$(shasum -a 256 "$BUNDLE" | cut -d' ' -f1)
if [ "$MODE" = build ]; then
  [ -n "$BUNDLE" ] || {
    echo "this pod cannot prove from the cache and no bundle was given." >&2
    echo "Build one with make-bundle.sh and pass it as the third argument." >&2
    exit 1
  }
  echo "=== uploading ($(wc -c < "$BUNDLE" | tr -d ' ') bytes) ==="
  time scp "${SCPOPTS[@]}" "$BUNDLE" "root@$HOST:/workspace/mizu-bundle.tar.gz"
  echo "=== verifying the uploaded bundle hash on the pod ==="
  GOT=$(ssh "${SSHOPTS[@]}" "root@$HOST" 'sha256sum /workspace/mizu-bundle.tar.gz' | cut -d' ' -f1)
  if [ "$WANT" != "$GOT" ]; then
    echo "upload corrupted: local $WANT, pod $GOT" >&2
    exit 1
  fi
  echo "bundle intact on the pod: $GOT"
fi

DRIVER=pod-run.sh
[ "$MODE" = prove ] && DRIVER=pod-prove.sh
echo "=== arming the watchdog and launching $DRIVER, both detached under tmux ==="
# tmux, not `setsid nohup ... &`: in r114 the latter did not survive the
# single SSH command that started it, and the run never began.
ssh "${SSHOPTS[@]}" "root@$HOST" "set -e
  apt-get update -qq >/dev/null 2>&1 || true
  command -v tmux >/dev/null || apt-get install -y -qq tmux >/dev/null 2>&1
  chmod +x /workspace/pod-run.sh /workspace/pod-prove.sh /workspace/watchdog.sh /workspace/cache-key.py
  tmux kill-server 2>/dev/null || true
  # A new-session immediately after kill-server races the dying server and
  # answers \"server exited unexpectedly\" without starting anything, which
  # cost this repository one launch on 2026-11-10.
  sleep 1
  tmux new-session -d -s watchdog \"DEADLINE_MINUTES=$DEADLINE_MINUTES RATE_PER_HOUR=$RATE_PER_HOUR CAP_USD=$CAP_USD LOG_DIR=$POD_RESULTS bash /workspace/watchdog.sh\"
  sleep 1
  tmux new-session -d -s mizu \"BUNDLE_SHA=$WANT SOURCE_FP=$SOURCE_FP CARGO_BUILD_JOBS=$JOBS ROOT=$POD_ROOT RESULTS=$POD_RESULTS CACHE='$CACHE' ARMS='$ARMS' TOOLCHAIN=$TOOLCHAIN ${CUDA_ARCHS:+CUDA_ARCHS=$CUDA_ARCHS} bash /workspace/$DRIVER\"
  sleep 2
  tmux ls"
echo "=== launched; watchdog deadline ${DEADLINE_MINUTES}m at \$${RATE_PER_HOUR}/hr against a \$${CAP_USD} cap ==="
echo "follow it with:  ssh ${SSHOPTS[*]} root@$HOST 'tail -f $POD_RESULTS/run.log'"
