#!/usr/bin/env bash
# The container start command for a SLIM pod: stock image, no registry, no build.
#
# This is not run from here. It is the body of a pod's start command: pass it to
# create-pod as
#
#   "entrypoint": ["/bin/bash","-c"],
#   "cmd": ["<the contents of this file>"],
#   "startSsh": true, "ports": ["22/tcp"], "image": "nvidia/cuda:12.8.1-base-ubuntu24.04"
#
# Why it exists. The drivers ran on runpod/pytorch (10,564 MB compressed) only
# because torch answered the CUDA-context gate, and --mode cuda-probe replaced
# that on 2026-11-11. What then looked like a blocker -- a stock nvidia/cuda
# image has no sshd, so scp and tmux are impossible -- is not one: Runpod's own
# docs give a start command for exactly this, `startSsh: true` injects the
# account's keys as $PUBLIC_KEY, and Secure Cloud pods are TCP-enabled, so sshd
# can be installed at boot instead of baked into an image. One apt (~10-20 s)
# in place of ~110 s of image pull, and no registry credential on a rented host.
#   docs.runpod.io/pods/configuration/use-ssh ("Full SSH via public IP")
#
# Nothing below is measured. It has never been run. The first two commands after
# sshd exist to settle the two assumptions a slim image rests on, before any
# money goes into proving: that the host driver is injected into a base image
# (nvidia-smi, libcuda.so.1) as it is into every CUDA container, and that the
# harness needs nothing dynamically beyond cudart, which this image carries as
# `cuda-cudart-12-8` (verified in the image's own build history).
set -eu
export DEBIAN_FRONTEND=noninteractive

# libgomp1: the GPU harness links -lgomp (sp1-gpu-sys build.rs). It is in the
# PyTorch image and is NOT in a bare Ubuntu base, so it is the one dependency
# that would fail late and confusingly if it were forgotten.
# python3-minimal: the cache-key check and the drivers' JSON.
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  openssh-server libgomp1 libstdc++6 python3-minimal ca-certificates

mkdir -p /root/.ssh /run/sshd
chmod 700 /root/.ssh
if [ -n "${PUBLIC_KEY:-}" ]; then
  printf '%s\n' "$PUBLIC_KEY" >> /root/.ssh/authorized_keys
  chmod 600 /root/.ssh/authorized_keys
else
  echo "WARNING: PUBLIC_KEY is empty -- pass startSsh:true and register a key, or this pod is unreachable"
fi
ssh-keygen -A
/usr/sbin/sshd
echo "sshd started: $(pgrep -x sshd | tr '\n' ' ')"

# --------------------------------------------------- the two open assumptions
nvidia-smi -L || echo "ASSUMPTION FAILED: nvidia-smi was not injected into this image"

BIN=/workspace/mizu-cache/bin/pasta-precompile-proof-6-10
if [ -x "$BIN" ]; then
  # Every DT_NEEDED the proving binary has, against what this image provides.
  # 'not found' here is the whole question: cudart is present, and nothing else
  # should be missing.
  ldd "$BIN" | tee /root/slim-image-ldd.txt || true
  if grep -q 'not found' /root/slim-image-ldd.txt; then
    echo "ASSUMPTION FAILED: the slim image is missing a shared library (see above)"
  else
    echo "every shared library resolved on a $(. /etc/os-release; echo "$PRETTY_NAME") base image"
    "$BIN" --mode cuda-probe --output /root/slim-image-cuda-probe.json \
      && cat /root/slim-image-cuda-probe.json \
      || echo "ASSUMPTION FAILED: no CUDA context on a base image"
  fi
else
  echo "note: $BIN absent -- mount the build-cache volume at /workspace to settle this here"
fi

# Hold the container open for push-and-launch.sh, which drives the run exactly
# as it does on the PyTorch image. The pod cannot terminate itself (no API key
# goes on a rented host), so the operator deletes it.
echo "=== slim pod ready $(date -u +%FT%TZ) ==="
sleep infinity
