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

## The ELF-scoped map, and what it takes

`--mode vk-map` covers the shapes one input happened to produce. The map that
outlives the input covers a whole **preprocessed block**, and
`shape-space-audit` (pipeline-runner bin, 2.7 s) establishes why that is enough:

```
[1] fixed tail           OK  last 6 are Compose(1..=4), Deferred, Shrink
[2] enumeration order    OK  33 contiguous blocks, cluster fastest, main next
[3] block sizes          OK  main multiple runs 1..=193-pp for T = 193
[4] closed form          OK  checked against all 249744 entries
[5] fuzz                 OK  200000 round trips, seed 0
[6] vk tree depth        OK  249750 distinct (0 duplicates) -> depth 18
```

A shard's shape is (preprocessed multiple, main multiple, padding columns,
cluster). The padding dimensions collapse to one value each, because
`(1 << 21).div_ceil(1 << 22)` is 1. The preprocessed multiple is fixed by the
*program*. So for one ELF only the cluster and the main multiple vary with the
input, and enumerating both exhaustively covers every input that ELF can take.

For our guest that block is **pp = 3, indices 16469..24639, 8,170 shapes**, main
multiples 1..=190 — the whole legal range, which the 2026-11-12 map already
spans end to end. `--mode vk-map-block` builds it with no proving at all.

> **It costs far more than the ~39 min / ~$0.38 first estimated here, and a
> single pod cannot do it.** Measured 2026-11-13 by the `--sample 500` probe:
> **1.1 s/program, not 0.285** (that figure came from 67 *real* shard shapes;
> a block sample reaches main multiples up to 190, whose programs are much
> bigger), so the block is **~2.5 hours**. And `build_vks` accumulates every
> built vk in the local in-memory artifact client before merging, so RSS grows
> with program count: the probe was **OOM-killed (exit 137) at 126 of 506
> programs** inside a 50 GB container. A block map needs a disk-backed artifact
> store or a sliced build with offline merging. Not attempted.

Measured on the Poseidon+retention ELF (`vk-map-local --shapes-only`, 95 s on
this Mac, no vks built; records in
`benchmarks/pipeline/elf-shape-census-20261113/`):

| | |
| --- | --- |
| preprocessed blocks reached | **{3}** — one, as the theory requires |
| raw preprocessed area | **4,722,688 cells, identical in all 89 records** |
| block ceiling (3 × 2^21) | 6,291,456 cells |
| headroom | **1,568,768 cells — 75.1% of the block used** |
| main multiples seen | 1..190 of 17 distinct |
| clusters reached | **10 of 43**: 0, 9, 23, 24, 35-38, 40, 41 |

The constant raw area across every shard is the direct evidence that the
preprocessed multiple is a property of the program, not the input. The 10
clusters correct an earlier figure of 12 in this document.

**So: build block 3.** On the headroom, the guest can grow by a third before it
crosses into pp = 4, and shrink by 528,384 cells before dropping to pp = 2.
Covering **3 and 4 together** is 16,303 programs, ~78 min and ~$0.76, and buys
a root that survives a guest rebuild that crosses the boundary — cheap against
redeploying the verifier contract. Block 2 as well would be 24,516 programs,
~116 min, ~$1.15.

Depth stays 18 either way: `from_map` pads to the full 249,750-entry space, so
the key count moves the root, not the height. An all-cluster map also makes the
card-class caveat go away — a 24 GB card's smaller shards still land in this
block.

### Before building the one we deploy against

1. **The tail was 6 too wide** until 2026-11-13: the tools took `len()-12..`,
   where only the last 6 are Compose/Deferred/Shrink. The other 6 were
   arbitrary `pp=33` Normalize shapes, built, never used, and in the root.
   Fixed at all three sites; check [1] above guards it.
2. **Confirm the guest's block and its headroom** with
   `vk-map-local --shapes-only`, which reports the raw preprocessed area
   against the block ceiling. A guest that grows past it moves to pp = 4
   (another 8,127 shapes) and the map is dead.
