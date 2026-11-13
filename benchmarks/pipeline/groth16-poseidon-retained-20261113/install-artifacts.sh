#!/bin/bash
# One-time: put Succinct's released Groth16 artifacts on the network volume.
#
# SP1 cannot do this itself when ~/.sp1 is on MooseFS. Its installer runs
# `tar -Pxzf` (prover/src/build.rs:215) with no --no-same-owner, and the tarball
# was packed on a Mac (owner fakedev9999:wheel, uid 501 gid 0, plus ._ AppleDouble
# entries). As root, GNU tar restores ownership; the overlay disk permits it and
# MooseFS refuses -- "Cannot change ownership to uid 501, gid 0" -- so the extract
# fails and SP1 reports "artifact not found".
#
# circuit_artifacts_installed (build.rs:639) only checks for the .complete marker,
# so extracting correctly ourselves and touching it makes SP1 skip the download.
set -uo pipefail
V=/workspace
VER=v6.1.0
DIR=$V/sp1/circuits/groth16/$VER
URL=https://sp1-circuits.s3-us-east-2.amazonaws.com/$VER-groth16.tar.gz
if [ -f "$DIR/.complete" ]; then echo "already installed: $(du -sh $DIR | cut -f1)"; exit 0; fi
mkdir -p "$DIR"
echo "downloading 5.8 GiB to container disk..."
t=$(date +%s)
curl -sSfL "$URL" -o /root/g16.tar.gz || { echo "download failed"; exit 1; }
echo "  download $(( $(date +%s) - t ))s, $(du -sh /root/g16.tar.gz | cut -f1)"
echo "extracting to the volume with --no-same-owner..."
t=$(date +%s)
tar --no-same-owner --no-same-permissions -Pxzf /root/g16.tar.gz -C "$DIR" 2>&1 | grep -v "Ignoring unknown extended header" | head -5
echo "  extract $(( $(date +%s) - t ))s"
rm -f /root/g16.tar.gz /root/._* 2>/dev/null
# AppleDouble junk from the Mac packing; harmless but remove it.
find "$DIR" -name '._*' -delete 2>/dev/null
ls -la "$DIR" | grep -vE '^total|^d' | awk '{print "  ", $5, $9}'
touch "$DIR/.complete"
echo "installed: $(du -sh $DIR | cut -f1), .complete written"
