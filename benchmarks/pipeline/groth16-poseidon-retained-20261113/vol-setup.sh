#!/bin/bash
# Wire the expensive, reusable state onto the network volume at /workspace.
# Idempotent: safe on both the populating run and every run after.
set -euo pipefail
V=/workspace
mountpoint -q $V || { echo "FATAL: $V is not a mount -- was the volume attached?"; exit 1; }
mkdir -p $V/{sp1,cargo-registry,target,elf,vkmap,out,src}

# The 7.9 GB Groth16 artifacts live on the volume, but SP1 cannot install them
# there itself: its installer runs `tar -Pxzf` with no --no-same-owner
# (prover/src/build.rs:215) and the tarball carries macOS ownership, which
# MooseFS refuses to set. install-artifacts.sh does it correctly, once.
# Measured 2026-11-13: MooseFS gives 636 MB/s write and 835 MB/s read
# sequentially, and gnark's "Reading R1CS" is deserialisation rather than I/O
# (2.4 GB in 22.5 s from a 9.3 GB/s local disk), so serving them from here costs
# nothing per proof and saves the 4m11s download on every fresh pod.
mkdir -p $V/sp1
[ -L /root/.sp1 ] || { rm -rf /root/.sp1; ln -sfn $V/sp1 /root/.sp1; }

# The crate registry, ~755 MB. Symlink the registry alone rather than moving
# CARGO_HOME, which also holds the cargo shims rustup just installed.
[ -L /root/.cargo/registry ] || { rm -rf /root/.cargo/registry; ln -sfn $V/cargo-registry /root/.cargo/registry; }

# Go, ~250 MB, otherwise re-downloaded every boot.
if [ -d $V/go ] && [ ! -e /usr/local/go ]; then ln -sfn $V/go /usr/local/go; fi

cat <<EOF
volume wired:
  /root/.sp1            -> $V/sp1            $(du -sh $V/sp1 2>/dev/null | cut -f1)
  groth16 artifacts present: $([ -f $V/sp1/circuits/groth16/v6.1.0/.complete ] && echo yes || echo NO -- run install-artifacts.sh)
  /root/.cargo/registry -> $V/cargo-registry $(du -sh $V/cargo-registry 2>/dev/null | cut -f1)
  CARGO_TARGET_DIR       = $V/target          $(du -sh $V/target 2>/dev/null | cut -f1)
  note: df shows the MooseFS cluster, not this volume's 30 GB quota
EOF