3. **Decide bucket 3 alone, or 3 and 4** on that headroom.
4. **Run `vk-map-guard` first.** Seconds, and it fails if the cluster set has
   drifted off depth 18 — cheaper than discovering it after 39 minutes.
5. **Sample before committing.** `build_vks` has no resume: the controller
   submits every chunk up front and holds each one's artifacts until the end
   (`worker/controller/vk_tree.rs:95`), so a failure at 90% loses everything.
   `--sample n` builds an even spread including both ends of the block.
6. **Decide the panic policy.** `build_vk_map` *collects* shapes that panic
   rather than failing (`worker/prover/vk_worker.rs:39`). Both drivers now
   refuse by default and record the indices; `--allow-panics true` accepts a
   map without them, whose root differs from a clean build's and whose missing
   shapes can never be proved.

A local smoke test built both ends of the block — main multiple 1 and 190,
the shapes least likely to have been built before — in 100 s on CPU at 12.49
s/program, with no panics.

## Two workloads, one root, verified on the EVM (2026-11-13)

With the whole block out of reach, the ELF-scoped claim was tested at the scale
that was affordable: build a map for each of two deliberately different
workloads, **merge them offline into one map**, and prove both against it.

| | proof A | proof B |
| --- | --- | --- |
| guest ELF | `dc8e4b40..` Poseidon + Fp precompiles | `0cfb3eaf..` default |
| retention | on | **off** |
| shards | 84 | **138** |
| distinct shard shapes | 21 | **55** |
| own map | 27 keys | 61 keys |
| `prove` | 316.6 s | 368.1 s |
| `programVKey` | `0x00f23c18bd..` | `0x00416e4a23..` — different |
| **`vk_root`** | `0x007ef473..ae0` | **identical** |

The merged map is **75 keys**: 27 + 61 with 13 shared — the 6 fixed programs
plus 7 shard shapes both workloads genuinely reach, which matches the census
(both ELFs hit clusters 0, 9, 23, 24).

Merging offline is sound rather than a shortcut. The controller merges chunk
outputs by collecting every vk into a `BTreeSet` and `.enumerate()`-ing it
(`worker/controller/vk_tree.rs`), and `from_map` derives the root from the map
alone, so a union assembled with `tools/merge-vkmap.py` is byte-identical to
what one build over that union would produce. The tool checks its own
assumptions: every input map must already be sorted under its comparator with
values exactly 0..n-1, and it round-trips the bytes. **Both proofs verifying
against the merged map is the end-to-end evidence that it is right.**

So a map is not tied to one input, and not even to one program: what it is tied
to is the set of shard shapes it covers. Two programs sharding two different
ways share one root and so one deployed verifier constant.

## Verified locally with forge

`contracts-check/` runs Succinct's released v6.1.0 verifier contracts —
`SP1VerifierGroth16.sol` and `Groth16Verifier.sol`, taken from the installed
artifacts — against both proofs. The **only** edit is `VK_ROOT()`:

```
0x002f850ee998974d6cc00e50cd0814b098c05bfade466d28573240d057f25352   upstream
0x007ef473fdfbe096cc4e1e3235be51f83c613ca77f431564b062220769ff8ae0   ours
```

`forge test` — 8 passed, 0 failed, no `forge-std` dependency:

| | |
| --- | --- |
| `verifyProof(bytes32,bytes,bytes)` | **219,096 gas** (both proofs) |
| deployment | 1,510,757 gas, 6,770 bytes |
| a flipped bit in the `vkRoot` word | rejected at 9,923 gas (`InvalidVkRoot`, before the pairing) |
| a flipped bit in the proof | rejected by the pairing |

219,096 against the ~250,000 this document used to estimate. `publicValues` is
the single byte `0x01` — the guest's boolean — confirmed by
`sha256(0x01) & ((1<<253)-1)` matching public input 1 of both proofs.

