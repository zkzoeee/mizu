# Groth16 runs: measured breakdowns

Three Groth16 proofs exist, all on a RunPod **A40** ($0.59/hr, ECC on, 46,068 MiB,
sm_86), all verified, all under **Succinct's released v6.1.0 artifacts** with no
ceremony of our own. Records in `benchmarks/pipeline/groth16-20261112/` and
`benchmarks/pipeline/groth16-poseidon-retained-20261113/`.

| | run A | run B | run C |
| --- | --- | --- | --- |
| date | 2026-11-12 | 2026-11-13 | 2026-11-13 |
| guest | default ELF | Poseidon+Fp precompiles | same |
| `elf_sha256` | `0cfb3eaf..` | `dc8e4b40..` | `dc8e4b40..` |
| cycles | 142,638,298 | **130,367,324** | 130,367,324 |
| Pasta retention | no | **yes** | yes |
| shards | 138 | **84** | 84 |
| VK map | 67 keys | 33 keys | 33 keys |
| network volume | no | yes | yes, fully warm |
| **wall clock** | 9m09.5s | 10m04.9s | **5m50.9s** |
| `prove` | 497.4 s | 547.5 s | **294.5 s** |
| raw proof | 324 B | 324 B | 324 B |

Run C is the steady state: a fresh pod, nothing uploaded, nothing built, nothing
downloaded.

## Where run C's 5m50.9s goes

Timestamps from `/workspace/out/run2/run.log`; the `seconds` block is the
driver's own instrumentation.

| stage | time | note |
| --- | ---: | --- |
| `node_build` | 17.9 s | CUDA context, prover setup |
| `execute` | 26.0 s | guest run for the cycle count |
| `setup` | 7.3 s | program key |
| **`prove`** | **294.5 s** | broken down below |
| `verify` | 0.4 s | SP1 verify of the Groth16 proof |

And inside `prove`:

| sub-stage | time | boundary |
| --- | ---: | --- |
| re-execution | ~45 s | `prove_with_mode` re-executes from the ELF; 00:53:32→00:54:17 |
| STARK core → compress → shrink → wrap | **~111 s** | 00:54:17→00:56:08 |
| artifact check | 0 s | *"already installed"* — the cached path |
| gnark read R1CS | 24.0 s | 2.4 GB, deserialisation not I/O |
| gnark read proving key | **16.0 s** | 5.9 GB from the volume; 3.8-4.0 s from container disk |
| gnark witness | 1.2 s | |
| gnark solver | 25.4 s | 15,972,262 constraints |
| gnark prover | **63.5 s** | the only part ICICLE would accelerate |
| gnark verify | 3.2 ms | the pairing check alone |

`execute` and the re-execution are the same work twice: the driver executes once
for the cycle count, then `prove_with_mode` executes again because it proves from
the ELF rather than from a proof. Dropping `--mode core`'s instrumentation would
save ~26 s.

## What the precompiles and retention bought

Comparing run A with run C, same card, same inputs:

| | run A | run C | |
| --- | ---: | ---: | --- |
| cycles | 142,638,298 | 130,367,324 | **-8.6%**, from the Poseidon+Fp precompiles |
| shards | 138 | 84 | **-39%**, from Pasta retention |
| re-execute + STARK | ~4m07s | ~2m36s | **-37%** |
| distinct shard shapes | 55 | 21 | the number that decides whether another proof fits the map |
| VK map programs | 67 | 33 | |

The guest is built from `benchmarks/dev/pasta-poseidon.json`
(`MIZU_POSEIDON_PRECOMPILE=1`, `MIZU_PASTA_FP_PRECOMPILE=1`, plus 40 ZEKO flags).
Retention is `Uint256MulMod` + `VestaAdd` + `PallasAdd` on top of SP1's five
defaults, applied through `SP1WorkerBuilder::with_core_opts`.

Both are compatible with the ceremony-free property: neither the guest ELF nor
the cluster mix is an input to the wrap verifying key.

## What the network volume bought, and what it costs

Volume `mue9i1oojc`, 30 GB, EU-SE-1, STANDARD, at 19 GB used. Measured
sequentially: **636 MB/s write, 835 MB/s read**, against the container disk's
3.3 GB/s and 9.3 GB/s.

