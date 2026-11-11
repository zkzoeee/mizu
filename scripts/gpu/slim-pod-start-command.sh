#!/usr/bin/env bash
# The container start command for a SLIM pod: stock image, no registry, no build.
# Not run from here -- it is passed verbatim to create-pod as
#   "image": "nvidia/cuda:12.8.1-base-ubuntu24.04",
#   "entrypoint": ["/bin/bash","-c"], "cmd": ["<this file>"],
#   "startSsh": true, "ports": ["22/tcp"]
# The reasoning, the image sizes and the link set are in docs/running-a-proof.md.
# libgomp1 is the dependency that would otherwise fail late: the harness links
# -lgomp and a bare Ubuntu base has no libgomp. python3 -- NOT python3-minimal,
# which has no `json` and cost this a failed launch on 2026-11-11 -- is the
# cache key and the drivers' JSON; tmux is how push-and-launch.sh detaches.
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  openssh-server libgomp1 libstdc++6 python3 tmux ca-certificates

mkdir -p /root/.ssh /run/sshd
chmod 700 /root/.ssh
if [ -n "${PUBLIC_KEY:-}" ]; then
  printf '%s\n' "$PUBLIC_KEY" >> /root/.ssh/authorized_keys
  chmod 600 /root/.ssh/authorized_keys
else
  echo "WARNING: PUBLIC_KEY empty -- pass startSsh:true and register a key, or this pod is unreachable"
fi
ssh-keygen -A
/usr/sbin/sshd
echo "sshd started"

# The two assumptions a slim image rests on, answered before any money goes
# into proving: that the host driver is injected into a base image, and that
# the binary needs nothing dynamically beyond the cudart this image carries.
nvidia-smi -L || echo "ASSUMPTION FAILED: nvidia-smi was not injected into this image"
BIN=/workspace/mizu-cache/bin/pasta-precompile-proof-6-10
if [ -x "$BIN" ]; then
  ldd "$BIN" | tee /root/slim-image-ldd.txt || true
  if grep -q 'not found' /root/slim-image-ldd.txt; then
    echo "ASSUMPTION FAILED: this image is missing a shared library (see above)"
  else
    echo "every shared library resolved on this base image"
    "$BIN" --mode cuda-probe --output /root/slim-image-cuda-probe.json \
      && cat /root/slim-image-cuda-probe.json \
      || echo "ASSUMPTION FAILED: no CUDA context on a base image"
  fi
else
  echo "note: $BIN absent -- mount the build-cache volume at /workspace to settle this here"
fi

echo "=== slim pod ready $(date -u +%FT%TZ) ==="
sleep infinity
