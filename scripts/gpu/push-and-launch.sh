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
#   CUDA_ARCHS        override the architecture read from the card
#   CARGO_BUILD_JOBS  build parallelism (default 8; never $(nproc), see pod-run)
set -euo pipefail

HOST="${1:?usage: push-and-launch.sh <host> <port> <bundle-path>}"
PORT="${2:?missing port}"
BUNDLE="${3:?missing bundle path}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

DEADLINE_MINUTES="${DEADLINE_MINUTES:?DEADLINE_MINUTES is required; it is the cost ceiling}"
RATE_PER_HOUR="${RATE_PER_HOUR:?RATE_PER_HOUR is required; use the rate the created pod reported}"
CAP_USD="${CAP_USD:-unset}"
JOBS="${CARGO_BUILD_JOBS:-8}"
POD_ROOT="${POD_ROOT:-/workspace/mizu}"
CACHE="${CACHE-/workspace/mizu-cache}"
ARMS="${ARMS:-retain-pasta-and-uint default}"
TOOLCHAIN="${TOOLCHAIN:-1.92.0}"

SSHOPTS=(-p "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)
SCPOPTS=(-P "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)

echo "=== probing direct ssh to $HOST:$PORT ==="
ssh "${SSHOPTS[@]}" "root@$HOST" \
  'echo ok; nvidia-smi --query-gpu=name,compute_cap,memory.total,driver_version --format=csv,noheader; nproc; free -g | head -2; df -h /workspace | tail -1; ls -la /workspace/mizu-cache 2>/dev/null || echo "no cache on this pod"'

echo "=== uploading ($(wc -c < "$BUNDLE" | tr -d ' ') bytes) ==="
time scp "${SCPOPTS[@]}" "$BUNDLE" "root@$HOST:/workspace/mizu-bundle.tar.gz"
scp "${SCPOPTS[@]}" "$HERE/pod-run.sh" "$HERE/watchdog.sh" "$HERE/cache-key.py" \
  "root@$HOST:/workspace/"

echo "=== verifying the uploaded bundle hash on the pod ==="
WANT=$(shasum -a 256 "$BUNDLE" | cut -d' ' -f1)
GOT=$(ssh "${SSHOPTS[@]}" "root@$HOST" 'sha256sum /workspace/mizu-bundle.tar.gz' | cut -d' ' -f1)
if [ "$WANT" != "$GOT" ]; then
  echo "upload corrupted: local $WANT, pod $GOT" >&2
  exit 1
fi
echo "bundle intact on the pod: $GOT"

echo "=== arming the watchdog and launching, both detached under tmux ==="
# tmux, not `setsid nohup ... &`: in r114 the latter did not survive the
# single SSH command that started it, and the run never began.
ssh "${SSHOPTS[@]}" "root@$HOST" "set -e
  apt-get update -qq >/dev/null 2>&1 || true
  command -v tmux >/dev/null || apt-get install -y -qq tmux >/dev/null 2>&1
  chmod +x /workspace/pod-run.sh /workspace/watchdog.sh /workspace/cache-key.py
  tmux kill-server 2>/dev/null || true
  tmux new-session -d -s watchdog \"DEADLINE_MINUTES=$DEADLINE_MINUTES RATE_PER_HOUR=$RATE_PER_HOUR CAP_USD=$CAP_USD bash /workspace/watchdog.sh\"
  tmux new-session -d -s mizu \"BUNDLE_SHA=$WANT CARGO_BUILD_JOBS=$JOBS ROOT=$POD_ROOT CACHE='$CACHE' ARMS='$ARMS' TOOLCHAIN=$TOOLCHAIN ${CUDA_ARCHS:+CUDA_ARCHS=$CUDA_ARCHS} bash /workspace/pod-run.sh\"
  sleep 2
  tmux ls"
echo "=== launched; watchdog deadline ${DEADLINE_MINUTES}m at \$${RATE_PER_HOUR}/hr against a \$${CAP_USD} cap ==="
echo "follow it with:  ssh ${SSHOPTS[*]} root@$HOST 'tail -f /workspace/results/run.log'"