| | cold | warm | saved |
| --- | ---: | ---: | --- |
| source upload + extract | ~4.3 min | 0 | on the volume |
| Go toolchain | ~15 s | 0 | symlinked |
| cargo build | 10m23s | **10.5 s** | the CUDA kernels dominate |
| Groth16 artifacts | 2-4 min | 0 | *see the caveat below* |
| proving-key read | 3.8 s | 16.0 s | **costs +12 s** |

Net: a fresh pod goes from ~10 min to ~5m51s, about **$0.10 to $0.06** a proof.
The +12 s on the key read is real and worth stating — the volume is not free,
it is just much cheaper than the download it replaces. Two-to-four minutes is the
observed range of that download across runs A and B; S3 throughput varied.

The cold build is slower on the volume than off it (10m23s against 7m31s on
container disk) because `CARGO_TARGET_DIR` writes to MooseFS. That is paid once.

## Three more levers, measured on a 2xA40 (2026-11-13)

One pod, 16.15 cores against run C's 7.65, everything else identical. Records
in `benchmarks/pipeline/groth16-vcpu-20261113/`.

| | run C | run A | run B |
| --- | ---: | ---: | ---: |
| GPUs / cores | 1xA40 / 7.65 | 2xA40 / 16.15 | 2xA40 / 16.15 |
| go tags | `debug` | `debug` | none |
| redundant `execute` | yes | yes | no |
| **wall clock** | 5m50.9s | 5m05.8s | **4m39.2s** |
| gnark solver | 25.4 s | 12.9 s | 12.8 s |
| gnark prover | 63.5 s | 31.3 s | 31.5 s |

**Cores halve the CPU-bound stages** -- solver -49%, prover -51%, near linear --
but 2xA40 costs twice the hourly rate for 20% less wall clock, so cost per proof
rises. Latency or cost: pick one. The reads went the other way (R1CS
24.0 -> 27.0 s, key 16.0 -> 20.3 s), which looks like MooseFS variance and was
not isolated.

**The `debug` go tag does nothing.** Solver 12.9 -> 12.8 s, prover
31.3 -> 31.5 s, "Generating proof" 44.28 -> 44.27 s. The debug information that
costs 24 s to read is inside Succinct's released R1CS artifact, so no build tag
reaches it. `SP1_GNARK_GO_TAGS` in the fork stays only because it made this
measurable.

**Dropping the redundant execute saved 26.6 s**, as predicted. `--count-cycles`
now defaults to false in both drivers and the record reports `cycles: null`.

Run A also silently proved with the *previous* binary -- a driver edit failed to
compile and `vol-run.sh` only checked that the binary existed. It now deletes
the binary before building, logs `nproc`, logs the go tags, and falls back to
cgroup v1 for the memory sampler.

## What upstream is about to change, and by how much

succinctlabs/sp1#3017 (opened 2026-10-21, unmerged and unreviewed as of
2026-11-13; main's `gnark-ffi` untouched since v6.10.0) **deletes ICICLE** and
replaces it with Succinct's own BN254 CUDA kernels in Go. Their reason matters
to us: ICICLE's MSM *"gave wrong results for Montgomery-form inputs from about
2^15 points"*, and our circuit is 15,972,262 constraints. The `groth16-cuda`
lead this project was holding would have produced wrong proofs.

The replacement skips the R1CS debug info and the proving key's FFT twiddle
tables, reads both in parallel, decompresses the key's points on the GPU, and
replaces gnark's solver -- which is, stage for stage, our whole gnark bill.

| stage | ours, run C | upstream before | upstream now | our estimate |
| --- | ---: | ---: | ---: | ---: |
| read R1CS | 24.0 s | 11.0 s | 0.77 s | 3-5 s |
| read key + setup | 16.0 s | 2.3 s | 2.2 s | 8-10 s |
| witness | 1.2 s | -- | -- | 1.2 s |
| solve | 25.4 s | 2.6 s | 1.4-1.9 s | 7-12 s |
| prove | 63.5 s | 5.3 s | 0.78 s | 2-3 s |
| **total** | **130.1 s** | 21.6 s | **5.6 s** | **21-31 s** |

Upstream's figures are one run on an RTX 5090 with a Threadripper 7970X, from
the PR body, not reproduced here. The estimates scale them by our core count
for the host-bound stages and by bandwidth for the GPU one; the reads are then
I/O-bound on the volume (2.4 GB and 5.9 GB at 835 MB/s are floors of 2.9 s and
7.1 s).

