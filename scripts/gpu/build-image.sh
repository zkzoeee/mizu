#!/usr/bin/env bash
# Assemble the prover image from an already-built binary.
#
# This is the FASTER of two slim paths, not the only one. A stock
# nvidia/cuda:12.8.1-base-ubuntu24.04 (101 MB compressed) plus the start command in
# slim-pod-start-command.sh installs sshd at boot and needs no registry at all; a
# baked image wins only by carrying the binary, so there is no volume and no cache
# check. Build one when runs are frequent enough to earn the push.
#
# This script does not compile anything: the CUDA binary must already exist,
# built on x86_64 Linux for the architectures the pods will use. Three ways to
# get one, cheapest first — all established on 2026-11-11:
#
#   * GitHub Actions in a `nvidia/cuda:12.8.1-devel-ubuntu24.04` container job,
#     free and glibc-matched. Needs this repository pushed to a remote; today
#     it has only an `upstream` remote and the work is local, so Actions cannot
#     run yet.
#   * a RunPod CPU pod — `cpu3c` at $0.03/vCPU/hr, so 8 vCPU for $0.24/hr and
#     about 3 cents a build. Nothing links the CUDA driver API (only `cudart`,
#     `cudadevrt`, `nvToolsExt`, `stdc++`, `gomp`, `dl`), so no GPU is needed.
#   * the volume: a build already lives at `/workspace/mizu-cache/bin/`, and
#     scp'ing it off a pod costs nothing extra.
#
# The image build itself needs Docker and almost no memory, because it only
# copies files — the 1 GB zol droplet can do it even though it cannot compile.
#
# Usage: build-image.sh TAG BINARY INPUT_DIR [ELF] [MANIFEST]
#   TAG        e.g. ghcr.io/<owner>/mizu-prover:$(git rev-parse --short HEAD)
#   BINARY     the CUDA harness built for the target architectures
#   INPUT_DIR  holds proof-input.bin, receipt-input.bin, public-values.bin
#
# Pushing needs a credential this script never sees: log in first with
# `docker login ghcr.io`, and give RunPod a read-only one via `create-registry`
# (a PAT scoped to `read:packages`, nothing more, since it lands on a rented
# host).
set -euo pipefail

TAG="${1:?usage: build-image.sh TAG BINARY INPUT_DIR [ELF] [MANIFEST]}"
BINARY="${2:?missing binary}"
INPUT_DIR="${3:?missing input dir}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MANIFEST="${5:-$REPO/benchmarks/gpu/next-core-proof-manifest.json}"
ELF="${4:-}"
[ -n "$ELF" ] || ELF="$(python3 -I -c "import json;print(json.load(open('$MANIFEST'))['elf'])")"

field() { python3 -I -c "import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$MANIFEST" "$1"; }
check() {
  local want="$1" path="$2" what="$3" got
  got=$(shasum -a 256 "$path" 2>/dev/null | cut -d' ' -f1 || sha256sum "$path" | cut -d' ' -f1)
  [ "$want" = "$got" ] || { echo "$what sha mismatch: manifest pins $want, on disk $got" >&2; exit 1; }
  echo "$what matches its pin: $got"
}

# The same gate make-bundle.sh applies: nothing unpinned goes in, so a stale
# artefact costs nothing instead of costing a rental.
check "$(field elf_sha256)"           "$ELF"                       "guest ELF"
check "$(field proof_input_sha256)"   "$INPUT_DIR/proof-input.bin" "proof-input.bin"
check "$(field receipt_input_sha256)" "$INPUT_DIR/receipt-input.bin" "receipt-input.bin"
check "$(field public_values_sha256)" "$INPUT_DIR/public-values.bin" "public-values.bin"
test -x "$BINARY" || { echo "$BINARY is not executable" >&2; exit 1; }
file "$BINARY" | grep -qi "x86-64\|x86_64" \
  || { echo "$BINARY is not an x86_64 binary; build it on Linux x86_64" >&2; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin" "$STAGE/gpu/elf" "$STAGE/gpu/input"
cp "$BINARY" "$STAGE/bin/pasta-precompile-proof-6-10"
cp "$ELF" "$STAGE/gpu/elf/guest-program"
cp "$INPUT_DIR/proof-input.bin" "$INPUT_DIR/receipt-input.bin" \
   "$INPUT_DIR/public-values.bin" "$STAGE/gpu/input/"
cp "$MANIFEST" "$STAGE/gpu/run-manifest.json"
cp "$HERE/entrypoint.sh" "$HERE/cache-key.py" "$HERE/Dockerfile" "$STAGE/"

echo "=== building $TAG ==="
docker build --platform linux/amd64 -t "$TAG" "$STAGE"
docker image inspect "$TAG" --format 'image size: {{.Size}} bytes'
echo
echo "Next:"
echo "  docker push $TAG"
echo "  then create-registry with a read:packages PAT, and create-pod with:"
echo "    image: $TAG, registry: <id>, entrypoint from the image,"
echo "    env: ARMS, CUDA_ARCHS, DEADLINE_MINUTES"
echo "  no bundle, no volume and no SSH wait are needed with this image."
