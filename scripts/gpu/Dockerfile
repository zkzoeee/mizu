# A prover image that starts proving instead of preparing to prove.
#
# Measured on 2026-11-11, a cache-hit run spent 7.0 minutes of billed pod life
# on 3 minutes of work: ~112 s waiting for the 20 GB runpod/pytorch image to
# pull and for ssh.direct to appear, ~70 s of launcher latency, 22 s of apt and
# toolchain setup that a prebuilt binary never uses, and 139 s of actual proof.
# This image removes the first three.
#
# The base is the CUDA *runtime*, about 2.6 GB against 20, and carries nothing
# needed to compile: the binary is built elsewhere (CI, or a CPU pod) and
# copied in. It must be built for the architectures the pods will use —
# `CUDA_ARCHS=86,89,120` covers A40/A6000/3090, 4090/L40S and 5090/PRO — and
# against this same CUDA line, or libcudart will not match.
#
# Build it on x86_64 (a Mac is arm64, and emulating this is pointless):
#   scripts/gpu/build-image.sh ghcr.io/<owner>/mizu-prover:<tag> <binary> <input-dir>
FROM nvidia/cuda:12.8.1-runtime-ubuntu24.04

# python3 for the JSON and telemetry summaries the driver shells out to,
# `time` for /usr/bin/time -v, procps for the RSS sampler. Nothing else: no
# compiler, no cmake, no rustup.
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-minimal ca-certificates time procps \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/mizu
COPY bin/pasta-precompile-proof-6-10 bin/
COPY gpu/ gpu/
COPY entrypoint.sh cache-key.py ./
RUN chmod +x bin/pasta-precompile-proof-6-10 entrypoint.sh

# The CUDA-context gate cannot use torch here, because torch is exactly the
# 18 GB this image drops. The harness's own CUDA probe replaces it; see
# entrypoint.sh.
ENV MIZU_HOME=/opt/mizu
ENTRYPOINT ["/opt/mizu/entrypoint.sh"]