Still not deployed to any chain, and the gas figure is a local EVM measurement.

## The block-3 map, built and tested (2026-11-13)

Built on an **RTX A5000** at $0.27/hr — less than half the A40's rate, and the
A40 was out of stock anyway. Records in
`benchmarks/pipeline/block3-map-20261113/`.

**The cost estimate in this document was wrong twice.** The A40 probe reported
1.1 s/program and OOMed; that was a process slowing down as it approached the
50 GB ceiling, not the block's real rate. Built in bounded slices the rate is
**0.31-0.33 s/program**, measured at both ends of the block (slice 0, main
multiples 1-2; slice 102, main multiples ~189-190) — they differ by 5%, so the
big-main-multiple programs are *not* more expensive, which is the opposite of
what I assumed when sizing the slices.

| | |
| --- | --- |
| slices | 103 x ~80 programs, one process each |
| rate | 0.31-0.33 s/program |
| peak RSS | 23 GiB (30 GiB once the artifact download filled the page cache) |
| wall clock | 1h21m |
| cost | **~$0.37** |
| result | **8,176 keys**, `sha256 51d9a371..`, root `0x00ec8a08..e835` |

### What was checked, and what each check would have caught

| check | catches |
| --- | --- |
| slices tile 0..8,170, no gap or overlap | a missing or double-counted slice |
| exactly 8,176 keys | a silently dropped shape, or two shapes colliding to one digest |
| `panic_indices` empty on every slice | a shape that failed to build |
| 56 spot-rebuilt keys all present | a slice that built *something else* than its indices say |
| a proof verifies against it | `from_map` and the membership check agree |
| the same workload against `mapB` fails | that the membership check is live at all |

The last one matters most: it failed in 39 s with
`task failed with fatal error: vk not allowed`, which is what makes every
positive result above non-vacuous.

### The map covers a guest it was never built for

The proof against the block map is from the **simplechain** guest — built
against a *different Mina wrap VK* (`fixtures/simplechain/wrap0`), proving a
*different Pickles proof*, on a 24 GB A5000 whose reduced `element_threshold`
shards differently from the A40 the map was conceived on.

| | mainnet guest | simplechain guest |
| --- | ---: | ---: |
| wrap VK | mainnet-blockchain-snark | simplechain/wrap0 |
| ELF | `dc8e4b40..` | `51241c13..` |
| cycles | 130,367,324 | **130,307,322** |
| `programVKey` | `0x00f23c18bd..` | `0x00f8491590..` |
| verified against `block3.bin` | — | **yes**, 315.1 s |

Those cycle counts are 0.05% apart. The guest's size and shape are set by the
42 `ZEKO_*`/`MIZU_*` flags, not by which VK is baked in — which also corrects an
earlier scare in this document: a flagless simplechain build came out at 12.2 MB
against the mainnet guest's 26.4 MB, and I read that as the VK's circuit size
when it was the flags. Rebuilt with the flags, the simplechain guest is
26.4 MB and lands in the same preprocessed block.

**So one deployed root serves a family of guests**, which is what the block map
was for. `contracts-check` now verifies that proof on a local EVM with
`VK_ROOT()` set to the block-3 root: 7 tests pass, `verifyProof` **226,005 gas**,
and the earlier union-map proof is *rejected* by the same contract — the root
binds.

### Traps found on the way

**`make build-rust VK_JSON=... ` silently does nothing from a warm tree.**
`crates/o1-verifier-host/build.rs` declared no `rerun-if-env-changed`, so cargo
reran it only when the host crate's own sources changed. Changing `VK_JSON` or
the 42 flags never invalidated it, `sp1_build` was never called, and the build
exited 0 in 1.6 s leaving the previous guest in place — a guest against the
wrong VK, with no error anywhere. The Makefile advertises that override, so the
documented path was the broken one. Fixed: `crates/o1-verifier/build-env.rs`
now lists the 54 variables that change the guest, and both build scripts
declare them.

