# Proving the core guest on a rented GPU

Status: 2026-11-10. Drivers in [`scripts/gpu`](../scripts/gpu), run pinned by
[`benchmarks/gpu/next-core-proof-manifest.json`](../benchmarks/gpu/next-core-proof-manifest.json).
No rental has been made from this repository; the manifest's
`authorization.total_usd_cap` is the gate and it grants nothing until a cap is
set in it.

The drivers are a port of the ones in the sibling `wazabi` tree, which proved
**this same harness binary** on rented GPUs four times (r114, r120, r123,
r124). Those runs are the source of every number below. Port, not invention:
their gates, orderings and environment fixes are kept, and what changed is this
repository's paths, architecture detection from the card instead of a hardcoded
value, and the build cache described under [Reusing a build](#reusing-a-build).

## What the measurements say about cost

Taken from the wazabi runs, because they ran this workload rather than a
comparable one.

- **Buy clock, not cores.** 15 vCPU proved the retained arm in 301.8 s; 20 vCPU
  took 308.4 s. The pipeline keeps only **2.5–2.6 cores busy** on every host
  measured, because the RISC-V executor is sequential. But the same SP1 6.1
  code took **134 s** on r120's host and **308 s** on r124's, with GPU-busy time
  identical and host CPU-seconds 2.1x higher — per-core speed, not core count.
  That is a 2.3x swing in the bill for identical work, and choosing for clock
  costs nothing. `pod-run.sh` records `/proc/cpuinfo`'s model name every run,
  because r124 only had to exist since r120 and r123 had not.
- **Do not buy a bigger GPU.** Integrating r120's utilization series splits the
  wall clock into GPU work (44–63 s per arm) and GPU *waiting* (89–90 s, fixed
  whatever the routing). r123 reproduced it on another version and host: the
  card was idle **82.9%** of the run, median sample 0%. The ceiling for a
  faster card is 33–41% of wall, realistically near zero.
- **24 GB is enough, with thin headroom.** Peak VRAM on the retained arm is
  **19,657 MiB** under 6.10 and 21,928 MiB under 6.1. Where a 48 GB card costs
  the same or less than a 24 GB one — RTX A6000 and A40 at $0.59/hr in EU-SE-1
  against an RTX 4090 at $0.89/hr, from the catalogue read of 2026-11-10 — take
  the larger one and stop thinking about it.
- **Expect well under an hour.** r123 did two cold builds, a 42-case CPU proof
  matrix and 48 proofs in **59.8 minutes for ~$1.19**. Two arms of this
  manifest, with one CUDA build, is roughly 25–35 minutes.
- **Enforce the cap with a deadline, not a billing read.** `list-pod-billing`
  returned `recordCount: 0` for both r120 and r123, so their costs are
  timestamp estimates. The Groth16 run did get real billing, and it came in
  *below* the list-price estimate. Treat billing as a nice-to-have and the
  watchdog as the ceiling.
- **Community cloud is about half price and carries a real risk.** Attempt 1 of
  the Groth16 run lost **$0.199** on a community host where `nvidia-smi`
  reported the GPU correctly but no process could create a CUDA context; the
  device nodes were owned by `nobody:nogroup`. `pod-run.sh` gates that in
  seconds with `torch.cuda.is_available()` before anything is built. Network
  volumes attach only to Secure Cloud pods, so a cached build and community
  pricing are mutually exclusive.

## The volume

**There is one: `zvx1pq06vk`, "mizu-build-cache", 10 GB, STANDARD, EU-SE-1**,
created 2026-11-10. Mount it at `/workspace`, which is where `pod-run.sh`
already looks for `/workspace/mizu-cache`, and leave `CACHE` unset so it finds
it:

```json
"mounts": {"network": [{"volumeId": "zvx1pq06vk", "path": "/workspace"}]}
```

The pod must be in **EU-SE-1** and on **Secure Cloud** — a volume is
data-centre-local and community pods cannot mount one. Both A40 and RTX A6000
live in EU-SE-1 at $0.59/hr, so that is no sacrifice.

The first run there builds and populates `bin/`, `key.json`, `cargo-home` and
`rustup-home`, about 3.5 GB of the 10. Every later run with unchanged sources
reuses the binary after checking its recorded sha256 and making it answer
`--mode registration`, which takes the run from ~11 minutes to **~5**. Leave
`CACHE_TARGET` off: a full `target/` tree is 10–15 GB and would not fit.

Tiers are per data centre, and the two we have used differ: EU-SE-1 offers
`STANDARD`, **CA-MTL-1 offers only `HIGH_PERFORMANCE`** (CA-MTL-4 has both).
A volume created in one cannot be mounted from the other.

## Was a volume worth it?

The arithmetic below said mostly no, and four runs in one day settled it the
other way — the build is 7 of every 11 minutes, and it is the same build every
time. The original reasoning, kept because the trade-off is still real:

A cold CUDA build of this harness is **3:34** with `CUDA_ARCHS` pinned to one
architecture and 8 jobs (r123's measurement; its CPU-only build took 5:47, and
this port skips that one). A 60 GB STANDARD volume at RunPod's published
$0.07/GB/month is about $4.20/month, roughly 3.5 hours of pod time at $0.59/hr,
billed from creation until deletion whether or not anything is attached. So the
volume costs more than the builds it saves unless the pod is created often.

It is also not free in speed: r123 found `/workspace` is a **MooseFS network
mount**, so a `target/` kept there is built over the network. `pod-run.sh`
therefore builds in local container disk and copies only the finished binary to
the volume. `CACHE_TARGET=1` additionally parks the whole `target/` tree on the
volume, which is off by default because it is several GB of billed storage.

Use a volume when the pod will be re-created repeatedly against unchanged
sources — a measurement sweep, a host comparison like r124, a retry after a
stock-out. Skip it for a one-off proof.

Building on a cheap CPU pod instead of the GPU pod is a separate idea and a
small one: `cpu3c` is $0.03/vCPU/hr against the A40's $0.59/hr, so a ten-minute
build saves about six cents, once per source change. It also cannot be done in
EU-SE-1 or CA-MTL-1, which offer no CPU pods, so it would mean giving up the
48 GB cards. Not worth it; let the first GPU run populate the cache.

## Reusing a build

`pod-run.sh` resolves a cache before it builds anything:

1. `cache-key.py` hashes the build's identity — OS release, nvcc release, the
   `rustc --version` string, `CUDA_ARCHS`, and a fingerprint over every source
   that feeds the build (`tools/pasta-proof/{Cargo.toml,Cargo.lock,src}`,
   `vendor/sp1/Cargo.toml`, `vendor/sp1/crates`; 993 files at the time of
   writing). `vendor/sp1/sp1-gpu` is deliberately excluded, because the CUDA
   crates come from crates.io at versions `Cargo.lock` already pins.
2. On a hit, the cached binary is copied in, its recorded sha256 is checked,
   and it has to answer `--mode registration` on this host before it is
   trusted. Existing is not the same as working.
3. On a miss, `cache-key.py --compare` prints which component moved, so the
   log says "`cuda_archs: 120 -> 86`" rather than "miss".
4. After a build, the binary, its hash and `key.json` are written to the
   volume, along with `cargo-home` and `rustup-home`, which save the downloads
   even when the key moves.

Because the architecture is in the key, a binary built on a 5090 (`sm_120`) is
never reused on an A6000 (`sm_86`), which would otherwise fail inside the CUDA
prover rather than at the cache.

Set `CACHE=` empty to ignore any volume; the run then builds from scratch and
writes nothing back.

## The run

**1. Stage the inputs locally** (free, no rental). The two statement blobs are
already exported and hash-checked in the sibling tree at
`wazabi/handoff/proof-input/`, and their hashes equal this manifest's
`proof_input_sha256` and `receipt_input_sha256` exactly. The expected public
values differ, because this repository's core guest commits one byte where
wazabi's receipt guest committed 577:

```bash
mkdir -p /tmp/mizu-gpu-input && cp /Users/user/000/_Z/wazabi/handoff/proof-input/proof-input.bin /Users/user/000/_Z/wazabi/handoff/proof-input/receipt-input.bin /tmp/mizu-gpu-input/ && printf '\x01' > /tmp/mizu-gpu-input/public-values.bin
```

Or export them from the fixture, which produces the same bytes:

```bash
cargo run --release -p cycle-runner --bin export-fixture -- --fixture-dir fixtures/mainnet-blockchain-snark --output-dir /tmp/mizu-gpu-input
```

**2. Build the bundle.** It refuses to pack an artifact that does not match the
manifest, so a stale ELF costs nothing instead of costing a rental:

```bash
scripts/gpu/make-bundle.sh /tmp/mizu-bundle.tar.gz --input /tmp/mizu-gpu-input
```

**3. Set the cap** in `benchmarks/gpu/next-core-proof-manifest.json`
(`authorization.total_usd_cap`). The drivers print it; the watchdog deadline is
what enforces it.

**4. Create the pod** against a live stock read, in Secure Cloud if a volume is
to be mounted, with `22/tcp` and `startSsh` so scp works, and a CUDA floor of
`12.8` rather than a pin. Wait for `ssh.direct` to appear — it can take a
couple of minutes after the pod reports running, and reading it too early is
how r114 concluded direct SSH was unavailable and fell back to the PTY push
that cost it the pod.

**5. Push and launch.** `RATE_PER_HOUR` must be the rate the *created pod*
reported, not the catalogue's: r114 printed a stale $1.00/hr while billing
$1.29/hr.

```bash
DEADLINE_MINUTES=90 RATE_PER_HOUR=0.59 CAP_USD=2.00 scripts/gpu/push-and-launch.sh <host> <port> /tmp/mizu-bundle.tar.gz
```

**6. Follow it**, then collect. `WITH_PROOFS=0` leaves the proof bytes on the
pod, which is what r120 did deliberately once local re-verification had already
been closed:

```bash
scripts/gpu/collect-results.sh <host> <port> benchmarks/gpu/runs/$(date -u +%Y%m%dT%H%MZ)
```

**7. Verify off the machine that made them**, with `--mode
manifest-core-verify` on the collected directory. The verifier runs on the CPU
and generates nothing, so this is free and is the only acceptance that counts.

**8. Terminate the pod.** Stopping ends compute billing but keeps the disk
charge; a network volume bills from creation until it is deleted, whatever the
pod does.

## What the first run of these drivers measured (2026-11-10)

An A40 at $0.59/hr in EU-SE-1, both arms proved and verified, **~$0.40 of a
$2.00 cap**. The full record is
[mizu-gpu-core-proof-a40](optimizations/06-sp1-runtime-and-proving.md#mizu-gpu-core-proof-a40-the-first-real-gpu-proof-from-this-repository);
the phase timings are what matter for planning the next one:

| Phase | Wall | Note |
| --- | ---: | --- |
| Pod create to `ssh.direct` | ~90 s | the field is null until runtime is non-null |
| Bundle upload (72 MB) + hash check | ~30 s | direct SSH, not the PTY |
| apt, cmake ≥ 3.30, rustup | ~30 s | all from the image plus pip |
| Cold CUDA build, `CUDA_ARCHS=86`, 8 jobs | **~7 min** | most of it codegen of the one harness binary |
| Pre-flight (every pinned hash) | seconds | no proving |
| Arm `retain-pasta-and-uint` | **168.63 s** | 82 shards, 132 MB |
| Arm `default` | **223.14 s** | 148 shards, 228 MB |
| Both proofs verified from disk, on the pod | ~16 s | fresh process, key from the ELF |
| Downloading 343 MB of proof bytes | ~75 s | skip with `WITH_PROOFS=0` |

Pod life was 40.5 minutes, but roughly 17 of those were three failed build
attempts on real defects, now fixed. **A clean repeat is about 18–19 minutes,
or $0.19.** Within a single pod's life the retries were cheap, because
`CARGO_TARGET_DIR` is on container disk: after the first attempt only the
harness crate recompiled.

### Making the whole thing quicker, in order of effect

1. **Bake the binary into an image** (−7 min). The build is the single largest
   phase and it is the same build every time. Push an image carrying
   `pasta-precompile-proof-6-10` and the pod goes straight to pre-flight:
   about 8 minutes end to end. Needs a registry credential and an image
   rebuild whenever the harness or the fork changes.
2. **Mount a volume and reuse the cached build** (−7 min, no registry). This is
   what [the cache](#reusing-a-build) is for, and it is the case where a volume
   finally pays: `key.json` plus `bin/` plus the cargo and rustup homes fit in
   10 GB. The binary still has to answer `--mode registration` on the new host
   before it is trusted.
3. **Prove only the arm you need** (−3.7 min). `ARMS="retain-pasta-and-uint"`.
   The `default` arm exists for the routing comparison, not for a proof you
   want to keep.
4. **Leave the proof bytes on the pod** (−1.3 min). `WITH_PROOFS=0`. The
   on-pod reload verification has already run; download only when
   re-verification off the machine is the gate you need.
5. **Gate the host's clock, not the card** (the 168 s itself). The same arm
   took 301.84 s and 308.35 s on RTX 5090 pods whose cores were about half as
   fast. Measuring single-core speed in the first 20 seconds and discarding a
   slow host costs about a cent per discard against a 1.8x swing in run time.
   Not implemented yet; it is the obvious next addition to `pod-run.sh`.
6. **Parallelize record generation** (the only lever left on the proof itself).
   159.80 s of the 168.63 s is the executor, and
   `executor_seconds == gpu_task_total_seconds` to four decimals for the fifth
   run in a row. Checkpoint-and-replay is r120's ranked item 2 and a project,
   not a flag. Note that the *other* pipelining idea, proving shards
   concurrently, is implemented and **does not work**: see the record.

## Three things any GPU run must get right

All three were discovered the hard way in wazabi and are pinned by the manifest
or the driver here, but they are worth stating because getting one wrong
produces a convincing-looking failure:

1. `SP1CoreOpts::global_dependencies_opt` **must be true**, or every shard
   fails `shard_cryptographic_verification` while traces, public values and the
   verification key all look correct. Both arms of this manifest set it.
2. Record generation must stream. `generate_records` drains the whole execution
   into memory first; the GPU path uses
   `generate_records_streaming_with_summary`.
3. The PCS parameters must be the production ones. `CORE_MAX_LOG_ROW_COUNT` is
   22 and SP1 asserts `HEIGHT_THRESHOLD <= 1 << 22`, so the fixture suite's 17
   is not valid for a real program. `run_manifest.rs` refuses any other values.

## Environment traps, inherited

Each one cost billed time in wazabi and is handled in `pod-run.sh`:

- `nproc` reports the **host's** cores, not the pod's allocation (120 against 15
  allocated; 152 against 25 on another pod), so `CARGO_BUILD_JOBS` is pinned to
  8. The repository's `CARGO_BUILD_JOBS=1` rule is for the shared 16 GB Mac.
- nvcc is installed but not on `PATH`; `CUDACXX` is set explicitly and searched
  for if the usual path is missing.
- Ubuntu 24.04 ships cmake 3.28 and the CUDA build wants ≥ 3.30, which is
  installed from pip.
- macOS tar embeds AppleDouble members and xattrs that make GNU tar attempt a
  chown the MooseFS mount refuses; the bundle is built with
  `COPYFILE_DISABLE=1 --no-mac-metadata --no-xattrs` and extracted with
  `--no-same-owner --no-xattrs`. Both halves are kept.
- A MIG slice answers `[Insufficient Permissions]` for memory and utilization,
  which cost r114 its VRAM measurement; the driver fails on it rather than
  proving without telemetry.
- `RUST_LOG` at `info` emitted eight lines in a 2h26m proof, with no way to
  tell progress from a hang. The driver defaults to
  `sp1_core_machine=info` and takes an override.
- Launch under tmux. A `setsid nohup ... &` did not survive the SSH command
  that started it, and r114's run never began.

## If the CUDA build fails on the toolchain

The default is this repository's pin, 1.92.0, and the fork declares
`rust-version = "1.91.1"`. wazabi's 6.10 tree needed **1.97.1** for the CUDA
stack, so if the build stops on a toolchain floor, relaunch with
`TOOLCHAIN=1.97.1`. `pod-run.sh` says so in the failure message. A compile
error in the CUDA feature is an expected gate, not evidence the port is broken:
fix it narrowly, record the fix, relaunch into a fresh `ROOT`.
