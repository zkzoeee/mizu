#!/usr/bin/env bash
# Pull back the evidence, and optionally the proofs themselves.
#
# Ported from wazabi r123. Two transfers, deliberately separate: the receipts
# are small and come first, so a failure during the much larger proof transfer
# still leaves the evidence retrieved. Hashes are checked on both sides.
#
# One archive, not a per-file loop: r123's first version ran scp and ssh
# inside a `while read` loop fed by a pipe, ssh consumed the loop's stdin, and
# it transferred the FIRST proof and silently skipped the other 47. Proofs are
# high-entropy, so the archive is not compressed; gzip would cost CPU for
# nothing. Each proof's own hash is checked against what the pod reported
# before the transfer.
#
# Usage: collect-results.sh <host> <port> <dest-dir>
# Environment:
#   POD_RESULTS     the pod's results directory; must match the launch side's
#                   POD_RESULTS (default /root/results, i.e. container disk, so
#                   a mounted network volume carries only the build cache)
#   WITH_PROOFS=0   skip the proof bytes (r120 left 132 MB per arm on the pod
#                   deliberately, because local re-verification was already
#                   closed; set it to 0 when the receipts are the deliverable)
#   PROOF_DEST      where proof bytes land (default <dest>/proofs); keep them
#                   outside Git
set -euo pipefail

HOST="${1:?usage: collect-results.sh <host> <port> <dest-dir>}"
PORT="${2:?missing port}"
DEST="${3:?missing dest dir}"
WITH_PROOFS="${WITH_PROOFS:-1}"
POD_RESULTS="${POD_RESULTS:-/root/results}"

# Absolute, because the proof check runs inside `cd "$PROOFDEST"` and a
# relative DEST resolves against that directory instead: on 2026-11-10 the
# per-proof check reported corruption on files whose hashes were in fact
# identical to the pod's.
case "$DEST" in /*) ;; *) DEST="$PWD/$DEST" ;; esac

# Archives are staged on container disk, never on a mounted volume.
RBASE="$(basename "$POD_RESULTS")"
RDIR="$(dirname "$POD_RESULTS")"
STAGE=/root

SSHOPTS=(-p "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)
SCPOPTS=(-P "$PORT" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20)

mkdir -p "$DEST"

echo "=== what is on the pod ($POD_RESULTS) ==="
ssh -n "${SSHOPTS[@]}" "root@$HOST" "ls -la '$POD_RESULTS'; du -sh '$POD_RESULTS'"

echo "=== proof inventory, hashed on the pod before transfer ==="
ssh -n "${SSHOPTS[@]}" "root@$HOST" \
  "cd '$POD_RESULTS' && find . -name '*.proof.bin' -exec sha256sum {} \; -exec ls -l {} \;" \
  | tee "$DEST/proof-inventory-on-pod.txt"

echo "=== 1/2 pulling receipts, reports, telemetry and logs (no proofs) ==="
ssh -n "${SSHOPTS[@]}" "root@$HOST" \
  "cd '$RDIR' && rm -f $STAGE/mizu-evidence.tar.gz && tar czf $STAGE/mizu-evidence.tar.gz --exclude='*.proof.bin' '$RBASE' && sha256sum $STAGE/mizu-evidence.tar.gz && ls -l $STAGE/mizu-evidence.tar.gz"
scp "${SCPOPTS[@]}" "root@$HOST:$STAGE/mizu-evidence.tar.gz" "$DEST/"
EVID_WANT=$(ssh -n "${SSHOPTS[@]}" "root@$HOST" "sha256sum $STAGE/mizu-evidence.tar.gz" | cut -d' ' -f1)
EVID_GOT=$(shasum -a 256 "$DEST/mizu-evidence.tar.gz" | cut -d' ' -f1)
if [ "$EVID_WANT" != "$EVID_GOT" ]; then
  echo "evidence transfer corrupted: pod $EVID_WANT, local $EVID_GOT" >&2
  exit 1
fi
echo "evidence intact: $EVID_GOT"

if [ "$WITH_PROOFS" = 1 ]; then
  echo "=== 2/2 pulling the proofs themselves ==="
  PROOFDEST="${PROOF_DEST:-$DEST/proofs}"
  case "$PROOFDEST" in /*) ;; *) PROOFDEST="$PWD/$PROOFDEST" ;; esac
  mkdir -p "$PROOFDEST"
  ssh -n "${SSHOPTS[@]}" "root@$HOST" \
    "cd '$POD_RESULTS' && rm -f $STAGE/mizu-proofs.tar && find . -name '*.proof.bin' -print0 | tar cf $STAGE/mizu-proofs.tar --null -T - && sha256sum $STAGE/mizu-proofs.tar && ls -l $STAGE/mizu-proofs.tar"
  scp "${SCPOPTS[@]}" "root@$HOST:$STAGE/mizu-proofs.tar" "$PROOFDEST/"
  PW=$(ssh -n "${SSHOPTS[@]}" "root@$HOST" "sha256sum $STAGE/mizu-proofs.tar" | cut -d' ' -f1)
  PG=$(shasum -a 256 "$PROOFDEST/mizu-proofs.tar" | cut -d' ' -f1)
  if [ "$PW" != "$PG" ]; then
    echo "proof archive transfer corrupted: pod $PW, local $PG" >&2
    exit 1
  fi
  echo "proof archive intact: $PG"
  tar xf "$PROOFDEST/mizu-proofs.tar" -C "$PROOFDEST"
  rm -f "$PROOFDEST/mizu-proofs.tar"

  ssh -n "${SSHOPTS[@]}" "root@$HOST" \
    "cd '$POD_RESULTS' && find . -name '*.proof.bin' -exec sha256sum {} \;" \
    > "$DEST/proof-hashes-on-pod.txt"
  ( cd "$PROOFDEST" && shasum -a 256 -c "$DEST/proof-hashes-on-pod.txt" ) \
    || { echo "at least one proof does not match its pod hash" >&2; exit 1; }
  echo "all proof hashes match the pod"
else
  echo "=== 2/2 skipped: WITH_PROOFS=0, proof bytes stay on the pod ==="
fi

cd "$DEST"
# --strip-components=1 drops the archive's own leading results directory, which
# would otherwise land everything at <dest>/<basename>/...
tar xzf mizu-evidence.tar.gz --strip-components=1
rm -f mizu-evidence.tar.gz
echo "=== collected into $DEST ==="
find . -name '*.json' | sort
du -sh .
echo
echo "Next, off the machine that made them:"
echo "  cargo run --release --manifest-path tools/pasta-proof/Cargo.toml \\"
echo "    --bin pasta-precompile-proof-6-10 -- --mode manifest-core-verify \\"
echo "    --manifest benchmarks/gpu/next-core-proof-manifest.json \\"
echo "    --elf <the pinned ELF> --input $DEST \\"
echo "    --reference <input dir>/public-values.bin --output <fresh dir>"