**An ELF uploaded four minutes after something overwrote it is not the ELF you
think.** The first verification run proved a *panicking* guest fed mismatched
input; the membership check still passed, which was accidental evidence that the
map covers a foreign program, but it was not a valid proof and the run was
redone.

## statement-digest, and proving from a saved wrap proof (2026-11-13)

Two changes that together make a proof mean something on chain and make the
gnark half of the pipeline runnable without a GPU. Records in
`benchmarks/pipeline/statement-digest-20261113/`.

### The ABI is effectively free

`--features statement-digest` makes the guest commit
`keccak("mizu/verified-statement/v1" || vk_id || len || app_state)` instead of a
bare boolean, so the public values say **which** Mina VK verified **which**
statement. The boolean ABI says only that *some* Pickles proof verified, which
is close to meaningless to a contract.

| | boolean | statement-digest |
| --- | ---: | ---: |
| cycles | 130,367,324 | **130,374,139** (+6,815, +0.005%) |
| raw preprocessed area | 4,722,688 | 4,732,928 (+0.2%) |
| preprocessed block | 3 | **3** |
| clusters reached | 10 of 43 | the same 10 |
| distinct shard shapes | 21 | 24 |

Same block, same clusters, so **the block-3 map covers it and `vk_root` does
not move** — no re-map, no redeploy. Build it with
`make build-guest GUEST_CONFIG=benchmarks/dev/pasta-poseidon-digest.json`
(33 s).

### `--mode wrap` / `--mode from-wrap`

The Groth16 circuit reads nothing but the wrap proof, so the pipeline splits:

| | where | time |
| --- | --- | ---: |
| `--mode wrap` (execute, core, reduce, shrink, wrap) | GPU | 205 s |
| wrap proof | 1,340,949 bytes | |
| `--mode from-wrap` (gnark only) | **CPU, no GPU** | **133 s** |

`groth16_from_wrap` (`sp1_prover::build`) is `run_groth16`'s body as a free
function taking the wrap proof directly — no node, no artifact client, no
prover setup. It verifies before returning, so a wrap proof that does not match
the installed artifacts fails there rather than on chain.

**Peak memory 31.4 GiB.** The 4090 pod this wrap proof was made on has a 31.0 GB
cgroup limit, so from-wrap would have OOMed on it by 0.4 GiB. A from-wrap box
needs **40 GB or more**, which rules out the cheapest CPU flavours. (The figure
is `memory.current`, so it counts the page cache of 8 GB of artifacts as well as
the process.)

### Verified on the EVM, with no artifacts anywhere

`contracts-check` verifies both ABIs against one contract: **11 tests pass**,
`verifyProof` **225,922-226,005 gas**, on a machine with no `~/.sp1` at all.
On-chain verification needs the *verifying* key, which is already compiled into
`Groth16Verifier.sol`; the 7.9 GB is the proving key and the R1CS, needed only
to *make* a proof. The whole verification surface is 30,987 bytes of Solidity.

The suite also checks the digest is the right one: the contract's own
`hashPublicValues` reproduces public input 1 from the pinned mainnet fixture's
expected statement `837d502c..`, so the guest committed the statement it should
have.

## Latency: what is actually left, and one lever taken (2026-11-14)

Audited the two levers this project had been quoting for the post-CUDA proof.
**One was already harvested and the other was mis-sized.**

### The re-execution is not duplication, and the duplicate is already gone

The claim was "~45 s of pure duplication: the driver executes once for the cycle
count and `prove_with_mode` re-executes from the ELF". Half right, and the wrong
half. There were two executions; the removable one was the **driver's own**, at
**26.0 s** of run C's 350.9 s, and it was removed — `count_cycles` now defaults
to `false` in both drivers and its doc comment says why. The **~45 s inside
`prove_with_mode` is the execution that generates the trace**, so it is the
proof's own work and not available to drop. Nothing is left on this lever.

### The resident-process saving is real, already implemented in Go, and unused