That is about **-105 s of a ~325 s proof, -32%**, and at the 1xA40 rate roughly
**$0.017 a proof** -- a latency lever, not a cost one. It changes the prover
only, not the circuit or the keys, so the no-ceremony result, the v6.1.0
artifacts and `vk_root` all survive untouched.

Two consequences for when it lands. The reads become the largest part of the
gnark step and they are I/O-bound, so **the artifacts belong on container disk**
(9.3 GB/s) rather than the volume (835 MB/s), reversing the trade above. And
**extra vCPU stops paying**: it buys 45 s today and would buy about 5 s after.
What is left is STARK plus re-execution at 156 s, 71% of the remaining proof.

Their later proofs run 2.8 s against 5.6 s for the first, because circuit and
key stay resident. We prove once per pod and so always pay the first-proof
price; a batching daemon is worth ~40 s with or without CUDA, and becomes the
larger lever after it.

User decision 2026-11-13: revisit when #3017 merges.

## Gotchas, each paid for once

**1. The artifacts cannot be installed by SP1 onto network storage.** Its
installer runs `tar -Pxzf` with no `--no-same-owner`
(`prover/src/build.rs:215`), and the tarball is packed on macOS — owner
`fakedev9999:wheel`, uid 501 gid 0, with `._` AppleDouble entries and
`com.apple.provenance` xattrs. The overlay disk permits that `chown`; MooseFS
refuses it, the extract fails, and SP1 reports **"artifact not found"**.

`circuit_artifacts_installed` (`build.rs:639`) checks only for the `.complete`
marker, so extracting it ourselves with `--no-same-owner` and touching that file
makes SP1 skip the download. `install-artifacts.sh` does this: 207 s to download
6,211,807,514 bytes, 94 s to extract 7.9 GB, once per volume.

There is no alternative tarball. The bucket denies listing, and v6.1.0 is what
`SP1_CIRCUIT_VERSION` pins — another version would be another wrap circuit.

**2. The VK map must be built where the proof runs.** `vk-map-local` predicted 24
distinct shapes from 89 records; the pod produced 21 from 84 shards. It had 21 of
the pod's 27 indices and **missed 6**, all the same preprocessed multiple in
clusters it had identified correctly — so it gets the chip mix right and the exact
shard cut wrong. The proof failed with **`vk not allowed`** at the membership
check, loudly rather than silently.

On the pod `--mode vk-map` costs **11 s**; locally it took 10m38s. So
`vk-map-local`'s remaining value is choosing the guest and retention
configuration, and predicting map size, without renting anything.

**3. MooseFS is fast sequentially and slow on many small files.** Extracting the
1.4 GB source (thousands of files) ran at ~7 MB/s; large files run at 636 MB/s.
Do not generalise from the first number — it is what made me wrongly expect the
artifacts to be unusable from the volume.

**4. `vk_root` changes whenever the map changes.** Run A's was `0x0022bcaf..`,
runs B and C `0x0052a857..`. It is a Groth16 public input and the verifier
contract pins it as `VK_ROOT()`, so the map has to be settled before deployment.

**5. Two Groth16 proofs of the same input differ.** Run B and run C produced
different bytes and both verify: gnark's Groth16 proving is randomised.

## Still open

- **Nothing is deployed.** Our `vk_root` differs from Succinct's, so their
  deployed verifier would reject these proofs. `SP1VerifierGroth16.sol` has to be
  deployed with our `VK_ROOT()`; `Groth16Verifier.sol`, the pairing constants, is
  unchanged. Gas is still the ~250,000 estimate, not measured on chain.
- **One input, one map.** Only this Pickles proof has been proved. A
  differently-sharding input needs the map extended, which changes `vk_root`.
  An ELF-scoped map over the 12 clusters this guest reaches is ~2,280 programs,
  about 11 min and $0.11 on an A40.
- **Card class is load-bearing.** The map is valid only for a prover sharding
  identically — VRAM above ~26 GB. A 24 GB A5000/3090/4090 takes
  `local_gpu_opts`' reduced `element_threshold` and would fail the membership
  check.
- **GPU gnark is parked, not open.** `groth16-cuda` is a dead end: ICICLE is
  removed upstream and was wrong above 2^15 MSM points. Its replacement,
  succinctlabs/sp1#3017, is unmerged; the section above costs it out.
- **Volume headroom.** 19 GB of 30, with `cargo-registry` at 6.0 GB. A second
  guest ELF plus another artifact set would not fit.
