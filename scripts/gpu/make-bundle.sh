#!/usr/bin/env bash
# Build the source bundle shipped to a rented GPU pod, and pin its sha256.
#
# Ported from wazabi's r123 (`spikes/artifacts/r123-sp1-6.10-5090-validation/
# make-bundle.sh`), which proved this same harness binary on an RTX 5090.
# What changed: this repository's paths, the manifest as the single authority
# for every pinned hash, and the whole `vendor/sp1` tree rather than a
# four-crate fork, because `tools/pasta-proof` path-patches four crates that
# inherit from that workspace's root manifest. Without `target/` the tree is
# about 95 MB, which compresses to a bundle scp moves in seconds.
#
# Nothing here builds, executes or proves anything. It refuses to pack an ELF
# or an input that does not match the manifest, so a stale artifact costs
# nothing instead of costing a rental.
#
# Usage: make-bundle.sh OUT.tar.gz --input DIR [--manifest M] [--elf E]
#
# DIR must hold proof-input.bin, receipt-input.bin and public-values.bin.
# The first two are the exported statement; the third is the one byte 0x01
# the boolean core guest commits:
#
#   cargo run --release -p cycle-runner --bin export-fixture -- \
#     --fixture-dir fixtures/mainnet-blockchain-snark --output-dir DIR
#   printf '\x01' > DIR/public-values.bin
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT=""
INPUT=""
MANIFEST="$REPO/benchmarks/gpu/next-core-proof-manifest.json"
ELF=""

while [ $# -gt 0 ]; do
  case "$1" in
    --input) INPUT="$2"; shift 2 ;;
    --manifest) MANIFEST="$2"; shift 2 ;;
    --elf) ELF="$2"; shift 2 ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) OUT="$1"; shift ;;
  esac
done
[ -n "$OUT" ] || { echo "usage: make-bundle.sh OUT.tar.gz --input DIR" >&2; exit 2; }
[ -n "$INPUT" ] || { echo "--input DIR is required" >&2; exit 2; }
[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST" >&2; exit 2; }
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
case "$INPUT" in /*) ;; *) INPUT="$PWD/$INPUT" ;; esac

field() { python3 -I -c "import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$MANIFEST" "$1"; }
[ -n "$ELF" ] || ELF="$(field elf)"

check() {
  local want="$1" path="$2" what="$3" got
  [ -f "$path" ] || { echo "$what missing: $path" >&2; exit 1; }
  got=$(shasum -a 256 "$path" | cut -d' ' -f1)
  if [ "$want" != "$got" ]; then
    echo "$what sha mismatch: manifest pins $want, on disk $got" >&2
    exit 1
  fi
  echo "$what matches its pin: $got"
}

# Every hash comes from the manifest; none is retyped here.
check "$(field elf_sha256)"             "$ELF"                      "guest ELF"
check "$(field proof_input_sha256)"     "$INPUT/proof-input.bin"    "proof-input.bin"
check "$(field receipt_input_sha256)"   "$INPUT/receipt-input.bin"  "receipt-input.bin"
check "$(field public_values_sha256)"   "$INPUT/public-values.bin"  "public-values.bin"

PV_LEN=$(wc -c < "$INPUT/public-values.bin" | tr -d ' ')
PV_WANT=$(field public_values_len)
[ "$PV_LEN" = "$PV_WANT" ] || {
  echo "public-values.bin is $PV_LEN bytes, manifest pins $PV_WANT" >&2; exit 1; }
echo "public-values.bin is $PV_LEN byte(s), as pinned"

CAP=$(python3 -I -c "import json;print(json.load(open('$MANIFEST'))['authorization']['total_usd_cap'])")
echo "manifest spending cap: \$$CAP"

# The pod reads the artifacts at fixed paths, so the ELF, the inputs and the
# manifest are staged under gpu/ rather than carried at their authoring paths.
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/gpu/elf" "$STAGE/gpu/input"
cp "$ELF" "$STAGE/gpu/elf/guest-program"
cp "$INPUT/proof-input.bin" "$INPUT/receipt-input.bin" "$INPUT/public-values.bin" "$STAGE/gpu/input/"
cp "$MANIFEST" "$STAGE/gpu/run-manifest.json"

cd "$REPO"
# COPYFILE_DISABLE and --no-mac-metadata stop macOS tar from embedding
# AppleDouble "._name" members and xattr headers. r123 lost a launch to them:
# the pod's GNU tar warned on every member, then attempted a chown that the
# MooseFS /workspace refuses even as root, exited non-zero, and `set -e`
# killed the run before anything was built. The extraction side also passes
# --no-same-owner; both halves are kept, because either alone leaves the trap.
COPYFILE_DISABLE=1 tar czf "$OUT" \
  --no-mac-metadata \
  --no-xattrs \
  --exclude=target \
  --exclude=.git \
  --exclude=book \
  --exclude=.DS_Store \
  --exclude='._*' \
  --exclude='*.proof.bin' \
  tools/pasta-proof \
  vendor/sp1 \
  -C "$STAGE" gpu

echo "bundle: $OUT"
echo "bytes:  $(wc -c < "$OUT" | tr -d ' ')"
echo "sha256: $(shasum -a 256 "$OUT" | cut -d' ' -f1)"