Both gnark paths keep the circuit and the proving key for the life of the
process, so the artifact reads are a **per-process** cost, not a per-proof one:

| path | state | where |
| --- | --- | --- |
| CPU gnark | `globalR1cs`, `globalPk` behind `globalMutex` | `go/sp1/prove_groth16.go:19-23` |
| CUDA gnark | `groth16CudaProvers` keyed by `dataDir` | `go/sp1/prove_groth16_cuda.go`, whose comment says it "keeps the circuit and its proving key in host memory for the life of the process" |

So no Go change is needed. What was missing is a driver that proves **more than
once in one process** — `--mode from-wrap` took a single `--wrap-in` and exited,
so the cache was never reused.

`--wrap-in` is now repeatable. One process, N wrap proofs, and the record reports
`groth16_first`, `groth16_rest_mean` and `saved_per_proof_after_the_first`, so a
single run measures the lever instead of inferring it:

```sh
pipeline-runner --mode from-wrap --wrap-in a.bin --wrap-in b.bin --wrap-in c.bin \
  --output fromwrap-x3.json
```

A one-proof run keeps the exact record shape every earlier from-wrap run used, so
those records stay comparable.

### What it is worth, and the correction to "~40 s"

**40.0 s is the CPU-gnark figure** — R1CS 24.0 s plus proving key 16.0 s of that
path's 130.1 s. It is not the figure after CUDA. #3017's reader skips the R1CS
debug info and the key's FFT twiddle tables and reads both in parallel, and the
whole CUDA gnark half measured **35.6 s**, so the cacheable part cannot exceed
that and is likely **~10-13 s** on the estimates in the table above. The CUDA
run's internal stage split was not recorded, so this is bounded, not measured.
Any multi-proof run should print it.

### Where the remaining time actually is

After CUDA gnark the proof is roughly 205 s of wrap and 37 s of gnark, so **the
wrap half is about 85% of what is left** and it is GPU STARK work: execution,
core proving over 84 shards, the reduce tree, shrink and wrap. The levers there
are fewer cycles, fewer shards, or more card — not driver plumbing.

One more per-process cost is visible in run C and **not** addressed here:
`node_build` at 17.9 s (CUDA context and prover setup) and `setup` at 7.3 s.
`--mode from-wrap` pays neither, because it builds no node. A multi-proof
*full* pipeline would amortise both, worth a further ~25 s per extra proof, but
it needs several ELF/input pairs and the node held open across them — a larger
change than this one, and only worth making if more than one proof per pod is
actually the plan.

### Unverified

The repeatable `--wrap-in` type-checks locally; **it has not been run.** From-wrap
needs the 7.9 GB of Groth16 artifacts, so measuring it needs a pod. Until then
the amortisation is a property of the Go globals read from source, not a figure
of ours.

## Still open

- **Nothing is deployed.** Our `vk_root` differs from Succinct's, so their
  deployed verifier would reject these proofs. `SP1VerifierGroth16.sol` has to be
  deployed with our `VK_ROOT()`; `Groth16Verifier.sol`, the pairing constants, is
  unchanged. Verified locally at **219,096 gas** (see above); never on a chain.
- ~~**One input, one map.**~~ Settled: the block-3 map covers two different
  guests against two different Mina VKs, on two cards that shard differently.
- **Card class is load-bearing.** The map is valid only for a prover sharding
  identically — VRAM above ~26 GB. A 24 GB A5000/3090/4090 takes
  `local_gpu_opts`' reduced `element_threshold` and would fail the membership
  check.
- **GPU gnark is parked, not open.** `groth16-cuda` is a dead end: ICICLE is
  removed upstream and was wrong above 2^15 MSM points. Its replacement,
  succinctlabs/sp1#3017, is unmerged; the section above costs it out.
- **Volume headroom.** 19 GB of 30, with `cargo-registry` at 6.0 GB. A second
  guest ELF plus another artifact set would not fit.
